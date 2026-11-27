// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE every `Update*` operation that takes
//! `patchOperations` refuses a patch path it cannot apply with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the per-resource create and read
//! helpers are this crate's existing harness.
//!
//! The RULE under test: a patch path such an operation cannot apply is refused with a
//! `BadRequestException` (HTTP 400) whose message quotes the rejected path, names the `op`
//! of the same `patchOperations` entry and enumerates the paths that resource publishes for
//! THAT op — or, for an op it publishes no path for, tells the caller to choose one it
//! supports — and where the pointer reaches into a NESTED structure the service validates
//! on its own, the structure's own sentence names that member and enumerates its full
//! pointers instead. An `op` the resource takes no path at all for is refused as an op,
//! with the resource's own enumeration of the ops it does take.
//!
//! The rows span what the rule generalises over rather than repeating one call: the
//! OPERATION (and so the resource and its published list), the op that named the path, and
//! the FORM of the unapplicable path — a member no shape declares, a member the service
//! assigns, a pointer reaching into a single-valued member, a path that is no JSON Pointer,
//! an empty path. Each published list is pinned from the other side too: a path it names is
//! accepted for that resource and reads back through the resource's own reader.
//!
//! `UpdateApiKey` publishes a DIFFERENT list per op and has its own suite
//! (`update_api_key_patch_path_refusal`); the operations that answer with the bare
//! unaddressable-path sentence are covered by `refusal_message_constants` and
//! `patch_updates`.

use serde_json::{json, Value};

use crate::authorizer_lifecycle::{created_authorizer, token_request};
use crate::documentation_parts::{content_map, created_part_id};
use crate::documentation_versions::{create_version, snapshot_request};
use crate::models::{created_model, json_schema};
use crate::patch_updates::{op, patch, replace};
use crate::request_validator_lifecycle::created_validator;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, string_member, Gateway,
};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};
use crate::usage_plan_lifecycle::create_usage_plan;

/// The one sentence API Gateway refuses an `op` the resource supports no path for with:
/// `move`, `copy` and `test` name nothing on these resources, so there is no permitted-path
/// set to enumerate for them.
///
/// `pub(crate)` so a sibling suite asserts the one sentence rather than keeping a copy of
/// it (the `patch_entry_precedence` suite does).
pub(crate) const INVALID_OP: &str =
    "Invalid patch operation specified. Must be 'add'|'remove'|'replace'";

/// The sentence a STAGE refuses such an op with: a stage takes a `copy` (the canary
/// promotion `PatchOperation$from` documents), so the ops it enumerates — and the wording
/// it enumerates them in — are its own.
const STAGE_INVALID_OP: &str =
    "Invalid patch operation specified. Must be one of: 'replace', 'remove', 'copy'";

/// The ops a resource takes a path for, and so the ops a refusal names: each enumerates
/// the list that resource publishes for THAT op, which for most of them is a list under
/// `replace` and none at all under the other two.
const PATH_OPS: [&str; 3] = ["replace", "add", "remove"];

/// The ops no path can ride in on, which are refused as ops instead.
const OP_ONLY_REFUSALS: [&str; 3] = ["move", "copy", "test"];

/// The IAM role ARN an account's `cloudwatchRoleArn` takes: "The ARN of an Amazon
/// CloudWatch role for the current Account" (`Account$cloudwatchRoleArn`).
const CLOUDWATCH_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/apigw-cloudwatch";

/// One operation under test.
struct Target {
    /// The operation's name, for the case label.
    operation: &'static str,
    /// The URI a PATCH addresses the record by — and, for every one of these resources, the
    /// URI its own reader answers a GET on.
    uri: String,
    /// The paths this resource publishes for a `replace`, in the order its refusal
    /// enumerates them.
    replace_paths: Vec<&'static str>,
    /// The paths it publishes for an `add`, and those it publishes for a `remove` — the
    /// resource's multi-valued members and the nested settings those two ops create and
    /// drop, which need not be the same set. Empty for a resource that publishes none,
    /// whose refusal then names no list at all.
    add_paths: Vec<&'static str>,
    /// See [`Target::add_paths`].
    remove_paths: Vec<&'static str>,
    /// The sentence this resource refuses an `op` it takes no path at all for with: the ops
    /// it does take, enumerated the way the resource itself words them.
    invalid_op: &'static str,
    /// One published path that takes a `replace`, the value to send, and the member the
    /// resource reports it through: the other side of the list, so a path the refusal
    /// offers is seen to be one the resource accepts.
    accepted: (&'static str, &'static str, &'static str),
}

impl Target {
    /// The paths this resource publishes for `kind`.
    fn published(&self, kind: &str) -> &[&'static str] {
        match kind {
            "add" => &self.add_paths,
            "remove" => &self.remove_paths,
            _ => &self.replace_paths,
        }
    }

    /// The message this operation must refuse `path` under `kind` with.
    fn expected(&self, path: &str, kind: &str) -> String {
        if OP_ONLY_REFUSALS.contains(&kind) {
            return self.invalid_op.to_string();
        }
        let published = self.published(kind);
        if published.is_empty() {
            return format!(
                "Invalid patch path  '{path}' specified for op '{kind}'. \
                 Please choose supported operations"
            );
        }
        format!(
            "Invalid patch path  '{path}' specified for op '{kind}'. Must be one of: [{}]",
            published.join(", ")
        )
    }
}

/// Every operation under test, each standing its own record up on `gateway`.
///
/// The published lists are the ones API Gateway itself enumerates for these resources,
/// which are the members each handler's own arms apply (`smithy_query(query="operation",
/// operation="UpdateAccount,UpdateDeployment,UpdateDocumentationPart,\
/// UpdateDocumentationVersion,UpdateModel,UpdateRequestValidator,UpdateAuthorizer,\
/// UpdateResource,UpdateUsagePlan")` for the shapes behind them) — the usage plan's nested
/// entries spelled as the TEMPLATES the service publishes, placeholders and all.
async fn targets(gateway: &Gateway) -> Vec<Target> {
    let (api, root) = create_api(gateway, "patch-path-templates").await;
    let child = create_resource(gateway, &api, &root, "pets").await;
    let resource = string_member(&child, "id", "CreateResource(pets)");
    let deployment = create_deployed_stage(gateway, &api, "prod").await;
    let part = created_part_id(
        gateway,
        &api,
        json!({ "type": "API" }),
        &content_map("the part the rows target"),
        "CreateDocumentationPart",
    )
    .await;
    let version = create_version(gateway, &api, snapshot_request("1.0.0", Some("the snapshot")))
        .await;
    assert_eq!(
        version.status, 201,
        "case CreateDocumentationVersion: expected 201, got {} with {:?}",
        version.status, version.body
    );
    created_model(
        gateway,
        &api,
        "CreateModel",
        json!({
            "name": "Pet",
            "contentType": "application/json",
            "description": "the model the rows target",
            "schema": json_schema("Pet", 1),
        }),
    )
    .await;
    let validator = created_validator(
        gateway,
        &api,
        "CreateRequestValidator",
        json!({ "name": "body-only", "validateRequestBody": true }),
    )
    .await;
    let validator = string_member(&validator, "id", "CreateRequestValidator");
    let authorizer = created_authorizer(gateway, &api, "CreateAuthorizer", token_request("auth"))
        .await;
    let authorizer = string_member(&authorizer, "id", "CreateAuthorizer");
    let plan = create_usage_plan(
        gateway,
        "CreateUsagePlan",
        json!({ "name": "plan", "description": "the plan the rows target" }),
    )
    .await;
    let plan = string_member(&plan, "id", "CreateUsagePlan");

    vec![
        Target {
            operation: "UpdateAccount",
            uri: "/account".to_string(),
            replace_paths: vec!["/cloudwatchRoleArn"],
            // The account publishes its one patchable path whichever of the three ops
            // named the rejected one, as a recorded refusal of that path under an `add`
            // shows: the enumeration is what the resource PUBLISHES, not the set of ops
            // the member accepts.
            add_paths: vec!["/cloudwatchRoleArn"],
            remove_paths: vec!["/cloudwatchRoleArn"],
            invalid_op: INVALID_OP,
            accepted: ("/cloudwatchRoleArn", CLOUDWATCH_ROLE_ARN, "cloudwatchRoleArn"),
        },
        Target {
            operation: "UpdateDeployment",
            uri: format!("/restapis/{api}/deployments/{deployment}"),
            replace_paths: vec!["/description"],
            add_paths: vec![],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: ("/description", "the patched deployment", "description"),
        },
        Target {
            operation: "UpdateDocumentationPart",
            uri: format!("/restapis/{api}/documentation/parts/{part}"),
            replace_paths: vec!["/properties"],
            add_paths: vec![],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: (
                "/properties",
                "{\"description\":\"the patched part\"}",
                "properties",
            ),
        },
        Target {
            operation: "UpdateDocumentationVersion",
            uri: format!("/restapis/{api}/documentation/versions/1.0.0"),
            replace_paths: vec!["/description"],
            add_paths: vec![],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: ("/description", "the patched snapshot", "description"),
        },
        Target {
            operation: "UpdateModel",
            uri: format!("/restapis/{api}/models/Pet"),
            replace_paths: vec!["/description", "/schema"],
            add_paths: vec![],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: ("/description", "the patched model", "description"),
        },
        Target {
            operation: "UpdateRequestValidator",
            uri: format!("/restapis/{api}/requestvalidators/{validator}"),
            replace_paths: vec!["/name", "/validateRequestParameters", "/validateRequestBody"],
            add_paths: vec![],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: ("/name", "renamed-validator", "name"),
        },
        Target {
            operation: "UpdateAuthorizer",
            uri: format!("/restapis/{api}/authorizers/{authorizer}"),
            replace_paths: vec![
                "/name",
                "/type",
                "/authType",
                "/authorizerUri",
                "/authorizerCredentials",
                "/identitySource",
                "/identityValidationExpression",
                "/authorizerResultTtlInSeconds",
            ],
            // An `add` names a MULTI-VALUED member of the authorizer — the user-pool ARNs
            // and the identity sources, each of which an element is added to — so that op
            // enumerates its own set. A `remove` publishes none.
            add_paths: vec!["/providerARNs", "/identitySource"],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: ("/name", "renamed-authorizer", "name"),
        },
        Target {
            operation: "UpdateResource",
            uri: format!("/restapis/{api}/resources/{resource}"),
            replace_paths: vec!["/parentId", "/pathPart"],
            add_paths: vec![],
            remove_paths: vec![],
            invalid_op: INVALID_OP,
            accepted: ("/pathPart", "kittens", "pathPart"),
        },
        Target {
            operation: "UpdateUsagePlan",
            uri: format!("/usageplans/{plan}"),
            replace_paths: vec![
                "/name",
                "/description",
                "/productCode",
                "/quota/period",
                "/quota/limit",
                "/quota/offset",
                "/throttle/rateLimit",
                "/throttle/burstLimit",
                "/apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/rateLimit",
                "/apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/burstLimit",
                "/apiStages/<apidId:stageName>/throttle",
            ],
            // An API stage association is one ELEMENT of a list, so it heads the sets these
            // two ops publish and is in neither the replace set; the plan's `name` and
            // `description` are the other way round. A `remove` drops a nested setting
            // structure whole where an `add` names the structure's own members.
            add_paths: vec![
                "/apiStages",
                "/productCode",
                "/quota/period",
                "/quota/limit",
                "/quota/offset",
                "/throttle/rateLimit",
                "/throttle/burstLimit",
                "/apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/rateLimit",
                "/apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/burstLimit",
                "/apiStages/<apidId:stageName>/throttle",
            ],
            remove_paths: vec![
                "/apiStages",
                "/quota",
                "/throttle",
                "/productCode",
                "/apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>",
                "/apiStages/<apidId:stageName>/throttle",
            ],
            invalid_op: INVALID_OP,
            accepted: ("/name", "renamed-plan", "name"),
        },
    ]
}

/// The forms of unapplicable path each target is driven with: a member no shape declares, a
/// member the service assigns, a pointer reaching INTO one of the resource's own
/// single-valued members, a path that is no JSON Pointer at all, and an empty path.
fn unapplicable_paths(target: &Target) -> Vec<String> {
    let into_a_member = format!("{}/first", target.replace_paths[0]);
    vec![
        "/sim2realNoSuchField".to_string(),
        "/id".to_string(),
        into_a_member,
        "notAPointer".to_string(),
        String::new(),
    ]
}

/// Assert `answer` is the modelled refusal of `path` under `kind`, and that the enumerated
/// list — the information the message exists to convey — does not offer the caller the very
/// path it just rejected.
fn assert_refused(
    answer: &crate::rest_api_lifecycle::Answer,
    target: &Target,
    path: &str,
    kind: &str,
) {
    let case = format!("{}({kind} {path:?})", target.operation);
    assert_failure(answer, 400, "BadRequestException", &case);
    let expected = target.expected(path, kind);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(expected.as_str()),
        "case {case}: expected {expected:?}, got {:?}",
        answer.body["message"]
    );
    assert!(
        !target.published(kind).contains(&path),
        "case {case}: the enumerated list {:?} must not hold the rejected path {path:?}",
        target.published(kind)
    );
}

#[tokio::test]
async fn an_unapplicable_patch_path_names_the_op_and_the_paths_the_resource_publishes() {
    let gateway = Gateway::new();
    let targets = targets(&gateway).await;

    for target in &targets {
        for path in unapplicable_paths(target) {
            for kind in PATH_OPS {
                let answer = patch(&gateway, &target.uri, vec![op(kind, &path, Some("x"))]).await;
                assert_refused(&answer, target, &path, kind);
            }
        }
    }
}

#[tokio::test]
async fn an_op_the_resource_supports_no_path_for_is_refused_as_an_op() {
    let gateway = Gateway::new();
    let targets = targets(&gateway).await;

    // One path no resource accepts, under each op the model's `Op` enum declares beyond the
    // three a path can ride in on: there is no list to enumerate for them, so the op itself
    // is what the refusal names.
    for target in &targets {
        for kind in OP_ONLY_REFUSALS {
            let answer = patch(
                &gateway,
                &target.uri,
                vec![op(kind, "/sim2realNoSuchField", Some("x"))],
            )
            .await;
            assert_refused(&answer, target, "/sim2realNoSuchField", kind);
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_offending_entry_of_a_batch_and_applies_none_of_it() {
    let gateway = Gateway::new();
    let targets = targets(&gateway).await;

    for target in &targets {
        let (accepted, value, member) = target.accepted;
        // Rows differ in where the unapplicable entry sits in the batch: the message must
        // name THAT entry's path, whichever position it took, and the applicable entry
        // beside it must not be kept.
        let rows: Vec<(&str, Vec<Value>)> = vec![
            (
                "the last of two entries",
                vec![
                    replace(accepted, value),
                    replace("/sim2realNoSuchField", "x"),
                ],
            ),
            (
                "the first of two entries",
                vec![
                    replace("/sim2realNoSuchField", "x"),
                    replace(accepted, value),
                ],
            ),
        ];

        for (position, operations) in rows {
            let case = format!("{}({position})", target.operation);
            let answer = patch(&gateway, &target.uri, operations).await;
            assert_refused(&answer, target, "/sim2realNoSuchField", "replace");

            let read = gateway.call("GET", &target.uri, None).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from the resource's reader, got {} with {:?}",
                read.status, read.body
            );
            assert_ne!(
                read.body[member],
                json!(value),
                "case {case}: a refused list must leave {member} unapplied, got {:?}",
                read.body[member]
            );
        }
    }
}

#[tokio::test]
async fn a_published_path_is_accepted_and_reads_back_through_the_resources_own_reader() {
    let gateway = Gateway::new();
    let targets = targets(&gateway).await;

    // The other side of the rule: a path the refusal offers is one the resource accepts, so
    // each target's published path takes a `replace` and the resource reports it afterwards.
    for target in &targets {
        let (path, value, member) = target.accepted;
        let case = format!("{}(replace {path})", target.operation);
        let answer = patch(&gateway, &target.uri, vec![replace(path, value)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body[member],
            json!(value),
            "case {case}: the update must report {member} as {value:?}, got {:?}",
            answer.body[member]
        );

        let read = gateway.call("GET", &target.uri, None).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from the resource's reader, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body[member],
            json!(value),
            "case {case}: the reader must read back {member} as {value:?}, got {:?}",
            read.body[member]
        );
    }
}

#[tokio::test]
async fn an_op_a_stage_takes_no_path_for_is_refused_with_the_stages_own_op_list() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-op-grammar").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // Rows differ in the stage member the pointer names — a single-valued optional member,
    // a boolean one, an entry of a map member, the deployment the stage serves, the cache
    // capacity — and in the op that named it: an op the RESOURCE takes no path for is
    // refused as an op, so the sentence is the stage's own enumeration whichever member the
    // pointer named. A `remove` is not one of those ops — a stage publishes the per-method
    // settings path for it — so a `remove` the stage cannot apply is refused as that path
    // instead, which the `stage_remove_patch_grammar` suite pins.
    let rows: [(&str, &str); 6] = [
        ("/description", "move"),
        ("/description", "test"),
        ("/tracingEnabled", "move"),
        ("/variables/color", "move"),
        ("/deploymentId", "test"),
        ("/cacheClusterSize", "move"),
    ];

    for (path, kind) in rows {
        let case = format!("UpdateStage({kind} {path})");
        let answer = patch(&gateway, &uri, vec![op(kind, path, Some("x"))]).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(STAGE_INVALID_OP),
            "case {case}: expected {STAGE_INVALID_OP:?}, got {:?}",
            answer.body["message"]
        );
    }
}

/// The pointers a stage's `accessLogSettings` publishes, spelled from the stage root: the
/// two members `AccessLogSettings` declares (`smithy_query(query="shape",
/// shape="AccessLogSettings")`), in the order the structure's own refusal enumerates them.
const ACCESS_LOG_PATHS: [&str; 2] = [
    "/accessLogSettings/destinationArn",
    "/accessLogSettings/format",
];

/// The sentence a stage answers a pointer into `accessLogSettings` that it cannot apply
/// with: the member and its pointers, and nothing of the rejected path.
fn access_log_refusal() -> String {
    format!(
        "Invalid accessLogSettings path. Must be one of : [{}]",
        ACCESS_LOG_PATHS.join(", ")
    )
}

#[tokio::test]
async fn a_pointer_into_access_log_settings_is_refused_by_the_structure_not_the_stage() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "access-log-settings-paths").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // Rows differ along the FORM of the pointer — the structure named as a whole by an op
    // that cannot address it that way, a sub-member the structure does not declare, and a
    // pointer reaching PAST each of the two it does — and along the op that named it. A
    // `remove` of the structure itself is not a row: it clears the whole configuration,
    // which is the one way that pointer applies.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("/accessLogSettings", vec!["replace", "add"]),
        ("/accessLogSettings/sim2realNoSuchField", PATH_OPS.to_vec()),
        ("/accessLogSettings/destinationArn/first", PATH_OPS.to_vec()),
        ("/accessLogSettings/format/0", PATH_OPS.to_vec()),
    ];

    let expected = access_log_refusal();
    for (path, kinds) in rows {
        for kind in kinds {
            let case = format!("UpdateStage({kind} {path})");
            let answer = patch(&gateway, &uri, vec![op(kind, path, Some("x"))]).await;
            assert_failure(&answer, 400, "BadRequestException", &case);
            assert_eq!(
                answer.body["message"].as_str(),
                Some(expected.as_str()),
                "case {case}: expected {expected:?}, got {:?}",
                answer.body["message"]
            );
        }
    }

    // An op the STAGE takes no path for is refused as an op, with the stage's own
    // enumeration of the ops it does take. A `copy` is not one of them: a stage does take a
    // `copy` (the canary promotion), so a `copy` naming a pointer outside its copy grammar
    // is refused by that grammar instead, and the sibling `update_stage_copy_grammar`
    // suite pins that sentence.
    for kind in OP_ONLY_REFUSALS.iter().filter(|kind| **kind != "copy") {
        let case = format!("UpdateStage({kind} /accessLogSettings)");
        let answer = patch(
            &gateway,
            &uri,
            vec![op(kind, "/accessLogSettings", Some("x"))],
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(STAGE_INVALID_OP),
            "case {case}: expected {STAGE_INVALID_OP:?}, got {:?}",
            answer.body["message"]
        );
    }
}

#[tokio::test]
async fn every_access_log_settings_pointer_the_refusal_names_is_accepted_and_reads_back() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "access-log-settings-accepted").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // The other side of the rule: each pointer the structure's refusal enumerates takes a
    // `replace` and the stage reports it afterwards. The format carries
    // `$context.requestId`, which `AccessLogSettings$format` requires of every format.
    let values = [
        (
            "/accessLogSettings/destinationArn",
            "destinationArn",
            "arn:aws:logs:us-east-1:123456789012:log-group:api-access",
        ),
        (
            "/accessLogSettings/format",
            "format",
            "$context.requestId $context.status",
        ),
    ];
    for (path, member, value) in values {
        let case = format!("UpdateStage(replace {path})");
        let answer = patch(&gateway, &uri, vec![replace(path, value)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["accessLogSettings"][member],
            json!(value),
            "case {case}: UpdateStage must report accessLogSettings.{member} as {value:?}, got {:?}",
            answer.body["accessLogSettings"][member]
        );

        let read = get_stage(&gateway, &api, "prod").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetStage, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["accessLogSettings"][member],
            json!(value),
            "case {case}: GetStage must read back accessLogSettings.{member} as {value:?}, got {:?}",
            read.body["accessLogSettings"][member]
        );
    }

    // And the pointers the enumeration does NOT name are refused on the same stage, so the
    // list is exactly the set that applies.
    let expected = access_log_refusal();
    let answer = patch(
        &gateway,
        &uri,
        vec![replace("/accessLogSettings/destination", "x")],
    )
    .await;
    assert_failure(
        &answer,
        400,
        "BadRequestException",
        "UpdateStage(replace /accessLogSettings/destination)",
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(expected.as_str()),
        "case UpdateStage(replace /accessLogSettings/destination): expected {expected:?}, got {:?}",
        answer.body["message"]
    );
}
