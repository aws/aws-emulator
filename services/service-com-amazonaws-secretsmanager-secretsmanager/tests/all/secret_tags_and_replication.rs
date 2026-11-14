// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the metadata projection of a secret that carries tags or
//! participates in cross-region replication: `TagResource`, `UntagResource`,
//! `ReplicateSecretToRegions`, `RemoveRegionsFromReplication` and what
//! `DescribeSecret` / `ListSecrets` / `CreateSecret` report about them, driven
//! over the wire through the multi-service router.
//!
//! The rules under test:
//!
//! * a secret's metadata projection follows the secret's own state — every tag
//!   attached to it round-trips, and every replica Region it holds appears once
//!   in `ReplicationStatus` with a `StatusType` status;
//! * `PrimaryRegion` is part of the replication view, so it is reported exactly
//!   while the secret has at least one replica;
//! * a fresh replica is created `InProgress` and the clock moves it to `InSync`.
//!
//! Every test is a TABLE over the class its rule generalises: the number of
//! tags, the key and value charset and size, the number and names of replica
//! Regions, the form of the replica's KMS key, which Regions are removed, the
//! form of the `SecretId`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    DEFAULT_KMS_KEY_ALIAS, REPLICAS_PER_SECRET, REPLICATION_SUCCEEDED_MESSAGE,
    REPLICA_STATUS_IN_PROGRESS, REPLICA_STATUS_IN_SYNC, REPLICA_SYNC_DELAY, TAGS_PER_SECRET,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, which is the Region every secret it creates is
/// homed in — and so the one a replica may never be placed in.
const REGION: &str = "us-west-2";

/// A SigV4 header naming `secretsmanager` in `REGION`; the router routes on the
/// credential scope and the emulator verifies signatures only when an
/// authenticator is registered, which this harness does not do.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/secretsmanager/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request signed for `region`, and return its status and
/// JSON body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", authorization(region))
        .body(bytes::Bytes::from(
            // A write that stores a value creates a version the caller has to
            // name; see `secret_client_request_token::with_staked_version`.
            serde_json::to_vec(&crate::secret_client_request_token::with_staked_version(
                operation, body,
            ))
            .expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one `awsJson1_1` request signed for [`REGION`], and return its status
/// and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// Send one request that must fail, and return the wire error code.
async fn err(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> String {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// Create a secret with one value and no tags or replicas, and return its ARN.
async fn create(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        case,
    )
    .await;
    created
        .get("ARN")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no ARN in {created}"))
        .to_string()
}

/// The `{Key, Value}` pairs a response reports, in the order it reports them.
fn tags_of(parsed: &Value, case: &str) -> Vec<(String, String)> {
    parsed
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Tags in {parsed}"))
        .iter()
        .map(|tag| {
            (
                tag.get("Key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tag.get("Value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The `ReplicationStatus` entries a response reports, keyed by Region.
fn replicas_of(parsed: &Value, case: &str) -> Vec<Value> {
    parsed
        .get("ReplicationStatus")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no ReplicationStatus in {parsed}"))
        .clone()
}

/// The one `ReplicationStatus` entry for `region`.
fn replica_for(parsed: &Value, region: &str, case: &str) -> Value {
    let matching: Vec<Value> = replicas_of(parsed, case)
        .into_iter()
        .filter(|entry| entry.get("Region").and_then(Value::as_str) == Some(region))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one ReplicationStatus entry for {region}, got {parsed}"
    );
    matching.into_iter().next().expect("one entry")
}

/// A string member of a JSON object, or `None` when it is absent.
fn member(parsed: &Value, name: &str) -> Option<String> {
    parsed
        .get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// One row of the tag table: the tags to attach, then the keys to detach.
struct TagCase {
    label: &'static str,
    attach: Vec<(String, String)>,
    detach: Vec<String>,
}

impl TagCase {
    fn new(label: &'static str, attach: &[(&str, &str)], detach: &[&str]) -> Self {
        Self {
            label,
            attach: attach
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            detach: detach.iter().map(|key| (*key).to_string()).collect(),
        }
    }

    /// The `Tags` member of a `TagResource` request.
    fn wire_tags(&self) -> Value {
        Value::Array(
            self.attach
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        )
    }

    /// The tags left attached once `detach` has been applied.
    fn remaining(&self) -> Vec<(String, String)> {
        self.attach
            .iter()
            .filter(|(key, _)| !self.detach.contains(key))
            .cloned()
            .collect()
    }
}

/// The tag table: the rule is "whatever a caller attaches comes back", so the
/// rows differ along the number of tags, the charset and size of each half, and
/// how many of them are removed again. `TagKeyType` is modelled with a length of
/// 1..128 and `TagValueType` with 0..256, so the bounds of both are rows.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase::new("one tag", &[("team", "payments")], &[]),
        TagCase::new(
            "several tags, one removed",
            &[("env", "prod"), ("team", "core"), ("owner", "sre")],
            &["team"],
        ),
        TagCase::new(
            "every tag removed",
            &[("env", "prod"), ("team", "core")],
            &["env", "team"],
        ),
        TagCase::new(
            "a key that was never attached is ignored",
            &[("env", "prod")],
            &["absent"],
        ),
        TagCase::new("an empty value", &[("marker", "")], &[]),
        TagCase::new("non-ascii key and value", &[("éq", "✓ok")], &[]),
        TagCase {
            label: "at the modelled key and value maxima",
            attach: vec![("k".repeat(128), "v".repeat(256))],
            detach: vec![],
        },
        TagCase::new("one character either side", &[("k", "v")], &[]),
    ]
}

#[tokio::test]
async fn tags_attached_to_a_secret_round_trip_through_describe_and_list() {
    let router = emulator();
    for (index, tag_case) in tag_cases().iter().enumerate() {
        let case = tag_case.label;
        let name = format!("tagged/{index}");
        let arn = create(&router, &name, case).await;

        // Before anything is attached, Secrets Manager "only returns fields that
        // have a value", so there is no Tags member at all.
        let bare = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        assert_eq!(
            bare.get("Tags"),
            None,
            "case {case:?}: an untagged secret must report no Tags, got {bare}"
        );

        ok(
            &router,
            "TagResource",
            json!({"SecretId": &name, "Tags": tag_case.wire_tags()}),
            case,
        )
        .await;

        // Read the tags back by ARN, in a separate request.
        let described = ok(&router, "DescribeSecret", json!({"SecretId": &arn}), case).await;
        assert_eq!(
            tags_of(&described, case),
            tag_case.attach,
            "case {case:?}: expected every attached tag back from DescribeSecret, got {described}"
        );

        // And through the listing, which projects the same tags.
        let listed = ok(
            &router,
            "ListSecrets",
            json!({"Filters": [{"Key": "name", "Values": [&name]}]}),
            case,
        )
        .await;
        let entry = listed
            .get("SecretList")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .unwrap_or_else(|| panic!("case {case:?}: the secret is missing from {listed}"))
            .clone();
        assert_eq!(
            tags_of(&entry, case),
            tag_case.attach,
            "case {case:?}: expected the same tags from ListSecrets, got {entry}"
        );

        // Re-attaching a key replaces its value rather than attaching it twice:
        // "This operation appends tags to the existing list of tags", and a key
        // identifies one tag.
        let (first_key, _) = tag_case.attach.first().expect("every row attaches a tag");
        ok(
            &router,
            "TagResource",
            json!({"SecretId": &name, "Tags": [{"Key": first_key, "Value": "replaced"}]}),
            case,
        )
        .await;
        let replaced = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        let expected_after_replace: Vec<(String, String)> = tag_case
            .attach
            .iter()
            .map(|(key, value)| {
                if key == first_key {
                    (key.clone(), "replaced".to_string())
                } else {
                    (key.clone(), value.clone())
                }
            })
            .collect();
        assert_eq!(
            tags_of(&replaced, case),
            expected_after_replace,
            "case {case:?}: re-tagging a key must replace its value, got {replaced}"
        );

        // Detaching removes both halves of each named key, and a key that is not
        // attached is not an error.
        ok(
            &router,
            "UntagResource",
            json!({"SecretId": &name, "TagKeys": tag_case.detach}),
            case,
        )
        .await;
        let after = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        let expected: Vec<(String, String)> = expected_after_replace
            .into_iter()
            .filter(|(key, _)| !tag_case.detach.contains(key))
            .collect();
        if expected.is_empty() {
            assert_eq!(
                after.get("Tags"),
                None,
                "case {case:?}: a secret left with no tags must report no Tags, got {after}"
            );
        } else {
            assert_eq!(
                tags_of(&after, case),
                expected,
                "case {case:?}: expected only the tags that were not detached, got {after}"
            );
        }
        assert_eq!(
            tag_case.remaining().len(),
            expected.len(),
            "case {case:?}: the row's own expectation disagrees with the response {after}"
        );
    }
}

#[tokio::test]
async fn tags_are_matched_by_the_tag_key_and_tag_value_filters() {
    let router = emulator();
    // The filter keys come from `Filter$Key` in this service's model.
    let rows: &[(&str, &str, &str)] = &[
        ("tag-key matches a whole key", "tag-key", "team"),
        ("tag-key matches a prefix", "tag-key", "te"),
        ("tag-value matches a whole value", "tag-value", "payments"),
        ("tag-value matches a prefix", "tag-value", "pay"),
        ("all searches tag halves too", "all", "payments"),
    ];
    let name = "filtered";
    create(&router, name, "filter setup").await;
    ok(
        &router,
        "TagResource",
        json!({"SecretId": name, "Tags": [{"Key": "team", "Value": "payments"}]}),
        "filter setup",
    )
    .await;

    for (case, key, value) in rows {
        let listed = ok(
            &router,
            "ListSecrets",
            json!({"Filters": [{"Key": key, "Values": [value]}]}),
            case,
        )
        .await;
        let names: Vec<String> = listed
            .get("SecretList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {listed}"))
            .iter()
            .filter_map(|entry| member(entry, "Name"))
            .collect();
        assert_eq!(
            names,
            vec![name.to_string()],
            "case {case:?}: expected the tagged secret to match, got {listed}"
        );
    }

    // A value no tag carries matches nothing.
    let listed = ok(
        &router,
        "ListSecrets",
        json!({"Filters": [{"Key": "tag-key", "Values": ["absent"]}]}),
        "a tag key nothing carries",
    )
    .await;
    assert_eq!(
        listed.get("SecretList").and_then(Value::as_array),
        Some(&Vec::<Value>::new()),
        "case \"a tag key nothing carries\": expected no match, got {listed}"
    );
}

/// The statuses a replica created at `started` may legitimately be reported in
/// by a read taken NOW: `InProgress` until the creation span has elapsed, and
/// `InSync` only once it has. A read the span cannot yet have elapsed for pins
/// `InProgress` exactly, so this never trades the transient status away to a
/// loaded machine.
fn expected_statuses(started: Instant) -> Vec<String> {
    if started.elapsed() < REPLICA_SYNC_DELAY {
        vec![REPLICA_STATUS_IN_PROGRESS.to_string()]
    } else {
        vec![
            REPLICA_STATUS_IN_PROGRESS.to_string(),
            REPLICA_STATUS_IN_SYNC.to_string(),
        ]
    }
}

/// One row of the replication table: the replica Regions to add, with the KMS
/// key each one names (`None` for a replica that names none).
struct ReplicaCase {
    label: &'static str,
    regions: Vec<(&'static str, Option<&'static str>)>,
    remove: Vec<&'static str>,
}

/// The replication table. Region codes are `RegionType` values (pattern
/// `^([a-z]+-)+\d+$`), and the KMS key column spans the documented forms of
/// `ReplicaRegionType$KmsKeyId` — an alias, a key id, a key ARN, and absent,
/// which means `aws/secretsmanager`.
fn replica_cases() -> Vec<ReplicaCase> {
    vec![
        ReplicaCase {
            label: "one replica, no KMS key named",
            regions: vec![("us-east-1", None)],
            remove: vec!["us-east-1"],
        },
        ReplicaCase {
            label: "one replica with an alias",
            regions: vec![("eu-west-3", Some("alias/my-key"))],
            remove: vec!["ap-south-1"],
        },
        ReplicaCase {
            label: "three replicas, mixed key forms",
            regions: vec![
                ("eu-central-1", Some("1234abcd-12ab-34cd-56ef-1234567890ab")),
                (
                    "ap-southeast-2",
                    Some("arn:aws:kms:ap-southeast-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab"),
                ),
                ("sa-east-1", None),
            ],
            remove: vec!["sa-east-1", "eu-central-1"],
        },
        ReplicaCase {
            label: "a region code with three hyphenated words",
            regions: vec![("us-gov-west-1", None)],
            remove: vec!["us-gov-west-1"],
        },
    ]
}

#[tokio::test]
async fn every_replica_region_is_reported_once_with_its_key_and_status() {
    let router = emulator();
    for (index, replica_case) in replica_cases().iter().enumerate() {
        let case = replica_case.label;
        let name = format!("replicated/{index}");
        let arn = create(&router, &name, case).await;

        let replicated = ok(
            &router,
            "ReplicateSecretToRegions",
            json!({
                "SecretId": &name,
                "AddReplicaRegions": replica_case
                    .regions
                    .iter()
                    .map(|(region, key)| match key {
                        Some(key) => json!({"Region": region, "KmsKeyId": key}),
                        None => json!({"Region": region}),
                    })
                    .collect::<Vec<Value>>(),
            }),
            case,
        )
        .await;
        let started = Instant::now();
        assert_eq!(
            member(&replicated, "ARN"),
            Some(arn.clone()),
            "case {case:?}: expected the primary secret's ARN back, got {replicated}"
        );

        // The view the starting call reports, and the view a later
        // DescribeSecret reports, are the same: one entry per replica Region.
        // The starting call's own answer pins the transient status exactly; a
        // later read may already have been overtaken by the clock.
        let described = ok(&router, "DescribeSecret", json!({"SecretId": &arn}), case).await;
        for (view, allowed) in [
            (
                replicated.clone(),
                vec![REPLICA_STATUS_IN_PROGRESS.to_string()],
            ),
            (described.clone(), expected_statuses(started)),
        ] {
            assert_eq!(
                replicas_of(&view, case).len(),
                replica_case.regions.len(),
                "case {case:?}: expected one entry per replica Region, got {view}"
            );
            for (region, key) in &replica_case.regions {
                let entry = replica_for(&view, region, case);
                let status = member(&entry, "Status").unwrap_or_default();
                assert!(
                    allowed.contains(&status),
                    "case {case:?}: wrong Status for the replica in {region}: expected one of \
                     {allowed:?}, got {entry}"
                );
                assert_eq!(
                    member(&entry, "KmsKeyId"),
                    Some(key.unwrap_or(DEFAULT_KMS_KEY_ALIAS).to_string()),
                    "case {case:?}: wrong KmsKeyId for the replica in {region}, got {entry}"
                );
                assert_eq!(
                    entry.get("LastAccessedDate"),
                    None,
                    "case {case:?}: the replica in {region} has never been retrieved there, got \
                     {entry}"
                );
            }
        }
        // A replicated secret IS a replication primary, so DescribeSecret
        // reports the Region it is in.
        assert_eq!(
            member(&described, "PrimaryRegion").as_deref(),
            Some(REGION),
            "case {case:?}: a replicated secret must report its PrimaryRegion, got {described}"
        );
    }
}

#[tokio::test]
async fn a_replica_reaches_in_sync_once_its_creation_span_has_elapsed() {
    let router = emulator();
    let rows: &[(&str, &[&str])] = &[
        ("one replica", &["us-east-1"]),
        ("two replicas", &["us-east-1", "eu-west-1"]),
    ];
    for (index, (case, regions)) in rows.iter().enumerate() {
        let name = format!("settling/{index}");
        create(&router, &name, case).await;
        ok(
            &router,
            "ReplicateSecretToRegions",
            json!({
                "SecretId": &name,
                "AddReplicaRegions": regions
                    .iter()
                    .map(|region| json!({"Region": region}))
                    .collect::<Vec<Value>>(),
            }),
            case,
        )
        .await;
        let started = Instant::now();

        // A caller reading straight after the call still sees the transient
        // status: the span is never zero. Asserted only while the span provably
        // cannot have elapsed, so a loaded machine is not read as a defect.
        let immediately = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        if started.elapsed() < REPLICA_SYNC_DELAY {
            for region in *regions {
                assert_eq!(
                    member(&replica_for(&immediately, region, case), "Status"),
                    Some(REPLICA_STATUS_IN_PROGRESS.to_string()),
                    "case {case:?}: expected {region} to still be InProgress, got {immediately}"
                );
            }
        }

        // Once the span the clock owes has elapsed, the same read reports the
        // replica as created — without any call having moved it.
        tokio::time::sleep(REPLICA_SYNC_DELAY + Duration::from_millis(200)).await;
        let settled = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        for region in *regions {
            let entry = replica_for(&settled, region, case);
            assert_eq!(
                member(&entry, "Status"),
                Some(REPLICA_STATUS_IN_SYNC.to_string()),
                "case {case:?}: expected {region} to have reached InSync, got {settled}"
            );
            assert_eq!(
                member(&entry, "StatusMessage"),
                Some(REPLICATION_SUCCEEDED_MESSAGE.to_string()),
                "case {case:?}: a created replica reports that replication succeeded, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn removing_replica_regions_leaves_only_the_replicas_that_remain() {
    let router = emulator();
    for (index, replica_case) in replica_cases().iter().enumerate() {
        let case = replica_case.label;
        let name = format!("unreplicated/{index}");
        let arn = create(&router, &name, case).await;
        ok(
            &router,
            "ReplicateSecretToRegions",
            json!({
                "SecretId": &arn,
                "AddReplicaRegions": replica_case
                    .regions
                    .iter()
                    .map(|(region, _)| json!({"Region": region}))
                    .collect::<Vec<Value>>(),
            }),
            case,
        )
        .await;

        // Removing a Region that holds no replica of this secret changes
        // nothing: the operation names the replicas to delete.
        let untouched = ok(
            &router,
            "RemoveRegionsFromReplication",
            json!({"SecretId": &name, "RemoveReplicaRegions": ["me-south-1"]}),
            case,
        )
        .await;
        assert_eq!(
            replicas_of(&untouched, case).len(),
            replica_case.regions.len(),
            "case {case:?}: removing a Region with no replica must change nothing, got {untouched}"
        );

        let removed = ok(
            &router,
            "RemoveRegionsFromReplication",
            json!({"SecretId": &name, "RemoveReplicaRegions": replica_case.remove}),
            case,
        )
        .await;
        assert_eq!(
            member(&removed, "ARN"),
            Some(arn.clone()),
            "case {case:?}: expected the primary secret's ARN back, got {removed}"
        );

        let left: Vec<&str> = replica_case
            .regions
            .iter()
            .map(|(region, _)| *region)
            .filter(|region| !replica_case.remove.contains(region))
            .collect();
        let described = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        let expected: Vec<String> = left.iter().map(ToString::to_string).collect();
        // The removal itself reports "the status of replicas for this secret
        // after you remove Regions", so with none left it reports an EMPTY list;
        // the metadata projection omits the member instead, because Secrets
        // Manager "only returns fields that have a value".
        assert_eq!(
            replicas_of(&removed, case)
                .iter()
                .filter_map(|entry| member(entry, "Region"))
                .collect::<Vec<String>>(),
            expected,
            "case {case:?}: expected only the replicas that were not removed, got {removed}"
        );
        if expected.is_empty() {
            assert_eq!(
                described.get("ReplicationStatus"),
                None,
                "case {case:?}: expected no ReplicationStatus once every replica is gone, got \
                 {described}"
            );
        } else {
            assert_eq!(
                replicas_of(&described, case)
                    .iter()
                    .filter_map(|entry| member(entry, "Region"))
                    .collect::<Vec<String>>(),
                expected,
                "case {case:?}: expected the same replicas from DescribeSecret, got {described}"
            );
        }
        // `PrimaryRegion` lives on the metadata projection, and it follows the
        // same replication state: it is there exactly while a replica is.
        assert_eq!(
            member(&described, "PrimaryRegion").as_deref(),
            (!left.is_empty()).then_some(REGION),
            "case {case:?}: PrimaryRegion must be reported exactly while the secret has a \
             replica, got {described}"
        );
    }
}

#[tokio::test]
async fn replicating_again_into_a_region_restarts_that_one_replica() {
    let router = emulator();
    // The rule generalises over whether the caller asks for an overwrite: a
    // Region this secret already replicates to is replicated again either way,
    // because the conflict `ForceOverwriteReplicaSecret` resolves is a different
    // secret of the same name in the destination Region.
    let rows: &[(&str, Option<bool>)] = &[
        ("without ForceOverwriteReplicaSecret", None),
        ("with ForceOverwriteReplicaSecret false", Some(false)),
        ("with ForceOverwriteReplicaSecret true", Some(true)),
    ];
    for (index, (case, force)) in rows.iter().enumerate() {
        let name = format!("re-replicated/{index}");
        create(&router, &name, case).await;
        let region = "us-east-1";
        ok(
            &router,
            "ReplicateSecretToRegions",
            json!({"SecretId": &name, "AddReplicaRegions": [{"Region": region}]}),
            case,
        )
        .await;

        let mut request = json!({
            "SecretId": &name,
            "AddReplicaRegions": [{"Region": region, "KmsKeyId": "alias/second-key"}],
        });
        if let Some(force) = force {
            request["ForceOverwriteReplicaSecret"] = json!(force);
        }
        let again = ok(&router, "ReplicateSecretToRegions", request, case).await;
        let started = Instant::now();
        // One entry, not two: the Region is the replica's identity.
        assert_eq!(
            replicas_of(&again, case).len(),
            1,
            "case {case:?}: a Region must not be replicated to twice over, got {again}"
        );
        let entry = replica_for(&again, region, case);
        assert_eq!(
            member(&entry, "Status"),
            Some(REPLICA_STATUS_IN_PROGRESS.to_string()),
            "case {case:?}: expected the replica to be created again, got {entry}"
        );
        assert_eq!(
            member(&entry, "KmsKeyId").as_deref(),
            Some("alias/second-key"),
            "case {case:?}: expected the KMS key of the latest request, got {entry}"
        );

        // And that is what a later read reports.
        let described = ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await;
        let entry = replica_for(&described, region, case);
        assert!(
            expected_statuses(started).contains(&member(&entry, "Status").unwrap_or_default()),
            "case {case:?}: wrong Status after re-replicating: expected one of {:?}, got {entry}",
            expected_statuses(started)
        );
        // The `StatusMessage` is the one the status it accompanies carries: a
        // replica that has reached `InSync` reports that replication succeeded,
        // and one still being created has nothing to report yet.
        let expected_message = (member(&entry, "Status").as_deref()
            == Some(REPLICA_STATUS_IN_SYNC))
        .then(|| REPLICATION_SUCCEEDED_MESSAGE.to_string());
        assert_eq!(
            member(&entry, "StatusMessage"),
            expected_message,
            "case {case:?}: wrong StatusMessage for the status reported, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_secret_created_with_replica_regions_reports_them_from_the_start() {
    let router = emulator();
    let rows: &[(&str, &[&str])] = &[
        ("one replica at create time", &["us-east-1"]),
        ("two replicas at create time", &["us-east-1", "eu-west-2"]),
    ];
    for (index, (case, regions)) in rows.iter().enumerate() {
        let name = format!("created-replicated/{index}");
        let created = ok(
            &router,
            "CreateSecret",
            json!({
                "Name": &name,
                "SecretString": "s3cret",
                "AddReplicaRegions": regions
                    .iter()
                    .map(|region| json!({"Region": region}))
                    .collect::<Vec<Value>>(),
            }),
            case,
        )
        .await;
        for view in [
            created,
            ok(&router, "DescribeSecret", json!({"SecretId": &name}), case).await,
        ] {
            let reported: Vec<String> = replicas_of(&view, case)
                .iter()
                .filter_map(|entry| member(entry, "Region"))
                .collect();
            assert_eq!(
                reported,
                regions.iter().map(ToString::to_string).collect::<Vec<String>>(),
                "case {case:?}: expected every requested replica Region, got {view}"
            );
        }
    }
}

#[tokio::test]
async fn a_bad_tag_or_replication_request_is_refused_with_the_modelled_error() {
    let router = emulator();
    let name = "refusals";
    create(&router, name, "refusal setup").await;

    // A secret no SecretId resolves to.
    let missing: &[(&str, Value)] = &[
        (
            "TagResource",
            json!({"SecretId": "absent", "Tags": [{"Key": "k", "Value": "v"}]}),
        ),
        (
            "UntagResource",
            json!({"SecretId": "absent", "TagKeys": ["k"]}),
        ),
        (
            "ReplicateSecretToRegions",
            json!({"SecretId": "absent", "AddReplicaRegions": [{"Region": "us-east-1"}]}),
        ),
        (
            "RemoveRegionsFromReplication",
            json!({"SecretId": "absent", "RemoveReplicaRegions": ["us-east-1"]}),
        ),
    ];
    for (operation, body) in missing {
        let case = format!("{operation} on a secret that does not exist");
        let code = err(&router, operation, body.clone(), &case).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code}"
        );
    }

    // A tag whose key is missing, or carries the prefix AWS reserves.
    let rows: &[(&str, &str, Value)] = &[
        (
            "a tag with no Key",
            "TagResource",
            json!({"SecretId": "refusals", "Tags": [{"Value": "v"}]}),
        ),
        (
            "a tag key under the reserved prefix",
            "TagResource",
            json!({"SecretId": "refusals", "Tags": [{"Key": "aws:owner", "Value": "v"}]}),
        ),
        (
            "removing a tag key under the reserved prefix",
            "UntagResource",
            json!({"SecretId": "refusals", "TagKeys": ["aws:owner"]}),
        ),
        (
            "a replica that names no Region",
            "ReplicateSecretToRegions",
            json!({"SecretId": "refusals", "AddReplicaRegions": [{"KmsKeyId": "alias/k"}]}),
        ),
        (
            "a replica in the Region the secret is already in",
            "ReplicateSecretToRegions",
            json!({"SecretId": "refusals", "AddReplicaRegions": [{"Region": REGION}]}),
        ),
        (
            "creating a secret with a replica in its own Region",
            "CreateSecret",
            json!({"Name": "self-replicated", "AddReplicaRegions": [{"Region": REGION}]}),
        ),
        (
            "more replica Regions than a secret may hold",
            "ReplicateSecretToRegions",
            json!({
                "SecretId": "refusals",
                "AddReplicaRegions": (0..=REPLICAS_PER_SECRET)
                    .map(|index| json!({"Region": format!("emu-region-{index}")}))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "removing more replica Regions than a secret may hold",
            "RemoveRegionsFromReplication",
            json!({
                "SecretId": "refusals",
                "RemoveReplicaRegions": (0..=REPLICAS_PER_SECRET)
                    .map(|index| json!(format!("emu-region-{index}")))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "creating a secret with more replica Regions than it may hold",
            "CreateSecret",
            json!({
                "Name": "over-replicated",
                "AddReplicaRegions": (0..=REPLICAS_PER_SECRET)
                    .map(|index| json!({"Region": format!("emu-region-{index}")}))
                    .collect::<Vec<Value>>(),
            }),
        ),
    ];
    for (case, operation, body) in rows {
        let code = err(&router, operation, body.clone(), case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException from {operation}, got {code}"
        );
    }

    // A write that would leave the secret carrying more tags than the tagging quota
    // allows is refused for the state of the resource, not for one bad parameter:
    // "A parameter value is not valid for the current state of the resource."
    // (`InvalidRequestException`, this service's Smithy model.)
    let over_quota: &[(&str, &str, Value)] = &[
        (
            "more tags than a secret may carry",
            "TagResource",
            json!({
                "SecretId": "refusals",
                "Tags": (0..=TAGS_PER_SECRET)
                    .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "more tag keys than a secret can carry",
            "UntagResource",
            json!({
                "SecretId": "refusals",
                "TagKeys": (0..=TAGS_PER_SECRET)
                    .map(|index| json!(format!("k{index}")))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "creating a secret with more tags than it may carry",
            "CreateSecret",
            json!({
                "Name": "over-tagged",
                "Tags": (0..=TAGS_PER_SECRET)
                    .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
                    .collect::<Vec<Value>>(),
            }),
        ),
    ];
    for (case, operation, body) in over_quota {
        let code = err(&router, operation, body.clone(), case).await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException from {operation}, got {code}"
        );
    }

    // The refusals left nothing attached and nothing replicated.
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "after the refusals",
    )
    .await;
    assert_eq!(
        (
            described.get("Tags"),
            described.get("ReplicationStatus"),
            described.get("PrimaryRegion")
        ),
        (None, None, None),
        "case \"after the refusals\": a refused request must change nothing, got {described}"
    );

    // A secret scheduled for deletion is not in a state these operations accept.
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": name}),
        "scheduling deletion",
    )
    .await;
    let rows: &[(&str, &str, Value)] = &[
        (
            "tagging a secret marked for deletion",
            "TagResource",
            json!({"SecretId": "refusals", "Tags": [{"Key": "k", "Value": "v"}]}),
        ),
        (
            "untagging a secret marked for deletion",
            "UntagResource",
            json!({"SecretId": "refusals", "TagKeys": ["k"]}),
        ),
        (
            "replicating a secret marked for deletion",
            "ReplicateSecretToRegions",
            json!({"SecretId": "refusals", "AddReplicaRegions": [{"Region": "us-east-1"}]}),
        ),
        (
            "unreplicating a secret marked for deletion",
            "RemoveRegionsFromReplication",
            json!({"SecretId": "refusals", "RemoveReplicaRegions": ["us-east-1"]}),
        ),
    ];
    for (case, operation, body) in rows {
        let code = err(&router, operation, body.clone(), case).await;
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException from {operation}, got {code}"
        );
    }
}

#[tokio::test]
async fn a_secret_holds_no_more_replicas_than_the_bound_allows() {
    let router = emulator();
    let name = "at-the-replica-bound";
    create(&router, name, "replica bound setup").await;

    // Exactly the bound is accepted, in one request.
    let filled = ok(
        &router,
        "ReplicateSecretToRegions",
        json!({
            "SecretId": name,
            "AddReplicaRegions": (0..REPLICAS_PER_SECRET)
                .map(|index| json!({"Region": format!("emu-region-{index}")}))
                .collect::<Vec<Value>>(),
        }),
        "exactly the bound",
    )
    .await;
    assert_eq!(
        replicas_of(&filled, "exactly the bound").len(),
        REPLICAS_PER_SECRET,
        "case \"exactly the bound\": expected every requested Region, got {filled}"
    );

    // A Region already replicated to is replaced rather than added, so
    // re-replicating one at the bound is still accepted.
    ok(
        &router,
        "ReplicateSecretToRegions",
        json!({"SecretId": name, "AddReplicaRegions": [{"Region": "emu-region-0"}]}),
        "re-replicating at the bound",
    )
    .await;

    // One Region past it is refused, and the secret keeps the replicas it had.
    let code = err(
        &router,
        "ReplicateSecretToRegions",
        json!({"SecretId": name, "AddReplicaRegions": [{"Region": "emu-region-past-1"}]}),
        "one past the bound",
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case \"one past the bound\": expected InvalidParameterException, got {code}"
    );
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "after the refusal at the bound",
    )
    .await;
    assert_eq!(
        replicas_of(&described, "after the refusal at the bound").len(),
        REPLICAS_PER_SECRET,
        "case \"after the refusal at the bound\": the refused request must store nothing, got \
         {described}"
    );
}

#[tokio::test]
async fn stop_replication_to_replica_answers_for_the_region_it_is_called_in() {
    let router = emulator();
    let name = "promotable";
    create(&router, name, "promotion setup").await;
    let replica_region = "us-east-1";
    ok(
        &router,
        "ReplicateSecretToRegions",
        json!({"SecretId": name, "AddReplicaRegions": [{"Region": replica_region}]}),
        "promotion setup",
    )
    .await;

    // The table is over the Region the call is made in, which is what this
    // operation is scoped by: "You must call this operation from the Region in
    // which you want to promote the replica to a primary secret." Every Region
    // but the replica's own refuses it.
    let rows: &[(&str, &str, u16, &str)] = &[
        (
            "called in the primary's own Region",
            REGION,
            400,
            "InvalidParameterException",
        ),
        (
            "called in a Region holding neither the secret nor a replica",
            "eu-west-1",
            400,
            "ResourceNotFoundException",
        ),
        (
            "called in a Region holding neither, with a different code shape",
            "ap-northeast-1",
            400,
            "ResourceNotFoundException",
        ),
    ];
    for (case, region, status, code) in rows {
        let (actual_status, parsed) =
            call_in(&router, region, "StopReplicationToReplica", &json!({"SecretId": name})).await;
        assert_eq!(
            actual_status, *status,
            "case {case:?}: expected HTTP {status}, got {actual_status} with body {parsed}"
        );
        let raw = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no __type in {parsed}"));
        let actual_code = raw.split('#').next_back().unwrap_or(raw);
        assert_eq!(
            actual_code, *code,
            "case {case:?}: expected {code}, got {actual_code} in {parsed}"
        );
    }

    // And none of it changed the secret's replication view.
    let described = ok(
        &router,
        "DescribeSecret",
        json!({"SecretId": name}),
        "after the refused promotions",
    )
    .await;
    assert_eq!(
        replicas_of(&described, "after the refused promotions")
            .iter()
            .filter_map(|entry| member(entry, "Region"))
            .collect::<Vec<String>>(),
        vec![replica_region.to_string()],
        "case \"after the refused promotions\": the replica must still be there, got {described}"
    );

    // Called in the replica's own Region it promotes: the replica answers with
    // its own ARN and the primary stops replicating to it.
    let case = "called in the replica's Region";
    let (status, promoted) =
        call_in(&router, replica_region, "StopReplicationToReplica", &json!({"SecretId": name}))
            .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the promotion to succeed, got {status} with body {promoted}"
    );
    let promoted_arn = member(&promoted, "ARN")
        .unwrap_or_else(|| panic!("case {case:?}: no ARN in {promoted}"));
    assert!(
        promoted_arn.contains(&format!(":{replica_region}:")),
        "case {case:?}: expected the promoted secret's own ARN, got {promoted_arn:?}"
    );
    let after = ok(&router, "DescribeSecret", json!({"SecretId": name}), case).await;
    assert!(
        after.get("ReplicationStatus").is_none() && after.get("PrimaryRegion").is_none(),
        "case {case:?}: the primary no longer replicates to the promoted Region, got {after}"
    );
}
