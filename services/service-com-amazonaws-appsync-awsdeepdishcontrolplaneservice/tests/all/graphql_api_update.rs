// SPDX-License-Identifier: CC0-1.0

//! `UpdateGraphqlApi`, end to end over HTTP.
//!
//! The rule: an update REPLACES the stored configuration of an existing GraphQL API
//! with the configuration the request carries — every member the request can set is
//! taken from it, a settable optional member the request omits is left absent, and
//! `introspectionConfig` takes its documented default when the request names none —
//! while the API's identity and the members fixed when it was created (`apiId`,
//! `arn`, `dns` / `uris`, `owner`, `apiType`, `visibility`, `tags`) survive the
//! update untouched, and a later read answers the updated record. A request whose
//! configuration is not well formed is refused on the same grounds the create call
//! refuses it on, and leaves the stored API exactly as it was.
//!
//! The tables span the classes the rule generalises over: every
//! `authenticationType` the model declares, as the type updated FROM and as the type
//! updated TO; each settable optional member both carried and omitted;
//! `introspectionConfig` at each of its values and absent; and each ground on which a
//! configuration is refused, at the API level and on an additional provider.
//!
//! The harness (`router`, `call`, `create_api`, `auth_config`, `merge_members`,
//! `AUTH_TYPES`) is the lifecycle suite's, and `complete_provider` is the create-rules
//! suite's.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::graphql_api_create_rules::complete_provider;
use crate::graphql_api_lifecycle::{
    auth_config, call, create_api, merge_members, router, AUTH_TYPES, REGION,
};

/// `ownerContact`'s documented maximum length: "This field accepts any string input
/// with a length of 0 - 256 characters" (`UpdateGraphqlApiRequest$ownerContact`).
const OWNER_CONTACT_MAX_LEN: usize = 256;

/// The number of characters an AppSync API id has, so one character more names no API
/// at all. Source: the ids `CreateGraphqlApi` mints, which `GetGraphqlApi` serves.
const API_ID_LEN: usize = 26;

/// Send one `UpdateGraphqlApi` request and return `(status, parsed body)`.
async fn update_api(
    router: &MultiServiceRouter,
    api_id: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &format!("/v1/apis/{api_id}"), Some(body)).await
}

/// The member an authentication type is configured through, or `None` for a type
/// that is configured by nothing.
///
/// Source: `UpdateGraphqlApiRequest`'s `openIDConnectConfig` / `userPoolConfig` /
/// `lambdaAuthorizerConfig` members; `API_KEY` and `AWS_IAM` are configured by no
/// member of the request.
fn auth_config_member(authentication_type: &str) -> Option<&'static str> {
    match authentication_type {
        "OPENID_CONNECT" => Some("openIDConnectConfig"),
        "AMAZON_COGNITO_USER_POOLS" => Some("userPoolConfig"),
        "AWS_LAMBDA" => Some("lambdaAuthorizerConfig"),
        _ => None,
    }
}

/// Every member an authentication type can be configured through, so a row can assert
/// that the one the updated type needs is present and the others are gone.
const AUTH_CONFIG_MEMBERS: &[&str] = &[
    "openIDConnectConfig",
    "userPoolConfig",
    "lambdaAuthorizerConfig",
];

/// A complete `logConfig`, whose `fieldLogLevel` the caller picks so two rows can
/// differ in it. Source: `LogConfig`, which requires `fieldLogLevel` and
/// `cloudWatchLogsRoleArn`.
fn log_config(field_log_level: &str) -> Value {
    json!({
        "fieldLogLevel": field_log_level,
        "cloudWatchLogsRoleArn": format!("arn:aws:iam::000000000000:role/{field_log_level}-logs"),
        "excludeVerboseContent": false,
    })
}

/// A complete `enhancedMetricsConfig`. Source: `EnhancedMetricsConfig`, which requires
/// all three of its behaviours.
fn enhanced_metrics_config(operation_level: &str) -> Value {
    json!({
        "resolverLevelMetricsBehavior": "FULL_REQUEST_RESOLVER_METRICS",
        "dataSourceLevelMetricsBehavior": "FULL_REQUEST_DATA_SOURCE_METRICS",
        "operationLevelMetricsConfig": operation_level,
    })
}

/// A well-formed update body for an `API_KEY` API: the required members and nothing
/// else, which each refusal row below perturbs in exactly one way.
fn minimal_update_body(name: &str) -> Value {
    json!({
        "name": name,
        "authenticationType": "API_KEY",
        "queryDepthLimit": 0,
        "resolverCountLimit": 0,
        "xrayEnabled": false,
    })
}

/// The whole configuration an API is created with before each update row, so that a
/// member the update omits is observably CLEARED rather than merely never set.
fn fully_configured_create_body(index: usize, authentication_type: &str) -> Value {
    merge_members(
        json!({
            "name": format!("Api_before_{index}"),
            "authenticationType": authentication_type,
            // Fixed when the API is created: an update carries neither member.
            "apiType": if index.is_multiple_of(2) { "GRAPHQL" } else { "MERGED" },
            "visibility": if index.is_multiple_of(2) { "GLOBAL" } else { "PRIVATE" },
            "queryDepthLimit": 5,
            "resolverCountLimit": 50,
            "xrayEnabled": true,
            "introspectionConfig": "DISABLED",
            "ownerContact": "before@example.com",
            "logConfig": log_config("ERROR"),
            "mergedApiExecutionRoleArn": "arn:aws:iam::000000000000:role/before-merge",
            "enhancedMetricsConfig": enhanced_metrics_config("DISABLED"),
            "tags": { "team": "orders" },
        }),
        auth_config(authentication_type),
    )
}

/// An update replaces the API's configuration member for member, clears the settable
/// members it omits, and leaves the API's identity and its creation-time members
/// alone — for every authentication type, updated to every other one.
#[tokio::test]
async fn an_update_replaces_the_configuration_and_leaves_the_api_identity_untouched() {
    let router = router();

    for (index, from_auth) in AUTH_TYPES.iter().enumerate() {
        // Each type is updated TO the next one in the model's value set, and the
        // extra provider names the one after that, so no row names a type twice.
        let to_auth = AUTH_TYPES[(index + 1) % AUTH_TYPES.len()];
        let extra_auth = AUTH_TYPES[(index + 2) % AUTH_TYPES.len()];
        // Half the rows carry every settable optional member, half omit them all.
        let carries_optionals = index.is_multiple_of(2);
        // `introspectionConfig` at each of its two values and, on every third row,
        // absent — where the model documents the value it takes.
        let introspection = [Some("DISABLED"), Some("ENABLED"), None][index % 3];
        // 0 and the published maxima of queryDepthLimit (0..75) and
        // resolverCountLimit (0..10000), plus a value inside each range.
        let depth = [0, 1, 75, 40, 12][index];
        let resolvers = [0, 1, 10_000, 500, 9_999][index];
        let xray = !index.is_multiple_of(2);
        let case = format!("{from_auth} updated to {to_auth}");

        let created = create_api(&router, fully_configured_create_body(index, from_auth)).await;
        let api_id = created["apiId"].as_str().unwrap_or_default().to_string();
        assert!(
            !api_id.is_empty(),
            "case {case}: the API to update must have an apiId, got {created:?}"
        );

        let name = format!("Api_after_{index}");
        let mut body = merge_members(
            json!({
                "name": name,
                "authenticationType": to_auth,
                "queryDepthLimit": depth,
                "resolverCountLimit": resolvers,
                "xrayEnabled": xray,
            }),
            auth_config(to_auth),
        );
        if let Some(introspection) = introspection {
            body["introspectionConfig"] = json!(introspection);
        }
        if carries_optionals {
            body["ownerContact"] = json!("after@example.com");
            body["logConfig"] = log_config("ALL");
            body["mergedApiExecutionRoleArn"] =
                json!("arn:aws:iam::000000000000:role/after-merge");
            body["enhancedMetricsConfig"] = enhanced_metrics_config("ENABLED");
            body["additionalAuthenticationProviders"] = json!([complete_provider(extra_auth)]);
        }

        let (status, answer) = update_api(&router, &api_id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateGraphqlApi 200, got {status} {answer:?}"
        );
        let updated = answer["graphqlApi"].clone();

        assert_eq!(
            updated["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the submitted name, got {updated:?}"
        );
        assert_eq!(
            updated["authenticationType"].as_str(),
            Some(to_auth),
            "case {case}: expected the submitted authenticationType, got {updated:?}"
        );
        assert_eq!(
            updated["queryDepthLimit"].as_i64(),
            Some(i64::from(depth)),
            "case {case}: expected queryDepthLimit {depth}, got {updated:?}"
        );
        assert_eq!(
            updated["resolverCountLimit"].as_i64(),
            Some(i64::from(resolvers)),
            "case {case}: expected resolverCountLimit {resolvers}, got {updated:?}"
        );
        assert_eq!(
            updated["xrayEnabled"].as_bool(),
            Some(xray),
            "case {case}: expected xrayEnabled {xray}, got {updated:?}"
        );
        assert_eq!(
            updated["introspectionConfig"].as_str(),
            Some(introspection.unwrap_or("ENABLED")),
            "case {case}: expected the submitted introspectionConfig, or ENABLED where the \
             request named none, got {updated:?}"
        );

        // The configuration of the type updated TO is stored; the one the API was
        // created with is gone, because the update did not carry it.
        let needed = auth_config_member(to_auth);
        for member in AUTH_CONFIG_MEMBERS {
            if Some(*member) == needed {
                assert!(
                    !updated[member].is_null(),
                    "case {case}: expected {member} for {to_auth}, got {updated:?}"
                );
            } else {
                assert!(
                    updated[member].is_null(),
                    "case {case}: expected {member} to be absent after the update, \
                     got {updated:?}"
                );
            }
        }

        // The settable optional members: present with the submitted value on a row
        // that carries them, and cleared on a row that omits them even though the
        // create set every one.
        if carries_optionals {
            assert_eq!(
                updated["ownerContact"].as_str(),
                Some("after@example.com"),
                "case {case}: expected the submitted ownerContact, got {updated:?}"
            );
            assert_eq!(
                updated["logConfig"]["fieldLogLevel"].as_str(),
                Some("ALL"),
                "case {case}: expected the submitted logConfig, got {updated:?}"
            );
            assert_eq!(
                updated["mergedApiExecutionRoleArn"].as_str(),
                Some("arn:aws:iam::000000000000:role/after-merge"),
                "case {case}: expected the submitted mergedApiExecutionRoleArn, got {updated:?}"
            );
            assert_eq!(
                updated["enhancedMetricsConfig"]["operationLevelMetricsConfig"].as_str(),
                Some("ENABLED"),
                "case {case}: expected the submitted enhancedMetricsConfig, got {updated:?}"
            );
            assert_eq!(
                updated["additionalAuthenticationProviders"]
                    .as_array()
                    .map(Vec::len),
                Some(1),
                "case {case}: expected the submitted additional provider, got {updated:?}"
            );
            assert_eq!(
                updated["additionalAuthenticationProviders"][0]["authenticationType"].as_str(),
                Some(extra_auth),
                "case {case}: expected the additional provider's type {extra_auth}, \
                 got {updated:?}"
            );
        } else {
            for member in [
                "ownerContact",
                "logConfig",
                "mergedApiExecutionRoleArn",
                "enhancedMetricsConfig",
                "additionalAuthenticationProviders",
            ] {
                assert!(
                    updated[member].is_null(),
                    "case {case}: expected {member} to be cleared by an update that omits it, \
                     got {updated:?}"
                );
            }
        }

        // The API's identity and the members fixed at creation are untouched.
        for member in [
            "apiId",
            "arn",
            "uris",
            "owner",
            "apiType",
            "visibility",
            "tags",
        ] {
            assert_eq!(
                updated[member], created[member],
                "case {case}: expected {member} to survive the update unchanged, \
                 got {updated:?}"
            );
        }

        // The round trip: an independent read answers the updated record.
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_read_answers_the_update(&case, &updated, &fetched["graphqlApi"], &created);
    }
}

/// The read answers the record the update left behind, on the one difference between
/// the two replies: the update reports the API's configuration WITHOUT its `dns`
/// records, which it carries no member for and cannot change, while the read — where a
/// caller learns the API's endpoints — reports them, unchanged from the create.
fn assert_read_answers_the_update(case: &str, updated: &Value, read: &Value, created: &Value) {
    assert!(
        updated["dns"].is_null(),
        "case {case}: expected the update's reply to carry no dns records, got {updated:?}"
    );
    assert_eq!(
        read["dns"], created["dns"],
        "case {case}: expected the read to report the API's dns records unchanged, got {read:?}"
    );
    let mut without_dns = read.clone();
    if let Some(members) = without_dns.as_object_mut() {
        members.remove("dns");
    }
    assert_eq!(
        updated, &without_dns,
        "case {case}: GetGraphqlApi must answer the updated record, got {read:?}"
    );
}

/// A second update of the same API replaces the first one's configuration too, so the
/// rule holds for every update rather than only the first after the create.
#[tokio::test]
async fn each_update_replaces_what_the_previous_one_left() {
    let router = router();
    let created = create_api(&router, fully_configured_create_body(0, "API_KEY")).await;
    let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    // The rows differ along the name, the authentication type and whether the row
    // carries `ownerContact`, so each row's own value has to displace the previous
    // row's rather than merge with it.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("Api_first", "AWS_IAM", Some("first@example.com")),
        ("Api_second", "AMAZON_COGNITO_USER_POOLS", None),
        ("Api_third", "OPENID_CONNECT", Some("third@example.com")),
        ("Api_fourth", "API_KEY", None),
    ];
    for (name, authentication_type, owner_contact) in rows {
        let case = format!("{name} as {authentication_type}");
        let mut body = merge_members(
            json!({
                "name": name,
                "authenticationType": authentication_type,
                "queryDepthLimit": 3,
                "resolverCountLimit": 30,
                "xrayEnabled": false,
            }),
            auth_config(authentication_type),
        );
        if let Some(owner_contact) = owner_contact {
            body["ownerContact"] = json!(owner_contact);
        }

        let (status, answer) = update_api(&router, &api_id, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateGraphqlApi 200, got {status} {answer:?}"
        );
        let updated = answer["graphqlApi"].clone();
        assert_eq!(
            updated["name"].as_str(),
            Some(name),
            "case {case}: expected the submitted name, got {updated:?}"
        );
        assert_eq!(
            updated["authenticationType"].as_str(),
            Some(authentication_type),
            "case {case}: expected the submitted authenticationType, got {updated:?}"
        );
        assert_eq!(
            updated["ownerContact"].as_str(),
            owner_contact,
            "case {case}: expected ownerContact {owner_contact:?}, got {updated:?}"
        );

        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_read_answers_the_update(&case, &updated, &fetched["graphqlApi"], &created);
    }
}

/// An update the service refuses is refused on the ground the create call refuses the
/// same configuration on, and leaves the stored API exactly as it was.
#[tokio::test]
async fn a_refused_update_names_its_ground_and_changes_nothing() {
    let router = router();
    let created = create_api(&router, fully_configured_create_body(0, "API_KEY")).await;
    let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    let overlong_id = "d".repeat(API_ID_LEN + 1);
    // (case, the apiId addressed, the body, the expected status, the expected text).
    let rows: Vec<(String, String, Value, u16, String)> = vec![
        (
            "an apiId one character past the id length".to_string(),
            overlong_id.clone(),
            minimal_update_body("Api_overlong_id"),
            400,
            format!("Invalid API ID {overlong_id}"),
        ),
        (
            "a well-formed apiId this account does not hold".to_string(),
            "nonexistent0000000000".to_string(),
            minimal_update_body("Api_absent_id"),
            404,
            "API not found.".to_string(),
        ),
        (
            "OPENID_CONNECT without its configuration".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_no_oidc");
                body["authenticationType"] = json!("OPENID_CONNECT");
                body
            },
            400,
            "OpenID Connect Config missing".to_string(),
        ),
        (
            "AMAZON_COGNITO_USER_POOLS without its configuration".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_no_pool");
                body["authenticationType"] = json!("AMAZON_COGNITO_USER_POOLS");
                body
            },
            400,
            "User Pool Config is empty".to_string(),
        ),
        (
            "AWS_LAMBDA without its configuration".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_no_lambda");
                body["authenticationType"] = json!("AWS_LAMBDA");
                body
            },
            400,
            "Lambda Authorizer Config is missing".to_string(),
        ),
        (
            "a first additional provider without its configuration".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_provider_one");
                body["additionalAuthenticationProviders"] =
                    json!([{ "authenticationType": "OPENID_CONNECT" }]);
                body
            },
            400,
            "OpenID Connect Config is missing for additional authentication provider 1"
                .to_string(),
        ),
        (
            "a second additional provider without its configuration".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_provider_two");
                body["additionalAuthenticationProviders"] = json!([
                    complete_provider("AWS_IAM"),
                    { "authenticationType": "AMAZON_COGNITO_USER_POOLS" },
                ]);
                body
            },
            400,
            "User Pool Config is missing for additional authentication provider 2".to_string(),
        ),
        (
            "an additional provider repeating the API's own type".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_repeat_api_type");
                body["additionalAuthenticationProviders"] = json!([complete_provider("API_KEY")]);
                body
            },
            400,
            "Authentication type API_KEY for additional authentication provider 1 already \
             specified on the API. It can only be specified once."
                .to_string(),
        ),
        (
            "an additional provider repeating an earlier provider's type".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_repeat_provider_type");
                body["additionalAuthenticationProviders"] =
                    json!([complete_provider("AWS_IAM"), complete_provider("AWS_IAM")]);
                body
            },
            400,
            "Authentication type AWS_IAM for additional authentication provider 2 already \
             specified on the API. It can only be specified once."
                .to_string(),
        ),
        (
            "an OIDC issuer that is not an absolute URL".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_bare_issuer");
                body["authenticationType"] = json!("OPENID_CONNECT");
                body["openIDConnectConfig"] = json!({ "issuer": "issuer.example.com" });
                body
            },
            400,
            "Issuer is not a valid URL".to_string(),
        ),
        (
            "an additional provider's issuer that is not an absolute URL".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_relative_issuer");
                body["additionalAuthenticationProviders"] = json!([{
                    "authenticationType": "OPENID_CONNECT",
                    "openIDConnectConfig": { "issuer": "/oauth2" },
                }]);
                body
            },
            400,
            "Issuer is not a valid URL".to_string(),
        ),
        (
            "an ownerContact one character past its documented length".to_string(),
            api_id.clone(),
            {
                let mut body = minimal_update_body("Api_long_contact");
                body["ownerContact"] = json!("c".repeat(OWNER_CONTACT_MAX_LEN + 1));
                body
            },
            400,
            format!("ownerContact accepts 0-{OWNER_CONTACT_MAX_LEN} characters"),
        ),
    ];

    for (case, addressed, body, expected_status, expected_message) in rows {
        let (status, answer) = update_api(&router, &addressed, body).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected status {expected_status}, got {status} {answer:?}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case}: expected the refusal to state {expected_message:?}, got {answer:?}"
        );

        // The refused update wrote nothing: the API still reads back as created.
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["graphqlApi"], created,
            "case {case}: a refused update must leave the stored API as it was, got {fetched:?}"
        );
    }

    // The bound itself is accepted, so the refusal above is about exceeding the
    // documented length rather than about carrying an `ownerContact` at all.
    let contact = "c".repeat(OWNER_CONTACT_MAX_LEN);
    let mut body = minimal_update_body("Api_contact_at_bound");
    body["ownerContact"] = json!(contact);
    let (status, answer) = update_api(&router, &api_id, body).await;
    assert_eq!(
        status, 200,
        "case an ownerContact of exactly {OWNER_CONTACT_MAX_LEN} characters: expected 200, \
         got {status} {answer:?}"
    );
    assert_eq!(
        answer["graphqlApi"]["ownerContact"].as_str(),
        Some(contact.as_str()),
        "case an ownerContact of exactly {OWNER_CONTACT_MAX_LEN} characters: expected it \
         stored, got {answer:?}"
    );
}

/// What an update does not touch includes the resources held UNDER the API: the API
/// keys created beneath it, its environmental variables and its data sources all
/// survive, so an update replaces the API's configuration and nothing else.
#[tokio::test]
async fn an_update_leaves_the_resources_held_under_the_api_alone() {
    let router = router();
    let created = create_api(
        &router,
        json!({ "name": "Api_with_children", "authenticationType": "API_KEY" }),
    )
    .await;
    let api_id = created["apiId"].as_str().unwrap_or_default().to_string();

    let (status, key) = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({ "description": "the key" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed an API key: expected 200, got {status} {key:?}"
    );
    let key_id = key["apiKey"]["id"].as_str().unwrap_or_default().to_string();

    let (status, variables) = call(
        &router,
        "PUT",
        &format!("/v1/apis/{api_id}/environmentVariables"),
        Some(json!({ "environmentVariables": { "STAGE": "beta" } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed an environmental variable: expected 200, got {status} {variables:?}"
    );

    let (status, data_source) = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": "orders",
            "type": "AMAZON_DYNAMODB",
            // AppSync assumes a role to reach a DynamoDB table, so the create call
            // needs the role it assumes (`CreateDataSource`'s own refusal states it).
            "serviceRoleArn": "arn:aws:iam::000000000000:role/appsync-orders",
            "dynamodbConfig": { "tableName": "orders", "awsRegion": REGION },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed a data source: expected 200, got {status} {data_source:?}"
    );

    let (status, answer) = update_api(
        &router,
        &api_id,
        minimal_update_body("Api_with_children_renamed"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case update the API: expected 200, got {status} {answer:?}"
    );

    let (status, keys) = call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        status, 200,
        "case the API keys after the update: expected 200, got {status} {keys:?}"
    );
    assert_eq!(
        keys["apiKeys"][0]["id"].as_str(),
        Some(key_id.as_str()),
        "case the API keys after the update: expected the key created before it, got {keys:?}"
    );

    let (status, read_variables) = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/environmentVariables"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the environmental variables after the update: expected 200, \
         got {status} {read_variables:?}"
    );
    assert_eq!(
        read_variables["environmentVariables"]["STAGE"].as_str(),
        Some("beta"),
        "case the environmental variables after the update: expected the variable written \
         before it, got {read_variables:?}"
    );

    let (status, read_data_source) = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/datasources/orders"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the data source after the update: expected 200, got {status} {read_data_source:?}"
    );
    assert_eq!(
        read_data_source["dataSource"]["name"].as_str(),
        Some("orders"),
        "case the data source after the update: expected the data source created before it, \
         got {read_data_source:?}"
    );
}
