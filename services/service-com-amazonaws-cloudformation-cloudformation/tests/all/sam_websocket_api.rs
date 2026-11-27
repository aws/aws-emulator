// SPDX-License-Identifier: CC0-1.0

//! What a SAM template's `AWS::Serverless::WebSocketApi` becomes, end to end: the
//! CloudFormation calls over its awsQuery wire, the api and its children read back over API
//! Gateway v2's own `restJson1` wire.
//!
//! RULE: a declaration of a type the `AWS::Serverless-2016-10-31` macro defines is EXPANDED by
//! that macro into the concrete resources it stands for, and those resources are created in the
//! service that owns them — so a WEBSOCKET api declaration becomes a WEBSOCKET
//! `AWS::ApiGatewayV2::Api` with the stage it is deployed to and one integration and route per
//! route key it declares, the stack's `Ref` is the `ApiId` API Gateway v2 minted and lists, and
//! a `DeleteStack` removes it from that service. A declaration that states none of a property
//! the resource it becomes cannot be created without fails the MACRO itself: the change set
//! ends `FAILED` carrying the validator's own `Missing required property '<name>'.` sentence,
//! with nothing planned, nothing deployed and nothing created in the owning service.
//!
//! The rows differ along what the rule generalises over: which route keys the declaration
//! states and how many, whether a route key names a function at all, whether two keys share
//! the alphanumeric characters a logical id is built from, whether it names a stage of its own
//! or takes the default, whether it states an authorization type for the connection, whether it
//! names the api or leaves it to be named, and which route-selection expression it routes
//! messages by — and, for the refusal,
//! whether the required property is absent, written as `null`, or stated (the positive control).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::apigateway;
use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{call, create_stack, member, router_with_engine, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The `ProtocolType` such an api is created with (`ProtocolType`: `HTTP | WEBSOCKET`).
const WEBSOCKET: &str = "WEBSOCKET";

/// The route key of the request that opens a connection, and the catch-all key a message with
/// no route of its own takes: the two keys API Gateway v2 reserves on a WEBSOCKET api.
const CONNECT: &str = "$connect";
const DEFAULT_ROUTE: &str = "$default";

/// The stage name an api that declares none is deployed to: the `$default` stage
/// (`Stage$ApiGatewayManaged`: "the $default stage").
const DEFAULT_STAGE: &str = "$default";

/// The authorization types a route of a WEBSOCKET api can carry, of the values
/// `Route$AuthorizationType` publishes ("NONE for open access, AWS_IAM for using AWS IAM
/// permissions"): the two this suite spans.
const AWS_IAM: &str = "AWS_IAM";
const NONE: &str = "NONE";

/// The capabilities a template carrying a macro and an IAM role is submitted with.
fn capabilities() -> Vec<(String, String)> {
    vec![
        member("Capabilities.member.1", "CAPABILITY_IAM"),
        member("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
    ]
}

/// The function every row's routes are backed by, declared inline so nothing has to be staged
/// in Amazon S3 for the stack to deploy.
fn function() -> Value {
    json!({
        "Type": "AWS::Serverless::Function",
        "Properties": {
            "Handler": "index.handler",
            "Runtime": "python3.12",
            "InlineCode": "def handler(event, context):\n    return {}\n",
        },
    })
}

/// One template: the SAM transform, the function and one `AWS::Serverless::WebSocketApi` whose
/// properties the row states.
fn template(properties: Value) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Fn": function(),
            "WsApi": { "Type": "AWS::Serverless::WebSocketApi", "Properties": properties },
        },
    })
    .to_string()
}

/// The `Routes` map of a declaration, one entry per route key, each backed by the template's
/// own function.
fn routes(keys: &[&str]) -> Value {
    let mut declared = serde_json::Map::new();
    for key in keys {
        declared.insert(
            (*key).to_string(),
            json!({ "FunctionArn": { "Fn::GetAtt": "Fn.Arn" } }),
        );
    }
    Value::Object(declared)
}

/// One row of the deployed table: a declaration and what API Gateway v2 must hold once the
/// stack has deployed it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The properties the `AWS::Serverless::WebSocketApi` declares.
    declared: Value,
    /// The `Name` the api must carry in API Gateway v2.
    name: &'static str,
    /// The `RouteSelectionExpression` it must carry.
    selection: &'static str,
    /// The stage name it must be deployed to.
    stage: &'static str,
    /// The route keys it must hold: each one's key, the `AuthorizationType` it must carry, and
    /// whether the entry named a function, which decides whether the route is wired to an
    /// integration of its own.
    routes: Vec<(&'static str, &'static str, bool)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the connect and default routes, an IAM-authorized connection and the \
                   default stage",
            stack: "sam-wsapi-iam",
            declared: json!({
                "Name": "sam-wsapi-iam-api",
                "RouteSelectionExpression": "$request.body.action",
                "Auth": { "AuthType": AWS_IAM },
                "Routes": routes(&[CONNECT, DEFAULT_ROUTE]),
            }),
            name: "sam-wsapi-iam-api",
            selection: "$request.body.action",
            stage: DEFAULT_STAGE,
            routes: vec![(CONNECT, AWS_IAM, true), (DEFAULT_ROUTE, NONE, true)],
        },
        Row {
            case: "one route, no authorization stated and a stage of the declaration's own",
            stack: "sam-wsapi-open",
            declared: json!({
                "Name": "sam-wsapi-open-api",
                "RouteSelectionExpression": "$request.body.meta.kind",
                "StageName": "dev",
                "Routes": routes(&[CONNECT]),
            }),
            name: "sam-wsapi-open-api",
            selection: "$request.body.meta.kind",
            stage: "dev",
            routes: vec![(CONNECT, NONE, true)],
        },
        Row {
            case: "a route key of the client's own beside the two reserved ones, on an api \
                   the declaration does not name",
            stack: "sam-wsapi-unnamed",
            declared: json!({
                "RouteSelectionExpression": "$request.body.action",
                "Auth": { "AuthType": AWS_IAM },
                "Routes": routes(&[CONNECT, DEFAULT_ROUTE, "sendMessage"]),
            }),
            // A declaration that names no api is named by its own logical id, the only name
            // it carries.
            name: "WsApi",
            selection: "$request.body.action",
            stage: DEFAULT_STAGE,
            routes: vec![
                (CONNECT, AWS_IAM, true),
                (DEFAULT_ROUTE, NONE, true),
                ("sendMessage", NONE, true),
            ],
        },
        Row {
            // A route key the declaration states with no function behind it is still a route
            // of the api, and two keys whose alphanumeric characters are the same are two
            // routes: neither may displace the other.
            case: "a route key with no function behind it, beside one whose alphanumeric \
                   characters are the same as another key's",
            stack: "sam-wsapi-mixed",
            declared: json!({
                "Name": "sam-wsapi-mixed-api",
                "RouteSelectionExpression": "$request.body.action",
                "Routes": {
                    CONNECT: { "FunctionArn": { "Fn::GetAtt": "Fn.Arn" } },
                    "sendMessage": {},
                    "send-Message": { "FunctionArn": { "Fn::GetAtt": "Fn.Arn" } },
                },
            }),
            name: "sam-wsapi-mixed-api",
            selection: "$request.body.action",
            stage: DEFAULT_STAGE,
            routes: vec![
                (CONNECT, NONE, true),
                ("sendMessage", NONE, false),
                ("send-Message", NONE, true),
            ],
        },
    ]
}

#[tokio::test]
async fn a_declared_websocket_api_is_created_in_api_gateway_and_removed_with_its_stack() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = router_with_engine();
        let (code, body) = create_stack(
            &router,
            row.stack,
            &template(row.declared.clone()),
            &capabilities(),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let described = describe_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the stack to deploy the expanded api, got {described}"
        );

        // `Ref` of the declaration is the api's own id, which only API Gateway v2 mints: the
        // stack publishes what that service assigned, and that service lists the api under it.
        let api_id = published_id(&router, case, row.stack, "WsApi").await;
        let listed = listed_apis(&router, case).await;
        let held: Vec<&Value> = listed
            .iter()
            .filter(|api| api.get("apiId").and_then(Value::as_str) == Some(api_id.as_str()))
            .collect();
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected GetApis to list exactly the api the stack published \
             ({api_id}), got {listed:?}"
        );
        let api = held[0];
        for (member, expected) in [
            ("name", row.name),
            ("protocolType", WEBSOCKET),
            ("routeSelectionExpression", row.selection),
        ] {
            let found = api.get(member).and_then(Value::as_str);
            assert_eq!(
                found,
                Some(expected),
                "case {case:?}: expected the listed api's {member} to be {expected:?}, got \
                 {found:?} ({api})"
            );
        }

        // The stage the api is deployed to, which is what makes the routes answerable.
        let stages = child_items(&router, case, &api_id, "stages", "GetStages").await;
        let names: Vec<&str> = stages
            .iter()
            .filter_map(|stage| stage.get("stageName").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec![row.stage],
            "case {case:?}: expected the api to carry the stage {:?}, got {names:?}",
            row.stage
        );

        // One route per declared route key, each wired to an integration of this api that
        // addresses the function the declaration named.
        let integrations =
            child_items(&router, case, &api_id, "integrations", "GetIntegrations").await;
        let held_routes = child_items(&router, case, &api_id, "routes", "GetRoutes").await;
        let mut keys: Vec<&str> = held_routes
            .iter()
            .filter_map(|route| route.get("routeKey").and_then(Value::as_str))
            .collect();
        keys.sort_unstable();
        let mut expected: Vec<&str> = row.routes.iter().map(|(key, _, _)| *key).collect();
        expected.sort_unstable();
        assert_eq!(
            keys, expected,
            "case {case:?}: expected the api to hold the declared routes {expected:?}, got \
             {keys:?} ({held_routes:?})"
        );
        for (key, authorization, backed) in &row.routes {
            let route = held_routes
                .iter()
                .find(|route| route.get("routeKey").and_then(Value::as_str) == Some(key))
                .unwrap_or_else(|| panic!("case {case:?}: no route {key:?} in {held_routes:?}"));
            let found = route.get("authorizationType").and_then(Value::as_str);
            assert_eq!(
                found,
                Some(*authorization),
                "case {case:?}: expected route {key:?} to carry authorizationType \
                 {authorization:?}, got {found:?} ({route})"
            );
            let target = route.get("target").and_then(Value::as_str);
            if !backed {
                assert_eq!(
                    target, None,
                    "case {case:?}: expected route {key:?}, whose entry named no function, to \
                     be wired to no integration, got {target:?}"
                );
                continue;
            }
            let target = target.unwrap_or_default();
            let integration_id = target.strip_prefix("integrations/").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected route {key:?} to name its integration as \
                     integrations/<IntegrationId>, got {target:?}"
                )
            });
            let integration = integrations
                .iter()
                .find(|held| {
                    held.get("integrationId").and_then(Value::as_str) == Some(integration_id)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: route {key:?} names the integration {integration_id:?}, \
                         which the api does not hold: {integrations:?}"
                    )
                });
            let uri = integration
                .get("integrationUri")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                uri.contains(":function:"),
                "case {case:?}: expected route {key:?}'s integration to address the declared \
                 function, got integrationUri {uri:?}"
            );
            let kind = integration.get("integrationType").and_then(Value::as_str);
            assert_eq!(
                kind,
                Some("AWS_PROXY"),
                "case {case:?}: expected route {key:?}'s integration to proxy to the function, \
                 got {kind:?}"
            );
        }

        // The api is the stack's: deleting the stack removes it from API Gateway v2.
        let (code, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DeleteStack to be accepted, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let listed = listed_apis(&router, case).await;
        assert!(
            listed
                .iter()
                .all(|api| api.get("apiId").and_then(Value::as_str) != Some(api_id.as_str())),
            "case {case:?}: expected the deleted stack's api ({api_id}) to be gone from \
             GetApis, got {listed:?}"
        );
    }
}

/// One row of the refusal table: a declaration missing a required property, and the sentence
/// the macro fails with — `None` for the positive control, which the macro processes.
struct Refused {
    case: &'static str,
    stack: &'static str,
    declared: Value,
    reason: Option<&'static str>,
}

/// The diagnostic the macro's own failure is reported in, around one cause.
fn transform_failure(cause: &str) -> String {
    format!(
        "Transform AWS::Serverless-2016-10-31 failed with: Invalid Serverless Application \
         Specification document. Number of errors found: 1. {cause}"
    )
}

fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a declaration that states no route selection expression at all",
            stack: "sam-wsapi-no-selection",
            declared: json!({
                "Name": "sam-wsapi-no-selection-api",
                "Routes": routes(&[CONNECT, DEFAULT_ROUTE]),
            }),
            reason: Some(
                "Resource with id [WsApi] is invalid. Missing required property \
                 'RouteSelectionExpression'.",
            ),
        },
        Refused {
            case: "a declaration whose route selection expression is written as null, which \
                   states nothing",
            stack: "sam-wsapi-null-selection",
            declared: json!({
                "Name": "sam-wsapi-null-selection-api",
                "RouteSelectionExpression": Value::Null,
                "Routes": routes(&[CONNECT]),
            }),
            reason: Some(
                "Resource with id [WsApi] is invalid. Missing required property \
                 'RouteSelectionExpression'.",
            ),
        },
        Refused {
            // The other side of the rule: the same declaration with the property stated is a
            // document the macro processes.
            case: "a declaration that states it",
            stack: "sam-wsapi-stated-selection",
            declared: json!({
                "Name": "sam-wsapi-stated-selection-api",
                "RouteSelectionExpression": "$request.body.action",
                "Routes": routes(&[CONNECT]),
            }),
            reason: None,
        },
    ]
}

#[tokio::test]
async fn a_declaration_missing_a_required_property_fails_the_transform_itself() {
    for row in refused_rows() {
        let case = row.case;
        let (router, engine) = router_with_engine();
        let mut members = vec![
            member("StackName", row.stack),
            member("ChangeSetName", "plan"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &template(row.declared.clone())),
        ];
        members.extend(capabilities());
        let (code, body) = call(&router, "CreateChangeSet", &members).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateChangeSet to be accepted, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, described) = call(
            &router,
            "DescribeChangeSet",
            &[
                member("ChangeSetName", "plan"),
                member("StackName", row.stack),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected the change set to be describable, got {code} {described}"
        );
        let reported = (
            xml_text(&described, "Status").unwrap_or_default(),
            xml_text(&described, "StatusReason").unwrap_or_default(),
        );
        match row.reason {
            Some(cause) => {
                let expected = ("FAILED".to_string(), transform_failure(cause));
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the macro's own failure {expected:?}, got \
                     {reported:?}"
                );
                // Nothing was planned, so nothing reached API Gateway v2 either.
                let listed = listed_apis(&router, case).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: expected a refused transform to create no api, got {listed:?}"
                );
            }
            None => {
                assert_eq!(
                    reported.0, "CREATE_COMPLETE",
                    "case {case:?}: expected the change set to be planned, got {reported:?}"
                );
            }
        }
    }
}

/// `DescribeStacks` for one stack, once the store has settled.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {code} {body}"
    );
    body
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
        "case {case:?}: expected DescribeStackResource({logical_id}) to answer, got {code} {body}"
    );
    xml_text(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("case {case:?}: no PhysicalResourceId in {body}"))
}

/// `GetApis` (`GET /v2/apis`): the apis the account holds, as that operation's `items` member
/// carries them.
async fn listed_apis(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (code, document) = apigateway(router, "GET", "/v2/apis").await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetApis to answer 200, got {code} with {document}"
    );
    document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The items one collection of an api carries (`GET /v2/apis/{ApiId}/<collection>`).
async fn child_items(
    router: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    collection: &str,
    operation: &str,
) -> Vec<Value> {
    let (code, document) =
        apigateway(router, "GET", &format!("/v2/apis/{api_id}/{collection}")).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected {operation} to answer 200, got {code} with {document}"
    );
    document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
