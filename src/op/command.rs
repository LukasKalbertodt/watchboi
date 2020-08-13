use std::{
    fmt,
    convert::TryFrom,
};
use serde::Deserialize;
use crate::{
    Context,
    prelude::*,
};
use super::{Operation, Outcome, RunningOperation};

#[derive(Debug, Clone, Deserialize)]
pub struct Command {
    run: ProgramAndArgs,

    /// What working directory to execute the command in.
    workdir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawProgramAndArgs")]
struct ProgramAndArgs {
    /// The command to run.
    program: String,

    /// Arguments for the command.
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawProgramAndArgs {
    Simple(String),
    Explicit(Vec<String>),
}

impl TryFrom<RawProgramAndArgs> for ProgramAndArgs {
    type Error = String;

    fn try_from(src: RawProgramAndArgs) -> Result<Self, Self::Error> {
        match src {
            RawProgramAndArgs::Simple(s) => {
                if s.is_empty() || s.chars().all(|c| c.is_whitespace()) {
                    return Err("command string is empty".into());
                }

                let mut split = s.split_whitespace();
                let program = split.next().unwrap().to_owned(); // checked above
                let args: Vec<_> = split.map(|s| s.to_owned()).collect();

                Ok(Self { program, args })
            }
            RawProgramAndArgs::Explicit(v) => {
                if v.is_empty() {
                    return Err("empty list as command specification".into());
                }

                if v.iter().any(|f| f.is_empty() || f.chars().all(|c| c.is_whitespace())) {
                    return Err("empty fragment in command specification".into());
                }

                let program = v[0].clone();
                let args = v.into_iter().skip(1).collect();

                Ok(Self { program, args })
            }
        }
    }
}

impl fmt::Display for ProgramAndArgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let print = |f: &mut fmt::Formatter, s: &str| {
            if s.contains(char::is_whitespace) {
                write!(f, r#""{}""#, s)
            } else {
                write!(f, "{}", s)
            }
        };


        print(f, &self.program)?;
        for arg in &self.args {
            write!(f, " ")?;
            print(f, arg)?;
        }

        Ok(())
    }
}


impl From<ProgramAndArgs> for Command {
    fn from(src: ProgramAndArgs) -> Self {
        Self {
            run: src,
            workdir: None,
        }
    }
}


impl Command {
    pub const KEYWORD: &'static str = "command";

    pub fn from_simple(s: &str) -> Result<Self, String> {
        Ok(ProgramAndArgs::try_from(RawProgramAndArgs::Simple(s.into()))?.into())
    }

    pub fn from_explicit(v: Vec<String>) -> Result<Self, String> {
        Ok(ProgramAndArgs::try_from(RawProgramAndArgs::Explicit(v))?.into())
    }
}

impl Operation for Command {
    fn keyword(&self) -> &'static str {
        Self::KEYWORD
    }

    fn dyn_clone(&self) -> Box<dyn Operation> {
        Box::new(self.clone())
    }

    fn start(&self, ctx: &Context) -> Result<Box<dyn RunningOperation + '_>> {
        msg!(run [ctx]["command"] "running: {[green]}", self.run);

        // Build `std::process::Command`.
        let mut command = std::process::Command::new(&self.run.program);
        command.args(&self.run.args);

        let workdir = match &self.workdir {
            Some(workdir) => ctx.join_workdir(&workdir),
            None => ctx.workdir(),
        };
        command.current_dir(workdir);

        // Run the command and get its status code
        match command.spawn() {
            Ok(child) => Ok(Box::new(RunningCommand { child, config: self })),
            Err(e) => {
                let mut context = format!("failed to spawn `{}`", self.run);
                if e.kind() == std::io::ErrorKind::NotFound {
                    context += &format!(
                        " (you probably don't have the command '{}' installed)",
                        self.run.program,
                    );
                }
                Err(e).context(context)
            }
        }
    }
}

struct RunningCommand<'a> {
    child: std::process::Child,
    config: &'a Command,
}

impl RunningCommand<'_> {
    fn finish_with_status(&self, status: std::process::ExitStatus, ctx: &Context) -> Outcome {
        if status.success() {
            Outcome::Success
        } else {
            msg!(warn [ctx]["command"] "{[green]} returned non-zero exit code", self.config.run);
            Outcome::Failure
        }
    }
}


impl RunningOperation for RunningCommand<'_> {
    fn finish(&mut self, ctx: &Context) -> Result<Outcome> {
        let status = self.child.wait().context("failed to wait for running process")?;
        Ok(self.finish_with_status(status, ctx))
    }
    fn try_finish(&mut self, ctx: &Context) -> Result<Option<Outcome>> {
        let status = self.child.try_wait().context("failed to wait for running process")?;
        Ok(status.map(|status| self.finish_with_status(status, ctx)))
    }
    fn cancel(&mut self) -> Result<()> {
        self.child.kill()?;
        Ok(())
    }
}
