// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what one RUN of an image build records, and what a read of the
//! image it produced reports about that run.
//!
//! The rule under test: a build resolves what its request pointed at and runs under an
//! identity, so the build version it files carries what the RUN did — and `GetImage` reports
//! it. From the modelled members of `Image`
//! (`smithy_query(query="shape",shape="Image")`) and the documentation of the request members
//! they answer:
//!
//! * `imageRecipe`/`containerRecipe` is "the image recipe that Image Builder used to create the
//!   image", resolved when the build starts, so the base image it names is the ONE image
//!   version the build built on — never the version line (`…/x.x.x`) a recipe may have been
//!   created with, which a read of the RECIPE itself still reports;
//! * `osVersion` is "the operating system version for instances that launch from this image",
//!   which is a property of that base image;
//! * `workflows` is "the build, test, and distribution workflow configurations that are
//!   associated with the image": the ones the request named, which "replace the default
//!   workflows that Image Builder otherwise runs for the build", or else every service-owned
//!   workflow of the build's output kind, one per stage and each at its own published build;
//! * `executionRole` is the role the build assumed, and "if you don't provide a role, Image
//!   Builder uses the Image Builder service-linked role in your account";
//! * `executionId` names the execution that produced the build version.
//!
//! Each table's rows differ along what the rule generalises over: the FORM of the base image
//! reference (a version line, a partly wildcarded version, an exact version, an AMI id, a
//! Systems Manager parameter, a Marketplace product id, an image ARN nothing holds, an image
//! this account itself built, a container base image), the kind of recipe the build builds,
//! whether the build tests the image, and whether the request named workflows and a role.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers), so what is asserted is what
//! a signed AWS SDK request observes. No Amazon EC2 or IAM is mounted beside this service: an
//! AMI id is then resolved by nothing and the recipe creates are stored as submitted, which is
//! what these cases are about.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::state::{
    MANAGED_IMAGE_OWNER, MANAGED_IMAGE_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN of this account
/// carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all a recipe these cases reference
/// needs in order to report a platform.
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

/// A workflow document declaring one step: the smallest document this service accepts.
const WORKFLOW_DOCUMENT: &str = concat!(
    "name: BuildImage\n",
    "schemaVersion: 1.0\n",
    "steps:\n",
    "  - name: LaunchBuildInstance\n",
    "    action: LaunchInstance\n",
    "    onFailure: Abort\n",
);

/// A container recipe's Dockerfile template, which the recipe create requires.
const DOCKERFILE_TEMPLATE: &str =
    "FROM {{{ imagebuilder:parentImage }}}\n{{{ imagebuilder:components }}}\n";

/// The status an image reports once the build queued for it is due to have finished
/// (`crate::state::IMAGE_STATUS_AVAILABLE`).
const AVAILABLE_STATUS: &str = "AVAILABLE";

/// How long a case waits for a queued build to reach its available instant before giving up.
/// Generous: the span itself is a few seconds (`crate::state::IMAGE_BUILD_SPAN`), and what a
/// case asserts is the CONDITION it polls for, never how fast the host reached it.
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

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on and the
/// handlers read the Region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
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
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
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

/// The text `name` reports, or `None` when the answer carries none.
fn optional(body: &Value, name: &str) -> Option<String> {
    body.get(name)?.as_str().map(str::to_string)
}

/// `CreateComponent` for `name`, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name}");
    let (status, answer) = call(router, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name` whose base image is `parent_image`, answering its ARN.
async fn create_recipe(
    router: &MultiServiceRouter,
    name: &str,
    parent_image: &str,
    component_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "parentImage": parent_image,
        "clientToken": format!("recipe-{name}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name}");
    let (status, answer) = call(router, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateContainerRecipe` for `name` whose base image is `parent_image`, answering its ARN.
async fn create_container_recipe(
    router: &MultiServiceRouter,
    name: &str,
    parent_image: &str,
    component_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "containerType": "DOCKER",
        "parentImage": parent_image,
        "clientToken": format!("container-recipe-{name}"),
        "components": [{ "componentArn": component_arn }],
        "dockerfileTemplateData": DOCKERFILE_TEMPLATE,
        "targetRepository": { "service": "ECR", "repositoryName": "team-a/my-repo" },
    });
    let case = format!("create the container recipe {name}");
    let (status, answer) = call(router, "PUT", "/CreateContainerRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "containerRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name}");
    let (status, answer) = call(
        router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// `CreateWorkflow` for the BUILD workflow `name`, answering its build version ARN.
async fn create_workflow(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "type": "BUILD",
        "clientToken": format!("workflow-{name}"),
        "data": WORKFLOW_DOCUMENT,
    });
    let case = format!("create the workflow {name}");
    let (status, answer) = call(router, "PUT", "/CreateWorkflow", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "workflowBuildVersionArn")
}

/// `CreateImage` of the recipe at `recipe_member`, answering the build version ARN it filed.
async fn create_image(
    router: &MultiServiceRouter,
    recipe_member: &str,
    recipe: &str,
    configuration: &str,
    token: &str,
    extra: &[(&str, Value)],
) -> String {
    let mut body = Map::new();
    body.insert(recipe_member.to_string(), json!(recipe));
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

/// The `image` of a successful `GetImage` for `reference`.
async fn image_of(router: &MultiServiceRouter, case: &str, reference: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/GetImage?imageBuildVersionArn={reference}"),
        None,
    )
    .await;
    let body = assert_ok(case, status, body);
    body.get("image")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an image in {body}"))
}

/// Read `reference` back until the build version it reaches reports `AVAILABLE`, and answer
/// that image.
///
/// The build a create queues is outstanding for a span this emulator chooses, so what a case
/// waits for is the CONDITION — the status the read reports — looked for again until it holds
/// or [`AVAILABLE_DEADLINE`] passes.
async fn available_image(router: &MultiServiceRouter, case: &str, reference: &str) -> Value {
    let giving_up = Instant::now() + AVAILABLE_DEADLINE;
    loop {
        let image = image_of(router, case, reference).await;
        let status = image
            .get("state")
            .and_then(|state| state.get("status"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if status.as_deref() == Some(AVAILABLE_STATUS) {
            return image;
        }
        assert!(
            Instant::now() < giving_up,
            "case {case:?}: expected the queued build to reach {AVAILABLE_STATUS}, last read \
             reported {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `imageRecipe` of a successful `GetImageRecipe` for `arn`.
async fn recipe_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/GetImageRecipe?imageRecipeArn={arn}"),
        None,
    )
    .await;
    let body = assert_ok(case, status, body);
    body.get("imageRecipe")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageRecipe in {body}"))
}

/// An Amazon-managed image's ARN in [`REGION`] at `version`: its owner field is Amazon's
/// catalog rather than an account id.
fn managed_image_arn(name: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{MANAGED_IMAGE_OWNER}:image/{name}/{version}")
}

/// The build version ARN of the published build the catalog holds of the managed image `name`.
fn managed_build_arn(name: &str) -> String {
    managed_image_arn(name, &format!("{MANAGED_IMAGE_VERSION}/1"))
}

/// One service-owned workflow of the default set, as `(stage, name, published version)`.
///
/// AWS versions each of its own workflows independently — a new build of one line is published
/// as that line's document changes — so each entry carries its OWN version node, and the
/// catalogue is not one version shared across the set. The entries below are the service-owned
/// workflow catalogue this emulator publishes
/// (`SERVICE_OWNED_WORKFLOWS` in the crate's `state::image_build_run`), one per stage of each
/// output kind — restated here by hand ON PURPOSE: reading the crate's own catalogue would make
/// the version assertions re-assert the implementation's literals, so they could not fail.
type ServiceOwnedWorkflow = (&'static str, &'static str, &'static str);

/// The default workflow set of a build whose output is an AMI, in the order a build runs them.
const AMI_WORKFLOWS: &[ServiceOwnedWorkflow] = &[
    ("build", "build-image", "1.0.1"),
    ("test", "test-image", "1.0.3"),
];

/// The default workflow set of a build whose output is a container image.
const CONTAINER_WORKFLOWS: &[ServiceOwnedWorkflow] = &[
    ("build", "build-container", "1.0.1"),
    ("test", "test-container", "1.0.1"),
];

/// The build version ARN of the `stage` workflow AWS itself owns under `name`, published at
/// `version`, as a build that ran it names it: AWS's own owner field, the stage segment the
/// workflow's type carries, and a version assigned down to the build node.
fn service_owned_workflow_arn(stage: &str, name: &str, version: &str) -> String {
    format!(
        "arn:aws:imagebuilder:{REGION}:{MANAGED_IMAGE_OWNER}:workflow/{stage}/{name}/\
         {version}/1"
    )
}

/// The build version ARN of every workflow in `catalogue`, in order.
fn service_owned_workflow_arns(catalogue: &[ServiceOwnedWorkflow]) -> Vec<String> {
    catalogue
        .iter()
        .map(|(stage, name, version)| service_owned_workflow_arn(stage, name, version))
        .collect()
}

/// The `workflowArn` of each configuration the image reports, in order.
fn reported_workflows(case: &str, image: &Value) -> Vec<String> {
    image
        .get("workflows")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected workflows in {image}"))
        .iter()
        .map(|configuration| text(case, configuration, "workflowArn"))
        .collect()
}

/// RULE: the base image a read of an image reports is the ONE image version the build resolved
/// its recipe's `parentImage` to and ran on, and the operating system it reports is the one
/// that image carries; a reference naming no image resource of this service stands as the
/// recipe holds it and says nothing about an operating system — while a read of the RECIPE goes
/// on reporting the reference it was created with.
///
/// The rows differ along the form of the base image reference: the version line a recipe is
/// usually created with, a partly wildcarded version, an exact published version, the three
/// non-ARN identifier forms `parentImage` documents, and an image ARN no catalog here holds.
/// The operating systems asserted are AWS's own for those managed images (the Image Builder
/// managed-image list), including two entries no recorded case names.
#[tokio::test]
async fn a_build_records_the_base_image_it_resolved_and_the_operating_system_it_carries() {
    let router = emulator();
    let component = create_component(&router, "Base Image Component").await;
    let configuration = create_configuration(&router, "Base Image Infra").await;

    // The major and minor nodes of the version the catalog publishes, so a row can wildcard
    // the nodes below them.
    let mut nodes = MANAGED_IMAGE_VERSION.split('.');
    let major = nodes.next().unwrap_or_default().to_string();
    let minor = nodes.next().unwrap_or_default().to_string();

    // (case, the recipe's parentImage, the base image the read reports, the osVersion it
    // reports)
    let rows: Vec<(&str, String, String, Option<&str>)> = vec![
        (
            "a managed image named by a full version line",
            managed_image_arn("amazon-linux-2023-x86", "x.x.x"),
            managed_build_arn("amazon-linux-2023-x86"),
            Some("Amazon Linux 2023"),
        ),
        (
            "a managed image named below its published major node",
            managed_image_arn("ubuntu-server-20-lts-x86", &format!("{major}.x.x")),
            managed_build_arn("ubuntu-server-20-lts-x86"),
            Some("Ubuntu 20.04"),
        ),
        (
            "a managed image named below its published minor node",
            managed_image_arn(
                "windows-server-2022-english-full-base-x86",
                &format!("{major}.{minor}.x"),
            ),
            managed_build_arn("windows-server-2022-english-full-base-x86"),
            Some("Microsoft Windows Server 2022"),
        ),
        (
            "a managed image named at exactly its published version",
            managed_image_arn("amazon-linux-2-arm64", MANAGED_IMAGE_VERSION),
            managed_build_arn("amazon-linux-2-arm64"),
            Some("Amazon Linux 2"),
        ),
        (
            // The three identifier forms `parentImage` documents beside an Image Builder image
            // ARN. Each names a catalog no part of this emulator resolves an identifier
            // against, so the build records the value as the recipe holds it.
            "an AMI id",
            "ami-0123456789abcdef0".to_string(),
            "ami-0123456789abcdef0".to_string(),
            None,
        ),
        (
            "a Systems Manager parameter",
            "ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64"
                .to_string(),
            "ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64"
                .to_string(),
            None,
        ),
        (
            "a Marketplace product id",
            "prod-0123456789abc".to_string(),
            "prod-0123456789abc".to_string(),
            None,
        ),
        (
            // An image ARN of a name no catalog here holds reaches no image, so nothing is
            // resolved and nothing is claimed.
            "an image ARN no catalog holds",
            managed_image_arn("no-such-managed-image", "x.x.x"),
            managed_image_arn("no-such-managed-image", "x.x.x"),
            None,
        ),
    ];

    for (row, (case, parent_image, expected_base, expected_os)) in rows.iter().enumerate() {
        let recipe = create_recipe(
            &router,
            &format!("Base Image Recipe {row}"),
            parent_image,
            &component,
        )
        .await;
        let arn = create_image(
            &router,
            "imageRecipeArn",
            &recipe,
            &configuration,
            &format!("base-image-{row}"),
            &[],
        )
        .await;

        let image = image_of(&router, case, &arn).await;
        let reported_recipe = image
            .get("imageRecipe")
            .unwrap_or_else(|| panic!("case {case:?}: expected an imageRecipe in {image}"));
        let reported_base = text(case, reported_recipe, "parentImage");
        assert_eq!(
            reported_base, *expected_base,
            "case {case:?}: expected the build to report the base image {expected_base:?}, got \
             {reported_base:?}"
        );
        let reported_os = optional(&image, "osVersion");
        assert_eq!(
            reported_os.as_deref(),
            *expected_os,
            "case {case:?}: expected the osVersion {expected_os:?}, got {reported_os:?}"
        );

        // A read of the RECIPE reports what is stored: the reference its create carried, even
        // where the build resolved it to one image version.
        let stored = recipe_of(&router, case, &recipe).await;
        let stored_base = text(case, &stored, "parentImage");
        assert_eq!(
            stored_base, *parent_image,
            "case {case:?}: expected the recipe to keep {parent_image:?}, got {stored_base:?}"
        );
    }
}

/// RULE: a build whose base image is an image THIS ACCOUNT built resolves it the same way — to
/// the one build version that reference reaches — and reports the operating system that build
/// recorded, so a chain of builds keeps reporting the system it ultimately runs.
#[tokio::test]
async fn a_build_on_an_image_this_account_built_reports_what_that_build_recorded() {
    let router = emulator();
    let component = create_component(&router, "Chained Component").await;
    let configuration = create_configuration(&router, "Chained Infra").await;

    // The first build runs on a managed base image, so it records that image's operating
    // system.
    let base_recipe = create_recipe(
        &router,
        "Chained Base Recipe",
        &managed_image_arn("amazon-linux-2023-arm64", "x.x.x"),
        &component,
    )
    .await;
    let base_image = create_image(
        &router,
        "imageRecipeArn",
        &base_recipe,
        &configuration,
        "chained-base",
        &[],
    )
    .await;
    // A version line reaches a build of this account only once that build is AVAILABLE, which
    // is the state the second build resolves it in.
    available_image(&router, "the base build", &base_image).await;

    // The second build names the FIRST image by a version line of this account's own.
    let chained_recipe = create_recipe(
        &router,
        "Chained Recipe",
        &format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/chained-base-recipe/x.x.x"),
        &component,
    )
    .await;
    let chained = create_image(
        &router,
        "imageRecipeArn",
        &chained_recipe,
        &configuration,
        "chained-build",
        &[],
    )
    .await;

    let case = "a build on an image this account built";
    let image = image_of(&router, case, &chained).await;
    let reported_recipe = image
        .get("imageRecipe")
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageRecipe in {image}"));
    let reported_base = text(case, reported_recipe, "parentImage");
    assert_eq!(
        reported_base, base_image,
        "case {case:?}: expected the base image {base_image:?}, got {reported_base:?}"
    );
    let reported_os = optional(&image, "osVersion");
    assert_eq!(
        reported_os.as_deref(),
        Some("Amazon Linux 2023"),
        "case {case:?}: expected the operating system the base build recorded, got \
         {reported_os:?}"
    );
}

/// One row of the workflow-and-role table: what the request named, and what a read of the
/// image the build filed reports.
struct Run<'a> {
    /// What this row is a case of.
    case: &'a str,
    /// The member the recipe reference arrives in, which decides the build's output kind.
    recipe_member: &'a str,
    /// The recipe the build builds.
    recipe: &'a str,
    /// What the request names beside the recipe and the infrastructure configuration.
    named: Vec<(&'a str, Value)>,
    /// The `workflowArn` of each configuration the read is expected to report, in order.
    workflows: Vec<String>,
    /// The `executionRole` the read is expected to report.
    role: String,
}

/// RULE: a read of an image reports the workflows the build is CONFIGURED with — the ones its
/// request named, which replace the defaults, or else every service-owned workflow of the
/// build's output kind, one per stage and whatever the build's test settings say — and the role
/// the build ASSUMED, which is this account's Image Builder service-linked role when the
/// request named none.
///
/// The rows differ along whether the request named workflows and a role, the form the role was
/// named in, whether the build tests the image, and the kind of recipe the build builds (which
/// decides the output kind, and so which service-owned workflows ran).
#[tokio::test]
async fn a_build_records_the_workflows_it_ran_and_the_role_it_assumed() {
    let router = emulator();
    let component = create_component(&router, "Run Component").await;
    let configuration = create_configuration(&router, "Run Infra").await;
    let recipe = create_recipe(
        &router,
        "Run Recipe",
        "ami-0123456789abcdef0",
        &component,
    )
    .await;
    let container_recipe = create_container_recipe(
        &router,
        "Run Container Recipe",
        "amazonlinux:latest",
        &component,
    )
    .await;
    let workflow = create_workflow(&router, "Run Workflow").await;

    // The role a request may name in either documented form: a role NAME, and a role ARN
    // (`RoleNameOrArn`, pattern `^(?:arn:aws(?:-[a-z]+)*:iam::[0-9]{12}:role/)?[\w+=,.@\-_/]+$`).
    let role_name = "ImageBuilderWorkflowRole".to_string();
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/ImageBuilderWorkflow");
    // "If you don't provide a role, Image Builder uses the Image Builder service-linked role in
    // your account": the role AWS publishes for this service, under the path IAM reserves for
    // service-linked roles.
    let service_linked = format!(
        "arn:aws:iam::{ACCOUNT}:role/aws-service-role/imagebuilder.amazonaws.com/\
         AWSServiceRoleForImageBuilder"
    );

    // (case, the recipe member, the recipe, what the request adds, the workflows the read
    // reports, the role it reports)
    let rows: Vec<Run> = vec![
        Run {
            case: "a build that named neither workflows nor a role",
            recipe_member: "imageRecipeArn",
            recipe: &recipe,
            named: Vec::new(),
            workflows: service_owned_workflow_arns(AMI_WORKFLOWS),
            role: service_linked.clone(),
        },
        Run {
            // "Specifies whether tests run after building the image" governs whether the test
            // stage RUNS, not which workflows the image is associated with: a build that
            // skips its tests is still associated with the whole default set.
            case: "a build that does not test the image",
            recipe_member: "imageRecipeArn",
            recipe: &recipe,
            named: vec![(
                "imageTestsConfiguration",
                json!({ "imageTestsEnabled": false }),
            )],
            workflows: service_owned_workflow_arns(AMI_WORKFLOWS),
            role: service_linked.clone(),
        },
        Run {
            case: "a build that tests the image by request",
            recipe_member: "imageRecipeArn",
            recipe: &recipe,
            named: vec![(
                "imageTestsConfiguration",
                json!({ "imageTestsEnabled": true, "timeoutMinutes": 90 }),
            )],
            workflows: service_owned_workflow_arns(AMI_WORKFLOWS),
            role: service_linked.clone(),
        },
        Run {
            // "If you specify workflows, they replace the default workflows that Image Builder
            // otherwise runs for the build, and you must also provide an executionRole."
            case: "a build that named one workflow and a role name",
            recipe_member: "imageRecipeArn",
            recipe: &recipe,
            named: vec![
                ("executionRole", json!(role_name)),
                ("workflows", json!([{ "workflowArn": workflow }])),
            ],
            workflows: vec![workflow.clone()],
            role: role_name.clone(),
        },
        Run {
            case: "a build that named a role by ARN",
            recipe_member: "imageRecipeArn",
            recipe: &recipe,
            named: vec![
                ("executionRole", json!(role_arn)),
                ("workflows", json!([{ "workflowArn": workflow }])),
            ],
            workflows: vec![workflow.clone()],
            role: role_arn.clone(),
        },
        Run {
            // A container build's output kind is a container image, so the service-owned
            // workflows it ran are the container ones.
            case: "a container build that named neither workflows nor a role",
            recipe_member: "containerRecipeArn",
            recipe: &container_recipe,
            named: Vec::new(),
            workflows: service_owned_workflow_arns(CONTAINER_WORKFLOWS),
            role: service_linked.clone(),
        },
        Run {
            // The test settings do not change the set for a container build either.
            case: "a container build that does not test the image",
            recipe_member: "containerRecipeArn",
            recipe: &container_recipe,
            named: vec![(
                "imageTestsConfiguration",
                json!({ "imageTestsEnabled": false }),
            )],
            workflows: service_owned_workflow_arns(CONTAINER_WORKFLOWS),
            role: service_linked.clone(),
        },
        Run {
            // A request that names workflows replaces the whole default set, whatever its
            // test settings say.
            case: "a build that named one workflow and does not test the image",
            recipe_member: "imageRecipeArn",
            recipe: &recipe,
            named: vec![
                ("executionRole", json!(role_name)),
                ("workflows", json!([{ "workflowArn": workflow }])),
                (
                    "imageTestsConfiguration",
                    json!({ "imageTestsEnabled": false }),
                ),
            ],
            workflows: vec![workflow.clone()],
            role: role_name.clone(),
        },
    ];

    for (row, expected) in rows.iter().enumerate() {
        let case = expected.case;
        let arn = create_image(
            &router,
            expected.recipe_member,
            expected.recipe,
            &configuration,
            &format!("run-{row}"),
            &expected.named,
        )
        .await;
        let image = image_of(&router, case, &arn).await;
        let reported = reported_workflows(case, &image);
        assert_eq!(
            reported, expected.workflows,
            "case {case:?}: expected the workflows {:?}, got {reported:?}",
            expected.workflows
        );
        let reported_role = optional(&image, "executionRole");
        assert_eq!(
            reported_role.as_deref(),
            Some(expected.role.as_str()),
            "case {case:?}: expected the executionRole {:?}, got {reported_role:?}",
            expected.role
        );
    }
}

/// The nodes of a service-owned workflow's build version ARN, as a configuration that names it
/// carries them: the Region and owner fields, and the stage, name, version and build of the
/// resource (`WorkflowConfiguration.workflowArn`,
/// `smithy_query(query="shape",shape="WorkflowConfiguration")`).
struct WorkflowArnNodes {
    region: String,
    owner: String,
    stage: String,
    name: String,
    version: String,
    build: String,
}

/// The nodes of `arn`, which must be a fully assigned workflow build version ARN.
fn workflow_arn_nodes(case: &str, arn: &str) -> WorkflowArnNodes {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected the six fields of an ARN, got {arn:?}"
    );
    let resource: Vec<&str> = fields[5].split('/').collect();
    assert_eq!(
        resource.len(),
        5,
        "case {case:?}: expected workflow/<stage>/<name>/<version>/<build>, got {arn:?}"
    );
    assert_eq!(
        resource[0], "workflow",
        "case {case:?}: expected a workflow resource, got {arn:?}"
    );
    WorkflowArnNodes {
        region: fields[3].to_string(),
        owner: fields[4].to_string(),
        stage: resource[1].to_string(),
        name: resource[2].to_string(),
        version: resource[3].to_string(),
        build: resource[4].to_string(),
    }
}

/// RULE: the default workflow set a build whose request named no `workflows` is associated with
/// holds ONE service-owned workflow per stage of the build's output kind, each named by a fully
/// assigned build-version ARN owned by AWS and carrying THAT workflow's own published semantic
/// version — AWS publishes each of its workflows on its own schedule, so no single version
/// describes the set.
///
/// The rows differ along the kind of recipe the build builds, which decides the output kind and
/// so which service-owned workflows the default set holds.
#[tokio::test]
async fn the_default_workflow_set_names_each_service_owned_workflow_at_its_own_published_build() {
    let router = emulator();
    let component = create_component(&router, "Default Set Component").await;
    let configuration = create_configuration(&router, "Default Set Infra").await;
    let recipe = create_recipe(
        &router,
        "Default Set Recipe",
        "ami-0123456789abcdef0",
        &component,
    )
    .await;
    let container_recipe = create_container_recipe(
        &router,
        "Default Set Container Recipe",
        "amazonlinux:latest",
        &component,
    )
    .await;

    // (case, the recipe member, the recipe, the catalogue of that output kind)
    let rows: &[(&str, &str, &str, &[ServiceOwnedWorkflow])] = &[
        (
            "a build whose output is an AMI",
            "imageRecipeArn",
            recipe.as_str(),
            AMI_WORKFLOWS,
        ),
        (
            "a build whose output is a container image",
            "containerRecipeArn",
            container_recipe.as_str(),
            CONTAINER_WORKFLOWS,
        ),
    ];

    for (row, (case, recipe_member, recipe, catalogue)) in rows.iter().enumerate() {
        let arn = create_image(
            &router,
            recipe_member,
            recipe,
            &configuration,
            &format!("default-set-{row}"),
            &[],
        )
        .await;
        let image = image_of(&router, case, &arn).await;
        let reported = reported_workflows(case, &image);
        assert_eq!(
            reported.len(),
            catalogue.len(),
            "case {case:?}: expected one configuration per service-owned workflow stage \
             ({}), got {reported:?}",
            catalogue.len()
        );
        for (entry, (stage, name, version)) in reported.iter().zip(catalogue.iter()) {
            let nodes = workflow_arn_nodes(case, entry);
            assert_eq!(
                nodes.region, REGION,
                "case {case:?}: expected the Region the build ran in, got {entry:?}"
            );
            assert_eq!(
                nodes.owner, MANAGED_IMAGE_OWNER,
                "case {case:?}: expected a workflow AWS owns, got {entry:?}"
            );
            assert_eq!(
                nodes.stage, *stage,
                "case {case:?}: expected the {stage:?} stage, got {entry:?}"
            );
            assert_eq!(
                nodes.name, *name,
                "case {case:?}: expected the workflow {name:?}, got {entry:?}"
            );
            // Each stage's own published version: the build stage and the test stage of one
            // output kind carry the versions their own lines were published at.
            assert_eq!(
                nodes.version, *version,
                "case {case:?}: expected {name:?} at its published version {version:?}, got \
                 {entry:?}"
            );
            assert_eq!(
                nodes.build, "1",
                "case {case:?}: expected the build node of the published build, got {entry:?}"
            );
        }
    }
}

/// RULE: a built image reports the id of the execution that produced it — one id per build,
/// stable across reads of that build and different for another build.
#[tokio::test]
async fn a_built_image_reports_the_id_of_the_execution_that_produced_it() {
    let router = emulator();
    let component = create_component(&router, "Execution Component").await;
    let configuration = create_configuration(&router, "Execution Infra").await;
    let recipe = create_recipe(
        &router,
        "Execution Recipe",
        "ami-0123456789abcdef0",
        &component,
    )
    .await;

    let mut seen: Vec<String> = Vec::new();
    for build in 1_u64..=2 {
        let case = format!("build {build}");
        let arn = create_image(
            &router,
            "imageRecipeArn",
            &recipe,
            &configuration,
            &format!("execution-{build}"),
            &[],
        )
        .await;
        let image = image_of(&router, &case, &arn).await;
        let execution = text(&case, &image, "executionId");
        // The 8-4-4-4-12 form this API's ids take: nothing predicts the value, so what is
        // asserted is the form and that it names ONE execution.
        let nodes: Vec<usize> = execution.split('-').map(str::len).collect();
        assert_eq!(
            nodes,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected an 8-4-4-4-12 execution id, got {execution:?}"
        );
        assert!(
            execution
                .chars()
                .all(|character| character.is_ascii_hexdigit() || character == '-'),
            "case {case:?}: expected a hexadecimal execution id, got {execution:?}"
        );
        // A read does not mint a new one: the execution is a fact of the build.
        let again = image_of(&router, &case, &arn).await;
        let repeated = text(&case, &again, "executionId");
        assert_eq!(
            repeated, execution,
            "case {case:?}: expected the same execution id on a second read, got {repeated:?}"
        );
        assert!(
            !seen.contains(&execution),
            "case {case:?}: expected an execution id of its own, got {execution:?} again"
        );
        seen.push(execution);
    }
}

/// RULE: the operating system a build recorded is reported wherever an image reports one, so a
/// listing of images reports it too and the `osVersion` filter the listing documents selects on
/// it.
#[tokio::test]
async fn a_listing_reports_the_operating_system_a_build_recorded() {
    let router = emulator();
    let component = create_component(&router, "Listed Component").await;
    let configuration = create_configuration(&router, "Listed Infra").await;
    let recipe = create_recipe(
        &router,
        "Listed Recipe",
        &managed_image_arn("ubuntu-server-22-lts-x86", "x.x.x"),
        &component,
    )
    .await;
    let arn = create_image(
        &router,
        "imageRecipeArn",
        &recipe,
        &configuration,
        "listed-build",
        &[],
    )
    .await;

    // AWS's own operating system for that managed image (the Image Builder managed-image
    // list), which the build ran on.
    let expected_os = "Ubuntu 22.04";

    let case = "the build versions of the listed image";
    let (status, listed) = call(
        &router,
        "POST",
        "/ListImageBuildVersions",
        Some(&json!({
            "imageVersionArn":
                format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/listed-recipe/1.0.0"),
        })),
    )
    .await;
    let listed = assert_ok(case, status, listed);
    let summaries = listed
        .get("imageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageSummaryList in {listed}"))
        .clone();
    let summary = summaries
        .iter()
        .find(|summary| optional(summary, "arn").as_deref() == Some(arn.as_str()))
        .unwrap_or_else(|| panic!("case {case:?}: expected the build version in {summaries:?}"));
    let reported = optional(summary, "osVersion");
    assert_eq!(
        reported.as_deref(),
        Some(expected_os),
        "case {case:?}: expected the summary to report {expected_os:?}, got {reported:?}"
    );

    // (case, the filter's values, whether the listing selects the built image)
    let rows: &[(&str, &str, bool)] = &[
        ("the operating system the build ran on", expected_os, true),
        // "Filter values are case-sensitive", and an operating system no build here ran on
        // selects none.
        ("another operating system", "Amazon Linux 2", false),
        ("the same value in another case", "ubuntu 22.04", false),
    ];

    for (case, value, selects) in rows {
        let (status, listed) = call(
            &router,
            "POST",
            "/ListImages",
            Some(&json!({
                "filters": [{ "name": "osVersion", "values": [value] }],
            })),
        )
        .await;
        let listed = assert_ok(case, status, listed);
        let versions = listed
            .get("imageVersionList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let own: Vec<String> = versions
            .iter()
            .filter(|entry| optional(entry, "owner").as_deref() == Some(ACCOUNT))
            .map(|entry| text(case, entry, "arn"))
            .collect();
        let expected: Vec<String> = if *selects {
            vec![format!(
                "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/listed-recipe/1.0.0"
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            own, expected,
            "case {case:?}: expected the listing to select {expected:?}, got {own:?}"
        );
    }
}
