// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a `Stage`'s CANARY RELEASE and for the grammar its
//! `patchOperations` are read by: which op reaches which pointer, and how a value is read.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The emulator and the `CreateRestApi` / `CreateDeployment` / `GetStage` helpers are
//! this crate's existing harness.
//!
//! The rules under test, each over a TABLE of inputs rather than one call:
//!
//! * a deployment created WITH canary settings for a stage the API already has is the
//!   CANARY's deployment: the stage keeps serving the deployment it points at, and
//!   promoting the canary is what makes its deployment, and its variable overrides, the
//!   stage's;
//! * a canary configured on a stage always serves a deployment, so one carrying none of
//!   its own serves the deployment the stage serves;
//! * the stage's cache capacity is stored as the request spells it;
//! * an access-log DESTINATION is named by an ARN, and the access-log FORMAT is not a part
//!   of a stage a `remove` takes away on its own, which is read as a method setting's path;
//! * a `remove` of a method setting takes it away and one the stage carries no settings for
//!   is refused;
//! * an `add` names one entry of a collection a stage keys by name and nothing else;
//! * a boolean member of a stage is true only when the patch value spells `true`.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// The capacities `CacheClusterSize` declares (`smithy_query(query="shape",
/// shape="CacheClusterSize")`), in the order the model lists them.
const CACHE_CLUSTER_SIZES: [&str; 8] = [
    "0.5", "1.6", "118", "13.5", "237", "28.4", "58.2", "6.1",
];

/// `UpdateStage` for `stage_name` of `api` with `operations`.
async fn patch_stage(gateway: &Gateway, api: &str, stage_name: &str, operations: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{stage_name}"),
            Some(json!({ "patchOperations": operations })),
        )
        .await
}

/// `UpdateStage` asserted to be ANSWERED, so a row that only sets the stage up cannot
/// pass by being refused.
async fn patch_ok(
    gateway: &Gateway,
    api: &str,
    stage_name: &str,
    case: &str,
    operations: Value,
) -> Value {
    let answer = patch_stage(gateway, api, stage_name, operations).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from UpdateStage, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// One `replace` of `path` with `value`.
fn replace(path: &str, value: &str) -> Value {
    json!([{ "op": "replace", "path": path, "value": value }])
}

/// One `remove` of `path`.
fn remove(path: &str) -> Value {
    json!([{ "op": "remove", "path": path }])
}

/// `CreateStage` for `stage_name` of `api` pointing at `deployment`, plus `extra`.
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

/// The member a slash-separated pointer names in a reported stage, or `Value::Null`.
///
/// A segment spells a `/` inside one key as `~1`, the way a JSON Pointer does, so a
/// `methodSettings` key (`*/*`) can be addressed by a pointer that is split on `/`.
fn member<'a>(body: &'a Value, pointer: &str) -> &'a Value {
    let mut current = body;
    for segment in pointer.split('/') {
        current = &current[segment.replace("~1", "/")];
    }
    current
}

/// Assert that `body` reports each pointer of `expected` with its value.
fn assert_members(body: &Value, expected: &[(&str, Value)], case: &str, operation: &str) {
    for (pointer, value) in expected {
        assert_eq!(
            member(body, pointer),
            value,
            "case {case}: {operation} must report {pointer} as {value:?}, got {:?}",
            member(body, pointer)
        );
    }
}

#[tokio::test]
async fn a_deployment_created_with_canary_settings_is_the_canarys_not_the_stages() {
    let gateway = Gateway::new();

    // Rows differ along what the rule generalises over: the canary's share of the
    // traffic, whether it overrides stage variables, and whether it uses the stage cache.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a canary at a whole percentage",
            json!({ "percentTraffic": 10.0, "useStageCache": false }),
        ),
        (
            "a canary at a fractional percentage using the stage cache",
            json!({ "percentTraffic": 12.5, "useStageCache": true }),
        ),
        (
            "a canary overriding one stage variable",
            json!({
                "percentTraffic": 25.0,
                "useStageCache": false,
                "stageVariableOverrides": { "table": "pets-canary" }
            }),
        ),
        (
            "a canary that takes none of the traffic yet",
            json!({ "percentTraffic": 0.0, "useStageCache": false }),
        ),
    ];

    for (case, canary) in rows {
        let (api, _root) = create_api(&gateway, &format!("canary-{}", canary["percentTraffic"]))
            .await;
        let base = create_deployment(&gateway, &api, case, json!({})).await;
        create_stage(
            &gateway,
            &api,
            &base,
            "prod",
            json!({ "variables": { "table": "pets" } }),
        )
        .await;

        // The canary release: a deployment created FOR a stage the API already has, with
        // canary settings.
        let canary_deployment = create_deployment(
            &gateway,
            &api,
            case,
            json!({ "stageName": "prod", "canarySettings": canary }),
        )
        .await;
        assert_ne!(
            canary_deployment, base,
            "case {case}: CreateDeployment must mint a second deployment, got {canary_deployment}"
        );

        let read = get_stage(&gateway, &api, "prod").await;
        let mut expected = vec![
            ("deploymentId", json!(base)),
            ("canarySettings/deploymentId", json!(canary_deployment)),
            ("canarySettings/percentTraffic", canary["percentTraffic"].clone()),
            ("canarySettings/useStageCache", canary["useStageCache"].clone()),
            // The stage's own variables are the ones it was created with: a canary
            // overrides them for its own share of the traffic only.
            ("variables", json!({ "table": "pets" })),
        ];
        if let Some(overrides) = canary.get("stageVariableOverrides") {
            expected.push((
                "canarySettings/stageVariableOverrides",
                overrides.clone(),
            ));
        }
        assert_members(&read.body, &expected, case, "GetStage");
    }
}

#[tokio::test]
async fn a_deployment_with_canary_settings_for_a_new_stage_is_served_by_both() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "canary-new-stage").await;
    // A stage this call CREATES serves the deployment it was created from, and its canary
    // serves the same one: there is no earlier deployment for either to point at.
    let deployment = create_deployment(
        &gateway,
        &api,
        "a canary on a stage the call creates",
        json!({
            "stageName": "fresh",
            "canarySettings": { "percentTraffic": 30.0, "useStageCache": false }
        }),
    )
    .await;
    let read = get_stage(&gateway, &api, "fresh").await;
    assert_members(
        &read.body,
        &[
            ("deploymentId", json!(deployment)),
            ("canarySettings/deploymentId", json!(deployment)),
            ("canarySettings/percentTraffic", json!(30.0)),
        ],
        "a canary on a stage the call creates",
        "GetStage",
    );
}

#[tokio::test]
async fn promoting_a_canary_makes_its_deployment_and_its_overrides_the_stages() {
    let gateway = Gateway::new();

    // Rows differ along the overrides the canary carries: none, one that overrides a
    // variable the stage has, one that introduces a name the stage does not have, and
    // both at once.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a canary that overrides nothing",
            json!({}),
            json!({ "table": "pets", "color": "blue" }),
        ),
        (
            "a canary that overrides one of the stage's variables",
            json!({ "color": "green" }),
            json!({ "table": "pets", "color": "green" }),
        ),
        (
            "a canary that introduces a variable of its own",
            json!({ "shard": "canary" }),
            json!({ "table": "pets", "color": "blue", "shard": "canary" }),
        ),
        (
            "a canary that both overrides and introduces one",
            json!({ "color": "green", "shard": "canary" }),
            json!({ "table": "pets", "color": "green", "shard": "canary" }),
        ),
    ];

    for (index, (case, overrides, promoted)) in rows.into_iter().enumerate() {
        let (api, _root) = create_api(&gateway, &format!("canary-promote-{index}")).await;
        let base = create_deployment(&gateway, &api, case, json!({})).await;
        create_stage(
            &gateway,
            &api,
            &base,
            "prod",
            json!({ "variables": { "table": "pets", "color": "blue" } }),
        )
        .await;
        let mut canary = json!({ "percentTraffic": 20.0, "useStageCache": false });
        if let (Some(target), Some(source)) = (canary.as_object_mut(), overrides.as_object()) {
            if !source.is_empty() {
                target.insert("stageVariableOverrides".to_string(), overrides.clone());
            }
        }
        let canary_deployment = create_deployment(
            &gateway,
            &api,
            case,
            json!({ "stageName": "prod", "canarySettings": canary }),
        )
        .await;

        // The promotion `PatchOperation$from` documents, plus the canary's overrides,
        // which promoting the canary makes the stage's own variables.
        let promoted_stage = patch_ok(
            &gateway,
            &api,
            "prod",
            case,
            json!([
                { "op": "copy", "from": "/canarySettings/deploymentId", "path": "/deploymentId" },
                { "op": "copy", "from": "/canarySettings/stageVariableOverrides",
                  "path": "/variables" }
            ]),
        )
        .await;
        let expected = vec![
            ("deploymentId", json!(canary_deployment)),
            ("variables", promoted.clone()),
        ];
        assert_members(&promoted_stage, &expected, case, "UpdateStage");
        let read = get_stage(&gateway, &api, "prod").await;
        assert_members(&read.body, &expected, case, "GetStage");
    }
}

#[tokio::test]
async fn a_canary_carrying_no_deployment_of_its_own_serves_the_stages() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "canary-without-a-deployment").await;
    let first = create_deployment(&gateway, &api, "a canary with no deployment", json!({})).await;
    let second = create_deployment(&gateway, &api, "a canary with no deployment", json!({})).await;
    create_stage(&gateway, &api, &first, "prod", json!({})).await;

    // Rows differ along HOW the canary came to carry no deployment of its own: configured
    // by a patch that never named one, and one whose own deployment was taken away.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a canary configured without a deployment",
            replace("/canarySettings/percentTraffic", "5"),
        ),
        (
            "a canary whose own deployment was removed",
            json!([
                { "op": "replace", "path": "/canarySettings/deploymentId", "value": second },
                { "op": "remove", "path": "/canarySettings/deploymentId" }
            ]),
        ),
    ];

    for (case, operations) in rows {
        let updated = patch_ok(&gateway, &api, "prod", case, operations).await;
        // The stage serves `first`, so its canary does too.
        assert_members(
            &updated,
            &[
                ("deploymentId", json!(first)),
                ("canarySettings/deploymentId", json!(first)),
            ],
            case,
            "UpdateStage",
        );
        // Derived on every report rather than stored: re-pointing the stage moves the
        // canary with it.
        let repointed = patch_ok(
            &gateway,
            &api,
            "prod",
            case,
            replace("/deploymentId", &second),
        )
        .await;
        assert_members(
            &repointed,
            &[
                ("deploymentId", json!(second)),
                ("canarySettings/deploymentId", json!(second)),
            ],
            case,
            "UpdateStage",
        );
        let read = get_stage(&gateway, &api, "prod").await;
        assert_members(
            &read.body,
            &[("canarySettings/deploymentId", json!(second))],
            case,
            "GetStage",
        );
        // Promoting such a canary is therefore the no-op the rule implies: the stage
        // already serves the deployment the canary serves.
        let promoted = patch_ok(
            &gateway,
            &api,
            "prod",
            case,
            json!([
                { "op": "copy", "from": "/canarySettings/deploymentId", "path": "/deploymentId" }
            ]),
        )
        .await;
        assert_members(
            &promoted,
            &[
                ("deploymentId", json!(second)),
                ("canarySettings/deploymentId", json!(second)),
            ],
            case,
            "UpdateStage",
        );
        // Back to the stage's first deployment for the next row.
        patch_ok(
            &gateway,
            &api,
            "prod",
            case,
            replace("/deploymentId", &first),
        )
        .await;
        patch_ok(&gateway, &api, "prod", case, remove("/canarySettings")).await;
    }
}

#[tokio::test]
async fn the_cache_capacity_is_stored_as_the_request_spells_it() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-cache-capacity").await;
    let deployment = create_deployment(&gateway, &api, "cache capacity", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    // Rows span every capacity the model declares — including ones no recording cites —
    // and capacities outside that catalogue, which a patch value carries as a bare string.
    let outside = ["2.0", "3", "0.25", "1024"];
    for size in CACHE_CLUSTER_SIZES.iter().chain(outside.iter()) {
        let case = format!("replace /cacheClusterSize with {size:?}");
        let updated = patch_ok(
            &gateway,
            &api,
            "prod",
            &case,
            replace("/cacheClusterSize", size),
        )
        .await;
        assert_members(
            &updated,
            &[("cacheClusterSize", json!(size))],
            &case,
            "UpdateStage",
        );
        let read = get_stage(&gateway, &api, "prod").await;
        assert_members(
            &read.body,
            &[("cacheClusterSize", json!(size))],
            &case,
            "GetStage",
        );
    }
}

/// The sentence API Gateway refuses a value that is no ARN with: its unreadable-ARN text,
/// the rule the value broke, and the value the request sent.
fn invalid_arn(value: &str) -> String {
    format!("Invalid ARN specified in the request. ARNs must start with 'arn:': {value}")
}

/// The sentence API Gateway refuses a `remove` of a method setting the stage carries none
/// of with; the variable part is the path the request sent, without its leading slash.
fn cannot_remove_method_setting(setting: &str) -> String {
    format!(
        "Cannot remove method setting {setting} because there is no method setting for this \
         method "
    )
}

/// Assert `answer` is a 400 `BadRequestException` carrying `expected`.
fn assert_refusal(answer: &Answer, expected: &str, case: &str) {
    assert_failure(answer, 400, "BadRequestException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(expected),
        "case {case}: expected {expected:?}, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn an_access_log_destination_is_named_by_an_arn() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "access-log-destination-arn").await;
    let deployment = create_deployment(&gateway, &api, "access log destination", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    // Rows differ along the FORM of the value: a bare word, the ARN's fields without the
    // prefix, the prefix mis-cased, and a URL. A value of no characters is not one of them:
    // a `replace` carrying none names no destination to set, so it is the documented unset
    // of the member rather than a value read as an ARN (see `valueless_patch_replace`).
    for value in [
        "not-an-arn",
        "aws:logs:us-east-1:000000000000:log-group:api",
        "ARN:aws:logs:us-east-1:000000000000:log-group:api",
        "https://logs.us-east-1.amazonaws.com/api",
    ] {
        let case = format!("replace /accessLogSettings/destinationArn with {value:?}");
        let answer = patch_stage(
            &gateway,
            &api,
            "prod",
            replace("/accessLogSettings/destinationArn", value),
        )
        .await;
        assert_refusal(&answer, &invalid_arn(value), &case);
        let read = get_stage(&gateway, &api, "prod").await;
        assert_eq!(
            read.body["accessLogSettings"],
            Value::Null,
            "case {case}: a refused destination must leave the stage with no access log \
             settings, got {:?}",
            read.body["accessLogSettings"]
        );
    }

    // The other side of the rule: each destination the member documents — a CloudWatch
    // Logs log group and a Kinesis Data Firehose delivery stream — is accepted and read
    // back.
    for destination in [
        "arn:aws:logs:us-east-1:000000000000:log-group:api-access",
        "arn:aws:firehose:us-east-1:000000000000:deliverystream/amazon-apigateway-access",
    ] {
        let case = format!("replace /accessLogSettings/destinationArn with {destination:?}");
        let updated = patch_ok(
            &gateway,
            &api,
            "prod",
            &case,
            replace("/accessLogSettings/destinationArn", destination),
        )
        .await;
        assert_members(
            &updated,
            &[("accessLogSettings/destinationArn", json!(destination))],
            &case,
            "UpdateStage",
        );
    }
}

#[tokio::test]
async fn the_access_log_format_is_not_a_part_of_a_stage_a_remove_takes_away() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "access-log-asymmetric-remove").await;
    let deployment = create_deployment(&gateway, &api, "access log remove", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    let destination = "arn:aws:logs:us-east-1:000000000000:log-group:api-access";
    let format = "$context.requestId $context.status";
    let configured = json!([
        { "op": "replace", "path": "/accessLogSettings/destinationArn", "value": destination },
        { "op": "replace", "path": "/accessLogSettings/format", "value": format }
    ]);
    patch_ok(
        &gateway,
        &api,
        "prod",
        "access log settings configured",
        configured.clone(),
    )
    .await;

    // The format is refused — and the settings are left exactly as they were.
    let case = "remove /accessLogSettings/format";
    let answer = patch_stage(&gateway, &api, "prod", remove("/accessLogSettings/format")).await;
    assert_refusal(
        &answer,
        &cannot_remove_method_setting("accessLogSettings/format"),
        case,
    );
    let read = get_stage(&gateway, &api, "prod").await;
    assert_members(
        &read.body,
        &[
            ("accessLogSettings/destinationArn", json!(destination)),
            ("accessLogSettings/format", json!(format)),
        ],
        case,
        "GetStage",
    );

    // The destination, which turning access logging off takes away, IS removable, and the
    // format the stage still carries is reported without it.
    let case = "remove /accessLogSettings/destinationArn";
    let updated = patch_ok(
        &gateway,
        &api,
        "prod",
        case,
        remove("/accessLogSettings/destinationArn"),
    )
    .await;
    assert_members(
        &updated,
        &[
            ("accessLogSettings/destinationArn", Value::Null),
            ("accessLogSettings/format", json!(format)),
        ],
        case,
        "UpdateStage",
    );

    // And the structure itself goes away whole, which is the one way the format goes.
    let case = "remove /accessLogSettings";
    let updated = patch_ok(&gateway, &api, "prod", case, remove("/accessLogSettings")).await;
    assert_members(
        &updated,
        &[("accessLogSettings", Value::Null)],
        case,
        "UpdateStage",
    );
}

#[tokio::test]
async fn a_remove_of_a_method_setting_takes_away_only_settings_the_stage_carries() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "method-setting-removes").await;
    let deployment = create_deployment(&gateway, &api, "method setting removes", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    // A method the stage carries no settings for has nothing to take away, whichever
    // pointer the `remove` spells: a setting of a method, a method, and a pointer that is
    // no method at all.
    for path in [
        "/*/*/caching/ttlInSeconds",
        "/~1pets/GET/metrics/enabled",
        "/~1pets/GET",
    ] {
        let case = format!("remove {path} with no settings on the stage");
        let answer = patch_stage(&gateway, &api, "prod", remove(path)).await;
        assert_refusal(
            &answer,
            &cannot_remove_method_setting(path.trim_start_matches('/')),
            &case,
        );
    }

    // A setting the stage DOES carry goes back to the value a stage carries for a setting
    // nobody configured: the documented default TTL, and metrics off.
    let case = "remove one setting of a method the stage carries settings for";
    patch_ok(
        &gateway,
        &api,
        "prod",
        case,
        json!([
            { "op": "replace", "path": "/*/*/caching/ttlInSeconds", "value": "60" },
            { "op": "replace", "path": "/*/*/metrics/enabled", "value": "true" }
        ]),
    )
    .await;
    let updated = patch_ok(
        &gateway,
        &api,
        "prod",
        case,
        remove("/*/*/caching/ttlInSeconds"),
    )
    .await;
    assert_members(
        &updated,
        &[
            ("methodSettings/*~1*/cacheTtlInSeconds", json!(300)),
            ("methodSettings/*~1*/metricsEnabled", json!(true)),
        ],
        case,
        "UpdateStage",
    );

    // The method itself: its whole entry leaves the map.
    let case = "remove the method a stage carries settings for";
    let updated = patch_ok(&gateway, &api, "prod", case, remove("/*/*")).await;
    assert_eq!(
        updated["methodSettings"],
        json!({}),
        "case {case}: the method's settings must leave the map, got {:?}",
        updated["methodSettings"]
    );
}

#[tokio::test]
async fn an_add_names_one_entry_of_a_collection_a_stage_keys_by_name() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-add-grammar").await;
    let deployment = create_deployment(&gateway, &api, "add grammar", json!({})).await;
    let other = create_deployment(&gateway, &api, "add grammar", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    // Rows differ along the member the pointer names and its type: the deployment a stage
    // serves, strings, a boolean, the capacity, and a sub-member of each nested structure.
    let refused: Vec<(&str, Value)> = vec![
        ("/deploymentId", json!(other)),
        ("/description", json!("added")),
        ("/documentationVersion", json!("v4")),
        ("/tracingEnabled", json!("true")),
        ("/cacheClusterEnabled", json!("false")),
        ("/cacheClusterSize", json!("1.6")),
        ("/canarySettings/percentTraffic", json!("10")),
        ("/canarySettings/useStageCache", json!("true")),
        (
            "/accessLogSettings/destinationArn",
            json!("arn:aws:logs:us-east-1:000000000000:log-group:api"),
        ),
    ];
    for (path, value) in refused {
        let case = format!("add {path}");
        let answer = patch_stage(
            &gateway,
            &api,
            "prod",
            json!([{ "op": "add", "path": path, "value": value }]),
        )
        .await;
        assert_refusal(
            &answer,
            &format!("Invalid add operation with path: {path}"),
            &case,
        );
    }

    // The other side of the rule: an entry of a collection the stage keys by name takes an
    // `add`, which is what creates it.
    let added = patch_ok(
        &gateway,
        &api,
        "prod",
        "add the entries a stage keys by name",
        json!([
            { "op": "add", "path": "/variables/table", "value": "pets" },
            { "op": "add", "path": "/canarySettings/stageVariableOverrides/table",
              "value": "pets-canary" },
            { "op": "add", "path": "/*/*/metrics/enabled", "value": "true" }
        ]),
    )
    .await;
    assert_members(
        &added,
        &[
            ("variables", json!({ "table": "pets" })),
            (
                "canarySettings/stageVariableOverrides",
                json!({ "table": "pets-canary" }),
            ),
            ("methodSettings/*~1*/metricsEnabled", json!(true)),
        ],
        "add the entries a stage keys by name",
        "UpdateStage",
    );

    // The POINTER is judged first where the stage HAS a member for it: a pointer it has none
    // for at all is refused as the `add` itself, because an `add` puts nothing anywhere on
    // such a pointer whether or not the stage could address it, while one reaching into a
    // structure the stage does have keeps that structure's own sentence.
    for (path, expected) in [
        (
            "/sim2realNoSuchField",
            "Invalid add operation with path: /sim2realNoSuchField".to_string(),
        ),
        (
            "/accessLogSettings/sim2realNoSuchField",
            "Invalid accessLogSettings path. Must be one of : \
             [/accessLogSettings/destinationArn, /accessLogSettings/format]"
                .to_string(),
        ),
    ] {
        let case = format!("add {path}");
        let answer = patch_stage(
            &gateway,
            &api,
            "prod",
            json!([{ "op": "add", "path": path, "value": "x" }]),
        )
        .await;
        assert_refusal(&answer, &expected, &case);
    }
}

#[tokio::test]
async fn a_boolean_member_of_a_stage_is_true_only_when_the_value_spells_true() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-boolean-values").await;
    let deployment = create_deployment(&gateway, &api, "boolean values", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    // Rows differ along the VALUE's form — the spelling an SDK sends, case, surrounding
    // space, a word that is not a boolean, a number, and an empty value — and are applied
    // to a boolean member of the stage, of its canary and of a method's settings.
    let spellings = [
        ("true", true),
        ("TRUE", true),
        (" true ", true),
        ("false", false),
        ("False", false),
        ("maybe", false),
        ("1", false),
        ("", false),
    ];
    let members = [
        ("/tracingEnabled", "tracingEnabled"),
        ("/canarySettings/useStageCache", "canarySettings/useStageCache"),
        ("/*/*/metrics/enabled", "methodSettings/*~1*/metricsEnabled"),
    ];
    for (value, expected) in spellings {
        for (path, pointer) in members {
            let case = format!("replace {path} with {value:?}");
            let updated = patch_ok(&gateway, &api, "prod", &case, replace(path, value)).await;
            assert_members(&updated, &[(pointer, json!(expected))], &case, "UpdateStage");
            let read = get_stage(&gateway, &api, "prod").await;
            assert_members(&read.body, &[(pointer, json!(expected))], &case, "GetStage");
        }
    }
}

#[tokio::test]
async fn a_stage_presents_only_a_client_certificate_the_account_has() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-client-certificate").await;
    let deployment = create_deployment(&gateway, &api, "client certificate", json!({})).await;
    create_stage(&gateway, &api, &deployment, "prod", json!({})).await;

    // Rows differ along the form of the identifier: one shaped like a minted id and one
    // that is not. The account has no client certificates — the operation that mints one is
    // `GenerateClientCertificate` — so neither of them names one.
    for value in ["abcdef1234", "not-a-certificate"] {
        let case = format!("replace /clientCertificateId with {value:?}");
        let answer = patch_stage(&gateway, &api, "prod", replace("/clientCertificateId", value))
            .await;
        assert_refusal(&answer, "Invalid client certificate id specified", &case);
        let read = get_stage(&gateway, &api, "prod").await;
        assert_eq!(
            read.body["clientCertificateId"],
            Value::Null,
            "case {case}: a refused certificate must leave the stage with none, got {:?}",
            read.body["clientCertificateId"]
        );
    }

    // An EMPTY value names no certificate and asks for none: it releases the association
    // rather than being looked up, as the recorded client-certificate lifecycle shows AWS
    // doing when a caller takes a stage's certificate back off it with a `replace` — so it
    // leaves the stage with none and is accepted.
    let case = "replace /clientCertificateId with \"\"";
    let updated = patch_ok(
        &gateway,
        &api,
        "prod",
        case,
        replace("/clientCertificateId", ""),
    )
    .await;
    assert_members(
        &updated,
        &[("clientCertificateId", Value::Null)],
        case,
        "UpdateStage",
    );

    // A `remove` of the member names no part of a stage that can go away — a stage reports
    // the member it was set, and the valueless `replace` above is how a caller takes it back
    // off — so it is refused with the one pointer template a `remove` publishes, and the
    // stage keeps what it had. The sibling `stage_remove_patch_grammar` suite holds the
    // table of that rule.
    let case = "remove /clientCertificateId";
    let answer = patch_stage(&gateway, &api, "prod", remove("/clientCertificateId")).await;
    assert_refusal(
        &answer,
        &crate::stage_remove_patch_grammar::remove_outside_the_grammar("/clientCertificateId"),
        case,
    );
}
