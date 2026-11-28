// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a `restApiId` that names no stored API, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateResource` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`.
//!
//! The RULE under test, which is this service's and not one operation's: an API Gateway
//! operation whose own target is the API — the REST API record itself, a collection of it,
//! or an API an input member REFERENCES — states an identifier it cannot resolve back,
//! qualified by the account the lookup ran in —
//! `Invalid API identifier specified <accountId>:<restApiId>` — so the sentence is built
//! from the request and is the same whichever operation resolved the identifier and
//! whichever error class that operation models for the failure.
//!
//! Where the rule STOPS: a call addressed by a record NESTED under the API — a stage of
//! it, a resource of it, or a record hanging off one of those — resolves that record by its
//! whole key in one lookup and reports THAT record instead, which
//! `nested_record_refusals` asserts.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! nine operations that resolve a `restApiId` (so the sentence is shown not to be one
//! handler's), a set of unknown identifiers differing in length, alphabet and case (so
//! the identifier is shown to be quoted from the request rather than fixed), both
//! bindings the identifier arrives on (a URI label and a body member), and both error
//! classes an operation models for it (the `NotFoundException` of a read or a write, and
//! the `BadRequestException` `CreateRestApi` models for `cloneFrom`, which models no
//! `NotFoundException`).

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, string_member, Answer, Gateway};

/// The sentence this service states for an API identifier that resolves to no API.
///
/// The account segment is the emulator's one account (`engine::emulator_account_id`),
/// never a literal, so this expectation stays true of whatever account the runtime
/// presents.
pub(crate) fn no_such_api(rest_api_id: &str) -> String {
    format!(
        "Invalid API identifier specified {}:{rest_api_id}",
        engine::emulator_account_id()
    )
}

/// Identifiers no stored API has. The rows differ along what the rule generalises over:
/// the length (one character, the ten characters API Gateway mints, longer), the
/// alphabet (letters, digits, a mix, a hyphen) and the case — the message quotes what
/// the request sent, and `restApiId` is a plain `String` with no `@pattern`
/// (`smithy_query(query="operation", operation="GetRestApi")`), so every row reaches the
/// lookup.
const UNKNOWN_IDS: &[&str] = &[
    "q",
    "zzzzzzzzzz",
    "0123456789",
    "Ab3Xy9Zq1W",
    "no-such-api-00000001",
];

/// A patch body that is valid in itself, so a row reaches the identifier lookup instead
/// of being refused for its body.
fn patch(path: &str, value: &str) -> Value {
    json!({ "patchOperations": [{ "op": "replace", "path": path, "value": value }] })
}

/// Every request of this slice whose own TARGET is the API and that resolves a `restApiId`
/// from the URI, with `api` the identifier under test: the record itself, a collection of it,
/// and the creates that snapshot it. `deployment` names a record of a DIFFERENT, stored API, so
/// the only thing wrong with the request is the API identifier.
///
/// A call addressed BY a record inside the api is not among them: such a call resolves the pair
/// as one lookup for that record and reports the RECORD rather than the api identifier, which
/// is the rule `crate::deployment_addressed_refusals` owns for a deployment-addressed call and
/// `crate::nested_record_refusals` for the records under a resource.
fn requests(
    api: &str,
    deployment: &str,
) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        ("GetRestApi", "GET", format!("/restapis/{api}"), None),
        ("DeleteRestApi", "DELETE", format!("/restapis/{api}"), None),
        (
            "UpdateRestApi",
            "PATCH",
            format!("/restapis/{api}"),
            Some(patch("/description", "d")),
        ),
        (
            "GetResources",
            "GET",
            format!("/restapis/{api}/resources"),
            None,
        ),
        (
            "GetModels",
            "GET",
            format!("/restapis/{api}/models"),
            None,
        ),
        (
            "GetAuthorizers",
            "GET",
            format!("/restapis/{api}/authorizers"),
            None,
        ),
        (
            "CreateDeployment",
            "POST",
            format!("/restapis/{api}/deployments"),
            Some(json!({})),
        ),
        (
            "CreateStage",
            "POST",
            format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": "prod", "deploymentId": deployment })),
        ),
    ]
}

/// Assert `answer` is the `status`/`code` failure whose message is exactly `message`.
fn assert_stated(answer: &Answer, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(code),
        "case {case}: expected {code}, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn an_unresolvable_api_identifier_is_stated_with_the_account_and_the_identifier() {
    let gateway = Gateway::new();
    // One stored API supplies the resource and deployment identifiers the rows reuse, so
    // every refusal below is about the API identifier alone.
    let (stored, _root) = create_api(&gateway, "identifier-refusals").await;
    // A deployment is the snapshot of the API's methods, so the API must carry one.
    crate::rest_api_lifecycle::make_deployable(&gateway, &stored).await;
    let created = gateway
        .call(
            "POST",
            &format!("/restapis/{stored}/deployments"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case CreateDeployment(stored): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let deployment = string_member(&created.body, "id", "CreateDeployment(stored)");

    for missing in UNKNOWN_IDS {
        for (operation, method, uri, body) in requests(missing, &deployment) {
            let case = format!("{operation} [{method} {uri}]");
            let answer = gateway.call(method, &uri, body).await;
            assert_stated(
                &answer,
                &case,
                404,
                "NotFoundException",
                &no_such_api(missing),
            );
        }
    }

    // The same reads against the STORED identifier are served, so the refusals above are
    // about the identifier and not about the requests.
    let served = gateway.call("GET", &format!("/restapis/{stored}"), None).await;
    assert_eq!(
        served.status, 200,
        "case GetRestApi(stored): expected 200, got {} with {:?}",
        served.status, served.body
    );
}

#[tokio::test]
async fn an_unresolvable_api_identifier_in_a_body_member_is_stated_the_same_way() {
    let gateway = Gateway::new();
    let (stored, _) = create_api(&gateway, "identifier-refusals-body").await;

    for missing in UNKNOWN_IDS {
        // `CreateApiKey$stageKeys` carries the identifier in the BODY: the same sentence,
        // reached through a different binding.
        let answer = gateway
            .call(
                "POST",
                "/apikeys",
                Some(json!({
                    "name": format!("key-{missing}"),
                    "stageKeys": [{ "restApiId": missing, "stageName": "prod" }]
                })),
            )
            .await;
        assert_stated(
            &answer,
            &format!("CreateApiKey(stageKeys naming {missing})"),
            404,
            "NotFoundException",
            &no_such_api(missing),
        );

        // `CreateRestApi$cloneFrom` also carries it in the body, and that operation
        // models NO `NotFoundException`
        // (`smithy_query(query="operation", operation="CreateRestApi")`), so the same
        // sentence arrives as the `BadRequestException` it does model: the message states
        // the identifier, independently of the error class.
        let cloned = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({ "name": format!("clone-{missing}"), "cloneFrom": missing })),
            )
            .await;
        assert_stated(
            &cloned,
            &format!("CreateRestApi(cloneFrom {missing})"),
            400,
            "BadRequestException",
            &no_such_api(missing),
        );
    }

    // Cloning the STORED api is accepted, so the refusals above are about the identifier.
    let accepted = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "clone-of-stored", "cloneFrom": stored })),
        )
        .await;
    assert_eq!(
        accepted.status, 201,
        "case CreateRestApi(cloneFrom stored): expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn an_unresolvable_api_identifier_is_stated_the_same_way_by_an_import() {
    let gateway = Gateway::new();
    let definition = "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"t\", \"version\": \"1\"}, \
                      \"paths\": {\"/pets\": {\"get\": {\"responses\": {\"200\": \
                      {\"description\": \"ok\"}}}}}}";

    // `PutRestApi` takes the definition as its `@httpPayload` blob, so the row goes
    // through the raw-body path; the identifier is still the URI label.
    for missing in UNKNOWN_IDS {
        for mode in ["", "?mode=merge", "?mode=overwrite"] {
            let uri = format!("/restapis/{missing}{mode}");
            let answer = gateway.call_raw("PUT", &uri, definition).await;
            assert_stated(
                &answer,
                &format!("PutRestApi [PUT {uri}]"),
                404,
                "NotFoundException",
                &no_such_api(missing),
            );
        }
    }
}
