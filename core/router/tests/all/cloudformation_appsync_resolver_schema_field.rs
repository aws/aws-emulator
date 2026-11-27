// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation stack does with an `AWS::AppSync::Resolver` whose field the API's
//! active schema decides, and with a `AWS::AppSync::GraphQLSchema` change the resolver depends
//! on.
//!
//! Two rules, each asserted over a stack UPDATE rather than over the one call that exposed the
//! defect:
//!
//! 1. **A resolver is attached to a field of a type the API's active schema declares.** A
//!    declaration naming a field the schema does not declare is refused by AppSync with its
//!    modelled not-found error, so that resource reaches `CREATE_FAILED`, the stack update ends
//!    `UPDATE_ROLLBACK_COMPLETE` naming exactly that resource, the resolver never lands — a
//!    later `GetResolver` answers `NotFoundException` — and the generation the stack served
//!    before the update still serves.
//! 2. **A resource is acted on only once the resource it depends on has finished.** A schema
//!    change AppSync carries out asynchronously holds the resources declared `DependsOn` it, so
//!    an update that widens the schema AND adds a resolver on the field that widening declares
//!    completes: the resolver's create is made against the schema the change installed, not
//!    against the one the API still had when the change was accepted.
//!
//! Two crates have to agree for either rule to hold — CloudFormation makes the calls and
//! AppSync serves them — so the suite lives here rather than in either one.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use super::query_wire::{blocks_of, element, elements, form_encode, opt_element};

/// How long one `DescribeStacks` poll waits before the next, and how many are made.
///
/// A stack's own transitions and the schema creation AppSync carries out asynchronously are
/// owed on their clocks, so the suite waits for the STATUS rather than for a span, with a
/// deadline generous enough for a loaded host.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 600;

/// The Region this suite deploys in.
const REGION: &str = "us-west-2";

/// The stack each case deploys under.
const STACK: &str = "appsync-resolver-field";

/// The type the resolvers of this suite are attached to.
const TYPE_NAME: &str = "Query";

/// The one field the first generation's schema declares, and the field the updates add a
/// resolver for: one the schema never declares, and one a wider schema does.
const DECLARED_FIELD: &str = "echo";
const ABSENT_FIELD: &str = "ghost";

/// The schema the stack installs, and the one that declares [`ABSENT_FIELD`] as well.
fn schema(fields: &[&str]) -> String {
    format!(
        "type Echo {{ msg: String }} type {TYPE_NAME} {{ {} }} schema {{ query: {TYPE_NAME} }}",
        fields
            .iter()
            .map(|field| format!("{field}(msg: String): Echo"))
            .collect::<Vec<String>>()
            .join(" ")
    )
}

/// The APPSYNC_JS definition every resolver of this suite carries.
const RESOLVER_CODE: &str =
    "export function request(ctx) { return {payload: {msg: ctx.args.msg}}; } \
     export function response(ctx) { return ctx.result; }";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from.
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
    (
        status,
        String::from_utf8_lossy(response.body().as_ref()).to_string(),
    )
}

/// Read one AWS AppSync resource over the `restJson1` wire its model declares.
async fn appsync(router: &MultiServiceRouter, path: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("host", format!("appsync.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("appsync"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The template one generation of the estate declares: the API, its schema, a local data
/// source, a resolver on `DECLARED_FIELD` and — where `extra_field` names one — a second
/// resolver on that field, each declared `DependsOn` the schema.
fn template(fields: &[&str], extra_field: Option<&str>) -> String {
    let definition = schema(fields).replace('"', "\\\"");
    let code = RESOLVER_CODE.replace('"', "\\\"");
    let resolver = |logical: &str, field: &str| {
        format!(
            concat!(
                r#""{logical}":{{"Type":"AWS::AppSync::Resolver","#,
                r#""DependsOn":["Schema","Source"],"Properties":{{"#,
                r#""ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"#,
                r#""TypeName":"{type_name}","FieldName":"{field}","Kind":"UNIT","#,
                r#""DataSourceName":{{"Fn::GetAtt":["Source","Name"]}},"Code":"{code}","#,
                r#""Runtime":{{"Name":"APPSYNC_JS","RuntimeVersion":"1.0.0"}}}}}},"#,
            ),
            logical = logical,
            type_name = TYPE_NAME,
            field = field,
            code = code,
        )
    };
    let mut resources = String::new();
    resources.push_str(
        r#""Api":{"Type":"AWS::AppSync::GraphQLApi","Properties":{"Name":"resolver-field-api","AuthenticationType":"API_KEY"}},"#,
    );
    resources.push_str(&format!(
        r#""Schema":{{"Type":"AWS::AppSync::GraphQLSchema","Properties":{{"ApiId":{{"Fn::GetAtt":["Api","ApiId"]}},"Definition":"{definition}"}}}},"#
    ));
    resources.push_str(
        r#""Source":{"Type":"AWS::AppSync::DataSource","Properties":{"ApiId":{"Fn::GetAtt":["Api","ApiId"]},"Name":"LocalSource","Type":"NONE"}},"#,
    );
    resources.push_str(&resolver("Echoes", DECLARED_FIELD));
    if let Some(field) = extra_field {
        resources.push_str(&resolver("Extra", field));
    }
    resources.push_str(
        r#""Key":{"Type":"AWS::AppSync::ApiKey","Properties":{"ApiId":{"Fn::GetAtt":["Api","ApiId"]}}}"#,
    );
    format!(
        r#"{{"Resources":{{{resources}}},"Outputs":{{"ApiIdOut":{{"Value":{{"Fn::GetAtt":["Api","ApiId"]}}}}}}}}"#
    )
}

/// Wait for the stack to report one of `terminal`, looking again until it does or the deadline
/// passes, and answer the status it reached with the answer that reported it.
async fn wait_for_terminal_status(
    router: &MultiServiceRouter,
    case: &str,
    terminal: &[&str],
) -> (String, String) {
    let mut body = String::new();
    for _ in 0..POLLS {
        let (status, reported) =
            cloudformation(router, "DescribeStacks", &[("StackName", STACK)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks expected HTTP 200, got {status} with {reported}"
        );
        body = reported;
        let reached = opt_element(&body, "StackStatus").unwrap_or_default();
        if terminal.contains(&reached.as_str()) {
            return (reached, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the stack never reached one of {terminal:?}; last answer was {body}");
}

/// The logical ids of the resources the stack's own events report as FAILED, and the reasons it
/// reported for them.
async fn failed_resources(router: &MultiServiceRouter, case: &str) -> (Vec<String>, Vec<String>) {
    let (status, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", STACK)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackEvents expected HTTP 200, got {status} with {body}"
    );
    let mut failed: Vec<String> = Vec::new();
    for event in blocks_of(&body, "member") {
        let reported = opt_element(event, "ResourceStatus").unwrap_or_default();
        if !reported.ends_with("_FAILED") {
            continue;
        }
        let logical = opt_element(event, "LogicalResourceId").unwrap_or_default();
        if !logical.is_empty() && !failed.contains(&logical) {
            failed.push(logical);
        }
    }
    (failed, elements(&body, "ResourceStatusReason"))
}

/// Deploy the first generation: the schema declaring one field and a resolver on it.
async fn created_api(router: &MultiServiceRouter, case: &str) -> String {
    let (status, created) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template(&[DECLARED_FIELD], None)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack expected HTTP 200, got {status} with {created}"
    );
    let (reached, body) = wait_for_terminal_status(
        router,
        case,
        &["CREATE_COMPLETE", "CREATE_FAILED", "ROLLBACK_COMPLETE"],
    )
    .await;
    assert_eq!(
        reached, "CREATE_COMPLETE",
        "case {case:?}: expected the estate to be created, got {reached:?} with {body}"
    );
    let outputs = super::query_wire::outputs_of(&body);
    outputs
        .get("ApiIdOut")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the API id as an output, got {outputs:?}"))
}

/// Send one `UpdateStack` for `fields`/`extra_field` and answer the terminal status it reached.
async fn updated(
    router: &MultiServiceRouter,
    case: &str,
    fields: &[&str],
    extra_field: Option<&str>,
) -> String {
    let (status, accepted) = cloudformation(
        router,
        "UpdateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template(fields, extra_field)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateStack expected HTTP 200, got {status} with {accepted}"
    );
    let (reached, _) = wait_for_terminal_status(
        router,
        case,
        &[
            "UPDATE_COMPLETE",
            "UPDATE_ROLLBACK_COMPLETE",
            "UPDATE_ROLLBACK_FAILED",
            "UPDATE_FAILED",
        ],
    )
    .await;
    reached
}

/// Read the resolver on `field` back through `GetResolver`.
async fn get_resolver(router: &MultiServiceRouter, api_id: &str, field: &str) -> (u16, Value) {
    appsync(
        router,
        &format!("/v1/apis/{api_id}/types/{TYPE_NAME}/resolvers/{field}"),
    )
    .await
}

/// Rule 1: a declared resolver on a field the active schema does not declare fails its own
/// resource and rolls the stack back, and the resolver never lands.
#[tokio::test]
async fn a_resolver_on_a_field_the_schema_does_not_declare_rolls_the_stack_back() {
    let case = "a resolver added on a field the schema never declares";
    let router = build_router();
    let api_id = created_api(&router, case).await;

    let reached = updated(&router, case, &[DECLARED_FIELD], Some(ABSENT_FIELD)).await;
    assert_eq!(
        reached, "UPDATE_ROLLBACK_COMPLETE",
        "case {case:?}: expected the update to roll back, got {reached:?}"
    );
    let (failed, reasons) = failed_resources(&router, case).await;
    assert_eq!(
        failed,
        vec!["Extra".to_string()],
        "case {case:?}: expected exactly the declared resolver to fail, got {failed:?}"
    );
    // The reason the stack reports is AppSync's own refusal of that create, which names the
    // field and the type the declaration asked for.
    let refused = format!("No field named {ABSENT_FIELD} found on type {TYPE_NAME}");
    assert!(
        reasons.iter().any(|reason| reason.contains(&refused)),
        "case {case:?}: expected a reason stating {refused:?}, got {reasons:?}"
    );
    // Nothing of the refused resolver landed…
    let (status, read) = get_resolver(&router, &api_id, ABSENT_FIELD).await;
    assert_eq!(
        (status, read["__type"].as_str()),
        (404, Some("NotFoundException")),
        "case {case:?}: expected no resolver on {ABSENT_FIELD:?}, got {status} with {read}"
    );
    // … and the generation the stack served before the update still serves.
    let (status, read) = get_resolver(&router, &api_id, DECLARED_FIELD).await;
    assert_eq!(
        (status, read["resolver"]["fieldName"].as_str()),
        (200, Some(DECLARED_FIELD)),
        "case {case:?}: expected the declared field to keep its resolver, got {status} with \
         {read}"
    );
}

/// Rule 2: an update that widens the schema AND adds a resolver on the field that widening
/// declares completes — the dependent resolver is created against the installed schema.
#[tokio::test]
async fn a_resolver_declared_on_a_field_the_same_update_adds_is_created() {
    let case = "a resolver added together with the schema that declares its field";
    let router = build_router();
    let api_id = created_api(&router, case).await;

    let reached = updated(
        &router,
        case,
        &[DECLARED_FIELD, ABSENT_FIELD],
        Some(ABSENT_FIELD),
    )
    .await;
    assert_eq!(
        reached, "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {reached:?} with the events {:?}",
        failed_resources(&router, case).await
    );
    // Both fields carry the resolver the template declares for them, which is AppSync's own
    // report rather than anything the stack says about itself.
    for field in [DECLARED_FIELD, ABSENT_FIELD] {
        let (status, read) = get_resolver(&router, &api_id, field).await;
        assert_eq!(
            (status, read["resolver"]["fieldName"].as_str()),
            (200, Some(field)),
            "case {case:?}: expected a resolver on {field:?}, got {status} with {read}"
        );
    }
    // And the API's own schema declares both, so neither resolver is bound to a field no query
    // can select.
    let (status, listed) = appsync(&router, &format!("/v1/apis/{api_id}/types?format=SDL")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTypes expected HTTP 200, got {status} with {listed}"
    );
    let declared = listed["types"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|declared| declared["definition"].as_str())
                .collect::<Vec<&str>>()
                .join("\n")
        })
        .unwrap_or_default();
    for field in [DECLARED_FIELD, ABSENT_FIELD] {
        assert!(
            declared.contains(field),
            "case {case:?}: expected the installed schema to declare {field:?}, got {declared:?}"
        );
    }
    // The stack's own record of the estate names no failed resource at all.
    let (failed, _) = failed_resources(&router, case).await;
    assert!(
        failed.is_empty(),
        "case {case:?}: expected no failed resource, got {failed:?}"
    );
    // The stack still publishes the API it created, which is what every read above addressed.
    let (status, described) =
        cloudformation(&router, "DescribeStacks", &[("StackName", STACK)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks expected HTTP 200, got {status} with {described}"
    );
    assert_eq!(
        element(&described, "StackName"),
        STACK,
        "case {case:?}: expected the stack to still be reported, got {described}"
    );
}
