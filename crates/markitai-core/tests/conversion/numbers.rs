use markitai_core::{ConvertOptions, convert};
use serde_json::json;

#[test]
fn numbers_public_api_and_publication_keep_real_fixture_tables() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("工作簿.numbers");
    std::fs::write(
        &source,
        include_bytes!("../../src/formats/numbers/fixtures/test-1.numbers"),
    )
    .unwrap();
    let options = || ConvertOptions {
        config: Some(
            json!({"llm":{"enabled":false},"ocr":{"enabled":false},"cache":{"enabled":false},"history":{"enabled":false}}),
        ),
        ..Default::default()
    };
    let memory = convert(source.to_str().unwrap(), options()).unwrap();
    assert_eq!(
        markitai_core::formats::extract(&source).unwrap().metadata["format"],
        "NUMBERS"
    );
    assert_eq!(memory.frontmatter["source"], "工作簿.numbers");
    assert!(memory.markdown.contains("YYY\\_4\\_2"));
    assert!(memory.markdown.contains("XXX\\_3\\_5"));
    assert!(memory.assets.is_empty());
    let disk = convert(
        source.to_str().unwrap(),
        ConvertOptions {
            output_dir: Some(temp.path().join("out")),
            ..options()
        },
    )
    .unwrap();
    assert_eq!(memory.markdown, disk.markdown);
    let saved = std::fs::read_to_string(disk.output_path.unwrap()).unwrap();
    assert!(saved.ends_with(&memory.markdown));
}
