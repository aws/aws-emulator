// SPDX-License-Identifier: CC0-1.0

//! The `System` lifecycle, end to end over HTTP: `CreateSystem` -> `GetSystem` ->
//! `UpdateSystem` -> `DeleteSystem`, and the user journeys a system holds.
//!
//! The rules under test:
//!
//! 1. `CreateSystem` STORES the system it was asked for and answers a `System` projected
//!    from that stored record alone — every member the request supplied echoed unchanged,
//!    `sharingEnabled` reporting the stored flag, `tags` always reported (empty map
//!    included), `systemArn` minted in the Region the request is signed for with the
//!    `systemId` it carries, and `createdAt` / `updatedAt` from the create instant — so a
//!    later `GetSystem` of that ARN answers the same projection.
//! 2. `clientToken` is an `@idempotencyToken`: a repeat of the create it was accepted for
//!    answers the system that call created and creates no second one, while the same token
//!    offered for DIFFERENT creation parameters is refused `ConflictException` and writes
//!    nothing.
//! 3. A name a live system already holds is refused `ConflictException`.
//! 4. A well-formed ARN naming no stored system is refused `ResourceNotFoundException`,
//!    which identifies the system by its own `systemId`.
//! 5. `UpdateSystem` applies the members it carries, leaves the rest untouched, and the
//!    next read reports what it stored.
//! 6. `DeleteSystem` removes the system — refused `ConflictException` while a service is
//!    still associated with it — after which every reader answers 404 and its name is free.
//! 7. A user journey is created within a system, reads back unchanged, and goes with the
//!    system when that is deleted.
//! 8. Every identifier this service mints is a lower-case version-4 UUID.
//!
//! Every suite drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are exercised
//! exactly as a client would exercise them.
//!
//! The suites are TABLES: each row is one input of the class the rule covers (which
//! optional members are present, name and description length at their modelled bounds, tag
//! count, each `kmsKeyId` form, which parameter a reused token differs in, Region), not one
//! recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::{
    POLICY_RESOURCE, SERVICE_RESOURCE, SYSTEM_RESOURCE, SYSTEM_RESOURCE_TYPE,
};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
const CREATE_SYSTEM: &str = "/v2/create-system";
const GET_SYSTEM: &str = "/v2/get-system";
const TAGS: &str = "/v2/tags";

/// The Region these suites sign for unless a case names another one.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// `EntityName`'s modelled bounds, which `CreateSystem$name` targets.
///
/// SOURCE: `smithy_query(query="shape", shape="EntityName")` — `length {min: 2, max: 60}`,
/// pattern `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`.
const ENTITY_NAME_MIN_LENGTH: usize = 2;
const ENTITY_NAME_MAX_LENGTH: usize = 60;

/// `EntityDescription`'s modelled ceiling, which `CreateSystem$description` targets.
///
/// SOURCE: `smithy_query(query="operation", operation="CreateSystem")` — `description`
/// targets `EntityDescription`, `length {max: 500}`.
const ENTITY_DESCRIPTION_MAX_LENGTH: usize = 500;

/// `Arn`'s modelled minimum length, which every minted `systemArn` must satisfy.
///
/// SOURCE: `smithy_query(query="shape", shape="Arn")` — `length {min: 31}`.
const ARN_MIN_LENGTH: usize = 31;

/// The members `System` shares with `CreateSystemRequest`, which a create echoes unchanged
/// and reports for no request that did not supply them.
///
/// SOURCE: `smithy_query(query="operation", operation="CreateSystem")` — the request
/// members other than `clientToken` (an `@idempotencyToken`, not a stored member),
/// `sharingEnabled` (a flag with a stored default, asserted separately) and `tags` (always
/// reported, asserted separately).
const ECHOED_MEMBERS: [&str; 3] = ["name", "description", "kmsKeyId"];

/// The members `System` carries that nothing in this service can fill, so every read
/// reports them absent rather than inventing one.
///
/// SOURCE: `smithy_query(query="shape", shape="System")` — `organizationId` is "The AWS
/// Organizations identifier for the system" and `ouId` "The organizational unit (OU)
/// identifier for the system"; this emulator joins no account to an organization.
const UNFILLED_MEMBERS: [&str; 2] = ["organizationId", "ouId"];

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing name
/// (`resiliencehub`) and `region`. Signature verification is off for these tests, so the
/// value only has to parse.
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

async fn create_system(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", CREATE_SYSTEM, Some(body)).await
}

async fn get_system(router: &MultiServiceRouter, system_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{GET_SYSTEM}?systemArn={system_arn}"),
        None,
    )
    .await
}

async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("{TAGS}/{}", label(arn)),
        Some(json!({"tags": tags})),
    )
    .await
}

/// An ARN inside a single `@httpLabel` path segment: its `:` and `/` travel as `%3A` and
/// `%2F`, so the whole ARN stays one segment.
fn label(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
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

/// The `systemId` a system ARN carries: its resource segment without the `system/`
/// qualifier, which is the identifier a refusal naming a system reports. A segment naming
/// another kind of resource carries no such id, so it stands as the identifier of whatever
/// the caller addressed.
fn system_id_of_arn(arn: &str) -> &str {
    resource_id_after_kind(arn, SYSTEM_RESOURCE)
}

/// The `system` member of a successful create or read.
fn system_of(case: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} {body:?}"
    );
    let system = &body["system"];
    assert!(
        system.is_object(),
        "case {case:?}: the @required system member is absent from {body:?}"
    );
    system.clone()
}

fn system_arn_of(case: &str, system: &Value) -> String {
    system["systemArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no systemArn in {system:?}"))
        .to_string()
}

/// The message AWS words a reused `clientToken` refusal with: fixed text around the token
/// the REQUEST supplied, and nothing else.
fn token_conflict_message(client_token: &str) -> String {
    format!(
        "Got two different request with the same token: {client_token}, please use a \
         different token"
    )
}

/// The message AWS words a taken-name refusal with: the resource type, fixed text, and the
/// name the REQUEST supplied.
fn name_conflict_message(name: &str) -> String {
    format!("{SYSTEM_RESOURCE_TYPE} with name already exists: {name}")
}

/// One row of the create table: the body of a single `CreateSystem` call.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    body: Value,
}

/// The create table.
///
/// Rows differ along which optional members are present (none, some, all), the length of
/// `name` and `description` (both modelled extremes, and an empty description),
/// `sharingEnabled` (absent, `false`, `true`), tag count (none, one, five) and each form
/// `kmsKeyId` accepts ("accepts key ID, key ARN, alias name, or alias ARN").
fn create_rows() -> Vec<Row> {
    vec![
        Row {
            case: "only the required name, at EntityName's minimum length".to_string(),
            body: json!({"name": "Ab"}),
        },
        Row {
            case: "longest name and longest description".to_string(),
            body: json!({
                "name": format!("Z{}", "y".repeat(ENTITY_NAME_MAX_LENGTH - 1)),
                "description": "d".repeat(ENTITY_DESCRIPTION_MAX_LENGTH),
            }),
        },
        Row {
            case: "an empty description and sharing enabled".to_string(),
            body: json!({
                "name": "Empty_Description",
                "description": "",
                "sharingEnabled": true,
            }),
        },
        Row {
            case: "sharing disabled explicitly, a bare key id".to_string(),
            body: json!({
                "name": "0-Shared-Off",
                "sharingEnabled": false,
                "kmsKeyId": "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
            }),
        },
        Row {
            case: "an alias name and one tag".to_string(),
            body: json!({
                "name": "Payments-Platform",
                "description": "The payments system",
                "kmsKeyId": "alias/resilience",
                "tags": {"team": "payments"},
            }),
        },
        Row {
            case: "a key ARN and five tags".to_string(),
            body: json!({
                "name": "Tier0_System",
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
            case: "an alias ARN, every optional member set".to_string(),
            body: json!({
                "name": "Everything-Set",
                "description": "Every optional member of the create",
                "sharingEnabled": true,
                "kmsKeyId": format!("arn:aws:kms:{REGION}:{ACCOUNT}:alias/resilience"),
                "tags": {"team": "resilience"},
                "clientToken": "everything-set-token",
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_system_reads_back_through_get_system() {
    let router = emulator();
    let mut minted: Vec<String> = Vec::new();
    for Row { case, body } in create_rows() {
        let (status, answered) = create_system(&router, body.clone()).await;
        let created = system_of(&case, status, &answered);

        // Every member the request supplied is echoed unchanged, and none it did not is
        // reported.
        for member in ECHOED_MEMBERS {
            assert_eq!(
                created[member], body[member],
                "case {case:?}: expected {member} to echo {:?}, got {:?}",
                body[member], created[member]
            );
        }
        // `sharingEnabled` is a stored flag: the value the request set, or off.
        let expected_sharing = body["sharingEnabled"].as_bool().unwrap_or(false);
        assert_eq!(
            created["sharingEnabled"].as_bool(),
            Some(expected_sharing),
            "case {case:?}: expected sharingEnabled {expected_sharing}, got {:?}",
            created["sharingEnabled"]
        );
        // The tag map is ALWAYS reported — a system carrying no tags has the empty map,
        // never an absent member.
        let expected_tags = body["tags"].clone();
        let expected_tags = if expected_tags.is_object() {
            expected_tags
        } else {
            json!({})
        };
        assert_eq!(
            created["tags"], expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            created["tags"]
        );
        for member in UNFILLED_MEMBERS {
            assert!(
                created[member].is_null(),
                "case {case:?}: expected {member} to be absent, got {:?}",
                created[member]
            );
        }

        // The ARN is minted in the signing Region, under the system resource type, and
        // carries the `systemId` the body reports.
        let system_arn = system_arn_of(&case, &created);
        let prefix = format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{SYSTEM_RESOURCE}/");
        assert!(
            system_arn.starts_with(&prefix) && system_arn.len() >= ARN_MIN_LENGTH,
            "case {case:?}: expected an ARN starting {prefix:?} and at least \
             {ARN_MIN_LENGTH} characters, got {system_arn:?}"
        );
        let system_id = created["systemId"].as_str().unwrap_or_default();
        assert_eq!(
            system_arn.strip_prefix(&prefix),
            Some(system_id),
            "case {case:?}: expected the ARN to carry systemId {system_id:?}, got \
             {system_arn:?}"
        );
        assert!(
            !minted.contains(&system_arn),
            "case {case:?}: {system_arn:?} was already minted for another system"
        );
        minted.push(system_arn.clone());

        // A just-created system has not been changed since, so both timestamps are the
        // create instant.
        let created_at = created["createdAt"].clone();
        assert!(
            created_at.is_number(),
            "case {case:?}: expected a createdAt timestamp, got {created_at:?}"
        );
        assert_eq!(
            created["updatedAt"], created_at,
            "case {case:?}: expected updatedAt to equal createdAt {created_at:?}, got {:?}",
            created["updatedAt"]
        );

        // The read answers the same projection, from the same stored record.
        let (read_status, read_body) = get_system(&router, &system_arn).await;
        let read = system_of(&case, read_status, &read_body);
        assert_eq!(
            read, created,
            "case {case:?}: GetSystem must answer what CreateSystem stored"
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_answers_the_system_the_first_call_created() {
    // Rows differ along the token's form and length: `ClientToken` is `len 1..64` with
    // pattern `^[A-Za-z0-9_.-]{1,64}$` (`smithy_query(query="shape", shape="ClientToken")`),
    // so the shortest, a UUID-shaped one, one carrying every allowed punctuation class and
    // the longest are all tokens.
    let router = emulator();
    let tokens = [
        "t".to_string(),
        "3f2504e0-4f89-11d3-9a0c-0305e82c3301".to_string(),
        "token_1.2-3".to_string(),
        "k".repeat(64),
    ];
    for (index, token) in tokens.iter().enumerate() {
        let case = format!("client token of {} characters", token.len());
        let first_body = json!({
            "name": format!("Replayed-{index}"),
            "description": "the first call",
            "sharingEnabled": true,
            "tags": {"team": "payments", "env": "prod"},
            "clientToken": token,
        });
        let (status, body) = create_system(&router, first_body.clone()).await;
        let first = system_of(&case, status, &body);

        // The SAME create under the same token is a retry: it answers the stored system,
        // minting nothing. The tags arrive in the other order, which is the same map.
        let mut repeat = first_body.clone();
        repeat["tags"] = json!({"env": "prod", "team": "payments"});
        let (replay_status, replay_body) = create_system(&router, repeat).await;
        let replayed = system_of(&case, replay_status, &replay_body);
        assert_eq!(
            replayed, first,
            "case {case:?}: a retry under the same token must answer the first system"
        );

        // A DIFFERENT token under a different name is a different system. The token is the
        // same length, so it stays inside `ClientToken`'s bounds.
        let distinct = format!("Z{}", &token[1..]);
        let (other_status, other_body) = create_system(
            &router,
            json!({"name": format!("Distinct-{index}"), "clientToken": distinct}),
        )
        .await;
        let other = system_of(&case, other_status, &other_body);
        assert_ne!(
            system_arn_of(&case, &other),
            system_arn_of(&case, &first),
            "case {case:?}: a fresh clientToken must create a new system"
        );
    }
}

#[tokio::test]
async fn a_client_token_offered_for_different_parameters_is_refused() {
    // Rows differ along WHICH creation parameter the second call changes — the name, the
    // description (changed, added and dropped), the sharing flag, the KMS key, a tag's
    // value and a tag added — because every one of them is part of the create the token
    // was accepted for.
    let token = "shared-token";
    let first_body = json!({
        "name": "Guarded-System",
        "description": "the create the token was accepted for",
        "sharingEnabled": false,
        "kmsKeyId": "alias/resilience",
        "tags": {"team": "payments"},
        "clientToken": token,
    });
    let differences: [(&str, &str, Value); 7] = [
        ("a different name", "name", json!("Other-System")),
        ("a different description", "description", json!("changed")),
        ("no description at all", "description", Value::Null),
        ("the sharing flag flipped", "sharingEnabled", json!(true)),
        ("a different KMS key", "kmsKeyId", json!("alias/other")),
        (
            "a tag value changed",
            "tags",
            json!({"team": "platform"}),
        ),
        (
            "a tag added",
            "tags",
            json!({"team": "payments", "env": "prod"}),
        ),
    ];
    for (case, member, value) in differences {
        // One emulator per row: the rule is about one token against one stored system.
        let router = emulator();
        let (status, body) = create_system(&router, first_body.clone()).await;
        let first = system_of(case, status, &body);
        let system_arn = system_arn_of(case, &first);

        let mut differing = first_body.clone();
        if value.is_null() {
            differing
                .as_object_mut()
                .expect("the create body is an object")
                .remove(member);
        } else {
            differing[member] = value;
        }
        let (conflict_status, conflict) = create_system(&router, differing).await;
        assert_eq!(
            conflict_status, 409,
            "case {case:?}: expected HTTP 409 for a token offered for another create, got \
             {conflict_status} {conflict:?}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {conflict:?}"
        );
        assert_eq!(
            conflict["message"].as_str(),
            Some(token_conflict_message(token).as_str()),
            "case {case:?}: expected the refusal {:?}, got {:?}",
            token_conflict_message(token),
            conflict["message"]
        );

        // The refused call wrote nothing: the stored system is the one the first call
        // created, unchanged.
        let (read_status, read_body) = get_system(&router, &system_arn).await;
        let read = system_of(case, read_status, &read_body);
        assert_eq!(
            read, first,
            "case {case:?}: a refused reuse must leave the stored system untouched"
        );

        // Nothing was reserved either: the same parameters under a FRESH token create a
        // second system.
        let mut retried = first_body.clone();
        retried["name"] = json!("Retried-System");
        retried["clientToken"] = json!("a-fresh-token");
        let (retry_status, retry_body) = create_system(&router, retried).await;
        let retried = system_of(case, retry_status, &retry_body);
        assert_ne!(
            system_arn_of(case, &retried),
            system_arn,
            "case {case:?}: a fresh token must create a second system"
        );
    }
}

#[tokio::test]
async fn a_name_already_taken_is_refused_as_a_conflict() {
    // Rows differ along the name's form and length, so the rule is asserted over the class
    // rather than on one spelling.
    let router = emulator();
    let names = [
        "Ab".to_string(),
        "system_with-punctuation".to_string(),
        "0leading-digit".to_string(),
        format!("W{}", "x".repeat(ENTITY_NAME_MAX_LENGTH - 1)),
    ];
    for name in names {
        assert!(
            name.len() >= ENTITY_NAME_MIN_LENGTH && name.len() <= ENTITY_NAME_MAX_LENGTH,
            "case {name:?}: the row must itself satisfy EntityName's bounds"
        );
        let (status, body) = create_system(&router, json!({"name": name})).await;
        let first = system_of(&name, status, &body);
        let system_arn = system_arn_of(&name, &first);

        let (conflict_status, conflict) =
            create_system(&router, json!({"name": name, "description": "second"})).await;
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
        assert_eq!(
            conflict["message"].as_str(),
            Some(name_conflict_message(&name).as_str()),
            "case {name:?}: expected the refusal {:?}, got {:?}",
            name_conflict_message(&name),
            conflict["message"]
        );

        // The refused create left the live system as it was.
        let (read_status, read_body) = get_system(&router, &system_arn).await;
        assert_eq!(
            system_of(&name, read_status, &read_body),
            first,
            "case {name:?}: a refused duplicate must leave the live system untouched"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_stored_system_is_not_found() {
    // Rows differ along the ARN's form: a system id this emulator never minted, another
    // Region's and another account's. Each IS a system ARN of this service — the partition,
    // Region and account say where the system would live — so each reaches the store and
    // resolves to nothing. An ARN that names no system of this service AT ALL is a
    // different answer, pinned in `user_journey_deletion`.
    let router = emulator();
    // One stored system, so the refusals are about the ARN rather than an empty store.
    let (status, body) = create_system(&router, json!({"name": "A-Live-System"})).await;
    system_of("a live system", status, &body);
    let arns = [
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{SYSTEM_RESOURCE}/missing"),
        format!("arn:aws:resiliencehub:eu-central-1:{ACCOUNT}:{SYSTEM_RESOURCE}/absent"),
        format!("arn:aws:resiliencehub:{REGION}:210987654321:{SYSTEM_RESOURCE}/elsewhere"),
    ];
    for arn in arns {
        let (status, body) = get_system(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404 for an ARN naming no system, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn:?}: expected ResourceNotFoundException, got {body:?}"
        );
        // The refusal identifies the system by its `systemId` — the id the ARN carries
        // after the `system/` qualifier, which is the identifier `System$systemId` reports
        // for a system that IS there — under the type the model's shape for a system
        // carries.
        let resource_id = system_id_of_arn(&arn);
        assert_eq!(
            body["resourceId"].as_str(),
            Some(resource_id),
            "case {arn:?}: expected resourceId {resource_id:?}, got {:?}",
            body["resourceId"]
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(SYSTEM_RESOURCE_TYPE),
            "case {arn:?}: expected resourceType {SYSTEM_RESOURCE_TYPE:?}, got {:?}",
            body["resourceType"]
        );
        let expected = format!("{SYSTEM_RESOURCE_TYPE} not found: {resource_id}");
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {arn:?}: expected the refusal {expected:?}, got {:?}",
            body["message"]
        );
    }
}

#[tokio::test]
async fn a_minted_system_arn_names_the_region_the_request_is_signed_for() {
    // Rows differ along the Region's partition form: a commercial Region, a European one, a
    // three-segment one and a GovCloud one all satisfy `AwsRegion`'s modelled pattern.
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
            CREATE_SYSTEM,
            Some(json!({"name": format!("Regional-{index}")})),
        )
        .await;
        let created = system_of(&case, status, &body);
        let system_arn = system_arn_of(&case, &created);
        let prefix = format!("arn:aws:resiliencehub:{region}:{ACCOUNT}:{SYSTEM_RESOURCE}/");
        assert!(
            system_arn.starts_with(&prefix),
            "case {case:?}: expected an ARN starting {prefix:?}, got {system_arn:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_written_to_a_system_is_visible_to_the_next_read() {
    // Rows differ along what the system already carries and what the write does: adding to
    // an untagged system, adding a key beside an existing one, and overwriting a key's
    // value.
    let router = emulator();
    let rows: [(&str, Value, Value, Value); 3] = [
        (
            "an untagged system gains a tag",
            json!({"name": "Untagged-System"}),
            json!({"team": "payments"}),
            json!({"team": "payments"}),
        ),
        (
            "a tagged system gains another key",
            json!({"name": "Tagged-System", "tags": {"team": "payments"}}),
            json!({"env": "prod"}),
            json!({"team": "payments", "env": "prod"}),
        ),
        (
            "an existing key takes the new value",
            json!({"name": "Retagged-System", "tags": {"team": "payments"}}),
            json!({"team": "platform"}),
            json!({"team": "platform"}),
        ),
    ];
    for (case, create_body, write, expected) in rows {
        let (status, body) = create_system(&router, create_body).await;
        let created = system_of(case, status, &body);
        let system_arn = system_arn_of(case, &created);

        let (tag_status, tag_body) = tag_resource(&router, &system_arn, write).await;
        assert!(
            (200..300).contains(&tag_status),
            "case {case:?}: expected TagResource to succeed, got {tag_status} {tag_body:?}"
        );

        let (read_status, read_body) = get_system(&router, &system_arn).await;
        let read = system_of(case, read_status, &read_body);
        assert_eq!(
            read["tags"], expected,
            "case {case:?}: expected tags {expected:?}, got {:?}",
            read["tags"]
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateSystem, DeleteSystem and the user journeys a system holds
// ---------------------------------------------------------------------------

/// The modelled routes the suites below drive.
const UPDATE_SYSTEM: &str = "/v2/update-system";
const DELETE_SYSTEM: &str = "/v2/delete-system";
const CREATE_USER_JOURNEY: &str = "/v2/create-user-journey";
const GET_USER_JOURNEY: &str = "/v2/get-user-journey";
const CREATE_SERVICE: &str = "/v2/create-service";
const UPDATE_SERVICE: &str = "/v2/update-service";
const DELETE_SERVICE: &str = "/v2/delete-service";
const CREATE_POLICY: &str = "/v2/create-policy";

async fn update_system(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", UPDATE_SYSTEM, Some(body)).await
}

async fn delete_system(router: &MultiServiceRouter, system_arn: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        DELETE_SYSTEM,
        Some(json!({"systemArn": system_arn})),
    )
    .await
}

async fn create_user_journey(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", CREATE_USER_JOURNEY, Some(body)).await
}

async fn get_user_journey(
    router: &MultiServiceRouter,
    system_arn: &str,
    user_journey_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{GET_USER_JOURNEY}?systemArn={system_arn}&userJourneyId={user_journey_id}"),
        None,
    )
    .await
}

/// Create a system named `name` and return its ARN and its `systemId`.
async fn system_named(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = create_system(router, json!({"name": name})).await;
    let created = system_of(name, status, &body);
    (
        system_arn_of(name, &created),
        created["systemId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no systemId in {created:?}"))
            .to_string(),
    )
}

/// The simplest body that satisfies `CreateService`'s required members, associated with
/// `systems`.
fn service_body(name: &str, systems: Vec<Value>) -> Value {
    json!({
        "name": name,
        "regions": [REGION],
        "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
        "associatedSystems": systems,
    })
}

/// Create a service associated with `systems` and return its ARN.
async fn service_associated_with(
    router: &MultiServiceRouter,
    name: &str,
    systems: Vec<Value>,
) -> String {
    let (status, body) = call(router, "POST", CREATE_SERVICE, Some(service_body(name, systems))).await;
    assert_eq!(status, 200, "creating service {name}: got {body:?}");
    body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {body:?}"))
        .to_string()
}

/// Whether `candidate` is a lower-case version-4 UUID in the canonical 8-4-4-4-12 textual
/// form: five hex groups of those lengths, `4` as the first nibble of the third group and
/// one of `8`, `9`, `a`, `b` (RFC 9562's variant bits) as the first nibble of the fourth.
fn is_version_4_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    if [8usize, 4, 4, 4, 12]
        .iter()
        .zip(&groups)
        .any(|(expected, group)| group.len() != *expected)
    {
        return false;
    }
    if !candidate
        .chars()
        .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
    {
        return false;
    }
    groups[2].starts_with('4') && groups[3].starts_with(['8', '9', 'a', 'b'])
}

/// RULE: `UpdateSystem` applies each PRESENT member to the stored system, leaves every
/// absent member untouched, bumps `updatedAt`, and the merged record is what every later
/// read returns.
///
/// The rows differ along which members the patch carries and what they carry: a
/// description on its own, the flag on its own, both at once, the empty description, a
/// description at `EntityDescription`'s modelled ceiling, and the flag in both directions.
#[tokio::test]
async fn a_system_update_lands_and_the_next_read_reports_it() {
    let longest = "u".repeat(ENTITY_DESCRIPTION_MAX_LENGTH);
    let patches: Vec<(&str, Value)> = vec![
        ("description only", json!({"description": "updated"})),
        ("the empty description", json!({"description": ""})),
        (
            "a description at EntityDescription's ceiling",
            json!({"description": longest}),
        ),
        ("sharing enabled", json!({"sharingEnabled": true})),
        ("sharing disabled", json!({"sharingEnabled": false})),
        (
            "description and the flag together",
            json!({"description": "both", "sharingEnabled": true}),
        ),
        ("no member at all", json!({})),
    ];

    for (index, (case, patch)) in patches.into_iter().enumerate() {
        let router = emulator();
        let create_body = json!({
            "name": format!("Patched-{index}"),
            "description": "the created description",
            "sharingEnabled": true,
            "kmsKeyId": "alias/resilience",
            "tags": {"team": "payments"},
        });
        let (status, body) = create_system(&router, create_body).await;
        let created = system_of(case, status, &body);
        let system_arn = system_arn_of(case, &created);

        let mut request = patch.clone();
        request["systemArn"] = json!(system_arn);
        let (status, answered) = update_system(&router, request).await;
        let updated = system_of(case, status, &answered);

        // Each member the patch carried took the value it sent.
        for (member, sent) in patch.as_object().expect("a patch object") {
            assert_eq!(
                &updated[member], sent,
                "case {case:?}: expected {member} to take {sent:?}, got {:?}",
                updated[member]
            );
        }
        // Everything the patch did NOT carry is untouched — including the identifiers, the
        // name, the tags and `createdAt`, none of which this operation takes at all.
        for member in [
            "systemArn",
            "systemId",
            "name",
            "kmsKeyId",
            "tags",
            "createdAt",
            "description",
            "sharingEnabled",
        ] {
            if patch.get(member).is_some() {
                continue;
            }
            assert_eq!(
                updated[member], created[member],
                "case {case:?}: {member} was not sent, so it must be untouched, got {:?}",
                updated[member]
            );
        }
        let created_at = updated["createdAt"].as_f64().unwrap_or_default();
        let updated_at = updated["updatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected updatedAt at or past createdAt, got {updated_at} vs \
             {created_at}"
        );

        // The next read answers the merged record, not the created one.
        let (read_status, read_body) = get_system(&router, &system_arn).await;
        assert_eq!(
            system_of(case, read_status, &read_body),
            updated,
            "case {case:?}: a later read must report what the update stored"
        );
    }
}

/// RULE: an ARN naming no stored system is refused `ResourceNotFoundException` by every
/// operation that resolves one, the mutating ones included.
#[tokio::test]
async fn an_update_or_delete_of_an_absent_system_is_not_found() {
    let router = emulator();
    // One stored system, so the refusals are about the ARN rather than an empty store.
    let (live_arn, _) = system_named(&router, "A-Live-System").await;
    let absent = format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{SYSTEM_RESOURCE}/absent");

    for (case, arn) in [("an id this store never minted", absent.as_str())] {
        for (operation, answer) in [
            ("UpdateSystem", update_system(&router, json!({"systemArn": arn, "description": "x"})).await),
            ("DeleteSystem", delete_system(&router, arn).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from {operation}, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException from {operation}, got \
                 {body:?}"
            );
            let identifier = system_id_of_arn(arn);
            assert_eq!(
                body["resourceId"].as_str(),
                Some(identifier),
                "case {case:?}: expected resourceId {identifier:?} from {operation}, got {:?}",
                body["resourceId"]
            );
        }
    }

    // The live system is untouched by the refusals.
    let (status, body) = get_system(&router, &live_arn).await;
    system_of("the live system", status, &body);
}

/// RULE: a delete removes the system and echoes its ARN; afterwards every reader reports it
/// as not found, by its own `systemId`, its name is free for a new system, and the
/// `clientToken` that created it replays nothing.
///
/// The rows differ along what the deleted system carried: the bare required member, a
/// `clientToken`, tags and the optional members together.
#[tokio::test]
async fn a_deleted_system_is_gone_from_every_reader() {
    let rows: Vec<(&str, Value)> = vec![
        ("only the required name", json!({"name": "Deleted-Plain"})),
        (
            "a clientToken",
            json!({"name": "Deleted-Token", "clientToken": "deleted-token"}),
        ),
        (
            "tags and every optional member",
            json!({
                "name": "Deleted-Full",
                "description": "a doomed system",
                "sharingEnabled": true,
                "kmsKeyId": "alias/resilience",
                "tags": {"team": "payments"},
            }),
        ),
    ];

    for (case, create_body) in rows {
        let router = emulator();
        let (status, body) = create_system(&router, create_body.clone()).await;
        let created = system_of(case, status, &body);
        let system_arn = system_arn_of(case, &created);
        let system_id = created["systemId"].as_str().expect("a systemId").to_string();

        let (status, deleted) = delete_system(&router, &system_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to succeed, got {status} {deleted:?}"
        );
        assert_eq!(
            deleted["systemArn"].as_str(),
            Some(system_arn.as_str()),
            "case {case:?}: the delete must echo the ARN it removed, got {deleted:?}"
        );

        // Every reader of a system reports it gone, naming it by its own id.
        for (reader, answer) in [
            ("GetSystem", get_system(&router, &system_arn).await),
            (
                "UpdateSystem",
                update_system(&router, json!({"systemArn": system_arn, "description": "x"})).await,
            ),
            ("DeleteSystem", delete_system(&router, &system_arn).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from {reader} after the delete, got {status} \
                 {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("{SYSTEM_RESOURCE_TYPE} not found: {system_id}").as_str()),
                "case {case:?}: {reader} must name the system's own id, got {:?}",
                body["message"]
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(system_id.as_str()),
                "case {case:?}: expected resourceId {system_id:?} from {reader}, got {:?}",
                body["resourceId"]
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(SYSTEM_RESOURCE_TYPE),
                "case {case:?}: expected resourceType {SYSTEM_RESOURCE_TYPE:?} from {reader}, \
                 got {:?}",
                body["resourceType"]
            );
        }

        // The name is free again, and the create that takes it is a NEW system — including
        // for the `clientToken` the deleted one consumed.
        let (status, recreated) = create_system(&router, create_body).await;
        let fresh = system_of(case, status, &recreated);
        assert_ne!(
            system_arn_of(case, &fresh),
            system_arn,
            "case {case:?}: recreating must mint a fresh ARN, got {fresh:?}"
        );
    }
}

/// RULE: a system a service is associated with cannot be deleted — the delete is refused
/// `ConflictException` while any service still lists it, and succeeds once nothing does.
///
/// The rows differ along how the association is released: by deleting the service, by
/// updating the service onto another system, and by updating it to no associations at all.
#[tokio::test]
async fn a_system_a_service_is_associated_with_cannot_be_deleted() {
    #[derive(Debug, Clone, Copy)]
    enum Release {
        DeleteTheService,
        AssociateAnotherSystem,
        AssociateNothing,
    }

    for (case, release) in [
        ("the service is deleted", Release::DeleteTheService),
        ("the service moves to another system", Release::AssociateAnotherSystem),
        ("the service drops every association", Release::AssociateNothing),
    ] {
        let router = emulator();
        let (system_arn, system_id) = system_named(&router, "Held-System").await;
        let (other_arn, _) = system_named(&router, "Other-System").await;
        let service_arn = service_associated_with(
            &router,
            "Holder-Service",
            vec![json!({"systemArn": system_arn})],
        )
        .await;

        let (status, refused) = delete_system(&router, &system_arn).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 while a service is associated, got {status} \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused:?}"
        );
        let expected = format!("Cannot delete system with associated services: {system_id}");
        assert_eq!(
            refused["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            refused["message"]
        );
        // The refused delete left the system readable.
        let (status, body) = get_system(&router, &system_arn).await;
        system_of(case, status, &body);

        match release {
            Release::DeleteTheService => {
                let (status, body) = call(
                    &router,
                    "POST",
                    DELETE_SERVICE,
                    Some(json!({"serviceArn": service_arn})),
                )
                .await;
                assert_eq!(status, 200, "case {case:?}: deleting the service got {body:?}");
            }
            Release::AssociateAnotherSystem => {
                let (status, body) = call(
                    &router,
                    "POST",
                    UPDATE_SERVICE,
                    Some(json!({
                        "serviceArn": service_arn,
                        "associatedSystems": [{"systemArn": other_arn}],
                    })),
                )
                .await;
                assert_eq!(status, 200, "case {case:?}: moving the service got {body:?}");
            }
            Release::AssociateNothing => {
                let (status, body) = call(
                    &router,
                    "POST",
                    UPDATE_SERVICE,
                    Some(json!({"serviceArn": service_arn, "associatedSystems": []})),
                )
                .await;
                assert_eq!(status, 200, "case {case:?}: clearing the association got {body:?}");
            }
        }

        let (status, deleted) = delete_system(&router, &system_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must succeed once nothing is associated, got {status} \
             {deleted:?}"
        );
        let (status, body) = get_system(&router, &system_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted system must be gone, got {status} {body:?}"
        );
    }
}

/// RULE: a user journey is created WITHIN a system, stored there, and read back unchanged
/// through `GetUserJourney`; the system's own delete takes its journeys with it.
///
/// The rows differ along which optional members the create carries and what they carry: the
/// bare required members, a name carrying the space `EntityLabel` admits, a description at
/// the modelled ceiling, a `policyArn`, and a `clientToken` replay.
#[tokio::test]
async fn a_created_user_journey_reads_back_through_get_user_journey() {
    let router = emulator();
    let (system_arn, _) = system_named(&router, "Journeyed-System").await;
    let policy_arn = format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{POLICY_RESOURCE}/p");
    let longest = "j".repeat(ENTITY_DESCRIPTION_MAX_LENGTH);
    let rows: Vec<(&str, Value)> = vec![
        ("only the required name", json!({"name": "Checkout"})),
        (
            "a name with the space EntityLabel admits",
            json!({"name": "Checkout journey"}),
        ),
        (
            "a description at EntityDescription's ceiling",
            json!({"name": "Longest-Described", "description": longest}),
        ),
        (
            "a policyArn and a description",
            json!({
                "name": "Policy Held",
                "description": "held to a policy",
                "policyArn": policy_arn,
            }),
        ),
        (
            "a clientToken",
            json!({"name": "Idempotent", "clientToken": "journey-token"}),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body) in rows {
        let mut request = body.clone();
        request["systemArn"] = json!(system_arn);
        let (status, answered) = create_user_journey(&router, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {answered:?}"
        );
        let created = answered["userJourney"].clone();
        for member in ["name", "description", "policyArn"] {
            assert_eq!(
                created[member], body[member],
                "case {case:?}: expected {member} to echo {:?}, got {:?}",
                body[member], created[member]
            );
        }
        let journey_id = created["userJourneyId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no userJourneyId in {created:?}"))
            .to_string();
        assert!(
            !minted.contains(&journey_id),
            "case {case:?}: every create must mint a fresh id, but {journey_id} repeated"
        );
        minted.push(journey_id.clone());

        // The read answers the same projection the create did.
        let (read_status, read_body) = get_user_journey(&router, &system_arn, &journey_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected the journey to read back, got {read_status} {read_body:?}"
        );
        assert_eq!(
            read_body["userJourney"], created,
            "case {case:?}: GetUserJourney must answer what the create stored"
        );

        // A `clientToken` is an @idempotencyToken: the same call answers the same journey.
        if body.get("clientToken").is_some() {
            let (replay_status, replay) = create_user_journey(&router, request).await;
            assert_eq!(
                replay_status, 200,
                "case {case:?}: a replayed token got {replay_status} {replay:?}"
            );
            assert_eq!(
                replay["userJourney"], created,
                "case {case:?}: a replayed token must answer the first journey"
            );
        }
    }

    // Deleting the system takes its journeys with it: nothing can reach a journey whose
    // system is gone.
    let (status, body) = delete_system(&router, &system_arn).await;
    assert_eq!(status, 200, "deleting the journeyed system got {body:?}");
    let (status, body) = get_user_journey(&router, &system_arn, &minted[0]).await;
    assert_eq!(
        status, 404,
        "a journey of a deleted system must be gone, got {status} {body:?}"
    );
}

/// RULE: a `userJourneyId` is only meaningful within its own system — a read naming another
/// system's journey, or an id nothing minted, is refused `ResourceNotFoundException` under
/// the model's own shape name for the resource.
#[tokio::test]
async fn a_user_journey_is_only_found_within_its_own_system() {
    let router = emulator();
    let (first_arn, _) = system_named(&router, "First-System").await;
    let (second_arn, _) = system_named(&router, "Second-System").await;
    let (status, body) =
        create_user_journey(&router, json!({"systemArn": first_arn, "name": "Only Journey"})).await;
    assert_eq!(status, 200, "creating the journey got {body:?}");
    let journey_id = body["userJourney"]["userJourneyId"]
        .as_str()
        .expect("a userJourneyId")
        .to_string();

    for (case, system_arn, requested_id) in [
        (
            "the journey of another system",
            second_arn.as_str(),
            journey_id.as_str(),
        ),
        ("an id nothing minted", first_arn.as_str(), "no-such-journey"),
    ] {
        let (status, body) = get_user_journey(&router, system_arn, requested_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        let expected = format!("User journey not found: {requested_id}");
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            body["message"]
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("UserJourney"),
            "case {case:?}: expected the model's shape name as resourceType, got {:?}",
            body["resourceType"]
        );
    }
}

/// RULE: every identifier this service mints — the `systemId`, the id in each minted ARN's
/// resource segment, and the ids of the resources a parent holds — is a lower-case
/// version-4 UUID, the form AWS hands out for them.
///
/// The rows span the resources this crate mints an id for, not only the one the issue
/// cited, and the systems row repeats so a second mint under the same rule is covered.
#[tokio::test]
async fn every_minted_identifier_is_a_version_4_uuid() {
    let router = emulator();

    let mut identifiers: Vec<(String, String)> = Vec::new();
    for index in 0..2 {
        let case = format!("system {index}");
        let (status, body) = create_system(&router, json!({"name": format!("Minted-{index}")})).await;
        let created = system_of(&case, status, &body);
        let system_arn = system_arn_of(&case, &created);
        let system_id = created["systemId"].as_str().expect("a systemId").to_string();
        // The id the ARN carries and the id the body reports are the same resource's.
        assert_eq!(
            system_id_of_arn(&system_arn),
            system_id,
            "case {case:?}: the ARN must carry the systemId it reports, got {system_arn:?}"
        );
        identifiers.push((format!("{case} systemId"), system_id.clone()));

        let (status, journey) = create_user_journey(
            &router,
            json!({"systemArn": system_arn, "name": format!("Journey {index}")}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: creating a journey got {journey:?}");
        identifiers.push((
            format!("{case} userJourneyId"),
            journey["userJourney"]["userJourneyId"]
                .as_str()
                .expect("a userJourneyId")
                .to_string(),
        ));
    }

    let (status, policy) = call(
        &router,
        "POST",
        CREATE_POLICY,
        Some(json!({"name": "Minted-Policy"})),
    )
    .await;
    assert_eq!(status, 200, "creating the policy got {policy:?}");
    identifiers.push((
        "policy ARN resource id".to_string(),
        resource_id_after_kind(
            policy["policy"]["policyArn"].as_str().expect("a policyArn"),
            POLICY_RESOURCE,
        )
        .to_string(),
    ));

    let service_arn = service_associated_with(&router, "Minted-Service", Vec::new()).await;
    identifiers.push((
        "service ARN resource id".to_string(),
        resource_id_after_kind(&service_arn, SERVICE_RESOURCE).to_string(),
    ));

    for (case, identifier) in identifiers {
        assert!(
            is_version_4_uuid(&identifier),
            "case {case:?}: expected a lower-case version-4 UUID, got {identifier:?}"
        );
    }
}

/// The id an ARN's resource segment (everything after the fifth `:`) carries after its
/// `<kind>/` qualifier.
fn resource_id_after_kind<'a>(arn: &'a str, kind: &str) -> &'a str {
    let resource_id = arn.splitn(6, ':').nth(5).unwrap_or(arn);
    resource_id
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(resource_id)
}
