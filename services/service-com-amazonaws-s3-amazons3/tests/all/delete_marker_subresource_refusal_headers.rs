// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what EVERY object-level call reports when it is addressed at a
//! DELETE MARKER's own version — not just the two readers
//! ([`super::delete_marker_refusal_headers`] pins those) but each object SUBRESOURCE of this
//! service too.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire behaviour.
//!
//! The rules under test, with their sources:
//!
//! * "If the specified version is a delete marker, the response returns a 405 Method Not
//!   Allowed error" (`GetObject`'s own documentation): a marker holds no bytes, no metadata
//!   and no subresource, so no object-level method can be served from one. The document is
//!   `MethodNotAllowed` with the fixed sentence that code publishes, and the `<Method>` /
//!   `<ResourceType>` pair naming the method the request used and the marker it was refused
//!   against.
//! * That refusal REPORTS the marker it refused, because the document carries no member for
//!   it: `x-amz-delete-marker` "indicates whether the specified object is (true) or is not
//!   (false) a delete marker" (`GetObjectOutput$DeleteMarker`, bound to that header) beside
//!   the `x-amz-version-id` of the marker that was addressed (`$VersionId`) — the version id
//!   the store minted for it, which a marker has "like any other object" (Amazon S3 User
//!   Guide, *Working with delete markers*). Those two headers are the only thing separating
//!   this 405 from any other, which is what a client walking a versioned listing branches
//!   on.
//! * A 405 names what the resource DOES allow: "the origin server MUST generate an `Allow`
//!   header field in a 405 response containing a list of the target resource's currently
//!   supported methods" (RFC 9110 §15.5.6).
//! * The OBJECT version behind the marker is untouched by all of this: addressed by its own
//!   version id, every one of the same calls is served (or refused for a reason of its own)
//!   and reports no delete marker.

use service_com_amazonaws_s3_amazons3::state::{
    methods_allowed_on, DELETE_MARKER_RESOURCE_TYPE, GOVERNANCE_RETENTION_MODE,
    METHOD_NOT_ALLOWED_CODE, METHOD_NOT_ALLOWED_MESSAGE, METHOD_NOT_ALLOWED_STATUS,
    NULL_VERSION_ID,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_legal_hold::lock_enabled_bucket;
use super::object_retention::{now_seconds, retention_payload, ONE_HOUR};
use super::object_versioning::{delete_object, set_versioning};

/// The bytes of the object version a marker is laid over.
const BODY: &[u8] = b"a-version-behind-a-marker";

/// The XML namespace every request document of this service carries.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// One object-level call, as a request line and a document: WHICH operation, the method it
/// is bound to (which is what its `MethodNotAllowed` reports), the subresource query it
/// carries, the headers and body it needs to reach the address at all, and whether it is an
/// Object Lock call — one of those is served only on a bucket with an Object Lock
/// configuration, so the lock-less bucket's table leaves it out.
struct Call {
    operation: &'static str,
    method: &'static str,
    subresource: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
    needs_object_lock: bool,
}

impl Call {
    /// A call carrying no headers and no document of its own.
    fn bare(operation: &'static str, method: &'static str, subresource: &'static str) -> Self {
        Self {
            operation,
            method,
            subresource,
            headers: Vec::new(),
            body: String::new(),
            needs_object_lock: false,
        }
    }

    /// The same call with the headers it needs.
    fn with_headers(mut self, headers: Vec<(&'static str, String)>) -> Self {
        self.headers = headers;
        self
    }

    /// The same call with the request document it needs.
    fn with_body(mut self, body: String) -> Self {
        self.body = body;
        self
    }

    /// The same call marked as an Object Lock one.
    fn object_lock(mut self) -> Self {
        self.needs_object_lock = true;
        self
    }

    /// Whether the answer carries a document at all: a `HEAD` response never does
    /// ("the server … MUST NOT send content in the response", RFC 9110 §9.3.2), so only the
    /// headers of that one can be read.
    fn reports_a_document(&self) -> bool {
        self.method != "HEAD"
    }

    /// This call addressed at `version_id` of `bucket`/`key`.
    async fn send(
        &self,
        router: &MultiServiceRouter,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Answer {
        let separator = if self.subresource.is_empty() {
            "?"
        } else {
            "&"
        };
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        send(
            router,
            self.method,
            &format!(
                "/{bucket}/{key}{}{separator}versionId={version_id}",
                self.subresource
            ),
            &headers,
            &self.body,
        )
        .await
    }
}

/// Every object-level call of this service that addresses a stored VERSION, each spelled the
/// way its model binds it (`smithy_query(query="digest")` reports the method and the URI of
/// each): the two readers, the ACL, tagging, attributes, restore and the two Object Lock
/// subresources, read and written.
///
/// The rows differ along what the rule generalises over: WHICH operation, which HTTP method
/// it is bound to, whether it reads or writes, and whether it carries a request document.
fn calls() -> Vec<Call> {
    vec![
        Call::bare("GetObject", "GET", ""),
        Call::bare("HeadObject", "HEAD", ""),
        Call::bare("GetObjectAcl", "GET", "?acl"),
        // "The canned ACL to apply to the object" (`PutObjectAclRequest$ACL`), named in the
        // header the model binds it to, which is one of the three ways a put may name an ACL.
        Call::bare("PutObjectAcl", "PUT", "?acl")
            .with_headers(vec![("x-amz-acl", "private".to_string())]),
        Call::bare("GetObjectTagging", "GET", "?tagging"),
        Call::bare("PutObjectTagging", "PUT", "?tagging").with_body(format!(
            "<Tagging xmlns=\"{XMLNS}\"><TagSet><Tag><Key>phase</Key>\
             <Value>marked</Value></Tag></TagSet></Tagging>"
        )),
        Call::bare("DeleteObjectTagging", "DELETE", "?tagging"),
        // "Specifies the fields at the root level that you want returned"
        // (`GetObjectAttributesRequest$ObjectAttributes`), a required member bound to this
        // header.
        Call::bare("GetObjectAttributes", "GET", "?attributes")
            .with_headers(vec![("x-amz-object-attributes", "ETag".to_string())]),
        Call::bare("RestoreObject", "POST", "?restore").with_body(format!(
            "<RestoreRequest xmlns=\"{XMLNS}\"><Days>1</Days></RestoreRequest>"
        )),
        Call::bare("GetObjectRetention", "GET", "?retention").object_lock(),
        Call::bare("PutObjectRetention", "PUT", "?retention")
            .with_body(retention_payload(
                Some(GOVERNANCE_RETENTION_MODE),
                Some(now_seconds() + ONE_HOUR),
            ))
            .object_lock(),
        Call::bare("GetObjectLegalHold", "GET", "?legal-hold").object_lock(),
        Call::bare("PutObjectLegalHold", "PUT", "?legal-hold")
            .with_body(format!(
                "<LegalHold xmlns=\"{XMLNS}\"><Status>OFF</Status></LegalHold>"
            ))
            .object_lock(),
    ]
}

/// Assert that `answer` is the refusal every object-level call addressed at the marker whose
/// version is `marker_version` must be: the 405 `MethodNotAllowed` document naming the method
/// and the marker, the two headers reporting the marker, and the `Allow` of the resource.
fn assert_refusal_reports_the_marker(
    answer: &Answer,
    case: &str,
    call: &Call,
    marker_version: &str,
) {
    let operation = call.operation;
    assert_eq!(
        answer.status, METHOD_NOT_ALLOWED_STATUS,
        "case {case:?} on {operation}: expected HTTP {METHOD_NOT_ALLOWED_STATUS}, got {} with \
         body {:?}",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.header("x-amz-delete-marker").as_deref(),
        Some("true"),
        "case {case:?} on {operation}: the refusal must flag the delete marker, got {:?}",
        answer.header("x-amz-delete-marker")
    );
    assert_eq!(
        answer.header("x-amz-version-id").as_deref(),
        Some(marker_version),
        "case {case:?} on {operation}: the refusal must name the addressed marker's version \
         {marker_version:?}, got {:?}",
        answer.header("x-amz-version-id")
    );
    assert_eq!(
        answer.header("allow").as_deref(),
        methods_allowed_on(DELETE_MARKER_RESOURCE_TYPE),
        "case {case:?} on {operation}: expected the Allow header {:?}, got {:?}",
        methods_allowed_on(DELETE_MARKER_RESOURCE_TYPE),
        answer.header("allow")
    );
    if !call.reports_a_document() {
        return;
    }
    assert_eq!(
        answer.error_code(),
        METHOD_NOT_ALLOWED_CODE,
        "case {case:?} on {operation}: expected the code {METHOD_NOT_ALLOWED_CODE:?}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.error_message(),
        METHOD_NOT_ALLOWED_MESSAGE,
        "case {case:?} on {operation}: expected the message {METHOD_NOT_ALLOWED_MESSAGE:?}, \
         got {:?}",
        answer.error_message()
    );
    assert_eq!(
        answer.element("Method").as_deref(),
        Some(call.method),
        "case {case:?} on {operation}: the refusal must name the method it refused, got {:?}",
        answer.element("Method")
    );
    assert_eq!(
        answer.element("ResourceType").as_deref(),
        Some(DELETE_MARKER_RESOURCE_TYPE),
        "case {case:?} on {operation}: the refusal must name the resource type it refused \
         against, got {:?}",
        answer.element("ResourceType")
    );
}

/// The version id a write minted for the object at `bucket`/`key`, and the version id of the
/// delete marker a version-less delete then laid over it.
async fn version_and_marker_over_it(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
) -> (String, String) {
    let written = put_object(router, bucket_name, key, BODY, &[]).await;
    assert_eq!(
        written.status,
        200,
        "the object {key:?} the marker hides must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    let data_version = written
        .header("x-amz-version-id")
        .expect("a write to a versioning-enabled bucket must report the version it minted");
    let deleted = delete_object(router, bucket_name, key, "").await;
    assert_eq!(
        deleted.status,
        204,
        "the delete of {key:?} must lay a marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    let marker_version = deleted
        .header("x-amz-version-id")
        .unwrap_or_else(|| NULL_VERSION_ID.to_string());
    (data_version, marker_version)
}

#[tokio::test]
async fn every_object_level_call_addressed_at_a_delete_marker_reports_the_marker() {
    let router = router();
    let bucket_name = "marker-subresource";
    // Object Lock, so the two lock subresources are served on this bucket at all — and it
    // brings the versioning state a delete marker needs with it. `ObjectWriter`, so the ACL
    // write is not refused for the bucket's ownership setting before the address is resolved.
    lock_enabled_bucket(&router, bucket_name).await;
    let ownership = put_ownership_controls(&router, bucket_name, &["ObjectWriter"]).await;
    assert_eq!(
        ownership.status,
        200,
        "the bucket must take the ownership setting that enables ACLs, got {} with body {:?}",
        ownership.status,
        text(&ownership)
    );

    // Two keys, so the rule is pinned over both forms of key a request line carries.
    for key in ["marked.bin", "deep/marked.bin"] {
        let (_, marker_version) = version_and_marker_over_it(&router, bucket_name, key).await;
        for call in calls() {
            let answer = call.send(&router, bucket_name, key, &marker_version).await;
            assert_refusal_reports_the_marker(&answer, key, &call, &marker_version);
        }
    }
}

#[tokio::test]
async fn a_null_delete_marker_is_reported_by_the_null_version_id() {
    let router = router();
    let bucket_name = "marker-subresource-null";
    bucket(&router, bucket_name).await;
    let ownership = put_ownership_controls(&router, bucket_name, &["ObjectWriter"]).await;
    assert_eq!(
        ownership.status,
        200,
        "the bucket must take the ownership setting that enables ACLs, got {} with body {:?}",
        ownership.status,
        text(&ownership)
    );
    set_versioning(&router, bucket_name, "Enabled").await;
    let key = "suspended.bin";
    let written = put_object(&router, bucket_name, key, BODY, &[]).await;
    assert_eq!(
        written.status,
        200,
        "the object the marker hides must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    // "All objects added to the bucket receive the version ID null" while versioning is
    // SUSPENDED, so the marker this delete lays is the `null` version — the second form the
    // addressed marker's version id takes, and the one a request names by `versionId=null`.
    set_versioning(&router, bucket_name, "Suspended").await;
    let deleted = delete_object(&router, bucket_name, key, "").await;
    assert_eq!(
        deleted.status,
        204,
        "the delete must lay the null marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );

    for call in calls().into_iter().filter(|call| !call.needs_object_lock) {
        let answer = call.send(&router, bucket_name, key, NULL_VERSION_ID).await;
        assert_refusal_reports_the_marker(&answer, "the null marker", &call, NULL_VERSION_ID);
    }
}

#[tokio::test]
async fn the_version_behind_the_marker_is_still_addressable() {
    let router = router();
    let bucket_name = "marker-subresource-control";
    lock_enabled_bucket(&router, bucket_name).await;
    let ownership = put_ownership_controls(&router, bucket_name, &["ObjectWriter"]).await;
    assert_eq!(
        ownership.status,
        200,
        "the bucket must take the ownership setting that enables ACLs, got {} with body {:?}",
        ownership.status,
        text(&ownership)
    );
    let key = "still-there.bin";
    let (data_version, marker_version) =
        version_and_marker_over_it(&router, bucket_name, key).await;
    assert_ne!(
        data_version, marker_version,
        "the marker must be a version of its own, got {data_version:?} for both"
    );

    // The negative control for the rule above: the OBJECT version is still addressable by its
    // own id, so none of the same calls is the marker's 405 and none of them reports a marker.
    for call in calls() {
        let answer = call.send(&router, bucket_name, key, &data_version).await;
        assert_ne!(
            answer.status,
            METHOD_NOT_ALLOWED_STATUS,
            "case {:?}: a call on the stored object version is not the marker's refusal, got \
             {} with body {:?}",
            call.operation,
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.header("x-amz-delete-marker"),
            None,
            "case {:?}: an answer about a stored object version reports no delete marker, got \
             {:?}",
            call.operation,
            answer.header("x-amz-delete-marker")
        );
    }

    // The positive control the issue's own walk makes: the ACL of the version behind the
    // marker is still reported, owner and all.
    let acl = send(
        &router,
        "GET",
        &format!("/{bucket_name}/{key}?acl&versionId={data_version}"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        acl.status,
        200,
        "the ACL of the version behind the marker must still be served, got {} with body {:?}",
        acl.status,
        text(&acl)
    );
    assert!(
        acl.element("Owner").is_some() || acl.element("ID").is_some(),
        "the served ACL must report its Owner, got body {:?}",
        text(&acl)
    );
}
