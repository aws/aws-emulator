// SPDX-License-Identifier: CC0-1.0

//! An object-scoped refusal names the OBJECT it is about: `<Key>`, and `<VersionId>` when
//! the request named a version.
//!
//! The rule under test: S3's error document is
//! `<Error><Code/><Message/><the condition's own members/><RequestId/><HostId/></Error>`
//! (Amazon S3 API Reference, *Error responses*), and the condition a 404 about an object
//! states is a condition of ONE address — the key holds no current object, or the bucket
//! holds no such version of it — so the document carries that address: the key the request
//! addressed, decoded and whole, and the version id it named. The members cannot come from
//! the model, which declares none at all for these shapes
//! (`smithy_query(query="shape",shape="NoSuchKey",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
//! reports no members), so they are written from the request's own address — which is why
//! the rule has to hold for EVERY operation that answers one of those codes, not only for
//! the handlers that spell it out. A client that fans one operation out over many keys reads
//! which one failed from there, and an SDK error object surfaces it as `Key`.
//!
//! The tables span what that rule generalises over:
//!
//! * the OPERATION that refuses — every object sub-resource read and write this emulator
//!   serves, the object read itself, the restore, the version-scoped delete and a COPY
//!   (whose refusal is about its SOURCE rather than about the object the request addressed);
//! * whether the request named a VERSION, which adds `<VersionId>` and moves the code from
//!   the key's own not-found to the version's;
//! * how the key is SPELLED on the wire — several path segments, a trailing slash, an
//!   encoded separator, an encoded space, XML metacharacters and non-ASCII text — since the
//!   member carries the key the request addressed, decoded, and the document has to escape
//!   whatever the caller chose;
//! * which ADDRESS FORM the request used, path-style here and virtual-hosted in
//!   [`super::zz_host_style_probe`], because the key a virtual-hosted request addresses is
//!   its whole path and the member is the same either way;
//! * and a key a DELETE MARKER hides, which is a 404 whose only other distinguishing marks
//!   are the two headers the marker is reported in (the headers themselves are pinned by
//!   [`super::delete_marker_refusal_headers`]).
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! document rather than any handler's internals.

use service_com_amazonaws_s3_amazons3::state::{
    NO_SUCH_KEY_CODE, NO_SUCH_KEY_MESSAGE, NO_SUCH_VERSION_CODE, NO_SUCH_VERSION_MESSAGE,
    VERSION_ID_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::allow_public_access;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, set_versioning};

/// The bytes every object in this suite holds: the rule is about the address a refusal names,
/// not about what is stored at it, so one payload serves every row.
const BODY: &[u8] = b"object-scoped-envelope-subject";

/// The key each bucket in this suite holds, so a refusal about another key is about a key the
/// bucket really does not hold. It carries a separator, because the member carries the key
/// WHOLE.
///
/// `pub(crate)`, like the three items below it: the virtual-hosted half of the same table
/// ([`super::zz_host_style_probe`]) drives the same rule through the other address form and
/// reads it from here rather than standing up a second setup.
pub(crate) const HELD_KEY: &str = "ledger/held.bin";

/// The `Tagging` document a tagging write carries — the smallest well-formed one, since what
/// is under test is the refusal and not the tag set.
const TAGGING_BODY: &str = "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
     <TagSet><Tag><Key>stage</Key><Value>probe</Value></Tag></TagSet></Tagging>";

/// The `RestoreRequest` document a restore carries, for the same reason.
const RESTORE_BODY: &str = "<RestoreRequest xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
     <Days>1</Days></RestoreRequest>";

/// One row of the operation table: the case, the request's method, what follows the bucket in
/// its URI, the headers and body it carries, and the code, key and version its answer must
/// name.
struct Row {
    case: &'static str,
    method: &'static str,
    suffix: String,
    headers: Vec<(&'static str, &'static str)>,
    body: &'static str,
    code: &'static str,
    key: String,
    version_id: Option<String>,
}

/// A bucket holding [`HELD_KEY`], with versioning on and with the two bucket settings an ACL
/// write needs: ACLs are refused outright under the bucket-owner-enforced setting every new
/// bucket carries, and a canned ACL is refused by Block Public Access, so neither refusal
/// could be told from the not-found under test.
pub(crate) async fn bucket_holding(router: &MultiServiceRouter, name: &str) {
    bucket(router, name).await;
    let enabled = put_ownership_controls(router, name, &["ObjectWriter"]).await;
    assert_eq!(
        enabled.status, 200,
        "the bucket {name:?} must take the ownership setting that enables ACLs, got {} with \
         body {:?}",
        enabled.status,
        text(&enabled)
    );
    allow_public_access(router, name).await;
    set_versioning(router, name, "Enabled").await;
    let written = put_object(router, name, HELD_KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object {HELD_KEY:?} must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
}

/// Dispatch one row and report what it answered.
async fn send_row(router: &MultiServiceRouter, name: &str, row: &Row) -> Answer {
    send(
        router,
        row.method,
        &format!("/{name}/{}", row.suffix),
        &row.headers,
        row.body,
    )
    .await
}

/// Every element of an object-scoped refusal, asserted together: the status the code carries,
/// the code, the fixed sentence of that code, the key and the version.
pub(crate) fn assert_names_object(
    case: &str,
    answer: &Answer,
    code: &str,
    key: &str,
    version: Option<&str>,
) {
    let message = match code == NO_SUCH_VERSION_CODE {
        true => NO_SUCH_VERSION_MESSAGE,
        false => NO_SUCH_KEY_MESSAGE,
    };
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected the {code:?} code, got {:?} in {:?}",
        answer.error_code(),
        text(answer)
    );
    assert_eq!(
        answer.error_message(),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        answer.error_message()
    );
    assert_eq!(
        answer.element("Key").as_deref(),
        Some(key),
        "case {case:?}: the envelope must name the key the refusal is about, got {:?} in {:?}",
        answer.element("Key"),
        text(answer)
    );
    assert_eq!(
        answer.element("VersionId").as_deref(),
        version,
        "case {case:?}: the envelope must name the version the request addressed, and none \
         where it named none, got {:?} in {:?}",
        answer.element("VersionId"),
        text(answer)
    );
}

/// The rows: one per operation that answers an object-scoped 404, each in both the
/// version-less and (where the operation takes one) the version-named form.
fn operation_rows(absent_version: &str) -> Vec<Row> {
    // The key no bucket here holds. It carries a separator for the same reason the held key
    // does: the member must carry every segment of it.
    let absent = "ledger/absent.bin";
    let versioned = format!("?versionId={absent_version}");
    let mut rows = vec![
        Row {
            case: "GetObject",
            method: "GET",
            suffix: absent.to_string(),
            headers: Vec::new(),
            body: "",
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "GetObjectTagging",
            method: "GET",
            suffix: format!("{absent}?tagging"),
            headers: Vec::new(),
            body: "",
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "PutObjectTagging",
            method: "PUT",
            suffix: format!("{absent}?tagging"),
            headers: Vec::new(),
            body: TAGGING_BODY,
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "DeleteObjectTagging",
            method: "DELETE",
            suffix: format!("{absent}?tagging"),
            headers: Vec::new(),
            body: "",
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "GetObjectAcl",
            method: "GET",
            suffix: format!("{absent}?acl"),
            headers: Vec::new(),
            body: "",
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "PutObjectAcl",
            method: "PUT",
            suffix: format!("{absent}?acl"),
            headers: vec![("x-amz-acl", "private")],
            body: "",
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "RestoreObject",
            method: "POST",
            suffix: format!("{absent}?restore"),
            headers: Vec::new(),
            body: RESTORE_BODY,
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        Row {
            case: "GetObjectAttributes",
            method: "GET",
            suffix: format!("{absent}?attributes"),
            headers: vec![("x-amz-object-attributes", "ETag")],
            body: "",
            code: NO_SUCH_KEY_CODE,
            key: absent.to_string(),
            version_id: None,
        },
        // The same operations with a version the bucket holds no entry for: the code becomes
        // the version's own and the document names BOTH members.
        Row {
            case: "GetObject naming a version that is not there",
            method: "GET",
            suffix: format!("{HELD_KEY}{versioned}"),
            headers: Vec::new(),
            body: "",
            code: NO_SUCH_VERSION_CODE,
            key: HELD_KEY.to_string(),
            version_id: Some(absent_version.to_string()),
        },
        Row {
            case: "GetObjectAcl naming a version that is not there",
            method: "GET",
            suffix: format!("{HELD_KEY}?acl&versionId={absent_version}"),
            headers: Vec::new(),
            body: "",
            code: NO_SUCH_VERSION_CODE,
            key: HELD_KEY.to_string(),
            version_id: Some(absent_version.to_string()),
        },
        Row {
            case: "GetObjectAttributes naming a version that is not there",
            method: "GET",
            suffix: format!("{HELD_KEY}?attributes&versionId={absent_version}"),
            headers: vec![("x-amz-object-attributes", "ETag")],
            body: "",
            code: NO_SUCH_VERSION_CODE,
            key: HELD_KEY.to_string(),
            version_id: Some(absent_version.to_string()),
        },
    ];
    // The version-scoped DELETE is NOT among them: `DeleteObject` models no error at all, so
    // a version the bucket holds no entry for is its own no-op success rather than an
    // object-scoped refusal (asserted below, and spanned by
    // [`super::idempotent_object_deletes`]).
    // A key a request addresses under a version-less not-found, named a second way: the key
    // that is merely a PREFIX of the held one is still a key the bucket holds nothing at.
    rows.push(Row {
        case: "GetObject of a key that is a prefix of the held one",
        method: "GET",
        suffix: "ledger/held".to_string(),
        headers: Vec::new(),
        body: "",
        code: NO_SUCH_KEY_CODE,
        key: "ledger/held".to_string(),
        version_id: None,
    });
    rows
}

#[tokio::test]
async fn every_object_scoped_refusal_names_the_object_it_is_about() {
    let router = router();
    let name = "object-envelope-operations";
    bucket_holding(&router, name).await;

    // A token of the published shape that this store never minted, so it names no version of
    // any key: the width `VERSION_ID_CHARS` states, which is what makes it a version id
    // rather than a refused argument.
    let absent_version = "0".repeat(VERSION_ID_CHARS);

    for row in operation_rows(&absent_version) {
        let answer = send_row(&router, name, &row).await;
        assert_names_object(
            row.case,
            &answer,
            row.code,
            &row.key,
            row.version_id.as_deref(),
        );
    }

    // The version-scoped DELETE of the same absent version writes no envelope at all: it is
    // the operation of this set that models no error, so there is nothing for it to name.
    let deleted = send(
        &router,
        "DELETE",
        &format!("/{name}/{HELD_KEY}?versionId={absent_version}"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case \"DeleteObject naming a version that is not there\": expected the delete's own \
         204, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    assert!(
        deleted.body.is_empty(),
        "case \"DeleteObject naming a version that is not there\": that success carries no \
         envelope, got {:?}",
        text(&deleted)
    );

    // The held object still reads back, so every row above was refused about the address it
    // named rather than because the bucket lost its object.
    let read = get_object(&router, name, HELD_KEY, "", &[]).await;
    assert_eq!(
        read.body.as_ref(),
        BODY,
        "the held object must still read back, got {:?}",
        text(&read)
    );
}

#[tokio::test]
async fn a_copy_refused_about_its_source_names_the_source_object() {
    let router = router();
    let name = "object-envelope-copy";
    bucket_holding(&router, name).await;
    let absent_version = "0".repeat(VERSION_ID_CHARS);

    // (case, the `x-amz-copy-source` the copy carries, the key and version its refusal names)
    for (case, source, code, key, version) in [
        (
            "a copy source the bucket holds nothing at",
            format!("{name}/ledger/never-copied.bin"),
            NO_SUCH_KEY_CODE,
            "ledger/never-copied.bin",
            None,
        ),
        (
            "a copy source version the bucket holds no entry for",
            format!("{name}/{HELD_KEY}?versionId={absent_version}"),
            NO_SUCH_VERSION_CODE,
            HELD_KEY,
            Some(absent_version.as_str()),
        ),
    ] {
        let refused = send_bytes(
            &router,
            "PUT",
            &format!("/{name}/copies/destination.bin"),
            &[("x-amz-copy-source", source.as_str())],
            b"",
        )
        .await;
        assert_names_object(case, &refused, code, key, version);
    }
}

/// The key spellings the member has to carry whole: one row per thing a key can contain that
/// the path, the query or the XML document would otherwise swallow.
///
/// Each row is (case, the key as the request URI spells it, the key the member must name).
pub(crate) fn key_spelling_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("several path segments", "a/b/c/absent.bin", "a/b/c/absent.bin"),
        ("a trailing separator", "a/folder/", "a/folder/"),
        ("an encoded separator", "a%2Fb.bin", "a/b.bin"),
        ("an encoded space", "a%20b/c.bin", "a b/c.bin"),
        ("XML metacharacters", "a%3Cb%3E%26c/d.bin", "a<b>&c/d.bin"),
        ("non-ASCII text", "caf%C3%A9/na%C3%AFve.txt", "café/naïve.txt"),
        ("a plus sign", "a+b/c.bin", "a+b/c.bin"),
    ]
}

#[tokio::test]
async fn the_key_named_is_the_one_the_request_spelled() {
    let router = router();
    let name = "object-envelope-addressing";
    bucket_holding(&router, name).await;

    for (case, spelled, key) in key_spelling_rows() {
        // PATH-STYLE: the bucket is the first segment of the path, the key is the rest. The
        // virtual-hosted form of the same table is [`super::zz_host_style_probe`].
        let path_style = send_bytes(
            &router,
            "GET",
            &format!("/{name}/{spelled}?tagging"),
            &[],
            b"",
        )
        .await;
        assert_names_object(
            &format!("{case} addressed path-style"),
            &path_style,
            NO_SUCH_KEY_CODE,
            key,
            None,
        );
    }
}

#[tokio::test]
async fn a_refusal_about_a_key_a_delete_marker_hides_names_that_key() {
    let router = router();
    let name = "object-envelope-marker";
    bucket_holding(&router, name).await;

    // The delete lays a marker over the held key, so the key is a 404 while the bucket still
    // holds every version of it.
    let deleted = delete_object(&router, name, HELD_KEY, "").await;
    assert_eq!(
        deleted.status, 204,
        "the delete must lay a marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    let marker_version = deleted
        .header("x-amz-version-id")
        .expect("the delete must report the version id of the marker it laid");

    let read = get_object(&router, name, HELD_KEY, "", &[]).await;
    assert_names_object(
        "a key a delete marker hides",
        &read,
        NO_SUCH_KEY_CODE,
        HELD_KEY,
        None,
    );
    // Beside the key, the two headers that separate a key a marker hides from one that never
    // existed: both are a 404 and neither document carries a member for the marker.
    assert_eq!(
        read.header("x-amz-delete-marker").as_deref(),
        Some("true"),
        "the refusal must report that the current version is a delete marker, got {:?}",
        read.header("x-amz-delete-marker")
    );
    assert_eq!(
        read.header("x-amz-version-id").as_deref(),
        Some(marker_version.as_str()),
        "the refusal must name the marker's own version, got {:?}",
        read.header("x-amz-version-id")
    );
}
