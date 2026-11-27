// SPDX-License-Identifier: CC0-1.0

//! A stack delete asks the owning services in REVERSE DEPENDENCY order, so every resource is
//! deleted before the resources it depends on.
//!
//! The rule under test: **CloudFormation deletes a stack's resources in reverse dependency
//! order — a resource that depends on another, through `DependsOn` or through an intrinsic
//! reference to it, is deleted BEFORE the resource it depends on — so a delete the owning
//! service guards while something of its own still references the resource is never the delete
//! CloudFormation makes first.** AWS AppSync is where that is observable: it refuses
//! `DeleteDataSource` while a resolver or a pipeline function of the API still resolves against
//! the data source, and `DeleteFunction` while a pipeline resolver still invokes the function.
//! Two crates have to agree for the rule to hold (CloudFormation makes the calls, AppSync
//! guards them), so the suite lives here rather than in either one.
//!
//! The rows differ along what the rule generalises over: where the dependent is DECLARED
//! relative to its dependency (after it, and before it, so the declared order alone cannot be
//! the order the delete follows), whether the edge comes from `DependsOn` or only from an
//! intrinsic reference (`Fn::GetAtt` on the dependency's own attribute), and how long the chain
//! is (a pipeline resolver over a function over a data source, which has to come apart in that
//! order). One test of its own carries the fourth: the resolver the stack holds at delete time
//! is a REPLACEMENT an `UpdateReplacePolicy: Retain` update left it with.
//!
//! No expected value is read out of the answer being checked: each is what the row's own
//! template declares, or what AWS AppSync answers for the resource.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{element, form_encode, opt_element};

/// How long one poll waits before the next, and how many are made. The suite waits for the
/// STATUS rather than for a span, with a deadline generous enough for a loaded host.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 600;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other suite
/// here; the in-process call CloudFormation makes signs its request either way (the credential
/// scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one AWS AppSync operation over the `restJson1` wire its model declares, on the URI that
/// operation's own `@http` binding states.
async fn appsync(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{region}.amazonaws.com{path}"))
        .header("host", format!("appsync.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("appsync", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Deploy `template` under `stack` and wait for `CREATE_COMPLETE`, answering the stack's own
/// `StackId` — which is what every later read addresses it by, since a stack that has been
/// deleted is readable by id and not by name.
async fn create_stack(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    template: &str,
    case: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        region,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let stack_id = element(&body, "StackId");
    wait_for_stack_status(router, region, &stack_id, "CREATE_COMPLETE", case).await;
    stack_id
}

/// Wait for `stack` to report `wanted`, looking again until it does or the deadline passes.
///
/// A stack that reaches another TERMINAL status fails the suite at once, quoting its reason and
/// the events that carry the resource-level failures: the reason a resource could not be
/// deleted is in them.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    wanted: &str,
    case: &str,
) {
    let mut body = String::new();
    for _ in 0..POLLS {
        let (status, reported) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks for {stack:?}, got {status} \
             with body {reported}"
        );
        body = reported;
        let reported_status = opt_element(&body, "StackStatus").unwrap_or_default();
        if reported_status == wanted {
            return;
        }
        if reported_status.ends_with("_FAILED") || reported_status.starts_with("ROLLBACK") {
            let failures = failed_events(router, region, stack).await;
            panic!(
                "case {case:?}: expected {wanted:?}, got {reported_status:?} (reason {:?}); \
                 failed events: {failures:?}",
                opt_element(&body, "StackStatusReason").unwrap_or_default()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: stack {stack:?} never reached {wanted:?}; last answer was {body}");
}

/// The `(logical id, status, reason)` of every stack event that carries a failed status — what
/// a failure assertion quotes, so the resource and the owning service's own refusal are both in
/// the message.
async fn failed_events(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> Vec<(String, String, String)> {
    let (_, body) =
        cloudformation(router, region, "DescribeStackEvents", &[("StackName", stack)]).await;
    super::query_wire::blocks_of(&body, "member")
        .into_iter()
        .filter_map(|event| {
            let status = opt_element(event, "ResourceStatus").unwrap_or_default();
            status.ends_with("_FAILED").then(|| {
                (
                    opt_element(event, "LogicalResourceId").unwrap_or_default(),
                    status,
                    opt_element(event, "ResourceStatusReason").unwrap_or_default(),
                )
            })
        })
        .collect()
}

/// The id of the API the stack's `Api` resource stands for, read from the stack's own resource
/// record: `Ref` on an `AWS::AppSync::GraphQLApi` is the API's ARN, whose last segment is the
/// id every AppSync call on it binds.
async fn api_id_of(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    case: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        region,
        "DescribeStackResource",
        &[("StackName", stack), ("LogicalResourceId", "Api")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResource, got {status} with body \
         {body}"
    );
    let arn = element(&body, "PhysicalResourceId");
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected an API ARN, got {arn:?}"))
        .to_string()
}

/// Delete `stack`, wait for `DELETE_COMPLETE`, and hold AppSync to answering `NotFoundException`
/// for the API the stack owned — which it can only reach by deleting each resource before the
/// one it depends on.
async fn delete_stack_and_expect_the_api_gone(
    router: &MultiServiceRouter,
    region: &str,
    stack_id: &str,
    api_id: &str,
    case: &str,
) {
    let (status, body) =
        cloudformation(router, region, "DeleteStack", &[("StackName", stack_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    wait_for_stack_status(router, region, stack_id, "DELETE_COMPLETE", case).await;
    let (status, read) = appsync(router, region, "GET", &format!("/v1/apis/{api_id}")).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 from GetGraphqlApi once the stack deleted the API, got \
         {status} with {read}"
    );
    assert_eq!(
        read.get("message").and_then(Value::as_str),
        Some(API_NOT_FOUND),
        "case {case:?}: expected the API's own not-found message, got {read}"
    );
}

/// The message AWS AppSync answers a read of an API it does not hold with.
const API_NOT_FOUND: &str = "API not found.";

/// The schema every row installs: two fields on `Query`, so a resolver can be moved from one to
/// the other.
const SCHEMA: &str = "type Query { first(msg: String): String second(msg: String): String } \
                      schema { query: Query }";

/// A VTL request mapping template a resolver and a pipeline function carry.
const REQUEST_TEMPLATE: &str = r#"{\"version\":\"2017-02-28\"}"#;

/// Its response mapping template.
const RESPONSE_TEMPLATE: &str = "$util.toJson($ctx.result)";

/// The `APPSYNC_JS` code the replaced resolver carries, and the changed code an IN-PLACE update
/// of it states (`Code` is a member of `UpdateResolver`, so a changed one is applied in place).
const CODE: &str = "export function request(ctx) { return {}; } \
                    export function response(ctx) { return ctx.result; }";
/// See [`CODE`].
const CHANGED_CODE: &str = "export function request(ctx) { return {payload: null}; } \
                            export function response(ctx) { return ctx.result; }";

/// One row of the table: a template whose resources hold a dependency edge one of AppSync's
/// in-use guards makes observable.
struct Row {
    case: &'static str,
    region: &'static str,
    stack: &'static str,
    /// The template body, built by [`template_of`] from the row's own shape.
    shape: Shape,
}

/// Which template a row deploys.
enum Shape {
    /// A resolver that `DependsOn` the data source it names, declared AFTER it.
    DependsOnAfter,
    /// A resolver whose only edge to the data source is `Fn::GetAtt` on its `Name`, declared
    /// BEFORE it.
    ReferenceBefore,
    /// A pipeline resolver over a function over a data source: a two-level chain.
    PipelineChain,
}

/// The template a row deploys.
fn template_of(shape: &Shape) -> String {
    let api = |name: &str| {
        format!(
            r#""Api":{{"Type":"AWS::AppSync::GraphQLApi","Properties":{{"Name":"{name}",
               "AuthenticationType":"API_KEY"}}}}"#
        )
    };
    let schema = format!(
        r#""Schema":{{"Type":"AWS::AppSync::GraphQLSchema","Properties":{{
           "ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"Definition":"{SCHEMA}"}}}}"#
    );
    let source = r#""Source":{"Type":"AWS::AppSync::DataSource","Properties":{
        "ApiId":{"Fn::GetAtt":["Api","ApiId"]},"Name":"none","Type":"NONE"}}"#;
    match shape {
        Shape::DependsOnAfter => format!(
            r#"{{"Resources":{{{api},{schema},{source},
               "Field":{{"Type":"AWS::AppSync::Resolver","DependsOn":["Schema","Source"],
                 "Properties":{{"ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},
                 "TypeName":"Query","FieldName":"first","DataSourceName":"none","Kind":"UNIT",
                 "RequestMappingTemplate":"{REQUEST_TEMPLATE}",
                 "ResponseMappingTemplate":"{RESPONSE_TEMPLATE}"}}}}}}}}"#,
            api = api("depends-on-after"),
        ),
        Shape::ReferenceBefore => format!(
            r#"{{"Resources":{{
               "Field":{{"Type":"AWS::AppSync::Resolver","DependsOn":["Schema"],
                 "Properties":{{"ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},
                 "TypeName":"Query","FieldName":"first",
                 "DataSourceName":{{"Fn::GetAtt":["Source","Name"]}},"Kind":"UNIT",
                 "RequestMappingTemplate":"{REQUEST_TEMPLATE}",
                 "ResponseMappingTemplate":"{RESPONSE_TEMPLATE}"}}}},
               {source},{schema},{api}}}}}"#,
            api = api("reference-before"),
        ),
        Shape::PipelineChain => format!(
            r#"{{"Resources":{{
               "Field":{{"Type":"AWS::AppSync::Resolver","DependsOn":["Schema"],
                 "Properties":{{"ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},
                 "TypeName":"Query","FieldName":"first","Kind":"PIPELINE",
                 "PipelineConfig":{{"Functions":[{{"Fn::GetAtt":["Enrich","FunctionId"]}}]}},
                 "RequestMappingTemplate":"{REQUEST_TEMPLATE}",
                 "ResponseMappingTemplate":"{RESPONSE_TEMPLATE}"}}}},
               "Enrich":{{"Type":"AWS::AppSync::FunctionConfiguration","DependsOn":["Schema"],
                 "Properties":{{"ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"Name":"Enrich",
                 "DataSourceName":{{"Fn::GetAtt":["Source","Name"]}},
                 "FunctionVersion":"2018-05-29",
                 "RequestMappingTemplate":"{REQUEST_TEMPLATE}",
                 "ResponseMappingTemplate":"{RESPONSE_TEMPLATE}"}}}},
               {source},{schema},{api}}}}}"#,
            api = api("pipeline-chain"),
        ),
    }
}

const ROWS: &[Row] = &[
    Row {
        case: "a resolver that DependsOn the data source it names, declared after it",
        region: "us-east-1",
        stack: "appsync-depends-on-after",
        shape: Shape::DependsOnAfter,
    },
    Row {
        case: "a resolver whose only edge is Fn::GetAtt on the data source's Name, declared \
               before it",
        region: "eu-west-1",
        stack: "appsync-reference-before",
        shape: Shape::ReferenceBefore,
    },
    Row {
        case: "a pipeline resolver over a function over a data source, both declared after it",
        region: "ap-southeast-2",
        stack: "appsync-pipeline-chain",
        shape: Shape::PipelineChain,
    },
];

/// Every row: the stack creates its AppSync estate, and `DeleteStack` then reaches
/// `DELETE_COMPLETE` with the API gone — which is only reachable by deleting each resource
/// before the one it depends on, since AppSync refuses the other order.
#[tokio::test]
async fn a_stack_delete_removes_each_resource_before_what_it_depends_on() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let region = row.region;
        let template = template_of(&row.shape);
        let stack_id = create_stack(&router, region, row.stack, &template, case).await;
        let api_id = api_id_of(&router, region, &stack_id, case).await;
        delete_stack_and_expect_the_api_gone(&router, region, &stack_id, &api_id, case).await;
    }
}

/// The same rule where the dependent is a REPLACEMENT: a resolver whose `FieldName` moved is a
/// different resolver (that member is a URI label of every call on one), and
/// `UpdateReplacePolicy: Retain` leaves the one the stack had in place. The stack's delete has
/// to remove the resolver it now holds before the data source that resolver names.
#[tokio::test]
async fn a_stack_delete_removes_a_replacement_resolver_before_its_data_source() {
    let router = build_router();
    let region = "us-west-2";
    let case = "a resolver replaced under UpdateReplacePolicy Retain";

    let stack_id = create_stack(
        &router,
        region,
        "appsync-retained-replacement",
        &retained_replacement_template("first", CODE),
        case,
    )
    .await;
    let api_id = api_id_of(&router, region, &stack_id, case).await;

    // An IN-PLACE change of the resolver's code, which `UpdateResolver` applies to the resolver
    // the stack already holds.
    update_stack(
        &router,
        region,
        &stack_id,
        &retained_replacement_template("first", CHANGED_CODE),
        case,
    )
    .await;
    // ...and a REPLACEMENT, which the retained policy keeps the old resolver through.
    update_stack(
        &router,
        region,
        &stack_id,
        &retained_replacement_template("second", CHANGED_CODE),
        case,
    )
    .await;

    for field in ["first", "second"] {
        let (status, read) = appsync(
            &router,
            region,
            "GET",
            &format!("/v1/apis/{api_id}/types/Query/resolvers/{field}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetResolver for Query.{field} after the \
             retained replacement, got {status} with {read}"
        );
    }

    // The retained resolver is removed by hand, exactly as a client cleaning up after the
    // replacement does, so the only resolver left is the one the stack itself holds.
    let (status, read) = appsync(
        &router,
        region,
        "DELETE",
        &format!("/v1/apis/{api_id}/types/Query/resolvers/first"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteResolver for the retained resolver, got \
         {status} with {read}"
    );

    delete_stack_and_expect_the_api_gone(&router, region, &stack_id, &api_id, case).await;
}

/// Deploy `template` onto the stack and wait for `UPDATE_COMPLETE`.
async fn update_stack(
    router: &MultiServiceRouter,
    region: &str,
    stack_id: &str,
    template: &str,
    case: &str,
) {
    let (status, body) = cloudformation(
        router,
        region,
        "UpdateStack",
        &[("StackName", stack_id), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    wait_for_stack_status(router, region, stack_id, "UPDATE_COMPLETE", case).await;
}

/// The template the replacement test deploys, with the resolver's `FieldName` and `Code` as
/// parameters so one body serves the create, the in-place change and the replacement.
fn retained_replacement_template(field: &str, code: &str) -> String {
    format!(
        r#"{{"Resources":{{
           "Api":{{"Type":"AWS::AppSync::GraphQLApi","Properties":{{"Name":"retained-api",
             "AuthenticationType":"API_KEY"}}}},
           "Schema":{{"Type":"AWS::AppSync::GraphQLSchema","Properties":{{
             "ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"Definition":"{SCHEMA}"}}}},
           "Source":{{"Type":"AWS::AppSync::DataSource","Properties":{{
             "ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"Name":"none","Type":"NONE"}}}},
           "Field":{{"Type":"AWS::AppSync::Resolver","DependsOn":["Schema","Source"],
             "UpdateReplacePolicy":"Retain","Properties":{{
             "ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"TypeName":"Query",
             "FieldName":"{field}","DataSourceName":"none","Kind":"UNIT",
             "Runtime":{{"Name":"APPSYNC_JS","RuntimeVersion":"1.0.0"}},
             "Code":"{code}"}}}},
           "Key":{{"Type":"AWS::AppSync::ApiKey","Properties":{{
             "ApiId":{{"Fn::GetAtt":["Api","ApiId"]}}}}}}}}}}"#
    )
}
