// SPDX-License-Identifier: CC0-1.0

//! The Aurora DSQL cluster lifecycle, end to end over the wire.
//!
//! Every case here drives the real request path — `MultiServiceRouter::dispatch`
//! with a signed-looking restJson1 request — so the routes, the `@httpLabel` /
//! `@httpQuery` bindings, the boundary constraint checks and the handlers are all
//! exercised the way a client exercises them.
//!
//! The tables assert RULES over a class of inputs, not one recorded request: the
//! shape of a minted identifier and ARN for several regions, the round trip of every
//! optional member a cluster can carry, pagination for several page sizes over
//! several cluster counts, and the refusals for the whole class of malformed
//! identifiers.
//!
//! The `Dsql` harness in `tests/all/harness.rs` is this crate's ONE test client:
//! the sibling suites (`tests/all/resource_tags.rs`, `tests/all/error_contract.rs`,
//! `tests/all/stream_lifecycle.rs`) drive the same `dispatch` path through it
//! rather than building a second one.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    new_vpc_endpoint_service_suffix, Cluster, ComAmazonawsDsqlDsqlState, PendingTransition,
    RecordedCall, TokenScope, MAX_CLUSTERS, MAX_CREATE_TOKENS, MAX_RETIRED_CLUSTERS,
    STATUS_DELETED,
};
use service_common::data_plane_addressing::{
    announced_zone_of_authority, AWS_ENDPOINT_DNS_SUFFIXES, LOOPBACK_ZONE,
};

use crate::harness::{
    absent_identifier, expected_arn, is_cluster_id, wait_for_the_clock, Dsql,
};

/// The values `ClusterStatus` accepts on the wire. Source: the service's own model,
/// `smithy_query(query="shape", shape="ClusterStatus")`.
const CLUSTER_STATUS_VALUES: &[&str] = &[
    "CREATING",
    "ACTIVE",
    "IDLE",
    "INACTIVE",
    "UPDATING",
    "DELETING",
    "DELETED",
    "FAILED",
    "PENDING_SETUP",
    "PENDING_DELETE",
];

/// `maxResults` published bounds. Source: `smithy_query(query="operation",
/// operation="ListClusters")` — `@range(min: 1, max: 100)`, `@default: 20`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// One `CreateCluster` request and what the persisted cluster must then report.
struct CreateCase {
    name: &'static str,
    region: &'static str,
    request: Value,
    deletion_protection: bool,
    /// The tag set `GetCluster` must then REPORT: the empty map for a cluster created
    /// with no tags, since a read reports the set itself rather than leaving the member
    /// out.
    tags: Value,
    encryption_type: &'static str,
    kms_key_arn: Option<String>,
    multi_region_properties: Value,
}

fn create_cases() -> Vec<CreateCase> {
    let peer = format!(
        "arn:aws:dsql:us-west-2:{}:cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa",
        engine::EMULATOR_ACCOUNT_ID
    );
    vec![
        CreateCase {
            name: "no members at all: deletionProtectionEnabled takes its modeled default",
            region: "us-east-1",
            request: json!({}),
            deletion_protection: true,
            tags: json!({}),
            encryption_type: "AWS_OWNED_KMS_KEY",
            kms_key_arn: None,
            multi_region_properties: Value::Null,
        },
        CreateCase {
            name: "protection off, one tag",
            region: "eu-west-1",
            request: json!({"deletionProtectionEnabled": false, "tags": {"env": "test"}}),
            deletion_protection: false,
            tags: json!({"env": "test"}),
            encryption_type: "AWS_OWNED_KMS_KEY",
            kms_key_arn: None,
            multi_region_properties: Value::Null,
        },
        CreateCase {
            name: "protection on, several tags",
            region: "ap-southeast-2",
            request: json!({
                "deletionProtectionEnabled": true,
                "tags": {"env": "prod", "owner": "team-a", "cost-centre": "42"}
            }),
            deletion_protection: true,
            tags: json!({"env": "prod", "owner": "team-a", "cost-centre": "42"}),
            encryption_type: "AWS_OWNED_KMS_KEY",
            kms_key_arn: None,
            multi_region_properties: Value::Null,
        },
        CreateCase {
            name: "customer-managed key given as a bare key id",
            region: "us-east-2",
            request: json!({"deletionProtectionEnabled": false, "kmsEncryptionKey": "1234abcd-12ab-34cd-56ef"}),
            deletion_protection: false,
            tags: json!({}),
            encryption_type: "CUSTOMER_MANAGED_KMS_KEY",
            kms_key_arn: Some(format!(
                "arn:aws:kms:us-east-2:{}:key/1234abcd-12ab-34cd-56ef",
                engine::EMULATOR_ACCOUNT_ID
            )),
            multi_region_properties: Value::Null,
        },
        CreateCase {
            name: "customer-managed key given as an alias",
            region: "us-west-1",
            request: json!({"deletionProtectionEnabled": false, "kmsEncryptionKey": "alias/my-dsql-key"}),
            deletion_protection: false,
            tags: json!({}),
            encryption_type: "CUSTOMER_MANAGED_KMS_KEY",
            kms_key_arn: Some(format!(
                "arn:aws:kms:us-west-1:{}:alias/my-dsql-key",
                engine::EMULATOR_ACCOUNT_ID
            )),
            multi_region_properties: Value::Null,
        },
        CreateCase {
            name: "customer-managed key given as a full ARN",
            region: "us-east-1",
            request: json!({
                "deletionProtectionEnabled": false,
                "kmsEncryptionKey": format!("arn:aws:kms:us-east-1:{}:key/abcd", engine::EMULATOR_ACCOUNT_ID)
            }),
            deletion_protection: false,
            tags: json!({}),
            encryption_type: "CUSTOMER_MANAGED_KMS_KEY",
            kms_key_arn: Some(format!(
                "arn:aws:kms:us-east-1:{}:key/abcd",
                engine::EMULATOR_ACCOUNT_ID
            )),
            multi_region_properties: Value::Null,
        },
        CreateCase {
            name: "multi-Region cluster with a witness region and a peer",
            region: "us-east-1",
            request: json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": {"witnessRegion": "us-west-2", "clusters": [peer.clone()]}
            }),
            deletion_protection: false,
            tags: json!({}),
            encryption_type: "AWS_OWNED_KMS_KEY",
            kms_key_arn: None,
            multi_region_properties: json!({"witnessRegion": "us-west-2", "clusters": [peer]}),
        },
    ]
}

/// RULE: `CreateCluster` mints and PERSISTS a cluster, and `GetCluster` reports back
/// exactly what was persisted — for every combination of optional members and in
/// every region.
#[tokio::test]
async fn create_persists_a_cluster_that_get_reports_back() {
    for case in create_cases() {
        let dsql = Dsql::in_region(case.region);
        let created = dsql.create(case.request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCluster must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );

        let identifier = created.str("identifier");
        assert!(
            is_cluster_id(&identifier),
            "case {:?}: identifier must match ^[a-z0-9]{{26}}$, got {identifier:?}",
            case.name
        );
        assert_eq!(
            created.str("arn"),
            expected_arn(case.region, &identifier),
            "case {:?}: arn must name the minted cluster in the signed region",
            case.name
        );
        assert_eq!(
            created.str("status"),
            "CREATING",
            "case {:?}: a freshly minted cluster reports CREATING, got {:?}",
            case.name,
            created.str("status")
        );
        assert_eq!(
            created.get("deletionProtectionEnabled"),
            json!(case.deletion_protection),
            "case {:?}: deletionProtectionEnabled must be the effective value",
            case.name
        );
        assert_eq!(
            created.str("endpoint"),
            LOOPBACK_ZONE,
            "case {:?}: endpoint must name the host this emulator is served at, which for \
             one embedded in a process is the loopback zone, got {:?}",
            case.name,
            created.str("endpoint")
        );
        let creation_time = created.get("creationTime");
        assert!(
            creation_time.as_f64().is_some_and(|t| t > 0.0),
            "case {:?}: creationTime must be a timestamp, got {creation_time:?}",
            case.name
        );

        // The round trip: everything above came out of the store, not out of the
        // request, so a read through GetCluster reports the same values.
        let fetched = dsql.get(&identifier).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: GetCluster on the created cluster must answer 200, got {} {:?}",
            case.name, fetched.status, fetched.body
        );
        assert_eq!(
            fetched.str("identifier"),
            identifier,
            "case {:?}: GetCluster must report the stored identifier",
            case.name
        );
        assert_eq!(
            fetched.str("arn"),
            created.str("arn"),
            "case {:?}: GetCluster must report the stored arn",
            case.name
        );
        assert_eq!(
            fetched.str("endpoint"),
            created.str("endpoint"),
            "case {:?}: GetCluster must report the stored endpoint {:?}, got {:?}",
            case.name,
            created.str("endpoint"),
            fetched.str("endpoint")
        );
        assert_eq!(
            fetched.get("creationTime"),
            creation_time,
            "case {:?}: creationTime must not change between reads",
            case.name
        );
        assert_eq!(
            fetched.get("deletionProtectionEnabled"),
            json!(case.deletion_protection),
            "case {:?}: GetCluster must report the stored deletionProtectionEnabled",
            case.name
        );
        assert_eq!(
            fetched.get("tags"),
            case.tags,
            "case {:?}: GetCluster must report the tags supplied at create time, got {:?}",
            case.name,
            fetched.get("tags")
        );
        assert_eq!(
            fetched.get("multiRegionProperties"),
            case.multi_region_properties,
            "case {:?}: GetCluster must report the stored multiRegionProperties, got {:?}",
            case.name,
            fetched.get("multiRegionProperties")
        );
        assert_eq!(
            fetched
                .get("encryptionDetails")
                .get("encryptionType")
                .cloned()
                .unwrap_or(Value::Null),
            json!(case.encryption_type),
            "case {:?}: encryptionType must follow the supplied kmsEncryptionKey, got {:?}",
            case.name,
            fetched.get("encryptionDetails")
        );
        assert_eq!(
            fetched
                .get("encryptionDetails")
                .get("kmsKeyArn")
                .and_then(Value::as_str)
                .map(str::to_string),
            case.kms_key_arn,
            "case {:?}: kmsKeyArn must be the supplied key expanded into an ARN, got {:?}",
            case.name,
            fetched.get("encryptionDetails")
        );
        assert!(
            CLUSTER_STATUS_VALUES.contains(&fetched.str("status").as_str()),
            "case {:?}: status must be a ClusterStatus value, got {:?}",
            case.name,
            fetched.str("status")
        );
    }
}

/// The endpoint bases this emulator can be served at, with the host a cluster's
/// connection endpoints are announced at when it is served there.
///
/// The rows span the forms that address takes: a loopback literal with a port, a name
/// with another port, a name with no port at all, a bracketed IPv6 literal (a host
/// `Endpoint`'s `^[a-zA-Z0-9.-]+$` cannot carry) and the base of an emulator served on no
/// port at all (`smithy_server_runtime::UNSERVED_URL_BASE`, the base of one embedded in
/// another process). The announced host is the served host itself, with the loopback zone
/// — the interface the core binds its listeners on — standing in wherever that host is not
/// one a hostname member can carry.
const ENDPOINT_BASES: [(&str, &str); 5] = [
    ("https://127.0.0.1:9297", "127.0.0.1"),
    ("https://localhost:39355", "localhost"),
    ("https://emulator-host", "emulator-host"),
    ("https://[::1]:8443", LOOPBACK_ZONE),
    (smithy_server_runtime::UNSERVED_URL_BASE, LOOPBACK_ZONE),
];

/// Whether `value` is a host `Endpoint` and `ClusterVpcEndpoint` admit: non-empty, and
/// drawn from `^[a-zA-Z0-9.-]+$`. Source: `smithy_query(query="shape", shape="Endpoint")`.
fn is_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// RULE: every response that reports the address a client dials to reach a cluster's data
/// plane — `CreateCluster`'s own answer, the retry a repeated `clientToken` replays and
/// `GetCluster`'s read of the stored cluster — names the host THIS emulator is served at, so
/// a client that dials the value reaches this process; and the `clusterVpcEndpoint`
/// `GetVpcEndpointServiceName` reports names the CLUSTER, under the zone this emulator
/// announces its own names in. Never a host in a zone Amazon Web Services publishes its own
/// endpoints in, whatever Region the request is signed for.
#[tokio::test]
async fn every_reported_cluster_endpoint_names_this_emulators_own_host() {
    for (base, announced) in ENDPOINT_BASES {
        // Two Regions per base: the announced host is where this process answers, so it
        // follows the endpoint the emulator is served at and not the request's Region.
        for region in ["us-east-1", "eu-west-1"] {
            let case = format!("served at {base:?} in {region}");
            let dsql = Dsql::served_at(region, base);
            let body = json!({"deletionProtectionEnabled": false, "clientToken": "tok-endpoint"});
            let created = dsql.create(body.clone()).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateCluster must answer 200, got {} {:?}",
                created.status, created.body
            );
            let identifier = created.str("identifier");

            // Each reported value, asked for the way a client asks for it.
            let replayed = dsql.create(body).await;
            let fetched = dsql.get(&identifier).await;
            let fronted = dsql
                .call(
                    "GET",
                    &format!("/clusters/{identifier}/vpc-endpoint-service-name"),
                    None,
                )
                .await;
            for (member, reported, wanted) in [
                (
                    "CreateCluster's endpoint",
                    created.str("endpoint"),
                    announced.to_string(),
                ),
                (
                    "the replayed create's endpoint",
                    replayed.str("endpoint"),
                    announced.to_string(),
                ),
                (
                    "GetCluster's endpoint",
                    fetched.str("endpoint"),
                    announced.to_string(),
                ),
            ] {
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: {member} must be {wanted:?}, got {reported:?}"
                );
                assert!(
                    is_endpoint(&reported),
                    "case {case:?}: {member} must satisfy Endpoint's ^[a-zA-Z0-9.-]+$, \
                     got {reported:?}"
                );
                for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
                    assert!(
                        !reported.ends_with(suffix),
                        "case {case:?}: {member} must name no endpoint of Amazon Web \
                         Services' own ({suffix}), got {reported:?}"
                    );
                }
            }

            // The VPC access path is a name of the CLUSTER's, in the zone this emulator may
            // announce a name of its own in — read through the one helper the service reads it
            // through, so the expectation cannot drift from the handler's own reading.
            let zone = announced_zone_of_authority(announced);
            let vpc_host = fronted.str("clusterVpcEndpoint");
            assert!(
                vpc_host.starts_with(&format!("{identifier}.")),
                "case {case:?}: clusterVpcEndpoint must name the cluster {identifier:?}, got \
                 {vpc_host:?}"
            );
            assert!(
                vpc_host.ends_with(&format!(".{region}.{zone}")),
                "case {case:?}: clusterVpcEndpoint must end in the cluster's Region and this \
                 emulator's zone ({region}.{zone}), got {vpc_host:?}"
            );
            assert!(
                is_endpoint(&vpc_host),
                "case {case:?}: clusterVpcEndpoint must satisfy ^[a-zA-Z0-9.-]+$, got \
                 {vpc_host:?}"
            );
            for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
                assert!(
                    !vpc_host.ends_with(suffix),
                    "case {case:?}: clusterVpcEndpoint must name no endpoint of Amazon Web \
                     Services' own ({suffix}), got {vpc_host:?}"
                );
            }
            assert_eq!(
                replayed.str("identifier"),
                identifier,
                "case {case:?}: the replayed create must answer the first cluster, got {:?}",
                replayed.str("identifier")
            );
        }
    }
}

/// RULE: each `CreateCluster` without a repeated `clientToken` mints a NEW cluster;
/// a repeated `clientToken` returns the first one and mints nothing.
#[tokio::test]
async fn identifiers_are_fresh_per_call_and_idempotent_per_client_token() {
    let dsql = Dsql::new();

    let mut minted = BTreeSet::new();
    for n in 0..5u32 {
        let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
        let identifier = created.str("identifier");
        assert!(
            minted.insert(identifier.clone()),
            "call {n}: CreateCluster must mint a fresh identifier, got the repeat {identifier:?}"
        );
    }

    for token in ["token-a", "token-b", "a-much-longer-client-token-0123456789"] {
        let first = dsql
            .create(json!({"deletionProtectionEnabled": false, "clientToken": token}))
            .await;
        let second = dsql
            .create(json!({"deletionProtectionEnabled": false, "clientToken": token}))
            .await;
        assert_eq!(
            first.str("identifier"),
            second.str("identifier"),
            "case {token:?}: a repeated clientToken must return the first result, got {:?} then {:?}",
            first.str("identifier"),
            second.str("identifier")
        );
        minted.insert(first.str("identifier"));
    }

    let listed = dsql.list("?max-results=100").await;
    let count = listed
        .get("clusters")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        count,
        minted.len(),
        "a repeated clientToken must not create a second cluster: listed {count}, minted {}",
        minted.len()
    );
}

/// RULE: a cluster the clock owes a transition reports the transient status until the
/// transition comes due, and the settled one afterwards — without anybody calling.
#[tokio::test]
async fn the_clock_settles_a_created_cluster_into_active() {
    let dsql = Dsql::new();
    let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
    let identifier = created.str("identifier");

    let immediately = dsql.get(&identifier).await;
    assert_eq!(
        immediately.str("status"),
        "CREATING",
        "a caller reading straight after CreateCluster still sees CREATING, got {:?}",
        immediately.str("status")
    );

    wait_for_the_clock().await;
    let settled = dsql.get(&identifier).await;
    assert_eq!(
        settled.str("status"),
        "ACTIVE",
        "once the transition is due the cluster is ACTIVE, got {:?}",
        settled.str("status")
    );
    assert_eq!(
        settled.get("creationTime"),
        immediately.get("creationTime"),
        "settling a status must not move creationTime"
    );
}

/// RULE: `UpdateCluster` applies ONLY the members the request carries, and a later
/// `GetCluster` observes exactly those changes.
#[tokio::test]
async fn update_mutates_only_the_members_the_request_carries() {
    struct UpdateCase {
        name: &'static str,
        request: Value,
        want_protection: bool,
        want_encryption_type: &'static str,
        want_witness: Value,
        /// The status the clock settles the reconfigured cluster into: `ACTIVE`,
        /// or `PENDING_SETUP` while a multi-Region peering the update asks for is
        /// not established on both sides (see
        /// `tests/all/multi_region_cluster_status.rs`).
        want_settled_status: &'static str,
    }

    let peer = format!(
        "arn:aws:dsql:us-west-2:{}:cluster/bbbbbbbbbbbbbbbbbbbbbbbbbb",
        engine::EMULATOR_ACCOUNT_ID
    );
    let cases = vec![
        UpdateCase {
            name: "empty request changes nothing",
            request: json!({}),
            want_protection: false,
            want_encryption_type: "AWS_OWNED_KMS_KEY",
            want_witness: Value::Null,
            want_settled_status: "ACTIVE",
        },
        UpdateCase {
            name: "deletionProtectionEnabled only",
            request: json!({"deletionProtectionEnabled": true}),
            want_protection: true,
            want_encryption_type: "AWS_OWNED_KMS_KEY",
            want_witness: Value::Null,
            want_settled_status: "ACTIVE",
        },
        UpdateCase {
            name: "kmsEncryptionKey only",
            request: json!({"kmsEncryptionKey": "alias/rotated"}),
            want_protection: false,
            want_encryption_type: "CUSTOMER_MANAGED_KMS_KEY",
            want_witness: Value::Null,
            want_settled_status: "ACTIVE",
        },
        UpdateCase {
            name: "multiRegionProperties only",
            request: json!({"multiRegionProperties": {"witnessRegion": "eu-west-2", "clusters": [peer]}}),
            want_protection: false,
            want_encryption_type: "AWS_OWNED_KMS_KEY",
            want_witness: json!("eu-west-2"),
            want_settled_status: "PENDING_SETUP",
        },
        UpdateCase {
            name: "every updatable member at once",
            request: json!({
                "deletionProtectionEnabled": true,
                "kmsEncryptionKey": "alias/rotated",
                "multiRegionProperties": {"witnessRegion": "eu-central-1"}
            }),
            want_protection: true,
            want_encryption_type: "CUSTOMER_MANAGED_KMS_KEY",
            want_witness: json!("eu-central-1"),
            want_settled_status: "PENDING_SETUP",
        },
    ];

    for case in cases {
        let dsql = Dsql::new();
        let identifier = dsql.create_settled().await;
        let before = dsql.get(&identifier).await;

        let updated = dsql.update(&identifier, case.request.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: UpdateCluster must answer 200, got {} {:?}",
            case.name, updated.status, updated.body
        );
        assert_eq!(
            updated.str("identifier"),
            identifier,
            "case {:?}: UpdateCluster must report the cluster it mutated",
            case.name
        );
        assert_eq!(
            updated.str("arn"),
            before.str("arn"),
            "case {:?}: an update must not change the cluster ARN",
            case.name
        );
        assert_eq!(
            updated.get("creationTime"),
            before.get("creationTime"),
            "case {:?}: an update must not change creationTime",
            case.name
        );
        assert_eq!(
            updated.str("status"),
            "UPDATING",
            "case {:?}: a reconfigured cluster reports UPDATING, got {:?}",
            case.name,
            updated.str("status")
        );

        wait_for_the_clock().await;
        let after = dsql.get(&identifier).await;
        assert_eq!(
            after.str("status"),
            case.want_settled_status,
            "case {:?}: the clock must settle UPDATING into {:?}, got {:?}",
            case.name,
            case.want_settled_status,
            after.str("status")
        );
        assert_eq!(
            after.get("deletionProtectionEnabled"),
            json!(case.want_protection),
            "case {:?}: deletionProtectionEnabled after the update",
            case.name
        );
        assert_eq!(
            after
                .get("encryptionDetails")
                .get("encryptionType")
                .cloned()
                .unwrap_or(Value::Null),
            json!(case.want_encryption_type),
            "case {:?}: encryptionType after the update, got {:?}",
            case.name,
            after.get("encryptionDetails")
        );
        assert_eq!(
            after
                .get("multiRegionProperties")
                .get("witnessRegion")
                .cloned()
                .unwrap_or(Value::Null),
            case.want_witness,
            "case {:?}: multiRegionProperties after the update, got {:?}",
            case.name,
            after.get("multiRegionProperties")
        );
        assert_eq!(
            after.get("tags"),
            before.get("tags"),
            "case {:?}: an update must not touch members the request did not carry",
            case.name
        );
    }
}

/// RULE: deletion protection refuses the delete; with it off the cluster goes away,
/// and once it is gone `GetCluster` answers `ResourceNotFoundException` and
/// `ListClusters` no longer lists it.
#[tokio::test]
async fn delete_is_refused_while_protected_and_retires_the_cluster_once_not() {
    let dsql = Dsql::new();
    let created = dsql.create(json!({"deletionProtectionEnabled": true})).await;
    let identifier = created.str("identifier");
    wait_for_the_clock().await;

    let refused = dsql.delete(&identifier).await;
    assert_eq!(
        refused.status, 400,
        "a protected cluster must refuse deletion with 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "the refusal must be a ValidationException \u{2014} the condition is permanent until the \
         caller changes the cluster, not a transient conflict \u{2014} got {:?}",
        refused.error
    );
    assert_eq!(
        dsql.get(&identifier).await.status,
        200,
        "a refused delete must leave the cluster in place"
    );

    let unprotected = dsql
        .update(&identifier, json!({"deletionProtectionEnabled": false}))
        .await;
    assert_eq!(
        unprotected.status, 200,
        "disabling deletion protection must succeed, got {} {:?}",
        unprotected.status, unprotected.body
    );
    wait_for_the_clock().await;

    let deleted = dsql.delete(&identifier).await;
    assert_eq!(
        deleted.status, 200,
        "an unprotected cluster must delete, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.str("status"),
        "DELETING",
        "a cluster accepted for deletion reports DELETING, got {:?}",
        deleted.str("status")
    );
    assert_eq!(
        deleted.str("arn"),
        created.str("arn"),
        "DeleteCluster must report the stored ARN"
    );
    assert_eq!(
        deleted.get("creationTime"),
        created.get("creationTime"),
        "DeleteCluster must report the stored creationTime"
    );

    wait_for_the_clock().await;
    let gone = dsql.get(&identifier).await;
    assert_eq!(
        gone.status, 404,
        "once the deletion is due GetCluster must answer 404, got {} {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error.as_deref(),
        Some("ResourceNotFoundException"),
        "the 404 must be a ResourceNotFoundException, got {:?}",
        gone.error
    );
    let listed = dsql.list("?max-results=100").await;
    assert_eq!(
        listed.get("clusters"),
        json!([]),
        "a retired cluster must not be listed, got {:?}",
        listed.get("clusters")
    );
}

/// RULE: `ListClusters` enumerates every persisted cluster, `maxResults` per page,
/// resuming from the token the previous page emitted, and the last page omits the
/// token. Asserted for several cluster counts crossed with several page sizes.
#[tokio::test]
async fn list_pages_through_every_cluster_exactly_once() {
    for cluster_count in [0usize, 1, 2, 5, 7] {
        let dsql = Dsql::new();
        let mut all = BTreeSet::new();
        for _ in 0..cluster_count {
            let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
            all.insert(created.str("identifier"));
        }

        for page_size in [1usize, 2, 3, 20, 100] {
            let case = format!("{cluster_count} clusters, max-results={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;

            loop {
                pages += 1;
                assert!(
                    pages <= cluster_count + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                let query = match &token {
                    Some(t) => format!("?max-results={page_size}&next-token={t}"),
                    None => format!("?max-results={page_size}"),
                };
                let page = dsql.list(&query).await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?}: ListClusters must answer 200, got {} {:?}",
                    page.status, page.body
                );
                let items = page
                    .get("clusters")
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: clusters is @required"));
                assert!(
                    items.len() <= page_size,
                    "case {case:?}: a page must hold at most max-results items, got {}",
                    items.len()
                );
                for item in &items {
                    let identifier = item
                        .get("identifier")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        is_cluster_id(&identifier),
                        "case {case:?}: a summary identifier must match ^[a-z0-9]{{26}}$, got {identifier:?}"
                    );
                    assert_eq!(
                        item.get("arn").and_then(Value::as_str).unwrap_or_default(),
                        expected_arn("us-east-1", &identifier),
                        "case {case:?}: a summary must carry the stored ARN"
                    );
                    seen.push(identifier);
                }
                token = page
                    .get("nextToken")
                    .as_str()
                    .map(str::to_string);
                if token.is_none() {
                    break;
                }
                assert_eq!(
                    items.len(),
                    page_size,
                    "case {case:?}: a token may only be emitted when the page was full"
                );
            }

            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: pagination returned a duplicate: {seen:?}"
            );
            assert_eq!(
                unique, all,
                "case {case:?}: pagination must return every cluster exactly once"
            );
        }
    }
}

/// RULE: a `nextToken` this service did not issue is refused as invalid input, not
/// served as an empty page or as a page starting after some arbitrary key.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let dsql = Dsql::new();
    dsql.create_settled().await;

    // A cursor is the hex of a `ClusterId`, so the class of things that are not one
    // spans both "not hex at all" and "hex of something that is not an identifier".
    let fabricated_hex = [
        ::hex::encode("hello"),
        ::hex::encode("AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ::hex::encode("aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ::hex::encode([0xff, 0xfe]),
    ];
    let tokens: Vec<&str> = ["not-a-cursor", "zzzz", "ff00ffzz", "0"]
        .into_iter()
        .chain(fabricated_hex.iter().map(String::as_str))
        .collect();

    for token in tokens {
        let answer = dsql.list(&format!("?max-results=10&next-token={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: an unissued next-token must be refused with 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {token:?}: the refusal must be a ValidationException, got {:?}",
            answer.error
        );
        // The MODELLED shape, so the `@required` `reason` is there to branch on.
        // The whole body contract is asserted in
        // `tests/all/modeled_validation_exception.rs`.
        assert_eq!(
            answer.str("reason"),
            "fieldValidationFailed",
            "case {token:?}: the modelled reason must say the member's value failed, got {:?}",
            answer.body
        );
    }
}

/// RULE: `maxResults` is honoured inside its published range and refused outside it.
#[tokio::test]
async fn max_results_is_enforced_at_its_published_bounds() {
    let dsql = Dsql::new();
    for _ in 0..3 {
        dsql.create(json!({"deletionProtectionEnabled": false})).await;
    }

    for value in [MAX_RESULTS_MIN, 2, 50, MAX_RESULTS_MAX] {
        let answer = dsql.list(&format!("?max-results={value}")).await;
        assert_eq!(
            answer.status, 200,
            "case max-results={value}: inside the published range must be accepted, got {} {:?}",
            answer.status, answer.body
        );
    }

    for value in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = dsql.list(&format!("?max-results={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case max-results={value}: outside the published range must be refused, got {} {:?}",
            answer.status, answer.body
        );
    }

    // The modeled default page size applies when the member is omitted.
    let defaulted = dsql.list("").await;
    assert_eq!(
        defaulted.status, 200,
        "omitting max-results must use the modeled default, got {} {:?}",
        defaulted.status, defaulted.body
    );
    assert_eq!(
        defaulted
            .get("clusters")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        3,
        "all three clusters fit inside the default page size of 20"
    );
}

/// RULE: every operation keyed by a cluster identifier answers
/// `ResourceNotFoundException` (404) for an identifier this service does not hold.
#[tokio::test]
async fn an_unknown_identifier_is_not_found_on_every_cluster_operation() {
    let dsql = Dsql::new();
    // A cluster that does exist, so the 404 is about the identifier, not the store.
    dsql.create_settled().await;
    let absent = absent_identifier();

    let answers = vec![
        ("GetCluster", dsql.get(&absent).await),
        ("UpdateCluster", dsql.update(&absent, json!({})).await),
        ("DeleteCluster", dsql.delete(&absent).await),
    ];
    for (operation, answer) in answers {
        assert_eq!(
            answer.status, 404,
            "case {operation}: an unknown identifier must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {operation}: the 404 must be a ResourceNotFoundException, got {:?}",
            answer.error
        );
    }
}

/// RULE: an `identifier` outside `ClusterId`'s `^[a-z0-9]{26}$` is refused as invalid
/// input (400 `ValidationException`) rather than looked up.
#[tokio::test]
async fn a_malformed_identifier_is_refused_as_invalid() {
    let dsql = Dsql::new();
    let cases = [
        ("too short", "abc"),
        ("one char short", "aaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("one char long", "aaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("uppercase", "AAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ("hyphenated", "aaaaaaaaaaaa-aaaaaaaaaaaaa"),
        ("underscored", "aaaaaaaaaaaa_aaaaaaaaaaaaa"),
    ];

    for (name, identifier) in cases {
        for (operation, answer) in [
            ("GetCluster", dsql.get(identifier).await),
            ("UpdateCluster", dsql.update(identifier, json!({})).await),
            ("DeleteCluster", dsql.delete(identifier).await),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {name:?} on {operation}: a malformed identifier must be refused with 400, \
                 got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {name:?} on {operation}: the refusal must be a ValidationException, got {:?}",
                answer.error
            );
        }
    }
}

/// RULE: a retry of the same `@idempotent` `DeleteCluster` — same identifier, same
/// `client-token` — answers with the same result instead of a 404.
#[tokio::test]
async fn a_retried_delete_answers_from_the_retired_cluster() {
    let dsql = Dsql::new();
    let identifier = dsql.create_settled().await;

    let first = dsql
        .call(
            "DELETE",
            &format!("/cluster/{identifier}?client-token=retry-me"),
            None,
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first delete must succeed, got {} {:?}",
        first.status, first.body
    );
    wait_for_the_clock().await;

    let retry = dsql
        .call(
            "DELETE",
            &format!("/cluster/{identifier}?client-token=retry-me"),
            None,
        )
        .await;
    assert_eq!(
        retry.status, 200,
        "a retry with the same client-token must not answer 404, got {} {:?}",
        retry.status, retry.body
    );
    assert_eq!(
        retry.str("identifier"),
        identifier,
        "the retry must report the same cluster"
    );
    assert_eq!(
        retry.get("creationTime"),
        first.get("creationTime"),
        "the retry must report the same creationTime"
    );

    // A delete of the same identifier WITHOUT that token is a different request, and
    // the cluster is gone.
    let fresh = dsql.delete(&identifier).await;
    assert_eq!(
        fresh.status, 404,
        "a delete with no client-token must answer 404 once the cluster is gone, got {} {:?}",
        fresh.status, fresh.body
    );
}

/// RULE: every caller-driven store is bounded. `CreateCluster` past the ceiling is
/// refused with the modelled `ServiceQuotaExceededException` rather than growing the
/// emulator's heap, and a request already served still answers at the ceiling.
#[tokio::test]
async fn the_cluster_store_is_bounded_by_its_ceiling() {
    let dsql = Dsql::new();
    for n in 0..MAX_CLUSTERS {
        let created = dsql
            .create(json!({"deletionProtectionEnabled": false, "clientToken": format!("token-{n}")}))
            .await;
        assert_eq!(
            created.status, 200,
            "case create #{n}: a create below the ceiling must succeed, got {} {:?}",
            created.status, created.body
        );
    }

    let refused = dsql.create(json!({"deletionProtectionEnabled": false})).await;
    assert_eq!(
        refused.status, 402,
        "case create #{MAX_CLUSTERS}: a create past the ceiling must be refused, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case create #{MAX_CLUSTERS}: the refusal must be a ServiceQuotaExceededException, got {:?}",
        refused.error
    );

    let repeat = dsql
        .create(json!({"deletionProtectionEnabled": false, "clientToken": "token-0"}))
        .await;
    assert_eq!(
        repeat.status, 200,
        "case repeated clientToken at the ceiling: a request already served must still answer \
         from the store, got {} {:?}",
        repeat.status, repeat.body
    );
}

/// RULE: the two bookkeeping maps are bounded too — the idempotency tokens by
/// evicting the oldest, and the delete tombstones likewise — so neither grows
/// without bound however long a caller loops.
///
/// Driven against the state directly: reaching these caps over the wire would need
/// thousands of requests, each waiting out a settle span.
#[test]
fn the_bookkeeping_stores_stay_bounded() {
    let mut state = ComAmazonawsDsqlDsqlState::default();
    // The endpoint the records below are recorded at: a ledger entry is keyed by
    // the token together with the Region of the endpoint that served the call.
    let region = "us-east-1";

    for n in 0..(MAX_CREATE_TOKENS * 2) {
        state.tokens(TokenScope::CreateCluster).record(
            region,
            format!("token-{n}"),
            format!("fingerprint-{n}"),
            RecordedCall::new(&cluster_id_for(n)),
        );
        assert!(
            state.tokens_of(TokenScope::CreateCluster).len() <= MAX_CREATE_TOKENS,
            "case token #{n}: the idempotency ledger must stay at or below {MAX_CREATE_TOKENS}, got {}",
            state.tokens_of(TokenScope::CreateCluster).len()
        );
    }
    assert_eq!(
        state.tokens_of(TokenScope::CreateCluster).len(),
        MAX_CREATE_TOKENS,
        "the idempotency ledger must fill to its cap and then evict, got {}",
        state.tokens_of(TokenScope::CreateCluster).len()
    );

    // Each round retires one cluster through the clock, which is the only writer of
    // the tombstone map.
    let already_due = SystemTime::now() - Duration::from_secs(1);
    for n in 0..(MAX_RETIRED_CLUSTERS + 25) {
        let identifier = cluster_id_for(n);
        state
            .clusters
            .insert(identifier.clone(), retiring_cluster(&identifier, already_due));
        state.settle(SystemTime::now());
        assert!(
            state.clusters.is_empty(),
            "case retire #{n}: a settled deletion must leave the live store empty, got {}",
            state.clusters.len()
        );
        assert!(
            state.retired_clusters.len() <= MAX_RETIRED_CLUSTERS,
            "case retire #{n}: the tombstone map must stay at or below \
             {MAX_RETIRED_CLUSTERS}, got {}",
            state.retired_clusters.len()
        );
    }
    assert_eq!(
        state.retired_clusters.len(),
        MAX_RETIRED_CLUSTERS,
        "the tombstone map must fill to its cap and then evict, got {}",
        state.retired_clusters.len()
    );
}

/// A `ClusterId`-shaped identifier for case `n`: 26 characters from `[a-z0-9]`.
pub fn cluster_id_for(n: usize) -> String {
    format!("{n:026}")
}

/// A stored cluster whose deletion is already due, so one `settle` retires it.
pub fn retiring_cluster(identifier: &str, due_at: SystemTime) -> Cluster {
    Cluster {
        identifier: identifier.to_string(),
        vpc_endpoint_service_suffix: new_vpc_endpoint_service_suffix(),
        arn: expected_arn("us-east-1", identifier),
        status: "DELETING".to_string(),
        creation_time: due_at,
        deletion_protection_enabled: false,
        kms_encryption_key: None,
        encryption_details: None,
        multi_region_properties: None,
        tags: Default::default(),
        metric_groups: Default::default(),
        policy: None,
        streams: Default::default(),
        pending: Some(PendingTransition {
            due_at,
            next_status: STATUS_DELETED,
        }),
        // Seeded with its encryption configuration already in place, so the
        // clock owes it nothing on that member.
        encryption_pending: None,
    }
}

/// RULE: a cluster the clock still owes a transition does not accept a mutation.

#[tokio::test]
async fn a_cluster_still_creating_refuses_a_mutation() {
    let dsql = Dsql::new();
    let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
    let identifier = created.str("identifier");

    for (operation, answer) in [
        (
            "UpdateCluster",
            dsql.update(&identifier, json!({"deletionProtectionEnabled": true}))
                .await,
        ),
        ("DeleteCluster", dsql.delete(&identifier).await),
    ] {
        assert_eq!(
            answer.status, 409,
            "case {operation}: a CREATING cluster must refuse the mutation with 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ConflictException"),
            "case {operation}: the refusal must be a ConflictException, got {:?}",
            answer.error
        );
    }

    wait_for_the_clock().await;
    assert_eq!(
        dsql.update(&identifier, json!({"deletionProtectionEnabled": true}))
            .await
            .status,
        200,
        "once the cluster is ACTIVE the same update must succeed"
    );
}
