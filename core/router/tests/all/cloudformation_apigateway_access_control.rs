// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack creates the Amazon API Gateway resources that govern ACCESS to a
//! REST API — an `AWS::ApiGateway::ApiKey`, an `::Authorizer`, a `::UsagePlan` and the
//! `::UsagePlanKey` that puts a key in a plan — in API Gateway itself, and publishes for each
//! of them the identifier that service assigned.
//!
//! The rule under test: **the physical resource id CloudFormation records for a resource —
//! what `Ref` resolves to and what a later resource's property referencing it is resolved to
//! — is the identifier the service that OWNS the type assigned when the stack created the
//! resource there, never a label derived from the declaration; so a stack that reaches
//! `CREATE_COMPLETE` really holds every declared resource of a type this emulator serves,
//! each published identifier resolves through that service's own read and its own list, and
//! the resource it names is the one the api's data plane enforces.** Two crates have to agree
//! for that to hold (CloudFormation makes the calls, API Gateway serves them and then the
//! invoke), so the suite lives here rather than in either of them.
//!
//! `cloudformation_rest_api_serves_its_stage.rs` owns the five types a REST API is built out
//! of; this suite owns the four that decide who reaches the stage it deployed.
//!
//! The rows differ along what the rule generalises over: whether the key's value is one the
//! template states or one API Gateway mints, whether the key declares stage keys, tags, a
//! customer id or only a name, whether the plan declares a quota, a plan-wide throttle, a
//! per-method throttle keyed by the caller's own method path, api stages or none of them, and
//! which of the three modelled authorizer types the authorizer is (and so which of its
//! members the declaration has to carry). Every expected value is computed here from what the
//! row declares or read from the owning service, never from the answer being checked.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, outputs_of};

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How many characters an identifier API Gateway mints for a record of the ACCOUNT — a key, a
/// plan, an authorizer — carries, and the alphabet every minted id draws from: ten lowercase
/// alphanumerics (that service's own state module mints every id at this width).
const MINTED_ID_LEN: usize = 10;

/// The emulator as it is ASSEMBLED, on one engine: the resources CloudFormation creates are
/// the resources this suite then reads and invokes through.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Read one API Gateway control-plane resource over its restJson1 wire: that service signs as
/// `apigateway` and binds every REST operation under its own URI.
async fn apigateway(router: &MultiServiceRouter, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// Invoke a deployed stage at the invoke URL the stack published, exactly as a client reaches
/// AWS: the request's host is the api's own execute-api endpoint and its path is the stage and
/// the resource path. `headers` are the ones the call presents beyond those.
async fn invoke(
    router: &MultiServiceRouter,
    invoke_url: &str,
    method: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let without_scheme = invoke_url
        .strip_prefix("https://")
        .or_else(|| invoke_url.strip_prefix("http://"))
        .unwrap_or(invoke_url);
    let (host, path) = match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Bytes::new()).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The ids one collection of API Gateway's own LIST answers, under the member name that
/// service's model gives the page (`jsonName: item`).
fn listed_ids(page: &Value) -> Vec<String> {
    page.get("item")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The api scaffold every row deploys the access control over: an api with one `MOCK` method
/// under its root, a deployment of it and the stage that serves it.
///
/// `api_key_required` is what makes the method one an API key admits, and `authorizer` names
/// the logical id of the authorizer that decides it — `None` leaving the method open.
fn api_scaffold(name: &str, api_key_required: bool, authorizer: Option<&str>) -> Value {
    let mut method = json!({
        "RestApiId": { "Ref": "Api" },
        "ResourceId": { "Ref": "Items" },
        "HttpMethod": "GET",
        "AuthorizationType": "NONE",
        "ApiKeyRequired": api_key_required,
        "MethodResponses": [{ "StatusCode": "200" }],
        "Integration": {
            "Type": "MOCK",
            "RequestTemplates": { "application/json": "{\"statusCode\": 200}" },
            "IntegrationResponses": [{
                "StatusCode": "200",
                "ResponseTemplates": { "application/json": "{\"served\":true}" }
            }]
        }
    });
    if let Some(authorizer) = authorizer {
        let properties = method.as_object_mut().expect("a method object");
        properties.insert("AuthorizationType".to_string(), json!("CUSTOM"));
        properties.insert("AuthorizerId".to_string(), json!({ "Ref": authorizer }));
    }
    json!({
        "Api": {
            "Type": "AWS::ApiGateway::RestApi",
            "Properties": { "Name": name, "EndpointConfiguration": { "Types": ["REGIONAL"] } }
        },
        "Items": {
            "Type": "AWS::ApiGateway::Resource",
            "Properties": {
                "RestApiId": { "Ref": "Api" },
                "ParentId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                "PathPart": "items"
            }
        },
        "Method": { "Type": "AWS::ApiGateway::Method", "Properties": method },
        "Deployment": {
            "Type": "AWS::ApiGateway::Deployment",
            "DependsOn": ["Method"],
            "Properties": { "RestApiId": { "Ref": "Api" } }
        },
        "Stage": {
            "Type": "AWS::ApiGateway::Stage",
            "Properties": {
                "RestApiId": { "Ref": "Api" },
                "DeploymentId": { "Ref": "Deployment" },
                "StageName": "live"
            }
        }
    })
}

/// The `Outputs` every row publishes: the api, and the `Ref` of each access-control resource
/// — which is the value a client deploying the stack would then call API Gateway with.
fn access_control_outputs() -> Value {
    json!({
        "ApiId": { "Value": { "Ref": "Api" } },
        "KeyId": { "Value": { "Ref": "Key" } },
        "KeyIdAttribute": { "Value": { "Fn::GetAtt": ["Key", "APIKeyId"] } },
        "PlanId": { "Value": { "Ref": "Plan" } },
        "PlanIdAttribute": { "Value": { "Fn::GetAtt": ["Plan", "Id"] } },
        "PlanKeyId": { "Value": { "Ref": "PlanKey" } },
        "AuthorizerId": { "Value": { "Ref": "Auth" } },
        "AuthorizerIdAttribute": { "Value": { "Fn::GetAtt": ["Auth", "AuthorizerId"] } }
    })
}

/// One row of the table: what the template declares of each access-control type, and what
/// the owning service's own reads must then report.
struct Row {
    case: &'static str,
    /// The declarations of the four access-control resources, built from the stack name so
    /// each row's names are its own.
    resources: fn(&str) -> Value,
    /// The key's `value`, where the template STATES one — the row with none leaves the value
    /// to API Gateway, so nothing here can predict it.
    key_value: Option<&'static str>,
    /// The `keyType` the association declares, which its own read reports as `type`.
    key_type: &'static str,
}

/// The key's properties whose declared value its own read reports verbatim, as
/// `(property, member)` pairs: the scalar ones, each the property with a lower-cased initial
/// (`GetApiKey`'s own answer members). A property a row's declaration omits is skipped.
///
/// `StageKeys` and `Tags` are not among them: the read reports a stage key as
/// `{restApiId}/{stageName}` TEXT while a declaration states an object (and one whose api is
/// a `Ref` this suite has not resolved), and the tags a resource carries are reported beside
/// the ones CloudFormation adds of its own accord.
const KEY_REPORTED: &[(&str, &str)] = &[
    ("CustomerId", "customerId"),
    ("Description", "description"),
    ("Enabled", "enabled"),
    ("Name", "name"),
];

/// The plan's properties whose declared value its own read reports verbatim. The quota, the
/// throttle and the api stages are checked member by member below, since the read spells
/// their members in the model's own case.
const PLAN_REPORTED: &[(&str, &str)] =
    &[("Description", "description"), ("UsagePlanName", "name")];

/// The authorizer's properties whose declared value its own read reports verbatim.
///
/// `AuthorizerUri` and `ProviderARNs` are not among them: each row states them through
/// `Fn::Sub`, so the declaration this suite reads carries the intrinsic rather than the text
/// the deployment resolved it to.
const AUTHORIZER_REPORTED: &[(&str, &str)] = &[
    ("AuthorizerResultTtlInSeconds", "authorizerResultTtlInSeconds"),
    ("IdentitySource", "identitySource"),
    ("Name", "name"),
    ("Type", "type"),
];

/// A key whose VALUE the template states, stage-keyed to the api's own stage, with tags; a
/// plan with a quota, a plan-wide throttle and a per-method throttle on the api stage; and a
/// `TOKEN` authorizer.
fn stated_value_resources(stack: &str) -> Value {
    json!({
        "Key": {
            "Type": "AWS::ApiGateway::ApiKey",
            "Properties": {
                "Name": format!("{stack}-stated"),
                "Description": "the key whose value the template states",
                "Enabled": true,
                "Value": "stated-api-key-value-0001",
                "StageKeys": [{ "RestApiId": { "Ref": "Api" }, "StageName": "live" }],
                "Tags": [{ "Key": "tier", "Value": "gold" }]
            }
        },
        "Plan": {
            "Type": "AWS::ApiGateway::UsagePlan",
            "Properties": {
                "UsagePlanName": format!("{stack}-metered"),
                "Description": "a quota and a throttle",
                "Quota": { "Limit": 100, "Offset": 0, "Period": "DAY" },
                "Throttle": { "BurstLimit": 5, "RateLimit": 2.0 },
                "ApiStages": [{
                    "ApiId": { "Ref": "Api" },
                    "Stage": "live",
                    "Throttle": { "/items/GET": { "BurstLimit": 3, "RateLimit": 1.0 } }
                }],
                "Tags": [{ "Key": "tier", "Value": "gold" }]
            }
        },
        "PlanKey": {
            "Type": "AWS::ApiGateway::UsagePlanKey",
            "Properties": {
                "KeyId": { "Ref": "Key" },
                "KeyType": "API_KEY",
                "UsagePlanId": { "Ref": "Plan" }
            }
        },
        "Auth": {
            "Type": "AWS::ApiGateway::Authorizer",
            "Properties": {
                "Name": format!("{stack}-token"),
                "RestApiId": { "Ref": "Api" },
                "Type": "TOKEN",
                "IdentitySource": "method.request.header.Authorization",
                "AuthorizerResultTtlInSeconds": 300,
                "AuthorizerUri": {
                    "Fn::Sub": "arn:aws:apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:gate/invocations"
                }
            }
        }
    })
}

/// A key whose value API Gateway MINTS (the template states none), a plan whose only
/// configuration is the api stage it meters, and a `REQUEST` authorizer.
fn minted_value_resources(stack: &str) -> Value {
    json!({
        "Key": {
            "Type": "AWS::ApiGateway::ApiKey",
            "Properties": { "Name": format!("{stack}-minted"), "Enabled": true }
        },
        "Plan": {
            "Type": "AWS::ApiGateway::UsagePlan",
            "Properties": {
                "UsagePlanName": format!("{stack}-open"),
                "ApiStages": [{ "ApiId": { "Ref": "Api" }, "Stage": "live" }]
            }
        },
        "PlanKey": {
            "Type": "AWS::ApiGateway::UsagePlanKey",
            "Properties": {
                "KeyId": { "Ref": "Key" },
                "KeyType": "API_KEY",
                "UsagePlanId": { "Ref": "Plan" }
            }
        },
        "Auth": {
            "Type": "AWS::ApiGateway::Authorizer",
            "Properties": {
                "Name": format!("{stack}-request"),
                "RestApiId": { "Ref": "Api" },
                "Type": "REQUEST",
                "IdentitySource": "method.request.header.X-Caller",
                "AuthorizerResultTtlInSeconds": 0,
                "AuthorizerUri": {
                    "Fn::Sub": "arn:aws:apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:gate/invocations"
                }
            }
        }
    })
}

/// A key that declares a customer id and no name, a plan that meters NO api stage at all, and
/// a `COGNITO_USER_POOLS` authorizer, whose declaration carries provider ARNs instead of a
/// function URI.
fn customer_keyed_resources(stack: &str) -> Value {
    json!({
        "Key": {
            "Type": "AWS::ApiGateway::ApiKey",
            "Properties": {
                "CustomerId": format!("{stack}-customer"),
                "Description": "a key the marketplace customer holds",
                "Enabled": false,
                "Value": "customer-api-key-value-0002"
            }
        },
        "Plan": {
            "Type": "AWS::ApiGateway::UsagePlan",
            "Properties": {
                "UsagePlanName": format!("{stack}-unstaged"),
                "Description": "a plan over no api stage"
            }
        },
        "PlanKey": {
            "Type": "AWS::ApiGateway::UsagePlanKey",
            "Properties": {
                "KeyId": { "Ref": "Key" },
                "KeyType": "API_KEY",
                "UsagePlanId": { "Ref": "Plan" }
            }
        },
        "Auth": {
            "Type": "AWS::ApiGateway::Authorizer",
            "Properties": {
                "Name": format!("{stack}-pools"),
                "RestApiId": { "Ref": "Api" },
                "Type": "COGNITO_USER_POOLS",
                "IdentitySource": "method.request.header.Authorization",
                "ProviderARNs": [{
                    "Fn::Sub": "arn:aws:cognito-idp:${AWS::Region}:${AWS::AccountId}:userpool/us-east-1_pool"
                }]
            }
        }
    })
}

const ROWS: &[Row] = &[
    Row {
        case: "a key whose value the template states, a plan with a quota and both throttles",
        resources: stated_value_resources,
        key_value: Some("stated-api-key-value-0001"),
        key_type: "API_KEY",
    },
    Row {
        case: "a key whose value API Gateway mints, a plan whose only setting is its api stage",
        resources: minted_value_resources,
        key_value: None,
        key_type: "API_KEY",
    },
    Row {
        case: "a disabled key with a customer id, a plan over no api stage, user-pool authorizer",
        resources: customer_keyed_resources,
        key_value: Some("customer-api-key-value-0002"),
        key_type: "API_KEY",
    },
];

/// The declared value of one property of a resource the row's template states, as the text the
/// owning service would report it as — the expectation side of every member check below is
/// read out of the TEMPLATE, never out of the answer being checked.
fn declared(resources: &Value, logical_id: &str, property: &str) -> Option<Value> {
    resources
        .get(logical_id)?
        .get("Properties")?
        .get(property)
        .cloned()
}

/// Whether `id` is of the shape API Gateway MINTS an identifier in, and so not a label a
/// naming rule could have derived from the declaration (which carries the account id and a
/// `:`).
fn is_minted_id(id: &str) -> bool {
    id.chars().count() == MINTED_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[tokio::test]
async fn a_stack_s_access_control_resources_are_created_in_api_gateway() {
    let router = build_router();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("apigw-access-control-{index}");
        let resources = (row.resources)(&stack);
        let mut declarations = api_scaffold(&format!("{stack}-api"), false, None);
        for (logical_id, declaration) in resources.as_object().expect("declared resources") {
            declarations
                .as_object_mut()
                .expect("the scaffold's resources")
                .insert(logical_id.clone(), declaration.clone());
        }
        let template = json!({
            "Resources": declarations,
            "Outputs": access_control_outputs()
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", &stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (status, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        let failures = if status == CREATE_COMPLETE {
            Vec::new()
        } else {
            let (_, events) =
                cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack)]).await;
            elements(&events, "ResourceStatusReason")
        };
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
             resource failures {failures:?}"
        );

        let outputs = outputs_of(&described);
        let published = |name: &str| -> String {
            outputs.get(name).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: the stack published no {name} output in {described}")
            })
        };
        let api_id = published("ApiId");
        let key_id = published("KeyId");
        let plan_id = published("PlanId");
        let plan_key_id = published("PlanKeyId");
        let authorizer_id = published("AuthorizerId");

        // Every published identifier is one API Gateway minted, not a label derived from the
        // declaration: the `<account>:<name>` form a stack with no provider falls back to
        // resolves to nothing in that service.
        for (what, id) in [
            ("the api key", &key_id),
            ("the usage plan", &plan_id),
            ("the authorizer", &authorizer_id),
        ] {
            assert!(
                is_minted_id(id),
                "case {case:?}: expected {what}'s published id to be an identifier API Gateway \
                 minted ({MINTED_ID_LEN} lowercase alphanumerics), got {id:?}"
            );
        }
        // An ASSOCIATION is addressed by a PAIR — `GetUsagePlanKey` takes the plan and the key
        // — so no single minted id names one: what the stack publishes for it is both halves,
        // each of them an identifier API Gateway minted.
        let (published_key, published_plan) = plan_key_id
            .split_once(':')
            .unwrap_or((plan_key_id.as_str(), ""));
        for (what, id) in [
            ("the usage plan key's key half", published_key),
            ("the usage plan key's plan half", published_plan),
        ] {
            assert!(
                is_minted_id(id),
                "case {case:?}: expected {what} to be an identifier API Gateway minted \
                 ({MINTED_ID_LEN} lowercase alphanumerics), got {id:?} out of {plan_key_id:?}"
            );
        }

        // `Ref` and the type's own published attribute carry ONE identifier each.
        for (what, reference, attribute) in [
            ("the api key", &key_id, published("KeyIdAttribute")),
            ("the usage plan", &plan_id, published("PlanIdAttribute")),
            (
                "the authorizer",
                &authorizer_id,
                published("AuthorizerIdAttribute"),
            ),
        ] {
            assert_eq!(
                *reference, attribute,
                "case {case:?}: expected {what}'s Ref and its published attribute to carry the \
                 same identifier, got {reference:?} and {attribute:?}"
            );
        }

        // The key the stack published is one API Gateway HOLDS: the stack's own records are
        // not evidence of anything, the owning service's read is.
        let (status, key) = apigateway(&router, &format!("/apikeys/{key_id}?includeValue=true")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetApiKey for the key the stack published \
             ({key_id}), got {status} with body {key}"
        );
        assert_eq!(
            key.get("id").and_then(Value::as_str),
            Some(key_id.as_str()),
            "case {case:?}: expected the key to report the id the stack published, got {key}"
        );
        for (property, member) in KEY_REPORTED {
            let Some(stated) = declared(&resources, "Key", property) else {
                continue;
            };
            assert_eq!(
                key.get(*member),
                Some(&stated),
                "case {case:?}: expected the key's {member:?} to be the declared {property} \
                 {stated:?}, got {key}"
            );
        }
        if let Some(value) = row.key_value {
            assert_eq!(
                key.get("value").and_then(Value::as_str),
                Some(value),
                "case {case:?}: expected the key to carry the value the template stated, got \
                 {key}"
            );
        } else {
            let minted = key.get("value").and_then(Value::as_str).unwrap_or_default();
            assert!(
                !minted.is_empty(),
                "case {case:?}: expected API Gateway to have minted a value for the key the \
                 template stated none for, got {key}"
            );
        }

        // So is the plan, with the configuration the declaration asked for.
        let (status, plan) = apigateway(&router, &format!("/usageplans/{plan_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetUsagePlan for the plan the stack \
             published ({plan_id}), got {status} with body {plan}"
        );
        assert_eq!(
            plan.get("id").and_then(Value::as_str),
            Some(plan_id.as_str()),
            "case {case:?}: expected the plan to report the id the stack published, got {plan}"
        );
        for (property, member) in PLAN_REPORTED {
            let Some(stated) = declared(&resources, "Plan", property) else {
                continue;
            };
            assert_eq!(
                plan.get(*member),
                Some(&stated),
                "case {case:?}: expected the plan's {member:?} to be the declared {property} \
                 {stated:?}, got {plan}"
            );
        }
        if let Some(quota) = declared(&resources, "Plan", "Quota") {
            assert_eq!(
                plan.get("quota").and_then(|held| held.get("limit")),
                quota.get("Limit"),
                "case {case:?}: expected the plan's quota limit to be the declared one, got \
                 {plan}"
            );
            assert_eq!(
                plan.get("quota").and_then(|held| held.get("period")),
                quota.get("Period"),
                "case {case:?}: expected the plan's quota period to be the declared one, got \
                 {plan}"
            );
        }
        if let Some(throttle) = declared(&resources, "Plan", "Throttle") {
            assert_eq!(
                plan.get("throttle").and_then(|held| held.get("burstLimit")),
                throttle.get("BurstLimit"),
                "case {case:?}: expected the plan's throttle burst to be the declared one, got \
                 {plan}"
            );
        }
        // An api stage names the api by the id API Gateway minted for it, which is what the
        // declaration's `Ref` resolved to.
        let staged: Vec<&str> = plan
            .get("apiStages")
            .and_then(Value::as_array)
            .map(|stages| {
                stages
                    .iter()
                    .filter_map(|stage| stage.get("apiId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let declares_a_stage = declared(&resources, "Plan", "ApiStages").is_some();
        assert_eq!(
            staged.contains(&api_id.as_str()),
            declares_a_stage,
            "case {case:?}: expected the plan to meter the api {api_id:?} exactly when its \
             declaration names an api stage ({declares_a_stage}), got {staged:?}"
        );

        // And the association: it is addressed by the key it put in the plan AND the plan that
        // holds it, so the identifier the stack publishes for it carries both halves — and its
        // read lives under that plan.
        assert_eq!(
            plan_key_id,
            format!("{key_id}:{plan_id}"),
            "case {case:?}: expected the association's published id to be the id of the key it \
             put in the plan beside that plan ({key_id}:{plan_id}), got {plan_key_id:?}"
        );
        let (status, association) =
            apigateway(&router, &format!("/usageplans/{plan_id}/keys/{key_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetUsagePlanKey for the association the \
             stack published, got {status} with body {association}"
        );
        assert_eq!(
            association.get("type").and_then(Value::as_str),
            Some(row.key_type),
            "case {case:?}: expected the association to report the declared keyType, got \
             {association}"
        );

        // And the authorizer, under the api that holds it.
        let (status, authorizer) = apigateway(
            &router,
            &format!("/restapis/{api_id}/authorizers/{authorizer_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetAuthorizer for the authorizer the stack \
             published ({authorizer_id}), got {status} with body {authorizer}"
        );
        assert_eq!(
            authorizer.get("id").and_then(Value::as_str),
            Some(authorizer_id.as_str()),
            "case {case:?}: expected the authorizer to report the id the stack published, got \
             {authorizer}"
        );
        for (property, member) in AUTHORIZER_REPORTED {
            let Some(stated) = declared(&resources, "Auth", property) else {
                continue;
            };
            assert_eq!(
                authorizer.get(*member),
                Some(&stated),
                "case {case:?}: expected the authorizer's {member:?} to be the declared \
                 {property} {stated:?}, got {authorizer}"
            );
        }

        // Discovery by LIST answers the same resources: a client that finds a stack's key or
        // plan by listing the account's own collections reads them back too.
        let (status, keys) = apigateway(&router, "/apikeys").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetApiKeys, got {status} with body {keys}"
        );
        let listed = listed_ids(&keys);
        assert!(
            listed.contains(&key_id),
            "case {case:?}: expected GetApiKeys to list the key the stack created ({key_id}), \
             got {listed:?}"
        );
        let (status, plans) = apigateway(&router, "/usageplans").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetUsagePlans, got {status} with body {plans}"
        );
        let listed = listed_ids(&plans);
        assert!(
            listed.contains(&plan_id),
            "case {case:?}: expected GetUsagePlans to list the plan the stack created \
             ({plan_id}), got {listed:?}"
        );
    }
}

/// The deployed stage ENFORCES the access control the stack created: a key-required method
/// admits the key the stack's usage plan meters for that stage, and refuses a key the stack
/// created but put in no plan — which is only decidable if both keys, the plan and the
/// association really exist in API Gateway.
#[tokio::test]
async fn the_deployed_stage_admits_only_the_key_the_stack_metered() {
    let router = build_router();
    let stack = "apigw-access-control-enforced";
    let metered = "metered-api-key-value-00001";
    let unmetered = "unmetered-api-key-value-001";
    let mut declarations = api_scaffold(&format!("{stack}-api"), true, None);
    let resources = declarations
        .as_object_mut()
        .expect("the scaffold's resources");
    for (logical_id, declaration) in [
        (
            "Key",
            json!({
                "Type": "AWS::ApiGateway::ApiKey",
                "Properties": {
                    "Name": format!("{stack}-metered"),
                    "Enabled": true,
                    "Value": metered
                }
            }),
        ),
        (
            "Spare",
            json!({
                "Type": "AWS::ApiGateway::ApiKey",
                "Properties": {
                    "Name": format!("{stack}-unmetered"),
                    "Enabled": true,
                    "Value": unmetered
                }
            }),
        ),
        (
            "Plan",
            json!({
                "Type": "AWS::ApiGateway::UsagePlan",
                "Properties": {
                    "UsagePlanName": format!("{stack}-plan"),
                    "ApiStages": [{ "ApiId": { "Ref": "Api" }, "Stage": "live" }]
                }
            }),
        ),
        (
            "PlanKey",
            json!({
                "Type": "AWS::ApiGateway::UsagePlanKey",
                "Properties": {
                    "KeyId": { "Ref": "Key" },
                    "KeyType": "API_KEY",
                    "UsagePlanId": { "Ref": "Plan" }
                }
            }),
        ),
    ] {
        resources.insert(logical_id.to_string(), declaration);
    }
    let template = json!({
        "Resources": declarations,
        "Outputs": {
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub": "https://${Api}.execute-api.${AWS::Region}.amazonaws.com/live/items"
                }
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let invoke_url = outputs_of(&described)
        .get("InvokeUrl")
        .cloned()
        .unwrap_or_else(|| panic!("the stack published no InvokeUrl output in {described}"));

    let rows: &[(&str, Option<&str>, u16)] = &[
        ("the key the stack's plan meters", Some(metered), 200),
        ("a key the stack created and metered nowhere", Some(unmetered), 403),
        ("a value no key of the account carries", Some("no-such-api-key-value-0001"), 403),
        ("no key at all", None, 403),
    ];
    for (case, presented, expected) in rows {
        let headers: Vec<(&str, &str)> = presented
            .map(|value| vec![("x-api-key", value)])
            .unwrap_or_default();
        let (status, answered) = invoke(&router, &invoke_url, "GET", &headers).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected the deployed stage to answer HTTP {expected} at \
             {invoke_url}, got {status} with body {answered}"
        );
    }
}

/// A method authorized by the stack's own `AWS::ApiGateway::Authorizer` is decided by that
/// authorizer: a call presenting its identity source NOWHERE is refused 401 `Unauthorized` —
/// the answer for an authorizer that exists and was not called — rather than the 500 a method
/// whose `AuthorizerId` names no authorizer of the api earns.
#[tokio::test]
async fn a_method_authorized_by_the_stack_s_authorizer_enforces_its_identity_source() {
    let router = build_router();
    let stack = "apigw-access-control-authorizer";
    let mut declarations = api_scaffold(&format!("{stack}-api"), false, Some("Auth"));
    declarations
        .as_object_mut()
        .expect("the scaffold's resources")
        .insert(
            "Auth".to_string(),
            json!({
                "Type": "AWS::ApiGateway::Authorizer",
                "Properties": {
                    "Name": format!("{stack}-token"),
                    "RestApiId": { "Ref": "Api" },
                    "Type": "TOKEN",
                    "IdentitySource": "method.request.header.Authorization",
                    "AuthorizerResultTtlInSeconds": 300,
                    "AuthorizerUri": {
                        "Fn::Sub": "arn:aws:apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/arn:aws:lambda:${AWS::Region}:${AWS::AccountId}:function:gate/invocations"
                    }
                }
            }),
        );
    let template = json!({
        "Resources": declarations,
        "Outputs": {
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub": "https://${Api}.execute-api.${AWS::Region}.amazonaws.com/live/items"
                }
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let invoke_url = outputs_of(&described)
        .get("InvokeUrl")
        .cloned()
        .unwrap_or_else(|| panic!("the stack published no InvokeUrl output in {described}"));

    let (status, answered) = invoke(&router, &invoke_url, "GET", &[]).await;
    assert_eq!(
        status, 401,
        "case \"no identity source presented\": expected the stage to refuse the call with HTTP \
         401, got {status} with body {answered}"
    );
}
