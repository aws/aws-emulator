// SPDX-License-Identifier: CC0-1.0

//! The form a configuration revision's document is STORED in.
//!
//! RULE: Amazon MQ does not keep the bytes a request sent for a revision — it parses
//! the engine configuration document and stores its own serialization of it, which is
//! what every later read of the revision reports. The model's own evidence that the
//! service rewrites rather than echoes the document is `UpdateConfiguration`'s
//! `Warnings` member, "the first 20 warnings about the configuration elements or
//! attributes that were sanitized"
//! (`smithy_query(query="doc", shape="UpdateConfigurationResponse", member="Warnings")`).
//!
//! The one difference between a submitted and a stored XML document this service
//! serves is the spelling of an element with no content: the stored serialization
//! writes it self-closed (`<plugins/>`), whatever pair of tags the request spelled it
//! with. Everything else the request sent — the prolog, the element names, their
//! attributes as written, the text and comments they carry, and the whitespace between
//! them — is the document's own content and is kept.
//!
//! A document that is not XML at all is a RabbitMQ Cuttlefish configuration (`Data` is
//! "the base64-encoded XML configuration" for ActiveMQ and "the base64-encoded
//! Cuttlefish configuration" for RabbitMQ), which has no element to serialize: it is
//! stored as it was sent.
//!
//! The parsing and the re-serialization are the `quick-xml` crate's; this module only
//! decides which elements have no content. One pass over the document's events, with no
//! recursion on its nesting and an answer no larger than the document itself, is all
//! this costs: the work a request can ask for here is bounded by the document the
//! request already carried, and a document this module cannot parse is answered `None`
//! rather than by failing the call.

/// The `Data` a revision STORES for the document a request sent (both base64, as the
/// member documents).
///
/// The document is canonicalized when it is XML and the canonical serialization
/// differs from what was sent; anything else — a document that is not valid base64,
/// not UTF-8 text, not XML, or already canonical — is stored exactly as it arrived.
pub fn stored_configuration_data(data: &str) -> String {
    let Ok(bytes) = ::aws_smithy_types::base64::decode(data) else {
        return data.to_string();
    };
    let Ok(document) = String::from_utf8(bytes) else {
        return data.to_string();
    };
    match canonical_xml_document(&document) {
        Some(canonical) if canonical != document => {
            ::aws_smithy_types::base64::encode(canonical)
        }
        _ => data.to_string(),
    }
}

/// The canonical serialization of an XML configuration document: the same document
/// with every content-free element written self-closed.
///
/// `None` when the text is not a single-rooted XML document — a Cuttlefish
/// configuration, or markup this service cannot parse — which has no canonical
/// serialization and is therefore stored as sent.
pub fn canonical_xml_document(document: &str) -> Option<String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    // A configuration document is XML, so an element that never closes means the text
    // is not a document this service can serialize — it is kept as sent instead.
    reader.config_mut().check_end_names = true;
    let mut writer = ::quick_xml::Writer::new(Vec::new());

    // How deep in the element tree the reader is, and how many elements were opened at
    // the top: a canonical document has exactly one root, and carries nothing but
    // whitespace outside it.
    let mut depth = 0usize;
    let mut roots = 0usize;
    // An element whose start tag was read but whose content is not known yet, together
    // with the whitespace read since: if its end tag comes next the element has no
    // content and is written self-closed, otherwise both are written as they came.
    let mut pending: Option<::quick_xml::events::BytesStart<'_>> = None;
    let mut pending_space: Vec<::quick_xml::events::BytesText<'_>> = Vec::new();

    loop {
        let event = reader.read_event().ok()?;
        // Whitespace between a start tag and its own end tag is not content: it is the
        // indentation of an element the request spelled as a pair of tags, and the
        // canonical serialization of that element carries no content at all.
        if let Some(start) = pending.take() {
            if let Event::Text(text) = &event {
                if is_whitespace(text.as_ref()) {
                    pending_space.push(text.clone());
                    pending = Some(start);
                    continue;
                }
            }
            if matches!(event, Event::End(_)) {
                // A content-free element: one self-closed tag replaces the pair, and
                // the whitespace that stood between them goes with them.
                writer.write_event(Event::Empty(start)).ok()?;
                pending_space.clear();
                depth = depth.checked_sub(1)?;
                continue;
            }
            writer.write_event(Event::Start(start)).ok()?;
            for space in pending_space.drain(..) {
                writer.write_event(Event::Text(space)).ok()?;
            }
        }

        match event {
            Event::Eof => break,
            Event::Start(start) => {
                if depth == 0 {
                    roots = roots.saturating_add(1);
                }
                depth = depth.saturating_add(1);
                pending = Some(start);
            }
            Event::Empty(empty) => {
                if depth == 0 {
                    roots = roots.saturating_add(1);
                }
                writer.write_event(Event::Empty(empty)).ok()?;
            }
            Event::End(end) => {
                depth = depth.checked_sub(1)?;
                writer.write_event(Event::End(end)).ok()?;
            }
            Event::Text(text) => {
                // Text outside the root element is not part of an XML document, so a
                // Cuttlefish configuration — all text, no element — is not one.
                if depth == 0 && !is_whitespace(text.as_ref()) {
                    return None;
                }
                writer.write_event(Event::Text(text)).ok()?;
            }
            other => writer.write_event(other).ok()?,
        }
    }

    if depth != 0 || roots != 1 {
        return None;
    }
    String::from_utf8(writer.into_inner()).ok()
}

/// Whether a text node carries nothing but XML whitespace, which is what the
/// indentation between a pair of tags is made of.
fn is_whitespace(text: &[u8]) -> bool {
    !text.is_empty() && text.iter().all(|byte| byte.is_ascii_whitespace())
}
