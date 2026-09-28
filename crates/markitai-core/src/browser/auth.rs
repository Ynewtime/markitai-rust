//! Challenge-driven HTTP credentials scoped to one explicit web origin.
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::HashSet;
use url::{Origin, Url};

const MAX_SECRET_BYTES: usize = 4096;
const MAX_CHALLENGES: usize = 128;

#[derive(Clone)]
pub(super) struct Credentials {
    origin: Origin,
    username: String,
    password: String,
}

fn invalid() -> Error {
    Error::Config("Browser http_credentials require bounded username/password strings, an optional HTTP(S) origin, and send=unauthorized or always".into())
}

fn web_origin(value: &str) -> Option<Origin> {
    if value.len() > 4096 || value.trim() != value || value.chars().any(char::is_control) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    (["http", "https"].contains(&url.scheme())
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then(|| url.origin())
}

pub(super) fn parse(value: Option<&Value>, initial: &Url) -> Result<Option<Credentials>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let fields = value.as_object().ok_or_else(invalid)?;
    if fields
        .keys()
        .any(|key| !["username", "password", "origin", "send"].contains(&key.as_str()))
    {
        return Err(invalid());
    }
    let secret = |name: &str| {
        fields
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| text.len() <= MAX_SECRET_BYTES && !text.chars().any(char::is_control))
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    let username = secret("username")?;
    let password = secret("password")?;
    if username.contains(':') {
        return Err(invalid());
    }
    let origin = match fields.get("origin") {
        Some(Value::String(value)) => web_origin(value).ok_or_else(invalid)?,
        Some(_) => return Err(invalid()),
        None => initial.origin(),
    };
    if let Some(send) = fields.get("send")
        && !matches!(send.as_str(), Some("unauthorized" | "always"))
    {
        return Err(invalid());
    }
    Ok(Some(Credentials {
        origin,
        username,
        password,
    }))
}

pub(super) struct State {
    credentials: Option<Credentials>,
    attempts: HashSet<String>,
    challenges: usize,
}

impl State {
    pub(super) fn new(credentials: Option<Credentials>) -> Self {
        Self {
            credentials,
            attempts: HashSet::new(),
            challenges: 0,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.credentials.is_some()
    }

    pub(super) fn respond(&mut self, params: &Value) -> Result<Value> {
        self.challenges += 1;
        if self.challenges > MAX_CHALLENGES {
            return Err(Error::Fetch(
                "Browser HTTP authentication challenge limit exceeded".into(),
            ));
        }
        let id = params
            .get("requestId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 4096)
            .ok_or_else(|| Error::Fetch("Malformed Chromium authentication challenge".into()))?;
        let challenge = &params["authChallenge"];
        let request_origin = params
            .pointer("/request/url")
            .and_then(Value::as_str)
            .filter(|url| url.len() <= 16_384)
            .and_then(|value| Url::parse(value).ok())
            .filter(|url| {
                ["http", "https"].contains(&url.scheme())
                    && url.username().is_empty()
                    && url.password().is_none()
            })
            .map(|url| url.origin());
        let challenge_origin = challenge
            .get("origin")
            .and_then(Value::as_str)
            .and_then(web_origin);
        let eligible = self.credentials.as_ref().is_some_and(|credentials| {
            challenge.get("source").and_then(Value::as_str) == Some("Server")
                && challenge
                    .get("scheme")
                    .and_then(Value::as_str)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic"))
                && request_origin.as_ref() == Some(&credentials.origin)
                && challenge_origin.as_ref() == Some(&credentials.origin)
        });
        let answer = if eligible && self.attempts.insert(id.to_owned()) {
            let credentials = self.credentials.as_ref().expect("eligible credentials");
            json!({"response":"ProvideCredentials", "username":credentials.username, "password":credentials.password})
        } else {
            // Cancel rather than falling back to browser/OS credential stores.
            json!({"response":"CancelAuth"})
        };
        Ok(json!({"requestId":id,"authChallengeResponse":answer}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(credentials: Value) -> State {
        State::new(
            parse(
                Some(&credentials),
                &Url::parse("https://example.test/start").unwrap(),
            )
            .unwrap(),
        )
    }
    fn challenge(id: &str, url: &str, origin: &str, source: &str) -> Value {
        json!({"requestId":id,"request":{"url":url},"authChallenge":{"source":source,"origin":origin,"scheme":"basic","realm":"test realm"}})
    }
    fn response(state: &mut State, value: Value) -> Value {
        state.respond(&value).unwrap()["authChallengeResponse"].clone()
    }
    #[test]
    fn absent_and_null_differ_from_empty_or_invalid_credentials() {
        let url = Url::parse("https://example.test/").unwrap();
        assert!(parse(None, &url).unwrap().is_none());
        assert!(parse(Some(&Value::Null), &url).unwrap().is_none());
        assert!(parse(Some(&json!({"username":"","password":""})), &url).is_ok());
        for value in [
            json!({}),
            json!([]),
            json!({"username":"hidden-user","password":12}),
            json!({"username":"hidden-user","password":"secret","other":"secret"}),
            json!({"username":"hidden-user","password":"secret","origin":"https://example.test/private"}),
            json!({"username":"hidden-user","password":"secret","origin":"https://user:secret@example.test"}),
            json!({"username":"hidden-user","password":"secret","send":"invalid"}),
            json!({"username":"hidden-user","password":"secret\n"}),
            json!({"username":"a:b","password":"secret"}),
            json!({"username":"hidden-user","password":"x".repeat(MAX_SECRET_BYTES+1)}),
        ] {
            let error = parse(Some(&value), &url).err().unwrap().to_string();
            assert!(!error.contains("hidden-user") && !error.contains("secret"));
        }
    }
    #[test]
    fn exact_default_origin_allows_subpaths_but_not_redirected_ports_or_hosts() {
        let mut auth = state(json!({"username":"u","password":"p"}));
        assert_eq!(
            response(
                &mut auth,
                challenge(
                    "a",
                    "https://example.test:443/other",
                    "https://example.test",
                    "Server"
                )
            )["response"],
            "ProvideCredentials"
        );
        for (index, url, origin) in [
            (0, "http://example.test/a", "http://example.test"),
            (1, "https://example.test:444/a", "https://example.test:444"),
            (2, "https://sub.example.test/a", "https://sub.example.test"),
            (3, "https://other.test/a", "https://example.test"),
        ] {
            let answer = response(
                &mut auth,
                challenge(&format!("other-{index}"), url, origin, "Server"),
            );
            assert_eq!(answer, json!({"response":"CancelAuth"}));
        }
    }
    #[test]
    fn explicit_origin_is_independent_of_initial_url_and_send_is_not_preemptive() {
        let mut auth = state(
            json!({"username":"u","password":"p","origin":"http://[::1]:8123/","send":"always"}),
        );
        assert_eq!(
            response(
                &mut auth,
                challenge(
                    "a",
                    "https://example.test/",
                    "https://example.test",
                    "Server"
                )
            )["response"],
            "CancelAuth"
        );
        assert_eq!(
            response(
                &mut auth,
                challenge("b", "http://[::1]:8123/path", "http://[::1]:8123", "Server")
            )["response"],
            "ProvideCredentials"
        );
    }
    #[test]
    fn rejected_credentials_are_not_repeated_and_proxy_never_uses_server_secret() {
        let mut auth = state(json!({"username":"u","password":"private-value"}));
        let value = challenge(
            "same",
            "https://example.test/a",
            "https://example.test",
            "Server",
        );
        assert_eq!(
            response(&mut auth, value.clone())["response"],
            "ProvideCredentials"
        );
        assert_eq!(response(&mut auth, value), json!({"response":"CancelAuth"}));
        assert_eq!(
            response(
                &mut auth,
                challenge(
                    "proxy",
                    "https://example.test/",
                    "https://example.test",
                    "Proxy"
                )
            ),
            json!({"response":"CancelAuth"})
        );
        let mut digest = challenge(
            "digest",
            "https://example.test/",
            "https://example.test",
            "Server",
        );
        digest["authChallenge"]["scheme"] = json!("digest");
        assert_eq!(
            response(&mut auth, digest),
            json!({"response":"CancelAuth"})
        );
    }
    #[test]
    fn unbounded_new_request_ids_cannot_grow_authentication_state() {
        let mut auth = state(json!({"username":"u","password":"p"}));
        for i in 0..MAX_CHALLENGES {
            auth.respond(&challenge(
                &i.to_string(),
                "https://example.test/a",
                "https://example.test",
                "Server",
            ))
            .unwrap();
        }
        assert!(
            auth.respond(&challenge(
                "overflow",
                "https://example.test/a",
                "https://example.test",
                "Server"
            ))
            .is_err()
        );
        assert_eq!(auth.attempts.len(), MAX_CHALLENGES);
    }
}
