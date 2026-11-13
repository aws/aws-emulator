// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ceiling a .zip archive's CONTENTS are held to.
//!
//! The rule under test: a .zip archive is held to the published unzipped quota
//! (`AccountLimit$CodeSizeUnzipped`, "250 MB (unzipped)") on what it EXPANDS to rather than
//! on how long the stored archive is, so an archive whose entries declare that much or more
//! is refused — with the sentence naming the ceiling it was measured against — and nothing
//! is stored. It binds every write that stores one: the two that store a function's
//! deployment package, and the one that stores a layer version's archive.
//!
//! Each archive below is written in memory from entries of zeros, so an archive that expands
//! to a quarter of a gigabyte is a few hundred kilobytes on the wire — the shape the report
//! this suite pins came from. Every accepted write is read back through `GetFunction` or
//! `GetLayerVersion`, and every refused one is checked to have stored nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::CODE_SIZE_UNZIPPED_LIMIT;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_update_due, package_bytes, router_on, zip_file, Answer,
    ROLE,
};

/// The modelled 400 a package Lambda will not store is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// How large one entry of a written archive is, so a 250 MB archive is a handful of entries
/// rather than one allocation of that size.
const ENTRY_BYTES: i64 = 64 * 1024 * 1024;

/// How many bytes are written into an entry at a time.
const CHUNK_BYTES: usize = 1024 * 1024;

/// The body of an answer, for a failure message.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// The `message` of a wire error, however the protocol spells the member.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// The sentence a package past the ceiling is refused with, built from the published quota
/// the service enforces rather than from a recorded run.
fn over_the_ceiling_message() -> String {
    format!("Unzipped size must be smaller than {CODE_SIZE_UNZIPPED_LIMIT} bytes")
}

/// A .zip archive whose entries declare `expanded` bytes of contents in total.
///
/// The contents are zeros, written a chunk at a time and compressed, so the archive this
/// returns is small however much it expands to — which is exactly what makes the stored
/// length no measure of the expanded one.
fn archive_expanding_to(expanded: i64) -> Vec<u8> {
    let zeros = vec![0_u8; CHUNK_BYTES];
    let mut buffer = ::std::io::Cursor::new(Vec::new());
    {
        let mut writer = ::zip::ZipWriter::new(&mut buffer);
        let options: ::zip::write::FileOptions<'_, '_, ()> = ::zip::write::FileOptions::default()
            .compression_method(::zip::CompressionMethod::Deflated);
        let mut remaining = expanded;
        let mut index = 0_u32;
        while remaining > 0 {
            let entry = remaining.min(ENTRY_BYTES);
            writer
                .start_file(format!("payload-{index}.bin"), options)
                .expect("a new entry");
            let mut left = usize::try_from(entry).expect("an entry that fits in memory");
            while left > 0 {
                let take = left.min(CHUNK_BYTES);
                ::std::io::Write::write_all(&mut writer, &zeros[..take])
                    .expect("the entry's bytes");
                left -= take;
            }
            remaining -= entry;
            index += 1;
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create one function from the deployment package `archive`.
async fn create_from(router: &MultiServiceRouter, name: &str, archive: &[u8]) -> Answer {
    let body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(archive) },
    });
    create_function(router, &body).await
}

/// `UpdateFunctionCode` with the deployment package `archive`.
async fn update_code(router: &MultiServiceRouter, name: &str, archive: &[u8]) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/code");
    json_call(router, "PUT", &uri, &json!({ "ZipFile": zip_file(archive) })).await
}

/// `PublishLayerVersion` with the layer archive `archive`.
async fn publish_layer(router: &MultiServiceRouter, name: &str, archive: &[u8]) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(
        router,
        "POST",
        &uri,
        &json!({ "Content": { "ZipFile": zip_file(archive) } }),
    )
    .await
}

/// `GetFunction` on one function name.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetLayerVersion` on one layer name and version number.
async fn get_layer_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// One archive the rule is asserted over.
struct Case {
    /// The case name, stated in every assertion.
    case: &'static str,
    /// The resource name the row's write is made under.
    name: &'static str,
    /// How many bytes the archive's entries declare.
    expanded: i64,
    /// Whether a write naming it is accepted.
    accepted: bool,
}

/// The archives the rule is asserted over: an ordinary package, the published bound and one
/// byte under it — the refusal states that a package has to be SMALLER than the ceiling —
/// and one well past it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "an ordinary package",
            name: "ordinary-package",
            expanded: 4 * 1024,
            accepted: true,
        },
        Case {
            case: "one byte under the ceiling",
            name: "one-byte-under",
            expanded: CODE_SIZE_UNZIPPED_LIMIT - 1,
            accepted: true,
        },
        Case {
            case: "the ceiling itself",
            name: "the-ceiling-itself",
            expanded: CODE_SIZE_UNZIPPED_LIMIT,
            accepted: false,
        },
        Case {
            case: "a mebibyte past the ceiling",
            name: "a-mebibyte-past",
            expanded: CODE_SIZE_UNZIPPED_LIMIT + 1024 * 1024,
            accepted: false,
        },
    ]
}

/// Assert `answered` is the refusal a package past the ceiling gets.
fn assert_over_the_ceiling(case: &str, operation: &str, answered: &Answer) {
    assert_eq!(
        (answered.status, answered.error_code()),
        (400, INVALID_PARAMETER.to_string()),
        "case {case:?} ({operation}): expected 400 {INVALID_PARAMETER}, got {} {}",
        answered.status,
        body_text(answered)
    );
    let expected = over_the_ceiling_message();
    let reported = message_of(answered);
    assert_eq!(
        reported, expected,
        "case {case:?} ({operation}): expected the message {expected:?}, got {reported:?}"
    );
}

/// RULE: a deployment package is held to the published unzipped ceiling on what it expands
/// to, so an archive declaring that much is refused and no function is stored, while one
/// declaring a byte less is stored and read back.
#[tokio::test]
async fn a_deployment_package_is_held_to_the_unzipped_ceiling_on_what_it_expands_to() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in cases() {
        let archive = archive_expanding_to(case.expanded);
        let answered = create_from(&router, case.name, &archive).await;
        let read = get_function(&router, case.name).await;
        if case.accepted {
            assert_eq!(
                answered.status,
                201,
                "case {:?}: expected 201 from CreateFunction, got {} {}",
                case.case,
                answered.status,
                body_text(&answered)
            );
            // The stored package is the archive that was uploaded, so the function reports
            // the archive's own length rather than what it expands to.
            let stored = i64::try_from(archive.len()).expect("an archive that fits in memory");
            assert_eq!(
                read.json()["Configuration"]["CodeSize"],
                json!(stored),
                "case {:?}: expected GetFunction to report CodeSize {stored}, got {}",
                case.case,
                body_text(&read)
            );
        } else {
            assert_over_the_ceiling(case.case, "CreateFunction", &answered);
            assert_eq!(
                read.status,
                404,
                "case {:?}: expected the refused create to store no function, got {} {}",
                case.case,
                read.status,
                body_text(&read)
            );
        }
    }
}

/// RULE (the same one, on the other two writes that store an archive): the ceiling binds
/// `UpdateFunctionCode` and `PublishLayerVersion` too, and a refused write leaves the
/// package the function already had.
#[tokio::test]
async fn every_write_that_stores_an_archive_is_held_to_the_unzipped_ceiling() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let original = package_bytes(128);
    let created = create_from(&router, "ceiling-fn", &original).await;
    assert_eq!(
        created.status,
        201,
        "setup: expected 201 from CreateFunction, got {} {}",
        created.status,
        body_text(&created)
    );
    // "During this time, you can't invoke or modify the function": the update below is of
    // a function whose provisioning has finished, which is what waiting for it would give.
    make_update_due(&engine);

    for case in cases().into_iter().filter(|case| !case.accepted) {
        make_update_due(&engine);
        let archive = archive_expanding_to(case.expanded);

        let updated = update_code(&router, "ceiling-fn", &archive).await;
        assert_over_the_ceiling(case.case, "UpdateFunctionCode", &updated);
        let read = get_function(&router, "ceiling-fn").await;
        let kept = i64::try_from(original.len()).expect("a package that fits in memory");
        assert_eq!(
            read.json()["Configuration"]["CodeSize"],
            json!(kept),
            "case {:?}: expected the function to keep its CodeSize {kept}, got {}",
            case.case,
            body_text(&read)
        );

        let published = publish_layer(&router, case.name, &archive).await;
        assert_over_the_ceiling(case.case, "PublishLayerVersion", &published);
        let layer = get_layer_version(&router, case.name, 1).await;
        assert_eq!(
            layer.status,
            404,
            "case {:?}: expected the refused publish to store no layer version, got {} {}",
            case.case,
            layer.status,
            body_text(&layer)
        );
    }

    // And an archive inside the ceiling still publishes, so the rule refuses nothing it
    // should not.
    let inside = archive_expanding_to(4 * 1024);
    let published = publish_layer(&router, "inside-the-ceiling", &inside).await;
    assert_eq!(
        published.status,
        201,
        "expected 201 from PublishLayerVersion for an archive inside the ceiling, got {} {}",
        published.status,
        body_text(&published)
    );
    let layer = get_layer_version(&router, "inside-the-ceiling", 1).await;
    assert_eq!(
        layer.status,
        200,
        "expected the published version to be readable, got {} {}",
        layer.status,
        body_text(&layer)
    );
}
