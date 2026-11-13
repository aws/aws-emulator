// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a `versionId` is CLASSIFIED: whether the request named
//! something that could be a version id at all, judged before whether a version of that
//! name exists.
//!
//! The rule under test: a `versionId` that is not shaped like a version id S3 issues — or
//! that carries the one VALUE no issuer mints, the zero token — cannot name a version of any
//! object in any bucket, so it is a bad ARGUMENT: HTTP 400 `InvalidArgument` with `Invalid
//! version id specified`, naming the `versionId` parameter and echoing the token it
//! rejected. One that IS shaped like one and names no stored version is HTTP 404
//! `NoSuchVersion` with `The specified version does not exist.` The shape is the published
//! one: the literal `null`, or a token between `state::VERSION_ID_CHARS` and
//! `state::MAX_VERSION_ID_CHARS` characters, each an ASCII alphanumeric or one of `-`, `_`,
//! `.`.
//!
//! The same classification is asserted on the single-object READS (`GetObject`,
//! `HeadObject`), which is where both halves of it are visible. `DeleteObject` is held to
//! the ARGUMENT half alone: it models no error at all, so an address holding nothing is no
//! failure for it and the absent-version row is its modelled success (HTTP 204, empty body),
//! while a token that is not shaped like a version id is still the refused argument — the
//! classification that separates "fix the request" from "that version is gone" survives in
//! the one operation whose absent case is a no-op. The `DeleteObjects` batch is held to the
//! other half of the rule: "DeleteObjects itself has no modeled top-level errors; each result
//! entry carries Key, VersionId, Code, Message", and what an entry reports is what the batch
//! could not do with that pair — a version id it cannot resolve names no version of the key,
//! so the entry is the absent-version failure whatever the token's shape.

use service_com_amazonaws_s3_amazons3::state::{MAX_VERSION_ID_CHARS, VERSION_ID_CHARS};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, head_object, set_versioning};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The key every row addresses.
const KEY: &str = "classified.bin";

/// What a classification row expects: the wire error code and the message beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Expected {
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// A `versionId` that is not shaped like one: HTTP 400, `InvalidArgument`.
const MALFORMED: Expected = Expected {
    status: 400,
    code: "InvalidArgument",
    message: "Invalid version id specified",
};

/// A `versionId` of the published shape that names no stored version: HTTP 404.
const ABSENT: Expected = Expected {
    status: 404,
    code: "NoSuchVersion",
    message: "The specified version does not exist.",
};

/// What `DeleteObject` answers that same absent version with instead: its modelled success,
/// HTTP 204 with no body at all, because the operation models no error for an address holding
/// nothing (`smithy_query(query="operation",operation="DeleteObject")` reports `errors: []`
/// and `code: 204`).
const DELETED: u16 = 204;

/// `POST /<bucket>?delete` — `DeleteObjects`, naming each `(key, versionId)` pair.
async fn delete_objects(
    router: &MultiServiceRouter,
    bucket: &str,
    identifiers: &[(&str, &str)],
) -> Answer {
    let mut body = format!("<Delete xmlns=\"{XMLNS}\">");
    for (key, version_id) in identifiers {
        body.push_str(&format!(
            "<Object><Key>{key}</Key><VersionId>{version_id}</VersionId></Object>"
        ));
    }
    body.push_str("</Delete>");
    send(router, "POST", &format!("/{bucket}?delete"), &[], &body).await
}

/// The `<Code>` and `<Message>` of the one `<Error>` entry a `DeleteObjects` answer
/// carries, or `None` when it reports no error at all.
fn entry_error(answer: &Answer) -> Option<(String, String)> {
    let body = String::from_utf8_lossy(&answer.body).to_string();
    let (_, rest) = body.split_once("<Error>")?;
    let (entry, _) = rest.split_once("</Error>")?;
    let code = entry
        .split_once("<Code>")
        .and_then(|(_, rest)| rest.split_once("</Code>"))
        .map(|(code, _)| code.to_string())?;
    let message = entry
        .split_once("<Message>")
        .and_then(|(_, rest)| rest.split_once("</Message>"))
        .map(|(message, _)| message.to_string())?;
    Some((code, message))
}

/// The ids every row of the table is built from, paired with how each must be classified.
///
/// The rows differ along what the shape rule generalises over: LENGTH inside the published
/// range (its minimum, a width between the bounds, its maximum) and outside it at both ends
/// (one character, one short of the minimum, one past the maximum), the ALPHABET at a width
/// inside the range (the alphanumerics and the `-`, `_`, `.` the shape holds, against the
/// characters it does not — standard Base64's `+` and `/`, and an ASCII punctuation mark),
/// the CASE of the characters, which the shape does not restrict, and the one token whose
/// VALUE no issuer mints: the zero one, written as nothing but the alphabet's zero digit.
fn rows() -> Vec<(String, String, Expected)> {
    let min = VERSION_ID_CHARS;
    let max = MAX_VERSION_ID_CHARS;
    let between = min + 4;
    let mut rows: Vec<(String, String, Expected)> = Vec::new();

    // Shaped like a version id, and naming nothing: every one of these is a 404.
    for (case, id) in [
        ("at the minimum width", "a".repeat(min)),
        ("between the published widths", "b".repeat(between)),
        ("at the maximum width", "c".repeat(max)),
        ("upper case", format!("B{}", "A".repeat(min - 1))),
        ("every character a digit", "0".repeat(min)),
        (
            "mixed case and digits",
            (0..min)
                .map(|index| {
                    const ALPHABET: &[u8] = b"aB3dE6gH9jK2mN5pQ8sT1vW4xY7zA0bC";
                    char::from(ALPHABET[index % ALPHABET.len()])
                })
                .collect::<String>(),
        ),
        (
            "carrying each character the shape holds beside the alphanumerics",
            format!("{}-_.", "d".repeat(min - 3)),
        ),
        (
            // The published example of a version id, which is what a client that stored one
            // against the real service hands back.
            "the published example of an issued id",
            "3HL4kqtJlcpXroDTDmJ_RGrmHJ1PyFCY".to_string(),
        ),
    ] {
        let width = id.chars().count();
        assert!(
            (min..=max).contains(&width),
            "case {case:?}: every well-formed row must be within the published widths, got \
             {width}"
        );
        rows.push((format!("well formed, {case}"), id, ABSENT));
    }

    // Not shaped like a version id, or carrying the one value no issuer mints: every one of
    // these is a 400.
    let padded = "a".repeat(min);
    for (case, id) in [
        ("a single character", "x".to_string()),
        ("one character short of the minimum", padded[..min - 1].to_string()),
        ("one character past the maximum", "v".repeat(max + 1)),
        ("a word that is no id", "not-a-version".to_string()),
        (
            "standard Base64's plus",
            format!("{}+", &padded[..min - 1]),
        ),
        (
            "standard Base64's slash",
            format!("{}/", &padded[..min - 1]),
        ),
        (
            "an exclamation mark",
            format!("{}!", &padded[..min - 1]),
        ),
        ("the zero token, at the minimum width", "A".repeat(min)),
        ("the zero token, wider", "A".repeat(between)),
    ] {
        rows.push((format!("malformed, {case}"), id, MALFORMED));
    }

    rows
}

#[tokio::test]
async fn a_version_id_is_classified_the_same_way_wherever_it_arrives() {
    let router = router();
    let name = "version-id-classification";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;
    let written = put_object(&router, name, KEY, b"held", &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let minted = written
        .header("x-amz-version-id")
        .expect("a versioning-enabled bucket reports the version id it minted");
    assert_eq!(
        minted.chars().count(),
        VERSION_ID_CHARS,
        "a minted id must carry the published number of characters, got {minted:?}"
    );

    for (case, version_id, expected) in rows() {
        assert_ne!(
            version_id, minted,
            "case {case:?}: no row may name the id the store actually minted"
        );

        // `DeleteObject` takes the same `versionId`, and the ARGUMENT half of the rule is
        // its too: a malformed token is the same 400, while the well-formed token naming
        // nothing is the delete's own no-op success.
        let deleted = delete_object(&router, name, KEY, &format!("?versionId={version_id}")).await;
        if expected == ABSENT {
            assert_eq!(
                deleted.status, DELETED,
                "case {case:?}/DeleteObject: a well-formed id naming no version is deleted \
                 rather than refused, got {} with body {:?}",
                deleted.status,
                text(&deleted)
            );
            assert!(
                deleted.body.is_empty(),
                "case {case:?}/DeleteObject: that success carries no body, got {:?}",
                text(&deleted)
            );
        } else {
            assert_eq!(
                deleted.status, expected.status,
                "case {case:?}/DeleteObject: expected HTTP {} for {version_id:?}, got {} with \
                 body {:?}",
                expected.status,
                deleted.status,
                text(&deleted)
            );
            assert_eq!(
                deleted.error_code(),
                expected.code,
                "case {case:?}/DeleteObject: expected {:?}, got {:?}",
                expected.code,
                deleted.error_code()
            );
            assert_eq!(
                deleted.error_message(),
                expected.message,
                "case {case:?}/DeleteObject: expected the message {:?}, got {:?}",
                expected.message,
                deleted.error_message()
            );
        }

        // The single-object READS: both halves of the classification, on each operation that
        // takes a `versionId`.
        for (operation, answer) in [
            (
                "GetObject",
                get_object(&router, name, KEY, &format!("?versionId={version_id}"), &[]).await,
            ),
            (
                "HeadObject",
                head_object(&router, name, KEY, &format!("?versionId={version_id}")).await,
            ),
        ] {
            assert_eq!(
                answer.status, expected.status,
                "case {case:?}/{operation}: expected HTTP {} for {version_id:?}, got {} with \
                 body {:?}",
                expected.status,
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                expected.code,
                "case {case:?}/{operation}: expected {:?}, got {:?}",
                expected.code,
                answer.error_code()
            );
            // HEAD carries no body, so only the two that do are checked for the message.
            if operation != "HeadObject" {
                assert_eq!(
                    answer.error_message(),
                    expected.message,
                    "case {case:?}/{operation}: expected the message {:?}, got {:?}",
                    expected.message,
                    answer.error_message()
                );
            }
        }

        // The batch path: the same id, as one entry of a `DeleteObjects` request. The
        // request itself succeeds — the classification is that ENTRY's own error.
        let batch = delete_objects(&router, name, &[(KEY, &version_id)]).await;
        assert_eq!(
            batch.status, 200,
            "case {case:?}/DeleteObjects: the batch itself answers 200, got {} with body {:?}",
            batch.status,
            text(&batch)
        );
        let reported = entry_error(&batch);
        assert_eq!(
            reported,
            Some((ABSENT.code.to_string(), ABSENT.message.to_string())),
            "case {case:?}/DeleteObjects: the entry must report the version as absent, \
             whatever the token's shape, got {reported:?} in body {:?}",
            text(&batch)
        );
    }

    // The id the store DID mint is served throughout, so every row above failed on what it
    // named rather than on the `versionId` parameter being refused outright.
    let read = get_object(&router, name, KEY, &format!("?versionId={minted}"), &[]).await;
    assert_eq!(
        read.status, 200,
        "the minted version must still be readable, got {} with body {:?}",
        read.status,
        text(&read)
    );
    let batch = delete_objects(&router, name, &[(KEY, &minted)]).await;
    assert_eq!(
        entry_error(&batch),
        None,
        "the minted version must be deleted rather than reported as an error, got body {:?}",
        text(&batch)
    );

    // And once that version is gone, the very same id is the ABSENT case rather than the
    // malformed one: the two conditions are kept apart by the id's SHAPE, not by its
    // history.
    let gone = get_object(&router, name, KEY, &format!("?versionId={minted}"), &[]).await;
    assert_eq!(
        gone.error_code(),
        ABSENT.code,
        "a deleted version is reported absent, got {:?} with status {}",
        gone.error_code(),
        gone.status
    );
    assert_eq!(
        gone.error_message(),
        ABSENT.message,
        "a deleted version carries the absent message, got {:?}",
        gone.error_message()
    );
}
