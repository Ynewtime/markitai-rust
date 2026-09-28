//! A probe uses the conversion protocol but not its retry or fallback router.
use super::{Deployment, Prompts, Protocol, deployments, payload};
use crate::{Error, Result, provider_management};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::{collections::HashMap, io::Read, time::Duration};
const MAX_RESPONSE: u64 = 1024 * 1024;

pub(crate) fn probe(request: &Value) -> Result<()> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let model = request["model"]
        .as_str()
        .ok_or_else(|| Error::InvalidInput("Model is required".into()))?;
    let mut params = json!({"model":model,"max_tokens":16,"weight":1});
    for name in ["api_key", "api_base"] {
        if let Some(value) = request.get(name) {
            params[name] = value.clone();
        }
    }
    // The native Azure protocol needs an explicit version, including probes.
    if model.starts_with("azure/") {
        params["api_version"] = env
            .get("AZURE_API_VERSION")
            .map_or("2024-10-21", String::as_str)
            .into();
    }
    let cfg = json!({"llm":{"model_list":[{"model_name":"default","litellm_params":params}]}});
    let mut entries = deployments(&cfg, &env)?;
    if entries.len() != 1 {
        return Err(Error::Config(
            "Connection test requires exactly one deployment".into(),
        ));
    }
    let entry = entries.remove(0);
    provider_management::checked_url(&entry.endpoint)?;
    perform(&entry)
}
fn perform(entry: &Deployment) -> Result<()> {
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| failure("Cannot create model connection client"))?;
    let prompts = Prompts {
        system: String::new(),
        user: "Reply with exactly OK.".into(),
        image: None,
        cache_scope: String::new(),
    };
    let mut body = payload(entry, &prompts);
    // The shared payload supplies the provider's token field and content shape.
    // Probes have no conversion system prompt at all.
    if entry.protocol == Protocol::Anthropic {
        body.as_object_mut()
            .expect("payload object")
            .remove("system");
    } else if let Some(messages) = body["messages"].as_array_mut() {
        messages.retain(|message| message["role"] != "system");
    }
    let mut call = client
        .post(&entry.endpoint)
        .header("accept", "application/json");
    if let Some(key) = &entry.key {
        call = match entry.protocol {
            Protocol::Anthropic => call.header("x-api-key", key),
            Protocol::Azure => call.header("api-key", key),
            Protocol::Chat => call.bearer_auth(key),
        };
    }
    if entry.protocol == Protocol::Anthropic {
        call = call.header("anthropic-version", "2023-06-01");
    }
    let response = call.json(&body).send().map_err(|error| {
        failure(if error.is_timeout() {
            "Model connection test timed out"
        } else {
            "Model connection request failed"
        })
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(failure(&format!(
            "Model connection returned HTTP {}",
            status.as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE)
    {
        return Err(failure("Model connection response exceeds 1 MiB"));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure("Cannot read model connection response"))?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(failure("Model connection response exceeds 1 MiB"));
    }
    let data: Value = serde_json::from_slice(&bytes)
        .map_err(|_| failure("Model connection response is not valid JSON"))?;
    let complete = if entry.protocol == Protocol::Anthropic {
        data.get("content").and_then(Value::as_array).is_some()
    } else {
        data.get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                !choices.is_empty()
                    && choices
                        .iter()
                        .any(|choice| choice.get("message").is_some_and(Value::is_object))
            })
    };
    if !complete || data.get("error").is_some_and(|value| !value.is_null()) {
        return Err(failure("Model connection returned an invalid completion"));
    }
    Ok(())
}
fn failure(message: &str) -> Error {
    Error::Conversion(message.into())
}
