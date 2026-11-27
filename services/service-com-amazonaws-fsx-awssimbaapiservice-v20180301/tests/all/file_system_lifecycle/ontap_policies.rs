// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FSx for NetApp ONTAP policy slices — SnapMirror policies and
//! snapshot policies — driven over the wire.
//!
//! RULE: a policy is a resource of its own, created on EITHER a file system or one of its
//! SVMs and never both, which is what its `Scope` reports: an accepted create persists the
//! policy in `CREATING` with every documented default materialised and answers with the
//! stored record, the clock then owes it `CREATED`, the describe operation reports exactly
//! those stored values, the update replaces the members it carries, and the delete moves the
//! record to `DELETING` and lets the clock remove it. A `SnapshotCopyMode` and
//! `RetentionRules` combination the documentation refuses is refused on the create AND on
//! the update, and a resource the account does not have is the not-found error the model
//! declares for it.
//!
//! Every case goes through the same front door a client uses, through the helpers in the
//! parent suite. The tables span what the rules generalise over — the snapshot copy mode,
//! the presence of the retention rules and of each optional member, the two policy families,
//! and which of the two scope members the request carries — rather than one recorded
//! request.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    emulator, ok, with_client_request_token, Emulator, ACCOUNT, REGION,
};
use crate::not_found_messages::error_of;

/// How long a wait below gives a clock-owed transition to be materialised: far longer than
/// the service's own transition span, so what the wait is bounded by is the CONDITION and
/// not how fast the host runs.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
/// How long a wait below pauses between two looks.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// The modelled lifecycle values this suite observes (`SnapmirrorPolicyLifecycle` and
/// `SnapshotPolicyLifecycle`, each CREATING | CREATED | DELETING | FAILED).
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const CREATED: &str = "CREATED";
/// See [`CREATING`].
const DELETING: &str = "DELETING";

/// The modelled `SnapmirrorPolicyScope` / `SnapshotPolicyScope` value of a policy created on
/// a file system (FILESYSTEM | SVM).
const SCOPE_FILE_SYSTEM: &str = "FILESYSTEM";

/// The requirement AWS words on both creates' `FileSystemId` and `StorageVirtualMachineId`
/// members, which is the message a request naming neither or both is refused with.
const SCOPE_REQUIREMENT: &str =
    "You must specify either a FileSystemId or a StorageVirtualMachineId, but not both.";

/// An SVM id satisfying the modelled `StorageVirtualMachineId` (`length 21..21`,
/// `^(svm-[0-9a-f]{17,})$`). The account has no SVM — nothing in this emulator mints one yet
/// — so a create scoped to this id is refused rather than served.
const ABSENT_SVM: &str = "svm-0123456789abcdef0";

/// Create an ONTAP file system and answer its `FileSystemId`.
async fn ontap_file_system(router: &Emulator) -> String {
    let created = ok(
        router,
        "CreateFileSystem",
        &with_client_request_token(&json!({
            "FileSystemType": "ONTAP",
            "StorageCapacity": 1200,
            "StorageType": "SSD",
            "SubnetIds": [router.subnet_a(), router.subnet_b()],
        })),
    )
    .await;
    created["FileSystem"]["FileSystemId"]
        .as_str()
        .expect("CreateFileSystem must answer a FileSystemId")
        .to_string()
}

/// One `SnapshotPolicySchedule`, as the modelled shape requires it: an ONTAP schedule name
/// and the number of snapshots to retain for it.
fn schedule(name: &str, count: i64) -> Value {
    json!({ "Name": name, "Count": count })
}

/// Every SnapMirror policy this store holds, as `DescribeSnapmirrorPolicies` reports them.
async fn described_snapmirror_policies(router: &MultiServiceRouter) -> Vec<Value> {
    ok(router, "DescribeSnapmirrorPolicies", &json!({}))
        .await
        .get("SnapmirrorPolicies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every snapshot policy this store holds, as `DescribeSnapshotPolicies` reports them.
async fn described_snapshot_policies(router: &MultiServiceRouter) -> Vec<Value> {
    ok(router, "DescribeSnapshotPolicies", &json!({}))
        .await
        .get("SnapshotPolicies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The stored SnapMirror policy with this id, or `None` when the store holds none.
async fn described_snapmirror_policy(
    router: &MultiServiceRouter,
    policy_id: &str,
) -> Option<Value> {
    described_snapmirror_policies(router)
        .await
        .into_iter()
        .find(|policy| policy["SnapmirrorPolicyId"] == json!(policy_id))
}

/// The stored snapshot policy with this id, or `None` when the store holds none.
async fn described_snapshot_policy(router: &MultiServiceRouter, policy_id: &str) -> Option<Value> {
    described_snapshot_policies(router)
        .await
        .into_iter()
        .find(|policy| policy["SnapshotPolicyId"] == json!(policy_id))
}

/// Look again until the SnapMirror policy's `Lifecycle` is `expected`, or
/// [`SETTLE_DEADLINE`] passes. A transition this service owes the clock is never slept for:
/// the condition is polled, so the case is bounded by what the store reports and not by how
/// fast the host runs.
async fn await_snapmirror_lifecycle(
    router: &MultiServiceRouter,
    policy_id: &str,
    expected: &str,
) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let reported = described_snapmirror_policy(router, policy_id)
            .await
            .map(|policy| policy["Lifecycle"].clone());
        if reported == Some(json!(expected)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "policy {policy_id}: expected Lifecycle {expected}, got {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Look again until the store holds no snapshot policy with this id, or
/// [`SETTLE_DEADLINE`] passes.
async fn await_snapshot_policy_removed(router: &MultiServiceRouter, policy_id: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let reported = described_snapshot_policy(router, policy_id).await;
        if reported.is_none() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "policy {policy_id}: expected the clock to remove the record, still got {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Whether `id` matches the modelled id pattern `^<prefix>-[0-9a-f]{17}$`
/// (`SnapmirrorPolicyId` `snapmirrorpol-`, `SnapshotPolicyId` `snapshotpol-`).
fn is_well_formed_policy_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 17
            && hex
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_uppercase())
    })
}

/// One row of the SnapMirror policy create table: the snapshot copy mode, the retention
/// rules, the transfer schedule, whether network compression is stated, and the throttle.
type SnapmirrorCreateCase<'row> = (
    Option<&'row str>,
    Value,
    Option<&'row str>,
    Option<bool>,
    Option<i64>,
);

/// RULE: a created SnapMirror policy is persisted against the ONTAP file system it was
/// created on with every documented default materialised, the response is that stored
/// record, and `DescribeSnapmirrorPolicies` reports the very same values.
#[tokio::test]
async fn create_persists_a_snapmirror_policy_and_describe_reports_it_back() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;

    // (SnapshotCopyMode, RetentionRules, TransferSchedule, NetworkCompressionEnabled,
    // ThrottleInKbps): the modelled `SnapmirrorPolicySnapshotCopyMode` members that admit
    // each rule shape, with the optional members present and absent so the documented
    // default of `NetworkCompressionEnabled` is exercised both ways.
    let cases: &[SnapmirrorCreateCase<'_>] = &[
        (None, json!([]), None, None, None),
        (
            None,
            json!([{ "Label": "daily", "Count": 7 }]),
            Some("hourly"),
            Some(true),
            Some(1024),
        ),
        (
            Some("COPY_ALL_SOURCE_SNAPSHOTS"),
            json!([]),
            Some("daily"),
            Some(false),
            Some(0),
        ),
        (Some("COPY_LATEST_SOURCE_SNAPSHOT"), json!([]), None, None, None),
        (
            Some("NO_CREATE_SNAPSHOT_ON_SOURCE"),
            json!([
                { "Label": "weekly", "Count": 4, "CreationSchedule": "weekly" },
                { "Label": "monthly", "Count": 12 },
            ]),
            None,
            Some(true),
            None,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (snapshot_copy_mode, retention_rules, transfer_schedule, compression, throttle) = case;
        let mut body = with_client_request_token(&json!({
            "FileSystemId": file_system_id,
            "Name": format!("policy-{index}"),
        }));
        if let Some(mode) = snapshot_copy_mode {
            body["SnapshotCopyMode"] = json!(mode);
        }
        let rules = retention_rules.as_array().cloned().unwrap_or_default();
        if !rules.is_empty() {
            body["RetentionRules"] = retention_rules.clone();
        }
        if let Some(transfer_schedule) = transfer_schedule {
            body["TransferSchedule"] = json!(transfer_schedule);
        }
        if let Some(compression) = compression {
            body["NetworkCompressionEnabled"] = json!(compression);
        }
        if let Some(throttle) = throttle {
            body["ThrottleInKbps"] = json!(throttle);
        }

        let policy = ok(&router, "CreateSnapmirrorPolicy", &body).await["SnapmirrorPolicy"].clone();
        let id = policy["SnapmirrorPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_well_formed_policy_id(&id, "snapmirrorpol-"),
            "case {case:?}: expected a SnapmirrorPolicyId matching \
             ^snapmirrorpol-[0-9a-f]{{17}}$, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh SnapmirrorPolicyId, got the already-minted {id:?}"
        );
        assert_eq!(
            policy["Lifecycle"],
            json!(CREATING),
            "case {case:?}: expected Lifecycle {CREATING} on create, got {:?}",
            policy["Lifecycle"]
        );
        assert_eq!(
            policy["Scope"],
            json!(SCOPE_FILE_SYSTEM),
            "case {case:?}: a policy created on a file system has Scope {SCOPE_FILE_SYSTEM}, \
             got {:?}",
            policy["Scope"]
        );
        assert_eq!(
            policy["FileSystemId"],
            json!(file_system_id),
            "case {case:?}: expected the policy to be created on {file_system_id}, got {:?}",
            policy["FileSystemId"]
        );
        assert_eq!(
            policy["StorageVirtualMachineId"],
            Value::Null,
            "case {case:?}: a policy created on a file system is on no SVM, got {:?}",
            policy["StorageVirtualMachineId"]
        );
        assert_eq!(
            policy["ResourceARN"],
            json!(format!(
                "arn:aws:fsx:{REGION}:{ACCOUNT}:snapmirror-policy/{id}"
            )),
            "case {case:?}: expected the SnapMirror policy ARN for the signed region, got {:?}",
            policy["ResourceARN"]
        );
        assert_eq!(
            policy["Name"],
            json!(format!("policy-{index}")),
            "case {case:?}: expected the stated Name, got {:?}",
            policy["Name"]
        );
        assert_eq!(
            policy["SnapshotCopyMode"],
            snapshot_copy_mode.map_or(Value::Null, |mode| json!(mode)),
            "case {case:?}: expected the stated SnapshotCopyMode, got {:?}",
            policy["SnapshotCopyMode"]
        );
        // "This value defaults to false" (`CreateSnapmirrorPolicy$NetworkCompressionEnabled`):
        // the default is MATERIALISED, so the policy reports the behaviour it really has.
        assert_eq!(
            policy["NetworkCompressionEnabled"],
            json!(compression.unwrap_or(false)),
            "case {case:?}: expected NetworkCompressionEnabled to be the stated value or the \
             documented default false, got {:?}",
            policy["NetworkCompressionEnabled"]
        );
        assert_eq!(
            policy["ThrottleInKbps"],
            throttle.map_or(Value::Null, |throttle| json!(throttle)),
            "case {case:?}: expected the stated ThrottleInKbps, got {:?}",
            policy["ThrottleInKbps"]
        );
        assert_eq!(
            policy["TransferSchedule"],
            transfer_schedule.map_or(Value::Null, |schedule| json!(schedule)),
            "case {case:?}: expected the stated TransferSchedule, got {:?}",
            policy["TransferSchedule"]
        );
        if rules.is_empty() {
            assert_eq!(
                policy["RetentionRules"],
                Value::Null,
                "case {case:?}: a policy with no retention rules reports none, got {:?}",
                policy["RetentionRules"]
            );
        } else {
            assert_eq!(
                policy["RetentionRules"], *retention_rules,
                "case {case:?}: expected the stated RetentionRules, got {:?}",
                policy["RetentionRules"]
            );
        }

        // The persisted record, read back through the implemented reader.
        let described = described_snapmirror_policy(&router, &id)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: DescribeSnapmirrorPolicies must report {id}")
            });
        for member in [
            "SnapmirrorPolicyId",
            "FileSystemId",
            "ResourceARN",
            "Name",
            "SnapshotCopyMode",
            "RetentionRules",
            "TransferSchedule",
            "NetworkCompressionEnabled",
            "ThrottleInKbps",
            "Scope",
            "UUID",
        ] {
            assert_eq!(
                described[member], policy[member],
                "case {case:?}: DescribeSnapmirrorPolicies must report the stored {member}; \
                 create said {:?} and describe said {:?}",
                policy[member], described[member]
            );
        }

        minted.push(id);
    }

    for id in &minted {
        await_snapmirror_lifecycle(&router, id, CREATED).await;
    }
}

/// RULE: what each `SnapshotCopyMode` member documents about `RetentionRules` holds wherever
/// the pair is written — on the create, and on the update that replaces the rules of a
/// policy whose stored mode constrains them.
#[tokio::test]
async fn retention_rules_follow_the_snapshot_copy_mode() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;
    let rules = json!([{ "Label": "daily", "Count": 7 }]);

    // (SnapshotCopyMode, whether RetentionRules are stated, whether the create is served):
    // the three modelled members and the absent mode, each with and without rules.
    let cases: &[(Option<&str>, bool, bool)] = &[
        (None, false, true),
        (None, true, true),
        (Some("COPY_ALL_SOURCE_SNAPSHOTS"), false, true),
        (Some("COPY_ALL_SOURCE_SNAPSHOTS"), true, false),
        (Some("COPY_LATEST_SOURCE_SNAPSHOT"), false, true),
        (Some("COPY_LATEST_SOURCE_SNAPSHOT"), true, false),
        (Some("NO_CREATE_SNAPSHOT_ON_SOURCE"), true, true),
        (Some("NO_CREATE_SNAPSHOT_ON_SOURCE"), false, false),
    ];

    for (index, case) in cases.iter().enumerate() {
        let (snapshot_copy_mode, with_rules, served) = *case;
        let mut body = with_client_request_token(&json!({
            "FileSystemId": file_system_id,
            "Name": format!("combination-{index}"),
        }));
        if let Some(mode) = snapshot_copy_mode {
            body["SnapshotCopyMode"] = json!(mode);
        }
        if with_rules {
            body["RetentionRules"] = rules.clone();
        }

        if served {
            let policy = ok(&router, "CreateSnapmirrorPolicy", &body).await["SnapmirrorPolicy"]
                .clone();
            assert_eq!(
                policy["SnapshotCopyMode"],
                snapshot_copy_mode.map_or(Value::Null, |mode| json!(mode)),
                "case {case:?}: expected the create to store the stated mode, got {:?}",
                policy["SnapshotCopyMode"]
            );
        } else {
            let (status, code, message) =
                error_of(&router, "CreateSnapmirrorPolicy", &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "BadRequest"),
                "case {case:?}: expected HTTP 400 BadRequest for a combination the \
                 documentation refuses, got {status} {code}"
            );
            let mode = snapshot_copy_mode.unwrap_or_default();
            let expected = if with_rules {
                format!("You can't specify RetentionRules when SnapshotCopyMode is {mode}.")
            } else {
                format!("You must specify RetentionRules when SnapshotCopyMode is {mode}.")
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message to state the requirement for that mode, \
                 got {message:?}"
            );
        }
    }

    // The same rule on the UPDATE: a policy stored with a mode that forbids retention rules
    // cannot be given any, while one that requires them cannot be left without.
    let forbidding = ok(
        &router,
        "CreateSnapmirrorPolicy",
        &with_client_request_token(&json!({
            "FileSystemId": file_system_id,
            "Name": "update-forbids-rules",
            "SnapshotCopyMode": "COPY_LATEST_SOURCE_SNAPSHOT",
        })),
    )
    .await["SnapmirrorPolicy"]["SnapmirrorPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, message) = error_of(
        &router,
        "UpdateSnapmirrorPolicy",
        &json!({ "SnapmirrorPolicyId": forbidding, "RetentionRules": rules }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "expected HTTP 400 BadRequest for an update adding rules to a policy whose stored \
         mode forbids them, got {status} {code}"
    );
    assert_eq!(
        message,
        "You can't specify RetentionRules when SnapshotCopyMode is \
         COPY_LATEST_SOURCE_SNAPSHOT.",
        "expected the message to state the requirement for the stored mode, got {message:?}"
    );

    let requiring = ok(
        &router,
        "CreateSnapmirrorPolicy",
        &with_client_request_token(&json!({
            "FileSystemId": file_system_id,
            "Name": "update-requires-rules",
            "SnapshotCopyMode": "NO_CREATE_SNAPSHOT_ON_SOURCE",
            "RetentionRules": rules,
        })),
    )
    .await["SnapmirrorPolicy"]["SnapmirrorPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, _) = error_of(
        &router,
        "UpdateSnapmirrorPolicy",
        &json!({ "SnapmirrorPolicyId": requiring, "RetentionRules": [] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "BadRequest"),
        "expected HTTP 400 BadRequest for an update emptying the rules of a policy whose \
         stored mode requires them, got {status} {code}"
    );
    // The refused update left the stored rules alone.
    let described = described_snapmirror_policy(&router, &requiring)
        .await
        .expect("the policy must still be stored");
    assert_eq!(
        described["RetentionRules"], rules,
        "a refused update must leave the stored RetentionRules as they were, got {:?}",
        described["RetentionRules"]
    );
}

/// RULE: a policy is created on EITHER a file system or one of its SVMs and never both, so a
/// request naming neither or both is refused with the requirement AWS documents, and one
/// naming a resource the account does not have is that resource's not-found error. Both
/// policy families follow it.
#[tokio::test]
async fn a_policy_is_created_on_exactly_one_resource_the_account_has() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;

    // (operation, the members that name what the policy is created on, the required members
    // of that family)
    let families: &[(&str, Value)] = &[
        ("CreateSnapmirrorPolicy", json!({ "Name": "scope-case" })),
        (
            "CreateSnapshotPolicy",
            json!({ "Name": "scope-case", "Schedules": [{ "Name": "daily", "Count": 7 }] }),
        ),
    ];

    for (operation, base) in families {
        // Neither member, and both at once: the request names no scope at all.
        for scope in [json!({}), json!({ "FileSystemId": file_system_id, "StorageVirtualMachineId": ABSENT_SVM })] {
            let mut body = base.clone();
            for (member, value) in scope.as_object().expect("the scope is an object") {
                body[member] = value.clone();
            }
            let (status, code, message) =
                error_of(&router, operation, &with_client_request_token(&body)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "BadRequest"),
                "case {operation} with {scope}: expected HTTP 400 BadRequest, got {status} \
                 {code}"
            );
            assert_eq!(
                message, SCOPE_REQUIREMENT,
                "case {operation} with {scope}: expected the documented requirement as the \
                 message, got {message:?}"
            );
        }

        // A file system the account does not have.
        let absent_file_system = "fs-0000000000badbeef";
        let mut body = base.clone();
        body["FileSystemId"] = json!(absent_file_system);
        let (status, code, message) =
            error_of(&router, operation, &with_client_request_token(&body)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FileSystemNotFound"),
            "case {operation}: expected HTTP 400 FileSystemNotFound, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("File system '{absent_file_system}' does not exist."),
            "case {operation}: expected the message to name the requested file system, got \
             {message:?}"
        );

        // An SVM the account does not have: nothing in this emulator mints one yet, so every
        // SVM id resolves to nothing.
        let mut body = base.clone();
        body["StorageVirtualMachineId"] = json!(ABSENT_SVM);
        let (status, code, message) =
            error_of(&router, operation, &with_client_request_token(&body)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "StorageVirtualMachineNotFound"),
            "case {operation}: expected HTTP 400 StorageVirtualMachineNotFound, got {status} \
             {code}"
        );
        assert_eq!(
            message,
            format!("Storage virtual machine '{ABSENT_SVM}' does not exist."),
            "case {operation}: expected the message to name the requested SVM, got {message:?}"
        );
    }

    // A policy is an ONTAP resource, so a file system of another type holds none.
    let open_zfs = ok(
        &router,
        "CreateFileSystem",
        &with_client_request_token(&json!({
            "FileSystemType": "OPENZFS",
            "StorageCapacity": 1200,
            "StorageType": "INTELLIGENT_TIERING",
            "SubnetIds": [router.subnet_a()],
        })),
    )
    .await["FileSystem"]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for (operation, base, policies) in [
        (
            "CreateSnapmirrorPolicy",
            json!({ "Name": "wrong-type" }),
            "SnapMirror policies",
        ),
        (
            "CreateSnapshotPolicy",
            json!({ "Name": "wrong-type", "Schedules": [{ "Name": "daily", "Count": 7 }] }),
            "Snapshot policies",
        ),
    ] {
        let mut body = base.clone();
        body["FileSystemId"] = json!(open_zfs);
        let (status, code, message) =
            error_of(&router, operation, &with_client_request_token(&body)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {operation}: expected HTTP 400 BadRequest for a file system of another \
             type, got {status} {code}"
        );
        assert_eq!(
            message,
            format!("{policies} are not supported for file systems of type OPENZFS."),
            "case {operation}: expected the message to name the file system's type, got \
             {message:?}"
        );
    }

    // Nothing above stored a policy.
    let snapmirror = described_snapmirror_policies(&router).await;
    let snapshot = described_snapshot_policies(&router).await;
    assert!(
        snapmirror.is_empty() && snapshot.is_empty(),
        "a refused create must store nothing, got {snapmirror:?} and {snapshot:?}"
    );
}

/// RULE: a created snapshot policy is persisted with its schedules and the documented
/// default of `Enabled`, `DescribeSnapshotPolicies` reports the stored record, the update
/// replaces `Enabled`, and the delete moves the record to `DELETING` and lets the clock
/// remove it.
#[tokio::test]
async fn snapshot_policy_round_trips_through_its_own_lifecycle() {
    let router = emulator().await;
    let file_system_id = ontap_file_system(&router).await;

    // (Enabled as stated, the schedules): the documented default both ways, and one and
    // several schedules with their optional members present and absent.
    let cases: &[(Option<bool>, Value)] = &[
        (None, json!([schedule("daily", 7)])),
        (
            Some(false),
            json!([
                schedule("hourly", 24),
                { "Name": "weekly", "Count": 4, "Prefix": "weekly", "SnapmirrorLabel": "weekly" },
            ]),
        ),
        (Some(true), json!([schedule("monthly", 0)])),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (enabled, schedules) = case;
        let mut body = with_client_request_token(&json!({
            "FileSystemId": file_system_id,
            "Name": format!("snapshot-policy-{index}"),
            "Schedules": schedules,
        }));
        if let Some(enabled) = enabled {
            body["Enabled"] = json!(enabled);
        }

        let policy = ok(&router, "CreateSnapshotPolicy", &body).await["SnapshotPolicy"].clone();
        let id = policy["SnapshotPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_well_formed_policy_id(&id, "snapshotpol-"),
            "case {case:?}: expected a SnapshotPolicyId matching ^snapshotpol-[0-9a-f]{{17}}$, \
             got {id:?}"
        );
        assert_eq!(
            policy["Lifecycle"],
            json!(CREATING),
            "case {case:?}: expected Lifecycle {CREATING} on create, got {:?}",
            policy["Lifecycle"]
        );
        assert_eq!(
            policy["ResourceARN"],
            json!(format!(
                "arn:aws:fsx:{REGION}:{ACCOUNT}:snapshot-policy/{id}"
            )),
            "case {case:?}: expected the snapshot policy ARN for the signed region, got {:?}",
            policy["ResourceARN"]
        );
        // "This value defaults to true" (`CreateSnapshotPolicy$Enabled`).
        assert_eq!(
            policy["Enabled"],
            json!(enabled.unwrap_or(true)),
            "case {case:?}: expected Enabled to be the stated value or the documented default \
             true, got {:?}",
            policy["Enabled"]
        );
        assert_eq!(
            policy["Schedules"], *schedules,
            "case {case:?}: expected the stated Schedules, got {:?}",
            policy["Schedules"]
        );
        assert_eq!(
            policy["Scope"],
            json!(SCOPE_FILE_SYSTEM),
            "case {case:?}: a policy created on a file system has Scope {SCOPE_FILE_SYSTEM}, \
             got {:?}",
            policy["Scope"]
        );

        let described = described_snapshot_policy(&router, &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeSnapshotPolicies must report {id}"));
        assert_eq!(
            described, policy,
            "case {case:?}: DescribeSnapshotPolicies must report exactly the stored record"
        );

        minted.push(id);
    }

    // `Enabled` is the one member the update carries: it replaces the stored value, and a
    // request that omits it leaves the policy as it was.
    let id = minted[0].clone();
    for enabled in [false, true] {
        let answered = ok(
            &router,
            "UpdateSnapshotPolicy",
            &json!({ "SnapshotPolicyId": id, "Enabled": enabled }),
        )
        .await["SnapshotPolicy"]
            .clone();
        assert_eq!(
            answered["Enabled"],
            json!(enabled),
            "case Enabled={enabled}: expected the update to replace Enabled, got {:?}",
            answered["Enabled"]
        );
        let described = described_snapshot_policy(&router, &id)
            .await
            .expect("the policy must still be stored");
        assert_eq!(
            described, answered,
            "case Enabled={enabled}: DescribeSnapshotPolicies must report what the update \
             answered"
        );
    }
    let untouched = ok(
        &router,
        "UpdateSnapshotPolicy",
        &json!({ "SnapshotPolicyId": id }),
    )
    .await["SnapshotPolicy"]
        .clone();
    assert_eq!(
        untouched["Enabled"],
        json!(true),
        "an update that states nothing must leave Enabled as it was, got {:?}",
        untouched["Enabled"]
    );

    // The delete moves the record to `DELETING` and the clock removes it.
    let deleted = ok(
        &router,
        "DeleteSnapshotPolicy",
        &with_client_request_token(&json!({ "SnapshotPolicyId": id })),
    )
    .await;
    assert_eq!(
        deleted["SnapshotPolicyId"],
        json!(id),
        "the delete must answer the id it deleted, got {:?}",
        deleted["SnapshotPolicyId"]
    );
    assert_eq!(
        deleted["Lifecycle"],
        json!(DELETING),
        "the delete must answer the lifecycle the policy now has, got {:?}",
        deleted["Lifecycle"]
    );
    await_snapshot_policy_removed(&router, &id).await;

    let (status, code, message) = error_of(
        &router,
        "DescribeSnapshotPolicies",
        &json!({ "SnapshotPolicyIds": [id] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "SnapshotPolicyNotFound"),
        "expected HTTP 400 SnapshotPolicyNotFound once the policy is removed, got {status} \
         {code}"
    );
    assert_eq!(
        message,
        format!("Snapshot policy '{id}' does not exist."),
        "expected the message to name the requested id, got {message:?}"
    );

    // The policies the delete did not name are untouched.
    let held: Vec<String> = described_snapshot_policies(&router)
        .await
        .iter()
        .map(|policy| {
            policy["SnapshotPolicyId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        held,
        minted[1..].to_vec(),
        "a delete must remove only the policy it named, got {held:?}"
    );
}

/// RULE: both policy describes select by their ids and by their `Filters` TOGETHER, page at
/// `MaxResults` with an opaque `NextToken`, and refuse an id they hold nothing for — or one
/// a filter excludes — with that family's not-found error.
#[tokio::test]
async fn policy_describes_select_by_id_and_filter_together_and_page() {
    let router = emulator().await;
    let first_file_system = ontap_file_system(&router).await;
    let second_file_system = ontap_file_system(&router).await;

    let mut on_first: Vec<String> = Vec::new();
    for index in 0..3 {
        let policy = ok(
            &router,
            "CreateSnapmirrorPolicy",
            &with_client_request_token(&json!({
                "FileSystemId": first_file_system,
                "Name": format!("paged-{index}"),
            })),
        )
        .await["SnapmirrorPolicy"]
            .clone();
        on_first.push(
            policy["SnapmirrorPolicyId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    let on_second = ok(
        &router,
        "CreateSnapmirrorPolicy",
        &with_client_request_token(&json!({
            "FileSystemId": second_file_system,
            "Name": "other-file-system",
        })),
    )
    .await["SnapmirrorPolicy"]["SnapmirrorPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let filtered = ok(
        &router,
        "DescribeSnapmirrorPolicies",
        &json!({
            "Filters": [{ "Name": "file-system-id", "Values": [first_file_system] }],
        }),
    )
    .await;
    let mut selected: Vec<String> = filtered["SnapmirrorPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|policy| {
            policy["SnapmirrorPolicyId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    selected.sort();
    let mut expected = on_first.clone();
    expected.sort();
    assert_eq!(
        selected, expected,
        "the file-system-id filter must select exactly the policies of that file system, got \
         {selected:?}"
    );

    // A policy created on a file system is on no SVM, so no value of the
    // `storage-virtual-machine-id` filter selects one.
    let by_svm = ok(
        &router,
        "DescribeSnapmirrorPolicies",
        &json!({
            "Filters": [{
                "Name": "storage-virtual-machine-id",
                "Values": [ABSENT_SVM],
            }],
        }),
    )
    .await;
    assert_eq!(
        by_svm["SnapmirrorPolicies"],
        json!([]),
        "no file-system-scoped policy is selected by an SVM filter, got {:?}",
        by_svm["SnapmirrorPolicies"]
    );

    // One page at a time: every record is walked exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 2 });
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(&router, "DescribeSnapmirrorPolicies", &body).await;
        let ids: Vec<String> = page["SnapmirrorPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|policy| {
                policy["SnapmirrorPolicyId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            ids.len() <= 2,
            "a page of MaxResults 2 must hold at most two policies, got {ids:?}"
        );
        walked.extend(ids);
        next_token = page["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            walked.len() <= 4,
            "paging must end: walked {walked:?} over four stored policies"
        );
    }
    walked.sort();
    let mut every_policy = on_first.clone();
    every_policy.push(on_second.clone());
    every_policy.sort();
    assert_eq!(
        walked, every_policy,
        "paging must walk every stored policy exactly once, got {walked:?}"
    );

    // An id this store holds nothing for, and an id the filters exclude.
    let absent = "snapmirrorpol-0000000000badbeef";
    let (status, code, message) = error_of(
        &router,
        "DescribeSnapmirrorPolicies",
        &json!({ "SnapmirrorPolicyIds": [absent] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "SnapmirrorPolicyNotFound"),
        "expected HTTP 400 SnapmirrorPolicyNotFound for an absent id, got {status} {code}"
    );
    assert_eq!(
        message,
        format!("SnapMirror policy '{absent}' does not exist."),
        "expected the message to name the requested id, got {message:?}"
    );
    let (status, code, _) = error_of(
        &router,
        "DescribeSnapmirrorPolicies",
        &json!({
            "SnapmirrorPolicyIds": [on_second],
            "Filters": [{ "Name": "file-system-id", "Values": [first_file_system] }],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "SnapmirrorPolicyNotFound"),
        "expected HTTP 400 SnapmirrorPolicyNotFound for an id the filter excludes, got \
         {status} {code}"
    );

    // A `Filter` neither listing can read as a selection is a bad request, not an empty
    // page: a name outside the modelled filter-name enum is refused by the modelled
    // constraint before the handler runs, and a filter carrying no name at all is refused by
    // the handler, naming the names the listing accepts.
    for operation in ["DescribeSnapmirrorPolicies", "DescribeSnapshotPolicies"] {
        let (status, code, _) = error_of(
            &router,
            operation,
            &json!({ "Filters": [{ "Name": "volume-id", "Values": ["fsvol-0"] }] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {operation}: expected HTTP 400 BadRequest for a filter name outside the \
             modelled enum, got {status} {code}"
        );
        let (status, code, message) = error_of(
            &router,
            operation,
            &json!({ "Filters": [{ "Values": [first_file_system] }] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "BadRequest"),
            "case {operation}: expected HTTP 400 BadRequest for a filter that names nothing, \
             got {status} {code}"
        );
        assert!(
            message.contains("file-system-id") && message.contains("storage-virtual-machine-id"),
            "case {operation}: expected the message to name the supported filter names, got \
             {message:?}"
        );
    }
}
