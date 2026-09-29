use super::*;

fn remote(id: &str, nonce: &str) -> Value {
    let mut value = batch_value("in_progress");
    value["id"] = json!(id);
    value["metadata"] =
        json!({"markitai_submission":nonce,"other_app_private":"must not enter diagnostics"});
    value
}
fn page(rows: Vec<Value>, has_more: bool) -> Value {
    let last = rows
        .last()
        .and_then(|row| row["id"].as_str())
        .map(str::to_owned);
    json!({"object":"list","data":rows,"last_id":last,"has_more":has_more})
}
fn only_get(server: &Server, count: usize) {
    let requests = server.requests();
    assert_eq!(requests.len(), count);
    assert!(
        requests
            .iter()
            .all(|request| request.method == "GET" && request.body.is_empty())
    );
    assert!(requests.iter().all(|request| {
        request
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-only-secret")
    }));
}

#[test]
fn complete_paginated_nonce_match_requires_one_fresh_strict_lookup() {
    let server = Server::new(|request, index| match index {
        0 => {
            assert_eq!(request.path, "/prefix/v1/batches?limit=100");
            let mut unrelated = remote("batch_unrelated", "another_submission");
            unrelated["endpoint"] = json!("/v1/embeddings");
            Reply::json(page(vec![unrelated], true))
        }
        1 => {
            assert_eq!(
                request.path,
                "/prefix/v1/batches?limit=100&after=batch_unrelated"
            );
            Reply::json(page(vec![remote("batch_found", "attempt_123")], false))
        }
        2 => {
            assert_eq!(request.path, "/prefix/v1/batches/batch_found");
            let mut value = remote("batch_found", "attempt_123");
            value["status"] = json!("completed");
            Reply::json(value)
        }
        _ => panic!("unexpected extra request"),
    });
    let result = server
        .client()
        .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
        .unwrap();
    let Reconciliation::Found(identity) = result else {
        panic!("a complete unique match was expected")
    };
    assert_eq!(identity.batch().id, "batch_found");
    assert_eq!(identity.batch().status, BatchStatus::Completed);
    assert_eq!(identity.api_base(), server.base());
    assert!(identity.matches(&uploaded(), "attempt_123"));
    let debug = format!("{identity:?}");
    assert!(!debug.contains("attempt_123"));
    assert!(!debug.contains("must not enter"));
    assert!(!debug.contains("fixture-only-secret"));
    only_get(&server, 3);
}

#[test]
fn zero_matches_never_creates_and_partial_search_never_claims_uniqueness() {
    let empty = Server::new(|_, _| Reply::json(page(Vec::new(), false)));
    assert!(matches!(
        empty
            .client()
            .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
            .unwrap(),
        Reconciliation::NotFound
    ));
    only_get(&empty, 1);
    let partial =
        Server::new(|_, _| Reply::json(page(vec![remote("batch_found", "attempt_123")], true)));
    let limits = ReconcileLimits {
        pages: 1,
        ..Default::default()
    };
    assert!(matches!(
        partial
            .client()
            .reconcile(&uploaded(), "attempt_123", limits)
            .unwrap(),
        Reconciliation::Incomplete
    ));
    only_get(&partial, 1);
}

#[test]
fn two_distinct_matches_are_ambiguous_without_selecting_the_first() {
    let server = Server::new(|_, _| {
        Reply::json(page(
            vec![
                remote("batch_first", "attempt_123"),
                remote("batch_second", "attempt_123"),
            ],
            false,
        ))
    });
    let result = server
        .client()
        .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
        .unwrap();
    let Reconciliation::Ambiguous(ids) = result else {
        panic!("ambiguity must be explicit")
    };
    assert_eq!(ids, ["batch_first", "batch_second"]);
    only_get(&server, 1);
}

#[test]
fn repeated_ids_and_empty_nonterminal_pages_cannot_complete_a_search() {
    let repeated =
        Server::new(|_, _| Reply::json(page(vec![remote("batch_repeat", "other")], true)));
    assert!(matches!(
        repeated
            .client()
            .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
            .unwrap(),
        Reconciliation::Incomplete
    ));
    only_get(&repeated, 2);
    let empty = Server::new(|_, _| {
        Reply::json(
            json!({"object":"list","data":[],"has_more":true,"last_id":"batch_not_returned"}),
        )
    });
    assert!(matches!(
        empty
            .client()
            .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
            .unwrap(),
        Reconciliation::Incomplete
    ));
    only_get(&empty, 1);
}

#[test]
fn manual_binding_requires_present_exact_endpoint_nonce_file_and_requested_id() {
    for kind in 0..6 {
        let server = Server::new(move |_, _| {
            let mut value = remote("batch_fixture", "attempt_123");
            match kind {
                0 => {
                    value.as_object_mut().unwrap().remove("metadata");
                }
                1 => value["metadata"]["markitai_submission"] = json!("foreign_nonce"),
                2 => value["input_file_id"] = json!("file_other"),
                3 => {
                    value.as_object_mut().unwrap().remove("endpoint");
                }
                4 => value["endpoint"] = json!("/v1/embeddings"),
                _ => value["id"] = json!("batch_other"),
            }
            Reply::json(value)
        });
        assert!(
            server
                .client()
                .verify_binding("batch_fixture", &uploaded(), "attempt_123")
                .is_err(),
            "kind {kind}"
        );
        only_get(&server, 1);
    }
    let correct = Server::new(|_, _| Reply::json(remote("batch_fixture", "attempt_123")));
    assert!(
        correct
            .client()
            .verify_binding("batch_fixture", &uploaded(), "attempt_123")
            .unwrap()
            .matches(&uploaded(), "attempt_123")
    );
    only_get(&correct, 1);
}

#[test]
fn identity_change_between_list_and_retrieve_is_not_bound() {
    let server = Server::new(|_, index| {
        if index == 0 {
            Reply::json(page(vec![remote("batch_fixture", "attempt_123")], false))
        } else {
            Reply::json(remote("batch_fixture", "changed_nonce"))
        }
    });
    let error = server
        .client()
        .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
        .unwrap_err();
    assert_eq!(
        error,
        Error::Invalid("Batch identity changed during reconciliation")
    );
    only_get(&server, 2);
}

#[test]
fn same_nonce_with_foreign_input_is_conflict_not_a_missing_submission() {
    let server = Server::new(|_, _| {
        let mut value = remote("batch_fixture", "attempt_123");
        value["input_file_id"] = json!("file_other");
        Reply::json(page(vec![value], false))
    });
    assert!(
        server
            .client()
            .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
            .is_err()
    );
    only_get(&server, 1);
}

#[test]
fn redirect_and_auth_errors_do_not_forward_credentials_or_replay_requests() {
    let trap = Server::new(|_, _| panic!("reconciliation followed a redirect"));
    let location = trap.base();
    let redirect = Server::new(move |_, _| Reply::Redirect(location.clone()));
    assert_eq!(
        redirect
            .client()
            .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
            .unwrap_err(),
        Error::Http(307)
    );
    assert_eq!(
        redirect
            .client()
            .verify_binding("batch_fixture", &uploaded(), "attempt_123")
            .unwrap_err(),
        Error::Http(307)
    );
    assert!(trap.requests().is_empty());
    only_get(&redirect, 2);
    let auth = Server::new(|_, _| {
        Reply::Bytes(401, b"fixture-only-secret sensitive remote body".to_vec())
    });
    let error = auth
        .client()
        .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
        .unwrap_err();
    assert_eq!(error, Error::Http(401));
    assert!(!error.to_string().contains("fixture-only-secret"));
    only_get(&auth, 1);
}

#[test]
fn combined_byte_budget_includes_the_final_identity_retrieval() {
    let value = page(vec![remote("batch_fixture", "attempt_123")], false);
    let bytes = serde_json::to_vec(&value).unwrap().len();
    let server = Server::new(move |_, index| {
        if index == 0 {
            Reply::json(value.clone())
        } else {
            panic!("budget already exhausted")
        }
    });
    let limits = ReconcileLimits {
        bytes,
        ..Default::default()
    };
    assert!(matches!(
        server
            .client()
            .reconcile(&uploaded(), "attempt_123", limits)
            .unwrap(),
        Reconciliation::Incomplete
    ));
    only_get(&server, 1);
    let oversized = Server::new(|_, _| Reply::Chunked(vec![b' '; 512]));
    let limits = ReconcileLimits {
        bytes: 128,
        ..Default::default()
    };
    assert!(matches!(
        oversized
            .client()
            .reconcile(&uploaded(), "attempt_123", limits)
            .unwrap(),
        Reconciliation::Incomplete
    ));
    only_get(&oversized, 1);
}

#[test]
fn malformed_pagination_is_rejected_and_invalid_inputs_do_no_io() {
    for value in [
        json!({"object":"list","data":[],"has_more":"false"}),
        json!({"object":"other","data":[],"has_more":false}),
        json!({"object":"list","data":[{"id":"../../escape"}],"has_more":false}),
        page(
            (0..101)
                .map(|index| remote(&format!("batch_{index}"), "other"))
                .collect(),
            false,
        ),
    ] {
        let server = Server::new(move |_, _| Reply::json(value.clone()));
        assert!(
            server
                .client()
                .reconcile(&uploaded(), "attempt_123", ReconcileLimits::default())
                .is_err()
        );
        only_get(&server, 1);
    }
    let server = Server::new(|_, _| panic!("invalid local input reached network"));
    assert!(
        server
            .client()
            .reconcile(&uploaded(), "bad nonce", ReconcileLimits::default())
            .is_err()
    );
    assert!(
        server
            .client()
            .reconcile(
                &uploaded(),
                "attempt_123",
                ReconcileLimits {
                    pages: 0,
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(
        server
            .client()
            .verify_binding("../escape", &uploaded(), "attempt_123")
            .is_err()
    );
    assert!(server.requests().is_empty());
}

#[test]
fn flushed_headers_with_stalled_body_obey_the_whole_search_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
            assert!(headers.len() < 65536);
        }
        assert!(headers.starts_with(b"GET /v1/batches?limit=100 "));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        thread::sleep(Duration::from_millis(300));
    });
    let client = Client::new(
        &format!("http://{address}/v1"),
        "fixture-only-secret",
        Duration::from_secs(3),
        Limits::default(),
    )
    .unwrap();
    let result = client
        .reconcile(
            &uploaded(),
            "attempt_123",
            ReconcileLimits {
                timeout: Duration::from_millis(100),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(matches!(result, Reconciliation::Incomplete));
    worker.join().unwrap();
}
