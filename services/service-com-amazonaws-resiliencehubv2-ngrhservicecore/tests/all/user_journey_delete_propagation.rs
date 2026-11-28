// SPDX-License-Identifier: CC0-1.0

//! `DeleteUserJourney`, end to end over HTTP, from the point of view of every path that
//! RESOLVES a `userJourneyId` after the delete.
//!
//! The rule under test: a delete of a child resource removes it from the parent record that
//! holds it and echoes the identifier it removed, so no later call can resolve that
//! identifier again — the journey's own reader and writers (`GetUserJourney`,
//! `UpdateUserJourney`, a repeated `DeleteUserJourney`) answer `ResourceNotFoundException`
//! (HTTP 404) `User journey not found: <userJourneyId>`, the parent's listing stops naming
//! it, and an `associatedSystems` entry that REFERENCES it — "the list of user journey
//! identifiers that associate this system with the service", resolved against the journeys
//! of the system the entry names — is refused `ValidationException` (HTTP 400) `User journey
//! not found: <userJourneyId> for system: <systemArn>` on `CreateService` and on
//! `UpdateService` alike. Everything the delete did not address — the parent system, its
//! other journeys and the service whose association resolves through one of them — is left
//! exactly as it was.
//!
//! The suites are TABLES: the rows differ along what the deleted journey carried (the bare
//! required member, a description, a `policyArn`, a `clientToken`, members a later
//! `UpdateUserJourney` changed) and, within a row, along WHICH path resolves the identifier
//! afterwards; the second suite's rows differ along how many journeys the parent held when
//! every one of them is deleted.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are exercised
//! exactly as a client would exercise them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled routes these suites drive.
///
/// SOURCE: `smithy_query(query="operation", operation="DeleteUserJourney")` — `http {method:
/// POST, uri: /v2/delete-user-journey}` — and the same query for each sibling.
const CREATE_SYSTEM: &str = "/v2/create-system";
const GET_SYSTEM: &str = "/v2/get-system";
const CREATE_POLICY: &str = "/v2/create-policy";
const CREATE_SERVICE: &str = "/v2/create-service";
const UPDATE_SERVICE: &str = "/v2/update-service";
const GET_SERVICE: &str = "/v2/get-service";
const CREATE_USER_JOURNEY: &str = "/v2/create-user-journey";
const UPDATE_USER_JOURNEY: &str = "/v2/update-user-journey";
const DELETE_USER_JOURNEY: &str = "/v2/delete-user-journey";
const GET_USER_JOURNEY: &str = "/v2/get-user-journey";
const LIST_USER_JOURNEYS: &str = "/v2/list-user-journeys";

/// The Region these suites sign for.
const REGION: &str = "us-east-1";

/// `ListUserJourneys$maxResults`' published maximum, used so one listing reports every
/// journey a system holds.
///
/// SOURCE: `smithy_query(query="operation", operation="ListUserJourneys")` — `range {min: 1,
/// max: 100}`.
const MAX_RESULTS_MAX: usize = 100;

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing name
/// (`resiliencehub`) and Region. Signature verification is off for these tests, so the value
/// only has to parse.
fn signed() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{REGION}\
         /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
         Signature=0123456789abcdef"
    )
}

async fn call(
    router: &MultiServiceRouter,
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
        .header("authorization", signed())
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

/// Create a system named `name` and answer its ARN.
async fn system_named(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", CREATE_SYSTEM, Some(json!({"name": name}))).await;
    assert_eq!(status, 200, "creating system {name:?} got {status} {body:?}");
    body["system"]["systemArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no system.systemArn in {body:?}"))
        .to_string()
}

/// Create a resilience policy named `name` and answer its ARN, so a row that gives its
/// journey a `policyArn` names a policy this store really holds.
async fn policy_named(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        CREATE_POLICY,
        Some(json!({"name": name, "multiAz": {"rtoInMinutes": 60, "rpoInMinutes": 30}})),
    )
    .await;
    assert_eq!(status, 200, "creating policy {name:?} got {status} {body:?}");
    body["policy"]["policyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no policy.policyArn in {body:?}"))
        .to_string()
}

/// `CreateUserJourney` on `system_arn` from `body`, which must succeed; answers the created
/// journey's `userJourneyId`.
async fn journey_in(router: &MultiServiceRouter, system_arn: &str, body: &Value) -> String {
    let mut request = body.clone();
    request["systemArn"] = json!(system_arn);
    let (status, answered) = call(router, "POST", CREATE_USER_JOURNEY, Some(request)).await;
    assert_eq!(
        status, 200,
        "creating a journey from {body:?} got {status} {answered:?}"
    );
    answered["userJourney"]["userJourneyId"]
        .as_str()
        .unwrap_or_else(|| panic!("no userJourney.userJourneyId in {answered:?}"))
        .to_string()
}

async fn update_journey(
    router: &MultiServiceRouter,
    system_arn: &str,
    user_journey_id: &str,
    patch: &Value,
) -> (u16, Value) {
    let mut request = patch.clone();
    request["systemArn"] = json!(system_arn);
    request["userJourneyId"] = json!(user_journey_id);
    call(router, "POST", UPDATE_USER_JOURNEY, Some(request)).await
}

async fn delete_journey(
    router: &MultiServiceRouter,
    system_arn: &str,
    user_journey_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        DELETE_USER_JOURNEY,
        Some(json!({"systemArn": system_arn, "userJourneyId": user_journey_id})),
    )
    .await
}

async fn get_journey(
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

/// The `userJourneyId` of every journey `system_arn` holds, in the order listed.
async fn listed_ids(router: &MultiServiceRouter, system_arn: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("{LIST_USER_JOURNEYS}?systemArn={system_arn}&maxResults={MAX_RESULTS_MAX}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "listing the journeys of {system_arn} got {status} {body:?}"
    );
    body["userJourneySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("the @required userJourneySummaries is absent from {body:?}"))
        .iter()
        .map(|summary| {
            summary["userJourneyId"]
                .as_str()
                .unwrap_or_else(|| panic!("no userJourneyId in {summary:?}"))
                .to_string()
        })
        .collect()
}

/// The simplest body that satisfies `CreateService`'s required members, named `name`.
fn minimal_service(name: &str) -> Value {
    json!({
        "name": name,
        "regions": [REGION],
        "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
    })
}

/// Create a service named `name` whose one `associatedSystems` entry references `system_arn`
/// through `user_journey_ids`, and answer its ARN.
async fn service_associated_through(
    router: &MultiServiceRouter,
    name: &str,
    system_arn: &str,
    user_journey_ids: &[&str],
) -> String {
    let mut body = minimal_service(name);
    body["associatedSystems"] =
        json!([{"systemArn": system_arn, "userJourneyIds": user_journey_ids}]);
    let (status, answered) = call(router, "POST", CREATE_SERVICE, Some(body)).await;
    assert_eq!(
        status, 200,
        "creating service {name:?} got {status} {answered:?}"
    );
    answered["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {answered:?}"))
        .to_string()
}

/// The `associatedSystems` a later read of `service_arn` reports.
async fn read_associations(router: &MultiServiceRouter, service_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("{GET_SERVICE}?serviceArn={service_arn}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "reading {service_arn} got {status} {body:?}");
    body["service"]["associatedSystems"].clone()
}

/// The sentence a refusal naming a journey that is not there is worded with: the resource as
/// the service words it in prose, the fixed `not found: ` text, and the identifier the
/// request supplied.
fn journey_not_found_message(user_journey_id: &str) -> String {
    format!("User journey not found: {user_journey_id}")
}

/// Assert that `body` is the 404 a reader of a journey that is not there answers: the fixed
/// message around the id the request supplied, that id as `resourceId`, and the model's own
/// shape name for the resource as `resourceType`.
fn assert_journey_not_found(
    case: &str,
    resolver: &str,
    status: u16,
    body: &Value,
    user_journey_id: &str,
) {
    assert_eq!(
        status, 404,
        "case {case:?} via {resolver}: expected HTTP 404 for a deleted journey, got {status} \
         {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?} via {resolver}: expected ResourceNotFoundException, got {code:?} in \
         {body:?}"
    );
    let expected = journey_not_found_message(user_journey_id);
    assert_eq!(
        body["message"].as_str(),
        Some(expected.as_str()),
        "case {case:?} via {resolver}: expected the refusal {expected:?}, got {:?}",
        body["message"]
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(user_journey_id),
        "case {case:?} via {resolver}: expected resourceId {user_journey_id:?}, got {:?}",
        body["resourceId"]
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some("UserJourney"),
        "case {case:?} via {resolver}: expected resourceType \"UserJourney\", got {:?}",
        body["resourceType"]
    );
}

/// Assert that `body` is the 400 a write REFERENCING a journey its system does not hold is
/// refused with: the journey's own not-found sentence, plus the system the reference was
/// resolved in.
fn assert_unresolved_reference(
    case: &str,
    resolver: &str,
    status: u16,
    body: &Value,
    user_journey_id: &str,
    system_arn: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?} via {resolver}: expected HTTP 400 for a reference to a deleted \
         journey, got {status} {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "ValidationException",
        "case {case:?} via {resolver}: expected ValidationException, got {code:?} in {body:?}"
    );
    let expected = format!(
        "{} for system: {system_arn}",
        journey_not_found_message(user_journey_id)
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected.as_str()),
        "case {case:?} via {resolver}: expected the refusal {expected:?}, got {:?}",
        body["message"]
    );
}

// ---------------------------------------------------------------------------
// What a delete makes unresolvable
// ---------------------------------------------------------------------------

/// RULE: a delete removes the journey from the system that holds it and echoes the id it
/// removed, so every later path that resolves that id reports it as absent — its own
/// readers and writers with the journey's 404, a service association referencing it with the
/// unresolved-reference 400 — while the parent, the parent's other journeys and the service
/// whose association resolves through one of them are untouched.
///
/// The rows differ along what the deleted journey carried: the bare required member, a
/// description, a `policyArn` naming a stored policy, a `clientToken`, and a journey whose
/// members a later `UpdateUserJourney` changed.
#[tokio::test]
async fn every_path_resolving_a_deleted_journey_reports_it_absent() {
    let router = emulator();
    let policy_arn = policy_named(&router, "Journey-Delete-Policy").await;

    // (case, the create's members beyond `systemArn`, an optional patch applied before the
    // delete)
    let rows: Vec<(&str, Value, Option<Value>)> = vec![
        ("only the required name", json!({"name": "Bare Journey"}), None),
        (
            "a description",
            json!({"name": "Described Journey", "description": "what the journey covers"}),
            None,
        ),
        (
            "a policyArn naming a stored policy",
            json!({"name": "Policied Journey", "policyArn": policy_arn}),
            None,
        ),
        (
            "a clientToken",
            json!({"name": "Tokened Journey", "clientToken": "journey-propagation-token"}),
            None,
        ),
        (
            "members changed after creation",
            json!({"name": "Edited Journey"}),
            Some(json!({"name": "Edited Journey Again", "description": "edited"})),
        ),
    ];

    for (index, (case, create_body, patch)) in rows.into_iter().enumerate() {
        // Each row gets its own system, so one row's deletes cannot reach another's.
        let system_arn = system_named(&router, &format!("Propagation-Host-{index}")).await;
        let kept = journey_in(&router, &system_arn, &json!({"name": "Kept Journey"})).await;
        let doomed = journey_in(&router, &system_arn, &create_body).await;
        if let Some(patch) = patch.as_ref() {
            let (status, body) = update_journey(&router, &system_arn, &doomed, patch).await;
            assert_eq!(
                status, 200,
                "case {case:?}: editing the journey before the delete got {status} {body:?}"
            );
        }

        // A service whose one association resolves through the journey that SURVIVES, so a
        // refused write can be shown to have left it alone.
        let holder = service_associated_through(
            &router,
            &format!("propagation-holder-{index}"),
            &system_arn,
            &[kept.as_str()],
        )
        .await;
        let associations_before = read_associations(&router, &holder).await;

        // The delete answers the @required identifier of the journey it removed.
        let (status, body) = delete_journey(&router, &system_arn, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteUserJourney, got {status} {body:?}"
        );
        assert_eq!(
            body["userJourneyId"].as_str(),
            Some(doomed.as_str()),
            "case {case:?}: expected the deleted id {doomed:?} echoed, got {body:?}"
        );

        // Every path that addresses the journey ITSELF reports it as not found.
        for (resolver, (status, body)) in [
            ("GetUserJourney", get_journey(&router, &system_arn, &doomed).await),
            (
                "UpdateUserJourney",
                update_journey(
                    &router,
                    &system_arn,
                    &doomed,
                    &json!({"description": "an edit of something that is gone"}),
                )
                .await,
            ),
            (
                "DeleteUserJourney",
                delete_journey(&router, &system_arn, &doomed).await,
            ),
        ] {
            assert_journey_not_found(case, resolver, status, &body, &doomed);
        }

        // Every path that REFERENCES it from a service association is refused, naming the id
        // and the system the reference was resolved in.
        let reference = json!([{"systemArn": system_arn, "userJourneyIds": [doomed]}]);
        let mut created = minimal_service(&format!("propagation-referrer-{index}"));
        created["associatedSystems"] = reference.clone();
        for (resolver, (status, body)) in [
            (
                "CreateService",
                call(&router, "POST", CREATE_SERVICE, Some(created)).await,
            ),
            (
                "UpdateService",
                call(
                    &router,
                    "POST",
                    UPDATE_SERVICE,
                    Some(json!({"serviceArn": holder, "associatedSystems": reference})),
                )
                .await,
            ),
        ] {
            assert_unresolved_reference(case, resolver, status, &body, &doomed, &system_arn);
        }

        // Nothing the delete did not address moved: the parent still reads back, it still
        // holds exactly the surviving journey, that journey still reads back, and the
        // service's association is the one it was created with.
        let (status, body) = call(
            &router,
            "GET",
            &format!("{GET_SYSTEM}?systemArn={system_arn}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the parent system must still read back, got {status} {body:?}"
        );
        let listed = listed_ids(&router, &system_arn).await;
        assert_eq!(
            listed,
            vec![kept.clone()],
            "case {case:?}: expected only the surviving journey {kept:?} listed, got {listed:?}"
        );
        let (status, body) = get_journey(&router, &system_arn, &kept).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the surviving journey must still read back, got {status} {body:?}"
        );
        let associations_after = read_associations(&router, &holder).await;
        assert_eq!(
            associations_after, associations_before,
            "case {case:?}: expected the association {associations_before:?} untouched, got \
             {associations_after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What is left when every journey is deleted
// ---------------------------------------------------------------------------

/// RULE: deleting a child resource never reaches its parent — a system every one of whose
/// journeys has been deleted still reads back, now holding none, and accepts a new journey
/// afterwards.
///
/// The rows differ along how many journeys the parent held before they were all deleted.
#[tokio::test]
async fn deleting_every_journey_leaves_the_parent_holding_none() {
    let router = emulator();

    for held in [1_usize, 2, 5] {
        let case = format!("{held} journey(s) deleted one by one");
        let system_arn = system_named(&router, &format!("Emptied-Host-{held}")).await;
        let mut ids = Vec::new();
        for index in 0..held {
            let body = json!({"name": format!("Journey {index}")});
            ids.push(journey_in(&router, &system_arn, &body).await);
        }

        for (position, id) in ids.iter().enumerate() {
            let (status, body) = delete_journey(&router, &system_arn, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting journey {position} got {status} {body:?}"
            );
            let listed = listed_ids(&router, &system_arn).await;
            let expected: Vec<String> = ids[position + 1..].to_vec();
            assert_eq!(
                listed, expected,
                "case {case:?}: after deleting journey {position} expected {expected:?} left, \
                 got {listed:?}"
            );
        }

        let (status, body) = call(
            &router,
            "GET",
            &format!("{GET_SYSTEM}?systemArn={system_arn}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the emptied system must still read back, got {status} {body:?}"
        );

        // The parent still takes a journey, so the deletes freed its room rather than
        // closing it.
        let fresh = journey_in(&router, &system_arn, &json!({"name": "After The Deletes"})).await;
        let listed = listed_ids(&router, &system_arn).await;
        assert_eq!(
            listed,
            vec![fresh.clone()],
            "case {case:?}: expected the new journey {fresh:?} to be the only one, got {listed:?}"
        );
    }
}
