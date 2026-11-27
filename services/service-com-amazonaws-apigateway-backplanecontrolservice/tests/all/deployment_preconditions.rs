// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PRECONDITIONS `CreateDeployment` and `CreateStage` hold a
//! request to, and for the sentences they refuse one with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the `CreateRestApi` / `PutMethod` /
//! `PutIntegration` helpers are this crate's existing harness.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded call:
//!
//! * a canary release deployment is a share of ONE stage's traffic, so a request carrying
//!   `canarySettings` with no stage named — the member absent, or carrying a value that
//!   names no stage at all — is refused, naming the value it supplied;
//! * the stage a canary release deployment names has to be a stage the API HAS, so a name
//!   it holds no stage for is a bad INPUT to the call — the class and the sentence this
//!   service answers any supplied `stageName` that names no stage with — and leaves neither
//!   a deployment nor a stage behind;
//! * `stageName` is OPTIONAL on `CreateDeployment` and an EMPTY value names no stage: the
//!   deployment is taken, no stage is created, and the members that would have configured
//!   that stage are ignored — while a value that is nothing but blanks is no name at all
//!   and is refused, as is `CreateStage`'s own required `stageName` when it is blank;
//! * every method of the API has to carry the integration a stage would serve a matched
//!   request with, so an API holding a method with none cannot be deployed until it does.

use serde_json::{json, Value};

// The sentence a `stageName` a request SUPPLIES that names no stage of the addressed API
// earns is this service's own sentence for a stage identifier that names no record, read
// off the crate rather than restated here, so this suite and the sibling
// `crate::refusal_class_and_quota_wording` (which holds the same class against the other
// operations that take such a value) cannot drift apart.
use service_com_amazonaws_apigateway_backplanecontrolservice::state::NO_SUCH_STAGE;

use crate::deployment_collection::{get_deployments, reported_ids};
use crate::execute_api_data_plane::{put_integration, put_method};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, resource_under, string_member, Answer, Gateway,
};
use crate::stage_collection::get_stages;
use crate::stage_lifecycle::get_stage;

/// The modelled failure a refused request of this plane carries: a bad request is
/// `BadRequestException` (HTTP 400).
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// The fixed part of the sentence a canary release deployment that names no stage at all
/// earns — the member absent, or present and empty. The value the request supplied follows
/// it, with no separator of its own, which is why the constant ends in a space.
const CANARY_NEEDS_STAGE: &str = "Invalid deployment content specified.Non null and non empty \
                                  stageName must be provided for canary deployment. Provided \
                                  value is ";

/// How a `stageName` the request did not carry is spelled in that sentence.
const ABSENT: &str = "null";

/// The sentence a `stageName` carrying no name at all earns, on either operation.
const EMPTY_STAGE_NAME: &str = "Stage name must be non-empty";

/// The sentence a deployment of a method with no integration earns.
const NO_INTEGRATION: &str = "No integration defined for method";

/// `CreateDeployment` of `api` with `request`, as sent.
async fn create_deployment(gateway: &Gateway, api: &str, request: Value) -> Answer {
    gateway
        .call("POST", &format!("/restapis/{api}/deployments"), Some(request))
        .await
}

/// `CreateStage` of `api` with `request`, as sent.
async fn create_stage(gateway: &Gateway, api: &str, request: Value) -> Answer {
    gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await
}

/// Assert `answer` is the modelled failure `expected` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body
    );
}

/// An API carrying one deployable method on its root resource: `GET` on `NONE`
/// authorization with a `MOCK` integration, the integration type that needs no backend
/// ("for testing without actually invoking the backend", `IntegrationType`). Answers the
/// API id and its root resource id.
async fn deployable_api(gateway: &Gateway, name: &str) -> (String, String) {
    let (api, root) = create_api(gateway, name).await;
    put_method(gateway, &api, &root, "GET", json!({ "authorizationType": "NONE" })).await;
    put_integration(gateway, &api, &root, "GET", json!({ "type": "MOCK" })).await;
    (api, root)
}

/// The deployments of `api`, as the collection reports them.
async fn deployments_of(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    reported_ids(&get_deployments(gateway, api, "").await.body, case)
}

/// The number of stages `api` holds.
async fn stage_count(gateway: &Gateway, api: &str, case: &str) -> usize {
    page_items(&get_stages(gateway, api, "").await.body, case).len()
}

/// The canary configurations the tables below span: the share alone, a share with the
/// stage cache flag, and one carrying variable overrides — the members
/// `DeploymentCanarySettings` declares.
fn canary_shapes() -> Vec<(&'static str, Value)> {
    vec![
        ("the share alone", json!({ "percentTraffic": 50.0 })),
        (
            "a share that uses the stage cache",
            json!({ "percentTraffic": 0.5, "useStageCache": true }),
        ),
        (
            "a share carrying stage variable overrides",
            json!({
                "percentTraffic": 100.0,
                "stageVariableOverrides": { "table": "pets-canary" },
            }),
        ),
        ("no member at all", json!({})),
    ]
}

#[tokio::test]
async fn a_canary_deployment_that_names_no_stage_is_refused_naming_the_value_it_supplied() {
    // Rows differ along what the rule generalises over: HOW the request names no stage —
    // the member absent, or present and empty — crossed with every shape the canary
    // settings themselves take. The sentence is the same for all of them, and the value
    // the request supplied is what follows it.
    let gateway = Gateway::new();
    let (api, _root) = deployable_api(&gateway, "canary-without-a-stage").await;

    for (shape, canary) in canary_shapes() {
        for (named, stage_name) in [("absent", None), ("empty", Some(""))] {
            let case = format!("canarySettings {shape:?} with a {named} stageName");
            let mut request = json!({ "canarySettings": canary });
            if let (Some(target), Some(stage_name)) = (request.as_object_mut(), stage_name) {
                target.insert("stageName".to_string(), json!(stage_name));
            }
            let refused = create_deployment(&gateway, &api, request).await;
            assert_refusal(
                &refused,
                BAD_REQUEST,
                &format!("{CANARY_NEEDS_STAGE}{}", stage_name.unwrap_or(ABSENT)),
                &case,
            );
            assert!(
                deployments_of(&gateway, &api, &case).await.is_empty(),
                "case {case}: expected the refused call to leave no deployment behind"
            );
            assert_eq!(
                stage_count(&gateway, &api, &case).await,
                0,
                "case {case}: expected the refused call to leave no stage behind"
            );
        }
    }

    // The positive control: the same canary settings against a stage the API HAS are
    // accepted, so the rows above are refused for the stage they name and nothing else.
    let deployed = create_deployment(&gateway, &api, json!({ "stageName": "prod" })).await;
    assert_eq!(
        deployed.status, 201,
        "case \"the control deployment\": expected 201, got {} with {:?}",
        deployed.status, deployed.body
    );
    let accepted = create_deployment(
        &gateway,
        &api,
        json!({ "stageName": "prod", "canarySettings": { "percentTraffic": 50.0 } }),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case \"a canary on the stage the API has\": expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn a_canary_deployment_for_a_stage_the_api_does_not_have_is_refused_and_creates_nothing() {
    // Rows differ along the FORM of the stage name the canary names (a word, a hyphenated
    // name, an underscored one, a mixed-case one with a digit, a single character) and
    // along the canary shape: none of them is a stage of the API, so all of them are
    // refused as the unresolvable stage they name.
    let names = [
        "a lowercase word",
        "a hyphenated name",
        "an underscored name",
        "a mixed-case name with a digit",
        "a single character",
    ];
    let values = ["prod", "stage-two", "stage_three", "Stage4", "x"];

    let gateway = Gateway::new();
    let (api, _root) = deployable_api(&gateway, "canary-on-a-missing-stage").await;
    // One stage the API DOES have, so a row is refused for the name it sent rather than
    // for the API holding no stage at all.
    let existing = create_deployment(&gateway, &api, json!({ "stageName": "live" })).await;
    assert_eq!(
        existing.status, 201,
        "case \"the stage the API has\": expected 201, got {} with {:?}",
        existing.status, existing.body
    );
    let before = deployments_of(&gateway, &api, "the stage the API has").await;

    for (form, value) in names.iter().zip(values) {
        for (shape, canary) in canary_shapes() {
            let case = format!("{form} ({value:?}) with canarySettings {shape:?}");
            let refused = create_deployment(
                &gateway,
                &api,
                json!({ "stageName": value, "canarySettings": canary }),
            )
            .await;
            assert_refusal(&refused, BAD_REQUEST, NO_SUCH_STAGE, &case);
            assert_eq!(
                deployments_of(&gateway, &api, &case).await,
                before,
                "case {case}: expected the refused call to leave the deployments as they were"
            );
            let stage = get_stage(&gateway, &api, value).await;
            assert_eq!(
                stage.status, 404,
                "case {case}: expected no stage of that name, got {} with {:?}",
                stage.status, stage.body
            );
        }
    }

    // And the same call against the stage the API has deploys to that stage's CANARY: the
    // stage keeps serving the deployment it points at.
    let case = "the canary of the stage the API has";
    let canary_deployment = create_deployment(
        &gateway,
        &api,
        json!({ "stageName": "live", "canarySettings": { "percentTraffic": 25.0 } }),
    )
    .await;
    assert_eq!(
        canary_deployment.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        canary_deployment.status, canary_deployment.body
    );
    let minted = string_member(&canary_deployment.body, "id", case);
    let read = get_stage(&gateway, &api, "live").await;
    assert_eq!(
        read.body["canarySettings"]["deploymentId"].as_str(),
        Some(minted.as_str()),
        "case {case}: expected the canary to serve the deployment the call created, got {:?}",
        read.body
    );
    assert_eq!(
        read.body["deploymentId"].as_str(),
        Some(before[0].as_str()),
        "case {case}: expected the stage to keep serving the deployment it points at, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn an_empty_stage_name_deploys_without_a_stage_and_a_blank_one_is_refused() {
    // `CreateDeployment`'s `stageName` is optional, and an EMPTY value is no name: the
    // deployment is taken and the stage-scoped members are ignored. A value that is
    // nothing but blanks is no name either, but it was SENT as one, so it is refused.
    //
    // Rows differ along the stage-scoped members the request carries beside the empty name
    // (none, a description, variables, a cache cluster, tracing, all of them), because the
    // rule is that NONE of them configures anything when there is no stage to configure.
    let settled: Vec<(&str, Value)> = vec![
        ("no stage-scoped member at all", json!({})),
        (
            "a stage description",
            json!({ "stageDescription": "the stage this call does not create" }),
        ),
        ("stage variables", json!({ "variables": { "table": "pets" } })),
        (
            "a cache cluster",
            json!({ "cacheClusterEnabled": true, "cacheClusterSize": "0.5" }),
        ),
        ("tracing", json!({ "tracingEnabled": true })),
        (
            "every stage-scoped member at once",
            json!({
                "stageDescription": "ignored",
                "variables": { "table": "pets" },
                "cacheClusterEnabled": true,
                "cacheClusterSize": "1.6",
                "tracingEnabled": true,
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = deployable_api(&gateway, "empty-stage-name").await;
    let mut taken: Vec<String> = Vec::new();
    for (case, extra) in &settled {
        let mut request = json!({ "stageName": "" });
        if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
            for (member, value) in source {
                target.insert(member.clone(), value.clone());
            }
        }
        let answer = create_deployment(&gateway, &api, request).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected the deployment to be taken, got {} with {:?}",
            answer.status, answer.body
        );
        taken.push(string_member(&answer.body, "id", case));
        assert_eq!(
            deployments_of(&gateway, &api, case).await,
            taken,
            "case {case:?}: expected every deployment taken so far to be stored"
        );
        // Nothing was configured, because there is no stage: the API still holds none.
        assert_eq!(
            stage_count(&gateway, &api, case).await,
            0,
            "case {case:?}: expected the API to hold no stage, got {:?}",
            get_stages(&gateway, &api, "").await.body
        );
    }

    // The blank forms: a value that is nothing but whitespace, in each form whitespace
    // takes (one space, several, a tab, a newline, a mix).
    let blanks = [" ", "   ", "\t", "\n", " \t "];
    for blank in blanks {
        let case = format!("CreateDeployment(stageName={blank:?})");
        let refused = create_deployment(&gateway, &api, json!({ "stageName": blank })).await;
        assert_refusal(&refused, BAD_REQUEST, EMPTY_STAGE_NAME, &case);
        assert_eq!(
            deployments_of(&gateway, &api, &case).await,
            taken,
            "case {case}: expected the refused call to leave no deployment behind"
        );
        assert_eq!(
            stage_count(&gateway, &api, &case).await,
            0,
            "case {case}: expected the refused call to leave no stage behind"
        );
    }

    // `CreateStage`'s own `stageName` is REQUIRED, and the same values are no name there
    // either — the empty one included, because that call exists to create a stage.
    let deployment = taken[0].clone();
    for value in [""].iter().copied().chain(blanks) {
        let case = format!("CreateStage(stageName={value:?})");
        let refused = create_stage(
            &gateway,
            &api,
            json!({ "stageName": value, "deploymentId": deployment }),
        )
        .await;
        assert_refusal(&refused, BAD_REQUEST, EMPTY_STAGE_NAME, &case);
        assert_eq!(
            stage_count(&gateway, &api, &case).await,
            0,
            "case {case}: expected the refused call to leave no stage behind"
        );
    }

    // The positive control: a name that IS a name creates the stage, so the rows above
    // are refused for the name they sent and nothing else.
    let created = create_stage(
        &gateway,
        &api,
        json!({ "stageName": "prod", "deploymentId": deployment }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"a stageName that is a name\": expected 201, got {} with {:?}",
        created.status, created.body
    );
}

#[tokio::test]
async fn an_api_whose_method_carries_no_integration_cannot_be_deployed_until_it_does() {
    // A deployment snapshots the API, and what a stage serves a matched request WITH is the
    // method's integration, so every method has to have one. Rows differ along WHERE the
    // method with no integration sits (the root resource, a child, a nested child), along
    // its VERB, and along how many of the API's other methods already carry one.
    struct Row {
        case: &'static str,
        /// The path part the method sits on, under the root (`None` for the root itself).
        path_part: Option<&'static str>,
        verb: &'static str,
    }
    let rows = [
        Row {
            case: "the only method, on the root resource",
            path_part: None,
            verb: "GET",
        },
        Row {
            case: "a POST on a child resource, beside methods that are wired up",
            path_part: Some("pets"),
            verb: "POST",
        },
        Row {
            case: "an ANY catch-all on a child resource",
            path_part: Some("files"),
            verb: "ANY",
        },
        Row {
            case: "a DELETE on a child resource",
            path_part: Some("things"),
            verb: "DELETE",
        },
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "deploy-without-an-integration").await;
    for row in &rows {
        let case = row.case;
        let resource = match row.path_part {
            None => root.clone(),
            Some(path_part) => resource_under(&gateway, &api, &root, path_part).await,
        };
        put_method(
            &gateway,
            &api,
            &resource,
            row.verb,
            json!({ "authorizationType": "NONE" }),
        )
        .await;

        // While that method has no integration, the API cannot be deployed at all — not to
        // a stage, and not without one.
        for (named, request) in [
            ("no stage", json!({})),
            ("a stage", json!({ "stageName": "prod" })),
        ] {
            let case = format!("{case}, deployed with {named}");
            let refused = create_deployment(&gateway, &api, request).await;
            assert_refusal(&refused, BAD_REQUEST, NO_INTEGRATION, &case);
            assert!(
                deployments_of(&gateway, &api, &case).await.is_empty(),
                "case {case}: expected the refused call to leave no deployment behind"
            );
            assert_eq!(
                stage_count(&gateway, &api, &case).await,
                0,
                "case {case}: expected the refused call to leave no stage behind"
            );
        }

        // Putting the integration is what makes the SAME call succeed; the deployment is
        // deleted again so the next row starts from a collection it can count.
        put_integration(&gateway, &api, &resource, row.verb, json!({ "type": "MOCK" })).await;
        let taken = create_deployment(&gateway, &api, json!({})).await;
        assert_eq!(
            taken.status, 201,
            "case {case}: expected the wired-up API to deploy, got {} with {:?}",
            taken.status, taken.body
        );
        let id = string_member(&taken.body, "id", case);
        let deleted = gateway
            .call("DELETE", &format!("/restapis/{api}/deployments/{id}"), None)
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected the deployment to be deleted, got {} with {:?}",
            deleted.status, deleted.body
        );
    }
}
