// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER one `patchOperations` entry is judged in, and for the
//! sentences each verdict is worded with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the per-resource
//! create and read helpers are this crate's existing harness.
//!
//! The RULES under test:
//!
//! * the PATH is judged before the op. An operation that words a refusal of an
//!   unapplicable patch path by the path alone answers every op within `add`/`replace`/
//!   `remove` that the member the path names cannot take with that same path sentence, and
//!   the same path under the op the member DOES take is applied — so the refusal is about
//!   the (op, path) pair, not the path;
//! * an `op` outside those three — `move`, `copy` and `test`, which name nothing on these
//!   resources — is refused as an OP, in one fixed sentence that enumerates the three a
//!   path can ride in on, whichever resource and whichever path it was sent to;
//! * a gateway response's `statusCode` is not reachable by a patch at all, under any op:
//!   it is set by `PutGatewayResponse`, and the two string-to-string maps are what a patch
//!   edits;
//! * a request value outside an enum's value set is refused by the constraint layer in
//!   front of the operation, naming the member by its path from the operation's input
//!   shape — except a member bound to the QUERY STRING, which is named alone — and listing
//!   the set in the order the service declares the enum in;
//! * a stage reads every pointer that is no member of its own shape as a per-method
//!   setting's path, so it refuses one with that sentence and with the whole set of
//!   pointers it publishes.
//!
//! Each rule is asserted over a TABLE spanning what it generalises over: the operation (and
//! so the resource), the member and its kind, the op, and the form of the path.

use serde_json::json;

use crate::gateway_response::{
    default_response_templates, generated_status_code, get_gateway_response, put_gateway_response,
};
use crate::method_integration::{put_integration, resource_with_a_method};
use crate::patch_path_refusal_templates::INVALID_OP;
use crate::patch_updates::{baseline, op, patch, patch_ok, replace};
use crate::rest_api_import::put_definition;
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};
use crate::vpc_links::created_vpc_link;

/// The sentence an API's own `UpdateRestApi` words that refusal with: the same three ops,
/// bracketed. The wording belongs to the OPERATION, which is why the two sentences stand
/// side by side here — [`INVALID_OP`] is the one most of this service's resources use, and
/// is reused from the suite that pins it across them.
const REST_API_INVALID_OP: &str =
    "Invalid patch operation specified. Must be one of: [add, replace, remove]";

/// The pointers a STAGE publishes, which its own refusal of an unapplicable pointer
/// enumerates. FIXED TEXT of that sentence, reproduced as the service prints it: the two
/// `accessLogSettings` pointers and the `/*/*` stage-wide method settings are named, the
/// throttling/caching entries of each method-path family run TOGETHER inside one element
/// (with no separator in the `{resourcePath}` family and a single space in the `/*/*` one),
/// and no `/canarySettings` pointer or `/documentationVersion` appears — none of which
/// follows from which pointers the operation can apply.
///
/// Stated here rather than read from the crate, so the assertion is independent of the
/// constant under test (`STAGE_PATCH_PATHS` in src/ops/update_stage.rs, which carries a note
/// back to this copy).
const STAGE_PATCH_PATHS: &[&str] = &[
    "/deploymentId",
    "/description",
    "/cacheClusterEnabled",
    "/cacheClusterSize",
    "/clientCertificateId",
    "/accessLogSettings",
    "/accessLogSettings/destinationArn",
    "/accessLogSettings/format",
    "/{resourcePath}/{httpMethod}/metrics/enabled",
    "/{resourcePath}/{httpMethod}/logging/dataTrace",
    "/{resourcePath}/{httpMethod}/logging/loglevel",
    concat!(
        "/{resourcePath}/{httpMethod}/throttling/burstLimit",
        "/{resourcePath}/{httpMethod}/throttling/rateLimit",
        "/{resourcePath}/{httpMethod}/caching/ttlInSeconds",
    ),
    "/{resourcePath}/{httpMethod}/caching/enabled",
    "/{resourcePath}/{httpMethod}/caching/dataEncrypted",
    "/{resourcePath}/{httpMethod}/caching/requireAuthorizationForCacheControl",
    "/{resourcePath}/{httpMethod}/caching/unauthorizedCacheControlHeaderStrategy",
    "/*/*/metrics/enabled",
    "/*/*/logging/dataTrace",
    "/*/*/logging/loglevel",
    concat!(
        "/*/*/throttling/burstLimit",
        " /*/*/throttling/rateLimit",
        " /*/*/caching/ttlInSeconds",
    ),
    "/*/*/caching/enabled",
    "/*/*/caching/dataEncrypted",
    "/*/*/caching/requireAuthorizationForCacheControl",
    "/*/*/caching/unauthorizedCacheControlHeaderStrategy",
    "/variables/{variable_name}",
    "/tracingEnabled",
];

/// The sentence a STAGE refuses the patch path `path` with.
///
/// `pub(crate)` so the sibling `stage_canary_and_patch_grammar` suite asserts the one
/// sentence rather than keeping a second copy of the published list.
pub(crate) fn stage_invalid_patch_path(path: &str) -> String {
    format!(
        "Invalid method setting path: {path}. Must be one of: [{}]",
        STAGE_PATCH_PATHS.join(", ")
    )
}

/// The sentence an operation that words a path refusal by the PATH ALONE refuses `path`
/// with.
fn invalid_patch_path(path: &str) -> String {
    format!("Invalid patch path {path}")
}

/// Assert `answer` is the modelled `BadRequestException` carrying `message`.
fn assert_refused(answer: &crate::rest_api_lifecycle::Answer, message: &str, case: &str) {
    assert_failure(answer, 400, "BadRequestException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// One record of each shape the tables below patch, with the URI its PATCH is addressed by.
struct Records {
    /// The API itself (`UpdateRestApi`).
    rest_api: String,
    /// An `HTTP` integration of one method of that API (`UpdateIntegration`).
    integration: String,
    /// One gateway response of that API (`UpdateGatewayResponse`).
    gateway_response: String,
    /// A VPC link (`UpdateVpcLink`).
    vpc_link: String,
    /// An API key (`UpdateApiKey`), one of the resources that publish their patchable
    /// pointers per op.
    api_key: String,
}

/// The response type every gateway-response row patches: one that GENERATES a status code
/// of its own, so the record a refused patch leaves behind is a known one.
const RESPONSE_TYPE: &str = "INTEGRATION_TIMEOUT";

/// Stand one record of each shape up on `gateway`.
async fn records(gateway: &Gateway, name: &str) -> Records {
    let (api, root) = create_api(gateway, name).await;
    let resource = resource_with_a_method(gateway, &api, &root, "pets", "GET", json!({})).await;
    let created = put_integration(
        gateway,
        &api,
        &resource,
        "GET",
        json!({
            "type": "HTTP",
            "httpMethod": "GET",
            "uri": "https://example.com/pets",
            "timeoutInMillis": 5_000
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration({name}): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let put = put_gateway_response(
        gateway,
        &api,
        RESPONSE_TYPE,
        json!({ "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" } }),
    )
    .await;
    assert_eq!(
        put.status, 201,
        "case PutGatewayResponse({name}): expected 201, got {} with {:?}",
        put.status, put.body
    );
    let vpc_link_id = created_vpc_link(gateway, &format!("{name}-link")).await;
    let (api_key_id, _) = baseline(gateway, &format!("{name}-key")).await;
    Records {
        rest_api: format!("/restapis/{api}"),
        integration: format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
        gateway_response: format!("/restapis/{api}/gatewayresponses/{RESPONSE_TYPE}"),
        vpc_link: format!("/vpclinks/{vpc_link_id}"),
        api_key: format!("/apikeys/{api_key_id}"),
    }
}

#[tokio::test]
async fn an_op_a_pointer_cannot_take_is_refused_as_the_path_that_named_it() {
    let gateway = Gateway::new();
    let records = records(&gateway, "patch-op-before-path").await;

    // Rows differ along the operation (and so the resource), the member and its KIND — a
    // bounded integer the resource always reports, a string it always reports, an entry of
    // a map, the single-valued status code — and the op that named it: every op one of
    // these pointers cannot take, the three a patch applies as much as the three that
    // apply nothing, is refused as that pointer.
    let rows: Vec<(&str, &String, &str, &str)> = vec![
        (
            "UpdateIntegration",
            &records.integration,
            "remove",
            "/timeoutInMillis",
        ),
        (
            "UpdateIntegration",
            &records.integration,
            "remove",
            "/cacheNamespace",
        ),
        (
            "UpdateIntegration",
            &records.integration,
            "move",
            "/timeoutInMillis",
        ),
        (
            "UpdateIntegration",
            &records.integration,
            "test",
            "/uri",
        ),
        (
            "UpdateGatewayResponse",
            &records.gateway_response,
            "remove",
            "/statusCode",
        ),
        (
            "UpdateGatewayResponse",
            &records.gateway_response,
            "add",
            "/statusCode",
        ),
        (
            "UpdateGatewayResponse",
            &records.gateway_response,
            "copy",
            "/statusCode",
        ),
        (
            "UpdateGatewayResponse",
            &records.gateway_response,
            "move",
            "/responseTemplates/application~1json",
        ),
    ];

    for (operation, uri, kind, path) in rows {
        let case = format!("{operation}({kind} {path})");
        let answer = patch(
            &gateway,
            uri,
            vec![json!({ "op": kind, "path": path, "from": path, "value": "1" })],
        )
        .await;
        assert_refused(&answer, &invalid_patch_path(path), &case);
    }

    // The other side of the rule: the SAME pointers under the op they do take are applied,
    // so what the rows above refuse is the pair and not the pointer.
    for (case, uri, operations) in [
        (
            "UpdateIntegration(replace /timeoutInMillis)",
            &records.integration,
            vec![replace("/timeoutInMillis", "12000")],
        ),
        (
            "UpdateIntegration(replace /cacheNamespace)",
            &records.integration,
            vec![replace("/cacheNamespace", "pets")],
        ),
        (
            "UpdateGatewayResponse(replace /statusCode)",
            &records.gateway_response,
            vec![replace("/statusCode", "504")],
        ),
        (
            "UpdateGatewayResponse(add a template)",
            &records.gateway_response,
            vec![op(
                "add",
                "/responseTemplates/text~1plain",
                Some("timed out"),
            )],
        ),
    ] {
        patch_ok(&gateway, case, uri, operations).await;
    }

    // And each applied edit is what a later reader sees.
    let integration = gateway.call("GET", &records.integration, None).await;
    assert_eq!(
        integration.body["timeoutInMillis"],
        json!(12_000),
        "case GetIntegration: expected the replaced timeout, got {:?}",
        integration.body
    );
    assert_eq!(
        integration.body["cacheNamespace"].as_str(),
        Some("pets"),
        "case GetIntegration: expected the replaced namespace, got {:?}",
        integration.body
    );
    let response = gateway.call("GET", &records.gateway_response, None).await;
    assert_eq!(
        response.body["statusCode"].as_str(),
        Some("504"),
        "case GetGatewayResponse: expected the replaced status code, got {:?}",
        response.body
    );
    assert_eq!(
        response.body["responseTemplates"]["text/plain"].as_str(),
        Some("timed out"),
        "case GetGatewayResponse: expected the added template, got {:?}",
        response.body
    );
}

#[tokio::test]
async fn an_op_no_patch_path_can_ride_in_on_is_refused_in_the_operations_own_sentence() {
    let gateway = Gateway::new();
    let records = records(&gateway, "patch-op-enum").await;

    // The three ops the model's `Op` enum declares beyond the ones a path can ride in on,
    // each sent to a pointer the resource DOES publish for another op — so what the refusal
    // names is the op — on the operations that enumerate the ops they take. The sentence is
    // the OPERATION's: an API's own brackets the three, and the resources that word it with
    // quoted alternatives share [`INVALID_OP`].
    for kind in ["move", "copy", "test"] {
        for (operation, uri, path, expected) in [
            (
                "UpdateRestApi",
                &records.rest_api,
                "/name",
                REST_API_INVALID_OP,
            ),
            (
                "UpdateVpcLink",
                &records.vpc_link,
                "/description",
                INVALID_OP,
            ),
            (
                "UpdateApiKey",
                &records.api_key,
                "/description",
                INVALID_OP,
            ),
        ] {
            let case = format!("{operation}({kind} {path})");
            let answer = patch(
                &gateway,
                uri,
                vec![json!({ "op": kind, "path": path, "from": path, "value": "x" })],
            )
            .await;
            assert_refused(&answer, expected, &case);
        }
    }
}

#[tokio::test]
async fn a_gateway_responses_status_code_takes_a_replace_and_no_other_op() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-status-code").await;
    let stored = json!({
        "responseType": RESPONSE_TYPE,
        "statusCode": generated_status_code(RESPONSE_TYPE),
        "responseParameters": {},
        "responseTemplates": default_response_templates(),
        "defaultResponse": true
    });
    let uri = format!("/restapis/{api}/gatewayresponses/{RESPONSE_TYPE}");

    // Rows span the ops the member does NOT take and the values a client may send with
    // them: a code the modelled `^[1-5]\d\d$` pattern admits, one it does not, and none at
    // all. Each is refused as the pointer, and the record stays the one API Gateway
    // generates for the type.
    for kind in ["add", "remove", "move", "copy", "test"] {
        for value in [Some("503"), Some("099"), None] {
            let case = format!("UpdateGatewayResponse({kind} /statusCode = {value:?})");
            let answer = patch(&gateway, &uri, vec![op(kind, "/statusCode", value)]).await;
            assert_refused(&answer, &invalid_patch_path("/statusCode"), &case);
            let read = get_gateway_response(&gateway, &api, RESPONSE_TYPE).await;
            assert_eq!(
                read.body, stored,
                "case {case}: a refused patch must leave the record {stored:?}, got {:?}",
                read.body
            );
        }
    }

    // The other side: the one op that member takes, and the two map members a patch edits,
    // read back through `GetGatewayResponse`.
    let patched = patch_ok(
        &gateway,
        "UpdateGatewayResponse(the patchable pointers)",
        &uri,
        vec![
            replace("/statusCode", "502"),
            op(
                "add",
                "/responseTemplates/application~1json",
                Some("{\"message\":\"timeout\"}"),
            ),
            op(
                "add",
                "/responseParameters/gatewayresponse.header.Retry-After",
                Some("'60'"),
            ),
        ],
    )
    .await;
    let expected = json!({
        "responseType": RESPONSE_TYPE,
        "statusCode": "502",
        "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" },
        "responseParameters": { "gatewayresponse.header.Retry-After": "'60'" },
        "defaultResponse": false
    });
    assert_eq!(
        patched, expected,
        "case UpdateGatewayResponse(the patchable pointers): expected {expected:?}, got {patched:?}"
    );
    let read = get_gateway_response(&gateway, &api, RESPONSE_TYPE).await;
    assert_eq!(
        read.body, expected,
        "case GetGatewayResponse: expected {expected:?}, got {:?}",
        read.body
    );
}

/// A definition body `PutRestApi` is driven with: the mode is refused ahead of the handler,
/// so what the document says is immaterial — it only has to be the payload the required
/// member takes.
const DEFINITION: &str = "{\"openapi\": \"3.0.1\", \"info\": {\"title\": \"t\", \"version\": \"1\"}, \
                          \"paths\": {}}";

#[tokio::test]
async fn an_enum_value_set_names_a_query_member_alone_and_lists_the_declared_order() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "put-rest-api-mode").await;

    // Rows span the ways a value can miss the `PutMode` set (`smithy_query(query="shape",
    // shape="PutMode")` declares `[merge, overwrite]`): a word outside it, a declared value
    // in the wrong case, and one with a trailing character.
    for mode in ["append", "Merge", "overwrite_"] {
        let case = format!("PutRestApi(mode={mode})");
        let answer = put_definition(&gateway, &api, Some(mode), false, DEFINITION).await;
        assert_failure(&answer, 400, "ValidationException", &case);
        let expected = format!(
            "1 validation error detected: Value '{mode}' at 'mode' failed to satisfy \
             constraint: Member must satisfy enum value set: [overwrite, merge]"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected {expected:?}, got {:?}",
            answer.body["message"]
        );
    }

    // A member that travels in the BODY of another operation keeps the root the clause is
    // built from, so the query member's bare name is the member's binding and not a
    // service-wide change.
    let answer = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "rooted-member", "apiKeySource": "NOT_A_SOURCE" })),
        )
        .await;
    assert_failure(
        &answer,
        400,
        "ValidationException",
        "CreateRestApi(apiKeySource)",
    );
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("at 'createRestApiInput.apiKeySource'"),
        "case CreateRestApi(apiKeySource): expected the clause to name the member from the \
         input shape, got {message:?}"
    );

    // And each `PutMode` the set declares is accepted, so the refusals above are the value
    // set and not the member.
    for mode in ["overwrite", "merge"] {
        let case = format!("PutRestApi(mode={mode})");
        let answer = put_definition(&gateway, &api, Some(mode), false, DEFINITION).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_stage_refuses_a_pointer_it_has_no_member_for_as_a_method_setting_path() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-method-setting-paths").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // Rows span the FORMS of a pointer a stage cannot apply: a member no shape declares, a
    // member the service assigns, a one-token method path followed by a GROUPED setting
    // name, a method path whose resource half carries an unescaped separator of its own (so
    // nothing of the pointer says where that path stops, and the data plane would never
    // resolve a setting stored under it), a method path with no setting after it, and a
    // setting with no method path before it. Each is sent under every op a path can ride in
    // on.
    //
    // A two-token method path whose resource half simply LEAVES its leading separator to the
    // pointer (`/pets/GET/…`) is not one of them: that is the spelling
    // `MethodSetting$ResourcePath` states a path in, so it names a method and is applied —
    // keyed `pets/GET`, the method path that pointer spelled, see
    // `a_method_settings_key_is_the_method_path_its_pointer_names`.
    let paths = [
        "/sim2realNoSuchField",
        "/stageName",
        "/~1pets~1GET/throttling/rateLimit",
        "/pets~1GET/metricsEnabled",
        "/~1pets/GET",
        "/metrics/enabled",
    ];

    for path in paths {
        for kind in ["replace", "remove", "add"] {
            let case = format!("UpdateStage({kind} {path})");
            let answer = patch(&gateway, &uri, vec![op(kind, path, Some("1"))]).await;
            assert_failure(&answer, 400, "BadRequestException", &case);
            let message = answer.body["message"].as_str().unwrap_or_default();
            // Which sentence such a pointer earns follows from the op: an `add` puts nothing
            // anywhere on a pointer the stage has no applier for, so it is refused as the
            // operation; a `remove` names a part of the stage that goes away, so a pointer
            // that can spell a method path is read against the settings the stage carries
            // and one that cannot (a single reference token) earns the one template a
            // `remove` publishes; and a `replace` earns the path sentence with the whole
            // published enumeration. Each names the path the request sent.
            let spells_a_method_path = path.trim_start_matches('/').contains('/');
            let expected = match kind {
                "add" => format!("Invalid add operation with path: {path}"),
                "remove" if spells_a_method_path => format!(
                    "Cannot remove method setting {} because there is no method setting for \
                     this method ",
                    path.trim_start_matches('/')
                ),
                "remove" => crate::stage_remove_patch_grammar::remove_outside_the_grammar(path),
                _ => stage_invalid_patch_path(path),
            };
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The other side of the rule: the method-setting grammar the sentence publishes — two
    // tokens naming the method, then the setting — is applied, and one method spelled that
    // way is keyed one way whichever spelling of the SETTING the request used.
    let patched = patch_ok(
        &gateway,
        "UpdateStage(the published grammar)",
        &uri,
        vec![
            op("replace", "/~1pets/GET/throttling/rateLimit", Some("9")),
            op("replace", "/~1pets/GET/metricsEnabled", Some("true")),
            op("replace", "/*/*/caching/ttlInSeconds", Some("60")),
        ],
    )
    .await;
    let mut keys: Vec<String> = patched["methodSettings"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    assert_eq!(
        keys,
        vec!["*/*".to_string(), "~1pets/GET".to_string()],
        "case UpdateStage(the published grammar): expected one entry per method, got {:?}",
        patched["methodSettings"]
    );
    assert_eq!(
        patched["methodSettings"]["~1pets/GET"]["throttlingRateLimit"],
        json!(9.0),
        "case UpdateStage(the published grammar): expected the patched rate limit, got {:?}",
        patched["methodSettings"]["~1pets/GET"]
    );
    assert_eq!(
        patched["methodSettings"]["~1pets/GET"]["metricsEnabled"],
        json!(true),
        "case UpdateStage(the published grammar): both spellings of the setting must write \
         the one entry, got {:?}",
        patched["methodSettings"]["~1pets/GET"]
    );
    let read = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        read.body, patched,
        "case GetStage: expected {patched:?}, got {:?}",
        read.body
    );
}

/// The `methodSettings` key a patch writes is the method path its pointer SPELLED — written
/// exactly as the request wrote it, which is the key API Gateway reports the entry under — so
/// a client reading the stage finds the setting it just wrote under the path it patched.
#[tokio::test]
async fn a_method_settings_key_is_the_method_path_its_pointer_names() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-method-setting-keys").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // Rows span the method paths a client spells: a child resource, a nested one, the root
    // resource, the wildcard form, and the whole method path inside ONE token — whose LAST
    // escape is the separator before the HTTP method, so it spells the same key the pointer
    // that gives the method a token of its own spells.
    let rows: [(&str, &str); 5] = [
        ("/~1pets/GET/metrics/enabled", "~1pets/GET"),
        (
            "/~1pets~1{petId}/DELETE/metrics/enabled",
            "~1pets~1{petId}/DELETE",
        ),
        ("/~1/GET/metrics/enabled", "~1/GET"),
        ("/*/*/metrics/enabled", "*/*"),
        ("/~1pets~1POST/metricsEnabled", "~1pets/POST"),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (path, key) in rows {
        let case = format!("UpdateStage(replace {path})");
        let body = patch_ok(&gateway, &case, &uri, vec![replace(path, "true")]).await;
        assert_eq!(
            body["methodSettings"][key]["metricsEnabled"],
            json!(true),
            "case {case}: expected metricsEnabled under {key}, got {:?}",
            body["methodSettings"]
        );
        expected.push(key.to_string());
        expected.sort();
        let mut reported: Vec<String> = body["methodSettings"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case}: expected the keys {expected:?}, got {reported:?}"
        );
    }
}

/// A patch `value` for a boolean member of a stage is READ rather than validated, and the
/// cache capacity is stored as the request spells it — the two values API Gateway accepts
/// on a stage that no modelled constraint reaches, because a patch value is a bare string
/// on the wire.
#[tokio::test]
async fn a_stage_patch_value_is_read_rather_than_validated() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-patch-values").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // Rows span the boolean spellings a client sends and capacities inside and outside the
    // published `CacheClusterSize` catalog (`smithy_query(query="shape",
    // shape="CacheClusterSize")` declares `[0.5, 1.6, 6.1, 13.5, 28.4, 58.2, 118, 237]`).
    let rows: [(&str, &str, bool); 5] = [
        ("true", "0.5", true),
        ("TRUE", "237", true),
        ("false", "1.6", false),
        ("maybe", "3", false),
        ("", "118", false),
    ];

    for (flag, size, enabled) in rows {
        let case = format!("UpdateStage(cacheClusterEnabled={flag:?}, cacheClusterSize={size:?})");
        let body = patch_ok(
            &gateway,
            &case,
            &uri,
            vec![
                op("replace", "/cacheClusterEnabled", Some(flag)),
                op("replace", "/cacheClusterSize", Some(size)),
            ],
        )
        .await;
        assert_eq!(
            body["cacheClusterEnabled"],
            json!(enabled),
            "case {case}: expected cacheClusterEnabled {enabled}, got {:?}",
            body["cacheClusterEnabled"]
        );
        assert_eq!(
            body["cacheClusterSize"].as_str(),
            Some(size),
            "case {case}: the capacity must be stored as spelled, got {:?}",
            body["cacheClusterSize"]
        );
        let read = get_stage(&gateway, &api, "prod").await;
        assert_eq!(
            read.body["cacheClusterSize"].as_str(),
            Some(size),
            "case {case}: GetStage must echo the capacity, got {:?}",
            read.body["cacheClusterSize"]
        );
        assert_eq!(
            read.body["cacheClusterEnabled"],
            json!(enabled),
            "case {case}: GetStage must report the flag as read, got {:?}",
            read.body["cacheClusterEnabled"]
        );
    }
}
