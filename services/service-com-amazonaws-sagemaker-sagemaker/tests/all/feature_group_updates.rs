// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker Feature Store's `UpdateFeatureGroup`, proved
//! through the reader the change has to show up in — `DescribeFeatureGroup`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is what
//! a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call:
//!
//! * an update applies the ONE change it states to the stored group and answers that
//!   group's `FeatureGroupArn`, for every member the operation models — features added,
//!   a TTL set and removed, a storage-type migration, a throughput switch in either
//!   direction and a capacity change — addressed by name and by ARN;
//! * a name this account holds no live group under — never created, on its way out, or
//!   gone — is refused with the modelled `ResourceNotFound` and the sentence SageMaker
//!   words it with, and changes nothing;
//! * an update that cannot hold together — none of the three update members, more than one
//!   of them, or a value no rule admits — is refused 400 `ValidationException` and leaves
//!   the group byte-for-byte as it was;
//! * additions accumulate in order up to the modelled maximum number of
//!   `FeatureDefinitions`, and one feature past it is refused with the modelled
//!   `ResourceLimitExceeded`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, FEATURE_GROUP_STATUS_CREATE_FAILED,
};

use crate::inference_deployment_lifecycle::{
    error_code, f64_field, message, str_field, Emulator, ACCOUNT,
};

/// Longer than `state::FEATURE_GROUP_CREATED_AFTER` (300 ms), so the clock owes a created
/// group its `Created` status by the time the next call runs.
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// Longer than `state::FEATURE_GROUP_RELEASED_AFTER` (300 ms), so the clock owes a
/// requested deletion its completion by the time the next call runs.
const PAST_RELEASE_DUE: Duration = Duration::from_millis(600);

/// A role ARN matching `RoleArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`), required of a group that
/// has an OfflineStore.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/FeatureStoreRole";

/// The largest number of `FeatureDefinition`s a group's schema may hold, from the
/// `FeatureDefinitions` list's own `@length 1..2500`
/// (`smithy_query(query="shape",shape="FeatureDefinitions")`).
const MAX_FEATURE_DEFINITIONS: usize = 2500;

/// How SageMaker words a feature group this account does not hold, as the handlers answer
/// it: the `Resource Not Found:` prefix, the resource type and the name the request
/// resolved to, ending with NO period. Spelled out here rather than imported from the
/// crate, so the wording itself is under test.
fn not_found_sentence(name: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name {name}")
}

/// A valid `CreateFeatureGroup` body for `name`, with `extra`'s members laid over it.
///
/// A member of `extra` whose value is `null` is REMOVED from the body, which is how a row
/// drops an otherwise-present member rather than sending a JSON null — the same shape
/// `feature_group_lifecycle` and `feature_group_deletion` build their bodies in.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "FeatureGroupName": name,
        "RecordIdentifierFeatureName": "customer-id",
        "EventTimeFeatureName": "event-time",
        "FeatureDefinitions": [
            { "FeatureName": "customer-id", "FeatureType": "String" },
            { "FeatureName": "event-time", "FeatureType": "String" },
        ],
        "OnlineStoreConfig": { "EnableOnlineStore": true },
    });
    let object = body.as_object_mut().expect("a JSON object body");
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    body
}

/// Create the group `name` with `extra` laid over the default body and let the clock
/// provision it, answering the `FeatureGroupArn` the create minted.
async fn created_group(emulator: &Emulator, name: &str, extra: Value) -> String {
    let created = emulator
        .ok("CreateFeatureGroup", &create_body(name, extra))
        .await;
    tokio::time::sleep(PAST_CREATED_DUE).await;
    str_field(&created, "FeatureGroupArn")
}

/// `UpdateFeatureGroup` with `members` plus the `FeatureGroupName` it addresses.
async fn update(emulator: &Emulator, name_or_arn: &str, members: &Value) -> (u16, Value) {
    let mut body = members.clone();
    body.as_object_mut()
        .expect("a JSON object of update members")
        .insert("FeatureGroupName".to_string(), json!(name_or_arn));
    emulator.call("UpdateFeatureGroup", &body).await
}

/// The whole `DescribeFeatureGroup` response for `name_or_arn`.
async fn described(emulator: &Emulator, name_or_arn: &str) -> Value {
    emulator
        .ok(
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": name_or_arn }),
        )
        .await
}

/// One row of the update round trip: the members the create lays over the default body,
/// the update's own members, and the `DescribeFeatureGroup` members the update must leave
/// behind.
struct UpdateCase {
    case: &'static str,
    create_extra: Value,
    update: Value,
    /// Each member here must equal the one the describe reports EXACTLY, so a row pins
    /// what the update did and what it left alone.
    expected: Value,
    /// Whether the update addresses the group by its ARN rather than by its bare name —
    /// `FeatureGroupNameOrArn` admits both spellings for the one group.
    by_arn: bool,
}

#[tokio::test]
async fn an_update_applies_the_change_it_states_and_describe_reads_it_back() {
    // The rows differ along what the rule generalises over: WHICH modelled member states
    // the update, how many features it adds and of which types and collections, whether a
    // TTL is set, restated or removed, the storage tier migrated to, the direction of a
    // throughput switch, and the spelling the group is addressed by.
    let cases = [
        UpdateCase {
            case: "one feature added",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "late_addition", "FeatureType": "String" }],
            }),
            expected: json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": "late_addition", "FeatureType": "String" },
                ],
            }),
            by_arn: false,
        },
        UpdateCase {
            case: "three features of every modelled FeatureType, added by ARN",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [
                    { "FeatureName": "balance", "FeatureType": "Fractional" },
                    { "FeatureName": "visits", "FeatureType": "Integral" },
                    { "FeatureName": "tier", "FeatureType": "String" },
                ],
            }),
            expected: json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": "balance", "FeatureType": "Fractional" },
                    { "FeatureName": "visits", "FeatureType": "Integral" },
                    { "FeatureName": "tier", "FeatureType": "String" },
                ],
            }),
            by_arn: true,
        },
        UpdateCase {
            case: "a collection-typed feature added",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{
                    "FeatureName": "embedding",
                    "FeatureType": "Fractional",
                    "CollectionType": "Vector",
                    "CollectionConfig": { "VectorConfig": { "Dimension": 4 } },
                }],
            }),
            expected: json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    {
                        "FeatureName": "embedding",
                        "FeatureType": "Fractional",
                        "CollectionType": "Vector",
                        "CollectionConfig": { "VectorConfig": { "Dimension": 4 } },
                    },
                ],
            }),
            by_arn: false,
        },
        UpdateCase {
            case: "a default TtlDuration set on a group that had none",
            create_extra: json!({}),
            update: json!({
                "OnlineStoreConfig": { "TtlDuration": { "Unit": "Days", "Value": 30 } },
            }),
            expected: json!({
                "OnlineStoreConfig": {
                    "EnableOnlineStore": true,
                    "TtlDuration": { "Unit": "Days", "Value": 30 },
                },
            }),
            by_arn: false,
        },
        UpdateCase {
            case: "a TtlDuration replaced by one of another unit",
            create_extra: json!({
                "OnlineStoreConfig": {
                    "EnableOnlineStore": true,
                    "TtlDuration": { "Unit": "Days", "Value": 7 },
                },
            }),
            update: json!({
                "OnlineStoreConfig": { "TtlDuration": { "Unit": "Weeks", "Value": 2 } },
            }),
            expected: json!({
                "OnlineStoreConfig": {
                    "EnableOnlineStore": true,
                    "TtlDuration": { "Unit": "Weeks", "Value": 2 },
                },
            }),
            by_arn: true,
        },
        UpdateCase {
            // "To remove the default TtlDuration from an existing feature group, use the
            // UpdateFeatureGroup API and set the TtlDuration Unit and Value to null."
            case: "the default TtlDuration removed",
            create_extra: json!({
                "OnlineStoreConfig": {
                    "EnableOnlineStore": true,
                    "TtlDuration": { "Unit": "Hours", "Value": 12 },
                },
            }),
            update: json!({ "OnlineStoreConfig": { "TtlDuration": {} } }),
            expected: json!({ "OnlineStoreConfig": { "EnableOnlineStore": true } }),
            by_arn: false,
        },
        UpdateCase {
            // "Use this parameter to migrate an existing feature group from Standard to
            // Standard_V2 storage format."
            case: "the online store migrated to the Standard_V2 tier",
            create_extra: json!({
                "OnlineStoreConfig": { "EnableOnlineStore": true, "StorageType": "Standard" },
            }),
            update: json!({ "OnlineStoreConfig": { "StorageType": "Standard_V2" } }),
            expected: json!({
                "OnlineStoreConfig": { "EnableOnlineStore": true, "StorageType": "Standard_V2" },
            }),
            by_arn: false,
        },
        UpdateCase {
            case: "an on-demand group switched to provisioned throughput",
            create_extra: json!({}),
            update: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 7,
                    "ProvisionedWriteCapacityUnits": 3,
                },
            }),
            expected: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 7,
                    "ProvisionedWriteCapacityUnits": 3,
                },
            }),
            by_arn: false,
        },
        UpdateCase {
            case: "the read capacity of a provisioned group raised, the write capacity kept",
            create_extra: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 10,
                    "ProvisionedWriteCapacityUnits": 5,
                },
            }),
            update: json!({ "ThroughputConfig": { "ProvisionedReadCapacityUnits": 20 } }),
            expected: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 20,
                    "ProvisionedWriteCapacityUnits": 5,
                },
            }),
            by_arn: true,
        },
        UpdateCase {
            case: "a provisioned group switched to on-demand, dropping its capacity",
            create_extra: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 10,
                    "ProvisionedWriteCapacityUnits": 5,
                },
            }),
            update: json!({ "ThroughputConfig": { "ThroughputMode": "OnDemand" } }),
            expected: json!({ "ThroughputConfig": { "ThroughputMode": "OnDemand" } }),
            by_arn: false,
        },
        UpdateCase {
            case: "an offline-only group gains a feature",
            create_extra: json!({
                "OnlineStoreConfig": null,
                "OfflineStoreConfig": { "S3StorageConfig": { "S3Uri": "s3://feature-store/offline/" } },
                "RoleArn": ROLE_ARN,
            }),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "channel", "FeatureType": "String" }],
            }),
            expected: json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": "channel", "FeatureType": "String" },
                ],
            }),
            by_arn: false,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let UpdateCase {
            case,
            create_extra,
            update: members,
            expected,
            by_arn,
        } = row;
        let emulator = Emulator::new();
        let name = format!("updated-{index}");
        let arn = created_group(&emulator, &name, create_extra.clone()).await;
        assert_eq!(
            arn,
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:feature-group/{name}"),
            "case {case:?}: the create must mint the group's ARN from its own name"
        );
        let before = described(&emulator, &name).await;

        let identifier = if *by_arn { arn.clone() } else { name.clone() };
        let (status, answered) = update(&emulator, &identifier, members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateFeatureGroup, got {status} with \
             {answered}"
        );
        // The output's one member is `@required`, and it is the group's own ARN however
        // the request addressed it.
        assert_eq!(
            str_field(&answered, "FeatureGroupArn"),
            arn,
            "case {case:?}: expected the group's own FeatureGroupArn, got {answered}"
        );

        let after = described(&emulator, &name).await;
        for (member, wanted) in expected.as_object().expect("a JSON object of members") {
            assert_eq!(
                after.get(member),
                Some(wanted),
                "case {case:?}: expected {member} to read back as {wanted}, got {after}"
            );
        }
        // The update changed what it stated and nothing else: identity and status stand,
        // and the modification instant moved to this call.
        assert_eq!(
            (
                str_field(&after, "FeatureGroupArn"),
                str_field(&after, "FeatureGroupName"),
                str_field(&after, "FeatureGroupStatus"),
            ),
            (arn.clone(), name.clone(), "Created".to_string()),
            "case {case:?}: expected the group to keep its identity and status, got {after}"
        );
        assert!(
            f64_field(&after, "LastModifiedTime") >= f64_field(&before, "LastModifiedTime")
                && f64_field(&after, "LastModifiedTime") >= f64_field(&after, "CreationTime"),
            "case {case:?}: expected LastModifiedTime to move to the update, got \
             {} after {}",
            f64_field(&after, "LastModifiedTime"),
            f64_field(&before, "LastModifiedTime")
        );
    }
}

#[tokio::test]
async fn a_name_this_account_holds_no_live_group_under_is_refused_as_resource_not_found() {
    let emulator = Emulator::new();
    let kept = created_group(&emulator, "kept-group", json!({})).await;
    created_group(&emulator, "deleting-group", json!({})).await;
    let gone_arn = created_group(&emulator, "gone-group", json!({})).await;
    emulator
        .ok(
            "DeleteFeatureGroup",
            &json!({ "FeatureGroupName": "gone-group" }),
        )
        .await;
    tokio::time::sleep(PAST_RELEASE_DUE).await;
    // Asked for but not finished: this one is still readable, and still on its way out.
    emulator
        .ok(
            "DeleteFeatureGroup",
            &json!({ "FeatureGroupName": "deleting-group" }),
        )
        .await;

    // Every row satisfies `FeatureGroupName`'s pattern and length, so the refusal comes
    // from the handler rather than from boundary validation. The expected name is the one
    // the request RESOLVES to, which for the ARN spelling is the bare name it carries.
    let cases = [
        (
            "a name never created",
            "nosuchgroup".to_string(),
            "nosuchgroup".to_string(),
        ),
        (
            "a prefix of a held name",
            "kept".to_string(),
            "kept".to_string(),
        ),
        (
            "a held name with a suffix",
            "kept-group-2".to_string(),
            "kept-group-2".to_string(),
        ),
        (
            "a held name in another case",
            "KEPT-GROUP".to_string(),
            "KEPT-GROUP".to_string(),
        ),
        (
            "a group already Deleting",
            "deleting-group".to_string(),
            "deleting-group".to_string(),
        ),
        (
            "a group whose removal completed",
            "gone-group".to_string(),
            "gone-group".to_string(),
        ),
        (
            "the ARN of a group whose removal completed",
            gone_arn.clone(),
            "gone-group".to_string(),
        ),
        (
            "the longest name the pattern admits",
            format!("z{}", "y".repeat(63)),
            format!("z{}", "y".repeat(63)),
        ),
    ];
    // The update members are varied too, so the refusal is a property of the NAME and not
    // of what the request asked to change. Every row states exactly one update member,
    // which is what the operation requires of any request.
    let members = [
        json!({ "FeatureAdditions": [{ "FeatureName": "late_addition", "FeatureType": "String" }] }),
        json!({ "OnlineStoreConfig": { "TtlDuration": { "Unit": "Days", "Value": 1 } } }),
        json!({ "ThroughputConfig": { "ThroughputMode": "OnDemand" } }),
    ];
    for (index, (case, identifier, resolved)) in cases.iter().enumerate() {
        let stated = &members[index % members.len()];
        let (status, body) = update(&emulator, identifier, stated).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?} with {stated}: expected 400 ResourceNotFound, got {status} with \
             {body}"
        );
        assert_eq!(
            message(&body),
            not_found_sentence(resolved),
            "case {case:?}: expected the not-found sentence naming the resolved name"
        );
    }

    // None of the refusals touched the group this account does hold, and none of them
    // created one under the name it addressed. The group that was on its way out is left
    // to finish going, so every row below names a group this account no longer holds.
    tokio::time::sleep(PAST_RELEASE_DUE).await;
    let held = described(&emulator, &kept).await;
    assert_eq!(
        (
            str_field(&held, "FeatureGroupName"),
            str_field(&held, "FeatureGroupStatus")
        ),
        ("kept-group".to_string(), "Created".to_string()),
        "expected the untouched group to survive every refusal, got {held}"
    );
    for (case, identifier, _) in &cases {
        if identifier == "kept-group" {
            continue;
        }
        let (status, body) = emulator
            .call(
                "DescribeFeatureGroup",
                &json!({ "FeatureGroupName": identifier }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: a refused update must not mint a group, got {status} with {body}"
        );
    }
}

/// One row of the refusal table: the members the create lays over the default body and the
/// update that may not be accepted.
struct RefusalCase {
    case: &'static str,
    create_extra: Value,
    update: Value,
}

#[tokio::test]
async fn an_update_that_cannot_hold_together_is_refused_and_changes_nothing() {
    // One row per rule the model or the operation's doc states, plus one value past each
    // enum catalogue the update carries (`FeatureType`, `CollectionType`,
    // `TtlDurationUnit`, `StorageType`, `ThroughputMode` — the generated `*_VALUES` lists
    // in `ops::types` are the catalogues themselves).
    let cases = [
        RefusalCase {
            // "Use one of the following request parameters at a time while using the
            // UpdateFeatureGroup API": a call that states none of them states no change,
            // and would otherwise answer 200 and move LastModifiedTime for nothing.
            case: "no update member at all",
            create_extra: json!({}),
            update: json!({}),
        },
        RefusalCase {
            // "Use one of the following request parameters at a time while using the
            // UpdateFeatureGroup API."
            case: "two of the three updates at once",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "late_addition", "FeatureType": "String" }],
                "ThroughputConfig": { "ThroughputMode": "OnDemand" },
            }),
        },
        RefusalCase {
            case: "all three updates at once",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "late_addition", "FeatureType": "String" }],
                "OnlineStoreConfig": { "TtlDuration": { "Unit": "Days", "Value": 1 } },
                "ThroughputConfig": { "ThroughputMode": "OnDemand" },
            }),
        },
        RefusalCase {
            case: "a feature the group already declares",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "customer-id", "FeatureType": "String" }],
            }),
        },
        RefusalCase {
            case: "one feature name twice in one request",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [
                    { "FeatureName": "twice", "FeatureType": "String" },
                    { "FeatureName": "twice", "FeatureType": "Integral" },
                ],
            }),
        },
        RefusalCase {
            // "FeatureNames cannot be any of the following: is_deleted, write_time,
            // api_invocation_time."
            case: "a feature name the FeatureStore reserves",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "write_time", "FeatureType": "String" }],
            }),
        },
        RefusalCase {
            case: "a FeatureType past the catalogue",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{ "FeatureName": "late_addition", "FeatureType": "Decimal" }],
            }),
        },
        RefusalCase {
            case: "a CollectionType past the catalogue",
            create_extra: json!({}),
            update: json!({
                "FeatureAdditions": [{
                    "FeatureName": "late_addition",
                    "FeatureType": "Fractional",
                    "CollectionType": "Bag",
                }],
            }),
        },
        RefusalCase {
            case: "a TtlDuration carrying only a Unit",
            create_extra: json!({}),
            update: json!({ "OnlineStoreConfig": { "TtlDuration": { "Unit": "Days" } } }),
        },
        RefusalCase {
            case: "a TtlDuration carrying only a Value",
            create_extra: json!({}),
            update: json!({ "OnlineStoreConfig": { "TtlDuration": { "Value": 5 } } }),
        },
        RefusalCase {
            case: "a TtlDuration Unit past the catalogue",
            create_extra: json!({}),
            update: json!({
                "OnlineStoreConfig": { "TtlDuration": { "Unit": "Fortnights", "Value": 2 } },
            }),
        },
        RefusalCase {
            case: "a StorageType past the catalogue",
            create_extra: json!({}),
            update: json!({ "OnlineStoreConfig": { "StorageType": "Standard_V3" } }),
        },
        RefusalCase {
            // "Migration is a one-way operation and cannot be reversed."
            case: "a Standard_V2 migration reversed",
            create_extra: json!({
                "OnlineStoreConfig": { "EnableOnlineStore": true, "StorageType": "Standard_V2" },
            }),
            update: json!({ "OnlineStoreConfig": { "StorageType": "Standard" } }),
        },
        RefusalCase {
            case: "an online store configuration for a group that keeps no OnlineStore",
            create_extra: json!({
                "OnlineStoreConfig": null,
                "OfflineStoreConfig": { "S3StorageConfig": { "S3Uri": "s3://feature-store/offline/" } },
                "RoleArn": ROLE_ARN,
            }),
            update: json!({
                "OnlineStoreConfig": { "TtlDuration": { "Unit": "Days", "Value": 1 } },
            }),
        },
        RefusalCase {
            case: "an online store configuration for a group whose OnlineStore is off",
            create_extra: json!({ "OnlineStoreConfig": { "EnableOnlineStore": false } }),
            update: json!({ "OnlineStoreConfig": { "StorageType": "Standard_V2" } }),
        },
        RefusalCase {
            // "This field is not applicable for on-demand feature groups."
            case: "a read capacity for an on-demand group",
            create_extra: json!({}),
            update: json!({ "ThroughputConfig": { "ProvisionedReadCapacityUnits": 5 } }),
        },
        RefusalCase {
            case: "a switch to on-demand that keeps a write capacity",
            create_extra: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 10,
                    "ProvisionedWriteCapacityUnits": 5,
                },
            }),
            update: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "OnDemand",
                    "ProvisionedWriteCapacityUnits": 5,
                },
            }),
        },
        RefusalCase {
            case: "a switch to provisioned stating no capacity at all",
            create_extra: json!({}),
            update: json!({ "ThroughputConfig": { "ThroughputMode": "Provisioned" } }),
        },
        RefusalCase {
            case: "a switch to provisioned stating only the read capacity",
            create_extra: json!({}),
            update: json!({
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 5,
                },
            }),
        },
        RefusalCase {
            case: "a ThroughputMode past the catalogue",
            create_extra: json!({}),
            update: json!({ "ThroughputConfig": { "ThroughputMode": "Reserved" } }),
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let RefusalCase {
            case,
            create_extra,
            update: members,
        } = row;
        let emulator = Emulator::new();
        let name = format!("refused-{index}");
        created_group(&emulator, &name, create_extra.clone()).await;
        let before = described(&emulator, &name).await;

        let (status, body) = update(&emulator, &name, members).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body}"
        );
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );

        // A refused update is not a partial one: every member of the group reads back as
        // it did before the call, its modification instant included.
        let after = described(&emulator, &name).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the group exactly as it was"
        );
    }
}

#[tokio::test]
async fn additions_accumulate_in_order_up_to_the_modelled_maximum_and_no_further() {
    let emulator = Emulator::new();
    let name = "accumulating";
    created_group(&emulator, name, json!({})).await;

    // `FeatureAdditions` is `@length 1..100`, so the schema grows a page at a time. Every
    // batch keeps its order, and so does the schema as a whole.
    let mut expected: Vec<String> = vec!["customer-id".to_string(), "event-time".to_string()];
    let batches = 24;
    let batch = 100;
    for round in 0..batches {
        let additions: Vec<Value> = (0..batch)
            .map(|index| {
                let feature = format!("extra{}", round * batch + index);
                expected.push(feature.clone());
                json!({ "FeatureName": feature, "FeatureType": "String" })
            })
            .collect();
        let (status, body) = update(
            &emulator,
            name,
            &json!({ "FeatureAdditions": additions }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case batch {round}: expected HTTP 200 from UpdateFeatureGroup, got {status} \
             with {body}"
        );
    }
    assert_eq!(
        feature_names(&described(&emulator, name).await),
        expected,
        "case {batches} batches: expected every addition kept, in the order it was added"
    );

    // One batch past the modelled maximum is refused with the modelled
    // `ResourceLimitExceeded`, and the schema it would have overflowed is untouched.
    let over = expected.len() + batch - MAX_FEATURE_DEFINITIONS;
    assert!(
        over > 0,
        "case the bound: the table must reach past {MAX_FEATURE_DEFINITIONS}, it reached \
         {}",
        expected.len() + batch
    );
    let additions: Vec<Value> = (0..batch)
        .map(|index| json!({ "FeatureName": format!("over{index}"), "FeatureType": "String" }))
        .collect();
    let (status, body) = update(&emulator, name, &json!({ "FeatureAdditions": additions })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceLimitExceeded"),
        "case {over} features past the maximum: expected 400 ResourceLimitExceeded, got \
         {status} with {body}"
    );
    assert_eq!(
        feature_names(&described(&emulator, name).await),
        expected,
        "case {over} features past the maximum: the refused batch must add nothing"
    );

    // And the batch that reaches the maximum EXACTLY is accepted.
    let remaining = MAX_FEATURE_DEFINITIONS - expected.len();
    let additions: Vec<Value> = (0..remaining)
        .map(|index| {
            let feature = format!("last{index}");
            expected.push(feature.clone());
            json!({ "FeatureName": feature, "FeatureType": "String" })
        })
        .collect();
    let (status, body) = update(&emulator, name, &json!({ "FeatureAdditions": additions })).await;
    assert_eq!(
        status, 200,
        "case the {remaining} features that reach the maximum: expected HTTP 200, got \
         {status} with {body}"
    );
    let reported = feature_names(&described(&emulator, name).await);
    assert_eq!(
        reported.len(),
        MAX_FEATURE_DEFINITIONS,
        "case the maximum reached: expected {MAX_FEATURE_DEFINITIONS} features, got {}",
        reported.len()
    );
    assert_eq!(
        reported, expected,
        "case the maximum reached: expected every feature kept, in the order it was added"
    );

    // One single feature past the maximum is refused too, so the bound is the schema's
    // size and not the batch's.
    let (status, body) = update(
        &emulator,
        name,
        &json!({ "FeatureAdditions": [{ "FeatureName": "onemore", "FeatureType": "String" }] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceLimitExceeded"),
        "case one feature past the maximum: expected 400 ResourceLimitExceeded, got \
         {status} with {body}"
    );
}

/// RULE: a feature group whose provisioning FAILED holds no provisioned schema or store, so
/// `DescribeFeatureGroup` reports the modelled `CreateFailed` status with the `FailureReason`
/// beside it and `UpdateFeatureGroup` is refused naming that status — the remedy is to delete
/// the group and create it again.
///
/// Nothing this emulator serves leaves a group in that status: the one failure it establishes
/// — an offline store the group's role may not write — refuses the create before anything is
/// stored (`state::offline_store_admission_refusal`, exercised end to end by
/// `feature_group_offline_store_admission`). So the status is reached here the way a test
/// reaches any clock- or provisioning-owed state it cannot drive from the wire: by seeding the
/// record the handlers read, which is what keeps the modelled state's own reporting and
/// refusal under test.
#[tokio::test]
async fn a_feature_group_whose_provisioning_failed_reports_it_and_takes_no_update() {
    let emulator = Emulator::new();
    let name = "failedgroup";
    created_group(&emulator, name, json!({})).await;

    // The reason such a group carries, worded as the model documents the member: "the reason
    // that the FeatureGroup failed to be replicated in the OfflineStore".
    let reason = "The FeatureGroup could not be created in the OfflineStore.";
    {
        let store = emulator
            .engine
            .state::<ComAmazonawsSagemakerSagemakerState>();
        let mut store = store.lock();
        let group = store
            .feature_groups
            .get_mut(name)
            .unwrap_or_else(|| panic!("the created group {name} must be stored"));
        group.status = FEATURE_GROUP_STATUS_CREATE_FAILED.to_string();
        group.failure_reason = Some(reason.to_string());
    }

    let described = described(&emulator, name).await;
    assert_eq!(
        (
            described.get("FeatureGroupStatus").and_then(Value::as_str),
            described.get("FailureReason").and_then(Value::as_str)
        ),
        (Some(FEATURE_GROUP_STATUS_CREATE_FAILED), Some(reason)),
        "case a failed group: expected the status and the reason beside it, got {described}"
    );

    let (status, refused) = update(
        &emulator,
        name,
        &json!({ "ThroughputConfig": { "ThroughputMode": "OnDemand" } }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case a failed group: expected the update refused, got {status} with {refused}"
    );
    let refusal = message(&refused);
    for expected in [name, &FEATURE_GROUP_STATUS_CREATE_FAILED.to_uppercase()] {
        assert!(
            refusal.contains(expected),
            "case a failed group: expected the refusal to name {expected:?}, got {refusal:?}"
        );
    }
}

/// The `FeatureName`s a `DescribeFeatureGroup` response reports, in order.
fn feature_names(described: &Value) -> Vec<String> {
    described
        .get("FeatureDefinitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a FeatureDefinitions list, got {described}"))
        .iter()
        .map(|definition| str_field(definition, "FeatureName"))
        .collect()
}
