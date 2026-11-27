// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms protected-query slice
//! (`StartProtectedQuery` / `GetProtectedQuery`).
//!
//! The rule under test: a protected query is recorded against PERSISTED membership
//! state and every member of the response is computed from that state — the ids and
//! ARNs from the stored membership, the result configuration from the request else the
//! membership's stored default, the query-compute payer from the collaboration's payer
//! candidate, the worker compute from the request plus the model's declared defaults —
//! so a later `GetProtectedQuery` reports exactly what the start call returned, a
//! membership the store does not hold is a `ResourceNotFoundException`, and the
//! terminal status is the one the CLOCK owes rather than one a read invents.
//!
//! Each table varies what the rule generalises over (which SQL parameter carries the
//! query, which output configuration the results go to, where the result configuration
//! comes from, which worker compute members the caller filled in, which ability the
//! collaboration granted, who was configured to pay), so the recorded case the issue
//! cited — a start against a membership that does not exist — is one row among many.
//!
//! Every request goes over the wire through the same `MultiServiceRouter` the
//! production router uses.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::ops::types::RetryInfo;
use service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::state as cleanrooms_state;

use super::cleanrooms_analysis_templates::{artifacts_source, create_analysis_template};
use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, is_model_uuid, minimal_collaboration,
    Emulator, ALL_MEMBER_ABILITIES, REGION,
};
use super::cleanrooms_configured_table::{create_body, COLUMN_NAMES};

/// A well-formed `MembershipIdentifier`/`ProtectedQueryIdentifier` (model shape `UUID`:
/// 36 chars, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no
/// call ever created, so only state decides the answer.
const ABSENT_UUID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
/// A second one, for the rows that need two ids the store does not hold.
pub(crate) const OTHER_ABSENT_UUID: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

/// The published bounds of `WorkerComputeConfiguration$number` (model member
/// `number`, `range 2..1024`).
const WORKER_NUMBER_MIN: i64 = 2;
const WORKER_NUMBER_MAX: i64 = 1024;

/// Start a protected query against `membership_id`.
pub(crate) async fn start_query(
    emulator: &Emulator,
    membership_id: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/protectedQueries"),
            Some(body),
        )
        .await
}

/// Read a protected query back under the membership that started it.
pub(crate) async fn get_query(
    emulator: &Emulator,
    membership_id: &str,
    query_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/memberships/{membership_id}/protectedQueries/{query_id}"),
            None,
        )
        .await
}

/// How long a poll waits for the run a query is owed to have happened.
///
/// Generous rather than tight: the completion is due a span after the start call, so this
/// deadline is only the point at which the run is declared never to have happened — a
/// loaded host must not be the thing that trips it.
const RUN_PATIENCE: Duration = Duration::from_secs(120);

/// How often that poll asks again.
const RUN_POLL: Duration = Duration::from_millis(25);

/// Read the query until it reports a terminal status, and answer that query. Waits for the
/// CONDITION, never for a fixed time.
pub(crate) async fn poll_until_terminal(
    emulator: &Emulator,
    membership_id: &str,
    query_id: &str,
) -> Value {
    let deadline = std::time::Instant::now() + RUN_PATIENCE;
    loop {
        let (status, body) = get_query(emulator, membership_id, query_id).await;
        assert_eq!(
            status, 200,
            "expected GetProtectedQuery 200 while waiting for the run, got {status} with {body}"
        );
        let query = body["protectedQuery"].clone();
        if query["status"]
            .as_str()
            .is_some_and(cleanrooms_state::protected_query_is_terminal)
        {
            return query;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the query never reached a terminal status, last read {query}"
        );
        tokio::time::sleep(RUN_POLL).await;
    }
}

/// Create `collaboration`, join it with `membership_body` (its `collaborationIdentifier`
/// and `queryLogStatus` are filled in here) and return the stored `Membership`.
async fn join(emulator: &Emulator, collaboration: Value, membership_body: Value) -> Value {
    let (status, created) = emulator.create_collaboration(collaboration).await;
    assert_eq!(
        status, 200,
        "CreateCollaboration must succeed to set up the case, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no collaboration.id in {created}"))
        .to_string();

    let mut body = membership_body;
    body["collaborationIdentifier"] = json!(collaboration_id);
    body["queryLogStatus"] = json!("ENABLED");
    let (status, joined) = emulator.create_membership(body).await;
    assert_eq!(
        status, 200,
        "CreateMembership must succeed to set up the case, got {status} with {joined}"
    );
    joined["membership"].clone()
}

/// The membership of a collaboration whose creator holds every declared ability, with
/// no default result configuration.
pub(crate) async fn simple_membership(emulator: &Emulator, name: &str) -> Value {
    join(emulator, minimal_collaboration(name), json!({})).await
}

/// A minimal valid `StartProtectedQuery` body: the required `type` plus inline SQL.
pub(crate) fn minimal_query() -> Value {
    json!({
        "type": "SQL",
        "sqlParameters": { "queryString": "SELECT COUNT(*) FROM orders" },
    })
}

/// An `AnalysisTemplateArn` under `membership_id`, in the form the model's pattern
/// declares: `^arn:aws[-a-z]*:cleanrooms:[\w]{2}-[\w]{4,9}-[\d]:[\d]{12}:membership/[\d\w-]+/analysistemplate/[\d\w-]+$`.
fn analysis_template_arn(membership_id: &str) -> String {
    format!(
        "arn:aws:cleanrooms:{REGION}:{}:membership/{membership_id}/analysistemplate/{ABSENT_UUID}",
        account_id()
    )
}

/// The protected query without the two members the clock moves, so a round-trip
/// comparison is not a race against the owed transition.
pub(crate) fn without_settled_members(query: &Value) -> Value {
    let mut copy = query.clone();
    let members = copy
        .as_object_mut()
        .unwrap_or_else(|| panic!("a protected query is a JSON object, got {query}"));
    members.remove("status");
    members.remove("error");
    copy
}

/// Every state a stored protected query may legally be in: SUBMITTED with no error
/// while the owed run is not yet due, STARTED while its statement is being run, else the
/// outcome of that run — SUCCESS carrying what it computed, or FAILED carrying what the
/// engine refused the statement for. Nothing else, and never a SUCCESS with results
/// nothing computed.
fn assert_legal_state(case: &str, query: &Value) {
    match query["status"].as_str() {
        Some("SUBMITTED" | "STARTED") => assert!(
            query.get("error").is_none() && query.get("result").is_none(),
            "case {case:?}: a query that has not settled carries no error and no result, got {query}"
        ),
        Some("FAILED") => {
            // Every query of this suite reads relations no configured table association
            // of its membership names, so the statement is one the engine refuses.
            assert_eq!(
                query["error"]["code"].as_str(),
                Some(cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY),
                "case {case:?}: unexpected failure code, got {:?}",
                query["error"]
            );
            assert!(
                query["error"]["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the failure says what the statement was refused for, got {:?}",
                query["error"]
            );
            assert!(
                query.get("result").is_none() && query.get("statistics").is_none(),
                "case {case:?}: a query that never ran reports no result and no statistics, got {query}"
            );
        }
        other => panic!("case {case:?}: unexpected protected query status {other:?} in {query}"),
    }
}

/// The `computeConfiguration` a query reports: the worker members in effect — the
/// model's declared defaults where the request named none — with the engine's own worker
/// properties and any the request wrote over them.
fn expected_worker(worker_type: &str, number: i64, requested: &[(&str, &str)]) -> Value {
    let mut properties: std::collections::BTreeMap<String, String> =
        cleanrooms_state::default_worker_properties()
            .into_iter()
            .collect();
    properties.extend(
        requested
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    json!({
        "worker": {
            "type": worker_type,
            "number": number,
            "properties": { "spark": properties },
        },
    })
}

/// The `keyPrefix` a query's results really go under: the configured prefix, then the
/// collaboration the query ran in.
fn expected_key_prefix(membership: &Value, configured: Option<&str>) -> String {
    let collaboration = membership["collaborationId"]
        .as_str()
        .unwrap_or_else(|| panic!("no collaborationId in {membership}"));
    let partition = format!("collaboration={collaboration}");
    match configured {
        Some(prefix) => format!("{}/{partition}", prefix.trim_end_matches('/')),
        None => partition,
    }
}

/// What a row expects for the members the SERVICE decides. `Value::Null` means the
/// member must be ABSENT from the response.
struct Expected {
    result_configuration: Value,
    compute_configuration: Value,
    query_compute_payer_account_id: Value,
}

/// One row of the start/read round-trip table.
struct QueryCase {
    label: &'static str,
    /// Extra members for the `CreateMembership` body — a row that tests the fallback
    /// puts the membership's `defaultResultConfiguration` here.
    membership: fn() -> Value,
    /// The `StartProtectedQuery` body, built from the stored membership so a row can
    /// name that membership inside an ARN.
    request: fn(&Value) -> Value,
    /// What the response must carry, built from the stored membership: the result
    /// configuration in effect names the collaboration the query runs in.
    expected: fn(&Value) -> Expected,
}

fn membership_default_result_configuration() -> Value {
    json!({
        "defaultResultConfiguration": {
            "outputConfiguration": {
                "s3": { "resultFormat": "PARQUET", "bucket": "membership-default-results" },
            },
            "roleArn": "arn:aws:iam::000000000000:role/clean-rooms-results",
        },
    })
}

fn query_cases() -> Vec<QueryCase> {
    vec![
        QueryCase {
            label: "inline queryString, no result or compute configuration anywhere",
            membership: || json!({}),
            request: |_| minimal_query(),
            expected: |_| Expected {
                result_configuration: Value::Null,
                // Nothing to override the model's declared worker defaults with, so the
                // query reports the compute it runs on.
                compute_configuration: expected_worker(
                    cleanrooms_state::DEFAULT_WORKER_COMPUTE_TYPE,
                    i64::from(cleanrooms_state::DEFAULT_WORKER_COMPUTE_NUMBER),
                    &[],
                ),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "parameterized queryString, s3 CSV results, CR.4X worker compute",
            membership: || json!({}),
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": {
                        "queryString": "SELECT region, COUNT(*) FROM orders WHERE region = :region GROUP BY region",
                        "parameters": { "region": "eu-central" },
                    },
                    "resultConfiguration": {
                        "outputConfiguration": {
                            "s3": {
                                "resultFormat": "CSV",
                                "bucket": "clean-rooms-results",
                                "keyPrefix": "queries/2026",
                                "singleFileOutput": true,
                            },
                        },
                    },
                    "computeConfiguration": {
                        "worker": {
                            "type": "CR.4X",
                            "number": 32,
                            "properties": { "spark": { "spark.sql.shuffle.partitions": "8" } },
                        },
                    },
                })
            },
            expected: |membership| Expected {
                result_configuration: json!({
                    "outputConfiguration": {
                        "s3": {
                            "resultFormat": "CSV",
                            "bucket": "clean-rooms-results",
                            "keyPrefix": expected_key_prefix(membership, Some("queries/2026")),
                            "singleFileOutput": true,
                        },
                    },
                }),
                compute_configuration: expected_worker(
                    "CR.4X",
                    32,
                    &[("spark.sql.shuffle.partitions", "8")],
                ),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "analysisTemplateArn instead of inline SQL; the membership's default results apply",
            membership: membership_default_result_configuration,
            request: |membership| {
                json!({
                    "type": "SQL",
                    "sqlParameters": {
                        "analysisTemplateArn": analysis_template_arn(
                            membership["id"].as_str().unwrap_or_default(),
                        ),
                    },
                })
            },
            expected: |membership| Expected {
                // The membership-side shape carries a `roleArn` the query-side shape
                // does not model, so only the output configuration carries over — with
                // the collaboration partition the results really go under.
                result_configuration: json!({
                    "outputConfiguration": {
                        "s3": {
                            "resultFormat": "PARQUET",
                            "bucket": "membership-default-results",
                            "keyPrefix": expected_key_prefix(membership, None),
                        },
                    },
                }),
                compute_configuration: expected_worker(
                    cleanrooms_state::DEFAULT_WORKER_COMPUTE_TYPE,
                    i64::from(cleanrooms_state::DEFAULT_WORKER_COMPUTE_NUMBER),
                    &[],
                ),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "member output configuration; worker compute with no members at all",
            membership: || json!({}),
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT 1" },
                    "resultConfiguration": {
                        "outputConfiguration": { "member": { "accountId": account_id() } },
                    },
                    "computeConfiguration": { "worker": {} },
                })
            },
            expected: |_| Expected {
                // A member output configuration names no key prefix to partition.
                result_configuration: json!({
                    "outputConfiguration": { "member": { "accountId": account_id() } },
                }),
                // Both worker members carry a declared default, so the query reports
                // the compute it runs on.
                compute_configuration: expected_worker(
                    cleanrooms_state::DEFAULT_WORKER_COMPUTE_TYPE,
                    i64::from(cleanrooms_state::DEFAULT_WORKER_COMPUTE_NUMBER),
                    &[],
                ),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "distribute output configuration; worker number at the published minimum",
            membership: || json!({}),
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT * FROM impressions" },
                    "resultConfiguration": {
                        "outputConfiguration": {
                            "distribute": {
                                "locations": [
                                    { "s3": { "resultFormat": "PARQUET", "bucket": "distributed-results" } },
                                    { "member": { "accountId": account_id() } },
                                ],
                            },
                        },
                    },
                    "computeConfiguration": { "worker": { "number": WORKER_NUMBER_MIN } },
                })
            },
            expected: |membership| Expected {
                result_configuration: json!({
                    "outputConfiguration": {
                        "distribute": {
                            "locations": [
                                {
                                    "s3": {
                                        "resultFormat": "PARQUET",
                                        "bucket": "distributed-results",
                                        "keyPrefix": expected_key_prefix(membership, None),
                                    },
                                },
                                { "member": { "accountId": account_id() } },
                            ],
                        },
                    },
                }),
                compute_configuration: expected_worker(
                    cleanrooms_state::DEFAULT_WORKER_COMPUTE_TYPE,
                    WORKER_NUMBER_MIN,
                    &[],
                ),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
        QueryCase {
            label: "the request's result configuration overrides the membership default; worker number at the published maximum",
            membership: membership_default_result_configuration,
            request: |_| {
                json!({
                    "type": "SQL",
                    "sqlParameters": { "queryString": "SELECT * FROM clicks" },
                    "resultConfiguration": {
                        "outputConfiguration": {
                            "s3": { "resultFormat": "CSV", "bucket": "per-query-results" },
                        },
                    },
                    "computeConfiguration": { "worker": { "type": "CR.8X", "number": WORKER_NUMBER_MAX } },
                    "queryComputePayerAccountId": account_id(),
                })
            },
            expected: |membership| Expected {
                result_configuration: json!({
                    "outputConfiguration": {
                        "s3": {
                            "resultFormat": "CSV",
                            "bucket": "per-query-results",
                            "keyPrefix": expected_key_prefix(membership, None),
                        },
                    },
                }),
                compute_configuration: expected_worker("CR.8X", WORKER_NUMBER_MAX, &[]),
                query_compute_payer_account_id: json!(account_id()),
            },
        },
    ]
}

#[tokio::test]
async fn start_protected_query_persists_and_get_protected_query_returns_it_unchanged() {
    let emulator = Emulator::new();

    for (row, case) in query_cases().into_iter().enumerate() {
        let label = case.label;
        let membership = join(
            &emulator,
            minimal_collaboration(&format!("queries-{row}")),
            (case.membership)(),
        )
        .await;
        let membership_id = membership["id"].as_str().expect("membership id").to_string();
        let request = (case.request)(&membership);
        let expected = (case.expected)(&membership);

        let (status, started) = start_query(&emulator, &membership_id, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartProtectedQuery 200, got {status} with {started}"
        );
        let query = started
            .get("protectedQuery")
            .unwrap_or_else(|| panic!("case {label:?}: no `protectedQuery` in {started}"))
            .clone();

        let id = query["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no `protectedQuery.id` in {started}"));
        assert!(
            is_model_uuid(id),
            "case {label:?}: expected a 36-char lowercase-hex UUID id, got {id:?}"
        );
        assert_eq!(
            query["membershipId"].as_str(),
            Some(membership_id.as_str()),
            "case {label:?}: the query belongs to the membership it was started under, got {:?}",
            query["membershipId"]
        );
        assert_eq!(
            query["membershipArn"], membership["arn"],
            "case {label:?}: the membership ARN comes from the stored membership, got {:?}",
            query["membershipArn"]
        );
        assert_eq!(
            query["status"].as_str(),
            Some("SUBMITTED"),
            "case {label:?}: a query the service has accepted but not finished is SUBMITTED, got {:?}",
            query["status"]
        );
        assert!(
            query["createTime"].is_number(),
            "case {label:?}: expected an epoch-seconds createTime, got {:?}",
            query["createTime"]
        );
        assert_eq!(
            query["sqlParameters"], request["sqlParameters"],
            "case {label:?}: the SQL parameters are stored as sent, got {:?}",
            query["sqlParameters"]
        );
        for absent in ["result", "statistics", "error"] {
            assert!(
                query.get(absent).is_none(),
                "case {label:?}: nothing has run yet, so {absent} must be absent, got {:?}",
                query.get(absent)
            );
        }
        // `DifferentialPrivacyParameters$sensitivityParameters` is a required list, so the
        // member is reported — empty for a query no analysis rule configures differential
        // privacy for.
        assert_eq!(
            query["differentialPrivacy"],
            json!({ "sensitivityParameters": [] }),
            "case {label:?}: unexpected differentialPrivacy, got {:?}",
            query.get("differentialPrivacy")
        );
        for (member, expected) in [
            ("resultConfiguration", expected.result_configuration),
            ("computeConfiguration", expected.compute_configuration),
            (
                "queryComputePayerAccountId",
                expected.query_compute_payer_account_id,
            ),
        ] {
            match expected {
                Value::Null => assert!(
                    query.get(member).is_none(),
                    "case {label:?}: expected {member} to be absent, got {:?}",
                    query.get(member)
                ),
                expected => assert_eq!(
                    query[member], expected,
                    "case {label:?}: unexpected {member}, got {:?}",
                    query[member]
                ),
            }
        }

        // The round trip: the read reports the STORED query, member for member, apart
        // from the status the clock may already have moved on.
        let (status, fetched) = get_query(&emulator, &membership_id, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetProtectedQuery 200, got {status} with {fetched}"
        );
        let read_back = fetched["protectedQuery"].clone();
        assert_eq!(
            without_settled_members(&read_back),
            without_settled_members(&query),
            "case {label:?}: GetProtectedQuery must report the stored query, got {read_back}"
        );
        assert_legal_state(label, &read_back);
    }
}

#[tokio::test]
async fn start_protected_query_refuses_a_membership_the_store_does_not_hold() {
    let emulator = Emulator::new();

    // A membership that was never created, and one that was created and deleted: in
    // both cases the store holds no such membership, which is what decides the answer.
    // The deleted one is a membership of a collaboration this account only PARTICIPATES
    // in, because a participant may leave while the creator's own membership is removed
    // only by deleting the collaboration.
    let collaboration_id = collaboration_owned_elsewhere(
        &emulator,
        "deleted-membership",
        Some(&["CAN_QUERY"]),
    )
    .await;
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "CreateMembership must succeed to set up the case, got {status} with {joined}"
    );
    let deleted_id = joined["membership"]["id"]
        .as_str()
        .expect("membership id")
        .to_string();
    let (status, body) = emulator
        .call("DELETE", &format!("/memberships/{deleted_id}"), None)
        .await;
    assert_eq!(
        status, 204,
        "DeleteMembership must succeed to set up the case, got {status} with {body}"
    );

    for (label, membership_id) in [
        ("a membership id no call ever created", ABSENT_UUID.to_string()),
        ("the id of a deleted membership", deleted_id),
    ] {
        let (status, body) = start_query(&emulator, &membership_id, minimal_query()).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected StartProtectedQuery 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(membership_id.as_str()),
            "case {label:?}: the 404 names the membership the request addressed, got {:?}",
            body["resourceId"]
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("MEMBERSHIP"),
            "case {label:?}: unexpected resourceType, got {:?}",
            body["resourceType"]
        );

        // The refusal persisted nothing: no query is readable under that membership.
        let (status, body) = get_query(&emulator, &membership_id, OTHER_ABSENT_UUID).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected GetProtectedQuery 404, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn start_protected_query_refuses_a_membership_whose_collaboration_is_gone() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .create_collaboration(minimal_collaboration("doomed"))
        .await;
    assert_eq!(status, 200, "CreateCollaboration failed: {created}");
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .expect("collaboration id")
        .to_string();
    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(status, 200, "CreateMembership failed: {joined}");
    let membership_id = joined["membership"]["id"]
        .as_str()
        .expect("membership id")
        .to_string();

    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/collaborations/{collaboration_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "DeleteCollaboration must succeed to set up the case, got {status} with {body}"
    );

    let (status, body) = start_query(&emulator, &membership_id, minimal_query()).await;
    assert_eq!(
        status, 404,
        "a query runs inside a collaboration, so a membership left by a deleted one has nothing to run against: got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "unexpected error code, got {body}"
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(collaboration_id.as_str()),
        "the 404 names the collaboration that is gone, got {:?}",
        body["resourceId"]
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some("COLLABORATION"),
        "unexpected resourceType, got {:?}",
        body["resourceType"]
    );
}

#[tokio::test]
async fn start_protected_query_requires_the_can_query_ability() {
    let emulator = Emulator::new();

    // The whole `MemberAbility` catalog, one collaboration per ability, plus a
    // collaboration that grants every ability at once: starting a query is the
    // CAN_QUERY ability and nothing else.
    let mut cases: Vec<(String, Vec<&str>)> = ALL_MEMBER_ABILITIES
        .iter()
        .map(|ability| (format!("only {ability}"), vec![*ability]))
        .collect();
    cases.push((
        "every declared ability".to_string(),
        ALL_MEMBER_ABILITIES.to_vec(),
    ));

    for (row, (label, abilities)) in cases.into_iter().enumerate() {
        let membership = join(
            &emulator,
            json!({
                "name": format!("abilities-{row}"),
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": abilities,
                "members": [],
                "queryLogStatus": "ENABLED",
            }),
            json!({}),
        )
        .await;
        let membership_id = membership["id"].as_str().expect("membership id");

        let (status, body) = start_query(&emulator, membership_id, minimal_query()).await;
        if abilities.contains(&"CAN_QUERY") {
            assert_eq!(
                status, 200,
                "case {label:?}: a member that can query may start one, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {label:?}: a member without CAN_QUERY cannot start a query, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {label:?}: unexpected error code, got {body}"
            );
            assert_eq!(
                body["reason"].as_str(),
                Some("INSUFFICIENT_PERMISSIONS"),
                "case {label:?}: unexpected reason, got {:?}",
                body["reason"]
            );
        }
    }
}

#[tokio::test]
async fn start_protected_query_refuses_sql_parameters_that_name_no_single_query() {
    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "sql-parameters").await;
    let membership_id = membership["id"].as_str().expect("membership id");
    let arn = analysis_template_arn(membership_id);

    // `ProtectedQuerySQLParameters` models both members as optional, so the boundary
    // validator passes each of these: the SERVICE is what refuses a request that
    // identifies no single query to run.
    let cases: Vec<(&str, Value)> = vec![
        ("neither member", json!({})),
        ("an all-whitespace queryString", json!({ "queryString": "   " })),
        ("an empty queryString", json!({ "queryString": "" })),
        (
            "both members at once",
            json!({ "queryString": "SELECT 1", "analysisTemplateArn": arn }),
        ),
        (
            "both members, with query parameters",
            json!({
                "queryString": "SELECT :x",
                "analysisTemplateArn": analysis_template_arn(membership_id),
                "parameters": { "x": "1" },
            }),
        ),
    ];

    for (label, sql_parameters) in cases {
        let (status, body) = start_query(
            &emulator,
            membership_id,
            json!({ "type": "SQL", "sqlParameters": sql_parameters }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected StartProtectedQuery 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["name"].as_str(),
            Some("sqlParameters"),
            "case {label:?}: the refusal names the member at fault, got {body}"
        );
    }
}

#[tokio::test]
async fn start_protected_query_refuses_a_worker_count_outside_the_published_range() {
    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "worker-range").await;
    let membership_id = membership["id"].as_str().expect("membership id");

    // One past each published bound of `WorkerComputeConfiguration$number`
    // (`range 2..1024`); the bounds themselves are accepted by the table above.
    for number in [WORKER_NUMBER_MIN - 1, WORKER_NUMBER_MAX + 1] {
        let mut body = minimal_query();
        body["computeConfiguration"] = json!({ "worker": { "number": number } });
        let (status, answer) = start_query(&emulator, membership_id, body).await;
        assert_eq!(
            status, 400,
            "case {number:?} workers: outside the declared range 2..1024, got {status} with {answer}"
        );
    }
}

#[tokio::test]
async fn the_query_compute_payer_is_computed_from_the_collaboration() {
    let emulator = Emulator::new();

    let partner = "111111111111";
    // Rows: who the collaboration configured, and who must therefore pay. The payer
    // the request names wins; else the account the creator configured as responsible
    // for query compute; else, per `QueryComputePaymentConfig$isResponsible`, "the
    // member who can query".
    struct PayerCase {
        label: &'static str,
        collaboration: Value,
        requested_payer: Option<&'static str>,
        expected_payer: &'static str,
    }
    let cases = vec![
        PayerCase {
            label: "nobody configured: the member who can query is the default payer",
            collaboration: minimal_collaboration("payer-default"),
            requested_payer: None,
            expected_payer: account_id(),
        },
        PayerCase {
            label: "the creator configured as the query-compute payer",
            collaboration: json!({
                "name": "payer-creator",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY"],
                "creatorPaymentConfiguration": { "queryCompute": { "isResponsible": true } },
                "members": [],
                "queryLogStatus": "ENABLED",
            }),
            requested_payer: None,
            expected_payer: account_id(),
        },
        PayerCase {
            label: "another member configured as the query-compute payer",
            collaboration: json!({
                "name": "payer-member",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY"],
                "members": [{
                    "accountId": partner,
                    "displayName": "partner",
                    "memberAbilities": ["CAN_RECEIVE_RESULTS"],
                    "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
                }],
                "queryLogStatus": "ENABLED",
            }),
            requested_payer: None,
            expected_payer: partner,
        },
        PayerCase {
            label: "the request names a payer, overriding the configured candidate",
            collaboration: json!({
                "name": "payer-requested",
                "creatorDisplayName": "creator",
                "creatorMemberAbilities": ["CAN_QUERY"],
                "members": [{
                    "accountId": partner,
                    "displayName": "partner",
                    "memberAbilities": ["CAN_RECEIVE_RESULTS"],
                    "paymentConfiguration": { "queryCompute": { "isResponsible": true } },
                }],
                "queryLogStatus": "ENABLED",
            }),
            requested_payer: Some("000000000000"),
            expected_payer: account_id(),
        },
    ];

    for case in cases {
        let label = case.label;
        let membership = join(&emulator, case.collaboration, json!({})).await;
        let membership_id = membership["id"].as_str().expect("membership id");
        let mut body = minimal_query();
        if let Some(payer) = case.requested_payer {
            body["queryComputePayerAccountId"] = json!(payer);
        }

        let (status, started) = start_query(&emulator, membership_id, body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartProtectedQuery 200, got {status} with {started}"
        );
        assert_eq!(
            started["protectedQuery"]["queryComputePayerAccountId"].as_str(),
            Some(case.expected_payer),
            "case {label:?}: unexpected query compute payer, got {:?}",
            started["protectedQuery"]["queryComputePayerAccountId"]
        );
    }

    // An account the collaboration does not hold cannot be made to pay for it.
    let membership = simple_membership(&emulator, "payer-stranger").await;
    let membership_id = membership["id"].as_str().expect("membership id");
    let mut body = minimal_query();
    body["queryComputePayerAccountId"] = json!("222222222222");
    let (status, answer) = start_query(&emulator, membership_id, body).await;
    assert_eq!(
        status, 400,
        "an account outside the collaboration cannot be the payer, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "unexpected error code, got {answer}"
    );
    assert_eq!(
        answer["fieldList"][0]["name"].as_str(),
        Some("queryComputePayerAccountId"),
        "the refusal names the member at fault, got {answer}"
    );
}

#[tokio::test]
async fn a_started_query_is_submitted_until_the_clock_runs_it_and_then_reports_the_outcome() {
    // The span is the emulator's own choice, inside the band this project fixes for a
    // transition the clock owes, and it is never zero.
    let delay = cleanrooms_state::PROTECTED_QUERY_SETTLE_DELAY;
    assert!(
        (Duration::from_millis(250)..=Duration::from_millis(750)).contains(&delay),
        "the owed transition must be due 250-750 ms out, got {delay:?}"
    );

    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "settling").await;
    let membership_id = membership["id"].as_str().expect("membership id").to_string();

    let (status, started) = start_query(&emulator, &membership_id, minimal_query()).await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {started}"
    );
    let query_id = started["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();

    // Before the span has passed, a caller acting straight after the start call still
    // sees the transient state.
    let (status, fetched) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(status, 200, "expected GetProtectedQuery 200, got {fetched}");
    assert_eq!(
        fetched["protectedQuery"]["status"].as_str(),
        Some("SUBMITTED"),
        "inside the owed span the query is still SUBMITTED, got {fetched}"
    );
    assert!(
        fetched["protectedQuery"].get("error").is_none(),
        "a SUBMITTED query carries no error, got {fetched}"
    );

    // The outcome is waited for by looking again until it is there, never by sleeping a
    // fixed time: the deadline is only the point at which the run is declared never to
    // have happened.
    let settled_query = poll_until_terminal(&emulator, &membership_id, &query_id).await;
    // This membership associates no configured table, so the statement names a relation
    // the collaboration does not expose and the engine refuses it.
    assert_eq!(
        settled_query["status"].as_str(),
        Some("FAILED"),
        "a statement the engine refuses settles in FAILED, got {settled_query}"
    );
    assert_eq!(
        settled_query["error"]["code"].as_str(),
        Some(cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_MALFORMED_QUERY),
        "unexpected failure code, got {settled_query}"
    );
    assert!(
        settled_query["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("orders")),
        "the failure carries the engine's own sentence about the relation the statement \
         named, got {settled_query}"
    );
    for absent in ["result", "statistics"] {
        assert!(
            settled_query.get(absent).is_none(),
            "nothing ran, so {absent} must be absent, got {settled_query}"
        );
    }
    // Everything the start call recorded is unchanged by the transition.
    assert_eq!(
        without_settled_members(&settled_query),
        without_settled_members(&started["protectedQuery"]),
        "the owed transition moves only the status and the error, got {settled_query}"
    );

    // Reading again changes nothing further: the transition is materialized once.
    let (_, again) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(
        again["protectedQuery"], settled_query,
        "a second read reports the same settled query, got {again}"
    );

    // Settling the queries already due leaves a query that is not due alone.
    let (status, fresh) = start_query(&emulator, &membership_id, minimal_query()).await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {fresh}"
    );
    let fresh_id = fresh["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();
    let (_, fresh_read) = get_query(&emulator, &membership_id, &fresh_id).await;
    assert_eq!(
        fresh_read["protectedQuery"]["status"].as_str(),
        Some("SUBMITTED"),
        "a query started after another settled is still SUBMITTED, got {fresh_read}"
    );
    let (_, settled_again) = get_query(&emulator, &membership_id, &query_id).await;
    assert_eq!(
        settled_again["protectedQuery"], settled_query,
        "the already settled query is untouched, got {settled_again}"
    );
}

#[tokio::test]
async fn get_protected_query_is_addressed_under_the_membership_that_started_it() {
    let emulator = Emulator::new();

    let first = simple_membership(&emulator, "owner").await;
    let second = simple_membership(&emulator, "bystander").await;
    let first_id = first["id"].as_str().expect("membership id").to_string();
    let second_id = second["id"].as_str().expect("membership id").to_string();

    let (status, started) = start_query(&emulator, &first_id, minimal_query()).await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {started}"
    );
    let query_id = started["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();

    let cases: Vec<(&str, &str, &str, u16)> = vec![
        ("its own membership", first_id.as_str(), query_id.as_str(), 200),
        (
            "another membership of the same account",
            second_id.as_str(),
            query_id.as_str(),
            404,
        ),
        (
            "a query id the membership does not hold",
            first_id.as_str(),
            ABSENT_UUID,
            404,
        ),
        (
            "a membership the store does not hold",
            OTHER_ABSENT_UUID,
            query_id.as_str(),
            404,
        ),
    ];

    for (label, membership_id, requested_query, expected_status) in cases {
        let (status, body) = get_query(&emulator, membership_id, requested_query).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: unexpected GetProtectedQuery status, got {status} with {body}"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: unexpected error code, got {body}"
            );
        } else {
            assert_eq!(
                body["protectedQuery"]["id"].as_str(),
                Some(query_id.as_str()),
                "case {label:?}: unexpected query, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_result_prefix_in_effect_partitions_by_collaboration_whichever_source_names_it() {
    let emulator = Emulator::new();

    // The rule generalises over WHERE the configuration comes from (the call, else the
    // membership's default) and over the FORM of the prefix it names (none, a bare
    // segment, one with a trailing slash, a nested one, and one that already carries a
    // partition segment of its own).
    let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("the call names no prefix at all", Some(""), None),
        ("the call names a bare prefix", Some("results"), Some("results")),
        (
            "the call names a prefix with a trailing slash",
            Some("results/"),
            Some("results"),
        ),
        (
            "the call names a nested prefix",
            Some("tenants/acme/results/"),
            Some("tenants/acme/results"),
        ),
        (
            "the call names a prefix that is already partitioned",
            Some("results/dt=2026-11-09"),
            Some("results/dt=2026-11-09"),
        ),
        ("the membership default names the prefix", None, Some("results")),
    ];

    for (row, (label, call_prefix, configured)) in cases.into_iter().enumerate() {
        let mut membership_body = json!({});
        if call_prefix.is_none() {
            let mut s3 = json!({ "resultFormat": "CSV", "bucket": "clean-rooms-results" });
            if let Some(prefix) = configured {
                s3["keyPrefix"] = json!(prefix);
            }
            membership_body = json!({
                "defaultResultConfiguration": {
                    "outputConfiguration": { "s3": s3 },
                    "roleArn": "arn:aws:iam::000000000000:role/clean-rooms-results",
                },
            });
        }
        let membership = join(
            &emulator,
            minimal_collaboration(&format!("prefix-{row}")),
            membership_body,
        )
        .await;
        let membership_id = membership["id"].as_str().expect("membership id").to_string();

        let mut body = minimal_query();
        if let Some(prefix) = call_prefix {
            let mut s3 = json!({ "resultFormat": "CSV", "bucket": "clean-rooms-results" });
            if !prefix.is_empty() {
                s3["keyPrefix"] = json!(prefix);
            }
            body["resultConfiguration"] = json!({ "outputConfiguration": { "s3": s3 } });
        }
        let (status, started) = start_query(&emulator, &membership_id, body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartProtectedQuery 200, got {status} with {started}"
        );
        let query = started["protectedQuery"].clone();
        let expected = expected_key_prefix(
            &membership,
            configured.filter(|prefix| !prefix.is_empty()),
        );
        assert_eq!(
            query["resultConfiguration"]["outputConfiguration"]["s3"]["keyPrefix"].as_str(),
            Some(expected.as_str()),
            "case {label:?}: expected the prefix in effect {expected:?}, got {query}"
        );
        // `KeyPrefix` is `length 0..512, pattern ^[\w!.=*/-]*$`: the prefix the service
        // reports stays inside the modelled constraints.
        assert!(
            expected.len() <= 512
                && expected
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '!' | '.' | '=' | '*' | '/' | '-')),
            "case {label:?}: the reported prefix must satisfy the KeyPrefix pattern, got {expected:?}"
        );
        // The read path reports the same prefix: one projection serves both.
        let id = query["id"].as_str().expect("protected query id");
        let (status, fetched) = get_query(&emulator, &membership_id, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetProtectedQuery 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["protectedQuery"]["resultConfiguration"],
            query["resultConfiguration"],
            "case {label:?}: the read reports the stored configuration, got {fetched}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// What a FAILED query reports about its failure: the category, and whether running the
// same query again might succeed.
// ---------------------------------------------------------------------------------------

/// Every failure category the model documents for `ProtectedQueryError$code`, with what
/// `RetryInfo$retryable` says of it: `Some(true)` where the category IS a temporary
/// condition ("`true` means that the failure was transient"), `Some(false)` where running
/// the same query again "is not expected to succeed", and `None` for the category that
/// stands for a failure that "couldn't be classified", whose transience is therefore not
/// known.
///
/// Source: the valid values documented on `ProtectedQueryError$code` and the semantics of
/// `RetryInfo$retryable` — `smithy_query(query="shape",
/// shape="ProtectedQueryError,RetryInfo")`.
const DOCUMENTED_CATEGORIES: [(&str, Option<bool>); 8] = [
    ("MALFORMED_QUERY", Some(false)),
    ("RESOURCE_NOT_FOUND", Some(false)),
    ("INVALID_RESOURCE", Some(false)),
    ("RESOURCE_ACCESS_DENIED", Some(false)),
    ("INSUFFICIENT_COMPUTE", Some(false)),
    ("TRANSIENT_ERROR", Some(true)),
    ("INTERNAL_ERROR", Some(true)),
    ("UNKNOWN_ERROR", None),
];

/// The alias a statement names an associated configured table by (model shape
/// `TableAlias`, pattern `^[a-zA-Z0-9_](([a-zA-Z0-9_ ]+-)*([a-zA-Z0-9_ ]+))?$`).
const ASSOCIATION_ALIAS: &str = "orders_assoc";

/// What a row of the failure-category table holds before it submits its query.
enum FailureSetup {
    /// Nothing at all: the membership associates no configured table, so a statement that
    /// names one reads a relation the collaboration does not expose.
    Bare,
    /// No analysis template under the membership, queried by an ARN in the membership's
    /// own namespace.
    AbsentTemplate,
    /// An analysis template whose analysis is held as ARTIFACTS, queried by its ARN.
    ArtifactsTemplate,
    /// A configured table of the given `TableReferenceType`, associated under
    /// [`ASSOCIATION_ALIAS`], carrying a `list` analysis rule only where `analysis_rule`
    /// is set.
    Association {
        table_reference_type: &'static str,
        analysis_rule: bool,
    },
}

/// One row: what the membership holds, the statement submitted against it, and the
/// category the query must report having failed in.
struct FailureCase {
    label: &'static str,
    setup: FailureSetup,
    /// The inline SQL the row submits; a row querying by analysis template passes `None`.
    sql: Option<&'static str>,
    expected_code: &'static str,
}

/// What the documented catalog says about running a query that failed in `code` again.
fn documented_retryable(code: &str) -> Option<bool> {
    DOCUMENTED_CATEGORIES
        .iter()
        .find(|(category, _)| *category == code)
        .unwrap_or_else(|| panic!("{code:?} is not a documented ProtectedQueryError code"))
        .1
}

/// Associate a configured table of `table_reference_type` under [`ASSOCIATION_ALIAS`],
/// with a `list` analysis rule where `analysis_rule` is set.
async fn given_failure_association(
    emulator: &Emulator,
    label: &str,
    membership_id: &str,
    table_reference_type: &str,
    analysis_rule: bool,
) {
    let (status, created) = emulator
        .create_configured_table(create_body(
            "failure_category_table",
            table_reference_type,
            "DIRECT_QUERY",
            None,
            COLUMN_NAMES.len(),
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateConfiguredTable must succeed to set up the case, got {status} \
         with {created}"
    );
    let configured_table_id = created["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no configuredTable.id in {created}"))
        .to_string();

    if analysis_rule {
        let (status, rule) = emulator
            .create_analysis_rule(
                &configured_table_id,
                "LIST",
                json!({
                    "v1": {
                        "list": {
                            "joinColumns": [COLUMN_NAMES[0]],
                            "listColumns": [COLUMN_NAMES[1]],
                        },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateConfiguredTableAnalysisRule must succeed to set up the case, \
             got {status} with {rule}"
        );
    }

    let (status, associated) = emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/configuredTableAssociations"),
            Some(json!({
                "configuredTableIdentifier": configured_table_id,
                "name": ASSOCIATION_ALIAS,
                "roleArn": format!("arn:aws:iam::{}:role/clean-rooms-failure-category", account_id()),
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateConfiguredTableAssociation must succeed to set up the case, got \
         {status} with {associated}"
    );
}

/// Set a row up and answer the `sqlParameters` its `StartProtectedQuery` carries.
async fn failure_query_parameters(
    emulator: &Emulator,
    case: &FailureCase,
    membership_id: &str,
) -> Value {
    match case.setup {
        FailureSetup::Bare => {}
        FailureSetup::AbsentTemplate => {
            return json!({ "analysisTemplateArn": analysis_template_arn(membership_id) })
        }
        FailureSetup::ArtifactsTemplate => {
            let (status, created) = create_analysis_template(
                emulator,
                membership_id,
                json!({
                    "name": "artifacts_analysis",
                    "format": "PYSPARK_1_0",
                    "source": artifacts_source(),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: CreateAnalysisTemplate must succeed to set up the case, got {status} \
                 with {created}",
                case.label
            );
            let arn = created["analysisTemplate"]["arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {:?}: no analysisTemplate.arn in {created}", case.label)
                })
                .to_string();
            return json!({ "analysisTemplateArn": arn });
        }
        FailureSetup::Association {
            table_reference_type,
            analysis_rule,
        } => {
            given_failure_association(
                emulator,
                case.label,
                membership_id,
                table_reference_type,
                analysis_rule,
            )
            .await
        }
    }
    json!({
        "queryString": case
            .sql
            .unwrap_or_else(|| panic!("case {:?}: the row carries no SQL", case.label)),
    })
}

/// A query that could not run reports the CATEGORY of its failure out of the catalog the
/// model documents, and whether running the same query again might succeed.
///
/// The table varies what the rule generalises over — WHICH thing about the query failed:
/// the statement (unreadable by the parser, or refused by the engine), a resource it names
/// that nothing holds (an analysis template, the catalog table behind an associated
/// configured table), and a resource that is there but cannot be used as it stands (a
/// template whose analysis is artifacts rather than SQL, a configured table with no
/// analysis rule, a table reference into a store the collaboration cannot read).
#[tokio::test]
async fn a_failed_protected_query_reports_its_category_and_whether_a_retry_might_succeed() {
    let cases: Vec<FailureCase> = vec![
        FailureCase {
            label: "a statement naming a relation the membership associates nothing under",
            setup: FailureSetup::Bare,
            sql: Some("SELECT COUNT(*) FROM orders"),
            expected_code: "MALFORMED_QUERY",
        },
        FailureCase {
            label: "a statement the parser cannot read as a query at all",
            setup: FailureSetup::Bare,
            sql: Some("SELECT FROM WHERE"),
            expected_code: "MALFORMED_QUERY",
        },
        FailureCase {
            label: "an analysis template ARN nothing holds",
            setup: FailureSetup::AbsentTemplate,
            sql: None,
            expected_code: "RESOURCE_NOT_FOUND",
        },
        FailureCase {
            label: "an analysis template whose analysis is artifacts rather than SQL",
            setup: FailureSetup::ArtifactsTemplate,
            sql: None,
            expected_code: "INVALID_RESOURCE",
        },
        FailureCase {
            label: "an associated configured table carrying no analysis rule",
            setup: FailureSetup::Association {
                table_reference_type: "GLUE",
                analysis_rule: false,
            },
            sql: Some("SELECT amount FROM orders_assoc"),
            expected_code: "INVALID_RESOURCE",
        },
        FailureCase {
            label: "an associated configured table referencing a catalog table nothing holds",
            setup: FailureSetup::Association {
                table_reference_type: "GLUE",
                analysis_rule: true,
            },
            sql: Some("SELECT amount FROM orders_assoc"),
            expected_code: "RESOURCE_NOT_FOUND",
        },
        FailureCase {
            label: "an associated configured table referencing a store with no data behind it",
            setup: FailureSetup::Association {
                table_reference_type: "SNOWFLAKE",
                analysis_rule: true,
            },
            sql: Some("SELECT amount FROM orders_assoc"),
            expected_code: "INVALID_RESOURCE",
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        let label = case.label;
        // One membership per row: an association is held under the membership that made
        // it, so each row's own setup is the only state its query reads.
        let emulator = Emulator::new();
        let membership = simple_membership(&emulator, &format!("failure-category-{row}")).await;
        let membership_id = membership["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no membership id in {membership}"))
            .to_string();
        let sql_parameters = failure_query_parameters(&emulator, case, &membership_id).await;

        let (status, started) = start_query(
            &emulator,
            &membership_id,
            json!({ "type": "SQL", "sqlParameters": sql_parameters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartProtectedQuery 200, got {status} with {started}"
        );
        let query_id = started["protectedQuery"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no protectedQuery.id in {started}"))
            .to_string();

        let query = poll_until_terminal(&emulator, &membership_id, &query_id).await;
        assert_eq!(
            query["status"].as_str(),
            Some("FAILED"),
            "case {label:?}: a query that could not run settles in FAILED, got {query}"
        );
        assert_eq!(
            query["error"]["code"].as_str(),
            Some(case.expected_code),
            "case {label:?}: expected failure category {:?}, got {:?}",
            case.expected_code,
            query["error"]
        );
        assert!(
            query["error"]["message"]
                .as_str()
                .is_some_and(|message| !message.trim().is_empty()),
            "case {label:?}: the failure says why, got {:?}",
            query["error"]
        );
        assert_eq!(
            query["error"]["retry"]["retryable"].as_bool(),
            documented_retryable(case.expected_code),
            "case {label:?}: expected the retry advice the category {:?} carries, got {:?}",
            case.expected_code,
            query["error"]
        );
        assert!(
            query.get("result").is_none() && query.get("statistics").is_none(),
            "case {label:?}: a query that did not run reports no result and no statistics, got \
             {query}"
        );

        // Reading again reports the same failure: the error is the stored outcome of the
        // run, not something a read decides.
        let (status, again) = get_query(&emulator, &membership_id, &query_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetProtectedQuery 200, got {status} with {again}"
        );
        assert_eq!(
            again["protectedQuery"], query,
            "case {label:?}: a second read reports the same failed query, got {again}"
        );
    }
}

/// The row that must carry no failure at all: a statement reading no relation is one the
/// engine answers, so the query succeeds and reports neither a category nor retry advice.
#[tokio::test]
async fn a_query_that_runs_reports_no_failure_category_at_all() {
    let emulator = Emulator::new();
    let membership = simple_membership(&emulator, "failure-category-none").await;
    let membership_id = membership["id"].as_str().expect("membership id").to_string();

    let (status, started) = start_query(
        &emulator,
        &membership_id,
        json!({ "type": "SQL", "sqlParameters": { "queryString": "SELECT 1" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected StartProtectedQuery 200, got {status} with {started}"
    );
    let query_id = started["protectedQuery"]["id"]
        .as_str()
        .expect("protected query id")
        .to_string();

    let query = poll_until_terminal(&emulator, &membership_id, &query_id).await;
    assert_eq!(
        query["status"].as_str(),
        Some("SUCCESS"),
        "a statement the engine answers succeeds, got {query}"
    );
    assert!(
        query.get("error").is_none(),
        "a query that ran carries no error, got {query}"
    );
}

/// The retry advice is a property of the CATEGORY, so every category the model documents
/// carries the one its own definition gives it — including the categories no end-to-end
/// case of this suite reaches.
#[test]
fn every_documented_failure_category_carries_the_retry_advice_its_definition_gives_it() {
    assert_eq!(
        cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_RETRYABILITY.len(),
        DOCUMENTED_CATEGORIES.len(),
        "the service covers every documented failure category, got {:?}",
        cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_RETRYABILITY
    );
    for (code, retryable) in DOCUMENTED_CATEGORIES {
        assert!(
            cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_RETRYABILITY.contains(&(code, retryable)),
            "case {code:?}: the service declares the documented retryability {retryable:?}, got \
             {:?}",
            cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_RETRYABILITY
        );
        let error = cleanrooms_state::protected_query_error(code, format!("a {code} failure"));
        assert_eq!(
            error.code, code,
            "case {code:?}: the error reports the category it was built for, got {error:?}"
        );
        assert_eq!(
            error.retry.as_ref().and_then(|retry| retry.retryable),
            retryable,
            "case {code:?}: expected retryable {retryable:?}, got {:?}",
            error.retry
        );
        assert_eq!(
            error.retry.is_some(),
            retryable.is_some(),
            "case {code:?}: a category with no known transience reports no retry at all, got \
             {:?}",
            error.retry
        );
    }

    // A category outside the documented catalog carries no retry advice: nothing is
    // invented for a code the model says nothing about.
    let unknown = cleanrooms_state::protected_query_error(
        "NOT_A_DOCUMENTED_CATEGORY",
        "an unclassified failure".to_string(),
    );
    assert!(
        unknown.retry.is_none(),
        "an undocumented category carries no retry advice, got {:?}",
        unknown.retry
    );
    // The advice is reported through the model's own `RetryInfo`, so a transient category
    // reports the retryable failure that shape describes.
    let transient = cleanrooms_state::protected_query_error_retry(
        cleanrooms_state::PROTECTED_QUERY_ERROR_CODE_TRANSIENT_ERROR,
    );
    assert!(
        matches!(
            transient,
            Some(RetryInfo {
                retryable: Some(true)
            })
        ),
        "a transient category reports a retryable failure, got {transient:?}"
    );
}
