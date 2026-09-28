use super::{Result, failure};
use quick_xml::{events::Event, name::ResolveResult};
use std::io::{Cursor, Read};

pub(super) fn odp(bytes: &[u8]) -> Result<usize> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| failure("invalid ODP package"))?;
    if archive.len() > 16_384 {
        return Err(failure("ODP package has too many entries"));
    }
    let file = archive
        .by_name("content.xml")
        .map_err(|_| failure("ODP has no content.xml"))?;
    const LIMIT: u64 = 16 * 1024 * 1024;
    if file.size() > LIMIT {
        return Err(failure("ODP content exceeds 16 MiB"));
    }
    let mut xml = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut xml)?;
    if xml.len() as u64 > LIMIT {
        return Err(failure("ODP content exceeds 16 MiB"));
    }
    let mut reader = quick_xml::NsReader::from_reader(xml.as_slice());
    let mut depth = 0usize;
    let mut presentation = None;
    let mut count = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| failure("invalid ODP XML"))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if depth >= 128 {
                    return Err(failure("ODP XML nesting exceeds 128"));
                }
                let ns = reader.resolver().resolve_element(element.name()).0;
                if element.local_name().as_ref() == b"presentation"
                    && matches!(ns, ResolveResult::Bound(uri) if uri.as_ref()==b"urn:oasis:names:tc:opendocument:xmlns:office:1.0")
                {
                    if presentation.is_some() {
                        return Err(failure("nested ODP presentation"));
                    }
                    if !empty {
                        presentation = Some(depth);
                    }
                }
                if presentation.is_some_and(|parent| depth == parent + 1)
                    && element.local_name().as_ref() == b"page"
                    && matches!(ns, ResolveResult::Bound(uri) if uri.as_ref()==b"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0")
                {
                    count += 1;
                    if count > super::MAX_PAGES {
                        return Err(failure("ODP has more than 1,000 slides"));
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| failure("unbalanced ODP XML"))?;
                if presentation == Some(depth) {
                    presentation = None;
                }
            }
            Event::DocType(_) => return Err(failure("ODP document types are not accepted")),
            Event::Eof => break,
            _ => {}
        }
    }
    if count == 0 || depth != 0 {
        return Err(failure("ODP has no complete presentation slides"));
    }
    Ok(count)
}
