// SPDX-License-Identifier: CC0-1.0

//! An `UpdateStack` that re-declares an `AWS::ApiGatewayV2::Api` the stack already has,
//! end to end: the CloudFormation calls over its awsQuery wire, the api reads over API
//! Gateway v2's own `restJson1` wire.
//!
//! RULE: a property a deployment states differently for a resource the stack already holds
//! is applied to that resource through the owning service's own update call when that call
//! takes a member for it — the resource keeps the identity its service minted, that service
//! really holds the new value, and the stack operation reaches `UPDATE_COMPLETE` — while a
//! property the update call takes no member for is create-only, so such a declaration asks
//! for a DIFFERENT resource and the stack replaces it. For an api, `UpdateApi`
//! (`PATCH /v2/apis/{ApiId}`) takes every property of the type but its protocol, its
//! definition and its tags, and `ProtocolType` is a member of `CreateApi` alone.
//!
//! The rows differ along what the rule generalises over: which property changes, whether the
//! create declared it at all, how many change at once, and the FORM of the value — a string,
//! the empty string, a boolean, a value out of a closed set, a selection expression and a
//! nested object.
//!
//! A replacement is the other half of the same rule, and the cases that drive it hold the
//! stack to what CloudFormation reports and does about one: the resource's events are the
//! UPDATE's own and the first of them says why a new physical resource is being created, the
//! resources that live inside the replaced api are created inside the one that replaced it,
//! and what becomes of the OLD physical resource is its `UpdateReplacePolicy` — deleted by
//! default, and kept, with its deletion reported as skipped, under `Retain`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::apigateway;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, status, RESOURCE_CREATION_INITIATED, RESOURCE_REPLACEMENT_IN_PROGRESS,
};

/// How many reads a case gives a stack operation to reach its terminal status in: a stack is
/// owed that status once the services that own its resources report them ready, which only a
/// read asks them about, so a client polls — which is what this is.
const POLL_ATTEMPTS: usize = 20;

/// The two values `ProtocolType` takes (`ProtocolType`: `HTTP | WEBSOCKET`).
const HTTP: &str = "HTTP";
const WEBSOCKET: &str = "WEBSOCKET";

/// A one-api template whose `Api` declares `properties`.
fn api_template(properties: Value) -> String {
    json!({
        "Resources": {
            "Api": { "Type": "AWS::ApiGatewayV2::Api", "Properties": properties }
        }
    })
    .to_string()
}

/// Read a stack until it reports a status that is no longer in progress, and answer that
/// read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (code, body) = describe_stack(router, stack).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DescribeStacks to answer 200, got {code} {body}"
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// The `StackStatus` and `StackStatusReason` a stack settled on.
async fn settled(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let described = poll_to_terminal(router, case, stack).await;
    (
        xml_text(&described, "StackStatus").unwrap_or_default(),
        xml_text(&described, "StackStatusReason").unwrap_or_default(),
    )
}

/// The `ApiId` the stack publishes for its `Api` resource — the handle a client reaches the
/// api by, which an in-place update must not change.
async fn published_api_id(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    published_id(router, case, stack, "Api").await
}

/// The `PhysicalResourceId` the stack publishes for one of its logical resources.
async fn published_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackResource({logical_id}) to answer 200, got {code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// `GetApi` (`GET /v2/apis/{ApiId}`): the status and the api record that service holds.
async fn get_api(router: &MultiServiceRouter, api_id: &str) -> (u16, Value) {
    apigateway(router, "GET", &format!("/v2/apis/{api_id}")).await
}

/// Every `(ResourceStatus, ResourceStatusReason)` the stack's event stream carries for one
/// logical resource, NEWEST FIRST — `DescribeStackEvents` "returns all stack related events
/// for a specified stack in reverse chronological order".
///
/// The statuses and the reasons are read as PAIRS, which neither reader in `stack_lifecycle`
/// answers: what a replacement has to be read on is the reason that sits on ONE of two events
/// of the same status, so the two members have to come back together. An event carrying no
/// reason reports the empty string for it.
async fn resource_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Vec<(String, String)> {
    let (code, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackEvents to answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .filter_map(|entry| {
            Some((
                xml_text(entry, "ResourceStatus")?,
                xml_text(entry, "ResourceStatusReason").unwrap_or_default(),
            ))
        })
        .collect()
}

/// The api API Gateway v2 really holds under `api_id`, as that service reports it.
async fn held_api(router: &MultiServiceRouter, case: &str, api_id: &str) -> Value {
    let (code, document) = get_api(router, api_id).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetApi({api_id}) to answer 200, got {code} with {document}"
    );
    document
}

/// The value a reported record carries at a dotted member `path` (`corsConfiguration
/// .allowOrigins`), or `None` where it carries none.
fn held_member(record: &Value, path: &str) -> Option<Value> {
    let mut found = record;
    for segment in path.split('.') {
        found = found.get(segment)?;
    }
    Some(found.clone())
}

/// One row of the in-place update table.
struct UpdateCase {
    case: &'static str,
    stack: &'static str,
    /// The properties the CREATE declares.
    created: Value,
    /// The properties the revision declares.
    revised: Value,
    /// What API Gateway v2 must report for the api once the revision is deployed, as
    /// `(dotted member path, value)` pairs.
    expected: Vec<(&'static str, Value)>,
}

/// The class the rule covers: a revision that states a different value for a property
/// `UpdateApi` takes a member for.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a description the create declared, re-valued",
            stack: "apigwv2-inplace-description",
            created: json!({
                "Name": "apigwv2-inplace-description-api",
                "ProtocolType": HTTP,
                "Description": "before",
            }),
            revised: json!({
                "Name": "apigwv2-inplace-description-api",
                "ProtocolType": HTTP,
                "Description": "after",
            }),
            expected: vec![("description", json!("after"))],
        },
        UpdateCase {
            case: "a description the create did not declare at all",
            stack: "apigwv2-inplace-added",
            created: json!({ "Name": "apigwv2-inplace-added-api", "ProtocolType": HTTP }),
            revised: json!({
                "Name": "apigwv2-inplace-added-api",
                "ProtocolType": HTTP,
                "Description": "added by the revision",
            }),
            expected: vec![("description", json!("added by the revision"))],
        },
        UpdateCase {
            case: "a description emptied, which that shape admits",
            stack: "apigwv2-inplace-emptied",
            created: json!({
                "Name": "apigwv2-inplace-emptied-api",
                "ProtocolType": HTTP,
                "Description": "to be emptied",
            }),
            revised: json!({
                "Name": "apigwv2-inplace-emptied-api",
                "ProtocolType": HTTP,
                "Description": "",
            }),
            expected: vec![("description", json!(""))],
        },
        UpdateCase {
            case: "two properties at once, one of them new",
            stack: "apigwv2-inplace-several",
            created: json!({
                "Name": "apigwv2-inplace-several-api",
                "ProtocolType": HTTP,
                "Description": "before",
            }),
            revised: json!({
                "Name": "apigwv2-inplace-several-api",
                "ProtocolType": HTTP,
                "Description": "after",
                "Version": "2024-05-01",
            }),
            expected: vec![
                ("description", json!("after")),
                ("version", json!("2024-05-01")),
            ],
        },
        UpdateCase {
            case: "a BOOLEAN property turned on",
            stack: "apigwv2-inplace-boolean",
            created: json!({
                "Name": "apigwv2-inplace-boolean-api",
                "ProtocolType": HTTP,
                "DisableExecuteApiEndpoint": false,
            }),
            revised: json!({
                "Name": "apigwv2-inplace-boolean-api",
                "ProtocolType": HTTP,
                "DisableExecuteApiEndpoint": true,
            }),
            expected: vec![("disableExecuteApiEndpoint", json!(true))],
        },
        UpdateCase {
            // The two values `IpAddressType` takes (`IpAddressType`: `dualstack | ipv4`).
            case: "a value out of a CLOSED set re-valued",
            stack: "apigwv2-inplace-closed-set",
            created: json!({
                "Name": "apigwv2-inplace-closed-set-api",
                "ProtocolType": HTTP,
                "IpAddressType": "ipv4",
            }),
            revised: json!({
                "Name": "apigwv2-inplace-closed-set-api",
                "ProtocolType": HTTP,
                "IpAddressType": "dualstack",
            }),
            expected: vec![("ipAddressType", json!("dualstack"))],
        },
        UpdateCase {
            case: "a selection EXPRESSION re-valued",
            stack: "apigwv2-inplace-expression",
            created: json!({
                "Name": "apigwv2-inplace-expression-api",
                "ProtocolType": HTTP,
                "ApiKeySelectionExpression": "$request.header.x-api-key",
            }),
            revised: json!({
                "Name": "apigwv2-inplace-expression-api",
                "ProtocolType": HTTP,
                "ApiKeySelectionExpression": "$context.authorizer.usageIdentifierKey",
            }),
            expected: vec![(
                "apiKeySelectionExpression",
                json!("$context.authorizer.usageIdentifierKey"),
            )],
        },
        UpdateCase {
            // `UpdateApi` takes an `ApiId` together with a new `Name`, so the api API Gateway
            // v2 identifies by that id is RENAMED rather than replaced.
            case: "the NAME the api is called by, re-valued",
            stack: "apigwv2-inplace-renamed",
            created: json!({
                "Name": "apigwv2-inplace-renamed-before",
                "ProtocolType": HTTP,
            }),
            revised: json!({
                "Name": "apigwv2-inplace-renamed-after",
                "ProtocolType": HTTP,
            }),
            expected: vec![("name", json!("apigwv2-inplace-renamed-after"))],
        },
        UpdateCase {
            case: "a NESTED OBJECT property added",
            stack: "apigwv2-inplace-object",
            created: json!({ "Name": "apigwv2-inplace-object-api", "ProtocolType": HTTP }),
            revised: json!({
                "Name": "apigwv2-inplace-object-api",
                "ProtocolType": HTTP,
                "CorsConfiguration": {
                    "AllowOrigins": ["https://orders.example.com"],
                    "AllowMethods": ["GET", "POST"],
                    "MaxAge": 600,
                },
            }),
            expected: vec![
                (
                    "corsConfiguration.allowOrigins",
                    json!(["https://orders.example.com"]),
                ),
                ("corsConfiguration.allowMethods", json!(["GET", "POST"])),
                ("corsConfiguration.maxAge", json!(600)),
            ],
        },
    ]
}

#[tokio::test]
async fn a_changed_api_property_is_applied_to_the_api_the_stack_already_has() {
    for UpdateCase {
        case,
        stack,
        created,
        revised,
        expected,
    } in update_cases()
    {
        let router = router();
        let (code, body) = create_stack(&router, stack, &api_template(created), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to succeed, got {code} {body}"
        );
        let (reported, reason) = settled(&router, case, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
        );
        let api_id = published_api_id(&router, case, stack).await;

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &api_template(revised)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
        );
        let (reported, reason) = settled(&router, case, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the update to complete, got {reported:?} ({reason:?})"
        );

        // The api kept its identity: the handle the stack publishes is the one it published
        // before the update, so nothing was replaced.
        let after = published_api_id(&router, case, stack).await;
        assert_eq!(
            after, api_id,
            "case {case:?}: expected the api to keep its ApiId across an in-place update, got \
             {after:?}"
        );

        // ...and API Gateway v2 really holds the declared configuration.
        let held = held_api(&router, case, &api_id).await;
        for (path, value) in &expected {
            let found = held_member(&held, path);
            assert_eq!(
                found.as_ref(),
                Some(value),
                "case {case:?}: expected API Gateway v2 to hold {path}={value}, got {found:?} \
                 (api {held})"
            );
        }
    }
}

/// One row of the table that changes a property of a resource that lives INSIDE an api.
struct ChildCase {
    case: &'static str,
    stack: &'static str,
    /// The type of the child the row declares beside the api.
    resource_type: &'static str,
    /// The last segment of the collection that child lives in, as its own operations' URIs
    /// spell it — what its read is addressed by.
    collection: &'static str,
    /// The properties the CREATE declares, beside the `ApiId` that addresses the api.
    created: Value,
    /// The properties the revision declares.
    revised: Value,
    /// What API Gateway v2 must report for the child once the revision is deployed, as
    /// `(dotted member path, value)` pairs.
    expected: Vec<(&'static str, Value)>,
}

/// The class the rule covers for the resources inside an api: a revision that states a
/// different value for a property the child's own patch takes a member for.
///
/// The rows differ by TYPE — each of the four the stack provisions whose patch this emulator
/// serves — and, within a type, along the same dimensions the api's own rows do: a string, a
/// number, a list, and a map whose keys are the caller's own data rather than modelled member
/// names (a stage's variables), which travels with its keys exactly as declared.
fn child_cases() -> Vec<ChildCase> {
    vec![
        ChildCase {
            case: "an integration's uri, timeout and description",
            stack: "apigwv2-child-integration",
            resource_type: "AWS::ApiGatewayV2::Integration",
            collection: "integrations",
            created: json!({
                "IntegrationType": "HTTP_PROXY",
                "IntegrationMethod": "GET",
                "IntegrationUri": "https://backend.example.com/one",
                "PayloadFormatVersion": "1.0",
                "Description": "before",
                "TimeoutInMillis": 5000,
            }),
            revised: json!({
                "IntegrationType": "HTTP_PROXY",
                "IntegrationMethod": "POST",
                "IntegrationUri": "https://backend.example.com/two",
                "PayloadFormatVersion": "1.0",
                "Description": "after",
                "TimeoutInMillis": 12000,
            }),
            expected: vec![
                ("integrationUri", json!("https://backend.example.com/two")),
                ("integrationMethod", json!("POST")),
                ("description", json!("after")),
                ("timeoutInMillis", json!(12000)),
            ],
        },
        ChildCase {
            case: "a route's key and operation name",
            stack: "apigwv2-child-route",
            resource_type: "AWS::ApiGatewayV2::Route",
            collection: "routes",
            created: json!({ "RouteKey": "GET /one", "OperationName": "getOne" }),
            revised: json!({ "RouteKey": "GET /two", "OperationName": "getTwo" }),
            expected: vec![
                ("routeKey", json!("GET /two")),
                ("operationName", json!("getTwo")),
            ],
        },
        ChildCase {
            case: "a stage's description and its data-keyed variables",
            stack: "apigwv2-child-stage",
            resource_type: "AWS::ApiGatewayV2::Stage",
            collection: "stages",
            created: json!({
                "StageName": "beta",
                "Description": "before",
                "StageVariables": { "flavour": "vanilla" },
            }),
            revised: json!({
                "StageName": "beta",
                "Description": "after",
                "StageVariables": { "flavour": "chocolate", "scoops": "2" },
            }),
            expected: vec![
                ("description", json!("after")),
                ("stageVariables.flavour", json!("chocolate")),
                ("stageVariables.scoops", json!("2")),
            ],
        },
        ChildCase {
            case: "an authorizer's name, identity source and result ttl",
            stack: "apigwv2-child-authorizer",
            resource_type: "AWS::ApiGatewayV2::Authorizer",
            collection: "authorizers",
            created: json!({
                "Name": "before-authorizer",
                "AuthorizerType": "REQUEST",
                "AuthorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:000000000000:function:authorize/invocations",
                "AuthorizerPayloadFormatVersion": "2.0",
                "IdentitySource": ["$request.header.Authorization"],
            }),
            revised: json!({
                "Name": "after-authorizer",
                "AuthorizerType": "REQUEST",
                "AuthorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:000000000000:function:authorize/invocations",
                "AuthorizerPayloadFormatVersion": "2.0",
                "IdentitySource": ["$request.header.X-Token", "$request.querystring.token"],
                "AuthorizerResultTtlInSeconds": 300,
            }),
            expected: vec![
                ("name", json!("after-authorizer")),
                (
                    "identitySource",
                    json!(["$request.header.X-Token", "$request.querystring.token"]),
                ),
                ("authorizerResultTtlInSeconds", json!(300)),
            ],
        },
    ]
}

#[tokio::test]
async fn a_changed_property_of_a_resource_inside_an_api_is_applied_to_that_resource() {
    for ChildCase {
        case,
        stack,
        resource_type,
        collection,
        created,
        revised,
        expected,
    } in child_cases()
    {
        let router = router();
        let template = |properties: &Value| {
            let mut declared = properties.clone();
            declared["ApiId"] = json!({ "Ref": "Api" });
            json!({
                "Resources": {
                    "Api": {
                        "Type": "AWS::ApiGatewayV2::Api",
                        "Properties": {
                            "Name": format!("{stack}-api"),
                            "ProtocolType": HTTP,
                        },
                    },
                    "Child": { "Type": resource_type, "Properties": declared },
                }
            })
            .to_string()
        };

        let (code, body) = create_stack(&router, stack, &template(&created), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to succeed, got {code} {body}"
        );
        let (reported, reason) = settled(&router, case, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
        );
        let api_id = published_api_id(&router, case, stack).await;
        let child_id = published_id(&router, case, stack, "Child").await;

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &template(&revised)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
        );
        let (reported, reason) = settled(&router, case, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the update to complete, got {reported:?} ({reason:?})"
        );

        // The child kept BOTH halves of its identity: the api it lives in, and the
        // identifier that api knows it by — so nothing was created a second time.
        let api_after = published_api_id(&router, case, stack).await;
        assert_eq!(
            api_after, api_id,
            "case {case:?}: expected the api to keep its ApiId, got {api_after:?}"
        );
        let after = published_id(&router, case, stack, "Child").await;
        assert_eq!(
            after, child_id,
            "case {case:?}: expected the child to keep the identity it was created with, got \
             {after:?}"
        );

        // ...and API Gateway v2 really holds the declared configuration, read back through
        // that resource's own read.
        let (code, held) = apigateway(
            &router,
            "GET",
            &format!("/v2/apis/{api_id}/{collection}/{child_id}"),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected the child's own read to answer 200, got {code} with {held}"
        );
        for (path, value) in &expected {
            let found = held_member(&held, path);
            assert_eq!(
                found.as_ref(),
                Some(value),
                "case {case:?}: expected API Gateway v2 to hold {path}={value}, got {found:?} \
                 (child {held})"
            );
        }
    }
}

#[tokio::test]
async fn an_in_place_update_leaves_the_resources_inside_the_api_alone() {
    // The api's own property changes while the integration and the route inside it are
    // re-declared exactly as they were: each of them names the api it lives in, and that api
    // keeps its identity, so each is still the same resource — nothing is created again and
    // nothing in API Gateway v2 is asked for a second time.
    let case = "an api with children, described differently";
    let stack = "apigwv2-inplace-children";
    let router = router();
    let template = |description: &str| {
        json!({
            "Resources": {
                "Api": {
                    "Type": "AWS::ApiGatewayV2::Api",
                    "Properties": {
                        "Name": "apigwv2-inplace-children-api",
                        "ProtocolType": HTTP,
                        "Description": description,
                    },
                },
                "Integration": {
                    "Type": "AWS::ApiGatewayV2::Integration",
                    "Properties": {
                        "ApiId": { "Ref": "Api" },
                        "IntegrationType": "HTTP_PROXY",
                        "IntegrationMethod": "GET",
                        "IntegrationUri": "https://backend.example.com/hello",
                        "PayloadFormatVersion": "1.0",
                    },
                },
                "Route": {
                    "Type": "AWS::ApiGatewayV2::Route",
                    "Properties": {
                        "ApiId": { "Ref": "Api" },
                        "RouteKey": "GET /hello",
                        "Target": {
                            "Fn::Join": ["/", ["integrations", { "Ref": "Integration" }]]
                        },
                    },
                },
            }
        })
        .to_string()
    };

    let (code, body) = create_stack(&router, stack, &template("before"), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
    );
    let api_before = published_api_id(&router, case, stack).await;
    let integration_before = published_id(&router, case, stack, "Integration").await;
    let route_before = published_id(&router, case, stack, "Route").await;

    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template("after")),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the update to complete, got {reported:?} ({reason:?})"
    );

    for (logical_id, before) in [
        ("Api", &api_before),
        ("Integration", &integration_before),
        ("Route", &route_before),
    ] {
        let after = published_id(&router, case, stack, logical_id).await;
        assert_eq!(
            &after, before,
            "case {case:?}: expected {logical_id} to keep the identity it was created with, \
             got {after:?}"
        );
    }
    let held = held_api(&router, case, &api_before).await;
    assert_eq!(
        held_member(&held, "description"),
        Some(json!("after")),
        "case {case:?}: expected API Gateway v2 to hold the revised description, got {held}"
    );
    // The route the api holds is still the ONE the stack created, under the key it was
    // created with: a second create would have been refused, and a route created afresh
    // would carry an id of its own.
    let (code, routes) = apigateway(&router, "GET", &format!("/v2/apis/{api_before}/routes")).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetRoutes to answer 200, got {code} with {routes}"
    );
    let items = routes
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let keys: Vec<Option<&str>> = items
        .iter()
        .map(|route| route.get("routeKey").and_then(Value::as_str))
        .collect();
    assert_eq!(
        keys,
        vec![Some("GET /hello")],
        "case {case:?}: expected the api to hold exactly the route the stack created, got \
         {routes}"
    );
}

#[tokio::test]
async fn a_replaced_api_takes_the_resources_that_live_inside_it_with_it() {
    // An integration, a route and a stage exist INSIDE the api their `ApiId` names, and API
    // Gateway v2 publishes no call that moves one to another api — so when a revision
    // replaces the api, each of them is created afresh inside the api that replaced it and
    // the ones in the api it replaced are taken away with that api.
    let case = "an api with children, re-protocolled";
    let stack = "apigwv2-replaced-children";
    let router = router();
    let template = |protocol: &str, description: &str| {
        json!({
            "Resources": {
                "Api": {
                    "Type": "AWS::ApiGatewayV2::Api",
                    "Properties": {
                        "Name": "apigwv2-replaced-children-api",
                        "ProtocolType": protocol,
                        "Description": description,
                        "RouteSelectionExpression": "$request.body.action",
                    },
                },
                "Integration": {
                    "Type": "AWS::ApiGatewayV2::Integration",
                    "Properties": {
                        "ApiId": { "Ref": "Api" },
                        "IntegrationType": "HTTP_PROXY",
                        "IntegrationMethod": "GET",
                        "IntegrationUri": "https://backend.example.com/hello",
                        "PayloadFormatVersion": "1.0",
                    },
                },
                "Route": {
                    "Type": "AWS::ApiGatewayV2::Route",
                    "Properties": {
                        "ApiId": { "Ref": "Api" },
                        "RouteKey": "$default",
                        "Target": {
                            "Fn::Join": ["/", ["integrations", { "Ref": "Integration" }]]
                        },
                    },
                },
            }
        })
        .to_string()
    };

    let (code, body) = create_stack(&router, stack, &template(HTTP, "before"), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
    );
    let api_before = published_api_id(&router, case, stack).await;
    let route_before = published_id(&router, case, stack, "Route").await;

    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template(WEBSOCKET, "after")),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the replacing update to complete, got {reported:?} ({reason:?})"
    );

    let api_after = published_api_id(&router, case, stack).await;
    assert_ne!(
        api_after, api_before,
        "case {case:?}: expected the replacement api to publish an ApiId of its own, got \
         {api_after:?}"
    );
    let route_after = published_id(&router, case, stack, "Route").await;
    assert_ne!(
        route_after, route_before,
        "case {case:?}: expected the route to be created inside the replacement api, got \
         {route_after:?}"
    );
    // ...and the route the stack now publishes is a route of the api it now publishes.
    let (code, document) = apigateway(
        &router,
        "GET",
        &format!("/v2/apis/{api_after}/routes/{route_after}"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetRoute({api_after}, {route_after}) to answer 200, got \
         {code} with {document}"
    );
}

#[tokio::test]
async fn a_changed_protocol_type_replaces_the_api() {
    // `ProtocolType` is a member of `CreateApi` and of no member of `UpdateApi`, so a
    // revision that states another protocol asks for a DIFFERENT api: the stack creates one
    // under the id API Gateway v2 mints for it, publishes that id, and takes the api it
    // replaced away — which is what makes the old id answer for nothing.
    let case = "a protocol type re-valued";
    let stack = "apigwv2-replaced-protocol";
    let router = router();
    let created = json!({
        "Name": "apigwv2-replaced-protocol-api",
        "ProtocolType": HTTP,
        "Description": "the http api",
    });
    let (code, body) = create_stack(&router, stack, &api_template(created), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
    );
    let before = published_api_id(&router, case, stack).await;
    let held = held_api(&router, case, &before).await;
    assert_eq!(
        held_member(&held, "protocolType"),
        Some(json!(HTTP)),
        "case {case:?}: expected the created api to be an {HTTP} api, got {held}"
    );

    // `RouteSelectionExpression` "is required for WebSocket APIs", so the revision that
    // re-protocols the api states one.
    let revised = json!({
        "Name": "apigwv2-replaced-protocol-api",
        "ProtocolType": WEBSOCKET,
        "Description": "the websocket api",
        "RouteSelectionExpression": "$request.body.action",
    });
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &api_template(revised)),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
    );
    let (reported_status, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported_status,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the replacing update to complete, got {reported_status:?} \
         ({reason:?})"
    );

    let after = published_api_id(&router, case, stack).await;
    assert_ne!(
        after, before,
        "case {case:?}: expected the replacement to publish an ApiId of its own, got {after:?}"
    );
    let held = held_api(&router, case, &after).await;
    assert_eq!(
        held_member(&held, "protocolType"),
        Some(json!(WEBSOCKET)),
        "case {case:?}: expected the replacement to be a {WEBSOCKET} api, got {held}"
    );
    assert_eq!(
        held_member(&held, "description"),
        Some(json!("the websocket api")),
        "case {case:?}: expected the replacement to carry the revised description, got {held}"
    );

    // The api it replaced is taken away in the update's cleanup, which runs after the
    // replacement is in force — so the old id is polled until that service no longer holds
    // it rather than read once.
    let mut last = 200;
    for _ in 0..POLL_ATTEMPTS {
        let (code, _) = get_api(&router, &before).await;
        last = code;
        if code == 404 {
            break;
        }
        wait_for_the_transition().await;
    }
    assert_eq!(
        last, 404,
        "case {case:?}: expected the replaced api {before:?} to be gone from API Gateway v2, \
         got {last}"
    );

    // ...and the stack ANNOUNCED the replacement: the resource's own events are the update's,
    // and the one that OPENS them says why a new physical resource was created at all, which
    // is what tells a client polling the stack that the api it had was replaced rather than
    // created for the first time. Read newest first, so the three run in reverse: the
    // announcement, the create's own initiated event, then the status it ended at.
    let events = resource_events(&router, case, stack, "Api").await;
    let announced = [
        (
            resource_status::UPDATE_COMPLETE.to_string(),
            String::new(),
        ),
        (
            resource_status::UPDATE_IN_PROGRESS.to_string(),
            RESOURCE_CREATION_INITIATED.to_string(),
        ),
        (
            resource_status::UPDATE_IN_PROGRESS.to_string(),
            RESOURCE_REPLACEMENT_IN_PROGRESS.to_string(),
        ),
    ];
    assert!(
        events.windows(announced.len()).any(|window| window == announced),
        "case {case:?}: expected the replacement to be announced, initiated and completed as \
         {announced:?}, got {events:?}"
    );
    // The old physical resource declares no `UpdateReplacePolicy`, so it is DELETED, which is
    // that attribute's documented default.
    assert!(
        events
            .iter()
            .any(|(status, _)| status == resource_status::DELETE_COMPLETE),
        "case {case:?}: expected the replaced api's deletion to be reported, got {events:?}"
    );
}

#[tokio::test]
async fn update_replace_policy_retain_keeps_the_api_the_update_replaced() {
    // "With the UpdateReplacePolicy attribute you can specify how CloudFormation handles the
    // old physical resource": under `Retain` the api the update replaced is KEPT — nothing is
    // asked of API Gateway v2 about it, so a client still reads it back — and the one thing
    // the stack records about it is that its deletion was SKIPPED. The same declaration as
    // the row above but for that attribute, which is the one thing this case turns on.
    let case = "a replaced api retained";
    let stack = "apigwv2-retained-protocol";
    let router = router();
    let template = |protocol: &str, expression: Option<&str>| {
        let mut properties = json!({
            "Name": "apigwv2-retained-protocol-api",
            "ProtocolType": protocol,
        });
        if let Some(expression) = expression {
            properties["RouteSelectionExpression"] = json!(expression);
        }
        json!({
            "Resources": {
                "Api": {
                    "Type": "AWS::ApiGatewayV2::Api",
                    "UpdateReplacePolicy": "Retain",
                    "Properties": properties,
                }
            }
        })
        .to_string()
    };

    let (code, body) = create_stack(&router, stack, &template(HTTP, None), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
    );
    let before = published_api_id(&router, case, stack).await;

    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member(
                "TemplateBody",
                &template(WEBSOCKET, Some("$request.body.action")),
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
    );
    let (reported, reason) = settled(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the replacing update to complete, got {reported:?} ({reason:?})"
    );
    let after = published_api_id(&router, case, stack).await;
    assert_ne!(
        after, before,
        "case {case:?}: expected the replacement to publish an ApiId of its own, got {after:?}"
    );

    let events = resource_events(&router, case, stack, "Api").await;
    assert!(
        events
            .iter()
            .any(|(status, _)| status == resource_status::DELETE_SKIPPED),
        "case {case:?}: expected the retained api's deletion to be reported as skipped, got \
         {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(status, _)| status == resource_status::DELETE_IN_PROGRESS
                || status == resource_status::DELETE_COMPLETE),
        "case {case:?}: expected nothing to be deleted for a retained api, got {events:?}"
    );
    // ...and the retained api is still there to be read, under the id it always had.
    let (code, document) = get_api(&router, &before).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected the retained api {before:?} to still be held by API Gateway \
         v2, got {code} with {document}"
    );
    assert_eq!(
        held_member(&document, "protocolType"),
        Some(json!(HTTP)),
        "case {case:?}: expected the retained api to be the one the create made, got {document}"
    );
}
