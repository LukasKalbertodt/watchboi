use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{mpsc::{Receiver, Sender}, Arc, Mutex},
    thread, time::{Duration, Instant},
};
use anyhow::{bail, Error, Result};
use hyper::{
    Body, Client, Request, Response, Server, Uri, StatusCode,
    header,
    service::{make_service_fn, service_fn}
};
use tungstenite::WebSocket;

use crate::{
    config,
    ui::Ui,
};


pub fn run(
    config: &config::Http,
    ui: Ui,
    errors_tx: Sender<Error>,
    refresh: Receiver<()>,
) -> Result<()> {
    {
        let config = config.clone();
        let ui = ui.clone();
        let errors_tx = errors_tx.clone();
        thread::spawn(move || {
            if let Err(e) = run_server(&config, ui) {
                let _ = errors_tx.send(e);
            }
        });
    }

    if config.auto_reload() {
        let config = config.clone();
        let ui = ui.clone();
        let errors_tx = errors_tx.clone();
        thread::spawn(move || {
            if let Err(e) = serve_ws(&config, ui, refresh) {
                let _ = errors_tx.send(e);
            }
        });
    }

    Ok(())
}

#[tokio::main]
pub async fn run_server(config: &config::Http, ui: Ui) -> Result<()> {
    let addr = config.addr();
    let ws_addr = config.ws_addr();

    let service = if let Some(proxy_target) = config.proxy {
        let auto_reload = config.auto_reload();

        make_service_fn(move |_| {
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    proxy(req, proxy_target, ws_addr, auto_reload)
                }))
            }
        })
    } else {
        bail!("bug: invalid http config");
    };

    let server = Server::bind(&addr).serve(service);
    ui.listening(&addr);

    server.await?;

    Ok(())
}


async fn proxy(
    mut req: Request<Body>,
    target: SocketAddr,
    ws_addr: SocketAddr,
    auto_reload: bool,
) -> Result<Response<Body>> {
    let uri = Uri::builder()
        .scheme("http")
        .authority(target.to_string().as_str())
        .path_and_query(req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or(""))
        .build()
        .expect("failed to build URI");
    *req.uri_mut() = uri.clone();

    let client = Client::new();
    let response = match client.request(req).await {
        Ok(response) if !auto_reload => response,
        Ok(response) => {
            let content_type = response.headers().get(header::CONTENT_TYPE);
            if content_type.is_some() && content_type.unwrap().as_ref().starts_with(b"text/html") {
                let (parts, body) = response.into_parts();
                let body = hyper::body::to_bytes(body).await?;

                let new_body = inject_into(&body, ws_addr);
                let new_len = new_body.len();
                let new_body = Body::from(new_body);

                let mut response = Response::from_parts(parts, new_body);
                if let Some(content_len) = response.headers_mut().get_mut(header::CONTENT_LENGTH) {
                    *content_len = new_len.into();
                }
                response
            } else {
                response
            }
        }
        Err(e) => {
            let msg = format!("failed to reach {}\nError:\n\n{}", uri, e);

            Response::builder()
                // TODO: sometimes this should be 504 GATEWAY TIMEOUT
                .status(StatusCode::BAD_GATEWAY)
                .header("Content-Type", "text/plain")
                .body(msg.into())
                .unwrap()
        }
    };

    Ok(response)
}

fn inject_into(input: &[u8], ws_addr: SocketAddr) -> Vec<u8> {
    let mut body_close_idx = None;
    let mut inside_comment = false;
    for i in 0..input.len() {
        let rest = &input[i..];
        if !inside_comment && rest.starts_with(b"</body>") {
            body_close_idx = Some(i);
        } else if !inside_comment && rest.starts_with(b"<!--") {
            inside_comment = true;
        } else if inside_comment && rest.starts_with(b"-->") {
            inside_comment = false;
        }
    }

    let js = include_str!("inject.js")
        .replace("INSERT_PORT_HERE_KTHXBYE", &ws_addr.port().to_string());
    let script = format!("<script>\n{}</script>", js);

    // If we haven't found a closing body tag, we just insert our JS at the very
    // end.
    let insert_idx = body_close_idx.unwrap_or(input.len());
    let mut out = input[..insert_idx].to_vec();
    out.extend_from_slice(script.as_bytes());
    out.extend_from_slice(&input[insert_idx..]);
    out
}

fn serve_ws(config: &config::Http, ui: Ui, refresh: Receiver<()>) -> Result<()> {
    let sockets = Arc::new(Mutex::new(Vec::<WebSocket<_>>::new()));

    // Start thread that listens for incoming refresh requests.
    {
        let proxy_target = config.proxy;
        let sockets = sockets.clone();
        thread::spawn(move || {
            for _ in refresh {
                if let Some(target) = proxy_target {
                    wait_until_socket_open(target);
                }

                // All connections are closed when the `TcpStream` inside those
                // `WebSocket` is dropped.
                sockets.lock().unwrap().clear();
            }
        });
    }

    // Listen for new WS connections, accept them and push them in the vector.
    let server = TcpListener::bind(config.ws_addr())?;
    ui.listening_ws(&config.ws_addr());
    for stream in server.incoming() {
        let websocket = tungstenite::accept(stream?)?;
        sockets.lock().unwrap().push(websocket);
    }

    Ok(())
}

fn wait_until_socket_open(target: SocketAddr) {
    const POLL_PERIOD: Duration = Duration::from_millis(20);
    const PORT_WAIT_TIMEOUT: Duration = Duration::from_secs(3);

    let start_wait = Instant::now();

    while start_wait.elapsed() < PORT_WAIT_TIMEOUT {
        let before_connect = Instant::now();
        if TcpStream::connect_timeout(&target, POLL_PERIOD).is_ok() {
            return;
        }

        if let Some(remaining) = POLL_PERIOD.checked_sub(before_connect.elapsed()) {
            thread::sleep(remaining);
        }
    }

    println!("WARNING");
}