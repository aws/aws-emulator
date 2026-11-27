// SPDX-License-Identifier: CC0-1.0

//! The REGION segment of every ARN this service MINTS, over the awsJson1_0 wire (`POST /`
//! with `X-Amz-Target: AWSParallelComputingService.<Operation>`) through the same router
//! the emulator serves from, on the harness [`crate::cluster_lifecycle`] builds.
//!
//! RULE under test: the Region a request is signed for is caller-supplied text, so an ARN
//! this service mints carries it only as a Region LABEL — non-empty, at most
//! `service_common::arn::MAX_REGION_LENGTH` characters, composed only of `[a-z0-9-]`, the
//! form every AWS model's ARN pattern states for the field — and anything outside that
//! grammar is not interpolated at all, the service's own `DEFAULT_REGION` answering for it.
//! That is an INVARIANT of the mint rather than a property of one signed Region: whatever a
//! caller signs for, the published ARN stays inside the modelled `Arn` bounds and is the
//! key the ARN-keyed surface — `TagResource`, `ListTagsForResource`, `UntagResource` —
//! resolves back to the resource that published it. An ARN whose Region segment carried a
//! field delimiter would move the resource field of its own six-field parse, and one past
//! the modelled length would not fit the member it is read back through; either way the
//! service would hand out an ARN none of its own lookups could resolve.
//!
//! The rows differ along the one part of a minted ARN a request can influence — the signed
//! Region's charset, its case, its length (the published bound for the field and one
//! character past it) and whether it carries an ARN field delimiter — and the sweep below
//! walks every label length and the forms a credential scope cannot even carry.
//!
//! Each test builds its own engine, so the state one test writes is invisible to the
//! others, and nothing here waits on the wall clock.

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{
    cluster_arn, compute_node_group_arn, queue_arn, DEFAULT_REGION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{
    call_signed_for as call, create_cluster, create_compute_node_group, create_queue, emulator,
};

/// The modelled `@length` bounds of the `Arn` shape every tagging operation's
/// `resourceArn` targets (`smithy_query(query="shape", shape="Arn")`), which a minted ARN
/// has to stay inside to be readable back at all.
const ARN_LENGTH: std::ops::RangeInclusive<usize> = 1..=1011;

/// The longest Region label an ARN's Region field carries, stated once by the grammar this
/// tree reads that field with (`service_common::arn::MAX_REGION_LENGTH`: every AWS model's
/// ARN pattern bounds the field at `[a-z0-9-]{1,20}`).
const MAX_REGION_LABEL_LENGTH: usize = service_common::arn::MAX_REGION_LENGTH;

/// A Region label of exactly `length` characters, inside the `[a-z0-9-]` charset.
fn region_of_length(length: usize) -> String {
    let mut label = String::from("us-");
    while label.len() < length {
        label.push('a');
    }
    label.truncate(length);
    label
}

/// One row: how a failing assertion names it, the Region the request signs for, and the
/// Region segment the ARNs that call mints must then carry.
struct Row {
    case: &'static str,
    signed: String,
    minted: String,
    /// The `clusterName` this row's cluster is created under — distinct per row, so the
    /// rows share one emulator without colliding on the name.
    cluster_name: &'static str,
}

/// What a request can sign for, and the Region the ARNs it mints must carry.
///
/// A scope whose Region field is EMPTY or carries `/` is not a row here: the credential
/// scope is `/`-separated and its Region field has to be non-empty for the router to
/// resolve a service from it at all, so such a request reaches no handler. Both forms are
/// covered by the mint sweep below.
fn rows() -> Vec<Row> {
    let default = DEFAULT_REGION.to_string();
    vec![
        Row {
            case: "a deployed Region label",
            signed: "us-east-1".to_string(),
            minted: "us-east-1".to_string(),
            cluster_name: "arn-region-deployed",
        },
        Row {
            case: "a longer label carrying a digit",
            signed: "ap-southeast-4".to_string(),
            minted: "ap-southeast-4".to_string(),
            cluster_name: "arn-region-digit",
        },
        Row {
            case: "a one-character label",
            signed: "a".to_string(),
            minted: "a".to_string(),
            cluster_name: "arn-region-one-char",
        },
        Row {
            case: "a label at the published Region bound",
            signed: region_of_length(MAX_REGION_LABEL_LENGTH),
            minted: region_of_length(MAX_REGION_LABEL_LENGTH),
            cluster_name: "arn-region-at-bound",
        },
        Row {
            case: "one character past the published Region bound",
            signed: region_of_length(MAX_REGION_LABEL_LENGTH + 1),
            minted: default.clone(),
            cluster_name: "arn-region-past-bound",
        },
        Row {
            case: "a label carrying an ARN field delimiter",
            signed: "us-east-1:x".to_string(),
            minted: default.clone(),
            cluster_name: "arn-region-delimiter",
        },
        Row {
            case: "a label that is nothing but ARN field delimiters",
            signed: ":::".to_string(),
            minted: default.clone(),
            cluster_name: "arn-region-delimiters",
        },
        Row {
            case: "an upper-case label",
            signed: "US-EAST-1".to_string(),
            minted: default.clone(),
            cluster_name: "arn-region-upper-case",
        },
        Row {
            case: "a label outside the Region charset",
            signed: "us_east_1".to_string(),
            minted: default.clone(),
            cluster_name: "arn-region-charset",
        },
        Row {
            case: "a label long enough to push the ARN past its modelled length",
            signed: "a".repeat(*ARN_LENGTH.end()),
            minted: default,
            cluster_name: "arn-region-overlong",
        },
    ]
}

/// Every assertion a minted ARN must satisfy: the Region segment the mint owes, the fixed
/// part the service builds around it, and the modelled `Arn` shape — `@length(1..1011)`
/// and the pattern `^arn:aws.*:pcs:.*:[0-9]{12}:.*/[a-z0-9_\-]+$`, whose trailing
/// `[a-z0-9_\-]+` id segment is what the tagging operations read the value through.
///
/// Checked structurally, so the suite needs no regex engine: `prefix` is everything the
/// ARN carries ahead of that trailing id.
fn assert_minted_arn(case: &str, arn: &str, minted_region: &str, resource_prefix: &str) {
    let expected = format!(
        "arn:aws:pcs:{minted_region}:{}:{resource_prefix}",
        engine::EMULATOR_ACCOUNT_ID
    );
    let Some(id) = arn.strip_prefix(expected.as_str()) else {
        panic!("case {case:?}: expected an ARN starting {expected:?}, got {arn:?}");
    };
    assert!(
        !id.is_empty()
            && id.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || byte == b'_'
                || byte == b'-'),
        "case {case:?}: the id the ARN ends in must match [a-z0-9_-]+, got {id:?} in {arn:?}"
    );
    assert!(
        ARN_LENGTH.contains(&arn.len()),
        "case {case:?}: Arn is @length({}..{}), got {} characters in {arn:?}",
        ARN_LENGTH.start(),
        ARN_LENGTH.end(),
        arn.len()
    );
}

/// A string member of a response body, by path.
fn string_at(case: &str, body: &Value, path: &[&str]) -> String {
    let mut cursor = body;
    for step in path {
        cursor = &cursor[step];
    }
    cursor
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {path:?} in {body}"))
        .to_string()
}

/// Drive the whole ARN-keyed tagging surface over `arn`, signing for `region`: a write, the
/// read that must answer it back, and the removal that must leave the set empty. An ARN the
/// service published and its own lookups cannot resolve shows up here as the
/// `ResourceNotFoundException` the tagging operations answer an unknown ARN with.
async fn assert_tagging_resolves(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
) {
    let (status, body) = call(
        router,
        region,
        "TagResource",
        &json!({"resourceArn": arn, "tags": {"minted": "by-this-service"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource on the published ARN {arn:?} to answer 200, got \
         {status} {body}"
    );

    let (status, body) = call(
        router,
        region,
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource on {arn:?} to answer 200, got {status} \
         {body}"
    );
    assert_eq!(
        body["tags"],
        json!({"minted": "by-this-service"}),
        "case {case:?}: expected the tag written through {arn:?} to read back, got {body}"
    );

    let (status, body) = call(
        router,
        region,
        "UntagResource",
        &json!({"resourceArn": arn, "tagKeys": ["minted"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UntagResource on {arn:?} to answer 200, got {status} {body}"
    );

    let (status, body) = call(
        router,
        region,
        "ListTagsForResource",
        &json!({"resourceArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource on {arn:?} to answer 200, got {status} \
         {body}"
    );
    assert_eq!(
        body["tags"],
        json!({}),
        "case {case:?}: expected the removed tag to be gone from {arn:?}, got {body}"
    );
}

#[tokio::test]
async fn every_published_arn_is_one_the_tagging_operations_resolve_whatever_region_was_signed_for()
{
    let router = emulator();
    let mut minted: Vec<String> = Vec::new();

    for row in rows() {
        let case = row.case;
        let region = row.signed.as_str();
        let want = row.minted.as_str();

        let (status, body) = call(
            &router,
            region,
            "CreateCluster",
            &create_cluster(row.cluster_name, "SMALL"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateCluster to answer 200, got {status} {body}"
        );
        let cluster_id = string_at(case, &body, &["cluster", "id"]);
        let cluster_arn_published = string_at(case, &body, &["cluster", "arn"]);
        assert_minted_arn(case, &cluster_arn_published, want, "cluster/");

        let (status, body) = call(
            &router,
            region,
            "CreateComputeNodeGroup",
            &create_compute_node_group(&cluster_id, "arn-region-nodes"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateComputeNodeGroup to answer 200, got {status} {body}"
        );
        let group_arn = string_at(case, &body, &["computeNodeGroup", "arn"]);
        assert_minted_arn(
            case,
            &group_arn,
            want,
            &format!("cluster/{cluster_id}/computenodegroup/"),
        );

        let (status, body) = call(
            &router,
            region,
            "CreateQueue",
            &create_queue(&cluster_id, "arn-region-queue", &["arn-region-nodes"]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateQueue to answer 200, got {status} {body}"
        );
        let queue_arn_published = string_at(case, &body, &["queue", "arn"]);
        assert_minted_arn(
            case,
            &queue_arn_published,
            want,
            &format!("cluster/{cluster_id}/queue/"),
        );

        // The ARN the create published is the one the identifier-keyed reader reports, so
        // there is ONE ARN per resource rather than one per lookup path.
        let (status, body) = call(
            &router,
            region,
            "GetCluster",
            &json!({"clusterIdentifier": row.cluster_name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetCluster to answer 200, got {status} {body}"
        );
        assert_eq!(
            string_at(case, &body, &["cluster", "arn"]),
            cluster_arn_published,
            "case {case:?}: expected GetCluster to report the ARN the create published, got \
             {body}"
        );

        for arn in [&cluster_arn_published, &group_arn, &queue_arn_published] {
            assert_tagging_resolves(&router, case, region, arn).await;
            minted.push(arn.clone());
        }
    }

    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "each resource must publish an ARN of its own, got {minted:?}"
    );
}

#[test]
fn the_mint_holds_the_region_segment_to_a_region_label_at_every_length_and_form() {
    // Every label LENGTH a caller could sign for, the dimension that decides whether the
    // minted ARN still fits the member it is read back through. A few characters past the
    // bound is enough: the grammar is a length test, not a prefix test.
    for length in 1..=(MAX_REGION_LABEL_LENGTH + 5) {
        let candidate = region_of_length(length);
        let case = format!("a Region label of {length} characters");
        let want = if length <= MAX_REGION_LABEL_LENGTH {
            candidate.as_str()
        } else {
            DEFAULT_REGION
        };
        assert_minted_arn(
            &case,
            &cluster_arn(&candidate, "pcs_0123456789abcdef"),
            want,
            "cluster/",
        );
        assert_minted_arn(
            &case,
            &compute_node_group_arn(&candidate, "pcs_cluster", "pcs_0123456789abcdef"),
            want,
            "cluster/pcs_cluster/computenodegroup/",
        );
        assert_minted_arn(
            &case,
            &queue_arn(&candidate, "pcs_cluster", "pcs_0123456789abcdef"),
            want,
            "cluster/pcs_cluster/queue/",
        );
    }

    // The forms the charset rejects at any length, the empty Region a request with no
    // credential scope hands the mint included, and the delimiters that would move an
    // ARN's own fields.
    for candidate in [
        "",
        "US-EAST-1",
        "us_east_1",
        "us east 1",
        "us-east-1:x",
        ":",
        "us-east-1/x",
        "us-east-1.",
        "us-east-1:000000000000:cluster",
    ] {
        let case = format!("the Region {candidate:?}");
        assert_minted_arn(
            &case,
            &cluster_arn(candidate, "pcs_0123456789abcdef"),
            DEFAULT_REGION,
            "cluster/",
        );
        assert_minted_arn(
            &case,
            &compute_node_group_arn(candidate, "pcs_cluster", "pcs_0123456789abcdef"),
            DEFAULT_REGION,
            "cluster/pcs_cluster/computenodegroup/",
        );
        assert_minted_arn(
            &case,
            &queue_arn(candidate, "pcs_cluster", "pcs_0123456789abcdef"),
            DEFAULT_REGION,
            "cluster/pcs_cluster/queue/",
        );
    }
}
