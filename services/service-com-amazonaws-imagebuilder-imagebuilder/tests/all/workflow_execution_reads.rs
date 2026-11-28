// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the runtime instances of the workflows an image build ran: what
//! `GetWorkflowExecution` reports about one, what the listing of a build's instances carries,
//! how an id that reaches none is reported, and how a request that leaves out a member its
//! operation requires outside the body is refused.
//!
//! The rules under test:
//!
//! * `GetWorkflowExecution` "retrieves runtime information for a specific runtime instance of
//!   the workflow", named by the `workflowExecutionId` "Image Builder assigned to keep track
//!   of runtime details when it ran the workflow". A runtime instance exists because a BUILD
//!   ran a workflow, so a read of one reports the build that owns it
//!   (`imageBuildVersionArn`), the workflow build version that defines it
//!   (`workflowBuildVersionArn`, resolved to ONE build version however the configuration
//!   referenced it), that workflow's `type`, the `parallelGroup` the configuration named, the
//!   `totalStepCount` "the workflow document defines", and a `status` that moves with the
//!   build's own: a build still running has running workflows and a build that finished has
//!   workflows in a terminal state, where "the sum of succeeded, skipped, and failed steps
//!   only reaches this total if every step finishes in one of those states".
//! * `ListWorkflowExecutions` "returns a list of workflow runtime instance metadata objects
//!   for a specific image build version", which is how a caller learns the ids that build
//!   recorded: one entry per workflow the build ran, in the order it ran them, paged at the
//!   size the request asked for, and agreeing with a read of each instance it names. "Image
//!   Builder retains workflow execution records for a limited time, so this array can be
//!   empty", so a build this Region holds none of lists no instance rather than refusing.
//! * An id no build of the calling Region recorded reaches no runtime instance and is reported
//!   as the missing resource it names (HTTP 404 `ResourceNotFoundException`), while a value
//!   outside the modelled `WorkflowExecutionId` pattern is refused as that parameter's value
//!   (HTTP 400 `InvalidParameterValueException`).
//! * A request whose method and path are the ones an operation binds IS that operation's
//!   request: a member that operation requires and the request leaves out is a bad request,
//!   refused HTTP 400 `InvalidRequestException` naming the properties the request had to
//!   carry — the same answer a required member absent from a BODY already earns, and never a
//!   report that the operation is not implemented.
//!
//! The rows span what each rule generalises over: which operation and method the request
//! addressed and whether its member is bound to the query string or to the path, the way a
//! runtime instance was configured (a default workflow set, a workflow this account created
//! referenced exactly or by a wildcard version, a parallel group named or not), the stage of
//! the workflow that ran, the number of steps its document defines, the page size the listing
//! is walked at, and the way an id misses the pattern it has to match.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door and the handlers behind it — so what is asserted
//! is what a signed AWS SDK request observes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call is signed for unless a case signs for another; the ARNs it builds
/// carry it.
const REGION: &str = "us-west-2";

/// A second Region, which a case reads a runtime instance of the first Region's build from.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN of this account
/// carries.
const ACCOUNT: &str = "000000000000";

/// The error this service answers a request that left out a member its operation requires
/// with: the modelled `InvalidRequestException`
/// (`smithy_query(query="shape",shape="InvalidRequestException")`, `@httpError` 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The error this service refuses a parameter whose value its model does not admit with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`, `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The error a resource the request named that does not exist is reported as
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`, `@httpError` 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The modelled `WorkflowExecutionId` pattern
/// (`smithy_query(query="shape",shape="WorkflowExecutionId")`), which a refusal of that member
/// quotes as the model writes it: there is no partition or Region field in it to restate.
const WORKFLOW_EXECUTION_ID_PATTERN: &str =
    "^wf-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// The largest page size any listing of this service admits, which this suite walks the
/// listing at as well as at its smallest: the maximum this crate's overlay model declares for
/// the `RestrictedInteger` every `maxResults` of this service targets
/// (`smithy-model/local-additions.json`, `crate::state::MAX_LIST_PAGE_SIZE`).
const MAX_PAGE_SIZE: i64 = 25;

/// A component document with one build phase, which is all a recipe these cases build needs.
const BUILD_DOCUMENT: &str = concat!(
    "name: HelloWorld\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: Greet\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo hello\n",
);

/// The status an image reports once the build queued for it is due to have finished.
const AVAILABLE_STATUS: &str = "AVAILABLE";

/// The modelled `WorkflowExecutionStatus` value a runtime instance of a workflow that has
/// finished its work carries, and the one it carries while it is still running
/// (`smithy_query(query="shape",shape="WorkflowExecutionStatus")`).
const COMPLETED_STATUS: &str = "COMPLETED";
const RUNNING_STATUS: &str = "RUNNING";

/// How long a case waits for a queued build to reach its available instant before giving up.
/// Generous: the span itself is a few seconds, and what a case asserts is the CONDITION it
/// polls for, never how fast the host reached it.
const AVAILABLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long a case waits between two reads while polling for that condition.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// Status and parsed body of one call, signed for `region`. The error shape an answer names is
/// folded into the body under `__type`, where `restJson1` also carries it.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// [`call_in`] for [`REGION`], which every case but the cross-Region row signs for.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// Assert the call succeeded and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// Read `name` out of an answer, panicking with the case when it is absent.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Read the whole number `name` reports, panicking with the case when it is absent.
fn number(case: &str, body: &Value, name: &str) -> i64 {
    body.get(name)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a number in {name} of {body}"))
}

/// The error shape an answer names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence an answer carries.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CreateComponent` for `name`, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let case = format!("create the component {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateComponent",
        Some(&json!({
            "name": name,
            "semanticVersion": "1.0.0",
            "platform": "Linux",
            "clientToken": format!("component-{name}"),
            "data": BUILD_DOCUMENT,
        })),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name`, answering its ARN.
async fn create_recipe(router: &MultiServiceRouter, name: &str, component_arn: &str) -> String {
    let case = format!("create the image recipe {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateImageRecipe",
        Some(&json!({
            "name": name,
            "semanticVersion": "1.0.0",
            "parentImage": "ami-0123456789abcdef0",
            "clientToken": format!("recipe-{name}"),
            "components": [{ "componentArn": component_arn }],
        })),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let case = format!("create the infrastructure configuration {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&json!({
            "name": name,
            "clientToken": format!("configuration-{name}"),
            "instanceProfileName": "EC2InstanceProfileForImageBuilder",
        })),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// `CreateWorkflow` for the `stage` workflow `name` whose document defines `steps` steps,
/// answering its build version ARN.
async fn create_workflow(
    router: &MultiServiceRouter,
    name: &str,
    stage: &str,
    steps: usize,
) -> String {
    let mut document = String::from("name: Probe\nschemaVersion: 1.0\nsteps:\n");
    for step in 0..steps {
        document.push_str(&format!(
            "  - name: Step{step}\n    action: ExecuteBash\n    onFailure: Abort\n"
        ));
    }
    let case = format!("create the {stage} workflow {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateWorkflow",
        Some(&json!({
            "name": name,
            "semanticVersion": "1.0.0",
            "type": stage,
            "clientToken": format!("workflow-{name}"),
            "data": document,
        })),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "workflowBuildVersionArn")
}

/// `CreateImage` of `recipe`, answering the image build version ARN it filed.
async fn create_image(
    router: &MultiServiceRouter,
    recipe: &str,
    configuration: &str,
    token: &str,
    extra: &[(&str, Value)],
) -> String {
    let mut body = Map::new();
    body.insert("imageRecipeArn".to_string(), json!(recipe));
    body.insert(
        "infrastructureConfigurationArn".to_string(),
        json!(configuration),
    );
    body.insert("clientToken".to_string(), json!(token));
    for (member, value) in extra {
        body.insert((*member).to_string(), value.clone());
    }
    let case = format!("queue the build {token}");
    let (status, answer) = call(router, "PUT", "/CreateImage", Some(&Value::Object(body))).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageBuildVersionArn")
}

/// One page of `ListWorkflowExecutions` for the build `arn`, with `paging` added to the
/// request, signed for `region`.
async fn list_executions_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    paging: &[(&str, Value)],
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("imageBuildVersionArn".to_string(), json!(arn));
    for (member, value) in paging {
        body.insert((*member).to_string(), value.clone());
    }
    call_in(
        router,
        region,
        "POST",
        "/ListWorkflowExecutions",
        Some(&Value::Object(body)),
    )
    .await
}

/// One successful page of `ListWorkflowExecutions` for the build `arn`, signed for
/// [`REGION`].
async fn list_executions(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    paging: &[(&str, Value)],
) -> Value {
    let (status, body) = list_executions_in(router, REGION, arn, paging).await;
    assert_ok(case, status, body)
}

/// The runtime instances one listed page carries.
fn listed_executions(case: &str, page: &Value) -> Vec<Value> {
    page.get("workflowExecutions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a workflowExecutions array in {page}"))
        .clone()
}

/// Every runtime instance of the build `arn`, read by walking the listing to its end — which is
/// how a caller learns the ids the build recorded.
async fn all_listed_executions(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut paging: Vec<(&str, Value)> = match page_size {
        Some(size) => vec![("maxResults", json!(size))],
        None => Vec::new(),
    };
    loop {
        let page = list_executions(router, case, arn, &paging).await;
        walked.extend(listed_executions(case, &page));
        let Some(token) = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return walked;
        };
        assert!(
            walked.len() <= 64,
            "case {case:?}: the listing must end, got {walked:?}"
        );
        paging.retain(|(member, _)| *member != "nextToken");
        paging.push(("nextToken", json!(token)));
    }
}

/// The `workflowExecutionId` of each entry, in the order the page carried them.
fn listed_ids(case: &str, entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| text(case, entry, "workflowExecutionId"))
        .collect()
}

/// One successful `GetWorkflowExecution` for `id`.
async fn workflow_execution(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/GetWorkflowExecution?workflowExecutionId={id}"),
        None,
    )
    .await;
    assert_ok(case, status, body)
}

/// The `workflowArn` of each workflow configuration a read of the image `arn` reports, in
/// order.
async fn image_workflow_arns(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/GetImage?imageBuildVersionArn={arn}"),
        None,
    )
    .await;
    let body = assert_ok(case, status, body);
    let image = body
        .get("image")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an image in {body}"));
    image
        .get("workflows")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected workflows in {image}"))
        .iter()
        .map(|configuration| text(case, configuration, "workflowArn"))
        .collect()
}

/// The `dateCreated` and the build status a read of the image `arn` reports.
async fn image_state(router: &MultiServiceRouter, case: &str, arn: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "GET",
        &format!("/GetImage?imageBuildVersionArn={arn}"),
        None,
    )
    .await;
    let body = assert_ok(case, status, body);
    let image = body
        .get("image")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an image in {body}"));
    let build_status = image
        .get("state")
        .and_then(|state| state.get("status"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (text(case, &image, "dateCreated"), build_status)
}

/// Read the image `arn` back until the build it queued reports `AVAILABLE`.
///
/// The build a create queues is outstanding for a span this emulator chooses, so what a case
/// waits for is the CONDITION — the status the read reports — looked for again until it holds
/// or [`AVAILABLE_DEADLINE`] passes.
async fn wait_for_available(router: &MultiServiceRouter, case: &str, arn: &str) {
    let giving_up = Instant::now() + AVAILABLE_DEADLINE;
    loop {
        let (_, build_status) = image_state(router, case, arn).await;
        if build_status == AVAILABLE_STATUS {
            return;
        }
        assert!(
            Instant::now() < giving_up,
            "case {case:?}: expected the queued build to reach {AVAILABLE_STATUS}, last read \
             reported {build_status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The stage segment a workflow build version ARN carries, as the modelled `WorkflowType`
/// spells it: the ARN's `workflow/<stage>/…` field in upper case.
fn stage_of(case: &str, workflow_arn: &str) -> String {
    workflow_arn
        .rsplit(":workflow/")
        .next()
        .and_then(|resource| resource.split('/').next())
        .unwrap_or_else(|| panic!("case {case:?}: expected a workflow ARN, got {workflow_arn:?}"))
        .to_ascii_uppercase()
}

/// What a read of ONE runtime instance a build recorded must report of the workflow that
/// defines it: that workflow's build version ARN, the number of steps its document defines,
/// and the parallel group the configuration that ran it named.
type Expected = (String, i64, Option<&'static str>);

/// One build a case queues: what its request adds, and what each runtime instance it records
/// must report, in the order the build runs its workflows.
///
/// An empty `expected` says the build ran the default workflow set, whose ARNs the row does
/// not restate: they are the ones the IMAGE reports as the workflows it was configured with,
/// so the two reads are compared with each other rather than with a literal, and this store
/// holds no document for a workflow AWS owns, so no step count is claimed for one.
struct Build {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// What the `CreateImage` request adds beside its recipe and configuration.
    named: Vec<(&'static str, Value)>,
    /// What each runtime instance the build records must report.
    expected: Vec<Expected>,
}

/// RULE: a request whose method and path are the ones an operation of this service binds IS
/// that operation's request, so a member that operation requires and the request leaves out is
/// refused as a bad request naming the properties it had to carry — never reported as an
/// operation nobody has written.
///
/// The rows differ along the operation addressed, its method, and whether the absent member is
/// bound to the query string of a reader, of a delete, or of a request that also carries a
/// path label it DID supply.
#[tokio::test]
async fn a_request_that_omits_a_required_query_member_is_refused_as_a_bad_request() {
    let router = emulator();
    let tagged = format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/probe/1.0.0/1");
    let encoded_tagged = tagged.replace(':', "%3A").replace('/', "%2F");

    // (case, method, uri, the member the request had to carry)
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "the workflow runtime instance read",
            "GET",
            "/GetWorkflowExecution".to_string(),
            "workflowExecutionId",
        ),
        (
            "the workflow runtime instance read with an empty query string",
            "GET",
            "/GetWorkflowExecution?".to_string(),
            "workflowExecutionId",
        ),
        (
            "the workflow runtime instance read carrying another parameter",
            "GET",
            "/GetWorkflowExecution?stepExecutionId=step-0123abcd-ef01-2345-6789-abcdef012345"
                .to_string(),
            "workflowExecutionId",
        ),
        (
            "the workflow step runtime instance read",
            "GET",
            "/GetWorkflowStepExecution".to_string(),
            "stepExecutionId",
        ),
        (
            "the image read",
            "GET",
            "/GetImage".to_string(),
            "imageBuildVersionArn",
        ),
        (
            "the workflow read",
            "GET",
            "/GetWorkflow".to_string(),
            "workflowBuildVersionArn",
        ),
        (
            "the component read",
            "GET",
            "/GetComponent".to_string(),
            "componentBuildVersionArn",
        ),
        (
            "the infrastructure configuration read",
            "GET",
            "/GetInfrastructureConfiguration".to_string(),
            "infrastructureConfigurationArn",
        ),
        (
            "a delete",
            "DELETE",
            "/DeleteComponent".to_string(),
            "componentBuildVersionArn",
        ),
        (
            "a tag removal that supplied its path label and no tag key",
            "DELETE",
            format!("/tags/{encoded_tagged}"),
            "tagKeys",
        ),
    ];
    for (case, method, uri, member) in rows {
        let (status, refused) = call(&router, method, &uri, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {method} {uri}, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, INVALID_REQUEST,
            "case {case:?}: expected the error code {INVALID_REQUEST:?}, got {code:?}"
        );
        let expected = format!(
            "Your request is invalid. Request must contain ['{member}'] properties. Verify your \
             request and try again."
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a runtime instance of a workflow exists because a build RAN that workflow, so the
/// listing of a build's runtime instances carries one entry per workflow it ran and a read of
/// one reports the build that owns it, the ONE workflow build version that defines it, that
/// workflow's stage, the parallel group it ran in and the steps its document defines — with a
/// status that moves with the build's own.
///
/// The rows differ along the way the build was configured: the default workflow set a build
/// that names none runs, a workflow this account created referenced by its exact build
/// version, and one referenced by a wildcard version and run in a parallel group.
#[tokio::test]
async fn a_builds_workflow_runs_are_listed_and_read_back_by_the_id_the_build_recorded() {
    let router = emulator();
    let component = create_component(&router, "Runtime Instance Component").await;
    let configuration = create_configuration(&router, "Runtime Instance Infra").await;
    let recipe = create_recipe(&router, "Runtime Instance Recipe", &component).await;
    let build_workflow = create_workflow(&router, "Runtime Build Workflow", "BUILD", 3).await;
    let test_workflow = create_workflow(&router, "Runtime Test Workflow", "TEST", 2).await;
    // The same workflow, referenced by a version whose trailing nodes are wildcards: what RAN
    // is one build version, so that is what the runtime instance records.
    let wildcard_reference = format!(
        "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/test/runtime-test-workflow/x.x.x"
    );

    // What each build runs, and what each runtime instance it records must report.
    let rows: Vec<Build> = vec![
        Build {
            case: "a build that named no workflows",
            named: Vec::new(),
            expected: Vec::new(),
        },
        Build {
            case: "a build that named one workflow it created by its build version",
            named: vec![
                ("executionRole", json!("ImageBuilderWorkflowRole")),
                ("workflows", json!([{ "workflowArn": build_workflow }])),
            ],
            expected: vec![(build_workflow.clone(), 3, None)],
        },
        Build {
            // "Parallel groups apply only to test workflows", and a reference whose version
            // nodes are wildcards resolves to the build version that ran.
            case: "a build that named a test workflow by a wildcard version and a parallel group",
            named: vec![
                ("executionRole", json!("ImageBuilderWorkflowRole")),
                (
                    "workflows",
                    json!([{ "workflowArn": wildcard_reference, "parallelGroup": "probe-group" }]),
                ),
            ],
            expected: vec![(test_workflow.clone(), 2, Some("probe-group"))],
        },
    ];

    for (row, Build { case, named, expected }) in rows.into_iter().enumerate() {
        let image = create_image(
            &router,
            &recipe,
            &configuration,
            &format!("runtime-instance-{row}"),
            &named,
        )
        .await;
        let configured = image_workflow_arns(&router, case, &image).await;
        let expected: Vec<Expected> = if expected.is_empty() {
            configured
                .iter()
                .map(|arn| (arn.clone(), 0, None))
                .collect()
        } else {
            expected
        };
        assert_eq!(
            configured.len(),
            expected.len(),
            "case {case:?}: expected the image to report {} workflow configurations, got \
             {configured:?}",
            expected.len()
        );

        // A caller learns the ids the build recorded from the listing of that build's runtime
        // instances, which is the only place they are published.
        let listed = all_listed_executions(&router, case, &image, None).await;
        let ids = listed_ids(case, &listed);
        assert_eq!(
            ids.len(),
            expected.len(),
            "case {case:?}: expected one runtime instance per workflow the build ran, got {ids:?}"
        );

        let (date_created, _) = image_state(&router, case, &image).await;
        for (id, (workflow_arn, steps, parallel_group)) in ids.iter().zip(expected.iter()) {
            let case = &format!("{case}, the runtime instance of {workflow_arn}");
            let read = workflow_execution(&router, case, id).await;

            let reported_id = text(case, &read, "workflowExecutionId");
            assert_eq!(
                &reported_id, id,
                "case {case:?}: expected the id the build recorded, got {reported_id:?}"
            );
            let owner = text(case, &read, "imageBuildVersionArn");
            assert_eq!(
                owner, image,
                "case {case:?}: expected the build version that owns it, got {owner:?}"
            );
            let defined_by = text(case, &read, "workflowBuildVersionArn");
            assert_eq!(
                &defined_by, workflow_arn,
                "case {case:?}: expected the workflow build version that defines it, got \
                 {defined_by:?}"
            );
            let reported_stage = text(case, &read, "type");
            assert_eq!(
                reported_stage,
                stage_of(case, workflow_arn),
                "case {case:?}: expected the stage its workflow ARN carries, got \
                 {reported_stage:?}"
            );
            let reported_steps = number(case, &read, "totalStepCount");
            assert_eq!(
                reported_steps, *steps,
                "case {case:?}: expected the steps its document defines, got {reported_steps}"
            );
            let reported_group = read.get("parallelGroup").and_then(Value::as_str);
            assert_eq!(
                reported_group, *parallel_group,
                "case {case:?}: expected the parallel group its configuration named, got \
                 {reported_group:?}"
            );
            let started = text(case, &read, "startTime");
            assert_eq!(
                started, date_created,
                "case {case:?}: expected the instant the build started, got {started:?}"
            );
            // The build is outstanding or finished by the time this read is answered, and
            // either way the runtime instance reports the state of the build that runs it,
            // with no step in a final state before the run reaches one.
            let reported_status = text(case, &read, "status");
            assert!(
                [RUNNING_STATUS, COMPLETED_STATUS].contains(&reported_status.as_str()),
                "case {case:?}: expected a running or completed instance, got {reported_status:?}"
            );
            if reported_status == RUNNING_STATUS {
                assert_eq!(
                    number(case, &read, "totalStepsSucceeded"),
                    0,
                    "case {case:?}: a running instance has finished no step, got {read}"
                );
                assert!(
                    read.get("endTime").is_none_or(Value::is_null),
                    "case {case:?}: a running instance has not ended, got {read}"
                );
            }
        }

        // Once the build is due, every one of its runtime instances reports the terminal
        // status the build reached, the steps its document defines as succeeded, and the
        // instant it ended.
        wait_for_available(&router, case, &image).await;
        for (id, (workflow_arn, steps, _)) in ids.iter().zip(expected.iter()) {
            let case = &format!("{case}, the finished runtime instance of {workflow_arn}");
            let read = workflow_execution(&router, case, id).await;
            let reported_status = text(case, &read, "status");
            assert_eq!(
                reported_status, COMPLETED_STATUS,
                "case {case:?}: expected the instance of a finished build to report \
                 {COMPLETED_STATUS}, got {reported_status:?}"
            );
            assert_eq!(
                number(case, &read, "totalStepsSucceeded"),
                *steps,
                "case {case:?}: expected every step the document defines to have succeeded, got \
                 {read}"
            );
            for counted in ["totalStepsFailed", "totalStepsSkipped"] {
                assert_eq!(
                    number(case, &read, counted),
                    0,
                    "case {case:?}: expected no step counted in {counted}, got {read}"
                );
            }
            assert!(
                read.get("endTime").and_then(Value::as_str).is_some(),
                "case {case:?}: expected the instant the run ended, got {read}"
            );
            // "The request ID that uniquely identifies this request": every answer of this
            // service names the request it answered, and the id is per call.
            let named_request = text(case, &read, "requestId");
            assert!(
                !named_request.is_empty(),
                "case {case:?}: expected the answer to name its request, got {read}"
            );
            // A read changes nothing a later read sees: the same instance is reported again,
            // beside the id of the new request.
            let again = workflow_execution(&router, case, id).await;
            for member in [
                "workflowExecutionId",
                "imageBuildVersionArn",
                "workflowBuildVersionArn",
                "type",
                "status",
                "totalStepCount",
                "totalStepsSucceeded",
                "totalStepsFailed",
                "totalStepsSkipped",
                "startTime",
                "endTime",
                "parallelGroup",
            ] {
                assert_eq!(
                    again.get(member),
                    read.get(member),
                    "case {case:?}: a second read must report the same {member}, got {again} \
                     and {read}"
                );
            }
        }

        // The listing of the build's runtime instances reports the same instances a read of
        // each one reports, in the order the build ran its workflows, walked one entry at a
        // time: every member the two answers share is the same answer.
        let walked = all_listed_executions(&router, case, &image, Some(1)).await;
        assert_eq!(
            listed_ids(case, &walked),
            ids,
            "case {case:?}: expected the same instances in the same order at every page size"
        );
        for entry in &walked {
            let id = text(case, entry, "workflowExecutionId");
            let case = &format!("{case}, the listed entry for {id}");
            let read = workflow_execution(&router, case, &id).await;
            for member in [
                "workflowExecutionId",
                "workflowBuildVersionArn",
                "type",
                "status",
                "message",
                "totalStepCount",
                "totalStepsSucceeded",
                "totalStepsFailed",
                "totalStepsSkipped",
                "startTime",
                "endTime",
                "parallelGroup",
            ] {
                assert_eq!(
                    entry.get(member),
                    read.get(member),
                    "case {case:?}: the listed entry and the read must agree on {member}, got \
                     {entry} and {read}"
                );
            }
        }
    }
}

/// RULE: the listing of a build's runtime instances walks the instances THAT build recorded,
/// in the order it ran its workflows, carrying at most as many entries per page as the request
/// asked for and a `nextToken` only while an instance it did not carry remains — and a build
/// this Region holds none of has run no workflow, so its listing is empty rather than a
/// refusal. A token this store did not issue is not a position in the listing.
///
/// The rows differ along the page size the listing is walked at, including the ends of the
/// range the model admits.
#[tokio::test]
async fn the_listing_of_a_builds_workflow_runs_pages_in_the_order_the_build_ran_them() {
    let router = emulator();
    let component = create_component(&router, "Paged Instance Component").await;
    let configuration = create_configuration(&router, "Paged Instance Infra").await;
    let recipe = create_recipe(&router, "Paged Instance Recipe", &component).await;
    let image = create_image(&router, &recipe, &configuration, "paged-instance-0", &[]).await;

    // The whole listing, as a page large enough to carry it reports it.
    // The whole listing, as a page of the largest size this service's listings admit reports
    // it (`MAX_LIST_PAGE_SIZE`, the maximum this crate's overlay model declares for
    // `RestrictedInteger`, which every paginated listing of this service shares).
    let whole = list_executions(
        &router,
        "the whole listing",
        &image,
        &[("maxResults", json!(MAX_PAGE_SIZE))],
    )
    .await;
    let ids = listed_ids("the whole listing", &listed_executions("the whole listing", &whole));
    assert!(
        !ids.is_empty(),
        "case \"the whole listing\": expected the build's runtime instances, got {whole}"
    );
    assert_eq!(
        whole.get("imageBuildVersionArn").and_then(Value::as_str),
        Some(image.as_str()),
        "case \"the whole listing\": expected the build version the request named, got {whole}"
    );
    assert!(
        whole.get("nextToken").is_none_or(Value::is_null),
        "case \"the whole listing\": a complete page carries no nextToken, got {whole}"
    );

    // Walking it at every page size the modelled range admits at its ends and in between
    // reports the same instances in the same order.
    for page_size in [None, Some(1), Some(2), Some(7), Some(MAX_PAGE_SIZE)] {
        let case = &format!("the listing walked at maxResults={page_size:?}");
        let walked = listed_ids(case, &all_listed_executions(&router, case, &image, page_size).await);
        assert_eq!(
            walked, ids,
            "case {case:?}: expected the same instances in the same order, got {walked:?}"
        );
    }

    // A build this Region holds none of has run no workflow.
    for (case, arn, region) in [
        (
            "an image build version nothing built",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/never-built/1.0.0/1"),
            REGION,
        ),
        (
            "a build of another name that nothing built",
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/paged-instance-recipe/9.9.9/4"),
            REGION,
        ),
        (
            "a build this Region does not hold, read from another Region",
            image.clone(),
            OTHER_REGION,
        ),
    ] {
        let (status, page) = list_executions_in(&router, region, &arn, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {arn} in {region}, got {status} with {page}"
        );
        let entries = listed_executions(case, &page);
        assert!(
            entries.is_empty(),
            "case {case:?}: expected no runtime instance, got {entries:?}"
        );
        assert_eq!(
            page.get("imageBuildVersionArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the build version the request named, got {page}"
        );
    }

    // A token this store did not issue is not a position in the listing.
    let (status, refused) = list_executions_in(
        &router,
        REGION,
        &image,
        &[("nextToken", json!("not-a-token-this-store-issued"))],
    )
    .await;
    let case = "a token this store did not issue";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused}"
    );
    let code = error_code(case, &refused);
    assert_eq!(
        code, INVALID_PARAMETER_VALUE,
        "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?}"
    );
    let expected = "The value supplied for parameter 'nextToken' is not valid. The provided token \
                    does not meet the required format.";
    let actual = error_message(&refused);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}

/// RULE: an id no build of the calling Region recorded reaches no runtime instance and is
/// reported as the missing resource it names.
///
/// The rows differ along why the id reaches nothing: never minted at all, minted for a build of
/// ANOTHER Region, and an id of the shape this API mints that no build ever used.
#[tokio::test]
async fn an_id_no_build_of_the_region_recorded_reaches_no_runtime_instance() {
    let router = emulator();
    let component = create_component(&router, "Absent Instance Component").await;
    let configuration = create_configuration(&router, "Absent Instance Infra").await;
    let recipe = create_recipe(&router, "Absent Instance Recipe", &component).await;
    let image = create_image(&router, &recipe, &configuration, "absent-instance-0", &[]).await;
    let case = "the id the build recorded";
    let recorded = listed_ids(case, &all_listed_executions(&router, case, &image, None).await);
    let recorded = recorded
        .first()
        .expect("the build records a runtime instance per workflow it ran");

    // (case, the id read, the Region the read is signed for)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "an id nothing minted",
            "wf-00000000-0000-0000-0000-000000000000",
            REGION,
        ),
        (
            "an id of the minted shape no build used",
            "wf-0123abcd-ef01-2345-6789-abcdef012345",
            REGION,
        ),
        (
            "an id uppercase hex, which the pattern admits, that no build used",
            "wf-0123ABCD-EF01-2345-6789-ABCDEF012345",
            REGION,
        ),
        (
            "an id a build of another Region recorded",
            recorded.as_str(),
            OTHER_REGION,
        ),
    ];
    for (case, id, region) in rows {
        let (status, refused) = call_in(
            &router,
            region,
            "GET",
            &format!("/GetWorkflowExecution?workflowExecutionId={id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {id:?} in {region}, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, RESOURCE_NOT_FOUND,
            "case {case:?}: expected the error code {RESOURCE_NOT_FOUND:?}, got {code:?}"
        );
        let expected =
            format!("The following required resource 'WorkflowExecution' cannot be found: '{id}'.");
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }

    // The id the build DID record is reached in the Region that build ran in, so the rows above
    // report what is missing rather than refusing everything.
    let read = workflow_execution(&router, "the id the build recorded", recorded).await;
    let reported = text("the id the build recorded", &read, "workflowExecutionId");
    assert_eq!(
        &reported, recorded,
        "case \"the id the build recorded\": expected it to be reached in its own Region, got \
         {read}"
    );
}

/// RULE: a `workflowExecutionId` outside the pattern the model declares for it is refused as
/// the value of the parameter it arrived in, quoting the expression that value had to match.
///
/// The rows differ along the way the value misses the pattern — the prefix, the dashes, the
/// group widths, a character that is not hex, the empty value, trailing text.
#[tokio::test]
async fn an_id_outside_its_pattern_is_refused_as_that_parameters_value() {
    let router = emulator();

    let rows = [
        ("the hex digits with no dashes", "wf-0123456789abcdef0123456789abcdef"),
        ("no wf- prefix", "0123abcd-ef01-2345-6789-abcdef012345"),
        ("another prefix", "step-0123abcd-ef01-2345-6789-abcdef012345"),
        ("too few hex digits in the first group", "wf-0123abc-ef01-2345-6789-abcdef012345"),
        ("too many hex digits in the last group", "wf-0123abcd-ef01-2345-6789-abcdef0123456"),
        ("a character that is not hex", "wf-0123abcg-ef01-2345-6789-abcdef012345"),
        ("trailing text after a well-formed id", "wf-0123abcd-ef01-2345-6789-abcdef012345x"),
        ("underscores for its dashes", "wf-0123abcd_ef01_2345_6789_abcdef012345"),
        ("the prefix alone", "wf-"),
        ("the empty value", ""),
    ];
    for (case, id) in rows {
        let (status, refused) = call(
            &router,
            "GET",
            &format!("/GetWorkflowExecution?workflowExecutionId={id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {id:?}, got {status} with {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?}"
        );
        let expected = format!(
            "The value supplied for parameter 'workflowExecutionId' is not valid. \
             workflowExecutionId must match pattern {WORKFLOW_EXECUTION_ID_PATTERN}"
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}
