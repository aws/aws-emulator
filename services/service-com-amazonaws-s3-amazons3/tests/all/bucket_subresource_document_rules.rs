// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for four rules of S3's bucket sub-resource and listing planes, each
//! driven as a real HTTP request through the router so what is pinned is the wire behaviour
//! — status, error code, message and what a later read reports — rather than any handler's
//! internals.
//!
//! The rules under test, with their sources:
//!
//! * A `Deleted` entry of a multi-object delete reports the version it removed AND, when
//!   that version WAS a delete marker, that marker's own id: `DeletedObject$DeleteMarker`
//!   "indicates whether the specified object version that was permanently deleted was
//!   (true) or was not (false) a delete marker before deletion", and
//!   `DeletedObject$DeleteMarkerVersionId` is "the version ID of the delete marker".
//! * A `Delete` payload outside the published bound — "the request can contain a list of up
//!   to 1,000 keys" (`DeleteObjects`' own documentation), and the container carries at least
//!   one `Object` — is refused `MalformedXML`, which carries that code's own FIXED published
//!   sentence and nothing appended to it.
//! * With a delimiter, a listing's start token names an ENTRY of the listing — a key or a
//!   common prefix — and the listing resumes after that whole entry: "CommonPrefixes is
//!   filtered out from results if it is not lexicographically greater than the key-marker"
//!   (`ListObjectsRequest$Delimiter`), the same token `ListObjectsV2Request$StartAfter` is.
//! * A bucket-configuration `continuation-token` is "an opaque value that Amazon S3
//!   understands" and is scoped to the listing it was issued for AND to the bucket it was
//!   issued over, so a value S3 never issued names no page at all and one issued over
//!   another bucket names no page of this one.
//! * A lifecycle rule whose actions cannot be scoped the way its filter asks, or whose
//!   filter names a predicate that is not one, is refused with the code and the sentence S3
//!   publishes for the rule it broke rather than stored.
//! * A configuration `Filter` container is a CHOICE of ONE predicate — "a `Filter` must
//!   specify exactly one `Prefix`, `Tag`, or an `And` child element"
//!   (`ReplicationRuleFilter`), "a `Filter` can have exactly one of `Prefix`, `Tag`,
//!   `ObjectSizeGreaterThan`, `ObjectSizeLessThan`, or `And` specified"
//!   (`LifecycleRuleFilter`), and "if you specify both a `Prefix` and a `Tag` filter, wrap
//!   these filters in an `And` tag" (`ReplicationRuleFilter$And`) — so a document whose
//!   filter names more than one of them directly does not validate against the published
//!   schema and is refused `MalformedXML` (HTTP 400), under that code's own FIXED sentence,
//!   with nothing recorded. One rule, so it holds for every configuration family that
//!   carries such a container.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_intelligent_tiering_configuration::PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    INTERNAL_ERROR_CODE, INTERNAL_ERROR_MESSAGE, MALFORMED_CONTINUATION_TOKEN_CODE,
    MALFORMED_CONTINUATION_TOKEN_MESSAGE, MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE,
    MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, NO_SUCH_CONFIGURATION_CODE,
    VERSIONING_ENABLED_STATUS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_lifecycle_configuration::{configuration, put_lifecycle, rule};
use super::bucket_metrics_configuration::put_metrics;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};

/// The XML namespace every request document of this service carries.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// `PUT /<bucket>?versioning` with `Enabled`, so a delete lays a marker instead of removing
/// the object — and so the bucket may carry a replication configuration, which applies to
/// object VERSIONS.
async fn enable_versioning(router: &MultiServiceRouter, name: &str) {
    let document = format!(
        "<VersioningConfiguration xmlns=\"{XMLNS}\">\
         <Status>{VERSIONING_ENABLED_STATUS}</Status></VersioningConfiguration>"
    );
    let answer = send(
        router,
        "PUT",
        &format!("/{name}?versioning"),
        &[],
        &document,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "versioning must be enabled on {name:?} first, got {} ({})",
        answer.status,
        text(&answer)
    );
}

/// `POST /<bucket>?delete` — `DeleteObjects`, over the `Object` elements given.
async fn delete_objects(router: &MultiServiceRouter, name: &str, objects: &str) -> Answer {
    let document = format!("<Delete xmlns=\"{XMLNS}\">{objects}</Delete>");
    send(
        router,
        "POST",
        &format!("/{name}?delete"),
        &[],
        &document,
    )
    .await
}

/// `DELETE /<bucket>/<key>` — `DeleteObject`, whose answer names the delete marker it laid.
async fn delete_object(router: &MultiServiceRouter, name: &str, key: &str) -> Answer {
    send(router, "DELETE", &format!("/{name}/{key}"), &[], "").await
}

/// Removing a delete-marker VERSION reports the removal as a marker removal of that id, and
/// removing an ordinary version reports neither member — the two halves of the one rule
/// `DeletedObject$DeleteMarker` and `$DeleteMarkerVersionId` state.
///
/// The rows differ along what the rule generalises over: which KIND of version the batch
/// names (a delete marker and an ordinary object version) and how many versions of the key
/// the bucket holds behind it.
#[tokio::test]
async fn a_batch_delete_reports_the_delete_marker_it_removed() {
    let router = router();
    let name = "mbes-marker-removals";
    bucket(&router, name).await;
    enable_versioning(&router, name).await;

    for (case, writes) in [("one version behind the marker", 1), ("three versions", 3)] {
        let key = format!("undelete-{writes}.txt");
        let mut versions: Vec<String> = Vec::new();
        for write in 0..writes {
            let put = put_object(
                &router,
                name,
                &key,
                format!("body {write}").as_bytes(),
                &[],
            )
            .await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: the write must succeed, got {} ({})",
                put.status,
                text(&put)
            );
            versions.push(put.header("x-amz-version-id").unwrap_or_else(|| {
                panic!("case {case:?}: a versioned write names the version it stored")
            }));
        }

        // The marker a simple delete lays, which "becomes the current version of the
        // object".
        let laid = delete_object(&router, name, &key).await;
        let marker = laid.header("x-amz-version-id").unwrap_or_else(|| {
            panic!(
                "case {case:?}: a delete on a versioning-enabled bucket names the marker it \
                 laid, got {}",
                laid.status
            )
        });
        assert_eq!(
            laid.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {case:?}: the delete must report the version it laid as a marker"
        );

        // Removing the MARKER by its own id: the entry says it was a marker and names which
        // marker went away, so an undelete routine can confirm the one it removed.
        let removed = delete_objects(
            &router,
            name,
            &format!("<Object><Key>{key}</Key><VersionId>{marker}</VersionId></Object>"),
        )
        .await;
        assert_eq!(
            removed.status,
            200,
            "case {case:?}: the batch must answer 200, got {} ({})",
            removed.status,
            text(&removed)
        );
        assert_eq!(
            removed.element("DeleteMarker").as_deref(),
            Some("true"),
            "case {case:?}: expected the entry to report a marker removal, got {}",
            text(&removed)
        );
        assert_eq!(
            removed.element("DeleteMarkerVersionId").as_deref(),
            Some(marker.as_str()),
            "case {case:?}: expected the removed marker's id {marker:?}, got {:?}",
            removed.element("DeleteMarkerVersionId")
        );

        // And removing an ORDINARY version reports neither member: nothing about it was a
        // marker.
        let current = versions
            .last()
            .cloned()
            .expect("the case writes at least one version");
        let plain = delete_objects(
            &router,
            name,
            &format!("<Object><Key>{key}</Key><VersionId>{current}</VersionId></Object>"),
        )
        .await;
        assert_eq!(
            plain.element("DeleteMarker"),
            None,
            "case {case:?}: an ordinary version is no marker, got {}",
            text(&plain)
        );
        assert_eq!(
            plain.element("DeleteMarkerVersionId"),
            None,
            "case {case:?}: an ordinary version names no marker, got {}",
            text(&plain)
        );
    }
}

/// A `Delete` payload outside the published bound is refused `MalformedXML` carrying that
/// code's own fixed sentence and nothing else, and one inside it is carried out.
///
/// The rows differ along the COUNT, at each end of the published bound and one past each:
/// none, one, exactly the maximum, and one more.
#[tokio::test]
async fn a_delete_batch_outside_the_published_bound_is_refused_in_aws_words() {
    let router = router();
    let name = "mbes-batch-bounds";
    bucket(&router, name).await;

    // "The request can contain a list of up to 1,000 keys" (`DeleteObjects`' own
    // documentation); the `Delete` container carries at least one `Object`.
    const MAX_KEYS_PER_DELETE: usize = 1_000;
    for (case, count, refused) in [
        ("no Object element at all", 0, true),
        ("one key", 1, false),
        ("exactly the published maximum", MAX_KEYS_PER_DELETE, false),
        ("one key past the maximum", MAX_KEYS_PER_DELETE + 1, true),
    ] {
        let objects: String = (0..count)
            .map(|index| format!("<Object><Key>absent-{index}.txt</Key></Object>"))
            .collect();
        let answer = delete_objects(&router, name, &objects).await;
        match refused {
            false => assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected 200, got {} ({})",
                answer.status,
                text(&answer)
            ),
            true => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: expected 400, got {} ({})",
                    answer.status,
                    text(&answer)
                );
                assert_eq!(
                    answer.error_code(),
                    MALFORMED_XML_CODE,
                    "case {case:?}: expected {MALFORMED_XML_CODE}, got {}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.error_message(),
                    MALFORMED_XML_MESSAGE,
                    "case {case:?}: expected the code's own published sentence, got {:?}",
                    answer.error_message()
                );
            }
        }
    }
}

/// `GET /<bucket>?list-type=2&...` — `ListObjectsV2`.
async fn list_v2(router: &MultiServiceRouter, name: &str, query: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}?list-type=2{query}"),
        &[],
        "",
    )
    .await
}

/// The common prefixes a listing reports, in document order: the `<Prefix>` of each
/// `<CommonPrefixes>` container, which is a DIFFERENT element from the request's own
/// `<Prefix>` echo at the top of the body.
fn common_prefixes(answer: &Answer) -> Vec<String> {
    let body = text(answer);
    body.split("<CommonPrefixes>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</CommonPrefixes>"))
        .filter_map(|(container, _)| {
            container
                .split_once("<Prefix>")
                .and_then(|(_, rest)| rest.split_once("</Prefix>"))
                .map(|(prefix, _)| prefix.to_string())
        })
        .collect()
}

/// With a delimiter, the start token names an entry of the listing and the listing resumes
/// after that whole entry: a token naming a common prefix leaves the group it rolls up
/// behind it, and a token naming a key inside a group leaves that group behind too, since
/// the group's prefix is not lexicographically greater than the token.
///
/// The rows differ along what the token is — absent, a common prefix, a key inside a group,
/// a key with no group, and a value no key starts with — and along whether the listing
/// states a delimiter at all.
#[tokio::test]
async fn a_start_token_with_a_delimiter_resumes_after_a_whole_group() {
    let router = router();
    let name = "listing-start-after-groups";
    bucket(&router, name).await;
    // A folder MARKER among them: a zero-byte object whose key ENDS in the delimiter, which
    // rolls into its own common prefix.
    for key in ["a/1", "b/1", "b/2", "c/1", "empty/", "top.txt"] {
        let put = put_object(&router, name, key, b"x", &[]).await;
        assert_eq!(
            put.status,
            200,
            "the object {key:?} must be stored first, got {} ({})",
            put.status,
            text(&put)
        );
    }

    for (case, query, prefixes, keys) in [
        (
            "no start token",
            "&delimiter=%2F",
            vec!["a/", "b/", "c/", "empty/"],
            vec!["top.txt"],
        ),
        (
            "a token naming a common prefix",
            "&delimiter=%2F&start-after=b%2F",
            vec!["c/", "empty/"],
            vec!["top.txt"],
        ),
        (
            "a token naming the first common prefix",
            "&delimiter=%2F&start-after=a%2F",
            vec!["b/", "c/", "empty/"],
            vec!["top.txt"],
        ),
        (
            "a token naming a key inside a group",
            "&delimiter=%2F&start-after=b%2F1",
            vec!["c/", "empty/"],
            vec!["top.txt"],
        ),
        (
            "a token naming the folder marker's own prefix",
            "&delimiter=%2F&start-after=empty%2F",
            vec![],
            vec!["top.txt"],
        ),
        (
            "a token no key starts with",
            "&delimiter=%2F&start-after=bb",
            vec!["c/", "empty/"],
            vec!["top.txt"],
        ),
        (
            // With NO delimiter nothing rolls up, so the token is read against the keys
            // themselves and every later key is reported.
            "the same token with no delimiter",
            "&start-after=b%2F1",
            vec![],
            vec!["b/2", "c/1", "empty/", "top.txt"],
        ),
    ] {
        let listing = list_v2(&router, name, query).await;
        assert_eq!(
            listing.status,
            200,
            "case {case:?}: expected 200, got {} ({})",
            listing.status,
            text(&listing)
        );
        assert_eq!(
            common_prefixes(&listing),
            prefixes
                .iter()
                .map(|prefix| (*prefix).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the common prefixes {prefixes:?}, got {:?} ({})",
            common_prefixes(&listing),
            text(&listing)
        );
        assert_eq!(
            listing.elements("Key"),
            keys.iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the keys {keys:?}, got {:?}",
            listing.elements("Key")
        );
    }
}

/// `GET /<bucket>?<subresource>&continuation-token=<token>` for one of the four listings of
/// a bucket's id-addressed configurations.
///
/// The token travels as a query value, percent-encoded as one
/// ([`crate::continuation_token_alphabet::handed_back_in_query`]).
async fn list_configurations(
    router: &MultiServiceRouter,
    name: &str,
    subresource: &str,
    token: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!(
            "/{name}?{subresource}&continuation-token={}",
            super::continuation_token_alphabet::handed_back_in_query(token)
        ),
        &[],
        "",
    )
    .await
}

/// A continuation token is a cursor into ONE listing over ONE bucket: a value S3 never
/// issued names no page at all, and one S3 issued over another bucket names no page of this
/// one.
///
/// The rows differ along the SUBRESOURCE the token is handed to (every one of the four
/// listings that pages a bucket's id-addressed configurations) and along the shape of the
/// forged value (text that is not a token, a rendering of a stored id, and base64 that
/// carries no cursor).
#[tokio::test]
async fn a_continuation_token_is_a_cursor_only_into_the_listing_that_issued_it() {
    let router = router();
    let name = "token-scope-owner";
    let other = "token-scope-stranger";
    bucket(&router, name).await;
    bucket(&router, other).await;

    for (case, token) in [
        ("text that is not a token", "not-a-token"),
        ("a rendering of a stored id", "cfg-0"),
        ("base64 carrying no cursor", "a2VwdA=="),
    ] {
        for subresource in ["analytics", "intelligent-tiering", "inventory", "metrics"] {
            let refused = list_configurations(&router, name, subresource, token).await;
            assert_eq!(
                refused.status,
                400,
                "case {case:?} ({subresource}): expected 400, got {} ({})",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                MALFORMED_CONTINUATION_TOKEN_CODE,
                "case {case:?} ({subresource}): expected \
                 {MALFORMED_CONTINUATION_TOKEN_CODE}, got {}",
                refused.error_code()
            );
            assert_eq!(
                refused.error_message(),
                MALFORMED_CONTINUATION_TOKEN_MESSAGE,
                "case {case:?} ({subresource}): expected the published sentence, got {:?}",
                refused.error_message()
            );
        }
    }

    // A token S3 really issued, over ANOTHER bucket: enough configurations on each bucket to
    // truncate the first page, so the listing hands one back.
    let total = 103;
    for holder in [name, other] {
        for index in 0..total {
            let put = put_metrics(&router, holder, &format!("cfg-{index:03}"), None).await;
            assert_eq!(
                put.status,
                204,
                "setup must put cfg-{index:03} on {holder:?}, got {} ({})",
                put.status,
                text(&put)
            );
        }
    }
    let first = list_configurations(&router, name, "metrics", "").await;
    let token = first.element("NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "a truncated listing hands back a continuation token, got {}",
            text(&first)
        )
    });

    // On the bucket it was issued over it resumes the listing.
    let resumed = list_configurations(&router, name, "metrics", &token).await;
    assert_eq!(
        resumed.status,
        200,
        "case \"the bucket it was issued over\": expected 200, got {} ({})",
        resumed.status,
        text(&resumed)
    );
    assert_eq!(
        resumed.elements("Id").len(),
        total - 100,
        "case \"the bucket it was issued over\": expected the remaining {} ids, got {:?}",
        total - 100,
        resumed.elements("Id")
    );

    // On another bucket it is no cursor at all, and the listing says so rather than serving
    // that bucket's first page.
    let elsewhere = list_configurations(&router, other, "metrics", &token).await;
    assert_eq!(
        elsewhere.error_code(),
        INTERNAL_ERROR_CODE,
        "case \"another bucket\": expected {INTERNAL_ERROR_CODE}, got {} ({})",
        elsewhere.error_code(),
        text(&elsewhere)
    );
    assert_eq!(
        elsewhere.error_message(),
        INTERNAL_ERROR_MESSAGE,
        "case \"another bucket\": expected the published sentence, got {:?}",
        elsewhere.error_message()
    );
}

/// One row of the lifecycle table: the case, the rule body, and the code and sentence the
/// put must answer — `None` for a rule S3 stores.
struct LifecycleRow {
    case: &'static str,
    body: String,
    refusal: Option<(&'static str, String)>,
}

/// The lifecycle rules this fix added, each stated once: an action that cannot be scoped by
/// object tags beside a tag filter, a conjunction of fewer predicates than a logical AND
/// applies to, two tag predicates on one key, a tag predicate outside the published tag
/// bounds, and an expiration due no later than the transition meant to move the object
/// first.
///
/// The rows differ along what each rule generalises over: WHICH action the tag filter cannot
/// scope, WHERE the tag predicate sits (the filter's own `Tag` and the one inside its
/// `And`), WHICH half of the tag is too long and by how much, and whether the expiration and
/// the transition are tied or inverted.
fn lifecycle_rows() -> Vec<LifecycleRow> {
    let tag = "<Tag><Key>team</Key><Value>analytics</Value></Tag>";
    let over_long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let longest_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    let longest_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    vec![
        LifecycleRow {
            case: "an incomplete-upload abort beside a tag filter",
            body: format!(
                "<ID>abort-tagged</ID><Status>Enabled</Status><Filter>{tag}</Filter>\
                 <AbortIncompleteMultipartUpload><DaysAfterInitiation>7\
                 </DaysAfterInitiation></AbortIncompleteMultipartUpload>"
            ),
            refusal: Some((
                "InvalidRequest",
                "AbortIncompleteMultipartUpload cannot be specified with Tags.".to_string(),
            )),
        },
        LifecycleRow {
            case: "an expired-delete-marker expiration beside a tag filter",
            body: format!(
                "<ID>eodm-tagged</ID><Status>Enabled</Status><Filter>{tag}</Filter>\
                 <Expiration><ExpiredObjectDeleteMarker>true</ExpiredObjectDeleteMarker>\
                 </Expiration>"
            ),
            refusal: Some((
                "InvalidRequest",
                "ExpiredObjectDeleteMarker cannot be specified with Tags.".to_string(),
            )),
        },
        LifecycleRow {
            case: "an And of one tag alone",
            body: format!(
                "<ID>and-one-tag</ID><Status>Enabled</Status><Filter><And>{tag}</And></Filter>\
                 <Expiration><Days>30</Days></Expiration>"
            ),
            refusal: Some((MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE.to_string())),
        },
        LifecycleRow {
            case: "an And of one prefix alone",
            body: "<ID>and-one-prefix</ID><Status>Enabled</Status>\
                   <Filter><And><Prefix>logs/</Prefix></And></Filter>\
                   <Expiration><Days>30</Days></Expiration>"
                .to_string(),
            refusal: Some((MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE.to_string())),
        },
        LifecycleRow {
            case: "an And carrying an EMPTY prefix beside one tag",
            body: format!(
                "<ID>and-empty-prefix</ID><Status>Enabled</Status>\
                 <Filter><And><Prefix></Prefix>{tag}</And></Filter>\
                 <Expiration><Days>30</Days></Expiration>"
            ),
            refusal: None,
        },
        LifecycleRow {
            case: "two tag predicates on one key",
            body: "<ID>and-duplicate-key</ID><Status>Enabled</Status><Filter><And>\
                   <Tag><Key>team</Key><Value>one</Value></Tag>\
                   <Tag><Key>team</Key><Value>two</Value></Tag></And></Filter>\
                   <Expiration><Days>30</Days></Expiration>"
                .to_string(),
            refusal: Some((
                "InvalidRequest",
                "Duplicate Tag Keys are not allowed.".to_string(),
            )),
        },
        LifecycleRow {
            case: "a tag key one character past the published bound",
            body: format!(
                "<ID>long-key</ID><Status>Enabled</Status><Filter>\
                 <Tag><Key>{over_long_key}</Key><Value>yes</Value></Tag></Filter>\
                 <Expiration><Days>30</Days></Expiration>"
            ),
            refusal: Some((
                "InvalidRequest",
                format!("A Tag's Key must be a length between 1 and {MAX_TAG_KEY_CHARS}."),
            )),
        },
        LifecycleRow {
            // The floor is a modelled constraint (`Tag$Key` targets `ObjectKey`,
            // `length {min 1}`), so the request boundary is what refuses a key spelled empty
            // — with the sentence the published bounds on a tag key state, the same one the
            // rule validator answers a key of no characters with, so a client reads one
            // refusal for a key outside those bounds at either end.
            case: "a tag key below the published floor",
            body: "<ID>empty-key</ID><Status>Enabled</Status><Filter><And>\
                   <Prefix>logs/</Prefix><Tag><Key></Key><Value>yes</Value></Tag></And></Filter>\
                   <Expiration><Days>30</Days></Expiration>"
                .to_string(),
            refusal: Some((
                "InvalidRequest",
                format!("A Tag's Key must be a length between 1 and {MAX_TAG_KEY_CHARS}."),
            )),
        },
        LifecycleRow {
            case: "a tag key exactly at the published bound",
            body: format!(
                "<ID>longest-key</ID><Status>Enabled</Status><Filter>\
                 <Tag><Key>{longest_key}</Key><Value>yes</Value></Tag></Filter>\
                 <Expiration><Days>30</Days></Expiration>"
            ),
            refusal: None,
        },
        LifecycleRow {
            case: "a tag value one character past the published bound, inside an And",
            body: format!(
                "<ID>long-value</ID><Status>Enabled</Status><Filter><And>\
                 <Prefix>logs/</Prefix>\
                 <Tag><Key>team</Key><Value>{over_long_value}</Value></Tag></And></Filter>\
                 <Expiration><Days>30</Days></Expiration>"
            ),
            refusal: Some((
                "InvalidRequest",
                format!("A Tag's Value must be a length between 0 and {MAX_TAG_VALUE_CHARS}."),
            )),
        },
        LifecycleRow {
            case: "a tag value exactly at the published bound",
            body: format!(
                "<ID>longest-value</ID><Status>Enabled</Status><Filter>\
                 <Tag><Key>team</Key><Value>{longest_value}</Value></Tag></Filter>\
                 <Expiration><Days>30</Days></Expiration>"
            ),
            refusal: None,
        },
        LifecycleRow {
            case: "an expiration on the same day as the transition",
            body: "<ID>expiry-tied</ID><Status>Enabled</Status><Filter></Filter>\
                   <Transition><Days>90</Days><StorageClass>GLACIER</StorageClass></Transition>\
                   <Expiration><Days>90</Days></Expiration>"
                .to_string(),
            refusal: Some((
                "InvalidArgument",
                "'Days' in the Expiration action for filter '()' must be greater than 'Days' \
                 in the Transition action"
                    .to_string(),
            )),
        },
        LifecycleRow {
            case: "an expiration before the transition",
            body: "<ID>expiry-early</ID><Status>Enabled</Status>\
                   <Filter><Prefix>cold/</Prefix></Filter>\
                   <Transition><Days>90</Days><StorageClass>GLACIER</StorageClass></Transition>\
                   <Expiration><Days>60</Days></Expiration>"
                .to_string(),
            refusal: Some((
                "InvalidArgument",
                "'Days' in the Expiration action for filter '(prefix=cold/)' must be greater \
                 than 'Days' in the Transition action"
                    .to_string(),
            )),
        },
        LifecycleRow {
            case: "an expiration after the transition",
            body: "<ID>expiry-late</ID><Status>Enabled</Status>\
                   <Filter><Prefix>cold/</Prefix></Filter>\
                   <Transition><Days>90</Days><StorageClass>GLACIER</StorageClass></Transition>\
                   <Expiration><Days>120</Days></Expiration>"
                .to_string(),
            refusal: None,
        },
    ]
}

/// The `ID` a rule body names, which is what a later read reports the stored rule under.
fn stored_id(body: &str) -> String {
    body.split_once("<ID>")
        .and_then(|(_, rest)| rest.split_once("</ID>"))
        .map(|(id, _)| id.to_string())
        .unwrap_or_default()
}

/// Every rule of the table above, judged on the wire: a refused rule carries S3's own code
/// and sentence and leaves the bucket carrying no configuration, and an accepted one reads
/// back through `GetBucketLifecycleConfiguration`.
#[tokio::test]
async fn a_lifecycle_rule_s3_refuses_is_refused_in_aws_words() {
    let router = router();
    let name = "lifecycle-document-rules";
    bucket(&router, name).await;

    for row in lifecycle_rows() {
        let case = row.case;
        let document = configuration(&[rule(&row.body)]);
        let answer = put_lifecycle(&router, name, &[], &document).await;
        match &row.refusal {
            None => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected the put to answer 200, got {} ({})",
                    answer.status,
                    text(&answer)
                );
                // Round-tripped: the stored rule is the one that was put.
                let read = send(&router, "GET", &format!("/{name}?lifecycle"), &[], "").await;
                assert_eq!(
                    read.status,
                    200,
                    "case {case:?}: the stored configuration must read back, got {} ({})",
                    read.status,
                    text(&read)
                );
                assert!(
                    text(&read).contains(&format!("<ID>{}</ID>", stored_id(&row.body))),
                    "case {case:?}: expected the stored rule to read back, got {}",
                    text(&read)
                );
            }
            Some((code, message)) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: expected 400, got {} ({})",
                    answer.status,
                    text(&answer)
                );
                assert_eq!(
                    answer.error_code(),
                    *code,
                    "case {case:?}: expected the code {code}, got {}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.error_message(),
                    *message,
                    "case {case:?}: expected the message {message:?}, got {:?}",
                    answer.error_message()
                );
            }
        }
    }
}

/// The `x-amz-transition-default-minimum-object-size` header names one of the two behaviours
/// its member publishes, and a value outside them names none the service could apply — which
/// S3 reports as an `InvalidRequest` echoing the value it was handed.
///
/// The rows differ along the value: each published behaviour, and values outside the set
/// spanning what one can be (a storage-class name, another case of a published value, and
/// the empty string).
#[tokio::test]
async fn an_undefined_transition_default_minimum_object_size_is_refused_with_its_value() {
    let router = router();
    let name = "lifecycle-minimum-object-size";
    bucket(&router, name).await;

    let document = configuration(&[rule(
        "<ID>sized</ID><Status>Enabled</Status><Filter></Filter>\
         <Expiration><Days>30</Days></Expiration>",
    )]);
    // The two values `TransitionDefaultMinimumObjectSize` publishes
    // (`smithy_query(query="shape",shape="TransitionDefaultMinimumObjectSize")`).
    for published in ["all_storage_classes_128K", "varies_by_storage_class"] {
        let answer = put_lifecycle(
            &router,
            name,
            &[("x-amz-transition-default-minimum-object-size", published)],
            &document,
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {published:?}: a published behaviour must be applied, got {} ({})",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer
                .header("x-amz-transition-default-minimum-object-size")
                .as_deref(),
            Some(published),
            "case {published:?}: the applied behaviour must be reported back"
        );
    }

    for value in [
        "nonexistent_storage_class",
        "ALL_STORAGE_CLASSES_128K",
        "varies-by-storage-class",
    ] {
        let refused = put_lifecycle(
            &router,
            name,
            &[("x-amz-transition-default-minimum-object-size", value)],
            &document,
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {value:?}: expected 400, got {} ({})",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequest",
            "case {value:?}: expected InvalidRequest, got {}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            format!("Invalid TransitionDefaultMinimumObjectSize found: {value}"),
            "case {value:?}: expected the value to be named, got {:?}",
            refused.error_message()
        );
    }
}

/// One `Tag` predicate, as every filter container of this service spells one: `Tag$Key` and
/// `$Value` are both `@required` (`smithy_query(query="shape",shape="Tag")`).
const ONE_TAG: &str = "<Tag><Key>team</Key><Value>gold</Value></Tag>";

/// One `Filter` container, as a configuration document spells it, and whether the published
/// schema admits it.
///
/// The predicates are the three every STRUCTURE-typed filter of this service declares —
/// `Prefix`, `Tag` and `And`
/// (`smithy_query(query="shape",shape="IntelligentTieringFilter,ReplicationRuleFilter,LifecycleRuleFilter")`)
/// — so one table is spellable in every family that carries such a container.
struct FilterCase {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The `<Filter>` element, exactly as the document carries it.
    filter: String,
    /// Whether the schema admits it: a container naming at most the ONE predicate a filter
    /// is a choice of.
    admitted: bool,
}

/// The filter containers every configuration family is judged over.
///
/// The rows differ along what the rule generalises over: WHICH predicates the container names
/// (a prefix, a tag, a conjunction, and each combination of them) and HOW MANY it names
/// directly (one, two and three). A container naming NONE is each family's own documented
/// reading of an empty filter rather than this rule, and is pinned where that family is
/// tested.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "one prefix",
            filter: "<Filter><Prefix>logs/</Prefix></Filter>".to_string(),
            admitted: true,
        },
        FilterCase {
            case: "one tag",
            filter: format!("<Filter>{ONE_TAG}</Filter>"),
            admitted: true,
        },
        FilterCase {
            case: "a conjunction of a prefix and a tag",
            filter: format!("<Filter><And><Prefix>logs/</Prefix>{ONE_TAG}</And></Filter>"),
            admitted: true,
        },
        FilterCase {
            case: "a prefix and a tag, both named directly",
            filter: format!("<Filter><Prefix>logs/</Prefix>{ONE_TAG}</Filter>"),
            admitted: false,
        },
        FilterCase {
            case: "a prefix beside a conjunction",
            filter: format!(
                "<Filter><Prefix>logs/</Prefix><And><Prefix>logs/</Prefix>{ONE_TAG}</And>\
                 </Filter>"
            ),
            admitted: false,
        },
        FilterCase {
            case: "a tag beside a conjunction",
            filter: format!(
                "<Filter>{ONE_TAG}<And><Prefix>logs/</Prefix>{ONE_TAG}</And></Filter>"
            ),
            admitted: false,
        },
        FilterCase {
            case: "a prefix, a tag and a conjunction",
            filter: format!(
                "<Filter><Prefix>logs/</Prefix>{ONE_TAG}\
                 <And><Prefix>logs/</Prefix>{ONE_TAG}</And></Filter>"
            ),
            admitted: false,
        },
    ]
}

/// Every refusal of a document the published schema does not admit: 400 `MalformedXML`,
/// carrying that code's own FIXED sentence and nothing of the request.
fn assert_malformed(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected 400, got {} ({})",
        answer.status,
        text(answer)
    );
    assert_eq!(
        answer.error_code(),
        MALFORMED_XML_CODE,
        "case {case:?}: expected the code {MALFORMED_XML_CODE}, got {}",
        answer.error_code()
    );
    assert_eq!(
        answer.error_message(),
        MALFORMED_XML_MESSAGE,
        "case {case:?}: expected the published sentence {MALFORMED_XML_MESSAGE:?}, got {:?}",
        answer.error_message()
    );
}

/// `PUT /<bucket>?intelligent-tiering&id=<id>` — `PutBucketIntelligentTieringConfiguration`
/// with `filter` as the configuration's filter container.
async fn put_tiering(
    router: &MultiServiceRouter,
    name: &str,
    id: &str,
    filter: &str,
) -> Answer {
    let document = format!(
        "<IntelligentTieringConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id>{filter}\
         <Status>Enabled</Status>\
         <Tiering><Days>90</Days><AccessTier>ARCHIVE_ACCESS</AccessTier></Tiering>\
         </IntelligentTieringConfiguration>"
    );
    send(
        router,
        "PUT",
        &format!("/{name}?intelligent-tiering&id={id}"),
        &[],
        &document,
    )
    .await
}

/// `GET /<bucket>?intelligent-tiering&id=<id>` —
/// `GetBucketIntelligentTieringConfiguration`.
async fn get_tiering(router: &MultiServiceRouter, name: &str, id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}?intelligent-tiering&id={id}"),
        &[],
        "",
    )
    .await
}

/// An S3 Intelligent-Tiering configuration whose `Filter` names more than the one predicate
/// such a container is a choice of is refused as a document that did not validate, and
/// leaves whatever the bucket carried under that id exactly as it was.
#[tokio::test]
async fn an_intelligent_tiering_filter_names_at_most_one_predicate() {
    let router = router();
    let name = "itc-filter-choice";
    bucket(&router, name).await;

    for (index, row) in filter_cases().into_iter().enumerate() {
        let case = row.case;
        let id = format!("choice-{index}");
        // What the bucket carries under this id BEFORE the case's put: a configuration the
        // schema admits, so a refused put is visible as this one surviving.
        let kept = "<Filter><Prefix>kept/</Prefix></Filter>";
        let seeded = put_tiering(&router, name, &id, kept).await;
        assert_eq!(
            seeded.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: the seed configuration must be stored, got {} ({})",
            seeded.status,
            text(&seeded)
        );

        let answer = put_tiering(&router, name, &id, &row.filter).await;
        if row.admitted {
            assert_eq!(
                answer.status,
                PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
                "case {case:?}: expected the put to answer {}, got {} ({})",
                PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
                answer.status,
                text(&answer)
            );
        } else {
            assert_malformed(case, &answer);
        }

        let read = get_tiering(&router, name, &id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the configuration under {id:?} must read back, got {} ({})",
            read.status,
            text(&read)
        );
        // The payload IS the configuration, so the filter the LAST stored put named is the
        // one the read reports: the case's own filter when it was admitted, and the seeded
        // one when it was refused.
        let expected = match row.admitted {
            true => row.filter.as_str(),
            false => kept,
        };
        assert!(
            text(&read).contains(expected),
            "case {case:?}: expected the read to report {expected}, got {}",
            text(&read)
        );
    }
}

/// A configuration a bucket carries no configuration of reads as absent, which is what a
/// refused put of a NEW id leaves behind.
#[tokio::test]
async fn a_refused_intelligent_tiering_filter_records_no_configuration() {
    let router = router();
    let name = "itc-filter-choice-absent";
    bucket(&router, name).await;

    for (index, row) in filter_cases()
        .into_iter()
        .filter(|row| !row.admitted)
        .enumerate()
    {
        let case = row.case;
        let id = format!("never-stored-{index}");
        let answer = put_tiering(&router, name, &id, &row.filter).await;
        assert_malformed(case, &answer);

        let read = get_tiering(&router, name, &id).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: a refused put must record nothing, got {} ({})",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: expected {NO_SUCH_CONFIGURATION_CODE}, got {}",
            read.error_code()
        );
    }
}

/// The ARN of the IAM role a replication configuration names, and the destination bucket as
/// `Destination$Bucket` takes it — an S3 bucket ARN.
const REPLICATION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/s3-replication-role";
const REPLICATION_DESTINATION_ARN: &str = "arn:aws:s3:::replication-destination";

/// `PUT /<bucket>?replication` — `PutBucketReplication` with one V2 rule identified by `id`
/// and scoped by `filter`.
async fn put_replication_filter(
    router: &MultiServiceRouter,
    name: &str,
    id: &str,
    filter: &str,
) -> Answer {
    let document = format!(
        "<ReplicationConfiguration xmlns=\"{XMLNS}\">\
         <Role>{REPLICATION_ROLE_ARN}</Role>\
         <Rule><ID>{id}</ID><Priority>1</Priority>{filter}<Status>Enabled</Status>\
         <DeleteMarkerReplication><Status>Disabled</Status></DeleteMarkerReplication>\
         <Destination><Bucket>{REPLICATION_DESTINATION_ARN}</Bucket></Destination></Rule>\
         </ReplicationConfiguration>"
    );
    send(
        router,
        "PUT",
        &format!("/{name}?replication"),
        &[],
        &document,
    )
    .await
}

/// A replication rule whose `Filter` names more than one predicate directly is refused the
/// same way, and leaves the stored configuration as it was.
#[tokio::test]
async fn a_replication_filter_names_at_most_one_predicate() {
    let router = router();
    let name = "replication-filter-choice";
    bucket(&router, name).await;
    // Replication copies object VERSIONS, so the configuration applies only to a bucket whose
    // versioning is enabled — put in force through this file's one versioning helper. Both
    // ENDS of a replication have to satisfy that, so the destination bucket the rules below
    // name is stood up the same way.
    enable_versioning(&router, name).await;
    // Through the one place this crate reads the bucket out of a destination ARN
    // ([`super::bucket_replication::destination_bucket`]), so no suite repeats that step.
    let destination = super::bucket_replication::destination_bucket(REPLICATION_DESTINATION_ARN);
    bucket(&router, destination).await;
    enable_versioning(&router, destination).await;

    for (index, row) in filter_cases().into_iter().enumerate() {
        let case = row.case;
        // The configuration the bucket carries BEFORE the case's put, under an id of its
        // own, so a refused put is visible as this one surviving.
        let kept = format!("kept-{index}");
        let seeded = put_replication_filter(
            &router,
            name,
            &kept,
            "<Filter><Prefix>kept/</Prefix></Filter>",
        )
        .await;
        assert_eq!(
            seeded.status,
            200,
            "case {case:?}: the seed configuration must be stored, got {} ({})",
            seeded.status,
            text(&seeded)
        );

        let id = format!("choice-{index}");
        let answer = put_replication_filter(&router, name, &id, &row.filter).await;
        if row.admitted {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the put to answer 200, got {} ({})",
                answer.status,
                text(&answer)
            );
        } else {
            assert_malformed(case, &answer);
        }

        let read = send(&router, "GET", &format!("/{name}?replication"), &[], "").await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the stored configuration must read back, got {} ({})",
            read.status,
            text(&read)
        );
        // The payload is the complete container, so the rule the LAST stored put named is the
        // only one the read reports.
        let (reported, absent) = match row.admitted {
            true => (&id, &kept),
            false => (&kept, &id),
        };
        assert!(
            text(&read).contains(&format!("<ID>{reported}</ID>")),
            "case {case:?}: expected the rule {reported:?} to read back, got {}",
            text(&read)
        );
        assert!(
            !text(&read).contains(&format!("<ID>{absent}</ID>")),
            "case {case:?}: expected no rule {absent:?} in the report, got {}",
            text(&read)
        );
    }
}

/// A lifecycle rule's filter is the same choice, judged through the same reading: the family
/// whose own documentation states the rule for five predicates rather than three.
#[tokio::test]
async fn a_lifecycle_filter_names_at_most_one_predicate() {
    let router = router();
    let name = "lifecycle-filter-choice";
    bucket(&router, name).await;

    for (index, row) in filter_cases().into_iter().enumerate() {
        let case = row.case;
        let id = format!("choice-{index}");
        let document = configuration(&[rule(&format!(
            "<ID>{id}</ID><Status>Enabled</Status>{}\
             <Expiration><Days>30</Days></Expiration>",
            row.filter
        ))]);
        let answer = put_lifecycle(&router, name, &[], &document).await;
        if row.admitted {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the put to answer 200, got {} ({})",
                answer.status,
                text(&answer)
            );
        } else {
            assert_malformed(case, &answer);
        }

        let read = send(&router, "GET", &format!("/{name}?lifecycle"), &[], "").await;
        assert_eq!(
            text(&read).contains(&format!("<ID>{id}</ID>")),
            row.admitted,
            "case {case:?}: expected the rule {id:?} to be stored: {}, got {}",
            row.admitted,
            text(&read)
        );
    }
}

/// A lifecycle filter also names the two OBJECT SIZE bounds its own documentation lists, so
/// the same choice holds for a container naming one of them beside another predicate.
///
/// The rows are the bounds the shape declares beside `Prefix`, `Tag` and `And`
/// (`smithy_query(query="shape",shape="LifecycleRuleFilter")`), each on its own and each
/// beside a prefix — a predicate the issue's own family cannot spell, which is why it is
/// pinned here.
#[tokio::test]
async fn a_lifecycle_filter_counts_its_object_size_bounds_as_predicates() {
    let router = router();
    let name = "lifecycle-filter-size-choice";
    bucket(&router, name).await;

    let rows = [
        (
            "one size floor alone",
            "<Filter><ObjectSizeGreaterThan>128</ObjectSizeGreaterThan></Filter>",
            true,
        ),
        (
            "one size ceiling alone",
            "<Filter><ObjectSizeLessThan>4096</ObjectSizeLessThan></Filter>",
            true,
        ),
        (
            "a size floor beside a prefix",
            "<Filter><Prefix>logs/</Prefix>\
             <ObjectSizeGreaterThan>128</ObjectSizeGreaterThan></Filter>",
            false,
        ),
        (
            "both size bounds, named directly",
            "<Filter><ObjectSizeGreaterThan>128</ObjectSizeGreaterThan>\
             <ObjectSizeLessThan>4096</ObjectSizeLessThan></Filter>",
            false,
        ),
        (
            "both size bounds inside the conjunction",
            "<Filter><And><ObjectSizeGreaterThan>128</ObjectSizeGreaterThan>\
             <ObjectSizeLessThan>4096</ObjectSizeLessThan></And></Filter>",
            true,
        ),
    ];

    for (index, (case, filter, admitted)) in rows.into_iter().enumerate() {
        let id = format!("sized-{index}");
        let document = configuration(&[rule(&format!(
            "<ID>{id}</ID><Status>Enabled</Status>{filter}\
             <Expiration><Days>30</Days></Expiration>"
        ))]);
        let answer = put_lifecycle(&router, name, &[], &document).await;
        if admitted {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected the put to answer 200, got {} ({})",
                answer.status,
                text(&answer)
            );
        } else {
            assert_malformed(case, &answer);
        }

        let read = send(&router, "GET", &format!("/{name}?lifecycle"), &[], "").await;
        assert_eq!(
            text(&read).contains(&format!("<ID>{id}</ID>")),
            admitted,
            "case {case:?}: expected the rule {id:?} to be stored: {admitted}, got {}",
            text(&read)
        );
    }
}
