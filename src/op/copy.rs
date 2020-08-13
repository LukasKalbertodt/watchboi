use serde::Deserialize;
use crate::{
    Context,
    prelude::*,
};
use super::{Operation, RunningOperation};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copy {
    src: String,
    dst: String,
}

impl Copy {
    pub const KEYWORD: &'static str = "copy";
}

impl Operation for Copy {
    fn keyword(&self) -> &'static str {
        Self::KEYWORD
    }

    fn dyn_clone(&self) -> Box<dyn Operation> {
        Box::new(self.clone())
    }

    fn start(&self, _ctx: &Context) -> Result<Box<dyn RunningOperation>> {
        todo!()
    }
}
