//! Whole-workbook API acceptance against an actual optional renderer.
#![cfg(target_os = "macos")]

use super::*;
use std::path::{Path, PathBuf};

const XLSX: &[u8] = include_bytes!("../../src/office_render/fixtures/whole-workbook.xlsx");
const ODS: &[u8] = include_bytes!("../../src/office_render/fixtures/whole-workbook.ods");
const XLS: &[u8] = include_bytes!("../../src/office_render/fixtures/whole-workbook.xls");

fn source(root: &Path, extension: &str, bytes: &[u8]) -> PathBuf {
    let source = root.join(format!("完整 工作簿.{extension}"));
    std::fs::write(&source, bytes).unwrap();
    source
}

fn config() -> Value {
    json!({
        "llm":{"enabled":false},"ocr":{"enabled":false},
        "cache":{"enabled":false},"history":{"record":false},
        "screenshot":{"enabled":true},
        "image":{"format":"png","max_width":5000,"max_height":5000,"alt_enabled":false,"desc_enabled":false}
    })
}

#[test]
#[ignore = "Requires installed LibreOffice and native macOS renderer; run explicitly"]
fn workbook_capture_keeps_native_tables_and_all_sheet_images_in_order() {
    for (extension, bytes) in [("xlsx", XLSX), ("ods", ODS), ("xls", XLS)] {
        let directory = tempfile::tempdir().unwrap();
        let source = source(directory.path(), extension, bytes);
        let mut native_config = config();
        native_config["screenshot"]["enabled"] = json!(false);
        let native = convert(
            source.to_str().unwrap(),
            ConvertOptions {
                config: Some(native_config),
                ..Default::default()
            },
        )
        .unwrap();
        let captured = convert(
            source.to_str().unwrap(),
            ConvertOptions {
                config: Some(config()),
                output_dir: Some(directory.path().join("out")),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            captured.markdown.starts_with(&native.markdown),
            "{extension}: native body changed"
        );
        for token in ["VISIBLE FIRST", "OUTSIDE PRINT AREA", "LAST SHEET"] {
            assert!(captured.markdown.contains(token), "{extension}: {token}");
        }
        assert_eq!(
            captured.markdown.contains("HIDDEN MIDDLE"),
            native.markdown.contains("HIDDEN MIDDLE"),
            "{extension}: native hidden-sheet text policy changed"
        );
        assert_eq!(captured.screenshots.len(), 4, "{extension}");
        let mut prior = 0;
        for page in 1..=4 {
            let marker = format!("<!-- ![Page {page}]");
            let offset = captured.markdown.find(&marker).unwrap();
            assert!(offset > prior);
            prior = offset;
        }
        let first = image::open(&captured.screenshots[0]).unwrap().to_rgb8();
        let hidden = image::open(&captured.screenshots[1]).unwrap().to_rgb8();
        let blank = image::open(&captured.screenshots[2]).unwrap().to_rgb8();
        let last = image::open(&captured.screenshots[3]).unwrap().to_rgb8();
        assert!(first.width() > 1400 && first.height() > 1400);
        assert!(hidden.pixels().any(|pixel| pixel.0 == [0, 255, 0]));
        assert!(blank.pixels().all(|pixel| pixel.0 == [255, 255, 255]));
        assert!(last.pixels().any(|pixel| pixel.0 == [255, 204, 0]));
        assert!(
            captured
                .warnings
                .iter()
                .any(|warning| warning.contains("complete-sheet export"))
        );
        assert_eq!(std::fs::read(source).unwrap(), bytes);
    }
}

#[test]
#[ignore = "Requires installed LibreOffice and native macOS renderer; run explicitly"]
fn workbook_all_sheet_budget_fails_before_model_admission() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "xlsx", XLSX);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut config = config();
    config["llm"] = json!({"enabled":true,"max_vision_pages_per_document":3,
        "router_settings":{"num_retries":0,"timeout":2},
        "model_list":[{"model_name":"mock","litellm_params":{"model":"openai/mock","api_key":"test-only","api_base":format!("http://{}/v1",listener.local_addr().unwrap())},"model_info":{"supports_vision":true}}]});
    let error = convert(
        source.to_str().unwrap(),
        ConvertOptions {
            config: Some(config),
            output_dir: Some(directory.path().join("out")),
            ..Default::default()
        },
    )
    .expect_err("all four sheets exceed the three-page model budget");
    assert!(error.to_string().contains("vision"), "{error}");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
#[ignore = "Requires installed LibreOffice and native macOS renderer; run explicitly"]
fn workbook_explicit_only_sends_every_sheet_and_pure_preserves_table_text() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path(), "xlsx", XLSX);
    let mut native_config = config();
    native_config["screenshot"]["enabled"] = json!(false);
    let native = convert(
        source.to_str().unwrap(),
        ConvertOptions {
            config: Some(native_config),
            ..Default::default()
        },
    )
    .unwrap();
    for only in [false, true] {
        let (base, server) = llm_server(
            200,
            r##"{"choices":[{"message":{"content":"# Reviewed\n\nAll four sheets."}}]}"##,
        );
        let mut config = config();
        config["llm"] = json!({"enabled":true,"pure":true,"keep_base":true,
            "router_settings":{"num_retries":0,"timeout":5},
            "model_list":[{"model_name":"mock","litellm_params":{"model":"openai/mock","api_base":base,"api_key":"test-only"},"model_info":{"supports_vision":true}}]});
        config["screenshot"]["screenshot_only"] = json!(only);
        let result = convert(
            source.to_str().unwrap(),
            ConvertOptions {
                config: Some(config),
                output_dir: Some(directory.path().join(format!("out-{only}"))),
                ..Default::default()
            },
        )
        .unwrap();
        let request = server.join().unwrap();
        let content = &request["messages"][1]["content"];
        if only {
            let blocks = content.as_array().unwrap();
            assert_eq!(
                blocks
                    .iter()
                    .filter(|block| block["type"] == "image_url")
                    .count(),
                4
            );
            assert!(
                !blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .any(|text| text.contains("VISIBLE FIRST"))
            );
            assert_eq!(
                result
                    .llm_markdown
                    .as_ref()
                    .unwrap()
                    .matches("<!-- ![Page ")
                    .count(),
                4
            );
        } else {
            assert!(content.as_str().unwrap().contains("OUTSIDE PRINT AREA"));
            assert!(
                !result
                    .llm_markdown
                    .as_ref()
                    .unwrap()
                    .contains("<!-- ![Page ")
            );
        }
        assert_eq!(result.screenshots.len(), 4);
        assert!(result.markdown.starts_with(&native.markdown));
        assert_eq!(
            result.markdown.contains("HIDDEN MIDDLE"),
            native.markdown.contains("HIDDEN MIDDLE")
        );
    }
}
