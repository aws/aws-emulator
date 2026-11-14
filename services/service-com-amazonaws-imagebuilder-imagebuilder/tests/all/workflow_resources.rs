// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the workflow references a BUILD names, and for the workflow
//! document an object store serves.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The workflow resource's own round trip, its build
//! versions, its dry run and its listing are covered in `workflow_versions.rs`; what is
//! covered here is what a request does with a workflow that already exists.
//!
//! The rules under test:
//!
//! * a workflow is a resource of this service, so a reference to one that a build or a
//!   pipeline carries has to name a stored workflow of the calling Region — resolved the way
//!   every versioned reference of this service is, a build version ARN naming one build and a
//!   version ARN the newest matching build — and a reference that reaches nothing is reported
//!   as the missing resource it names;
//! * how many references one request names is the caller's to choose while each one is a
//!   lookup against the store, so this emulator resolves at most
//!   `MAX_REQUEST_WORKFLOW_REFERENCES` DISTINCT references for one request and refuses a
//!   request naming more, counting a reference the request repeats once;
//! * `uri` is "The uri of a YAML workflow document file stored in Amazon S3", so a create
//!   naming one FETCHES that object and stores what it was served as the workflow's own
//!   document — a one-time snapshot a later overwrite of the object cannot change — and an
//!   address the object store serves nothing at is refused as that parameter's value with
//!   nothing minted.
//!
//! The tables span what each rule generalises over: the form of the reference (a build
//! version ARN and each wildcard version ARN), which operation carries it, how many
//! references a request names and how many of them are distinct, the shape of the object key,
//! what the served document declares, and which part of the address serves nothing.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::state::MAX_REQUEST_WORKFLOW_REFERENCES;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for.
///
/// Amazon S3 places a bucket created with no `LocationConstraint` in us-east-1 ("If you
/// don't specify a Region, the bucket is created in the US East (N. Virginia) Region
/// (us-east-1) by default"), which is also the Region imagebuilder serves a request in by
/// default, so one Region label serves the object store and the control plane alike.
const REGION: &str = "us-east-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The bucket the staged workflow documents live in.
const BUCKET: &str = "imagebuilder-workflows";

/// A bucket no `CreateBucket` here ever creates.
const ABSENT_BUCKET: &str = "imagebuilder-workflows-missing";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// A workflow document declaring one step and no input parameters: the smallest document
/// this service accepts, since the steps are the process Image Builder runs.
const PLAIN_DOCUMENT: &str = concat!(
    "name: BuildImage\n",
    "schemaVersion: 1.0\n",
    "steps:\n",
    "  - name: LaunchBuildInstance\n",
    "    action: LaunchInstance\n",
    "    onFailure: Abort\n",
);

/// A component document, for the recipe the build cases name.
const BUILD_COMPONENT_DOCUMENT: &str = concat!(
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

/// One emulator: imagebuilder and Amazon S3 on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// Status and parsed body of one imagebuilder call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
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
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes, so this is a raw request rather
/// than a modelled call.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

/// `PUT /<bucket>` — `CreateBucket` — so the staged documents have a bucket to live in.
async fn given_bucket(router: &MultiServiceRouter) {
    let status = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "case \"the document bucket\": expected it to be created, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `document` as the object's own body.
async fn given_object(router: &MultiServiceRouter, key: &str, document: &[u8]) {
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), document).await;
    assert_eq!(
        status, 200,
        "case {key:?}: expected the document object to be stored, got {status}"
    );
}

/// A `uri` naming the object `key` in [`BUCKET`].
fn uri_of(key: &str) -> String {
    format!("s3://{BUCKET}/{key}")
}

/// Read `name` out of an answer, panicking with the case when it is absent.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// One member of a response, or a panic naming the case.
fn member<'body>(case: &str, body: &'body Value, name: &str) -> &'body Value {
    body.get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"))
}

/// Assert the call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The `__type` of an error answer, with any shape-id prefix dropped.
fn error_code(case: &str, body: &Value) -> String {
    let rendered = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a __type member in {body}"));
    rendered
        .rsplit('#')
        .next()
        .unwrap_or(rendered)
        .to_string()
}

/// The `message` of an error answer.
fn error_message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message member in {body}"))
        .to_string()
}

/// Assert the answer is a refusal carrying `code` and `message`.
fn assert_refused(case: &str, status: u16, body: &Value, expected: u16, code: &str, message: &str) {
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected}, got {status} with {body}"
    );
    let actual_code = error_code(case, body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected {code}, got {actual_code:?} with {body}"
    );
    let actual_message = error_message(case, body);
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?}, got {actual_message:?}"
    );
}

/// Assert the answer is this service's not-found error, naming `resource_type` and `arn`.
fn assert_missing(case: &str, status: u16, body: &Value, resource_type: &str, arn: &str) {
    assert_refused(
        case,
        status,
        body,
        404,
        "ResourceNotFoundException",
        &format!("The following required resource '{resource_type}' cannot be found: '{arn}'."),
    );
}

/// A `CreateWorkflow` body for `name` on the build stage, with whatever `extra` names
/// overlaid — so a row can both add a member and replace one. A row that overlays `uri`
/// carries no inline document.
fn workflow_body(name: &str, version: &str, token: &str, extra: &Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!(version));
    body.insert("type".to_string(), json!("BUILD"));
    body.insert("clientToken".to_string(), json!(token));
    if let Value::Object(extra) = extra {
        if !extra.contains_key("uri") {
            body.insert("data".to_string(), json!(PLAIN_DOCUMENT));
        }
        for (member, value) in extra {
            body.insert(member.clone(), value.clone());
        }
    }
    Value::Object(body)
}

async fn create_workflow(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/CreateWorkflow", Some(body)).await
}

async fn get_workflow(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetWorkflow?workflowBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// The ARN of one build version of the build workflow `normalized` names at `version`.
fn workflow_arn(normalized: &str, version: &str, build: u64) -> String {
    format!(
        "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/build/{normalized}/{version}/{build}"
    )
}

/// The component the build cases' recipe includes.
async fn given_component(router: &MultiServiceRouter) -> String {
    let body = json!({
        "name": "Workflow Reference Component",
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": "workflow-reference-component",
        "data": BUILD_COMPONENT_DOCUMENT,
    });
    let answer = call(router, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok("the build cases' component", answer.0, answer.1);
    text(
        "the build cases' component",
        &answer,
        "componentBuildVersionArn",
    )
}

/// The recipe the build cases build.
async fn given_recipe(router: &MultiServiceRouter, component: &str) -> String {
    let body = json!({
        "name": "Workflow Reference Recipe",
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": "workflow-reference-recipe",
        "components": [{ "componentArn": component }],
    });
    let answer = call(router, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok("the build cases' recipe", answer.0, answer.1);
    text("the build cases' recipe", &answer, "imageRecipeArn")
}

/// The infrastructure configuration the build cases build in.
async fn given_configuration(router: &MultiServiceRouter) -> String {
    let body = json!({
        "name": "Workflow Reference Infra",
        "clientToken": "workflow-reference-infra",
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let answer = call(
        router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok("the build cases' configuration", answer.0, answer.1);
    text(
        "the build cases' configuration",
        &answer,
        "infrastructureConfigurationArn",
    )
}

/// The resources a build names beside its workflows.
struct Build {
    recipe: String,
    configuration: String,
}

impl Build {
    /// Stage the component, the recipe and the infrastructure configuration a build needs.
    async fn given(router: &MultiServiceRouter) -> Self {
        let component = given_component(router).await;
        Self {
            recipe: given_recipe(router, &component).await,
            configuration: given_configuration(router).await,
        }
    }

    /// A `CreateImage` body naming `references` as its workflows.
    fn image(&self, token: &str, references: &[String]) -> Value {
        json!({
            "imageRecipeArn": self.recipe,
            "infrastructureConfigurationArn": self.configuration,
            "clientToken": token,
            "executionRole": "ImageBuilderWorkflowRole",
            "workflows": references
                .iter()
                .map(|reference| json!({ "workflowArn": reference }))
                .collect::<Vec<_>>(),
        })
    }

    /// A `CreateImagePipeline` body naming `references` as its workflows.
    fn pipeline(&self, name: &str, token: &str, references: &[String]) -> Value {
        let mut body = self.image(token, references);
        if let Value::Object(map) = &mut body {
            map.insert("name".to_string(), json!(name));
        }
        body
    }
}

/// The operations a workflow reference travels on, each with the member its answer carries
/// the resource it created in.
const CARRIERS: &[(&str, &str, &str)] = &[
    ("CreateImage", "/CreateImage", "imageBuildVersionArn"),
    (
        "CreateImagePipeline",
        "/CreateImagePipeline",
        "imagePipelineArn",
    ),
];

/// A build resolves a workflow reference against the stored workflows of its Region: the
/// build version ARN and every wildcard version ARN the create reported reach the stored
/// workflow, and a reference nothing stored is the missing resource it names.
#[tokio::test]
async fn a_build_resolves_the_workflow_references_it_names() {
    let router = emulator();
    let build = Build::given(&router).await;

    let created = create_workflow(
        &router,
        &workflow_body("Referenced Workflow", "1.2.3", "referenced-flow", &json!({})),
    )
    .await;
    let created = assert_ok("the workflow a build references", created.0, created.1);
    let build_version = text(
        "the workflow a build references",
        &created,
        "workflowBuildVersionArn",
    );
    let references = member(
        "the workflow a build references",
        &created,
        "latestVersionReferences",
    )
    .clone();

    // The rows differ along the FORM of the reference: the build version ARN itself, each of
    // the four wildcard version ARNs the create reported — every one of which has to resolve
    // — and one that reaches nothing at all.
    let mut rows: Vec<(String, String, bool)> =
        vec![("the build version ARN".to_string(), build_version, true)];
    for (key, reference) in references
        .as_object()
        .expect("the four references are an object")
    {
        rows.push((
            key.clone(),
            reference
                .as_str()
                .expect("a reference is text")
                .to_string(),
            true,
        ));
    }
    rows.push((
        "a workflow nothing created".to_string(),
        workflow_arn("absent-flow", "1.0.0", 1),
        false,
    ));

    for (n, (reference_case, reference, resolves)) in rows.into_iter().enumerate() {
        // Both operations that carry a workflow reference read it the same way.
        for (carrier, path, created_member) in CARRIERS {
            let case = format!("{carrier} naming {reference_case}");
            let token = format!("workflow-reference-{carrier}-{n}");
            let body = if *carrier == "CreateImagePipeline" {
                build.pipeline(
                    &format!("Workflow Reference Pipeline {n}"),
                    &token,
                    std::slice::from_ref(&reference),
                )
            } else {
                build.image(&token, std::slice::from_ref(&reference))
            };
            let answer = call(&router, "PUT", path, Some(&body)).await;
            if resolves {
                let answer = assert_ok(&case, answer.0, answer.1);
                assert!(
                    answer.get(created_member).is_some(),
                    "case {case:?}: expected a {created_member} in {answer}"
                );
            } else {
                assert_missing(&case, answer.0, &answer.1, "Workflow", &reference);
            }
        }
    }
}

/// How many workflow references a request names is the caller's to choose and each one is a
/// lookup against the store, so the number this emulator resolves for one request is capped:
/// past it the request is refused, a reference the request repeats counts once, and a list at
/// the cap is still resolved.
#[tokio::test]
async fn a_request_names_at_most_the_workflow_references_this_emulator_resolves() {
    let router = emulator();
    let build = Build::given(&router).await;

    let created = create_workflow(
        &router,
        &workflow_body("Capped Workflow", "1.0.0", "capped-flow", &json!({})),
    )
    .await;
    let created = assert_ok("the one stored workflow", created.0, created.1);
    let stored = text("the one stored workflow", &created, "workflowBuildVersionArn");

    let cap = MAX_REQUEST_WORKFLOW_REFERENCES;
    // The rows differ along how many references the list carries and how many of them are
    // DISTINCT, which is what the cap counts. `Ok` is a build the emulator resolves; `Err`
    // names the reference it reports missing, or is empty for the cap refusal.
    let rows: Vec<(String, Vec<String>, Result<(), String>)> = vec![
        (
            format!("one reference repeated {} times", cap + 10),
            vec![stored.clone(); cap + 10],
            Ok(()),
        ),
        (
            format!("{cap} distinct references, the cap itself"),
            (0..cap)
                .map(|seed| workflow_arn(&format!("capped-absent-{seed}"), "1.0.0", 1))
                .collect(),
            Err(workflow_arn("capped-absent-0", "1.0.0", 1)),
        ),
        (
            format!("{} distinct references, one past the cap", cap + 1),
            (0..=cap)
                .map(|seed| workflow_arn(&format!("capped-absent-{seed}"), "1.0.0", 1))
                .collect(),
            Err(String::new()),
        ),
    ];

    for (n, (case, references, expected)) in rows.into_iter().enumerate() {
        let body = build.pipeline(
            &format!("Capped Pipeline {n}"),
            &format!("capped-pipeline-{n}"),
            &references,
        );
        let answer = call(&router, "PUT", "/CreateImagePipeline", Some(&body)).await;
        match expected {
            Ok(()) => {
                let answer = assert_ok(&case, answer.0, answer.1);
                assert!(
                    answer.get("imagePipelineArn").is_some(),
                    "case {case:?}: expected a pipeline ARN in {answer}"
                );
            }
            Err(unresolved) if !unresolved.is_empty() => {
                // At the cap the references ARE resolved, and the first that reaches nothing
                // is reported in the order the request named it.
                assert_missing(&case, answer.0, &answer.1, "Workflow", &unresolved);
            }
            Err(_) => assert_refused(
                &case,
                answer.0,
                &answer.1,
                402,
                "ServiceQuotaExceededException",
                &format!("One request can name at most {cap} workflows."),
            ),
        }
    }
}

/// A create naming a `uri` reads the document the object store serves and stores it as the
/// workflow's own: a one-time snapshot a later overwrite of the object cannot change, read
/// exactly like an inline document down to the input parameters it declares.
#[tokio::test]
async fn a_uri_names_the_document_the_object_store_serves() {
    let router = emulator();
    given_bucket(&router).await;

    // The rows differ along what the rule generalises over: the shape of the key (a bare
    // name, a nested prefix), and what the served document declares.
    let rows: Vec<(&str, &str, String, Value)> = vec![
        (
            "a bare key",
            "staged.yaml",
            PLAIN_DOCUMENT.to_string(),
            Value::Null,
        ),
        (
            "a nested prefix, and a document declaring a parameter",
            "team/build/staged.yaml",
            format!("{PLAIN_DOCUMENT}parameters:\n  - name: staged\n    type: string\n"),
            json!([{ "name": "staged", "type": "string" }]),
        ),
    ];

    for (n, (case, key, document, parameters)) in rows.into_iter().enumerate() {
        given_object(&router, key, document.as_bytes()).await;
        let created = create_workflow(
            &router,
            &workflow_body(
                &format!("Staged Workflow {n}"),
                "1.0.0",
                &format!("staged-{n}"),
                &json!({ "uri": uri_of(key) }),
            ),
        )
        .await;
        let created = assert_ok(case, created.0, created.1);
        let arn = text(case, &created, "workflowBuildVersionArn");
        assert_eq!(
            arn,
            workflow_arn(&format!("staged-workflow-{n}"), "1.0.0", 1),
            "case {case:?}: expected the ARN derived from the name the create carried"
        );

        let read = get_workflow(&router, &arn).await;
        let read = assert_ok(case, read.0, read.1);
        let workflow = member(case, &read, "workflow").clone();
        let actual = workflow.get("data").cloned();
        assert_eq!(
            actual,
            Some(json!(document)),
            "case {case:?}: expected the served document as the workflow's own, got {actual:?}"
        );
        // The document is read the same way whichever member named it, so the parameters it
        // declares are reported for a fetched document too.
        let actual = workflow.get("parameters").cloned();
        let expected = match parameters {
            Value::Null => None,
            declared => Some(declared),
        };
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the parameters {expected:?}, got {actual:?}"
        );

        // A ONE-TIME SNAPSHOT: overwriting the object does not change what a read of that
        // build version reports.
        given_object(
            &router,
            key,
            b"name: Replaced\nsteps:\n  - name: Other\n" as &[u8],
        )
        .await;
        let read = get_workflow(&router, &arn).await;
        let read = assert_ok(case, read.0, read.1);
        let workflow = member(case, &read, "workflow").clone();
        let actual = workflow.get("data").cloned();
        assert_eq!(
            actual,
            Some(json!(document)),
            "case {case:?}: expected the snapshot the create took, got {actual:?}"
        );
    }
}

/// An address the object store serves no workflow document from is refused as that
/// parameter's value, and nothing is minted.
#[tokio::test]
async fn an_address_that_serves_no_workflow_document_is_refused() {
    let router = emulator();
    given_bucket(&router).await;
    given_object(&router, "present.yaml", PLAIN_DOCUMENT.as_bytes()).await;

    // The rows differ along WHICH part of the address serves nothing: the bucket, the key, a
    // key that only prefixes a stored one, and a key that only extends one.
    let rows: Vec<(&str, String)> = vec![
        (
            "a bucket nothing created",
            format!("s3://{ABSENT_BUCKET}/present.yaml"),
        ),
        ("a key nothing stored", uri_of("absent.yaml")),
        ("a key that only prefixes a stored one", uri_of("present")),
        (
            "a key that only extends a stored one",
            uri_of("present.yaml.bak"),
        ),
    ];

    for (n, (case, uri)) in rows.into_iter().enumerate() {
        let answer = create_workflow(
            &router,
            &workflow_body(
                &format!("Unserved Workflow {n}"),
                "1.0.0",
                &format!("unserved-{n}"),
                &json!({ "uri": uri }),
            ),
        )
        .await;
        assert_refused(
            case,
            answer.0,
            &answer.1,
            400,
            "InvalidParameterValueException",
            "The value supplied for parameter 'uri' is not valid. Not Found",
        );
        let arn = workflow_arn(&format!("unserved-workflow-{n}"), "1.0.0", 1);
        let read = get_workflow(&router, &arn).await;
        assert_missing(case, read.0, &read.1, "Workflow", &arn);
    }
}
