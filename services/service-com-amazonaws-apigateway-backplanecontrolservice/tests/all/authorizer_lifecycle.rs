// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an API's authorizers: `CreateAuthorizer` and `GetAuthorizer`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` helper are this crate's existing harness, reused from
//! `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table spanning the modelled `AuthorizerType` catalog, which optional
//! members the request carries and how many provider ARNs it sets; the `authType` every
//! representation of one record reports, over that same catalog and over what a request
//! supplies for the member; the cached-result TTL
//! over its published window (each bound, values inside it and one past each bound) and
//! the default the member documents for a request that sets none; the members each `type`
//! documents as required; that a name does not identify an authorizer (two identical
//! requests make two records); and every identifier these operations resolve over the
//! records that do and do not exist.
//!
//! The `JWT` type of that catalog is the one whose whole configuration is a declaration of
//! its own (`jwtConfiguration`), so its round trip, the members it does not support and the
//! declarations it does not admit are asserted in the `authorizer_jwt_configuration` suite
//! beside this one.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::create_authorizer::{
    AUTH_TYPE_COGNITO_USER_POOLS, AUTH_TYPE_CUSTOM,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS, MAX_AUTHORIZERS_PER_API,
    MAX_AUTHORIZER_RESULT_TTL_SECONDS, MIN_AUTHORIZER_RESULT_TTL_SECONDS,
};

use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};

/// A Lambda authorizer URI of the form the `authorizerUri` member documents:
/// `arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/[FunctionARN]/invocations`.
/// Shared with the `flush_stage_authorizers_cache` suite, which needs authorizers on the
/// APIs whose stages it flushes.
pub(crate) fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:us-east-1:123456789012:function:{function}/invocations"
    )
}

/// A Cognito user pool ARN of the form the `providerARNs` member documents:
/// `arn:aws:cognito-idp:{region}:{account_id}:userpool/{user_pool_id}`. Shared with the
/// `flush_stage_authorizers_cache` suite.
pub(crate) fn user_pool_arn(pool: &str) -> String {
    format!("arn:aws:cognito-idp:us-east-1:123456789012:userpool/{pool}")
}

/// `CreateAuthorizer` on `api` with `request`.
pub(crate) async fn create_authorizer(gateway: &Gateway, api: &str, request: Value) -> Answer {
    gateway
        .call("POST", &format!("/restapis/{api}/authorizers"), Some(request))
        .await
}

/// `GetAuthorizer` for `authorizer` of `api`. Shared with the
/// `flush_stage_authorizers_cache` suite, which reads an authorizer back to see that a
/// cache flush did not touch it.
pub(crate) async fn get_authorizer(gateway: &Gateway, api: &str, authorizer: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            None,
        )
        .await
}

/// `CreateAuthorizer` with `request`, asserting the modelled 201 and answering the body.
/// Shared with the `flush_stage_authorizers_cache` suite.
pub(crate) async fn created_authorizer(
    gateway: &Gateway,
    api: &str,
    case: &str,
    request: Value,
) -> Value {
    let answer = create_authorizer(gateway, api, request).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateAuthorizer, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// A TOKEN authorizer request carrying every member that type requires, so a case that
/// varies one member varies only that one.
pub(crate) fn token_request(name: &str) -> Value {
    json!({
        "name": name,
        "type": "TOKEN",
        "identitySource": "method.request.header.Auth",
        "authorizerUri": lambda_uri("authorize"),
    })
}

#[tokio::test]
async fn a_created_authorizer_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: the authorizer `type` (the whole
    // modelled `AuthorizerType` catalog, from
    // `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`),
    // which optional members the request carries, how many provider ARNs it sets, and
    // whether it sets a TTL at all.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a TOKEN authorizer with only its required members",
            token_request("token-bare"),
            json!({
                "name": "token-bare",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": lambda_uri("authorize"),
                "authType": AUTH_TYPE_CUSTOM,
                "authorizerResultTtlInSeconds": DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS,
            }),
        ),
        (
            "a TOKEN authorizer with every optional member set",
            json!({
                "name": "token-full",
                "type": "TOKEN",
                "identitySource": "method.request.header.Authorization",
                "authorizerUri": lambda_uri("full"),
                "authorizerCredentials": "arn:aws:iam::123456789012:role/invoke",
                "identityValidationExpression": "^Bearer [-0-9a-zA-Z._]+$",
                "authType": "custom",
                "authorizerResultTtlInSeconds": 60,
            }),
            json!({
                "name": "token-full",
                "type": "TOKEN",
                "identitySource": "method.request.header.Authorization",
                "authorizerUri": lambda_uri("full"),
                "authorizerCredentials": "arn:aws:iam::123456789012:role/invoke",
                "identityValidationExpression": "^Bearer [-0-9a-zA-Z._]+$",
                "authType": "custom",
                "authorizerResultTtlInSeconds": 60,
            }),
        ),
        (
            "a REQUEST authorizer with caching disabled and no identity source",
            json!({
                "name": "request-uncached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "authorizerResultTtlInSeconds": 0,
            }),
            json!({
                "name": "request-uncached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "authType": AUTH_TYPE_CUSTOM,
                "authorizerResultTtlInSeconds": 0,
            }),
        ),
        (
            "a REQUEST authorizer caching on two identity sources",
            json!({
                "name": "request-cached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "identitySource": "method.request.header.Auth,method.request.querystring.Name",
                "authorizerResultTtlInSeconds": 120,
            }),
            json!({
                "name": "request-cached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "identitySource": "method.request.header.Auth,method.request.querystring.Name",
                "authType": AUTH_TYPE_CUSTOM,
                "authorizerResultTtlInSeconds": 120,
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer over one pool",
            json!({
                "name": "cognito-one",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_one")],
            }),
            json!({
                "name": "cognito-one",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_one")],
                "authType": AUTH_TYPE_COGNITO_USER_POOLS,
                "authorizerResultTtlInSeconds": DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS,
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer over three pools",
            json!({
                "name": "cognito-three",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [
                    user_pool_arn("us-east-1_a"),
                    user_pool_arn("us-east-1_b"),
                    user_pool_arn("us-east-1_c"),
                ],
                "authorizerResultTtlInSeconds": MAX_AUTHORIZER_RESULT_TTL_SECONDS,
            }),
            json!({
                "name": "cognito-three",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [
                    user_pool_arn("us-east-1_a"),
                    user_pool_arn("us-east-1_b"),
                    user_pool_arn("us-east-1_c"),
                ],
                "authType": AUTH_TYPE_COGNITO_USER_POOLS,
                "authorizerResultTtlInSeconds": MAX_AUTHORIZER_RESULT_TTL_SECONDS,
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-round-trip").await;

    for (case, request, expected) in &cases {
        let created = created_authorizer(&gateway, &api, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        assert_eq!(
            id.len(),
            10,
            "case {case}: an authorizer id is 10 characters, got {id:?}"
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case}: an authorizer id is lowercase alphanumeric, got {id:?}"
        );

        // The body is the stored record: what the request set, the id the service minted
        // and the TTL it defaulted, and nothing else.
        let mut without_id = created.clone();
        without_id
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {case}: expected an object, got {created:?}"))
            .remove("id");
        assert_eq!(
            &without_id, expected,
            "case {case}: expected the authorizer {expected:?}, got {without_id:?}"
        );

        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetAuthorizer, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created,
            "case {case}: GetAuthorizer must report the stored authorizer, got {:?} after {:?}",
            read.body, created
        );
    }
}

/// `GetAuthorizers` for `api`, asserting the modelled 200 and answering the `item` list.
async fn listed_authorizers(gateway: &Gateway, api: &str) -> Vec<Value> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/authorizers"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case GetAuthorizers: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["item"]
        .as_array()
        .unwrap_or_else(|| panic!("case GetAuthorizers: expected an item list, got {:?}", answer.body))
        .clone()
}

/// `UpdateAuthorizer` on `authorizer` of `api` with `patches`, asserting the modelled 200 and
/// answering the patched body.
async fn patched_authorizer(
    gateway: &Gateway,
    api: &str,
    authorizer: &str,
    case: &str,
    patches: Value,
) -> Value {
    let answer = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/authorizers/{authorizer}"),
            Some(json!({ "patchOperations": patches })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from UpdateAuthorizer, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

#[tokio::test]
async fn every_representation_of_an_authorizer_reports_its_auth_type() {
    // `Authorizer$authType` is an "optional customer-defined field, used in OpenAPI imports
    // and exports without functional impact", and what an OpenAPI document names a KIND of
    // authorization by is its `x-amazon-apigateway-authtype` — `custom` for an authorizer
    // that calls a function of the developer's own and `cognito_user_pools` for one that
    // authorizes against a user pool. So an authorizer created without the member carries
    // the label its own `type` implies, one created WITH it carries exactly what the request
    // supplied, and every representation of the record — the create's answer,
    // `GetAuthorizer`, the `GetAuthorizers` item and the patched record `UpdateAuthorizer`
    // answers — reports that one value.
    struct Case {
        /// What this row varies, for the assertion messages.
        name: &'static str,
        /// The `CreateAuthorizer` request, which differs along the authorizer `type` and
        /// along what it supplies for `authType`.
        request: Value,
        /// The `authType` every representation of the stored record must report.
        expected: &'static str,
    }

    // Rows differ along what the rule generalises over: the authorizer `type` (the whole
    // modelled `AuthorizerType` catalog, from
    // `smithy_query(query="shape", shape="com.amazonaws.apigateway#AuthorizerType")`) and
    // what the request says about the member — nothing, the implied label itself, a label of
    // the caller's own, and an empty value, which supplies no value at all.
    let cases = vec![
        Case {
            name: "a TOKEN authorizer supplying no authType",
            request: token_request("auth-type-token"),
            expected: AUTH_TYPE_CUSTOM,
        },
        Case {
            name: "a REQUEST authorizer supplying no authType",
            request: json!({
                "name": "auth-type-request",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "identitySource": "method.request.header.Auth",
            }),
            expected: AUTH_TYPE_CUSTOM,
        },
        Case {
            name: "a COGNITO_USER_POOLS authorizer supplying no authType",
            request: json!({
                "name": "auth-type-cognito",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_pool")],
            }),
            expected: AUTH_TYPE_COGNITO_USER_POOLS,
        },
        Case {
            name: "a TOKEN authorizer supplying the implied label itself",
            request: {
                let mut request = token_request("auth-type-stated");
                request["authType"] = json!(AUTH_TYPE_CUSTOM);
                request
            },
            expected: AUTH_TYPE_CUSTOM,
        },
        Case {
            name: "a TOKEN authorizer supplying a label of the caller's own",
            request: {
                let mut request = token_request("auth-type-caller");
                request["authType"] = json!("oauth2");
                request
            },
            expected: "oauth2",
        },
        Case {
            name: "a COGNITO_USER_POOLS authorizer supplying an empty authType",
            request: json!({
                "name": "auth-type-empty",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_pool")],
                "authType": "",
            }),
            expected: AUTH_TYPE_COGNITO_USER_POOLS,
        },
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-auth-type").await;

    for case in &cases {
        let name = case.name;
        let created = created_authorizer(&gateway, &api, name, case.request.clone()).await;
        let id = string_member(&created, "id", name);
        assert_eq!(
            created["authType"].as_str(),
            Some(case.expected),
            "case {name:?}: expected CreateAuthorizer to report authType {:?}, got {:?}",
            case.expected,
            created["authType"]
        );

        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body["authType"].as_str(),
            Some(case.expected),
            "case {name:?}: expected GetAuthorizer to report authType {:?}, got {:?}",
            case.expected,
            read.body
        );

        let listed = listed_authorizers(&gateway, &api).await;
        let item = listed
            .iter()
            .find(|item| item["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {name:?}: expected {id} among {listed:?}"));
        assert_eq!(
            item["authType"].as_str(),
            Some(case.expected),
            "case {name:?}: expected the GetAuthorizers item to report authType {:?}, got {item:?}",
            case.expected
        );

        // A patch of ANOTHER member leaves the member where it stood: the record keeps the
        // value it was created with.
        let patched = patched_authorizer(
            &gateway,
            &api,
            &id,
            name,
            json!([{ "op": "replace", "path": "/name", "value": format!("{id}-renamed") }]),
        )
        .await;
        assert_eq!(
            patched["authType"].as_str(),
            Some(case.expected),
            "case {name:?}: expected UpdateAuthorizer to report authType {:?}, got {patched:?}",
            case.expected
        );

        // And a patch OF the member reports what it was patched to, on the same record.
        let repatched = patched_authorizer(
            &gateway,
            &api,
            &id,
            name,
            json!([{ "op": "replace", "path": "/authType", "value": "awsSigv4" }]),
        )
        .await;
        assert_eq!(
            repatched["authType"].as_str(),
            Some("awsSigv4"),
            "case {name:?}: expected the patched authType, got {repatched:?}"
        );
        let after = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            after.body["authType"].as_str(),
            Some("awsSigv4"),
            "case {name:?}: expected GetAuthorizer to report the patched authType, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn the_cached_result_ttl_window_is_the_one_the_member_documents() {
    // From `Authorizer$authorizerResultTtlInSeconds`: "If it equals 0, authorization
    // caching is disabled", "If this field is not set, the default value is 300", "The
    // maximum value is 3600, or 1 hour." So the accepted rows are each published bound
    // and values inside the window, and the refused rows are one past each bound and
    // further outside it.
    let accepted = [
        MIN_AUTHORIZER_RESULT_TTL_SECONDS,
        MIN_AUTHORIZER_RESULT_TTL_SECONDS + 1,
        DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS,
        MAX_AUTHORIZER_RESULT_TTL_SECONDS - 1,
        MAX_AUTHORIZER_RESULT_TTL_SECONDS,
    ];
    let refused = [
        MIN_AUTHORIZER_RESULT_TTL_SECONDS - 1,
        MAX_AUTHORIZER_RESULT_TTL_SECONDS + 1,
        -300,
        86_400,
        i32::MAX,
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-ttl-window").await;

    for ttl in accepted {
        let case = format!("CreateAuthorizer(ttl={ttl})");
        // A TTL of 0 disables caching, which is exactly when a REQUEST authorizer needs
        // no identity source — so the row is driven as a TOKEN authorizer, whose identity
        // source is required for every TTL.
        let mut request = token_request(&format!("ttl-{ttl}"));
        request["authorizerResultTtlInSeconds"] = json!(ttl);
        let created = created_authorizer(&gateway, &api, &case, request).await;
        assert_eq!(
            created["authorizerResultTtlInSeconds"].as_i64(),
            Some(i64::from(ttl)),
            "case {case}: expected the TTL back, got {:?}",
            created["authorizerResultTtlInSeconds"]
        );
        let id = string_member(&created, "id", &case);
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body["authorizerResultTtlInSeconds"].as_i64(),
            Some(i64::from(ttl)),
            "case GetAuthorizer(ttl={ttl}): expected the stored TTL, got {:?}",
            read.body
        );
    }

    for ttl in refused {
        let case = format!("CreateAuthorizer(ttl={ttl})");
        let mut request = token_request(&format!("bad-ttl-{ttl}"));
        request["authorizerResultTtlInSeconds"] = json!(ttl);
        let answer = create_authorizer(&gateway, &api, request).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
    }

    // A request that sets no TTL at all gets the documented default.
    let defaulted =
        created_authorizer(&gateway, &api, "CreateAuthorizer(no ttl)", token_request("ttl-unset"))
            .await;
    assert_eq!(
        defaulted["authorizerResultTtlInSeconds"].as_i64(),
        Some(i64::from(DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS)),
        "case CreateAuthorizer(no ttl): expected the default TTL, got {:?}",
        defaulted["authorizerResultTtlInSeconds"]
    );
}

#[tokio::test]
async fn the_members_a_type_requires_are_the_ones_each_member_documents() {
    // Rows span the catalog: for each `type`, the request that omits a member that type
    // documents as required, and the request that supplies it.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a TOKEN authorizer with no authorizerUri",
            json!({
                "name": "token-no-uri",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
            }),
        ),
        (
            "a TOKEN authorizer with no identitySource",
            json!({
                "name": "token-no-source",
                "type": "TOKEN",
                "authorizerUri": lambda_uri("authorize"),
            }),
        ),
        (
            "a TOKEN authorizer whose identitySource is empty",
            json!({
                "name": "token-empty-source",
                "type": "TOKEN",
                "identitySource": "",
                "authorizerUri": lambda_uri("authorize"),
            }),
        ),
        (
            "a REQUEST authorizer with no authorizerUri",
            json!({
                "name": "request-no-uri",
                "type": "REQUEST",
                "identitySource": "method.request.header.Auth",
            }),
        ),
        (
            "a REQUEST authorizer caching with no identitySource",
            json!({
                "name": "request-cached-no-source",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "authorizerResultTtlInSeconds": 30,
            }),
        ),
        (
            "a REQUEST authorizer caching at the default TTL with no identitySource",
            json!({
                "name": "request-default-no-source",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer with no providerARNs",
            json!({
                "name": "cognito-no-pools",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer with an empty providerARNs",
            json!({
                "name": "cognito-empty-pools",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [],
            }),
        ),
        (
            "a COGNITO_USER_POOLS authorizer with no identitySource",
            json!({
                "name": "cognito-no-source",
                "type": "COGNITO_USER_POOLS",
                "providerARNs": [user_pool_arn("us-east-1_pool")],
            }),
        ),
        (
            "an authorizer whose name is empty",
            json!({
                "name": "",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": lambda_uri("authorize"),
            }),
        ),
    ];
    let accepted: Vec<(&str, Value)> = vec![
        ("the TOKEN authorizer completed", token_request("token-ok")),
        (
            "the REQUEST authorizer completed",
            json!({
                "name": "request-ok",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "identitySource": "method.request.header.Auth",
                "authorizerResultTtlInSeconds": 30,
            }),
        ),
        (
            "the REQUEST authorizer with caching off needs no identity source",
            json!({
                "name": "request-ok-uncached",
                "type": "REQUEST",
                "authorizerUri": lambda_uri("request"),
                "authorizerResultTtlInSeconds": 0,
            }),
        ),
        (
            "the COGNITO_USER_POOLS authorizer completed",
            json!({
                "name": "cognito-ok",
                "type": "COGNITO_USER_POOLS",
                "identitySource": "method.request.header.Authorization",
                "providerARNs": [user_pool_arn("us-east-1_pool")],
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-required-members").await;

    for (case, request) in &refused {
        let answer = create_authorizer(&gateway, &api, request.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);
    }
    for (case, request) in &accepted {
        let created = created_authorizer(&gateway, &api, case, request.clone()).await;
        let id = string_member(&created, "id", case);
        let read = get_authorizer(&gateway, &api, &id).await;
        assert_eq!(
            read.body, created,
            "case {case}: GetAuthorizer must report the stored authorizer, got {:?} after {:?}",
            read.body, created
        );
    }

    // A `type` outside the modelled enum is refused by the boundary validator, which
    // names the same `BadRequestException` this operation's own refusals do: API Gateway
    // classifies an unknown authorizer type as a bad request.
    let off_model = create_authorizer(
        &gateway,
        &api,
        json!({
            "name": "off-model",
            "type": "MUTUAL_TLS",
            "identitySource": "method.request.header.Auth",
            "authorizerUri": lambda_uri("authorize"),
        }),
    )
    .await;
    assert_failure(
        &off_model,
        400,
        "BadRequestException",
        "a type outside AuthorizerType",
    );
}

#[tokio::test]
async fn a_name_does_not_identify_an_authorizer_and_a_read_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-no-dedup").await;

    // Two requests carrying the SAME configuration make two records: an authorizer is
    // identified by the id the service mints, not by its name or its configuration.
    let first = created_authorizer(&gateway, &api, "the first create", token_request("same")).await;
    let second =
        created_authorizer(&gateway, &api, "the second create", token_request("same")).await;
    let first_id = string_member(&first, "id", "the first create");
    let second_id = string_member(&second, "id", "the second create");
    assert_ne!(
        first_id, second_id,
        "case two identical creates: expected distinct ids, both were {first_id:?}"
    );
    for (case, id, created) in [
        ("the first create", &first_id, &first),
        ("the second create", &second_id, &second),
    ] {
        let read = get_authorizer(&gateway, &api, id).await;
        assert_eq!(
            &read.body, created,
            "case {case}: both authorizers must stand, got {:?} for {id}",
            read.body
        );
    }

    // Reading an authorizer does not change what a later caller sees.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_authorizer(&gateway, &api, &first_id).await;
        assert_eq!(
            read.status, 200,
            "case GetAuthorizer(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetAuthorizer repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_authorizer_operation() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-notfound").await;
    let missing = "zzzzzzzzzz";
    let created =
        created_authorizer(&gateway, &api, "the authorizer to read", token_request("present"))
            .await;
    let id = string_member(&created, "id", "the authorizer to read");

    // Rows span each identifier these operations resolve, one unknown at a time.
    assert_failure(
        &create_authorizer(&gateway, missing, token_request("orphan")).await,
        404,
        "NotFoundException",
        "CreateAuthorizer on an unknown API",
    );
    assert_failure(
        &get_authorizer(&gateway, missing, &id).await,
        404,
        "NotFoundException",
        "GetAuthorizer on an unknown API",
    );
    assert_failure(
        &get_authorizer(&gateway, &api, missing).await,
        404,
        "NotFoundException",
        "GetAuthorizer for an unknown authorizer",
    );

    // An authorizer belongs to its own API: another API does not report it.
    let (other, _other_root) = create_api(&gateway, "authorizer-notfound-other").await;
    assert_failure(
        &get_authorizer(&gateway, &other, &id).await,
        404,
        "NotFoundException",
        "GetAuthorizer through another API",
    );

    // The authorizers of a deleted API go with it.
    let deleted = gateway.call("DELETE", &format!("/restapis/{api}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_authorizer(&gateway, &api, &id).await,
        404,
        "NotFoundException",
        "GetAuthorizer after DeleteRestApi",
    );
}

#[tokio::test]
async fn an_api_holds_at_most_the_quota_of_authorizers() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-quota").await;

    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_AUTHORIZERS_PER_API {
        let case = format!("CreateAuthorizer(#{index})");
        let created =
            created_authorizer(&gateway, &api, &case, token_request(&format!("auth-{index}")))
                .await;
        ids.push(string_member(&created, "id", &case));
    }
    let over = create_authorizer(&gateway, &api, token_request("one-too-many")).await;
    assert_failure(
        &over,
        429,
        "LimitExceededException",
        "CreateAuthorizer past the quota",
    );

    // The refusal stored nothing and took nothing away: every authorizer created up to
    // the quota is still readable.
    for (index, id) in ids.iter().enumerate() {
        let read = get_authorizer(&gateway, &api, id).await;
        assert_eq!(
            read.status, 200,
            "case GetAuthorizer(#{index}): expected 200, got {} with {:?}",
            read.status, read.body
        );
    }

    // The quota is per API: another API takes its own authorizers.
    let (other, _other_root) = create_api(&gateway, "authorizer-quota-other").await;
    let elsewhere =
        created_authorizer(&gateway, &other, "CreateAuthorizer on another API", token_request("elsewhere"))
            .await;
    assert!(
        elsewhere["id"].is_string(),
        "case CreateAuthorizer on another API: expected an id, got {elsewhere:?}"
    );
}
