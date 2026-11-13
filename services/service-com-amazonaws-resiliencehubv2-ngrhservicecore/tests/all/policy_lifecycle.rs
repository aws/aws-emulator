// SPDX-License-Identifier: CC0-1.0

//! The resilience-`Policy` lifecycle, end to end over HTTP: `CreatePolicy` ->
//! `GetPolicy`.
//!
//! The rule under test: `CreatePolicy` STORES the policy it was asked for and answers a
//! `Policy` projected from that stored record alone — every member the request supplied
//! echoed unchanged, `sharingEnabled` reporting the stored flag, `policyArn` minted in
//! the Region the request is signed for, `associatedServiceCount` counted from the
//! services that reference the ARN, and `createdAt` / `updatedAt` from the create
//! instant — so a later `GetPolicy` of that ARN answers the same projection, a replayed
//! `clientToken` answers the policy the first call created, a name already taken is
//! refused `ConflictException`, and an ARN naming no stored policy is refused
//! `ResourceNotFoundException`.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the
//! modelled constraints, the restJson1 (de)serializers and the HTTP status codes are
//! exercised exactly as a client would exercise them.
//!
//! The suites are TABLES: each row is one input of the class the rule covers (which
//! optional blocks are present, target values at their modelled bounds, each disaster
//! recovery approach, tag count, description length, name length, Region), not one
//! recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    POLICY_RESOURCE, POLICY_RESOURCE_TYPE,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
const CREATE_POLICY: &str = "/v2/create-policy";
const GET_POLICY: &str = "/v2/get-policy";
const CREATE_SERVICE: &str = "/v2/create-service";
const DELETE_SERVICE: &str = "/v2/delete-service";

/// The Region these suites sign for unless a case names another one.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The members `Policy` shares with `CreatePolicyRequest`. A create echoes each one it
/// was given, unchanged, and reports none it was not.
///
/// SOURCE: `smithy_query(query="operation", operation="CreatePolicy")` — the request
/// members other than `clientToken` (which is an `@idempotencyToken`, not a stored
/// member) and `sharingEnabled` (a flag with a stored default, asserted separately).
const ECHOED_MEMBERS: [&str; 8] = [
    "name",
    "description",
    "availabilitySlo",
    "multiAz",
    "multiRegion",
    "dataRecovery",
    "kmsKeyId",
    TAG_MAP_MEMBER,
];

/// `Policy$tags` — the member carrying the policy's tag map, which is reported whether or
/// not the policy carries any tags.
const TAG_MAP_MEMBER: &str = "tags";

/// The values `MultiAzDisasterRecoveryApproach` and `MultiRegionDisasterRecoveryApproach`
/// accept on the wire.
///
/// SOURCE: `smithy_query(query="shape",
/// shape="MultiAzDisasterRecoveryApproach,MultiRegionDisasterRecoveryApproach")` — both
/// enums carry exactly these five values.
const DISASTER_RECOVERY_APPROACHES: [&str; 5] = [
    "ACTIVE_ACTIVE",
    "HOT_STANDBY",
    "WARM_STANDBY",
    "PILOT_LIGHT",
    "BACKUP_AND_RESTORE",
];

/// `EntityName`'s modelled bounds, which `CreatePolicy$name` targets.
///
/// SOURCE: `smithy_query(query="shape", shape="EntityName")` — `length {min: 2, max: 60}`,
/// pattern `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`.
const ENTITY_NAME_MIN_LENGTH: usize = 2;
const ENTITY_NAME_MAX_LENGTH: usize = 60;

/// `LongDescription`'s modelled ceiling, which `CreatePolicy$description` targets.
///
/// SOURCE: `smithy_query(query="shape", shape="LongDescription")` — `length {max: 615}`.
const LONG_DESCRIPTION_MAX_LENGTH: usize = 615;

/// `AvailabilitySlo$target`'s modelled range.
///
/// SOURCE: `smithy_query(query="shape", shape="AvailabilitySlo")` — `range {min: 0,
/// max: 100}`.
const AVAILABILITY_SLO_MIN: f64 = 0.0;
const AVAILABILITY_SLO_MAX: f64 = 100.0;

/// `Arn`'s modelled minimum length, which every minted `policyArn` must satisfy.
///
/// SOURCE: `smithy_query(query="shape", shape="Arn")` — `length {min: 31}`.
const ARN_MIN_LENGTH: usize = 31;

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`) and `region`. Signature verification is off for these tests, so
/// the value only has to parse.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{region}\
         /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
         Signature=0123456789abcdef"
    )
}

async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

async fn create_policy(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", CREATE_POLICY, Some(body)).await
}

async fn get_policy(router: &MultiServiceRouter, policy_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{GET_POLICY}?policyArn={policy_arn}"),
        None,
    )
    .await
}

/// Create a service in `REGION`, optionally referencing `policy_arn`, and answer its ARN.
async fn create_service(
    router: &MultiServiceRouter,
    name: &str,
    policy_arn: Option<&str>,
) -> String {
    let mut body = json!({
        "name": name,
        "regions": [REGION],
        "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
    });
    if let Some(policy_arn) = policy_arn {
        body["policyArn"] = json!(policy_arn);
    }
    let (status, answered) = call(router, "POST", CREATE_SERVICE, Some(body)).await;
    assert_eq!(
        status, 200,
        "creating service {name:?}: expected HTTP 200, got {status} {answered:?}"
    );
    answered["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {answered:?}"))
        .to_string()
}

/// The modelled error code of a refusal, from the restJson1 `__type` discriminator.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `policy` member of a successful create or read.
fn policy_of(case: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} {body:?}"
    );
    let policy = &body["policy"];
    assert!(
        policy.is_object(),
        "case {case:?}: the @required policy member is absent from {body:?}"
    );
    policy.clone()
}

fn policy_arn_of(case: &str, policy: &Value) -> String {
    policy["policyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no policyArn in {policy:?}"))
        .to_string()
}

/// One row of the create table: the body of a single `CreatePolicy` call.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    body: Value,
}

/// The create table.
///
/// Rows differ along which optional blocks are present (none, one, all four), the target
/// values (both ends of `AvailabilitySlo$target`'s range, zero and large RTO/RPO minutes,
/// a backup interval), each modelled disaster recovery approach, `sharingEnabled`
/// (absent, `false`, `true`), tag count (none, one, five) and the length of `name` and
/// `description` (both modelled extremes).
fn create_rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "only the required name, at EntityName's minimum length".to_string(),
            body: json!({"name": "Ab"}),
        },
        Row {
            case: "longest name and longest description".to_string(),
            body: json!({
                "name": format!("Z{}", "y".repeat(ENTITY_NAME_MAX_LENGTH - 1)),
                "description": "d".repeat(LONG_DESCRIPTION_MAX_LENGTH),
            }),
        },
        Row {
            case: "empty description and the availability SLO at its upper bound".to_string(),
            body: json!({
                "name": "slo-upper-bound",
                "description": "",
                "availabilitySlo": {"target": AVAILABILITY_SLO_MAX},
            }),
        },
        Row {
            case: "availability SLO at its lower bound".to_string(),
            body: json!({
                "name": "slo_lower_bound",
                "availabilitySlo": {"target": AVAILABILITY_SLO_MIN},
            }),
        },
        Row {
            case: "data recovery targets only".to_string(),
            body: json!({
                "name": "backups-every-15-minutes",
                "dataRecovery": {"timeBetweenBackupsInMinutes": 15},
            }),
        },
        Row {
            case: "every target block together, sharing enabled, one tag".to_string(),
            body: json!({
                "name": "Tier0-Payments",
                "description": "Targets for the tier-0 payments path",
                "availabilitySlo": {"target": 99.95},
                "multiAz": {
                    "rtoInMinutes": 15,
                    "rpoInMinutes": 5,
                    "disasterRecoveryApproach": "HOT_STANDBY",
                },
                "multiRegion": {
                    "rtoInMinutes": 240,
                    "rpoInMinutes": 60,
                    "disasterRecoveryApproach": "WARM_STANDBY",
                },
                "dataRecovery": {"timeBetweenBackupsInMinutes": 1},
                "sharingEnabled": true,
                "kmsKeyId": "alias/resilience",
                "tags": {"team": "payments"},
            }),
        },
        Row {
            case: "sharing disabled explicitly, five tags, a key ARN".to_string(),
            body: json!({
                "name": "Shared_Off",
                "sharingEnabled": false,
                "kmsKeyId": format!(
                    "arn:aws:kms:{REGION}:{ACCOUNT}:key/\
                     3f2504e0-4f89-11d3-9a0c-0305e82c3301"
                ),
                "tags": {
                    "team": "platform",
                    "env": "prod",
                    "tier": "0",
                    "owner": "resilience",
                    "cost-center": "4711",
                },
            }),
        },
        Row {
            case: "zero RTO and RPO minutes, at the range's lower bound".to_string(),
            body: json!({
                "name": "Zero-Targets",
                "multiAz": {"rtoInMinutes": 0, "rpoInMinutes": 0},
                "multiRegion": {"rtoInMinutes": 0, "rpoInMinutes": 0},
            }),
        },
    ];
    // One row per modelled disaster recovery approach, on both the multi-AZ and the
    // multi-Region block, so the whole published value set is exercised rather than the
    // two approaches the rows above happen to name.
    for (index, approach) in DISASTER_RECOVERY_APPROACHES.iter().enumerate() {
        rows.push(Row {
            case: format!("disaster recovery approach {approach}"),
            body: json!({
                "name": format!("DR-{approach}-{index}"),
                "multiAz": {
                    "rtoInMinutes": 60 * (index as i64 + 1),
                    "disasterRecoveryApproach": approach,
                },
                "multiRegion": {
                    "rpoInMinutes": 30 * (index as i64 + 1),
                    "disasterRecoveryApproach": approach,
                },
            }),
        });
    }
    rows
}

/// Assert `policy` is `request` as stored: every shared member echoed, the members the
/// request did not supply absent, and the derived members as the rule requires.
fn assert_projects_request(case: &str, request: &Value, policy: &Value) {
    for member in ECHOED_MEMBERS {
        let supplied = &request[member];
        let answered = &policy[member];
        // A tag map is the SET of tags the policy carries, so a policy created without
        // tags carries the EMPTY map; every other member the request did not supply is
        // absent.
        let expected = match (member, supplied.is_null()) {
            (TAG_MAP_MEMBER, true) => json!({}),
            (_, true) => Value::Null,
            _ => supplied.clone(),
        };
        if expected.is_null() {
            assert!(
                answered.is_null(),
                "case {case:?}: {member} was not supplied, so it must be absent, got \
                 {answered:?}"
            );
        } else {
            assert_eq!(
                answered, &expected,
                "case {case:?}: expected {member} {expected:?}, got {answered:?}"
            );
        }
    }
    // A policy created without `sharingEnabled` has cross-account sharing off, so the
    // flag reports the stored state rather than being withheld.
    let expected_sharing = request["sharingEnabled"].as_bool().unwrap_or(false);
    assert_eq!(
        policy["sharingEnabled"].as_bool(),
        Some(expected_sharing),
        "case {case:?}: expected sharingEnabled {expected_sharing}, got {:?}",
        policy["sharingEnabled"]
    );
    // Nothing in this service shares a policy with an organization, so no organization
    // is named.
    assert!(
        policy["organizationId"].is_null(),
        "case {case:?}: expected no organizationId, got {:?}",
        policy["organizationId"]
    );
    let created = policy["createdAt"].as_f64();
    let updated = policy["updatedAt"].as_f64();
    assert!(
        created.is_some() && created == updated,
        "case {case:?}: expected createdAt and updatedAt to be the same create instant, \
         got {:?} and {:?}",
        policy["createdAt"],
        policy["updatedAt"]
    );
    assert_minted_arn(case, REGION, &policy_arn_of(case, policy));
}

/// Assert `policy_arn` has the form a minted policy ARN takes in `region`.
fn assert_minted_arn(case: &str, region: &str, policy_arn: &str) {
    let prefix = format!("arn:aws:resiliencehub:{region}:{ACCOUNT}:{POLICY_RESOURCE}/");
    let resource_id = policy_arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a policy ARN starting {prefix:?}, got {policy_arn:?}")
    });
    assert!(
        policy_arn.len() >= ARN_MIN_LENGTH,
        "case {case:?}: expected an ARN of at least {ARN_MIN_LENGTH} characters, got \
         {policy_arn:?}"
    );
    let groups: Vec<&str> = resource_id.split('-').collect();
    let shaped = groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        shaped,
        "case {case:?}: expected the ARN's resource id to be a UUID, got {resource_id:?}"
    );
}

#[tokio::test]
async fn create_policy_stores_every_row_and_reads_it_back() {
    let router = emulator();
    for row in create_rows() {
        let case = row.case.as_str();
        let (status, body) = create_policy(&router, row.body.clone()).await;
        let created = policy_of(case, status, &body);
        assert_projects_request(case, &row.body, &created);

        // A brand-new policy has no service referencing it yet, and the count is the
        // number of such services rather than an absent member.
        assert_eq!(
            created["associatedServiceCount"].as_i64(),
            Some(0),
            "case {case:?}: expected associatedServiceCount 0 for a new policy, got {:?}",
            created["associatedServiceCount"]
        );

        // The round trip: the reader answers the same projection, from the same record.
        let policy_arn = policy_arn_of(case, &created);
        let (read_status, read_body) = get_policy(&router, &policy_arn).await;
        let read = policy_of(case, read_status, &read_body);
        assert_eq!(
            read, created,
            "case {case:?}: GetPolicy answered a different policy than CreatePolicy stored"
        );
    }
}

#[tokio::test]
async fn get_policy_is_a_read_and_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let (status, body) = create_policy(
        &router,
        json!({"name": "Read-Twice", "multiAz": {"rtoInMinutes": 30}}),
    )
    .await;
    let created = policy_of("read twice", status, &body);
    let policy_arn = policy_arn_of("read twice", &created);
    let (first_status, first) = get_policy(&router, &policy_arn).await;
    let (second_status, second) = get_policy(&router, &policy_arn).await;
    let first = policy_of("first read", first_status, &first);
    let second = policy_of("second read", second_status, &second);
    assert_eq!(
        first, second,
        "case {:?}: two consecutive reads answered differently",
        "read twice"
    );
    assert_eq!(
        second, created,
        "case {:?}: a read changed what the next caller sees",
        "read twice"
    );
}

#[tokio::test]
async fn policy_arns_are_minted_in_the_region_the_request_is_signed_for() {
    // Rows differ along the Region's form: a two-segment commercial Region, a
    // four-segment one and a Region in another partition's naming shape.
    let router = emulator();
    for (index, region) in ["us-east-1", "eu-west-2", "ap-southeast-3", "us-gov-west-1"]
        .into_iter()
        .enumerate()
    {
        let case = format!("signed for {region}");
        let (status, body) = call_in(
            &router,
            region,
            "POST",
            CREATE_POLICY,
            Some(json!({"name": format!("Regional-{index}")})),
        )
        .await;
        let created = policy_of(&case, status, &body);
        assert_minted_arn(&case, region, &policy_arn_of(&case, &created));
    }
}

#[tokio::test]
async fn a_replayed_client_token_answers_the_first_policy() {
    // Rows differ along the token's form and length: `ClientToken` is `len 1..63` with
    // pattern `^[A-Za-z0-9_.-]{0,63}$` (`smithy_query(query="shape",
    // shape="ClientToken")`), so the shortest, a UUID-shaped one, one carrying every
    // allowed punctuation class and the longest are all tokens.
    let router = emulator();
    let tokens = [
        "t".to_string(),
        "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_string(),
        "token_1.2-3".to_string(),
        "k".repeat(63),
    ];
    for (index, token) in tokens.iter().enumerate() {
        let case = format!("client token of {} characters", token.len());
        let first_body = json!({
            "name": format!("Replayed-{index}"),
            "multiAz": {"rtoInMinutes": 10},
            "clientToken": token,
        });
        let (status, body) = create_policy(&router, first_body.clone()).await;
        let first = policy_of(&case, status, &body);

        // The same token with a DIFFERENT payload is still a replay of the first call:
        // it answers the stored policy and creates no second one.
        let (replay_status, replay_body) = create_policy(
            &router,
            json!({
                "name": format!("Replayed-{index}-other"),
                "multiAz": {"rtoInMinutes": 99},
                "clientToken": token,
            }),
        )
        .await;
        let replayed = policy_of(&case, replay_status, &replay_body);
        assert_eq!(
            replayed, first,
            "case {case:?}: a replayed clientToken must answer the first policy"
        );

        // A DIFFERENT token under a different name is a different policy. The token is
        // the same length, so it stays inside `ClientToken`'s bounds.
        let distinct = format!("Z{}", &token[1..]);
        let (other_status, other_body) = create_policy(
            &router,
            json!({
                "name": format!("Distinct-{index}"),
                "clientToken": distinct,
            }),
        )
        .await;
        let other = policy_of(&case, other_status, &other_body);
        assert_ne!(
            policy_arn_of(&case, &other),
            policy_arn_of(&case, &first),
            "case {case:?}: a fresh clientToken must create a new policy"
        );
    }
}

#[tokio::test]
async fn a_name_already_taken_is_refused_as_a_conflict() {
    // Rows differ along the name's form and length, so the rule is asserted over the
    // class rather than on one spelling.
    let router = emulator();
    let names = [
        "Ab".to_string(),
        "policy_with-punctuation".to_string(),
        "0leading-digit".to_string(),
        format!("W{}", "x".repeat(ENTITY_NAME_MAX_LENGTH - 1)),
    ];
    for name in names {
        assert!(
            name.len() >= ENTITY_NAME_MIN_LENGTH && name.len() <= ENTITY_NAME_MAX_LENGTH,
            "case {name:?}: the row must itself satisfy EntityName's bounds"
        );
        let (status, body) = create_policy(&router, json!({"name": name})).await;
        let first = policy_of(&name, status, &body);
        let policy_arn = policy_arn_of(&name, &first);

        let (conflict_status, conflict) =
            create_policy(&router, json!({"name": name, "description": "second"})).await;
        assert_eq!(
            conflict_status, 409,
            "case {name:?}: expected HTTP 409 for a name already taken, got \
             {conflict_status} {conflict:?}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {name:?}: expected ConflictException, got {conflict:?}"
        );
        let message = conflict["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&name) && message.contains(&policy_arn),
            "case {name:?}: expected the refusal to name the policy and the ARN already \
             holding the name, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_stored_policy_is_not_found() {
    // Rows differ along the ARN's form: another account's policy, another Region's, a
    // different resource type and a well-formed ARN of another service. Each satisfies
    // `Arn`'s modelled pattern, so each reaches the handler.
    let router = emulator();
    let arns = [
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{POLICY_RESOURCE}/missing"),
        format!("arn:aws:resiliencehub:eu-central-1:{ACCOUNT}:{POLICY_RESOURCE}/absent"),
        format!("arn:aws:resiliencehub:{REGION}:210987654321:{POLICY_RESOURCE}/elsewhere"),
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:service/not-a-policy"),
    ];
    for arn in arns {
        let (status, body) = get_policy(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404 for an ARN naming no policy, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_refuses_as_not_found(&arn, &body);
    }
}

/// Assert `body` identifies the missing policy the way the rule requires: `resourceId` is
/// the resource segment of the requested ARN, `resourceType` names the resource's type,
/// and `message` is that type, " not found: " and the same identifier.
fn assert_refuses_as_not_found(arn: &str, body: &Value) {
    let resource_id = arn.splitn(6, ':').nth(5).unwrap_or(arn);
    assert_eq!(
        body["resourceId"].as_str(),
        Some(resource_id),
        "case {arn:?}: expected the refusal to identify the policy as {resource_id:?}, \
         got {:?}",
        body["resourceId"]
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some(POLICY_RESOURCE_TYPE),
        "case {arn:?}: expected resourceType {POLICY_RESOURCE_TYPE:?}, got {:?}",
        body["resourceType"]
    );
    let expected = format!("{POLICY_RESOURCE_TYPE} not found: {resource_id}");
    assert_eq!(
        body["message"].as_str(),
        Some(expected.as_str()),
        "case {arn:?}: expected message {expected:?}, got {:?}",
        body["message"]
    );
}

#[tokio::test]
async fn associated_service_count_counts_the_services_that_reference_the_policy() {
    // Rows differ along the count: no service, one, and several. Each count is read back
    // through GetPolicy, then one service is removed so the count follows the store
    // rather than a tally kept beside it.
    let router = emulator();
    for attached in [0_usize, 1, 3] {
        let case = format!("{attached} service(s) attached");
        let (status, body) = create_policy(
            &router,
            json!({"name": format!("Counted-{attached}"), "multiAz": {"rtoInMinutes": 20}}),
        )
        .await;
        let created = policy_of(&case, status, &body);
        let policy_arn = policy_arn_of(&case, &created);

        let mut services = Vec::new();
        for index in 0..attached {
            services.push(
                create_service(
                    &router,
                    &format!("Consumer-{attached}-{index}"),
                    Some(&policy_arn),
                )
                .await,
            );
        }
        // A service that references NO policy must not be counted against this one.
        create_service(&router, &format!("Unattached-{attached}"), None).await;

        let (read_status, read_body) = get_policy(&router, &policy_arn).await;
        let read = policy_of(&case, read_status, &read_body);
        assert_eq!(
            read["associatedServiceCount"].as_i64(),
            Some(attached as i64),
            "case {case:?}: expected associatedServiceCount {attached}, got {:?}",
            read["associatedServiceCount"]
        );

        if let Some(service_arn) = services.pop() {
            let (delete_status, delete_body) = call(
                &router,
                "POST",
                DELETE_SERVICE,
                Some(json!({"serviceArn": service_arn})),
            )
            .await;
            assert_eq!(
                delete_status, 200,
                "case {case:?}: deleting {service_arn} answered {delete_status} \
                 {delete_body:?}"
            );
            let (after_status, after_body) = get_policy(&router, &policy_arn).await;
            let after = policy_of(&case, after_status, &after_body);
            let expected = attached as i64 - 1;
            assert_eq!(
                after["associatedServiceCount"].as_i64(),
                Some(expected),
                "case {case:?}: after deleting one service, expected \
                 associatedServiceCount {expected}, got {:?}",
                after["associatedServiceCount"]
            );
        }
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    // Rows differ along which modelled bound is passed: `name` one character under and
    // one over `EntityName`'s length, a `name` the pattern refuses, a `description` one
    // character past `LongDescription`'s ceiling, and an availability target past each
    // end of its range. The bound itself is exercised by the create table above.
    let router = emulator();
    let rows = [
        (
            "name one character under EntityName's minimum",
            json!({"name": "A".repeat(ENTITY_NAME_MIN_LENGTH - 1)}),
        ),
        (
            "name one character over EntityName's maximum",
            json!({"name": format!("Q{}", "z".repeat(ENTITY_NAME_MAX_LENGTH))}),
        ),
        (
            "name the pattern refuses",
            json!({"name": "bad name has spaces!"}),
        ),
        (
            "description one character over its ceiling",
            json!({
                "name": "Over-Description",
                "description": "d".repeat(LONG_DESCRIPTION_MAX_LENGTH + 1),
            }),
        ),
        (
            "availability target over its range",
            json!({
                "name": "Over-Slo",
                "availabilitySlo": {"target": AVAILABILITY_SLO_MAX + 0.5},
            }),
        ),
        (
            "availability target under its range",
            json!({
                "name": "Under-Slo",
                "availabilitySlo": {"target": AVAILABILITY_SLO_MIN - 0.5},
            }),
        ),
        (
            "negative RTO minutes",
            json!({"name": "Negative-Rto", "multiAz": {"rtoInMinutes": -1}}),
        ),
    ];
    for (case, body) in rows {
        let (status, answered) = create_policy(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a request outside a modelled \
             constraint, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        // Nothing was stored: a name the refused request carried that EntityName itself
        // accepts is still free.
        if let Some(name) = body["name"].as_str().filter(|name| is_entity_name(name)) {
            let (retry_status, retry) = create_policy(&router, json!({"name": name})).await;
            assert_eq!(
                retry_status, 200,
                "case {case:?}: the refused create must have stored nothing, so \
                 {name:?} is still free, got {retry_status} {retry:?}"
            );
        }
    }
}

/// Whether `candidate` satisfies `EntityName`'s modelled length and pattern (`len 2..60`,
/// `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`) — i.e. whether a create carrying it is refused
/// for some OTHER reason than the name itself.
fn is_entity_name(candidate: &str) -> bool {
    candidate.len() >= ENTITY_NAME_MIN_LENGTH
        && candidate.len() <= ENTITY_NAME_MAX_LENGTH
        && candidate
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
