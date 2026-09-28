//! Test-binary entrypoint for the isolated reference state-codec comparison.
use super::{Limits, LoadOutcome, Mode, Scope, StateStore, codec};
use serde_json::Value;
use std::path::Path;

#[test]
#[ignore = "invoked explicitly by scripts/audit_state.py with an authored fixture"]
fn legacy_fixture() {
    let request = std::env::var_os("MARKITAI_STATE_AUDIT_REQUEST")
        .expect("the state audit requires its private request file");
    let request: Value = serde_json::from_slice(&std::fs::read(request).unwrap()).unwrap();
    let mode = match request["mode"].as_str().unwrap() {
        "directory" => Mode::Directory,
        "url_list" => Mode::UrlList,
        _ => panic!("unknown state audit mode"),
    };
    let scope = Scope::new(
        mode,
        Path::new(request["input"].as_str().unwrap()),
        Path::new(request["output"].as_str().unwrap()),
    )
    .unwrap();
    let allow_symlinks = request["allow_symlinks"].as_bool().unwrap_or(false);
    let bytes = std::fs::read(request["fixture"].as_str().unwrap()).unwrap();
    let hash = codec::task_hash(&scope, &request["hash_options"]).unwrap();
    let mut snapshot = if let Some(journal) = request["journal"].as_str() {
        let directory = scope.output.join(".markitai/states");
        std::fs::create_dir_all(&directory).unwrap();
        let base = directory.join(format!("markitai.{hash}.state.json"));
        std::fs::write(&base, &bytes).unwrap();
        std::fs::copy(journal, base.with_extension("jsonl")).unwrap();
        let mut store =
            StateStore::open(scope.clone(), &hash, allow_symlinks, Limits::default()).unwrap();
        match store.load().unwrap() {
            LoadOutcome::Loaded { snapshot, .. } => *snapshot,
            other => panic!("authored legacy replay did not load: {other:?}"),
        }
    } else {
        codec::decode(&bytes, &scope, allow_symlinks, Limits::default()).unwrap()
    };
    codec::normalize_interrupted(&mut snapshot);
    let encoded = codec::encode(&snapshot, &scope, allow_symlinks, Limits::default()).unwrap();
    let mut output = format!(
        "{{\"hash\":{},\"snapshot\":",
        serde_json::to_string(&hash).unwrap()
    )
    .into_bytes();
    output.extend_from_slice(&encoded);
    output.extend_from_slice(b"}\n");
    std::fs::write(request["result"].as_str().unwrap(), output).unwrap();
}
