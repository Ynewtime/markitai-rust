use crate::{Document, Error, Result};
use mail_parser::MimeHeaders;
use serde_json::Value;

pub(super) fn decode(bytes: &[u8]) -> Result<String> {
    let (encoding, offset) =
        encoding_rs::Encoding::for_bom(bytes).unwrap_or((encoding_rs::UTF_8, 0));
    let bytes = &bytes[offset..];
    let (decoded, malformed) = encoding.decode_without_bom_handling(bytes);
    if !malformed {
        return Ok(decoded.into_owned());
    }
    if offset != 0 {
        return Err(Error::Conversion(
            "Invalid Unicode text after byte-order mark".into(),
        ));
    }
    // Legacy Western text is a supported input; never silently replace bytes.
    let (decoded, malformed) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
    if malformed {
        Err(Error::Conversion(
            "Input text is not valid UTF-8, UTF-16 or Windows-1252".into(),
        ))
    } else {
        Ok(decoded.into_owned())
    }
}

pub(super) fn cell(value: &str) -> String {
    value
        .trim()
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

pub(super) fn table(rows: &[Vec<String>], header: bool) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let row_text = |row: &[String]| {
        format!(
            "| {} |\n",
            (0..width)
                .map(|i| row.get(i).cloned().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    let mut output = String::new();
    let (first, rest) = if header {
        (rows[0].clone(), &rows[1..])
    } else {
        (vec![String::new(); width], rows)
    };
    output.push_str(&row_text(&first));
    output.push_str(&row_text(&vec!["---".into(); width]));
    for row in rest {
        output.push_str(&row_text(row));
    }
    output
}

pub(super) fn delimited(source: &str, delimiter: u8) -> Result<Document> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(source.as_bytes());
    let mut rows = reader
        .records()
        .map(|row| {
            row.map(|row| {
                row.iter()
                    .map(|value| {
                        if delimiter == b',' {
                            value.to_owned()
                        } else {
                            cell(value)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| Error::Conversion(format!("Invalid delimited text: {e}")))
        })
        .collect::<Result<Vec<_>>>()?;
    if delimiter == b',' {
        // CSV's public contract uses the header width, including truncating
        // surplus fields. TSV retains its widest row instead.
        if let Some(width) = rows.first().map(Vec::len) {
            for row in &mut rows {
                row.resize(width, String::new());
            }
        }
    }
    let markdown = table(&rows, true)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Document {
        markdown,
        ..Document::default()
    })
}

fn source_text(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                part.as_str()
                    .ok_or_else(|| Error::Conversion("Notebook source must contain strings".into()))
            })
            .collect::<Result<Vec<_>>>()
            .map(|parts| parts.concat()),
        _ => Err(Error::Conversion(
            "Notebook cell source must be a string or an array of strings".into(),
        )),
    }
}

pub(super) fn fence(source: &str, language: &str) -> String {
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let marker = "`".repeat(longest.saturating_add(1).max(3));
    format!(
        "{marker}{language}\n{}\n{marker}",
        source.trim_end_matches('\n')
    )
}

pub(super) fn notebook(source: &str) -> Result<Document> {
    let value: Value = serde_json::from_str(source)?;
    let cells = value
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Conversion("Notebook cells must be an array".into()))?;
    let language = value
        .pointer("/metadata/language_info/name")
        .and_then(Value::as_str)
        .unwrap_or("python");
    let language: String = language
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'))
        .collect();
    let mut blocks = Vec::new();
    for item in cells {
        let content = source_text(
            item.get("source")
                .ok_or_else(|| Error::Conversion("Notebook cell has no source".into()))?,
        )?;
        match item.get("cell_type").and_then(Value::as_str) {
            Some("code") => blocks.push(fence(&content, &language)),
            Some("markdown") => blocks.push(content.trim_end().to_owned()),
            Some("raw") => blocks.push(fence(&content, "")),
            _ => return Err(Error::Conversion("Unknown notebook cell_type".into())),
        }
    }
    let mut result = Document {
        markdown: blocks.join("\n\n"),
        ..Document::default()
    };
    if let Some(title) = value.pointer("/metadata/title").and_then(Value::as_str) {
        result.metadata.insert("title".into(), title.into());
    }
    Ok(result)
}

pub(super) fn json(source: &str) -> Result<Document> {
    let value: Value = serde_json::from_str(source)?;
    Ok(Document {
        markdown: fence(&serde_json::to_string_pretty(&value)?, "json"),
        ..Document::default()
    })
}

pub(super) fn xml(source: &str) -> Result<Document> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(source);
    let mut depth = 0usize;
    let mut roots = 0usize;
    let mut blocks = Vec::new();
    let mut pending = String::new();
    let malformed =
        |error: &dyn std::fmt::Display| Error::Conversion(format!("Malformed XML: {error}"));
    loop {
        let event = reader.read_event().map_err(|e| malformed(&e))?;
        match &event {
            Event::Start(_) | Event::Empty(_) | Event::End(_) | Event::Eof => {
                if !pending.trim().is_empty() {
                    if depth == 0 {
                        return Err(Error::Conversion(
                            "XML text outside document element".into(),
                        ));
                    }
                    blocks.push(pending.trim().to_owned());
                }
                pending.clear();
            }
            _ => (),
        }
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 0 {
                    roots += 1;
                }
                let level = depth + 1;
                if level > 256 {
                    return Err(Error::Conversion("XML nesting exceeds 256 elements".into()));
                }
                let local_name = element.local_name();
                let name = reader
                    .decoder()
                    .decode(local_name.as_ref())
                    .map_err(|e| malformed(&e))?;
                if level <= 6 {
                    blocks.push(format!("{} {name}", "#".repeat(level)));
                } else {
                    blocks.push(format!("{}- **{name}**", "  ".repeat(level - 7)));
                }
                for attr in element.attributes() {
                    let attr = attr.map_err(|e| malformed(&e))?;
                    if attr.key.as_ref() == b"xmlns" || attr.key.as_ref().starts_with(b"xmlns:") {
                        continue;
                    }
                    let key = attr.key.local_name();
                    let key = reader
                        .decoder()
                        .decode(key.as_ref())
                        .map_err(|e| malformed(&e))?;
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|e| malformed(&e))?;
                    blocks.push(format!("{key}: {value}"));
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Event::Text(text) => pending.push_str(
                &text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|e| malformed(&e))?,
            ),
            Event::CData(text) => pending.push_str(
                &text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|e| malformed(&e))?,
            ),
            Event::GeneralRef(reference) => {
                let name = reference.decode().map_err(|e| malformed(&e))?;
                let encoded = format!("&{name};");
                pending
                    .push_str(&quick_xml::escape::unescape(&encoded).map_err(|e| malformed(&e))?);
            }
            Event::Eof => break,
            Event::DocType(_) => {
                return Err(Error::Conversion(
                    "XML document types and external entities are not supported".into(),
                ));
            }
            _ => (),
        }
    }
    if depth != 0 || roots != 1 {
        return Err(Error::Conversion(
            "XML must contain one complete document element".into(),
        ));
    }
    let source = source.trim();
    if source.len() < 20 * 1024 {
        blocks.push(fence(source, "xml"));
    }
    Ok(Document {
        markdown: blocks.join("\n\n"),
        ..Document::default()
    })
}

pub(super) fn email(bytes: &[u8]) -> Result<Document> {
    let message = mail_parser::MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| Error::Conversion("Malformed email message".into()))?;
    let title = message.subject().unwrap_or("");
    let mut metadata = serde_json::Map::new();
    if !title.is_empty() {
        metadata.insert("title".into(), title.into());
    }
    if let Some(date) = message.date() {
        metadata.insert("date".into(), date.to_rfc3339().into());
    }
    let actual_html =
        message
            .html_body
            .iter()
            .find_map(|id| match &message.parts.get(*id as usize)?.body {
                mail_parser::PartType::Html(html) => Some(html.as_ref()),
                _ => None,
            });
    let body = if let Some(html) = actual_html {
        super::html::fragment(html)?
    } else {
        message
            .body_text(0)
            .map(|s| s.into_owned())
            .unwrap_or_default()
    };
    let mut headers = Vec::new();
    for name in ["From", "To", "Cc", "Date", "Subject"] {
        for value in message.header_as(name, mail_parser::HeaderForm::Text) {
            if let Some(value) = value.as_text() {
                let date = (name == "Date")
                    .then(|| message.date())
                    .flatten()
                    .map(|date| {
                        // RFC 5322 date headers are rendered with a two-digit day.
                        date.to_rfc822().replacen(
                            &format!(", {} ", date.day),
                            &format!(", {:02} ", date.day),
                            1,
                        )
                    });
                let value = date
                    .as_deref()
                    .unwrap_or(value)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .replace('<', "\\<")
                    .replace('>', "\\>");
                if !value.is_empty() {
                    headers.push(format!("**{name}:** {value}"));
                }
            }
        }
    }
    let mut markdown = "# Email Message".to_owned();
    if !headers.is_empty() {
        markdown.push_str(&format!("\n\n{}", headers.join("\n")));
    }
    markdown.push_str(&format!("\n\n## Content\n\n{}", body.trim()));
    let mut assets = Vec::new();
    for (index, attachment) in message.attachments().enumerate() {
        let name = attachment.attachment_name().unwrap_or("attachment.bin");
        let name: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let name = format!("email-{}-{name}", index + 1);
        assets.push(crate::Asset {
            name: name.clone(),
            bytes: attachment.contents().to_vec(),
        });
        markdown.push_str(&format!(
            "\n\n[Attachment {}](.markitai/assets/{name})",
            index + 1
        ));
    }
    Ok(Document {
        markdown,
        metadata,
        assets,
        warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodings_keep_non_ascii_text() {
        assert_eq!(decode(b"caf\xe9").unwrap(), "café");
        assert_eq!(decode(&[0xff, 0xfe, 0x2d, 0x4e]).unwrap(), "中");
        assert_eq!(decode(b"\xef\xbb\xbfhello").unwrap(), "hello");
        assert!(decode(&[0xff, 0xfe, 0x00]).is_err());
    }
    #[test]
    fn csv_keeps_quotes_newlines_and_reference_header_width() {
        let doc = delimited("name,body\n\"a,b\",\"line\nbreak | text\"\nlast\n", b',').unwrap();
        assert_eq!(
            doc.markdown,
            "| name | body |\n| --- | --- |\n| a,b | line\nbreak | text |\n| last |  |"
        );
        assert_eq!(
            delimited("a,b\n1\n2,3,4\n", b',').unwrap().markdown,
            "| a | b |\n| --- | --- |\n| 1 |  |\n| 2 | 3 |"
        );
        assert!(delimited("", b',').unwrap().markdown.is_empty());
    }
    #[test]
    fn tsv_escapes_cells_and_retains_surplus_columns() {
        let doc = delimited("a\tb\nx|y\t\"line\nbreak\"\textra\n", b'\t').unwrap();
        assert!(doc.markdown.contains("| a | b |  |"));
        assert!(doc.markdown.contains("x\\|y | line<br>break | extra"));
    }
    #[test]
    fn notebook_preserves_all_cell_types_and_long_fences() {
        let doc = notebook(r##"{"metadata":{"title":"Notebook","language_info":{"name":"python"}},"cells":[{"cell_type":"markdown","source":["# Title\n","text"]},{"cell_type":"code","source":"print('```')\n"},{"cell_type":"raw","source":"raw"}]}"##).unwrap();
        assert!(doc.markdown.contains("````python\nprint('```')\n````"));
        assert!(doc.markdown.ends_with("```\nraw\n```"));
        assert_eq!(doc.metadata["title"], "Notebook");
        assert!(notebook(r#"{"cells":null}"#).is_err());
    }
    #[test]
    fn xml_rejects_broken_or_external_entity_input() {
        assert!(
            xml("<root><child>text</child></root>")
                .unwrap()
                .markdown
                .starts_with("# root\n\n## child\n\ntext")
        );
        assert!(xml("<root>").is_err());
        assert!(xml("<a/><b/>").is_err());
        assert!(xml("<!DOCTYPE a SYSTEM 'https://example.test/x'><a/>").is_err());
        assert!(xml("outside<a/>").is_err());
        assert!(xml("<a>&unknown;</a>").is_err());
        let mixed =
            xml("<x:root xmlns:x=\"urn:test\" a=\"1&amp;2\">left &lt;<x:child/>right</x:root>")
                .unwrap();
        assert!(
            mixed
                .markdown
                .starts_with("# root\n\na: 1&2\n\nleft <\n\n## child\n\nright\n\n```xml")
        );
    }
    #[test]
    fn email_decodes_mime_and_subject() {
        let doc = email(b"Subject: =?UTF-8?B?5Lit5paH?=\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello body").unwrap();
        assert_eq!(doc.metadata["title"], "中文");
        assert!(doc.markdown.contains("Hello body"));
        let doc = email(b"From: from@example.test\nSubject: Example\nContent-Type: text/plain\n\nFirst line\nSecond line").unwrap();
        assert_eq!(
            doc.markdown,
            "# Email Message\n\n**From:** from@example.test\n**Subject:** Example\n\n## Content\n\nFirst line\nSecond line"
        );
    }
}
