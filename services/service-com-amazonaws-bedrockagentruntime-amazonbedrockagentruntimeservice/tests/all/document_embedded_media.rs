// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IMAGE a `GetDocumentContent` request names with `mediaId`
//! (`POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents/{documentId}/content`).
//!
//! THE RULES UNDER TEST:
//!
//! * `mediaId` names one image EMBEDDED in the named document — a resource of its own,
//!   which exists only because a parse of that document's bytes pulled it out and recorded
//!   it, which is why the member is documented as carrying the `_media_id` a retrieval
//!   reported in the metadata of a result. No ingestion of this emulator records such an
//!   image, so every identifier of one names an image that does not exist and the read is
//!   the modelled `ResourceNotFoundException` (404) naming that media — never the whole
//!   document's address, which is what a read naming no media asks for;
//! * a request naming an image asks for that image "instead of the document", so the only
//!   rendering it may ask of it is the one it was ingested in: `mediaId` beside any
//!   `outputFormat` but `RAW` is the modelled `ValidationException` (400) naming both
//!   members;
//! * the image is read only after the resources of the PATH have resolved, so a knowledge
//!   base, data source or document this emulator does not hold is reported as the missing
//!   resource it is whatever media the request named, and an unpermitted caller is refused
//!   before any of it;
//! * the read changes nothing: a later read of the same document reports the same content
//!   and its address still serves the ingested bytes.
//!
//! The tables span what those rules generalise over: the form of the media identifier
//! (every one of them a value the modelled `MediaId` admits, at its published length and
//! past it), the `outputFormat` leg beside it, how the document's bytes reached the
//! knowledge base (an object store, inline text, inline bytes — an image among them), and
//! which half of the document's identity is unknown.
//!
//! The knowledge base, the data source and the documents are created through the build-time
//! plane's own API, and the source objects written through S3's own store, by the helpers
//! the `document_content` suite beside this one defines.

use serde_json::{json, Value};

use crate::agent_memory::{allow, Emulator, ACCESS_DENIED, ACCOUNT_KEY, NOT_FOUND};
use crate::document_content::{
    inline_byte_document, inline_text_document, s3_document, Ingested, UNKNOWN_DATA_SOURCE,
    UNKNOWN_KNOWLEDGE_BASE,
};

/// The wire code and status of a refusal on the request itself, from the model:
/// `smithy_query(query="operation",operation="GetDocumentContent")` -> `ValidationException`
/// httpError 400.
const INVALID: (&str, u16) = ("ValidationException", 400);

/// The `DocumentOutputFormat` a request may name beside a media id
/// (`smithy_query(query="shape",shape="DocumentOutputFormat")` -> RAW, EXTRACTED).
const OUTPUT_FORMAT_RAW: &str = "RAW";

/// The other `DocumentOutputFormat`, which a request naming a media id may not ask for.
const OUTPUT_FORMAT_EXTRACTED: &str = "EXTRACTED";

/// How long every value the modelled `MediaId` admits is
/// (`smithy_query(query="shape",shape="MediaId")` -> length 36..36, pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9-]{35}$`).
const MEDIA_ID_LENGTH: usize = 36;

/// Media identifiers a request may carry, spanning the forms that one shape admits: the
/// UUID the member's own documentation names, 36 characters carrying no separator at all,
/// the separators the pattern's tail admits, and an id spelt in upper case. None of them is
/// an id this emulator ever handed out, because no result of it carries one.
fn media_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "a UUID, the form the member documents",
            "7b5d3c1a-9e42-4f08-b1d6-5a2c8e0f31ab".to_string(),
        ),
        (
            "36 characters carrying no separator",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8".to_string(),
        ),
        (
            "one character followed by separators only",
            format!("a{}", "-".repeat(MEDIA_ID_LENGTH - 1)),
        ),
        (
            "an identifier spelt in upper case",
            "7B5D3C1A9E424F08B1D65A2C8E0F31ABCDEF".to_string(),
        ),
    ]
}

/// The documents the rules are read over: every way a document's bytes reach a knowledge
/// base, an image among them — `(document id, the document as the ingestion takes it in)`.
fn documents() -> Vec<(&'static str, Value)> {
    vec![
        (
            "media-doc-in-s3",
            s3_document("media-doc-in-s3", "media/handbook.pdf"),
        ),
        (
            "media-doc-inline-text",
            inline_text_document("media-doc-inline-text", "the pump is serviced every 90 days"),
        ),
        (
            "media-doc-inline-pdf",
            inline_byte_document(
                "media-doc-inline-pdf",
                "application/pdf",
                &[0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x00, 0xff],
            ),
        ),
        (
            "media-doc-inline-image",
            inline_byte_document(
                "media-doc-inline-image",
                "image/png",
                &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
            ),
        ),
    ]
}

/// A knowledge base with one custom data source holding [`documents`], ready to be read.
async fn seeded(emulator: &Emulator, name: &str) -> Ingested {
    let source = emulator.custom_source(name).await;
    emulator.put_object(
        "media/handbook.pdf",
        b"%PDF-1.7 handbook",
        Some("application/pdf"),
    );
    for (document, ingested) in documents() {
        emulator
            .ingest_documents(document, &source, json!([ingested]))
            .await;
    }
    source
}

/// RULE: a read naming an image embedded in a document is answered for the media that was
/// not found — this emulator records no image inside any document it holds — and reports no
/// address at all, while the same read naming no media is served.
///
/// The rows cross every form of media identifier with both `outputFormat` legs a request
/// naming one may carry, over every way a document's bytes reached the knowledge base.
#[tokio::test]
async fn a_read_naming_an_embedded_image_answers_not_found_for_that_media() {
    let emulator = Emulator::new();
    let source = seeded(&emulator, "embedded-media").await;

    // The legs a request naming a media id may carry: no rendering asked for, and the one
    // rendering the model lets it ask for.
    let formats: [(&str, Option<Value>); 2] = [
        ("no outputFormat", None),
        (
            "outputFormat RAW",
            Some(json!({ "outputFormat": OUTPUT_FORMAT_RAW })),
        ),
    ];

    for (document, _) in documents() {
        // The same read naming NO media is served, which is what makes every refusal below
        // about the image rather than about the document.
        let whole = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                document,
                None,
            )
            .await;
        assert_eq!(
            whole.status,
            200,
            "case {document:?}: expected the read naming no media to be served, got {} {:?}",
            whole.status,
            whole.json(document)
        );

        for (form, media_id) in media_ids() {
            assert_eq!(
                media_id.chars().count(),
                MEDIA_ID_LENGTH,
                "case {form:?}: the suite's media id must be one the model admits, got {media_id:?}"
            );
            for (leg, format) in &formats {
                let case = format!("{document} with {form} and {leg}");
                let mut body = json!({ "mediaId": media_id.clone() });
                if let Some(Value::Object(fields)) = format {
                    for (key, value) in fields {
                        body[key] = value.clone();
                    }
                }
                let reply = emulator
                    .document_content(
                        ACCOUNT_KEY,
                        &source.knowledge_base_id,
                        &source.data_source_id,
                        document,
                        Some(body),
                    )
                    .await;
                assert_eq!(
                    (reply.error_code(&case).as_str(), reply.status),
                    NOT_FOUND,
                    "case {case:?}: expected {NOT_FOUND:?} for an image no document holds, got {} {:?}",
                    reply.status,
                    reply.json(&case)
                );
                assert!(
                    reply.message(&case).contains(&media_id),
                    "case {case:?}: the refusal must name the media that was not found, got {:?}",
                    reply.message(&case)
                );
                let answer = reply.json(&case);
                assert!(
                    answer.get("presignedUrl").is_none() && answer.get("mimeType").is_none(),
                    "case {case:?}: a refusal must carry no address of the document, got {answer:?}"
                );
            }
        }
    }
}

/// RULE: a request naming an image asks for that image instead of the document, so it must
/// omit `outputFormat` or set it to `RAW`; any other rendering beside a media id is the
/// modelled `ValidationException` naming both members. The contrast rows are the same
/// request without the media id — refused for the rendering alone — and the two legs the
/// pair admits, which get the media's own answer.
#[tokio::test]
async fn a_read_naming_an_embedded_image_may_ask_for_no_other_rendering_of_it() {
    let emulator = Emulator::new();
    let source = seeded(&emulator, "embedded-media-pairing").await;
    let (_, media_id) = media_ids()
        .into_iter()
        .next()
        .expect("the suite names media identifiers");

    /// One request, and the answer the pair of members earns it.
    struct Row {
        case: &'static str,
        body: Option<Value>,
        /// The error code and status, or `None` when the request is served.
        refused: Option<(&'static str, u16)>,
        /// The members the refusal must name.
        names: &'static [&'static str],
    }

    let rows = vec![
        Row {
            case: "a media id beside the rendering this service cannot produce",
            body: Some(json!({
                "mediaId": media_id.clone(),
                "outputFormat": OUTPUT_FORMAT_EXTRACTED,
            })),
            refused: Some(INVALID),
            names: &["mediaId", "outputFormat"],
        },
        Row {
            case: "that rendering alone, with no media id",
            body: Some(json!({ "outputFormat": OUTPUT_FORMAT_EXTRACTED })),
            refused: Some(INVALID),
            names: &["outputFormat"],
        },
        Row {
            case: "a media id beside the rendering the pair admits",
            body: Some(json!({
                "mediaId": media_id.clone(),
                "outputFormat": OUTPUT_FORMAT_RAW,
            })),
            refused: Some(NOT_FOUND),
            names: &[],
        },
        Row {
            case: "a media id with no rendering asked for",
            body: Some(json!({ "mediaId": media_id.clone() })),
            refused: Some(NOT_FOUND),
            names: &[],
        },
        Row {
            case: "neither member",
            body: None,
            refused: None,
            names: &[],
        },
        Row {
            case: "the rendering the pair admits, with no media id",
            body: Some(json!({ "outputFormat": OUTPUT_FORMAT_RAW })),
            refused: None,
            names: &[],
        },
    ];

    for row in rows {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                "media-doc-in-s3",
                row.body.clone(),
            )
            .await;
        match row.refused {
            Some((code, status)) => {
                assert_eq!(
                    (reply.error_code(row.case).as_str(), reply.status),
                    (code, status),
                    "case {:?}: expected {code} {status}, got {} {:?}",
                    row.case,
                    reply.status,
                    reply.json(row.case)
                );
            }
            None => assert_eq!(
                reply.status,
                200,
                "case {:?}: expected the document's own content, got {} {:?}",
                row.case,
                reply.status,
                reply.json(row.case)
            ),
        }
        for named in row.names {
            assert!(
                reply.message(row.case).contains(named),
                "case {:?}: the refusal must name {named:?}, got {:?}",
                row.case,
                reply.message(row.case)
            );
        }
    }
}

/// RULE: a media identifier outside the class the model publishes for it never reaches the
/// service: the boundary refuses it with the modelled `ValidationException` naming the
/// member. The rows sit on the published length (36, which reaches the service and earns
/// the media's own not-found) and one character either side of it, plus the two ways the
/// pattern can be broken at the published length.
#[tokio::test]
async fn a_media_identifier_outside_its_published_class_is_refused_at_the_boundary() {
    let emulator = Emulator::new();
    let source = seeded(&emulator, "embedded-media-bounds").await;

    // (case, the identifier the request carries, the error code and status it earns)
    let rows: Vec<(&str, String, (&str, u16))> = vec![
        (
            "one character short of the published length",
            "a".repeat(MEDIA_ID_LENGTH - 1),
            INVALID,
        ),
        (
            "the published length",
            "a".repeat(MEDIA_ID_LENGTH),
            NOT_FOUND,
        ),
        (
            "one character past the published length",
            "a".repeat(MEDIA_ID_LENGTH + 1),
            INVALID,
        ),
        (
            "the published length, opening with a separator",
            format!("-{}", "a".repeat(MEDIA_ID_LENGTH - 1)),
            INVALID,
        ),
        (
            "the published length, carrying a character the class excludes",
            format!("a_{}", "a".repeat(MEDIA_ID_LENGTH - 2)),
            INVALID,
        ),
        ("no identifier at all", String::new(), INVALID),
    ];

    for (case, media_id, (code, status)) in rows {
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                "media-doc-inline-text",
                Some(json!({ "mediaId": media_id })),
            )
            .await;
        assert_eq!(
            (reply.error_code(case).as_str(), reply.status),
            (code, status),
            "case {case:?}: expected {code} {status} for {media_id:?}, got {} {:?}",
            reply.status,
            reply.json(case)
        );
        assert!(
            reply.message(case).contains("mediaId") || code == NOT_FOUND.0,
            "case {case:?}: the refusal must name the member, got {:?}",
            reply.message(case)
        );
    }
}

/// RULE: the resources the PATH names answer for themselves before any image inside the
/// document is read, and the authorization decision comes before all of it: a knowledge
/// base, data source or document this emulator does not hold is reported as that missing
/// resource whatever media the request named, and a caller granted nothing is refused
/// `AccessDeniedException` either way.
#[tokio::test]
async fn the_documents_own_resources_answer_before_any_image_of_it_is_read() {
    let emulator = Emulator::new();
    let source = seeded(&emulator, "embedded-media-resolution").await;
    let (_, media_id) = media_ids()
        .into_iter()
        .next()
        .expect("the suite names media identifiers");

    /// One read naming an image of something the plane does not hold, and the identifier
    /// the refusal has to name — the resource that was missing, not the media.
    struct Row {
        case: &'static str,
        knowledge_base: Option<&'static str>,
        data_source: Option<&'static str>,
        document: &'static str,
        names: &'static str,
    }

    let rows = [
        Row {
            case: "a knowledge base that was never created",
            knowledge_base: Some(UNKNOWN_KNOWLEDGE_BASE),
            data_source: None,
            document: "media-doc-in-s3",
            names: UNKNOWN_KNOWLEDGE_BASE,
        },
        Row {
            case: "a data source that was never created",
            knowledge_base: None,
            data_source: Some(UNKNOWN_DATA_SOURCE),
            document: "media-doc-in-s3",
            names: UNKNOWN_DATA_SOURCE,
        },
        Row {
            case: "a document that was never ingested",
            knowledge_base: None,
            data_source: None,
            document: "media-doc-never-ingested",
            names: "media-doc-never-ingested",
        },
    ];

    for row in rows {
        let knowledge_base = row.knowledge_base.unwrap_or(&source.knowledge_base_id);
        let data_source = row.data_source.unwrap_or(&source.data_source_id);
        let reply = emulator
            .document_content(
                ACCOUNT_KEY,
                knowledge_base,
                data_source,
                row.document,
                Some(json!({ "mediaId": media_id.clone() })),
            )
            .await;
        assert_eq!(
            (reply.error_code(row.case).as_str(), reply.status),
            NOT_FOUND,
            "case {:?}: expected {NOT_FOUND:?}, got {} {:?}",
            row.case,
            reply.status,
            reply.json(row.case)
        );
        assert!(
            reply.message(row.case).contains(row.names),
            "case {:?}: the refusal must name the resource that was missing, got {:?}",
            row.case,
            reply.message(row.case)
        );
        assert!(
            !reply.message(row.case).contains(&media_id),
            "case {:?}: the refusal must not report on the media, got {:?}",
            row.case,
            reply.message(row.case)
        );
    }

    let unpermitted = emulator
        .assumed_role_key("embedded-media-nothing", None)
        .await;
    let permitted = emulator
        .assumed_role_key(
            "embedded-media-reader",
            Some(&allow(&["bedrock:GetDocumentContent"])),
        )
        .await;
    for document in ["media-doc-in-s3", "media-doc-never-ingested"] {
        let denied = emulator
            .document_content(
                &unpermitted,
                &source.knowledge_base_id,
                &source.data_source_id,
                document,
                Some(json!({ "mediaId": media_id.clone() })),
            )
            .await;
        assert_eq!(
            (denied.error_code(document).as_str(), denied.status),
            ACCESS_DENIED,
            "case {document:?}: expected {ACCESS_DENIED:?} for a caller granted nothing, got {} {:?}",
            denied.status,
            denied.json(document)
        );
        let reached = emulator
            .document_content(
                &permitted,
                &source.knowledge_base_id,
                &source.data_source_id,
                document,
                Some(json!({ "mediaId": media_id.clone() })),
            )
            .await;
        assert_eq!(
            (reached.error_code(document).as_str(), reached.status),
            NOT_FOUND,
            "case {document:?}: expected the permitted caller to reach the read, got {} {:?}",
            reached.status,
            reached.json(document)
        );
    }
}

/// RULE: a read reports; it changes nothing a later caller sees. A read naming an image
/// leaves the document exactly as it was: the next read of it reports the same content and
/// the address it hands out still serves the ingested bytes.
#[tokio::test]
async fn reading_an_embedded_image_changes_nothing_a_later_read_sees() {
    let emulator = Emulator::new();
    let source = seeded(&emulator, "embedded-media-reads").await;

    for (document, _) in documents() {
        let before = emulator
            .document_content(
                ACCOUNT_KEY,
                &source.knowledge_base_id,
                &source.data_source_id,
                document,
                None,
            )
            .await;
        assert_eq!(
            before.status,
            200,
            "case {document:?}: expected the first read to be served, got {} {:?}",
            before.status,
            before.json(document)
        );
        let first = before.json(document);

        for (form, media_id) in media_ids() {
            let case = format!("{document} after {form}");
            let refused = emulator
                .document_content(
                    ACCOUNT_KEY,
                    &source.knowledge_base_id,
                    &source.data_source_id,
                    document,
                    Some(json!({ "mediaId": media_id })),
                )
                .await;
            assert_eq!(
                refused.status,
                NOT_FOUND.1,
                "case {case:?}: expected the media read to be refused, got {} {:?}",
                refused.status,
                refused.json(&case)
            );

            let after = emulator
                .document_content(
                    ACCOUNT_KEY,
                    &source.knowledge_base_id,
                    &source.data_source_id,
                    document,
                    None,
                )
                .await;
            assert_eq!(
                after.status,
                200,
                "case {case:?}: expected the later read to be served, got {} {:?}",
                after.status,
                after.json(&case)
            );
            let answer = after.json(&case);
            assert_eq!(
                (
                    answer["mimeType"].clone(),
                    answer["documentContentLength"].clone()
                ),
                (
                    first["mimeType"].clone(),
                    first["documentContentLength"].clone()
                ),
                "case {case:?}: expected the content the first read reported, got {answer:?}"
            );
            let url = answer["presignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no presignedUrl in {answer:?}"));
            let (status, bytes) = emulator.follow(&case, url).await;
            assert_eq!(
                status, 200,
                "case {case:?}: following the address must still reach the content, got {status}"
            );
            assert!(
                !bytes.is_empty(),
                "case {case:?}: expected the ingested bytes to still be served"
            );
        }
    }
}
