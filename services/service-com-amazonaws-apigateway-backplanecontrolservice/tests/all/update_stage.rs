// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateStage`: applying `patchOperations` to a stored `Stage`
//! (`PATCH /restapis/{restApiId}/stages/{stageName}`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `stage_lifecycle`.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded call:
//!
//! * a supported `path` sets the member of the stored stage that the JSON Pointer names,
//!   and the stage reads back through `GetStage` exactly as the update reported it —
//!   asserted over the single-valued members, the `variables` map, the `canarySettings`
//!   and `accessLogSettings` structures and the per-method settings;
//! * a value outside a member's published value set or range, a path the `Stage` shape
//!   has no updateable member for, and an op a member does not support are each refused,
//!   and a refusal leaves the record exactly as it was;
//! * enabling, resizing and disabling the stage's cache cluster are transitions the CLOCK
//!   owes, observable as `CREATE_IN_PROGRESS` / `DELETE_IN_PROGRESS` until their span has
//!   passed;
//! * a method path that has no settings yet takes API Gateway's documented defaults for
//!   the settings the patch did not name;
//! * a deployment id that names no deployment of the addressed API is refused by its own
//!   class AND its own sentence depending on WHERE the request carries it: as a VALUE (a
//!   `/deploymentId` or `/canarySettings/deploymentId` patch value, and the same members on
//!   `CreateStage`) it is the bad request a caller fixes by sending another value, while in
//!   the URI it is the not-found of a resource that is gone — neither refusal reuses the
//!   other's sentence.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    CACHE_CLUSTER_AVAILABLE, CACHE_CLUSTER_CREATE_IN_PROGRESS, CACHE_CLUSTER_CREATE_SPAN,
    CACHE_CLUSTER_DELETE_IN_PROGRESS, CACHE_CLUSTER_DELETE_SPAN, CACHE_CLUSTER_NOT_AVAILABLE,
};

use crate::account_settings::{patch_account, role_arn};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// A margin on top of a stored span, so a test waiting one out is not racing it.
const MARGIN: Duration = Duration::from_millis(150);

/// One row of the member table: the case name, the stage the row patches, the ops it
/// sends, and the JSON pointers the updated stage must report, each with its value.
type MemberCase = (
    &'static str,
    &'static str,
    Value,
    Vec<(&'static str, Value)>,
);

/// `UpdateStage` for `stage_name` of `api` with `operations`.
async fn update_stage(gateway: &Gateway, api: &str, stage_name: &str, operations: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{stage_name}"),
            Some(json!({ "patchOperations": operations })),
        )
        .await
}

/// One `replace` of `path` with `value`, the op shape every row below is built from.
fn replace(path: &str, value: &str) -> Value {
    json!([{ "op": "replace", "path": path, "value": value }])
}

/// An API with one deployment, and a second deployment to re-point a stage at.
async fn api_with_two_deployments(gateway: &Gateway, name: &str) -> (String, String, String) {
    let (api, _) = create_api(gateway, name).await;
    let first = create_deployment(gateway, &api, &format!("{name}/first"), json!({})).await;
    let second = create_deployment(gateway, &api, &format!("{name}/second"), json!({})).await;
    (api, first, second)
}

/// Create `stage_name` on `api` pointing at `deployment`, from `extra`.
async fn create_stage(
    gateway: &Gateway,
    api: &str,
    deployment: &str,
    stage_name: &str,
    extra: Value,
) -> Value {
    let mut request = json!({ "stageName": stage_name, "deploymentId": deployment });
    if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
    let created = gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await;
    assert_eq!(
        created.status, 201,
        "case CreateStage({stage_name}): expected 201, got {} with {:?}",
        created.status, created.body
    );
    created.body
}

#[tokio::test]
async fn a_patch_sets_the_member_its_path_names_and_the_stage_reads_back() {
    // Rows differ along what the rule generalises over: which member the pointer names,
    // its type (string, boolean, double, a map entry, a sub-member of a nested
    // structure, a per-method setting), the op, and how many ops the list carries.
    // `expected` is the member path and the value the stage must report afterwards.
    let cases: Vec<MemberCase> = vec![
        (
            "a single-valued string member",
            "s_description",
            replace("/description", "the production stage"),
            vec![("description", json!("the production stage"))],
        ),
        (
            "clearing an optional string member through a valueless replace",
            "s_undescribed",
            json!([{ "op": "replace", "path": "/description", "value": "" }]),
            vec![("description", Value::Null)],
        ),
        (
            "a boolean member",
            "s_traced",
            replace("/tracingEnabled", "true"),
            vec![("tracingEnabled", json!(true))],
        ),
        (
            "another string member",
            "s_documented",
            json!([{ "op": "replace", "path": "/documentationVersion", "value": "v2" }]),
            vec![("documentationVersion", json!("v2"))],
        ),
        (
            "an entry added to the variables map",
            "s_vars",
            json!([{ "op": "add", "path": "/variables/table", "value": "pets" }]),
            vec![("variables", json!({ "table": "pets" }))],
        ),
        (
            "two map entries and one of them dropped again",
            "s_vars2",
            json!([
                { "op": "add", "path": "/variables/table", "value": "pets" },
                { "op": "add", "path": "/variables/region", "value": "us-east-1" },
                { "op": "remove", "path": "/variables/table" }
            ]),
            vec![("variables", json!({ "region": "us-east-1" }))],
        ),
        (
            "a double sub-member of canarySettings at its lower bound",
            "s_canary0",
            replace("/canarySettings/percentTraffic", "0"),
            vec![("canarySettings/percentTraffic", json!(0.0))],
        ),
        (
            "a double sub-member of canarySettings at its upper bound",
            "s_canary100",
            replace("/canarySettings/percentTraffic", "100"),
            vec![("canarySettings/percentTraffic", json!(100.0))],
        ),
        (
            "a fractional percentage and the canary cache flag",
            "s_canary_frac",
            json!([
                { "op": "replace", "path": "/canarySettings/percentTraffic", "value": "12.5" },
                { "op": "replace", "path": "/canarySettings/useStageCache", "value": "true" },
                { "op": "add", "path": "/canarySettings/stageVariableOverrides/table",
                  "value": "pets-canary" }
            ]),
            vec![
                ("canarySettings/percentTraffic", json!(12.5)),
                ("canarySettings/useStageCache", json!(true)),
                (
                    "canarySettings/stageVariableOverrides",
                    json!({ "table": "pets-canary" }),
                ),
            ],
        ),
        (
            "the canary structure removed whole",
            "s_canary_gone",
            json!([
                { "op": "replace", "path": "/canarySettings/percentTraffic", "value": "5" },
                { "op": "remove", "path": "/canarySettings" }
            ]),
            vec![("canarySettings", Value::Null)],
        ),
        (
            "the sub-members of accessLogSettings",
            "s_logs",
            json!([
                { "op": "replace", "path": "/accessLogSettings/destinationArn",
                  "value": "arn:aws:logs:us-east-1:000000000000:log-group:api" },
                { "op": "replace", "path": "/accessLogSettings/format",
                  "value": "$context.requestId $context.status" }
            ]),
            vec![
                (
                    "accessLogSettings/destinationArn",
                    json!("arn:aws:logs:us-east-1:000000000000:log-group:api"),
                ),
                (
                    "accessLogSettings/format",
                    json!("$context.requestId $context.status"),
                ),
            ],
        ),
        (
            "access log settings removed whole",
            "s_logs_gone",
            json!([
                { "op": "replace", "path": "/accessLogSettings/destinationArn",
                  "value": "arn:aws:logs:us-east-1:000000000000:log-group:api" },
                { "op": "remove", "path": "/accessLogSettings" }
            ]),
            vec![("accessLogSettings", Value::Null)],
        ),
        (
            "a per-method setting for every method of the stage",
            "s_methods_all",
            json!([
                { "op": "replace", "path": "/*/*/caching/enabled", "value": "true" },
                { "op": "replace", "path": "/*/*/caching/ttlInSeconds", "value": "60" },
                { "op": "replace", "path": "/*/*/logging/loglevel", "value": "INFO" },
                { "op": "replace", "path": "/*/*/metrics/enabled", "value": "true" }
            ]),
            vec![
                ("methodSettings/*~1*/cachingEnabled", json!(true)),
                ("methodSettings/*~1*/cacheTtlInSeconds", json!(60)),
                ("methodSettings/*~1*/loggingLevel", json!("INFO")),
                ("methodSettings/*~1*/metricsEnabled", json!(true)),
            ],
        ),
        (
            "a per-method setting for one method only",
            "s_methods_one",
            json!([
                { "op": "replace", "path": "/~1pets/GET/throttling/rateLimit", "value": "25.5" },
                { "op": "replace", "path": "/~1pets/GET/throttling/burstLimit", "value": "7" },
                { "op": "replace", "path": "/~1pets/GET/logging/dataTrace", "value": "true" },
                { "op": "replace", "path": "/~1pets/GET/caching/dataEncrypted", "value": "true" }
            ]),
            vec![
                // The `methodSettings` key is `~1pets/GET`: the method path the pointer
                // SPELLED — its tokens before the setting, as written — which these JSON
                // Pointer tokens spell with the key's own separator escaped `~1`.
                (
                    "methodSettings/~01pets~1GET/throttlingRateLimit",
                    json!(25.5),
                ),
                (
                    "methodSettings/~01pets~1GET/throttlingBurstLimit",
                    json!(7),
                ),
                (
                    "methodSettings/~01pets~1GET/dataTraceEnabled",
                    json!(true),
                ),
                (
                    "methodSettings/~01pets~1GET/cacheDataEncrypted",
                    json!(true),
                ),
            ],
        ),
        (
            "the cache invalidation settings",
            "s_methods_invalidation",
            json!([
                { "op": "replace",
                  "path": "/*/*/caching/requireAuthorizationForCacheControl",
                  "value": "false" },
                { "op": "replace",
                  "path": "/*/*/caching/unauthorizedCacheControlHeaderStrategy",
                  "value": "FAIL_WITH_403" }
            ]),
            vec![
                (
                    "methodSettings/*~1*/requireAuthorizationForCacheControl",
                    json!(false),
                ),
                (
                    "methodSettings/*~1*/unauthorizedCacheControlHeaderStrategy",
                    json!("FAIL_WITH_403"),
                ),
            ],
        ),
    ];

    let gateway = Gateway::new();
    // Rows below turn a method's execution logging on, which API Gateway writes under the
    // ACCOUNT's CloudWatch Logs role, so the account carries one before the table runs —
    // the precondition the sibling `stage_execution_logging_account_role` suite covers.
    patch_account(
        &gateway,
        "replace",
        "/cloudwatchRoleArn",
        Some(&role_arn("update-stage-members")),
    )
    .await;

    for (index, (case, stage_name, operations, expected)) in cases.iter().enumerate() {
        // Each row gets its own API, so the table can be as wide as the rule needs
        // without running into the per-API stage quota.
        let (api, deployment, _) =
            api_with_two_deployments(&gateway, &format!("update-stage-members-{index}")).await;
        // Every row starts from a stage carrying a description, so the row that clears it
        // has something to clear.
        create_stage(
            &gateway,
            &api,
            &deployment,
            stage_name,
            json!({ "description": "as created" }),
        )
        .await;

        let updated = update_stage(&gateway, &api, stage_name, operations.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateStage, got {} with {:?}",
            updated.status, updated.body
        );
        for (pointer, value) in expected {
            let at = format!("/{pointer}");
            let actual = updated.body.pointer(&at).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, *value,
                "case {case}: expected {pointer} to be {value:?}, got {actual:?}"
            );
        }

        // The update reports the STORED record: a read of the stage answers the same
        // body, member for member.
        let read = get_stage(&gateway, &api, stage_name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetStage, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, updated.body,
            "case {case}: GetStage must report the updated stage, got {:?} after {:?}",
            read.body, updated.body
        );
        // The stage keeps its identity: a patch of its members is not a new stage.
        assert_eq!(
            read.body["stageName"].as_str(),
            Some(*stage_name),
            "case {case}: expected the stage name to be unchanged, got {:?}",
            read.body["stageName"]
        );
        assert_eq!(
            read.body["deploymentId"].as_str(),
            Some(deployment.as_str()),
            "case {case}: expected the stage to still serve {deployment}, got {:?}",
            read.body["deploymentId"]
        );
    }
}

#[tokio::test]
async fn a_method_path_with_no_settings_yet_takes_the_documented_defaults() {
    // Rows span the spellings of a method path the patch takes, and the key each
    // produces: the stage-wide wildcard, one method of one resource and one of a nested
    // resource spelled in two reference tokens — every one of them keyed by the method path
    // its POINTER spelled, which is the pointer without its own leading separator.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("every method of the stage", "/*/*", "*/*"),
        ("one method of one resource", "/~1pets/GET", "~1pets/GET"),
        (
            "one method of a nested resource",
            "/~1pets~1{petId}/DELETE",
            "~1pets~1{petId}/DELETE",
        ),
    ];

    let gateway = Gateway::new();
    let (api, deployment, _) = api_with_two_deployments(&gateway, "update-stage-defaults").await;

    for (index, (case, method_path, key)) in cases.iter().enumerate() {
        let stage_name = format!("defaults_{index}");
        create_stage(&gateway, &api, &deployment, &stage_name, json!({})).await;

        let updated = update_stage(
            &gateway,
            &api,
            &stage_name,
            replace(&format!("{method_path}/caching/enabled"), "true"),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateStage, got {} with {:?}",
            updated.status, updated.body
        );
        // The settings the patch did NOT name carry API Gateway's own defaults: the
        // documented 300-second cache TTL, the default account-level throttle of 10,000
        // requests per second with a burst of 5,000, and cache invalidation requiring
        // authorization and answering with the response header. `loggingLevel` is the one
        // member the model declares without a default, so it stays absent.
        let expected = json!({
            "metricsEnabled": false,
            "dataTraceEnabled": false,
            "throttlingBurstLimit": 5000,
            "throttlingRateLimit": 10000.0,
            "cachingEnabled": true,
            "cacheTtlInSeconds": 300,
            "cacheDataEncrypted": false,
            "requireAuthorizationForCacheControl": true,
            "unauthorizedCacheControlHeaderStrategy": "SUCCEED_WITH_RESPONSE_HEADER"
        });
        assert_eq!(
            updated.body["methodSettings"],
            json!({ key.to_string(): expected }),
            "case {case}: expected the entry under {key:?} with the documented defaults, got {:?}",
            updated.body["methodSettings"]
        );
        let read = get_stage(&gateway, &api, &stage_name).await;
        assert_eq!(
            read.body["methodSettings"], updated.body["methodSettings"],
            "case {case}: GetStage must report the stored method settings, got {:?}",
            read.body["methodSettings"]
        );
    }
}

#[tokio::test]
async fn a_patch_the_stage_shape_does_not_support_is_refused_and_writes_nothing() {
    // Rows span the refusal classes: a path the shape has no updateable member for, an op
    // the member does not support, a value outside a published value set, a value outside
    // a published range (and one past each bound), and an identifier that resolves to no
    // record. `(case, operations, status, code)`.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a member the service assigns",
            replace("/stageName", "renamed"),
            400,
            "BadRequestException",
        ),
        (
            "a member the shape does not declare",
            replace("/nonesuch", "x"),
            400,
            "BadRequestException",
        ),
        (
            "a path reaching into a single-valued member",
            replace("/description/first", "x"),
            400,
            "BadRequestException",
        ),
        (
            "removing the deployment a stage always points at",
            json!([{ "op": "remove", "path": "/deploymentId" }]),
            400,
            "BadRequestException",
        ),
        (
            "an op no member of this resource supports",
            json!([{ "op": "move", "path": "/description", "from": "/documentationVersion" }]),
            400,
            "BadRequestException",
        ),
        (
            "an add carrying no value",
            json!([{ "op": "add", "path": "/description" }]),
            400,
            "BadRequestException",
        ),
        (
            "a percentTraffic above its published range",
            replace("/canarySettings/percentTraffic", "100.1"),
            400,
            "BadRequestException",
        ),
        (
            "a percentTraffic below its published range",
            replace("/canarySettings/percentTraffic", "-0.5"),
            400,
            "BadRequestException",
        ),
        (
            "a percentTraffic that is not a number",
            replace("/canarySettings/percentTraffic", "half"),
            400,
            "BadRequestException",
        ),
        (
            // A stage presents "the identifier of a client certificate for an API stage",
            // so the value names one the ACCOUNT has; `GenerateClientCertificate` is the
            // operation that mints one and it is still scaffolded, so an account has none.
            "a client certificate this account does not have",
            replace("/clientCertificateId", "abcdef1234"),
            400,
            "BadRequestException",
        ),
        (
            "an access log destination that is not an ARN",
            replace("/accessLogSettings/destinationArn", "not-an-arn"),
            400,
            "BadRequestException",
        ),
        (
            "a logging level outside its documented values",
            replace("/*/*/logging/loglevel", "VERBOSE"),
            400,
            "BadRequestException",
        ),
        (
            "a cache TTL one past its documented maximum",
            replace("/*/*/caching/ttlInSeconds", "3601"),
            400,
            "BadRequestException",
        ),
        (
            "a negative cache TTL",
            replace("/*/*/caching/ttlInSeconds", "-1"),
            400,
            "BadRequestException",
        ),
        (
            // The one negative value either method-setting limit takes is the sentinel that
            // asks for no throttle at all; every other negative number is neither a count of
            // requests nor a rate.
            "a negative throttle rate past the unthrottled sentinel",
            replace("/*/*/throttling/rateLimit", "-2"),
            400,
            "BadRequestException",
        ),
        (
            "a method path naming no setting",
            replace("/~1pets/GET", "true"),
            400,
            "BadRequestException",
        ),
        (
            "a method path naming a setting that does not exist",
            replace("/*/*/caching/warmth", "true"),
            400,
            "BadRequestException",
        ),
        (
            "an access log format without the required context variable",
            replace("/accessLogSettings/format", "$context.status"),
            400,
            "BadRequestException",
        ),
        (
            // A patch VALUE that names no record is a refusal of the REQUEST: the stage
            // and the API the URI addresses both exist, so the caller has to fix the
            // value rather than abandon the stage.
            "a deployment this API does not have",
            replace("/deploymentId", "zzzzzzzzzz"),
            400,
            "BadRequestException",
        ),
        (
            "a canary deployment this API does not have",
            replace("/canarySettings/deploymentId", "zzzzzzzzzz"),
            400,
            "BadRequestException",
        ),
        (
            "an unsupported op later in the list",
            json!([
                { "op": "replace", "path": "/description", "value": "applied first" },
                { "op": "replace", "path": "/nonesuch", "value": "refused" }
            ]),
            400,
            "BadRequestException",
        ),
    ];

    let gateway = Gateway::new();
    let (api, deployment, _) = api_with_two_deployments(&gateway, "update-stage-refusals").await;
    let before = create_stage(
        &gateway,
        &api,
        &deployment,
        "refusals",
        json!({ "description": "as created", "variables": { "table": "pets" } }),
    )
    .await;

    for (case, operations, status, code) in &cases {
        let answer = update_stage(&gateway, &api, "refusals", operations.clone()).await;
        assert_failure(&answer, *status, code, case);
        // A refused patch writes NOTHING — not even the ops before the one that failed.
        let read = get_stage(&gateway, &api, "refusals").await;
        assert_eq!(
            read.body, before,
            "case {case}: a refused patch must leave the stage as it was, expected {before:?}, \
             got {:?}",
            read.body
        );
    }

    // The same values INSIDE the published bounds are accepted, so each refusal above is
    // about the bound and not about the path: percentTraffic at both ends of 0-100 and a
    // cache TTL at both ends of 0-3600.
    for (case, operations) in [
        (
            "percentTraffic at its lower bound",
            replace("/canarySettings/percentTraffic", "0"),
        ),
        (
            "percentTraffic at its upper bound",
            replace("/canarySettings/percentTraffic", "100"),
        ),
        (
            "a cache TTL of zero",
            replace("/*/*/caching/ttlInSeconds", "0"),
        ),
        (
            "a cache TTL at its documented maximum",
            replace("/*/*/caching/ttlInSeconds", "3600"),
        ),
        (
            "a cache size from the published catalog",
            replace("/cacheClusterSize", "1.6"),
        ),
        (
            // The capacity is stored as the request spells it: the published catalogue
            // constrains the modelled member, not the string a patch value carries.
            "a cache size outside the published catalog",
            replace("/cacheClusterSize", "2.0"),
        ),
        (
            // A boolean patch value is read, not validated: the setting is true when the
            // value spells true and false for any other word.
            "a boolean setting given a word that is not true",
            replace("/*/*/caching/enabled", "yes"),
        ),
    ] {
        let answer = update_stage(&gateway, &api, "refusals", operations).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 inside the published bound, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn an_empty_patch_list_and_an_unresolvable_target_change_nothing() {
    let gateway = Gateway::new();
    let (api, deployment, _) = api_with_two_deployments(&gateway, "update-stage-noop").await;
    let (other_api, _) = create_api(&gateway, "update-stage-noop-other").await;
    let before = create_stage(
        &gateway,
        &api,
        &deployment,
        "prod",
        json!({ "description": "as created" }),
    )
    .await;

    // An empty list — and a request carrying no list at all — is accepted and changes no
    // member of the record, its `lastUpdatedDate` included.
    for (case, body) in [
        ("an empty patchOperations list", json!({ "patchOperations": [] })),
        ("no patchOperations member", json!({})),
    ] {
        let answer = gateway
            .call("PATCH", &format!("/restapis/{api}/stages/prod"), Some(body))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body, before,
            "case {case}: an update that patches nothing must report the stored stage, \
             expected {before:?}, got {:?}",
            answer.body
        );
    }

    // Rows span each identifier the call resolves, one unresolvable at a time.
    let missing = "zzzzzzzzzz";
    for (case, target) in [
        ("an unknown restApiId", format!("{missing}/prod")),
        ("an unknown stageName", format!("{api}/{missing}")),
        (
            "a stage name stored on another API",
            format!("{other_api}/prod"),
        ),
    ] {
        let (target_api, stage) = target
            .split_once('/')
            .expect("every row names an API and a stage");
        assert_failure(
            &update_stage(&gateway, target_api, stage, replace("/description", "x")).await,
            404,
            "NotFoundException",
            case,
        );
    }
}

#[tokio::test]
async fn pointing_a_stage_at_another_deployment_includes_promoting_its_canary() {
    let gateway = Gateway::new();
    let (api, first, second) = api_with_two_deployments(&gateway, "update-stage-deployment").await;
    create_stage(&gateway, &api, &first, "prod", json!({})).await;

    // A replace points the stage at another deployment of the SAME API.
    let repointed = update_stage(&gateway, &api, "prod", replace("/deploymentId", &second)).await;
    assert_eq!(
        repointed.status, 200,
        "case replace /deploymentId: expected 200, got {} with {:?}",
        repointed.status, repointed.body
    );
    assert_eq!(
        repointed.body["deploymentId"].as_str(),
        Some(second.as_str()),
        "case replace /deploymentId: expected the stage to serve {second}, got {:?}",
        repointed.body["deploymentId"]
    );

    // A canary on the first deployment, then the promotion the model documents:
    // "op":"copy", "from":"/canarySettings/deploymentId", "path":"/deploymentId".
    let canary = update_stage(
        &gateway,
        &api,
        "prod",
        json!([
            { "op": "replace", "path": "/canarySettings/deploymentId", "value": first },
            { "op": "replace", "path": "/canarySettings/percentTraffic", "value": "10" }
        ]),
    )
    .await;
    assert_eq!(
        canary.status, 200,
        "case the canary settings: expected 200, got {} with {:?}",
        canary.status, canary.body
    );
    let promoted = update_stage(
        &gateway,
        &api,
        "prod",
        json!([{ "op": "copy", "from": "/canarySettings/deploymentId", "path": "/deploymentId" }]),
    )
    .await;
    assert_eq!(
        promoted.status, 200,
        "case the canary promotion: expected 200, got {} with {:?}",
        promoted.status, promoted.body
    );
    assert_eq!(
        promoted.body["deploymentId"].as_str(),
        Some(first.as_str()),
        "case the canary promotion: expected the canary's deployment {first}, got {:?}",
        promoted.body["deploymentId"]
    );

    // A copy from any other pointer names nothing this resource can copy, and a copy with
    // no canary deployment to read has nothing to promote.
    assert_failure(
        &update_stage(
            &gateway,
            &api,
            "prod",
            json!([{ "op": "copy", "from": "/description", "path": "/deploymentId" }]),
        )
        .await,
        400,
        "BadRequestException",
        "a copy from a pointer the promotion does not name",
    );
    create_stage(&gateway, &api, &first, "nocanary", json!({})).await;
    assert_failure(
        &update_stage(
            &gateway,
            &api,
            "nocanary",
            json!([{ "op": "copy", "from": "/canarySettings/deploymentId",
                     "path": "/deploymentId" }]),
        )
        .await,
        400,
        "BadRequestException",
        "a promotion on a stage with no canary",
    );
}

#[tokio::test]
async fn changing_the_cache_cluster_is_a_transition_the_clock_owes() {
    let gateway = Gateway::new();
    let (api, deployment, _) = api_with_two_deployments(&gateway, "update-stage-cache").await;
    create_stage(&gateway, &api, &deployment, "cached", json!({})).await;

    // A stage created without a cluster has none to report.
    let read = get_stage(&gateway, &api, "cached").await;
    assert_eq!(
        read.body["cacheClusterStatus"].as_str(),
        Some(CACHE_CLUSTER_NOT_AVAILABLE),
        "case the stage before any cache patch: expected {CACHE_CLUSTER_NOT_AVAILABLE}, got {:?}",
        read.body["cacheClusterStatus"]
    );

    // Enabling one provisions it: a caller reading straight after the call still sees it
    // coming up, and it is up once the span the clock owes has passed.
    let enabled = update_stage(
        &gateway,
        &api,
        "cached",
        json!([
            { "op": "replace", "path": "/cacheClusterEnabled", "value": "true" },
            { "op": "replace", "path": "/cacheClusterSize", "value": "0.5" }
        ]),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "case enabling the cluster: expected 200, got {} with {:?}",
        enabled.status, enabled.body
    );
    assert_eq!(
        enabled.body["cacheClusterEnabled"], json!(true),
        "case enabling the cluster: expected cacheClusterEnabled, got {:?}",
        enabled.body["cacheClusterEnabled"]
    );
    assert_eq!(
        enabled.body["cacheClusterStatus"].as_str(),
        Some(CACHE_CLUSTER_CREATE_IN_PROGRESS),
        "case enabling the cluster: expected {CACHE_CLUSTER_CREATE_IN_PROGRESS}, got {:?}",
        enabled.body["cacheClusterStatus"]
    );
    tokio::time::sleep(CACHE_CLUSTER_CREATE_SPAN + MARGIN).await;
    for round in 0..2 {
        let settled = get_stage(&gateway, &api, "cached").await;
        assert_eq!(
            settled.body["cacheClusterStatus"].as_str(),
            Some(CACHE_CLUSTER_AVAILABLE),
            "case the cluster after its span (round {round}): expected {CACHE_CLUSTER_AVAILABLE}, \
             got {:?}",
            settled.body["cacheClusterStatus"]
        );
    }

    // Re-sizing a cluster the stage HAS provisions one of the new capacity, which is a
    // creation the clock owes again.
    let resized = update_stage(&gateway, &api, "cached", replace("/cacheClusterSize", "1.6")).await;
    assert_eq!(
        resized.body["cacheClusterSize"].as_str(),
        Some("1.6"),
        "case re-sizing the cluster: expected the new capacity, got {:?}",
        resized.body["cacheClusterSize"]
    );
    assert_eq!(
        resized.body["cacheClusterStatus"].as_str(),
        Some(CACHE_CLUSTER_CREATE_IN_PROGRESS),
        "case re-sizing the cluster: expected {CACHE_CLUSTER_CREATE_IN_PROGRESS}, got {:?}",
        resized.body["cacheClusterStatus"]
    );
    tokio::time::sleep(CACHE_CLUSTER_CREATE_SPAN + MARGIN).await;

    // Asking for the size it already has provisions nothing, so the cluster stays up.
    let unchanged = update_stage(&gateway, &api, "cached", replace("/cacheClusterSize", "1.6")).await;
    assert_eq!(
        unchanged.body["cacheClusterStatus"].as_str(),
        Some(CACHE_CLUSTER_AVAILABLE),
        "case re-sizing to the capacity it has: expected {CACHE_CLUSTER_AVAILABLE}, got {:?}",
        unchanged.body["cacheClusterStatus"]
    );

    // Disabling it tears it down over the same kind of span, after which the stage has no
    // cluster at all.
    let disabled =
        update_stage(&gateway, &api, "cached", replace("/cacheClusterEnabled", "false")).await;
    assert_eq!(
        disabled.body["cacheClusterEnabled"], json!(false),
        "case disabling the cluster: expected cacheClusterEnabled false, got {:?}",
        disabled.body["cacheClusterEnabled"]
    );
    assert_eq!(
        disabled.body["cacheClusterStatus"].as_str(),
        Some(CACHE_CLUSTER_DELETE_IN_PROGRESS),
        "case disabling the cluster: expected {CACHE_CLUSTER_DELETE_IN_PROGRESS}, got {:?}",
        disabled.body["cacheClusterStatus"]
    );
    tokio::time::sleep(CACHE_CLUSTER_DELETE_SPAN + MARGIN).await;
    for round in 0..2 {
        let gone = get_stage(&gateway, &api, "cached").await;
        assert_eq!(
            gone.body["cacheClusterStatus"].as_str(),
            Some(CACHE_CLUSTER_NOT_AVAILABLE),
            "case the cluster after the teardown (round {round}): expected \
             {CACHE_CLUSTER_NOT_AVAILABLE}, got {:?}",
            gone.body["cacheClusterStatus"]
        );
    }
}

#[tokio::test]
async fn an_update_changes_only_the_stage_it_addresses() {
    let gateway = Gateway::new();
    let (api, deployment, _) = api_with_two_deployments(&gateway, "update-stage-isolation").await;
    let (other_api, _) = create_api(&gateway, "update-stage-isolation-other").await;
    let other_deployment =
        create_deployment(&gateway, &other_api, "the other API's deployment", json!({})).await;

    // Two stages of one API, and the SAME stage name on another API: a stage is addressed
    // by (API, stage), so neither of the other two may move.
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;
    let sibling = create_stage(&gateway, &api, &deployment, "beta", json!({})).await;
    let namesake = create_stage(
        &gateway,
        &other_api,
        &other_deployment,
        "prod",
        json!({}),
    )
    .await;

    let updated = update_stage(
        &gateway,
        &api,
        "prod",
        json!([
            { "op": "replace", "path": "/description", "value": "only this one" },
            { "op": "add", "path": "/variables/table", "value": "pets" },
            { "op": "replace", "path": "/*/*/caching/enabled", "value": "true" }
        ]),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case UpdateStage(prod): expected 200, got {} with {:?}",
        updated.status, updated.body
    );

    for (case, target_api, stage, before) in [
        ("the sibling stage", &api, "beta", &sibling),
        ("the same name on another API", &other_api, "prod", &namesake),
    ] {
        let read = get_stage(&gateway, target_api, stage).await;
        assert_eq!(
            read.body, *before,
            "case {case}: expected the untouched record {before:?}, got {:?}",
            read.body
        );
    }
}

/// API Gateway's sentence for a deployment id a request carries as a VALUE that names no
/// deployment of the api, and the one for a deployment id in the URI that names none.
///
/// Spelled out here rather than read from the crate's own constants — the way
/// `refusal_contract_sentences` pins a refusal's text — so what this suite holds the service
/// to is the WORDING a client keys on and not whichever constant the handler reached for.
/// Both are entirely fixed: neither carries the id it was given.
const UNKNOWN_DEPLOYMENT_VALUE: &str = "Deployment id does not exist";
const UNKNOWN_DEPLOYMENT_IDENTIFIER: &str = "Invalid Deployment identifier specified";

/// Assert `answer` is the refusal a deployment id carried as a request VALUE earns — the
/// bad request a caller answers by sending another value — sentence included.
fn assert_value_refusal(answer: &Answer, case: &str) {
    assert_failure(answer, 400, "BadRequestException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(UNKNOWN_DEPLOYMENT_VALUE),
        "case {case}: expected {UNKNOWN_DEPLOYMENT_VALUE:?}, got {:?}",
        answer.body["message"]
    );
}

/// Assert `answer` is the refusal a deployment id carried in the URI earns — the not-found
/// of a resource that is not there — sentence included.
fn assert_identifier_refusal(answer: &Answer, case: &str) {
    assert_failure(answer, 404, "NotFoundException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(UNKNOWN_DEPLOYMENT_IDENTIFIER),
        "case {case}: expected {UNKNOWN_DEPLOYMENT_IDENTIFIER:?}, got {:?}",
        answer.body["message"]
    );
}

/// The deployment id `stage_name` of `api` serves, read back through `GetStage`.
async fn served_deployment(gateway: &Gateway, api: &str, stage_name: &str, case: &str) -> String {
    let read = get_stage(gateway, api, stage_name).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetStage({stage_name}), got {} with {:?}",
        read.status, read.body
    );
    read.body["deploymentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a deploymentId, got {:?}", read.body))
        .to_owned()
}

#[tokio::test]
async fn an_unknown_deployment_id_is_a_bad_request_as_a_value_and_a_not_found_in_the_uri() {
    let gateway = Gateway::new();
    let (api, served, repointed_to) =
        api_with_two_deployments(&gateway, "update-stage-unknown-deployment").await;
    // A deployment that existed and then did not: the delete is what makes its id name no
    // deployment, so the rule is asserted on an id this service really minted as well as on
    // ids nothing ever minted.
    let deleted = create_deployment(&gateway, &api, "the deleted deployment", json!({})).await;
    let dropped = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/deployments/{deleted}"),
            None,
        )
        .await;
    assert_eq!(
        dropped.status, 202,
        "case DeleteDeployment({deleted}): expected 202, got {} with {:?}",
        dropped.status, dropped.body
    );
    // A deployment of ANOTHER API: a stage serves a deployment of its OWN API, so this id
    // names a real record and still names no deployment of the API patched below.
    let (other_api, _) = create_api(&gateway, "update-stage-unknown-deployment-other").await;
    let foreign =
        create_deployment(&gateway, &other_api, "the other API's deployment", json!({})).await;

    create_stage(&gateway, &api, &served, "prod", json!({})).await;

    // Rows differ along what the rule generalises over: HOW an id comes to name no
    // deployment of this API (never minted, minted and then deleted, minted under another
    // API) and the FORM it takes (letters, digits, and a string longer than a minted id).
    // The second field is the stage-name fragment the rows that create a stage use.
    let rows: Vec<(&str, &str, String)> = vec![
        ("letters only", "letters", "zzzzzzzzzz".to_string()),
        ("digits only", "digits", "0123456789".to_string()),
        (
            "longer than a minted id",
            "longer",
            "bogus1234567890xyz".to_string(),
        ),
        ("deleted after it was created", "deleted", deleted.clone()),
        ("a deployment of another API", "foreign", foreign.clone()),
    ];

    for (row, slug, unknown) in &rows {
        // As a patch VALUE, on the stage's own pointer and on its canary's: the bad request
        // with the value sentence, and the stage still serves the deployment it served.
        for pointer in ["/deploymentId", "/canarySettings/deploymentId"] {
            let case = format!("UpdateStage replace {pointer} ({row})");
            let answer = update_stage(&gateway, &api, "prod", replace(pointer, unknown)).await;
            assert_value_refusal(&answer, &case);
            let still = served_deployment(&gateway, &api, "prod", &case).await;
            assert_eq!(
                still, served,
                "case {case}: a refused patch leaves the stage serving {served}, got {still}"
            );
        }

        // The same value where the CREATE carries it, which is the same kind of value and
        // earns the same refusal — on the stage's member and on its canary's alike.
        for (member, request) in [
            (
                "deploymentId",
                json!({ "stageName": format!("s-{slug}"), "deploymentId": unknown }),
            ),
            (
                "canarySettings.deploymentId",
                json!({
                    "stageName": format!("c-{slug}"),
                    "deploymentId": served,
                    "canarySettings": { "deploymentId": unknown },
                }),
            ),
        ] {
            let case = format!("CreateStage {member} ({row})");
            let answer = gateway
                .call("POST", &format!("/restapis/{api}/stages"), Some(request))
                .await;
            assert_value_refusal(&answer, &case);
        }

        // The SAME id in the URI, where it addresses a resource instead of carrying a
        // value: the not-found class and the identifier sentence, from every reader that
        // resolves a deployment that way.
        for (reader, method, body) in [
            ("GetDeployment", "GET", None),
            ("DeleteDeployment", "DELETE", None),
            (
                "UpdateDeployment",
                "PATCH",
                Some(json!({ "patchOperations": [
                    { "op": "replace", "path": "/description", "value": "patched" }
                ] })),
            ),
        ] {
            let case = format!("{reader} ({row})");
            let answer = gateway
                .call(
                    method,
                    &format!("/restapis/{api}/deployments/{unknown}"),
                    body,
                )
                .await;
            assert_identifier_refusal(&answer, &case);
        }
    }

    // The positive control: the same patch with a REAL deployment of this API is applied
    // and reads back, so every refusal above is about the id and not about the stage — and
    // the id the URI readers refused above resolves for them once it names a deployment.
    let case = "UpdateStage replace /deploymentId (a real deployment)";
    let accepted =
        update_stage(&gateway, &api, "prod", replace("/deploymentId", &repointed_to)).await;
    assert_eq!(
        accepted.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    let now_served = served_deployment(&gateway, &api, "prod", case).await;
    assert_eq!(
        now_served, repointed_to,
        "case {case}: expected the stage to serve {repointed_to}, got {now_served}"
    );
    let read = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/deployments/{repointed_to}"),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case GetDeployment(a real deployment): expected 200, got {} with {:?}",
        read.status, read.body
    );
}
