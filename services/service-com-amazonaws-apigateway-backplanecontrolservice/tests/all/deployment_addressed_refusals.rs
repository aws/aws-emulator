// SPDX-License-Identifier: CC0-1.0

//! WHICH record a call addressed by a DEPLOYMENT of a REST API names when the address
//! resolves to nothing, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the create helpers are this crate's
//! existing harness.
//!
//! The RULE under test: a call addressed by a RECORD INSIDE an api resolves the
//! `(restApiId, <record id>)` pair as ONE lookup for that record, so whichever half of the
//! pair resolves to nothing is refused with 404 `NotFoundException` and the sentence of the
//! RECORD the call addressed — `state::NO_SUCH_DEPLOYMENT` for a deployment-addressed call,
//! as `state::NO_SUCH_RESOURCE` already answers a resource-addressed one. A call whose own
//! target IS the api — the record itself, or a COLLECTION of the records inside it — keeps
//! answering the account-qualified api identifier
//! (`crate::rest_api_identifier_refusals::no_such_api`), because that call's address is the
//! api.
//!
//! What a client does with the difference is branch: "this api is gone, stop" is a different
//! fact from "this identifier names no deployment of it", and an existence probe run after a
//! teardown reads the record it asked about.
//!
//! The rows differ along what the rule generalises over: WHICH operation resolves the pair
//! (the read, the patch and the delete — every call addressed by a deployment), how the api
//! came to be absent (deleted after being built out and deployed, deleted while flat, never
//! created at all), which deployment id travels beside it (one the service really minted for
//! that api, one minted for an api that is still stored, one no api ever had), and — as the
//! control the rows are read against — the same pair on a STORED api, both halves of which
//! resolve.

use serde_json::{json, Value};

// The two sentences are read off the crate rather than restated here, so this suite cannot
// drift from the service: the deployment one is what every call addressed by a deployment
// answers, and the resource one is the same rule for the records under a resource.
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    NO_SUCH_DEPLOYMENT, NO_SUCH_RESOURCE,
};

use crate::rest_api_identifier_refusals::no_such_api;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, make_deployable, resource_under, string_member, Answer, Gateway,
};

/// Deployment identifiers no api ever had, differing in length (one character, the ten
/// characters API Gateway mints, longer), alphabet (letters, digits, a mix, a hyphen) and
/// case. `deploymentId` is a plain `String` with no `@pattern`
/// (`smithy_query(query="operation", operation="GetDeployment")`), so every row reaches the
/// lookup.
const NEVER_MINTED: &[&str] = &["d", "zzzzzzzzzz", "0123456789", "Ab3Xy9Zq1W", "no-such-dep-02"];

/// A patch body that is valid in itself, so a row reaches the identifier lookup instead of
/// being refused for its body (`UpdateDeployment` patches the deployment's description).
fn patch() -> Value {
    json!({
        "patchOperations": [{ "op": "replace", "path": "/description", "value": "patched" }],
    })
}

/// Every call of this service addressed BY a deployment, with `api` and `deployment` the pair
/// under test: the record's own read, its patch and its delete.
fn deployment_addressed_requests(
    api: &str,
    deployment: &str,
) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "GetDeployment",
            "GET",
            format!("/restapis/{api}/deployments/{deployment}"),
            None,
        ),
        (
            "UpdateDeployment",
            "PATCH",
            format!("/restapis/{api}/deployments/{deployment}"),
            Some(patch()),
        ),
        (
            "DeleteDeployment",
            "DELETE",
            format!("/restapis/{api}/deployments/{deployment}"),
            None,
        ),
    ]
}

/// Assert `answer` is a 404 `NotFoundException` whose message is exactly `message`.
fn assert_reports(answer: &Answer, case: &str, message: &str) {
    assert_failure(answer, 404, "NotFoundException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// `DeleteRestApi` for `api`, asserting the removal was accepted.
async fn delete_api(gateway: &Gateway, api: &str) {
    let answer = gateway.call("DELETE", &format!("/restapis/{api}"), None).await;
    assert_eq!(
        answer.status, 202,
        "case DeleteRestApi({api}): expected 202, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateDeployment` of `api` with no stage, asserting it was taken, and answering the id
/// the service minted for it.
async fn deployment_of(gateway: &Gateway, api: &str, case: &str) -> String {
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/deployments"), Some(json!({})))
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected the deployment to be taken, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", case)
}

/// An api that is NOT in the store, with the deployment id the service minted for it while
/// it was.
struct Absent {
    /// How this api came to be absent, named in every assertion of the row.
    how: String,
    /// The `restApiId` the rows send.
    api: String,
    /// A deployment id the service really minted for this api, valid until it went away.
    stale_deployment: Option<String>,
}

/// The absent apis under test: two the account HELD and deleted — one built out into a tree
/// and deployed, the way a teardown leaves it, and one still flat — and the identifiers no
/// api ever had.
async fn absent_apis(gateway: &Gateway) -> Vec<Absent> {
    let mut rows = Vec::new();

    let (built, built_root) = create_api(gateway, "deployment-probe-built-out").await;
    resource_under(gateway, &built, &built_root, "pets").await;
    make_deployable(gateway, &built).await;
    let stale = deployment_of(gateway, &built, "the deployment of the built-out api").await;
    delete_api(gateway, &built).await;
    rows.push(Absent {
        how: "an api deleted after it was built out and deployed".to_string(),
        api: built,
        stale_deployment: Some(stale),
    });

    let (flat, _flat_root) = create_api(gateway, "deployment-probe-flat").await;
    delete_api(gateway, &flat).await;
    rows.push(Absent {
        how: "an api deleted while it was still flat".to_string(),
        api: flat,
        stale_deployment: None,
    });

    for never in ["r", "zzzzzzzzzz", "no-such-api-02"] {
        rows.push(Absent {
            how: format!("an api identifier {never:?} the account never held"),
            api: never.to_string(),
            stale_deployment: None,
        });
    }
    rows
}

/// RULE: every call addressed by a deployment resolves the pair as one lookup for that
/// deployment, so an api that is not in the store is reported as the DEPLOYMENT the address
/// names — whatever deployment id travels beside it.
#[tokio::test]
async fn a_deployment_addressed_call_on_a_gone_rest_api_names_the_deployment() {
    let gateway = Gateway::new();
    // One surviving api supplies a LIVE deployment id, so a row can name the absent api
    // beside a deployment id that does resolve somewhere in the account.
    let (stored, _stored_root) = create_api(&gateway, "deployment-probe-surviving").await;
    make_deployable(&gateway, &stored).await;
    let live = deployment_of(&gateway, &stored, "the surviving api's deployment").await;

    for absent in absent_apis(&gateway).await {
        let mut deployments: Vec<(String, String)> = absent
            .stale_deployment
            .iter()
            .map(|id| (format!("its own stale deployment {id}"), id.clone()))
            .collect();
        deployments.push((
            "the live deployment of another stored api".to_string(),
            live.clone(),
        ));
        for never in NEVER_MINTED {
            deployments.push((
                format!("the deployment id {never:?} no api ever had"),
                (*never).to_string(),
            ));
        }

        for (position, deployment) in &deployments {
            for (operation, method, uri, body) in
                deployment_addressed_requests(&absent.api, deployment)
            {
                let case =
                    format!("{operation} [{method} {uri}] on {} with {position}", absent.how);
                let answer = gateway.call(method, &uri, body).await;
                assert_reports(&answer, &case, NO_SUCH_DEPLOYMENT);
            }
        }
    }

    // The same calls on the SURVIVING api and its own deployment are served, so the refusals
    // above are about the address and not about the requests.
    for (operation, method, uri, body, expected) in [
        (
            "GetDeployment",
            "GET",
            format!("/restapis/{stored}/deployments/{live}"),
            None,
            200,
        ),
        (
            "UpdateDeployment",
            "PATCH",
            format!("/restapis/{stored}/deployments/{live}"),
            Some(patch()),
            200,
        ),
        (
            "DeleteDeployment",
            "DELETE",
            format!("/restapis/{stored}/deployments/{live}"),
            None,
            202,
        ),
    ] {
        let answer = gateway.call(method, &uri, body).await;
        assert_eq!(
            answer.status, expected,
            "case {operation}(the surviving api's own deployment): expected {expected}, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE, the other half: the same sentence answers a deployment id a STORED api holds no
/// deployment under — the lookup is one lookup, so which half is unresolvable does not
/// change what is reported — and a call whose own target IS the api keeps answering the
/// account-qualified api identifier.
#[tokio::test]
async fn a_stored_api_names_the_deployment_for_an_id_it_does_not_hold() {
    let gateway = Gateway::new();
    let (stored, _root) = create_api(&gateway, "deployment-scope-stored").await;
    make_deployable(&gateway, &stored).await;
    let own = deployment_of(&gateway, &stored, "the stored api's own deployment").await;

    // A second api, deployed and then deleted, so its deployment id is one the service really
    // minted and no api holds any more; and a third that is STILL stored, so its deployment
    // id resolves — just not under the api the request names.
    let (gone, _gone_root) = create_api(&gateway, "deployment-scope-gone").await;
    make_deployable(&gateway, &gone).await;
    let gone_deployment = deployment_of(&gateway, &gone, "the deleted api's deployment").await;
    delete_api(&gateway, &gone).await;

    let (elsewhere, _elsewhere_root) = create_api(&gateway, "deployment-scope-elsewhere").await;
    make_deployable(&gateway, &elsewhere).await;
    let elsewhere_deployment =
        deployment_of(&gateway, &elsewhere, "the other stored api's deployment").await;

    let mut deployments: Vec<(String, String)> = vec![
        (
            "the stale deployment of a deleted api".to_string(),
            gone_deployment,
        ),
        (
            "the live deployment of another stored api".to_string(),
            elsewhere_deployment,
        ),
    ];
    for never in NEVER_MINTED {
        deployments.push((
            format!("the deployment id {never:?} no api ever had"),
            (*never).to_string(),
        ));
    }

    for (position, deployment) in &deployments {
        for (operation, method, uri, body) in deployment_addressed_requests(&stored, deployment) {
            let case = format!("{operation} [{method} {uri}] on the stored api with {position}");
            let answer = gateway.call(method, &uri, body).await;
            assert_reports(&answer, &case, NO_SUCH_DEPLOYMENT);
        }
    }

    // The stored api still holds its own deployment, so none of the refused calls touched it.
    let read = gateway
        .call("GET", &format!("/restapis/{stored}/deployments/{own}"), None)
        .await;
    assert_eq!(
        read.status, 200,
        "case GetDeployment(the stored api's own deployment): expected 200, got {} with {:?}",
        read.status, read.body
    );

    // A call whose own target IS the api answers the api identifier instead: the COLLECTION of
    // an absent api's deployments, and the api record itself.
    let (collection, _collection_root) = create_api(&gateway, "deployment-collection-gone").await;
    make_deployable(&gateway, &collection).await;
    deployment_of(&gateway, &collection, "the collection api's deployment").await;
    delete_api(&gateway, &collection).await;
    for (operation, uri) in [
        ("GetDeployments", format!("/restapis/{collection}/deployments")),
        ("GetRestApi", format!("/restapis/{collection}")),
    ] {
        let case = format!("{operation} [GET {uri}] on a deleted api");
        let answer = gateway.call("GET", &uri, None).await;
        assert_reports(&answer, &case, &no_such_api(&collection));
    }

    // ...and a RESOURCE-addressed call on the same absent api answers the resource sentence,
    // which is the same rule one record further down: each call names the record IT addressed.
    let case = "GetResource on a deleted api";
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{collection}/resources/zzzzzzzzzz"),
            None,
        )
        .await;
    assert_reports(&answer, case, NO_SUCH_RESOURCE);
}
