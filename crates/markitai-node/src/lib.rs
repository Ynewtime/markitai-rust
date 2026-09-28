use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn run(request: &str) -> Result<String> {
    catch_unwind(AssertUnwindSafe(|| markitai_core::convert_json(request)))
        .map_err(|_| Error::from_reason("Native conversion failed unexpectedly"))
}

pub struct ConvertTask {
    request: String,
}

impl Task for ConvertTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        run(&self.request)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

#[napi]
pub fn convert_json(request: String) -> AsyncTask<ConvertTask> {
    AsyncTask::new(ConvertTask { request })
}

#[napi]
pub fn convert_json_sync(request: String) -> Result<String> {
    run(&request)
}

#[napi]
pub fn version() -> &'static str {
    markitai_core::VERSION
}
