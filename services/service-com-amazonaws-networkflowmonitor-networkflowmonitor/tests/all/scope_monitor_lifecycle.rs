// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Network Flow Monitor scope + monitor slice — a scope, a
//! monitor and a PERFORMANCE monitor — driven over the service's own `restJson1`
//! wire protocol through the same `MultiServiceRouter` the production emulator
//! dispatches with.
//!
//! Every test asserts a RULE over a SET of inputs — the shapes the bootstrap issue
//! records are one row each: a create answers from the record it just stored, a
//! read answers that same record, a delete is observable by the next reader, and a
//! page token walks the store exactly once.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The ONE harness every suite in this crate dispatches through
// (`tests/all/harness.rs`).
use crate::harness::{
    client_token, extra_region, is_uuid_shaped, targets, Answer, Emulator, ACCOUNT_ID,
    PAST_PROVISIONING, REGION, SIGNING_NAME,
};

/// The statuses `MonitorStatus` declares, from the model
/// (`src/ops/types/shared_types.rs`: `MONITOR_STATUS_VALUES`). Used to assert the
/// `monitorStatus` list filter over the whole published set rather than one value.
const MONITOR_STATUS_VALUES: &[&str] = &["ACTIVE", "DELETING", "ERROR", "INACTIVE", "PENDING"];

/// The local resource types `MonitorLocalResourceType` declares, from the model
/// (`MONITOR_LOCAL_RESOURCE_TYPE_VALUES`).
const LOCAL_RESOURCE_TYPES: &[&str] = &[
    "AWS::AvailabilityZone",
    "AWS::EC2::Subnet",
    "AWS::EC2::VPC",
    "AWS::EKS::Cluster",
    "AWS::Region",
];

/// One row of the scope create-and-read-back table.
struct ScopeRow {
    case: &'static str,
    body: Value,
}

fn scope_rows() -> Vec<ScopeRow> {
    // 25 targets: the count dimension, well inside `TargetResourceList`'s 1..100.
    let many: Vec<(String, String)> = (0..25)
        .map(|index| (format!("{:012}", 100_000_000_000u64 + index), format!("us-west-{}", index % 2 + 1)))
        .collect();
    let many_refs: Vec<(&str, &str)> = many
        .iter()
        .map(|(account, region)| (account.as_str(), region.as_str()))
        .collect();
    vec![
        ScopeRow {
            case: "one target, no clientToken and no tags",
            body: json!({ "targets": targets(&[("123456789012", "us-east-1")]) }),
        },
        ScopeRow {
            case: "the issue's recorded shape, with a clientToken and tags",
            body: json!({
                "targets": targets(&[("123456789012", "us-east-1")]),
                "clientToken": client_token(1),
                "tags": { "owner": "net-team", "env": "prod" }
            }),
        },
        ScopeRow {
            case: "three targets spanning accounts and Regions",
            body: json!({
                "targets": targets(&[
                    ("111111111111", "eu-west-1"),
                    ("222222222222", "ap-southeast-2"),
                    ("333333333333", "us-east-2"),
                ]),
                "tags": { "cost-center": "4711" }
            }),
        },
        ScopeRow {
            case: "25 targets: the list's count dimension",
            body: json!({ "targets": targets(&many_refs) }),
        },
    ]
}

#[tokio::test]
async fn a_created_scope_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    // One store, one scope per Region: the account's scope quota is per Region
    // (`state::SCOPE_QUOTA`), so each row claims its own Region's slot and every
    // row's record still lives in the same store.
    for (index, row) in scope_rows().into_iter().enumerate() {
        let region = extra_region(index);
        let created = emulator.create_scope_in(region, row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );

        let scope_id = created.text("scopeId");
        assert!(
            is_uuid_shaped(&scope_id),
            "case {:?}: expected a UUID-shaped scopeId, got {scope_id:?}",
            row.case
        );
        assert!(
            seen_ids.insert(scope_id.clone()),
            "case {:?}: expected a fresh scopeId, got the already-minted {scope_id:?}",
            row.case
        );
        assert_eq!(
            created.text("scopeArn"),
            format!("arn:aws:{SIGNING_NAME}:{region}:{ACCOUNT_ID}:scope/{scope_id}"),
            "case {:?}: expected the ARN of {scope_id}, got {:?}",
            row.case,
            created.text("scopeArn")
        );
        // `MonitorStatus`' PENDING is documented as "in the process of being
        // created"; `ScopeStatus`' IN_PROGRESS is its scope counterpart, so a scope
        // that was just asked for is not yet SUCCEEDED.
        assert_eq!(
            created.text("status"),
            "IN_PROGRESS",
            "case {:?}: a scope is set up before it succeeds, got {:?}",
            row.case,
            created.text("status")
        );
        assert_eq!(
            created.member("tags"),
            row.body.get("tags").unwrap_or(&Value::Null),
            "case {:?}: expected the submitted tags echoed from the record, got {:?}",
            row.case,
            created.member("tags")
        );

        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored scope, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.text("scopeId"),
            scope_id,
            "case {:?}: expected scopeId {scope_id}, got {:?}",
            row.case,
            read.text("scopeId")
        );
        assert_eq!(
            read.text("scopeArn"),
            created.text("scopeArn"),
            "case {:?}: expected the stored scopeArn, got {:?}",
            row.case,
            read.text("scopeArn")
        );
        assert_eq!(
            read.member("targets"),
            row.body.get("targets").expect("every row carries targets"),
            "case {:?}: expected the targets exactly as submitted, got {:?}",
            row.case,
            read.member("targets")
        );
        assert_eq!(
            read.member("tags"),
            row.body.get("tags").unwrap_or(&Value::Null),
            "case {:?}: expected the stored tags, got {:?}",
            row.case,
            read.member("tags")
        );
    }
}

#[tokio::test]
async fn a_scope_arn_carries_the_region_the_request_is_signed_for() {
    // The ARN's Region segment comes from the request's credential scope, not from
    // a constant: the same create under three scopes mints three ARNs.
    for region in ["us-east-1", "eu-central-1", "ap-northeast-1"] {
        let emulator = Emulator::new_in(region);
        let created = emulator
            .create_scope(json!({ "targets": targets(&[("123456789012", region)]) }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: expected 200, got {} with body {}",
            created.status, created.body
        );
        let scope_id = created.text("scopeId");
        assert_eq!(
            created.text("scopeArn"),
            format!("arn:aws:{SIGNING_NAME}:{region}:{ACCOUNT_ID}:scope/{scope_id}"),
            "case {region:?}: expected an ARN in the signed Region, got {:?}",
            created.text("scopeArn")
        );
    }
}

#[tokio::test]
async fn a_scope_reaches_succeeded_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let created = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    let scope_id = created.text("scopeId");
    assert_eq!(
        created.text("status"),
        "IN_PROGRESS",
        "a fresh scope reports IN_PROGRESS, got {:?}",
        created.text("status")
    );
    // A caller acting straight after the create still sees the transient status.
    let immediate = emulator.get_scope(&scope_id).await;
    assert_eq!(
        immediate.text("status"),
        "IN_PROGRESS",
        "case {scope_id:?}: the transient status is observable, got {:?}",
        immediate.text("status")
    );

    tokio::time::sleep(PAST_PROVISIONING).await;

    let settled = emulator.get_scope(&scope_id).await;
    assert_eq!(
        settled.text("status"),
        "SUCCEEDED",
        "case {scope_id:?}: the clock owes the scope SUCCEEDED, got {:?}",
        settled.text("status")
    );
    // A read never changes what the next reader sees.
    let again = emulator.get_scope(&scope_id).await;
    assert_eq!(
        again.body, settled.body,
        "case {scope_id:?}: two reads of one scope must answer the same body, got {} then {}",
        settled.body, again.body
    );
}

/// A `scopeId` of the form this service generates that the store holds nothing
/// under is the modelled 404. A `scopeId` of ANOTHER form never reaches that path —
/// it is a request-shape fault, pinned in `wire_status_and_error_classes.rs`.
#[tokio::test]
async fn get_scope_on_an_id_the_store_does_not_hold_is_404() {
    let emulator = Emulator::new();
    // One real scope exists, so a 404 is about the id and not about an empty store.
    let created = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    let known = created.text("scopeId");

    for case in [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        // The same hex in upper case: still a well-formed identifier, so still the
        // not-found path rather than a refusal of the request's shape.
        "FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFE",
        "11111111-2222-3333-4444-555555555555",
    ] {
        assert_ne!(case, known, "the unknown-id table must not name a real scope");
        let answer = emulator.get_scope(case).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException")
                || answer
                    .text("__type")
                    .contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn one_client_token_creates_one_scope() {
    let emulator = Emulator::new();
    let token = client_token(7);
    let body = json!({
        "targets": targets(&[("444444444444", "us-east-1"), ("555555555555", "us-west-2")]),
        "clientToken": token,
        "tags": { "team": "net" }
    });

    let first = emulator.create_scope(body.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} with body {}",
        first.status, first.body
    );

    // Replaying the SAME token with the SAME parameters answers with the scope that
    // token already minted — three times over, not three scopes.
    for attempt in 0..3 {
        let replay = emulator.create_scope(body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "attempt {attempt}: expected the replay to succeed, got {} with body {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("scopeId"),
            first.text("scopeId"),
            "attempt {attempt}: expected the first scope {:?}, got {:?}",
            first.text("scopeId"),
            replay.text("scopeId")
        );
    }

    // A DIFFERENT token asks for ANOTHER scope, which the account's per-Region
    // quota has no slot for: the modelled 402. In another Region the same request
    // claims that Region's own slot and mints a second scope.
    let refused = emulator
        .create_scope(json!({
            "targets": targets(&[("444444444444", "us-east-1")]),
            "clientToken": client_token(8)
        }))
        .await;
    assert_eq!(
        refused.status, 402,
        "a second scope in one Region is past the quota, got {} with body {}",
        refused.status, refused.body
    );
    let other = emulator
        .create_scope_in(
            extra_region(0),
            json!({
                "targets": targets(&[("444444444444", "us-east-1")]),
                "clientToken": client_token(8)
            }),
        )
        .await;
    assert_eq!(
        other.status, 200,
        "another Region's slot must take the scope, got {} with body {}",
        other.status, other.body
    );
    assert_ne!(
        other.text("scopeId"),
        first.text("scopeId"),
        "a different clientToken must mint a different scope, got {:?} twice",
        other.text("scopeId")
    );

    // The same token with DIFFERENT parameters is the modelled conflict, not a
    // silent replay of a scope the caller did not ask for.
    for (case, changed) in [
        (
            "different targets",
            json!({ "targets": targets(&[("666666666666", "us-east-1")]), "clientToken": token }),
        ),
        (
            "different tags",
            json!({
                "targets": targets(&[("444444444444", "us-east-1"), ("555555555555", "us-west-2")]),
                "clientToken": token,
                "tags": { "team": "other" }
            }),
        ),
    ] {
        let conflict = emulator.create_scope(changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: expected 409, got {} with body {}",
            conflict.status, conflict.body
        );
    }
}

/// One row of the monitor create-and-read-back table.
struct MonitorRow {
    case: &'static str,
    name: &'static str,
    local_resources: Value,
    remote_resources: Option<Value>,
    tags: Option<Value>,
}

fn monitor_rows() -> Vec<MonitorRow> {
    vec![
        MonitorRow {
            case: "the issue's recorded shape: one VPC local resource, nothing else",
            name: "demo-monitor",
            local_resources: json!([{ "type": "AWS::EC2::VPC", "identifier": "vpc-123" }]),
            remote_resources: None,
            tags: None,
        },
        MonitorRow {
            case: "local and remote resources, plus tags",
            name: "monitor.with_remotes-2",
            local_resources: json!([
                { "type": "AWS::EC2::Subnet", "identifier": "arn:aws:ec2:us-west-2:000000000000:subnet/subnet-1" },
                { "type": "AWS::AvailabilityZone", "identifier": "us-west-2b" },
            ]),
            remote_resources: Some(json!([
                { "type": "AWS::AWSService", "identifier": "S3" },
                { "type": "AWS::Region", "identifier": "us-east-1" },
            ])),
            tags: Some(json!({ "owner": "net-team" })),
        },
        MonitorRow {
            case: "an EKS cluster local resource the issue does not cite",
            name: "eks-monitor",
            local_resources: json!([
                { "type": "AWS::EKS::Cluster", "identifier": "arn:aws:eks:us-west-2:000000000000:cluster/prod" },
            ]),
            remote_resources: Some(json!([])),
            tags: Some(json!({})),
        },
        MonitorRow {
            case: "a one-character name, the shortest ResourceName allows",
            name: "m",
            local_resources: json!([{ "type": "AWS::Region", "identifier": "us-west-2" }]),
            remote_resources: None,
            tags: None,
        },
    ]
}

#[tokio::test]
async fn a_created_monitor_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for row in monitor_rows() {
        let mut request = HashMap::from([
            ("monitorName".to_string(), json!(row.name)),
            ("scopeArn".to_string(), json!(scope_arn)),
            ("localResources".to_string(), row.local_resources.clone()),
        ]);
        if let Some(remote) = row.remote_resources.clone() {
            request.insert("remoteResources".to_string(), remote);
        }
        if let Some(tags) = row.tags.clone() {
            request.insert("tags".to_string(), tags);
        }
        let created = emulator.create_monitor(json!(request)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, created.status, created.body
        );
        assert_eq!(
            created.text("monitorArn"),
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:monitor/{}",
                row.name
            ),
            "case {:?}: expected the ARN of {}, got {:?}",
            row.case,
            row.name,
            created.text("monitorArn")
        );
        // "PENDING: The monitor is in the process of being created."
        assert_eq!(
            created.text("monitorStatus"),
            "PENDING",
            "case {:?}: a fresh monitor is PENDING, got {:?}",
            row.case,
            created.text("monitorStatus")
        );
        assert_eq!(
            created.member("localResources"),
            &row.local_resources,
            "case {:?}: expected the submitted localResources, got {:?}",
            row.case,
            created.member("localResources")
        );
        // `remoteResources` is optional in, `@required` out: a monitor created
        // without any has an empty list.
        assert_eq!(
            created.member("remoteResources"),
            row.remote_resources.as_ref().unwrap_or(&json!([])),
            "case {:?}: expected the submitted remoteResources, got {:?}",
            row.case,
            created.member("remoteResources")
        );
        let created_at = created.number("createdAt");
        let modified_at = created.number("modifiedAt");
        assert!(
            created_at > 0.0,
            "case {:?}: expected a real creation timestamp, got {created_at}",
            row.case
        );
        assert_eq!(
            created_at, modified_at,
            "case {:?}: a monitor nothing has changed yet has modifiedAt == createdAt, got {created_at} and {modified_at}",
            row.case
        );

        let read = emulator.get_monitor(row.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored monitor, got {} with body {}",
            row.case, read.status, read.body
        );
        // GetMonitor answers the same record CreateMonitor answered from: every
        // member the clock cannot move is identical.
        for member in [
            "monitorArn",
            "monitorName",
            "localResources",
            "remoteResources",
            "tags",
            "createdAt",
        ] {
            assert_eq!(
                read.member(member),
                created.member(member),
                "case {:?}: expected the stored {member}, got {:?} after the create answered {:?}",
                row.case,
                read.member(member),
                created.member(member)
            );
        }
        // The only member the clock may have moved is the status, and only onto the
        // terminal one it owes — with `modifiedAt` moving exactly when it does.
        match read.text("monitorStatus").as_str() {
            "PENDING" => assert_eq!(
                read.number("modifiedAt"),
                created_at,
                "case {:?}: a still-PENDING monitor has modifiedAt == createdAt, got {}",
                row.case,
                read.number("modifiedAt")
            ),
            "ACTIVE" => assert!(
                read.number("modifiedAt") > created_at,
                "case {:?}: reaching ACTIVE moves modifiedAt past createdAt {created_at}, got {}",
                row.case,
                read.number("modifiedAt")
            ),
            other => panic!(
                "case {:?}: a monitor is PENDING and then ACTIVE, got {other:?}",
                row.case
            ),
        }
    }
}

#[tokio::test]
async fn a_monitor_reaches_active_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let created = emulator
        .create_monitor(json!({
            "monitorName": "settling-monitor",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        created.text("monitorStatus"),
        "PENDING",
        "a fresh monitor is PENDING, got {:?}",
        created.text("monitorStatus")
    );
    let created_at = created.number("createdAt");

    tokio::time::sleep(PAST_PROVISIONING).await;

    let settled = emulator.get_monitor("settling-monitor").await;
    assert_eq!(
        settled.text("monitorStatus"),
        "ACTIVE",
        "the clock owes the monitor ACTIVE, got {:?}",
        settled.text("monitorStatus")
    );
    assert_eq!(
        settled.number("createdAt"),
        created_at,
        "createdAt never moves, got {} then {}",
        created_at,
        settled.number("createdAt")
    );
    let modified_at = settled.number("modifiedAt");
    assert!(
        modified_at > created_at && modified_at - created_at <= 0.75,
        "the transition is due 250..750 ms after the create: createdAt {created_at}, modifiedAt {modified_at}"
    );
    let again = emulator.get_monitor("settling-monitor").await;
    assert_eq!(
        again.body, settled.body,
        "two reads of one monitor must answer the same body, got {} then {}",
        settled.body, again.body
    );
}

/// RULE: `CreateMonitor` models no `ResourceNotFoundException`, so a `scopeArn` that
/// resolves to no scope is refused with the modelled `AccessDeniedException` (403) —
/// the whole table is pinned in `wire_status_and_error_classes.rs`; this row keeps
/// the lifecycle suite's own anchors (a live monitor ARN, another service's ARN) and
/// proves the refused create stored nothing.
#[tokio::test]
async fn create_monitor_refuses_a_scope_arn_that_names_no_scope() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    // A real monitor, so its ARN is a live ARN of this service that is still not a
    // scope.
    let monitor = emulator
        .create_monitor(json!({
            "monitorName": "anchor-monitor",
            "scopeArn": scope_arn.clone(),
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        monitor.status, 200,
        "the anchor monitor must be created, got {} with body {}",
        monitor.status, monitor.body
    );

    let rows = [
        (
            "a well-formed scope ARN for an id the store never minted",
            format!("arn:aws:{SIGNING_NAME}:us-east-1:123456789012:scope/does-not-exist"),
        ),
        (
            "this service's own scope ARN form in another Region",
            format!("arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT_ID}:scope/00000000-0000-4000-8000-000000000001"),
        ),
        (
            "a monitor ARN: a live ARN of this service that is not a scope",
            monitor.text("monitorArn"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc/vpc-01234567"),
        ),
    ];

    for (index, (case, arn)) in rows.iter().enumerate() {
        let answer = emulator
            .create_monitor(json!({
                "monitorName": format!("rejected-{index}"),
                "scopeArn": arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected 403, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("AccessDeniedException")
                || answer.text("__type").contains("AccessDeniedException"),
            "case {case:?}: expected AccessDeniedException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
        // The refusal left nothing behind.
        let read = emulator.get_monitor(&format!("rejected-{index}")).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store nothing, got {} with body {}",
            read.status, read.body
        );
    }

    // The same request with the stored scope's ARN succeeds, so the refusals above
    // are about the ARN and nothing else.
    let accepted = emulator
        .create_monitor(json!({
            "monitorName": "accepted",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "the stored scope's ARN must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn one_monitor_name_is_one_monitor() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for (case, name, token) in [
        ("without a clientToken", "dup-plain", None),
        ("with a clientToken", "dup-token", Some(client_token(21))),
    ] {
        let mut request = HashMap::from([
            ("monitorName".to_string(), json!(name)),
            ("scopeArn".to_string(), json!(scope_arn)),
            (
                "localResources".to_string(),
                json!([{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]),
            ),
        ]);
        if let Some(token) = token.clone() {
            request.insert("clientToken".to_string(), json!(token));
        }
        let first = emulator.create_monitor(json!(request)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {} with body {}",
            first.status, first.body
        );

        let second = emulator.create_monitor(json!(request)).await;
        if token.is_some() {
            // The same token and the same parameters: a replay of the monitor that
            // token minted, not a second monitor and not a conflict.
            assert_eq!(
                second.status, 200,
                "case {case:?}: expected the idempotent replay to succeed, got {} with body {}",
                second.status, second.body
            );
            assert_eq!(
                second.text("monitorArn"),
                first.text("monitorArn"),
                "case {case:?}: expected the first monitor's ARN, got {:?}",
                second.text("monitorArn")
            );
            assert_eq!(
                second.number("createdAt"),
                first.number("createdAt"),
                "case {case:?}: a replay reports the stored creation time, got {} then {}",
                first.number("createdAt"),
                second.number("createdAt")
            );
        } else {
            assert_eq!(
                second.status, 409,
                "case {case:?}: expected 409 for a second create of one name, got {} with body {}",
                second.status, second.body
            );
            assert!(
                second.error_type.contains("ConflictException")
                    || second.text("__type").contains("ConflictException"),
                "case {case:?}: expected ConflictException, got {:?} / body {}",
                second.error_type,
                second.body
            );
        }

        // Either way the store still holds exactly the monitor the first call made.
        let read = emulator.get_monitor(name).await;
        assert_eq!(
            read.text("monitorArn"),
            first.text("monitorArn"),
            "case {case:?}: expected the stored monitor to be the first one, got {:?}",
            read.text("monitorArn")
        );
    }

    // A name differing only in case is a different name: `ResourceName` is
    // case-sensitive, so this is a second monitor rather than a conflict.
    let upper = emulator
        .create_monitor(json!({
            "monitorName": "DUP-PLAIN",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        upper.status, 200,
        "a name differing in case is another monitor, got {} with body {}",
        upper.status, upper.body
    );
}

/// Every monitor `ListMonitors` reports, walked page by page with `max_results`:
/// the names in page order, and how many pages it took.
async fn page_through(emulator: &Emulator, filter: Option<&str>, max_results: i32) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = format!("?maxResults={max_results}");
        if let Some(status) = filter {
            query.push_str(&format!("&monitorStatus={status}"));
        }
        if let Some(cursor) = &token {
            query.push_str(&format!("&nextToken={cursor}"));
        }
        let answer = emulator.list_monitors(&query).await;
        assert_eq!(
            answer.status, 200,
            "maxResults {max_results}, filter {filter:?}, page {pages}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let items = answer
            .member("monitors")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= max_results as usize,
            "maxResults {max_results}, page {pages}: a page must hold at most {max_results} items, got {}",
            items.len()
        );
        for item in items {
            names.push(
                item.get("monitorName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        pages += 1;
        token = answer
            .member("nextToken")
            .as_str()
            .map(ToString::to_string);
        if token.is_none() {
            return (names, pages);
        }
        assert!(
            pages < 100,
            "maxResults {max_results}, filter {filter:?}: paging did not terminate after {pages} pages"
        );
    }
}

#[tokio::test]
async fn paging_through_list_monitors_yields_each_monitor_exactly_once() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let expected: Vec<String> = (1..=7).map(|index| format!("mon-{index:02}")).collect();
    for name in &expected {
        let created = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": format!("vpc-{name}") }]
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the monitor must be created, got {} with body {}",
            created.status, created.body
        );
    }

    // The page size dimension, including both published bounds of `maxResults`
    // (1..25) and a size that divides the store exactly.
    for max_results in [1, 2, 7, 25] {
        let (names, pages) = page_through(&emulator, None, max_results).await;
        assert_eq!(
            names, expected,
            "maxResults {max_results}: expected every monitor exactly once in name order, got {names:?}"
        );
        let want_pages = expected.len().div_ceil(max_results as usize);
        assert_eq!(
            pages, want_pages,
            "maxResults {max_results}: expected {want_pages} pages for {} monitors, got {pages}",
            expected.len()
        );
    }

    // An absent `maxResults` is a bounded page too, and still lists everything.
    let answer = emulator.list_monitors("").await;
    let listed: Vec<String> = answer
        .member("monitors")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("monitorName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed, expected,
        "with no maxResults every monitor is listed once, got {listed:?}"
    );
    assert!(
        answer.member("nextToken").is_null(),
        "the last page carries no nextToken, got {:?}",
        answer.member("nextToken")
    );

    // Every summary is the record's own ARN, name and status. Read after the
    // provisioning span, so both the summary and the get report the monitor's
    // terminal status and the comparison cannot straddle the transition.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let settled = emulator.list_monitors("").await;
    for item in settled
        .member("monitors")
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let name = item
            .get("monitorName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let read = emulator.get_monitor(&name).await;
        assert_eq!(
            item.get("monitorArn").and_then(Value::as_str),
            Some(read.text("monitorArn").as_str()),
            "case {name:?}: the summary's ARN is the stored one, got {item}"
        );
        assert_eq!(
            item.get("monitorStatus").and_then(Value::as_str),
            Some(read.text("monitorStatus").as_str()),
            "case {name:?}: the summary's status is the stored one, got {item}"
        );
    }
}

#[tokio::test]
async fn the_monitor_status_filter_lists_only_monitors_in_that_status() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let expected: Vec<String> = (1..=3).map(|index| format!("filtered-{index}")).collect();
    for name in &expected {
        emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
    }

    // Fresh monitors are PENDING, so exactly the PENDING filter lists them and
    // every other modelled status lists none.
    for status in MONITOR_STATUS_VALUES {
        let (names, _) = page_through(&emulator, Some(status), 2).await;
        let want: Vec<String> = if *status == "PENDING" {
            expected.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            names, want,
            "case {status:?} before the provisioning span: expected {want:?}, got {names:?}"
        );
    }

    tokio::time::sleep(PAST_PROVISIONING).await;

    // Once the clock owes them ACTIVE, the same filters answer the other way.
    for status in MONITOR_STATUS_VALUES {
        let (names, _) = page_through(&emulator, Some(status), 2).await;
        let want: Vec<String> = if *status == "ACTIVE" {
            expected.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            names, want,
            "case {status:?} after the provisioning span: expected {want:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn list_monitors_refuses_a_next_token_it_never_issued() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    for name in ["only-monitor", "second-monitor"] {
        emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
    }

    // A token this service issued resumes the walk; every other shape is refused —
    // including one that decodes cleanly but carries no cursor this listing minted,
    // which must not resume from an arbitrary lower bound.
    let issued = emulator.list_monitors("?maxResults=1").await;
    let good = issued.text("nextToken");
    assert!(
        !good.is_empty(),
        "the first page of two monitors must carry a nextToken, got body {}",
        issued.body
    );
    let resumed = emulator
        .list_monitors(&format!("?maxResults=1&nextToken={good}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "the issued token must be accepted, got {} with body {}",
        resumed.status, resumed.body
    );

    let rows = [
        ("not hex at all", "not-a-token".to_string()),
        ("two non-hex characters", "zz".to_string()),
        ("an odd-length hex string", "abc".to_string()),
        ("a single hex digit", "0".to_string()),
        ("percent-escaped braces", "%7B%7D".to_string()),
        // Hex that decodes to text, but to no cursor this listing minted.
        ("hex of a bare monitor name", hex::encode("second-monitor")),
        ("hex of arbitrary text", hex::encode("zzzz")),
        ("hex of the tag with no key", hex::encode("monitors/1:")),
        (
            "hex of another listing's tag",
            hex::encode("scopes/1:some-id"),
        ),
    ];

    for (case, token) in rows {
        let answer = emulator.list_monitors(&format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({token:?}): expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException")
                || answer.text("__type").contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn a_deleted_monitor_is_gone_for_the_next_reader() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let names: Vec<String> = (1..=4).map(|index| format!("doomed-{index}")).collect();
    for name in &names {
        emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
    }

    let mut remaining: Vec<String> = names.clone();
    for name in &names {
        let deleted = emulator.delete_monitor(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected 200 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        let read = emulator.get_monitor(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: a deleted monitor is gone for the next reader, got {} with body {}",
            read.status, read.body
        );

        remaining.retain(|held| held != name);
        let (listed, _) = page_through(&emulator, None, 2).await;
        assert_eq!(
            listed, remaining,
            "case {name:?}: expected {remaining:?} still listed, got {listed:?}"
        );

        // Deleting it again names nothing: the modelled 404.
        let twice = emulator.delete_monitor(name).await;
        assert_eq!(
            twice.status, 404,
            "case {name:?}: a second delete names nothing, got {} with body {}",
            twice.status, twice.body
        );
    }

    // The name is free again after the delete, and the monitor that takes it is a
    // new record rather than the deleted one.
    let recreated = emulator
        .create_monitor(json!({
            "monitorName": names[0],
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::Region", "identifier": "us-west-2" }]
        }))
        .await;
    assert_eq!(
        recreated.status, 200,
        "a deleted name can be created again, got {} with body {}",
        recreated.status, recreated.body
    );
    assert_eq!(
        recreated.member("localResources"),
        &json!([{ "type": "AWS::Region", "identifier": "us-west-2" }]),
        "the recreated monitor holds the new request's resources, got {:?}",
        recreated.member("localResources")
    );
}

#[tokio::test]
async fn every_local_resource_type_the_model_declares_round_trips() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    // The catalog's source: `MonitorLocalResourceType` in the Smithy model, as
    // `MONITOR_LOCAL_RESOURCE_TYPE_VALUES` lists it.
    for (index, resource_type) in LOCAL_RESOURCE_TYPES.iter().enumerate() {
        let name = format!("typed-{index}");
        let local_resources = json!([{ "type": resource_type, "identifier": format!("resource-{index}") }]);
        let created = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": local_resources
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {resource_type:?}: expected 200, got {} with body {}",
            created.status, created.body
        );
        let read = emulator.get_monitor(&name).await;
        assert_eq!(
            read.member("localResources"),
            &local_resources,
            "case {resource_type:?}: expected the stored localResources, got {:?}",
            read.member("localResources")
        );
    }
}

/// The store's own monitor ceiling (`state::MAX_MONITORS`), so the test asserts
/// the bound the code states rather than a number of its own. (The SCOPE bound is
/// AWS's published per-account/Region quota, pinned in `scope_quota.rs`.)
const MAX_MONITORS: usize = 100;

#[tokio::test]
async fn a_create_past_the_store_s_ceiling_is_refused() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    // Fill the monitor store exactly to its ceiling: every create up to it
    // succeeds.
    for index in 0..MAX_MONITORS {
        let created = emulator
            .create_monitor(json!({
                "monitorName": format!("bounded-{index:03}"),
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "monitor {index}: every create up to the ceiling succeeds, got {} with body {}",
            created.status, created.body
        );
    }
    let refused = emulator
        .create_monitor(json!({
            "monitorName": "one-too-many",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        refused.status, 402,
        "monitor {MAX_MONITORS} is past the ceiling: expected 402, got {} with body {}",
        refused.status, refused.body
    );
    assert!(
        refused.is_error("ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException, got {:?} / body {}",
        refused.error_type,
        refused.body
    );
    assert_eq!(
        refused.sentence(),
        format!("Maximum limit of [{MAX_MONITORS}] for resource [monitor] exceeded for account [{ACCOUNT_ID}]."),
        "a quota refusal names the limit, the resource and the account, got {:?}",
        refused.sentence()
    );
    // The refusal stored nothing, and deleting a monitor frees a slot again.
    let read = emulator.get_monitor("one-too-many").await;
    assert_eq!(
        read.status, 404,
        "a refused create must store nothing, got {} with body {}",
        read.status, read.body
    );
    let deleted = emulator.delete_monitor("bounded-000").await;
    assert_eq!(
        deleted.status, 200,
        "the delete that frees a slot must succeed, got {} with body {}",
        deleted.status, deleted.body
    );
    let accepted = emulator
        .create_monitor(json!({
            "monitorName": "one-too-many",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "a freed slot takes a new monitor, got {} with body {}",
        accepted.status, accepted.body
    );
}

/// `DeleteScope` removes the scope the id names from the store, so every later
/// reader of that id is answered the modelled `ResourceNotFoundException` and no
/// other scope is touched.
///
/// The table is the create table (`scope_rows`) — scopes differing in target
/// count, tags and `clientToken` — plus the status dimension: the first row is
/// deleted while the clock still has it `IN_PROGRESS`, the rest once the clock owes
/// them `SUCCEEDED`.
#[tokio::test]
async fn a_deleted_scope_is_gone_for_every_later_reader() {
    let emulator = Emulator::new();
    let rows = scope_rows();
    let mut live: Vec<(String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // One scope per Region, so every row's scope is live in one store at once
        // (the account's scope quota is per Region).
        let created = emulator
            .create_scope_in(extra_region(index), row.body.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the scope must be created, got {} with body {}",
            row.case, created.status, created.body
        );
        live.push((created.text("scopeId"), row.case.to_string()));
    }

    for (index, (scope_id, case)) in live.clone().into_iter().enumerate() {
        // Row 0 is deleted inside the provisioning span, so a scope is deletable in
        // the transient status too; from row 1 on the clock has settled every
        // remaining scope into its terminal status.
        if index == 1 {
            tokio::time::sleep(PAST_PROVISIONING).await;
        }
        let before = emulator.get_scope(&scope_id).await;
        let expected_status = if index == 0 {
            "IN_PROGRESS"
        } else {
            "SUCCEEDED"
        };
        assert_eq!(
            before.text("status"),
            expected_status,
            "case {case:?}: expected the scope to be {expected_status} before the delete, got {:?}",
            before.text("status")
        );

        let deleted = emulator.delete_scope(&scope_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteScopeOutput models no members, got body {}",
            deleted.body
        );

        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a deleted scope is gone for the next reader, got {} with body {}",
            read.status, read.body
        );
        assert!(
            read.error_type.contains("ResourceNotFoundException")
                || read.text("__type").contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / body {}",
            read.error_type,
            read.body
        );

        // Deleting it again names nothing: the same modelled 404.
        let twice = emulator.delete_scope(&scope_id).await;
        assert_eq!(
            twice.status, 404,
            "case {case:?}: a second delete names nothing, got {} with body {}",
            twice.status, twice.body
        );

        // Only the named scope went: every scope still live reads back as itself.
        live.retain(|(held, _)| held != &scope_id);
        for (held, held_case) in &live {
            let still = emulator.get_scope(held).await;
            assert_eq!(
                still.status, 200,
                "case {case:?}: {held_case:?} must survive the delete, got {} with body {}",
                still.status, still.body
            );
            assert_eq!(
                still.text("scopeId"),
                *held,
                "case {case:?}: {held_case:?} must read back as itself, got {:?}",
                still.text("scopeId")
            );
        }
    }
}

/// Deleting a WELL-FORMED id the store does not hold is the modelled 404, and the
/// delete leaves the store alone. An id of another form is refused for its shape
/// instead (`wire_status_and_error_classes.rs`).
#[tokio::test]
async fn delete_scope_on_an_id_the_store_does_not_hold_is_404() {
    let emulator = Emulator::new();
    // One real scope exists, so a 404 is about the id and not about an empty store.
    let created = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    assert_eq!(
        created.status, 200,
        "the anchor scope must be created, got {} with body {}",
        created.status, created.body
    );
    let live_id = created.text("scopeId");

    // A scope this store did mint and that is already gone, so the table covers
    // both "never existed" and "deleted earlier". It claims another Region's quota
    // slot, so the anchor scope above stays live.
    let deleted_once = emulator
        .create_scope_in(
            extra_region(0),
            json!({ "targets": targets(&[("210987654321", "eu-west-1")]) }),
        )
        .await;
    let deleted_id = deleted_once.text("scopeId");
    assert_eq!(
        emulator.delete_scope(&deleted_id).await.status,
        200,
        "the scope deleted up front must delete cleanly"
    );

    let rows = [
        (
            "a UUID-shaped id this store never minted",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "the id of a scope deleted earlier in this test",
            deleted_id.clone(),
        ),
        (
            "a UUID-shaped id whose version and variant nibbles are other values",
            "abcdef01-2345-1678-9abc-def012345678".to_string(),
        ),
        (
            "the same id in upper-case hex",
            "ABCDEF01-2345-1678-9ABC-DEF012345679".to_string(),
        ),
    ];

    for (case, scope_id) in &rows {
        let answer = emulator.delete_scope(scope_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException")
                || answer.text("__type").contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
        let still = emulator.get_scope(&live_id).await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the live scope must be untouched, got {} with body {}",
            still.status, still.body
        );
    }
}

/// What a deleted scope's records are worth afterwards: its ARN resolves to no
/// scope (so `CreateMonitor` refuses it, exactly as it refuses an ARN that never
/// named one), and its `clientToken` mints a NEW scope rather than answering with
/// the deleted one.
#[tokio::test]
async fn deleting_a_scope_retires_its_arn_and_frees_its_client_token() {
    for (index, pairs) in [
        vec![("123456789012", "us-east-1")],
        vec![("111111111111", "eu-west-1"), ("222222222222", "us-east-2")],
    ]
    .into_iter()
    .enumerate()
    {
        // A store per row: each row ends holding the scope it recreated, and the
        // account's Region slot is for exactly one scope.
        let emulator = Emulator::new();
        let case = format!("{} target(s)", pairs.len());
        let token = client_token(900 + index as u32);
        let body = json!({ "targets": targets(&pairs), "clientToken": token });

        let created = emulator.create_scope(body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the scope must be created, got {} with body {}",
            created.status, created.body
        );
        let scope_id = created.text("scopeId");
        let scope_arn = created.text("scopeArn");

        // A monitor under the scope, so the ARN is live before the delete — and a
        // scope a monitor still references is not deletable, so the monitor goes
        // first (`scope_referential_integrity.rs` is the suite about that refusal).
        let monitor_name = format!("orphan-{index}");
        let monitor = emulator
            .create_monitor(json!({
                "monitorName": monitor_name,
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            monitor.status, 200,
            "case {case:?}: the monitor under the live scope must be created, got {} with body {}",
            monitor.status, monitor.body
        );
        assert_eq!(
            emulator.delete_monitor(&monitor_name).await.status,
            200,
            "case {case:?}: the referencing monitor must delete before the scope does"
        );

        assert_eq!(
            emulator.delete_scope(&scope_id).await.status,
            200,
            "case {case:?}: the delete must succeed"
        );

        // The ARN now names no scope, so a create that needs a scope is refused
        // just as it is for an ARN this store never minted.
        let refused = emulator
            .create_monitor(json!({
                "monitorName": format!("after-{index}"),
                "scopeArn": scope_arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: a retired scopeArn is refused, got {} with body {}",
            refused.status, refused.body
        );

        // The same `clientToken` is no longer a replay of the deleted scope: it
        // mints a new one, with a new id.
        let recreated = emulator.create_scope(body.clone()).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the token must create a new scope, got {} with body {}",
            recreated.status, recreated.body
        );
        assert_ne!(
            recreated.text("scopeId"),
            scope_id,
            "case {case:?}: the new scope must not be the deleted one, got {:?}",
            recreated.text("scopeId")
        );
        let read = emulator.get_scope(&recreated.text("scopeId")).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the new scope must be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.member("targets"),
            &body["targets"],
            "case {case:?}: the new scope holds the request's targets, got {:?}",
            read.member("targets")
        );
    }
}

// ---------------------------------------------------------------------------
// The PERFORMANCE MONITOR plane: `CreatePerformanceMonitor`,
// `GetPerformanceMonitor`, `UpdatePerformanceMonitor`, `ListPerformanceMonitors`,
// `DeletePerformanceMonitor`, the query family that hangs off a performance monitor
// and `GetMonitorScopeSuggestions`.
//
// The rules pinned below, each over a TABLE whose rows differ along what the rule
// generalizes over (target count, duplicate entries, tags, a named scope, the page
// size, the status the clock has reached):
//
// * A create stores the record and answers FROM it, so the reader answers the same
//   record; `monitorType` is read off the resources the monitor tracks, and
//   `targets` is held as a SET keyed by `type` + `identifier`.
// * A performance monitor reaches its terminal status on the clock alone, and a
//   resource AWS reports in a non-terminal status is observable in it first.
// * An update edits `targets` as a set, moves `modifiedAt` exactly when the record
//   changed, and a refused update leaves the record as it was.
// * A listing walks the store exactly once per cursor, under this listing's own
//   page token, and filters on the two members the model binds as query filters.
// * A delete is observable by every later reader, and a monitor and a performance
//   monitor of one name are two resources with two ARNs.
// ---------------------------------------------------------------------------

/// The one value `PerformanceMonitorTargetType` declares, from the model
/// (`src/ops/types/shared_types.rs`: `PERFORMANCE_MONITOR_TARGET_TYPE_VALUES`).
const PERFORMANCE_TARGET_TYPE: &str = "AWS::EC2::VPC";

/// The one value `PerformanceMonitorType` declares
/// (`PERFORMANCE_MONITOR_TYPE_VALUES`) — the type a monitor tracking
/// [`PERFORMANCE_TARGET_TYPE`] resources is.
const PERFORMANCE_MONITOR_TYPE: &str = "VPC";

/// The one value `PerformanceMonitorQueryType` declares
/// (`PERFORMANCE_MONITOR_QUERY_TYPE_VALUES`).
const PERFORMANCE_QUERY_TYPE: &str = "TOP_PATHS";

/// How long a poll for a status the clock owes keeps looking before it gives up.
/// Deliberately far longer than the span a fresh resource settles after, so a
/// loaded host does not decide the outcome.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one round of such a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A `PerformanceMonitorTarget` for the `index`-th VPC of a test. `identifier` is an
/// ARN of 20..128 characters and `type` the one target type the model declares
/// (`smithy_query(query="shape",shape="PerformanceMonitorTarget")`).
fn vpc_target(index: usize) -> Value {
    json!({
        "type": PERFORMANCE_TARGET_TYPE,
        "identifier": format!("arn:aws:ec2:{REGION}:123456789012:vpc/vpc-{index:017x}"),
    })
}

/// `count` distinct VPC targets, for the count dimension of the tables below.
fn vpc_targets(count: usize) -> Value {
    Value::Array((0..count).map(vpc_target).collect())
}

/// A query-string value with every character an ARN carries percent-encoded, as an
/// SDK puts a `monitorArn` on the wire.
fn query_encoded(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

/// Look again until the performance monitor reports `ACTIVE`, so a test that starts
/// from a settled monitor waits for the CONDITION rather than for a span of the
/// clock. Answers the settled read.
async fn await_performance_monitor_active(
    emulator: &Emulator,
    name: &str,
    case: &str,
) -> Answer {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = emulator.get_performance_monitor(name).await;
        if read.text("monitorStatus") == "ACTIVE" {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the performance monitor never reached ACTIVE; last status {:?} \
             with body {}",
            read.text("monitorStatus"),
            read.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The same for a query of the performance-monitor family: look again until it
/// reports `SUCCEEDED`.
async fn await_query_succeeded(
    emulator: &Emulator,
    name: &str,
    query_id: &str,
    case: &str,
) -> Answer {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = emulator.performance_query_status(name, query_id).await;
        if read.text("status") == "SUCCEEDED" {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the query never reached SUCCEEDED; last status {:?} with body {}",
            read.text("status"),
            read.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the performance-monitor create-and-read-back table.
struct PerformanceRow {
    case: &'static str,
    name: &'static str,
    /// The `targets` the request carries.
    targets: Value,
    /// The `targets` the record is expected to hold: the request's, as a set.
    stored: Value,
    tags: Option<Value>,
    /// Whether the create names the scope the store holds (`scopeArn` is optional
    /// on this operation).
    with_scope: bool,
}

fn performance_rows() -> Vec<PerformanceRow> {
    vec![
        PerformanceRow {
            case: "one target, no tags, no scope",
            name: "perf-single",
            targets: vpc_targets(1),
            stored: vpc_targets(1),
            tags: None,
            with_scope: false,
        },
        PerformanceRow {
            case: "three targets under a named scope",
            name: "perf-three.under_scope-1",
            targets: vpc_targets(3),
            stored: vpc_targets(3),
            tags: None,
            with_scope: true,
        },
        PerformanceRow {
            case: "25 targets and a tag set",
            name: "perf-many",
            targets: vpc_targets(25),
            stored: vpc_targets(25),
            tags: Some(json!({ "env": "prod", "owner": "net-team" })),
            with_scope: false,
        },
        PerformanceRow {
            // `targets` is a set keyed by `type` + `identifier`, so a repeated
            // entry is stored once and in the order it was first named.
            case: "a target named twice is stored once",
            name: "perf-duplicate",
            targets: json!([vpc_target(1), vpc_target(2), vpc_target(1)]),
            stored: json!([vpc_target(1), vpc_target(2)]),
            tags: None,
            with_scope: false,
        },
        PerformanceRow {
            case: "the 100 targets PerformanceMonitorTargets allows at most",
            name: "perf-at-the-bound",
            targets: vpc_targets(100),
            stored: vpc_targets(100),
            tags: None,
            with_scope: false,
        },
    ]
}

#[tokio::test]
async fn creating_a_performance_monitor_answers_the_record_the_reader_answers() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for (index, row) in performance_rows().into_iter().enumerate() {
        let mut body = json!({
            "monitorName": row.name,
            "targets": row.targets,
            "clientToken": client_token(500 + index as u32),
        });
        if let Some(tags) = &row.tags {
            body["tags"] = tags.clone();
        }
        if row.with_scope {
            body["scopeArn"] = json!(scope_arn);
        }

        let created = emulator.create_performance_monitor(body.clone()).await;
        // `@http` names 201 for this create, so that is the status a success
        // answers with.
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} with body {}",
            row.case, created.status, created.body
        );
        assert_eq!(
            created.text("monitorArn"),
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:performance-monitor/{}",
                row.name
            ),
            "case {:?}: expected the ARN of this resource type in the request's Region, got {:?}",
            row.case,
            created.text("monitorArn")
        );
        assert_eq!(
            created.text("monitorName"),
            row.name,
            "case {:?}: expected the submitted name, got {:?}",
            row.case,
            created.text("monitorName")
        );
        assert_eq!(
            created.text("monitorStatus"),
            "PENDING",
            "case {:?}: a fresh performance monitor is PENDING, got {:?}",
            row.case,
            created.text("monitorStatus")
        );
        // "The type of resource that you monitor determines the type of the
        // performance monitor": every target of these rows is a VPC.
        assert_eq!(
            created.text("monitorType"),
            PERFORMANCE_MONITOR_TYPE,
            "case {:?}: expected the type the targets determine, got {:?}",
            row.case,
            created.text("monitorType")
        );
        assert_eq!(
            created.member("targets"),
            &row.stored,
            "case {:?}: expected the submitted targets as a set, got {:?}",
            row.case,
            created.member("targets")
        );
        assert_eq!(
            created.member("tags"),
            row.tags.as_ref().unwrap_or(&Value::Null),
            "case {:?}: a record reports tags only when it has any, got {:?}",
            row.case,
            created.member("tags")
        );
        let created_at = created.number("createdAt");
        assert!(
            created_at > 0.0,
            "case {:?}: expected a real creation timestamp, got {created_at}",
            row.case
        );
        assert_eq!(
            created_at,
            created.number("modifiedAt"),
            "case {:?}: a monitor nothing has changed yet has modifiedAt == createdAt, got {} \
             and {}",
            row.case,
            created_at,
            created.number("modifiedAt")
        );

        // The reader answers the record the create answered from: every member the
        // clock cannot move is identical.
        let read = emulator.get_performance_monitor(row.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the stored monitor, got {} with body {}",
            row.case, read.status, read.body
        );
        for member in [
            "monitorArn",
            "monitorName",
            "monitorType",
            "targets",
            "tags",
            "createdAt",
        ] {
            assert_eq!(
                read.member(member),
                created.member(member),
                "case {:?}: expected the stored {member}, got {:?} after the create answered {:?}",
                row.case,
                read.member(member),
                created.member(member)
            );
        }
        // The only member the clock may have moved is the status, and only onto the
        // terminal one it owes — with `modifiedAt` moving exactly when it does.
        match read.text("monitorStatus").as_str() {
            "PENDING" => assert_eq!(
                read.number("modifiedAt"),
                created_at,
                "case {:?}: a still-PENDING monitor has modifiedAt == createdAt, got {}",
                row.case,
                read.number("modifiedAt")
            ),
            "ACTIVE" => assert!(
                read.number("modifiedAt") > created_at,
                "case {:?}: reaching ACTIVE moves modifiedAt past createdAt {created_at}, got {}",
                row.case,
                read.number("modifiedAt")
            ),
            other => panic!(
                "case {:?}: a performance monitor is PENDING and then ACTIVE, got {other:?}",
                row.case
            ),
        }

        // The `@idempotent` replay answers with the monitor the token minted
        // rather than a conflict, and the same token carrying a DIFFERENT change
        // is the modelled 409.
        let replayed = emulator.create_performance_monitor(body.clone()).await;
        assert_eq!(
            replayed.status, 201,
            "case {:?}: the replay must answer the minted monitor, got {} with body {}",
            row.case, replayed.status, replayed.body
        );
        assert_eq!(
            replayed.member("targets"),
            &row.stored,
            "case {:?}: the replay answers the stored targets, got {:?}",
            row.case,
            replayed.member("targets")
        );
        let mut changed = body.clone();
        changed["targets"] = vpc_targets(2);
        let conflict = emulator.create_performance_monitor(changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {:?}: a token reused for other parameters is a conflict, got {} with body {}",
            row.case, conflict.status, conflict.body
        );
        assert_eq!(
            conflict.class(),
            "ConflictException",
            "case {:?}: expected ConflictException, got {:?} with body {}",
            row.case,
            conflict.class(),
            conflict.body
        );

        // A second create of one name is a conflict however the request is
        // otherwise formed (no `clientToken` here, so this is not a replay).
        let duplicate = emulator
            .create_performance_monitor(json!({
                "monitorName": row.name,
                "targets": vpc_targets(1),
            }))
            .await;
        assert_eq!(
            duplicate.status, 409,
            "case {:?}: a second create of one name conflicts, got {} with body {}",
            row.case, duplicate.status, duplicate.body
        );
    }
}

#[tokio::test]
async fn a_performance_monitor_reaches_active_on_the_clock_alone() {
    // The rows differ in whether the monitor names a scope and in how many
    // resources it tracks: the transition is the clock's, so it holds for both.
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let rows = [
        ("no scope, one target", "settling-perf-1", 1usize, false),
        ("a named scope, four targets", "settling-perf-2", 4, true),
    ];

    for (case, name, target_count, with_scope) in rows {
        let mut body = json!({ "monitorName": name, "targets": vpc_targets(target_count) });
        if with_scope {
            body["scopeArn"] = json!(scope_arn);
        }
        let created = emulator.create_performance_monitor(body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the monitor must be created, got {} with body {}",
            created.status, created.body
        );
        // A caller acting straight after the create still observes the transient
        // status the model documents for it.
        assert_eq!(
            created.text("monitorStatus"),
            "PENDING",
            "case {case:?}: a fresh performance monitor is PENDING, got {:?}",
            created.text("monitorStatus")
        );
        let created_at = created.number("createdAt");

        let settled = await_performance_monitor_active(&emulator, name, case).await;
        assert_eq!(
            settled.number("createdAt"),
            created_at,
            "case {case:?}: createdAt never moves, got {} then {}",
            created_at,
            settled.number("createdAt")
        );
        assert!(
            settled.number("modifiedAt") > created_at,
            "case {case:?}: reaching ACTIVE moves modifiedAt past createdAt {created_at}, got {}",
            settled.number("modifiedAt")
        );

        // A terminal status does not move again, and a read changes nothing a later
        // reader would see.
        let again = emulator.get_performance_monitor(name).await;
        assert_eq!(
            again.body, settled.body,
            "case {case:?}: two reads of a settled monitor answer one body, got {} then {}",
            settled.body, again.body
        );
    }
}

/// One row of the performance-monitor update table: the request, the targets the
/// record must hold afterwards, and whether the record changed (which is what
/// decides whether `modifiedAt` moves).
struct UpdateRow {
    case: &'static str,
    body: Value,
    expected: Value,
    changed: bool,
}

#[tokio::test]
async fn updating_a_performance_monitor_edits_its_targets_as_a_set() {
    let emulator = Emulator::new();
    let name = "perf-under-update";
    let created = emulator
        .create_performance_monitor(json!({
            "monitorName": name,
            "targets": json!([vpc_target(1), vpc_target(2)]),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the monitor must be created, got {} with body {}",
        created.status, created.body
    );
    // Settled first, so a later `modifiedAt` move is the update's and not the
    // clock's.
    let settled = await_performance_monitor_active(&emulator, name, "the monitor under test").await;
    let created_at = settled.number("createdAt");
    let mut modified_at = settled.number("modifiedAt");

    let rows = vec![
        UpdateRow {
            case: "an addition the monitor does not hold is appended",
            body: json!({ "targetsToAdd": [vpc_target(3)] }),
            expected: json!([vpc_target(1), vpc_target(2), vpc_target(3)]),
            changed: true,
        },
        UpdateRow {
            case: "adding a target the monitor already holds is a no-op",
            body: json!({ "targetsToAdd": [vpc_target(2)] }),
            expected: json!([vpc_target(1), vpc_target(2), vpc_target(3)]),
            changed: false,
        },
        UpdateRow {
            case: "a removal drops exactly the target it names",
            body: json!({ "targetsToRemove": [vpc_target(2)] }),
            expected: json!([vpc_target(1), vpc_target(3)]),
            changed: true,
        },
        UpdateRow {
            case: "removing a target the monitor does not hold is a no-op",
            body: json!({ "targetsToRemove": [vpc_target(9)] }),
            expected: json!([vpc_target(1), vpc_target(3)]),
            changed: false,
        },
        UpdateRow {
            case: "removals land before additions in one request",
            body: json!({
                "targetsToRemove": [vpc_target(1)],
                "targetsToAdd": [vpc_target(4), vpc_target(4)],
            }),
            expected: json!([vpc_target(3), vpc_target(4)]),
            changed: true,
        },
        UpdateRow {
            case: "a request naming neither list changes nothing",
            body: json!({}),
            expected: json!([vpc_target(3), vpc_target(4)]),
            changed: false,
        },
    ];

    for row in rows {
        let updated = emulator.update_performance_monitor(name, row.body).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200, got {} with body {}",
            row.case, updated.status, updated.body
        );
        assert_eq!(
            updated.member("targets"),
            &row.expected,
            "case {:?}: expected {}, got {:?}",
            row.case,
            row.expected,
            updated.member("targets")
        );
        // The update answers the record the reader answers.
        let read = emulator.get_performance_monitor(name).await;
        assert_eq!(
            read.member("targets"),
            &row.expected,
            "case {:?}: expected the stored targets, got {:?}",
            row.case,
            read.member("targets")
        );
        // Nothing else about the record moved.
        assert_eq!(
            read.number("createdAt"),
            created_at,
            "case {:?}: createdAt never moves, got {}",
            row.case,
            read.number("createdAt")
        );
        assert_eq!(
            read.text("monitorType"),
            PERFORMANCE_MONITOR_TYPE,
            "case {:?}: the type still follows the targets, got {:?}",
            row.case,
            read.text("monitorType")
        );
        let after = read.number("modifiedAt");
        if row.changed {
            assert!(
                after > modified_at,
                "case {:?}: an update that changed the record moves modifiedAt past \
                 {modified_at}, got {after}",
                row.case
            );
        } else {
            assert_eq!(
                after, modified_at,
                "case {:?}: an update that changed nothing leaves modifiedAt at {modified_at}, \
                 got {after}",
                row.case
            );
        }
        modified_at = after;
    }
}

#[tokio::test]
async fn a_refused_performance_monitor_update_leaves_the_record_as_it_was() {
    let emulator = Emulator::new();
    let name = "perf-refused-updates";
    let created = emulator
        .create_performance_monitor(json!({
            "monitorName": name,
            "targets": json!([vpc_target(1), vpc_target(2)]),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the monitor must be created, got {} with body {}",
        created.status, created.body
    );
    let before = await_performance_monitor_active(&emulator, name, "the monitor under test").await;

    // (case, body, status, class) — one row per condition an update is refused
    // under, spanning both classes the model declares for them.
    let rows = vec![
        (
            "a target named in both lists",
            json!({
                "targetsToAdd": [vpc_target(1)],
                "targetsToRemove": [vpc_target(1)],
            }),
            400,
            "ValidationException",
        ),
        (
            // `targets` carries `@length(1, 100)`, so a monitor tracking nothing is
            // not one a create could have minted.
            "a removal of every target the monitor holds",
            json!({ "targetsToRemove": [vpc_target(1), vpc_target(2)] }),
            400,
            "ValidationException",
        ),
        (
            "an update of a name the store holds no performance monitor under",
            json!({ "targetsToAdd": [vpc_target(5)] }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, body, status, class) in rows {
        let target_name = if status == 404 { "perf-never-created" } else { name };
        let refused = emulator
            .update_performance_monitor(target_name, body)
            .await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected {status}, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.class(),
            class,
            "case {case:?}: expected {class}, got {:?} with body {}",
            refused.class(),
            refused.body
        );
        let read = emulator.get_performance_monitor(name).await;
        assert_eq!(
            read.member("targets"),
            before.member("targets"),
            "case {case:?}: a refused update leaves the targets as they were, got {:?}",
            read.member("targets")
        );
        assert_eq!(
            read.number("modifiedAt"),
            before.number("modifiedAt"),
            "case {case:?}: a refused update does not move modifiedAt, got {}",
            read.number("modifiedAt")
        );
    }

    // The missing monitor is reported by the ARN it would carry, in this service's
    // own missing-monitor wording.
    let missing = emulator
        .update_performance_monitor("perf-never-created", json!({}))
        .await;
    assert_eq!(
        missing.sentence(),
        format!(
            "Monitor [arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:performance-monitor/\
             perf-never-created] doesn't exist"
        ),
        "the refusal names the absent performance monitor by its own ARN, got {:?}",
        missing.sentence()
    );

    // An `@idempotent` replay neither applies twice nor moves `modifiedAt`, and the
    // same token carrying a DIFFERENT change is the modelled conflict.
    let token = client_token(601);
    let applied = emulator
        .update_performance_monitor(
            name,
            json!({ "targetsToAdd": [vpc_target(7)], "clientToken": token }),
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "the first update must be applied, got {} with body {}",
        applied.status, applied.body
    );
    let replayed = emulator
        .update_performance_monitor(
            name,
            json!({ "targetsToAdd": [vpc_target(7)], "clientToken": token }),
        )
        .await;
    assert_eq!(
        replayed.body, applied.body,
        "a replayed update answers the record as it stands, got {} after {}",
        replayed.body, applied.body
    );
    let reused = emulator
        .update_performance_monitor(
            name,
            json!({ "targetsToAdd": [vpc_target(8)], "clientToken": token }),
        )
        .await;
    assert_eq!(
        reused.status, 409,
        "a token reused for a different change is a conflict, got {} with body {}",
        reused.status, reused.body
    );
    assert_eq!(
        reused.class(),
        "ConflictException",
        "expected ConflictException, got {:?} with body {}",
        reused.class(),
        reused.body
    );
    let read = emulator.get_performance_monitor(name).await;
    assert_eq!(
        read.member("targets"),
        applied.member("targets"),
        "the refused reuse left the applied targets in place, got {:?}",
        read.member("targets")
    );
}

/// Every performance monitor `ListPerformanceMonitors` reports, walked page by page
/// with `max_results`: the names in page order, and how many pages it took.
async fn page_through_performance(
    emulator: &Emulator,
    filter: &str,
    max_results: i32,
) -> (Vec<String>, usize) {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = format!("?maxResults={max_results}{filter}");
        if let Some(cursor) = &token {
            query.push_str(&format!("&nextToken={cursor}"));
        }
        let answer = emulator.list_performance_monitors(&query).await;
        assert_eq!(
            answer.status, 200,
            "maxResults {max_results}, filter {filter:?}, page {pages}: expected 200, got {} \
             with body {}",
            answer.status, answer.body
        );
        let items = answer
            .member("monitors")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= max_results as usize,
            "maxResults {max_results}, page {pages}: a page holds at most {max_results} items, \
             got {}",
            items.len()
        );
        for item in items {
            names.push(
                item.get("monitorName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        pages += 1;
        token = answer
            .member("nextToken")
            .as_str()
            .map(ToString::to_string);
        if token.is_none() {
            return (names, pages);
        }
        assert!(
            pages < 100,
            "maxResults {max_results}, filter {filter:?}: paging did not terminate after \
             {pages} pages"
        );
    }
}

#[tokio::test]
async fn paging_through_list_performance_monitors_yields_each_monitor_exactly_once() {
    let emulator = Emulator::new();
    let expected: Vec<String> = (1..=7).map(|index| format!("perf-{index:02}")).collect();
    for (index, name) in expected.iter().enumerate() {
        let created = emulator
            .create_performance_monitor(json!({
                "monitorName": name,
                "targets": [vpc_target(index)],
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: the monitor must be created, got {} with body {}",
            created.status, created.body
        );
    }
    // Every monitor is settled, so the status filter below is asserted against a
    // store whose statuses the clock is done with.
    for name in &expected {
        await_performance_monitor_active(&emulator, name, name).await;
    }

    // The page size dimension, including both published bounds of `maxResults`
    // (1..25) and a size that divides the store exactly.
    for max_results in [1, 2, 7, 25] {
        let (names, pages) = page_through_performance(&emulator, "", max_results).await;
        assert_eq!(
            names, expected,
            "maxResults {max_results}: expected every monitor exactly once in name order, got \
             {names:?}"
        );
        let want_pages = expected.len().div_ceil(max_results as usize);
        assert_eq!(
            pages, want_pages,
            "maxResults {max_results}: expected {want_pages} pages for {} monitors, got {pages}",
            expected.len()
        );
    }

    // An absent `maxResults` is a bounded page too, and still lists everything.
    let all = emulator.list_performance_monitors("").await;
    let listed: Vec<String> = all
        .member("monitors")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("monitorName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed, expected,
        "an absent maxResults lists every monitor, got {listed:?}"
    );
    // A summary carries the resources the monitor tracks and the type they
    // determine, read off the same record the reader answers.
    let first = all.member("monitors")[0].clone();
    assert_eq!(
        first.get("monitorType").and_then(Value::as_str),
        Some(PERFORMANCE_MONITOR_TYPE),
        "a summary reports the type the targets determine, got {first}"
    );
    assert_eq!(
        first.get("targets"),
        Some(&json!([vpc_target(0)])),
        "a summary reports the targets the record holds, got {first}"
    );

    // The two `@httpQuery` filters, over every value the model declares for each:
    // a status no monitor holds matches nothing, and the one type they all are
    // matches all of them.
    for status in MONITOR_STATUS_VALUES {
        let (names, _) =
            page_through_performance(&emulator, &format!("&monitorStatus={status}"), 25).await;
        let want: Vec<String> = if *status == "ACTIVE" {
            expected.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            names, want,
            "filter monitorStatus={status}: expected {want:?}, got {names:?}"
        );
    }
    let (typed, _) = page_through_performance(
        &emulator,
        &format!("&monitorType={PERFORMANCE_MONITOR_TYPE}"),
        25,
    )
    .await;
    assert_eq!(
        typed, expected,
        "filter monitorType={PERFORMANCE_MONITOR_TYPE}: expected every monitor, got {typed:?}"
    );

    // A cursor this listing did not mint is refused rather than served as "start
    // from the beginning" — including one ANOTHER listing of this service minted,
    // because a page token carries the listing it belongs to.
    let monitors_token = {
        let scope_arn = emulator.scope_for_monitors().await;
        for index in 0..2 {
            emulator
                .create_monitor(json!({
                    "monitorName": format!("plain-{index}"),
                    "scopeArn": scope_arn,
                    "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }],
                }))
                .await;
        }
        let page = emulator.list_monitors("?maxResults=1").await;
        page.text("nextToken")
    };
    assert!(
        !monitors_token.is_empty(),
        "the ListMonitors page must issue a cursor to reuse here"
    );
    for (case, token) in [
        ("a token nothing issued", "not-a-token".to_string()),
        ("hex that decodes to another listing's tag", monitors_token),
    ] {
        let refused = emulator
            .list_performance_monitors(&format!("?nextToken={token}"))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.sentence(),
            "NextToken provided isn't valid.",
            "case {case:?}: expected the one unissued-cursor sentence, got {:?}",
            refused.sentence()
        );
    }
}

#[tokio::test]
async fn deleting_a_performance_monitor_is_observable_by_every_later_reader() {
    let emulator = Emulator::new();
    // The rows differ in whether the monitor has a query started under it, so the
    // delete is asserted over a monitor with dependants and one without.
    for (index, (case, name, with_query)) in [
        ("no query started", "perf-doomed-1", false),
        ("a query started under it", "perf-doomed-2", true),
    ]
    .into_iter()
    .enumerate()
    {
        // Each row's own `clientToken`, so a row is never a replay of the row before
        // it; the recreate below deliberately reuses this row's.
        let token = client_token(700 + index as u32);
        let created = emulator
            .create_performance_monitor(json!({
                "monitorName": name,
                "targets": [vpc_target(1)],
                "clientToken": token,
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the monitor must be created, got {} with body {}",
            created.status, created.body
        );
        let arn = created.text("monitorArn");

        let query_id = if with_query {
            let started = emulator
                .start_performance_query(
                    name,
                    json!({
                        "startTime": "2026-10-22T19:07:07Z",
                        "endTime": "2026-10-22T20:07:07Z",
                        "queryType": PERFORMANCE_QUERY_TYPE,
                    }),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: the query must start, got {} with body {}",
                started.status, started.body
            );
            Some(started.text("queryId"))
        } else {
            None
        };

        let deleted = emulator.delete_performance_monitor(name).await;
        // `@http` names 204 for this delete, and the output declares no members.
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected 204, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {case:?}: the output declares no members, so the answer carries none, got {}",
            deleted.body
        );

        // Every later reader sees the delete: the record, the listing, the tag
        // plane that addressed it by ARN, and the query that hung off it.
        let read = emulator.get_performance_monitor(name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the reader must answer 404, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.sentence(),
            format!("Monitor [{arn}] doesn't exist"),
            "case {case:?}: expected the missing-monitor sentence, got {:?}",
            read.sentence()
        );
        let repeated = emulator.delete_performance_monitor(name).await;
        assert_eq!(
            repeated.status, 404,
            "case {case:?}: the delete repeated is 404, got {} with body {}",
            repeated.status, repeated.body
        );
        assert_eq!(
            repeated.body, read.body,
            "case {case:?}: one condition carries one body, got {} and {}",
            repeated.body, read.body
        );
        let listed = emulator.list_performance_monitors("").await;
        let names: Vec<String> = listed
            .member("monitors")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item.get("monitorName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !names.contains(&name.to_string()),
            "case {case:?}: the listing must not report a deleted monitor, got {names:?}"
        );
        if let Some(query_id) = query_id {
            let status = emulator.performance_query_status(name, &query_id).await;
            assert_eq!(
                status.status, 404,
                "case {case:?}: a query under a deleted monitor is unreachable, got {} with \
                 body {}",
                status.status, status.body
            );
        }

        // The same `clientToken` is no longer a replay of the deleted monitor: it
        // mints a new one.
        let recreated = emulator
            .create_performance_monitor(json!({
                "monitorName": name,
                "targets": [vpc_target(1)],
                "clientToken": token,
            }))
            .await;
        assert_eq!(
            recreated.status, 201,
            "case {case:?}: the token must create a new monitor, got {} with body {}",
            recreated.status, recreated.body
        );
    }
}

#[tokio::test]
async fn a_monitor_and_a_performance_monitor_of_one_name_are_two_resources() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let name = "shared-name";

    let monitor = emulator
        .create_monitor(json!({
            "monitorName": name,
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }],
        }))
        .await;
    assert_eq!(
        monitor.status, 200,
        "the monitor must be created, got {} with body {}",
        monitor.status, monitor.body
    );
    let performance = emulator
        .create_performance_monitor(json!({
            "monitorName": name,
            "targets": [vpc_target(1)],
        }))
        .await;
    assert_eq!(
        performance.status, 201,
        "a performance monitor of the same name must be created, got {} with body {}",
        performance.status, performance.body
    );
    assert_ne!(
        monitor.text("monitorArn"),
        performance.text("monitorArn"),
        "two resources carry two ARNs, got {:?} twice",
        monitor.text("monitorArn")
    );

    // Deleting one leaves the other, and each reader answers its own resource.
    assert_eq!(
        emulator.delete_performance_monitor(name).await.status,
        204,
        "the performance monitor must delete"
    );
    assert_eq!(
        emulator.get_monitor(name).await.status,
        200,
        "the monitor is untouched by the other resource's delete"
    );
    assert_eq!(
        emulator.get_performance_monitor(name).await.status,
        404,
        "the deleted performance monitor is gone"
    );
}

#[tokio::test]
async fn a_performance_monitor_query_runs_on_the_clock_and_reports_its_result_set() {
    let emulator = Emulator::new();
    let name = "perf-query-plane";
    let created = emulator
        .create_performance_monitor(json!({
            "monitorName": name,
            "targets": [vpc_target(1), vpc_target(2)],
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the monitor must be created, got {} with body {}",
        created.status, created.body
    );
    await_performance_monitor_active(&emulator, name, "the monitor the queries hang off").await;

    // The rows differ in the members a start may carry: the filters and the limit
    // are both optional, and `limit` is asserted at both published bounds (1..500).
    let rows = vec![
        ("no filters and no limit", json!({})),
        (
            "one filter",
            json!({
                "filterParameters": [{
                    "field": "localVpcId",
                    "operator": "EQUALS",
                    "values": ["vpc-0123456789abcdef0"],
                }],
            }),
        ),
        (
            "several filters and the smallest limit",
            json!({
                "limit": 1,
                "filterParameters": [
                    { "field": "localAz", "operator": "IN", "values": ["usw2-az1", "usw2-az2"] },
                    { "field": "remoteIp", "operator": "CONTAINS", "values": ["10."] },
                ],
            }),
        ),
        ("the largest limit", json!({ "limit": 500 })),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, extra) in rows {
        let mut body = json!({
            "startTime": "2026-10-22T19:07:07Z",
            "endTime": "2026-10-22T20:07:07Z",
            "queryType": PERFORMANCE_QUERY_TYPE,
        });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let started = emulator.start_performance_query(name, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            started.status, started.body
        );
        let query_id = started.text("queryId");
        assert!(
            is_uuid_shaped(&query_id),
            "case {case:?}: expected a minted query id, got {query_id:?}"
        );
        assert!(
            ids.insert(query_id.clone()),
            "case {case:?}: every start mints its own id, got {query_id:?} twice"
        );

        // A caller polling straight after the start sees a status the query
        // reaches on the clock alone, and its results only once it has SUCCEEDED.
        let polled = emulator.performance_query_status(name, &query_id).await;
        assert_eq!(
            polled.status, 200,
            "case {case:?}: expected 200 from the status poll, got {} with body {}",
            polled.status, polled.body
        );
        assert!(
            ["QUEUED", "RUNNING", "SUCCEEDED"].contains(&polled.text("status").as_str()),
            "case {case:?}: expected a status of the running lifecycle, got {:?}",
            polled.text("status")
        );
        if polled.text("status") != "SUCCEEDED" {
            let early = emulator
                .performance_query_results(name, &query_id, "")
                .await;
            assert_eq!(
                early.status, 400,
                "case {case:?}: results before SUCCEEDED are refused, got {} with body {}",
                early.status, early.body
            );
            assert_eq!(
                early.class(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                early.class()
            );
        }

        await_query_succeeded(&emulator, name, &query_id, case).await;
        let results = emulator
            .performance_query_results(name, &query_id, "")
            .await;
        assert_eq!(
            results.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            results.status, results.body
        );
        // The columns and the rows are reported together off one result set: this
        // emulator observes no network flow, so the set is empty — no rows, and so
        // no columns describing them.
        assert_eq!(
            results.member("data"),
            &json!([]),
            "case {case:?}: expected no rows, got {:?}",
            results.member("data")
        );
        assert_eq!(
            results.member("fields"),
            &json!([]),
            "case {case:?}: a result set with no rows carries no columns, got {:?}",
            results.member("fields")
        );
        assert_eq!(
            results.member("nextToken"),
            &Value::Null,
            "case {case:?}: a single page issues no resume token, got {:?}",
            results.member("nextToken")
        );
        // A read changes nothing a later reader would see.
        let again = emulator
            .performance_query_results(name, &query_id, "")
            .await;
        assert_eq!(
            again.body, results.body,
            "case {case:?}: two reads answer one body, got {} then {}",
            results.body, again.body
        );
        // A terminal query is not moved by a stop: it keeps the outcome it reached.
        let stopped = emulator.stop_performance_query(name, &query_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: stopping a finished query succeeds, got {} with body {}",
            stopped.status, stopped.body
        );
        assert_eq!(
            emulator
                .performance_query_status(name, &query_id)
                .await
                .text("status"),
            "SUCCEEDED",
            "case {case:?}: a terminal status does not move again"
        );
    }

    // A stop while the query is still moving is the terminal CANCELED, and a
    // canceled query never reports results.
    let started = emulator
        .start_performance_query(
            name,
            json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "queryType": PERFORMANCE_QUERY_TYPE,
            }),
        )
        .await;
    let query_id = started.text("queryId");
    let stopped = emulator.stop_performance_query(name, &query_id).await;
    assert_eq!(
        stopped.status, 200,
        "the stop must succeed, got {} with body {}",
        stopped.status, stopped.body
    );
    let after_stop = emulator
        .performance_query_status(name, &query_id)
        .await
        .text("status");
    assert!(
        ["CANCELED", "SUCCEEDED"].contains(&after_stop.as_str()),
        "a stopped query is terminal, got {after_stop:?}"
    );
    let results = emulator
        .performance_query_results(name, &query_id, "")
        .await;
    if after_stop == "CANCELED" {
        assert_eq!(
            results.status, 400,
            "a canceled query reports no results, got {} with body {}",
            results.status, results.body
        );
    } else {
        assert_eq!(
            results.status, 200,
            "a query that finished before the stop reports its results, got {} with body {}",
            results.status, results.body
        );
    }
    // The stop repeated is the no-op an idempotent delete is modelled for, and the
    // status it reached does not move.
    assert_eq!(
        emulator
            .stop_performance_query(name, &query_id)
            .await
            .status,
        200,
        "the stop repeated succeeds"
    );
    assert_eq!(
        emulator
            .performance_query_status(name, &query_id)
            .await
            .text("status"),
        after_stop,
        "a terminal query keeps the outcome it reached"
    );
}

#[tokio::test]
async fn every_performance_monitor_query_refusal_names_its_condition() {
    let emulator = Emulator::new();
    let name = "perf-query-refusals";
    let other = "perf-query-neighbour";
    for monitor in [name, other] {
        let created = emulator
            .create_performance_monitor(json!({
                "monitorName": monitor,
                "targets": [vpc_target(1)],
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "the monitor {monitor:?} must be created, got {} with body {}",
            created.status, created.body
        );
    }
    await_performance_monitor_active(&emulator, name, "the monitor the queries hang off").await;

    let started = emulator
        .start_performance_query(
            name,
            json!({
                "startTime": "2026-10-22T19:07:07Z",
                "endTime": "2026-10-22T20:07:07Z",
                "queryType": PERFORMANCE_QUERY_TYPE,
            }),
        )
        .await;
    let query_id = started.text("queryId");
    // A `queryId` of the shape this service mints that no start minted.
    let unminted = "00000000-0000-4000-9000-0000000000fe";
    let absent = "perf-never-created";
    let absent_sentence = format!(
        "Monitor [arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:performance-monitor/{absent}] \
         doesn't exist"
    );
    let no_such_query = format!("There's no query with id {{{unminted}}}");

    // (case, answer, status, class, sentence)
    let rows = vec![
        (
            "a window whose end precedes its start",
            emulator
                .start_performance_query(
                    name,
                    json!({
                        "startTime": "2026-10-22T20:07:07Z",
                        "endTime": "2026-10-22T19:07:07Z",
                        "queryType": PERFORMANCE_QUERY_TYPE,
                    }),
                )
                .await,
            400,
            "ValidationException",
            "EndTime cannot be before StartTime".to_string(),
        ),
        (
            "a start under a monitor nothing resolves",
            emulator
                .start_performance_query(
                    absent,
                    json!({
                        "startTime": "2026-10-22T19:07:07Z",
                        "endTime": "2026-10-22T20:07:07Z",
                        "queryType": PERFORMANCE_QUERY_TYPE,
                    }),
                )
                .await,
            404,
            "ResourceNotFoundException",
            absent_sentence.clone(),
        ),
        (
            "the status of an id no start minted",
            emulator.performance_query_status(name, unminted).await,
            404,
            "ResourceNotFoundException",
            no_such_query.clone(),
        ),
        (
            "the results of that same id",
            emulator.performance_query_results(name, unminted, "").await,
            404,
            "ResourceNotFoundException",
            no_such_query.clone(),
        ),
        (
            "a stop of that same id",
            emulator.stop_performance_query(name, unminted).await,
            404,
            "ResourceNotFoundException",
            no_such_query,
        ),
        (
            // A query is only reachable under the monitor it was started for.
            "a live query read under another monitor",
            emulator.performance_query_status(other, &query_id).await,
            404,
            "ResourceNotFoundException",
            format!("There's no query with id {{{query_id}}}"),
        ),
        (
            "the status read under a monitor nothing resolves",
            emulator.performance_query_status(absent, &query_id).await,
            404,
            "ResourceNotFoundException",
            absent_sentence.clone(),
        ),
        (
            "the results read under a monitor nothing resolves",
            emulator
                .performance_query_results(absent, &query_id, "")
                .await,
            404,
            "ResourceNotFoundException",
            absent_sentence,
        ),
    ];

    for (case, answer, status, class, sentence) in rows {
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.class(),
            class,
            "case {case:?}: expected {class}, got {:?} with body {}",
            answer.class(),
            answer.body
        );
        assert_eq!(
            answer.sentence(),
            sentence,
            "case {case:?}: expected {sentence:?}, got {:?}",
            answer.sentence()
        );
    }

    // No page of these results issues a resume token, so a request carrying one
    // carries a cursor this reader did not mint.
    await_query_succeeded(&emulator, name, &query_id, "the live query").await;
    let with_token = emulator
        .performance_query_results(name, &query_id, "?nextToken=abcd")
        .await;
    assert_eq!(
        with_token.status, 400,
        "a cursor this reader did not mint is refused, got {} with body {}",
        with_token.status, with_token.body
    );
    assert_eq!(
        with_token.class(),
        "ValidationException",
        "expected ValidationException, got {:?}",
        with_token.class()
    );
}

#[tokio::test]
async fn monitor_scope_suggestions_report_the_coverage_of_the_monitors_scope() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let name = "suggested-monitor";
    let monitor = emulator
        .create_monitor(json!({
            "monitorName": name,
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }],
        }))
        .await;
    assert_eq!(
        monitor.status, 200,
        "the monitor must be created, got {} with body {}",
        monitor.status, monitor.body
    );
    let monitor_arn = monitor.text("monitorArn");
    let query = format!("?monitorArn={}", query_encoded(&monitor_arn));

    // While nothing has determined the coverage yet, the status says so and no
    // instant is reported for it; once it has been determined, the instant is the
    // one the monitor reached its terminal status at.
    let first = emulator.monitor_scope_suggestions(&query).await;
    assert_eq!(
        first.status, 200,
        "expected 200, got {} with body {}",
        first.status, first.body
    );
    match first.text("status").as_str() {
        "PENDING" => assert_eq!(
            first.member("updatedAt"),
            &Value::Null,
            "coverage that has not been determined reports no instant, got {:?}",
            first.member("updatedAt")
        ),
        "COMPLETE" => assert!(
            first.number("updatedAt") > 0.0,
            "determined coverage reports the instant it was determined, got {:?}",
            first.member("updatedAt")
        ),
        other => panic!("expected PENDING or COMPLETE, got {other:?}"),
    }

    let settled = {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let read = emulator.monitor_scope_suggestions(&query).await;
            if read.text("status") != "PENDING" {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "the coverage was never determined; last body {}",
                read.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    // This emulator observes no traffic outside the monitor's scope, so there is no
    // account to suggest adding — which is what makes the coverage COMPLETE.
    assert_eq!(
        settled.text("status"),
        "COMPLETE",
        "a scope with no account to add is COMPLETE, got {:?}",
        settled.text("status")
    );
    assert_eq!(
        settled.member("accountsToAdd"),
        &json!([]),
        "expected no suggested accounts, got {:?}",
        settled.member("accountsToAdd")
    );
    assert_eq!(
        settled.member("nextToken"),
        &Value::Null,
        "a single page issues no resume token, got {:?}",
        settled.member("nextToken")
    );
    // The instant the coverage was determined is the one the monitor came up at,
    // read off the same record the monitor reader answers from.
    let read = emulator.get_monitor(name).await;
    assert_eq!(
        settled.number("updatedAt"),
        read.number("modifiedAt"),
        "expected the instant the monitor came up ({}), got {}",
        read.number("modifiedAt"),
        settled.number("updatedAt")
    );
    // A read changes nothing a later reader would see.
    let again = emulator.monitor_scope_suggestions(&query).await;
    assert_eq!(
        again.body, settled.body,
        "two reads answer one body, got {} then {}",
        settled.body, again.body
    );

    // An ARN that names no monitor of this store is the modelled 404 — including
    // the ARN of a resource of this service that is not a monitor, and the ARN of
    // the monitor after it is deleted.
    let performance = emulator
        .create_performance_monitor(json!({
            "monitorName": "suggested-performance",
            "targets": [vpc_target(1)],
        }))
        .await;
    assert_eq!(
        performance.status, 201,
        "the performance monitor must be created, got {} with body {}",
        performance.status, performance.body
    );
    assert_eq!(
        emulator.delete_monitor(name).await.status,
        200,
        "the monitor must delete"
    );
    for (case, arn) in [
        ("a monitor name nothing was minted for", format!(
            "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:monitor/never-created"
        )),
        ("a performance monitor's ARN", performance.text("monitorArn")),
        ("the scope's ARN", scope_arn.clone()),
        ("the ARN of the deleted monitor", monitor_arn.clone()),
        ("another service's ARN", format!(
            "arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc/vpc-123"
        )),
    ] {
        let refused = emulator
            .monitor_scope_suggestions(&format!("?monitorArn={}", query_encoded(&arn)))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.class(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.class()
        );
        assert_eq!(
            refused.sentence(),
            format!("Monitor [{arn}] doesn't exist"),
            "case {case:?}: the refusal names the ARN the request carried, got {:?}",
            refused.sentence()
        );
    }
}
