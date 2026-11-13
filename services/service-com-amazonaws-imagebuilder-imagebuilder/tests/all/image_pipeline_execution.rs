// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the image-pipeline slice of this service: creating a
//! pipeline, reading it back, and starting a build of it.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * `CreateImagePipeline` persists the pipeline the request describes under an ARN
//!   derived from a normalized form of the name, with every resource it references
//!   resolved against the stored resources of the calling Region and the documented
//!   defaults filled in; `GetImagePipeline` reports that stored record.
//! * `StartImagePipelineExecution` resolves the pipeline from the stored record, and when
//!   this Region holds one it QUEUES a build: it files a new image build version derived
//!   from the recipe the pipeline references — the image's name and three assigned version
//!   nodes come from that recipe, and the fourth node counts up per name+version pair —
//!   and reports that build version's ARN.
//! * A call naming an ARN this Region holds no resource under is answered with the
//!   not-found error this service models (`ResourceNotFoundException`, HTTP 404), whose
//!   message names the kind of resource the ARN identifies and the ARN the request
//!   carried, verbatim.
//! * The pipeline's own read REPORTS its runs rather than recording them: the date and
//!   status of its last run are read from the newest build version that names it, and that
//!   status moves from queued to available with the CLOCK, not with anything a caller does.
//!
//! The rows below span what each rule generalises over: how a reference resolves to
//! nothing (never created, another Region, a name nothing carries, a resource kind this
//! store holds none of, a deleted recipe), which version nodes a recipe reference pins and
//! which it wildcards, how many tags a run carries, and how many runs one pipeline has
//! had.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, which is all a recipe these cases
/// reference needs in order to report a platform.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// The status an image reports while the build queued for it is still outstanding, and the
/// one it reports once that build is due to have finished
/// (`crate::state::IMAGE_STATUS_PENDING` / `IMAGE_STATUS_AVAILABLE`).
const QUEUED_STATUS: &str = "PENDING";

/// See [`QUEUED_STATUS`].
const AVAILABLE_STATUS: &str = "AVAILABLE";

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

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
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
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
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

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`, so
/// the `:` and `/` of an ARN travel as `%3A` and `%2F` and the whole ARN stays ONE
/// segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Read `name` out of an answer, panicking with the case when it is absent.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert the call succeeded and answer its body.
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

/// Assert the answer is this service's not-found error, naming `resource_type` and `arn`.
fn assert_missing(case: &str, status: u16, body: &Value, resource_type: &str, arn: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} with {body}"
    );
    let expected =
        format!("The following required resource '{resource_type}' cannot be found: '{arn}'.");
    let actual = error_message(case, body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
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

/// `CreateComponent` for one build version of `name` in `region`, answering its build
/// version ARN.
async fn create_component(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{region}-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name} in {region}");
    let (status, answer) = call_in(router, region, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name` at `version` in `region`, including `component_arn`,
/// answering the recipe's own ARN.
async fn create_recipe(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: &str,
    component_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": version,
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{region}-{name}-{version}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name} {version} in {region}");
    let (status, answer) = call_in(router, region, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name` in `region`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{region}-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// `CreateImagePipeline` with `body`, in `region`.
async fn create_pipeline_with(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "PUT", "/CreateImagePipeline", Some(body)).await
}

/// `CreateImagePipeline` for `name` in `region`, naming `recipe_reference` and
/// `configuration_arn`, answering the pipeline's own ARN.
async fn create_pipeline(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    recipe_reference: &str,
    configuration_arn: &str,
) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("pipeline-{region}-{name}"),
        "imageRecipeArn": recipe_reference,
        "infrastructureConfigurationArn": configuration_arn,
    });
    let case = format!("create the image pipeline {name} in {region}");
    let (status, answer) = create_pipeline_with(router, region, &body).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imagePipelineArn")
}

/// `GetImagePipeline` for `arn`, in `region`.
async fn get_pipeline(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetImagePipeline?imagePipelineArn={}", encode_label(arn)),
        None,
    )
    .await
}

/// The `imagePipeline` one `GetImagePipeline` answer carries.
async fn read_pipeline(case: &str, router: &MultiServiceRouter, region: &str, arn: &str) -> Value {
    let (status, answer) = get_pipeline(router, region, arn).await;
    let answer = assert_ok(case, status, answer);
    answer
        .get("imagePipeline")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imagePipeline member in {answer}"))
}

/// `StartImagePipelineExecution` for `arn` in `region`, with `token` and `tags`.
async fn start_pipeline(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    token: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("imagePipelineArn".to_string(), json!(arn));
    body.insert("clientToken".to_string(), json!(token));
    if !tags.is_empty() {
        body.insert("tags".to_string(), tag_object(tags));
    }
    call_in(
        router,
        region,
        "PUT",
        "/StartImagePipelineExecution",
        Some(&Value::Object(body)),
    )
    .await
}

/// `ListImageBuildVersions` with `body`, in `region`.
async fn list_build_versions(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/ListImageBuildVersions",
        Some(body),
    )
    .await
}

/// The `imageSummaryList` one answer carries, in the order it carried it.
fn listed_arns(case: &str, body: &Value) -> Vec<String> {
    body.get("imageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageSummaryList in {body}"))
        .iter()
        .map(|summary| text(case, summary, "arn"))
        .collect()
}

/// `ListTagsForResource` for `arn`, in `region`.
async fn list_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        None,
    )
    .await
}

/// `DeleteImageRecipe` for `arn`, in `region`.
async fn delete_recipe(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "DELETE",
        &format!("/DeleteImageRecipe?imageRecipeArn={}", encode_label(arn)),
        None,
    )
    .await
}

/// The pairs as the JSON object a `tags` member carries.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The pairs as the map an assertion compares against, so key order never matters.
fn tag_map(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The `tags` an answer reports, as a map.
fn reported_tags(case: &str, body: &Value) -> BTreeMap<String, String> {
    let Some(tags) = body.get("tags") else {
        return BTreeMap::new();
    };
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags object in {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected text in {body}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The ARN the image pipeline `normalized` carries in `region`.
fn pipeline_arn(region: &str, normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image-pipeline/{normalized}")
}

/// The ARN the image recipe `normalized` carries at `version` in `region`.
fn recipe_arn(region: &str, normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image-recipe/{normalized}/{version}")
}

/// The ARN one build version of the image `normalized` carries in `region`.
fn image_arn(region: &str, normalized: &str, version: &str, build: u64) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image/{normalized}/{version}/{build}")
}

/// One pipeline over a stored recipe and infrastructure configuration, ready to start:
/// answers the pipeline's ARN and the recipe's name.
async fn pipeline_over_a_recipe(router: &MultiServiceRouter, slug: &str) -> String {
    let component = create_component(router, REGION, &format!("{slug}-component")).await;
    let recipe = create_recipe(
        router,
        REGION,
        &format!("{slug}-recipe"),
        "1.0.0",
        &component,
    )
    .await;
    let configuration = create_configuration(router, REGION, &format!("{slug}-infra")).await;
    create_pipeline(
        router,
        REGION,
        &format!("{slug}-pipeline"),
        &recipe,
        &configuration,
    )
    .await
}

/// A start that names an ARN this Region holds no pipeline under is reported missing, and
/// so is a start whose pipeline no longer has a recipe to build.
#[tokio::test]
async fn a_start_naming_nothing_stored_is_reported_missing() {
    let router = emulator();

    // One pipeline that DOES exist in this Region, and one that exists in another, so the
    // rows below are not answered by an empty store.
    let kept = pipeline_over_a_recipe(&router, "kept").await;
    let elsewhere_component = create_component(&router, OTHER_REGION, "away-component").await;
    let elsewhere_recipe = create_recipe(
        &router,
        OTHER_REGION,
        "away-recipe",
        "1.0.0",
        &elsewhere_component,
    )
    .await;
    let elsewhere_configuration = create_configuration(&router, OTHER_REGION, "away-infra").await;
    let elsewhere = create_pipeline(
        &router,
        OTHER_REGION,
        "away-pipeline",
        &elsewhere_recipe,
        &elsewhere_configuration,
    )
    .await;

    // A pipeline whose recipe is deleted after the pipeline was created: the run has
    // nothing to build, and the resource it names is the RECIPE.
    let orphan_component = create_component(&router, REGION, "orphan-component").await;
    let orphan_recipe = create_recipe(
        &router,
        REGION,
        "orphan-recipe",
        "1.0.0",
        &orphan_component,
    )
    .await;
    let orphan_configuration = create_configuration(&router, REGION, "orphan-infra").await;
    let orphan = create_pipeline(
        &router,
        REGION,
        "orphan-pipeline",
        &orphan_recipe,
        &orphan_configuration,
    )
    .await;
    let (status, body) = delete_recipe(&router, REGION, &orphan_recipe).await;
    assert_ok("delete the recipe the orphan pipeline builds", status, body);

    for (case, arn, resource_type, named) in [
        (
            "a pipeline that was never created",
            pipeline_arn(REGION, "absent-pipeline"),
            "ImagePipeline",
            None,
        ),
        (
            "a pipeline of another Region",
            elsewhere.clone(),
            "ImagePipeline",
            None,
        ),
        (
            "a name no stored pipeline carries",
            pipeline_arn(REGION, "kept-pipeline-2"),
            "ImagePipeline",
            None,
        ),
        (
            "a pipeline whose recipe was deleted",
            orphan.clone(),
            "ImageRecipe",
            Some(orphan_recipe.clone()),
        ),
    ] {
        let (status, body) = start_pipeline(&router, REGION, &arn, &format!("token-{case}"), &[]).await;
        let reported = named.unwrap_or_else(|| arn.clone());
        assert_missing(case, status, &body, resource_type, &reported);
    }

    // The pipeline that does exist is still startable, so the rows above refused for the
    // resource they named and not because starts are refused at all.
    let (status, body) = start_pipeline(&router, REGION, &kept, "token-kept", &[]).await;
    let body = assert_ok("start the stored pipeline", status, body);
    assert_eq!(
        text("start the stored pipeline", &body, "imageBuildVersionArn"),
        image_arn(REGION, "kept-recipe", "1.0.0", 1),
        "case \"start the stored pipeline\": expected the first build version of the recipe"
    );
}

/// Each start of one pipeline QUEUES a build: it files a new image build version, counting
/// the build number up per name+version pair, and the image carries the tags that start
/// named.
#[tokio::test]
async fn each_start_files_the_next_build_version() {
    let router = emulator();
    let pipeline = pipeline_over_a_recipe(&router, "runs").await;

    // The rows differ in how many tags the run applies to the image it creates; the build
    // number they expect is the run's position, which only the stored build versions can
    // decide.
    for (position, (case, tags)) in [
        ("a run carrying no tags", &[][..]),
        ("a run carrying one tag", &[("Environment", "dev")][..]),
        (
            "a run carrying several tags",
            &[("Environment", "prod"), ("Owner", "platform")][..],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let build = u64::try_from(position).expect("a small position") + 1;
        let token = format!("run-{build}");
        let (status, body) = start_pipeline(&router, REGION, &pipeline, &token, tags).await;
        let body = assert_ok(case, status, body);
        let expected = image_arn(REGION, "runs-recipe", "1.0.0", build);
        let actual = text(case, &body, "imageBuildVersionArn");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the build version {expected:?}, got {actual:?}"
        );
        assert_eq!(
            text(case, &body, "clientToken"),
            token,
            "case {case:?}: expected the request's own clientToken echoed"
        );

        // The image the run created carries exactly the tags the START named: "Builds that
        // you start manually use the tags from the StartImagePipelineExecution request".
        let (status, listed) = list_tags(&router, REGION, &actual).await;
        let listed = assert_ok(case, status, listed);
        let reported = reported_tags(case, &listed);
        assert_eq!(
            reported,
            tag_map(tags),
            "case {case:?}: expected the image to carry the tags the run named, got {reported:?}"
        );
    }
}

/// A retry of a start carrying the same `clientToken` answers the ORIGINAL run, queues no
/// second build, and the same token carrying different parameters is a mismatch.
#[tokio::test]
async fn a_retried_start_answers_the_original_run() {
    let router = emulator();
    let pipeline = pipeline_over_a_recipe(&router, "retry").await;

    let (status, first) = start_pipeline(&router, REGION, &pipeline, "one-token", &[]).await;
    let first = assert_ok("the first start", status, first);
    let first_arn = text("the first start", &first, "imageBuildVersionArn");
    let first_request_id = text("the first start", &first, "requestId");

    for case in ["the first retry", "the second retry"] {
        let (status, repeat) = start_pipeline(&router, REGION, &pipeline, "one-token", &[]).await;
        let repeat = assert_ok(case, status, repeat);
        assert_eq!(
            text(case, &repeat, "imageBuildVersionArn"),
            first_arn,
            "case {case:?}: expected the original build version"
        );
        assert_eq!(
            text(case, &repeat, "requestId"),
            first_request_id,
            "case {case:?}: expected the original response, down to its requestId"
        );
    }

    // A fresh token queues the SECOND build, so neither retry filed a build version of
    // its own.
    let (status, next) = start_pipeline(&router, REGION, &pipeline, "another-token", &[]).await;
    let next = assert_ok("a fresh token", status, next);
    let expected = image_arn(REGION, "retry-recipe", "1.0.0", 2);
    let actual = text("a fresh token", &next, "imageBuildVersionArn");
    assert_eq!(
        actual, expected,
        "case \"a fresh token\": expected {expected:?}, got {actual:?}"
    );

    // The same token with different parameters is a mismatch, whichever parameter differs.
    for (case, tags) in [
        ("one token, one tag", &[("Environment", "dev")][..]),
        (
            "one token, other tags",
            &[("Environment", "dev"), ("Owner", "platform")][..],
        ),
    ] {
        let (status, body) = start_pipeline(&router, REGION, &pipeline, "one-token", tags).await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "IdempotentParameterMismatchException",
            "The request parameters do not match those in the original idempotent request.",
        );
    }
}

/// The pipeline read REPORTS its runs: the date and status of its last run come from the
/// newest build version that names it, and that status moves with the clock.
#[tokio::test]
async fn the_pipeline_reports_the_run_it_last_started() {
    let router = emulator();
    let pipeline = pipeline_over_a_recipe(&router, "reported").await;

    // A pipeline nothing has started reports neither of the two.
    let before = read_pipeline("before any run", &router, REGION, &pipeline).await;
    for member in ["dateLastRun", "lastRunStatus"] {
        assert!(
            before.get(member).is_none(),
            "case \"before any run\": expected no {member}, got {before}"
        );
    }
    let created = text("before any run", &before, "dateCreated");

    let (status, run) = start_pipeline(&router, REGION, &pipeline, "reported-run", &[]).await;
    assert_ok("the first run", status, run);

    let queued = read_pipeline("just after the run", &router, REGION, &pipeline).await;
    let first_run_at = text("just after the run", &queued, "dateLastRun");
    assert!(
        first_run_at >= created,
        "case \"just after the run\": expected the run's date at or after the pipeline's \
         dateCreated {created:?}, got {first_run_at:?}"
    );
    assert_eq!(
        text("just after the run", &queued, "lastRunStatus"),
        QUEUED_STATUS,
        "case \"just after the run\": expected the queued status in {queued}"
    );

    // Nothing has to be polled for the build to finish: the span the queued build was
    // given is what the status is read against.
    tokio::time::sleep(std::time::Duration::from_millis(3_200)).await;
    let settled = read_pipeline("once the build is due", &router, REGION, &pipeline).await;
    assert_eq!(
        text("once the build is due", &settled, "lastRunStatus"),
        AVAILABLE_STATUS,
        "case \"once the build is due\": expected the available status in {settled}"
    );
    assert_eq!(
        text("once the build is due", &settled, "dateLastRun"),
        first_run_at,
        "case \"once the build is due\": expected the same run's date, which nothing changed"
    );

    // A second run is the one reported from then on, queued again.
    let (status, run) = start_pipeline(&router, REGION, &pipeline, "reported-run-2", &[]).await;
    assert_ok("the second run", status, run);
    let after = read_pipeline("after the second run", &router, REGION, &pipeline).await;
    let second_run_at = text("after the second run", &after, "dateLastRun");
    assert!(
        second_run_at >= first_run_at,
        "case \"after the second run\": expected a date at or after the first run's \
         {first_run_at:?}, got {second_run_at:?}"
    );
    assert_eq!(
        text("after the second run", &after, "lastRunStatus"),
        QUEUED_STATUS,
        "case \"after the second run\": expected the newest run's queued status in {after}"
    );
}

/// The recipe a run builds is resolved AT THE START: a reference whose trailing version
/// nodes are `x` wildcards reaches the newest matching recipe, and an exact reference
/// reaches the one it names.
#[tokio::test]
async fn a_run_builds_the_recipe_its_reference_reaches() {
    let router = emulator();
    let component = create_component(&router, REGION, "wild-component").await;
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        create_recipe(&router, REGION, "wild-recipe", version, &component).await;
    }
    let configuration = create_configuration(&router, REGION, "wild-infra").await;

    // The rows differ in which version nodes the reference pins and which it wildcards.
    // The expected version is the newest recipe the reference reaches, which only the
    // stored recipes can decide.
    for (case, reference_version, expected_version) in [
        ("an exact version", "1.0.0", "1.0.0"),
        ("the newest exact version", "1.2.0", "1.2.0"),
        ("a patch wildcard", "1.1.x", "1.1.0"),
        ("a minor wildcard", "1.x.x", "1.2.0"),
        ("every node wildcarded", "x.x.x", "1.2.0"),
    ] {
        let pipeline = create_pipeline(
            &router,
            REGION,
            &format!("wild-{reference_version}-pipeline").replace('.', "-"),
            &recipe_arn(REGION, "wild-recipe", reference_version),
            &configuration,
        )
        .await;
        let (status, body) =
            start_pipeline(&router, REGION, &pipeline, &format!("wild-{case}"), &[]).await;
        let body = assert_ok(case, status, body);
        let actual = text(case, &body, "imageBuildVersionArn");
        let expected_prefix = format!(
            "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/wild-recipe/{expected_version}/"
        );
        assert!(
            actual.starts_with(&expected_prefix),
            "case {case:?}: expected a build version of {expected_version}, got {actual:?}"
        );
    }
}

/// A create persists the pipeline and `GetImagePipeline` reports it back, with the
/// documented defaults filled in and the platform taken from the recipe.
#[tokio::test]
async fn a_created_pipeline_reads_back_with_its_defaults() {
    let router = emulator();
    let component = create_component(&router, REGION, "roundtrip-component").await;
    let recipe = create_recipe(
        &router,
        REGION,
        "roundtrip-recipe",
        "1.0.0",
        &component,
    )
    .await;
    let configuration = create_configuration(&router, REGION, "roundtrip-infra").await;

    // The rows differ in which optional members the create names, so each one checks both
    // what a stored value reports and what an omitted one defaults to.
    for (case, name, extra, status, enhanced, tests_enabled, timeout) in [
        (
            "every default left to the service",
            "plain pipeline",
            json!({}),
            "ENABLED",
            true,
            true,
            720,
        ),
        (
            "a disabled pipeline with its own test settings",
            "configured_pipeline",
            json!({
                "status": "DISABLED",
                "enhancedImageMetadataEnabled": false,
                "imageTestsConfiguration": { "imageTestsEnabled": false, "timeoutMinutes": 120 },
                "description": "the one the rows configure",
                "tags": { "Team": "platform" },
                "imageTags": { "Scheduled": "yes" },
                "schedule": { "scheduleExpression": "cron(0 9 * * ? *)", "timezone": "Etc/UTC" },
            }),
            "DISABLED",
            false,
            false,
            120,
        ),
        (
            "test settings that name only the timeout",
            "half configured pipeline",
            json!({ "imageTestsConfiguration": { "timeoutMinutes": 1440 } }),
            "ENABLED",
            true,
            true,
            1440,
        ),
    ] {
        let mut body = Map::new();
        body.insert("name".to_string(), json!(name));
        body.insert("clientToken".to_string(), json!(format!("create-{name}")));
        body.insert("imageRecipeArn".to_string(), json!(recipe));
        body.insert(
            "infrastructureConfigurationArn".to_string(),
            json!(configuration),
        );
        for (member, value) in extra.as_object().expect("an object of extra members") {
            body.insert(member.clone(), value.clone());
        }
        let (code, answer) = create_pipeline_with(&router, REGION, &Value::Object(body.clone())).await;
        let answer = assert_ok(case, code, answer);
        let arn = text(case, &answer, "imagePipelineArn");

        // The ARN carries a normalized form of the name: case folds down and a space or an
        // underscore is the same separator as a hyphen.
        let normalized: String = name
            .chars()
            .map(|c| match c {
                ' ' | '_' => '-',
                other => other.to_ascii_lowercase(),
            })
            .collect();
        assert_eq!(
            arn,
            pipeline_arn(REGION, &normalized),
            "case {case:?}: expected the ARN to carry the normalized name"
        );

        let stored = read_pipeline(case, &router, REGION, &arn).await;
        assert_eq!(
            text(case, &stored, "name"),
            name,
            "case {case:?}: expected the name as submitted"
        );
        assert_eq!(
            text(case, &stored, "arn"),
            arn,
            "case {case:?}: expected the pipeline's own ARN"
        );
        assert_eq!(
            text(case, &stored, "imageRecipeArn"),
            recipe,
            "case {case:?}: expected the recipe reference the create named"
        );
        assert_eq!(
            text(case, &stored, "platform"),
            "Linux",
            "case {case:?}: expected the platform inherited from the recipe in {stored}"
        );
        assert_eq!(
            text(case, &stored, "status"),
            status,
            "case {case:?}: expected the status {status} in {stored}"
        );
        assert_eq!(
            stored.get("enhancedImageMetadataEnabled"),
            Some(&json!(enhanced)),
            "case {case:?}: expected enhancedImageMetadataEnabled {enhanced} in {stored}"
        );
        assert_eq!(
            stored.get("imageTestsConfiguration"),
            Some(&json!({ "imageTestsEnabled": tests_enabled, "timeoutMinutes": timeout })),
            "case {case:?}: expected the stored test configuration in {stored}"
        );
        // A resource reports its own tags as a map on every read — an empty one when it
        // carries none — so a create that named none reads back an empty map.
        let expected_tags = extra.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            stored.get("tags"),
            Some(&expected_tags),
            "case {case:?}: expected tags {expected_tags} in {stored}"
        );
        for member in ["description", "imageTags", "schedule"] {
            assert_eq!(
                stored.get(member),
                extra.get(member),
                "case {case:?}: expected {member} reported exactly as the create named it"
            );
        }
        assert_eq!(
            stored.get("consecutiveFailures"),
            Some(&json!(0)),
            "case {case:?}: expected no consecutive scheduled failures in {stored}"
        );
        assert!(
            stored.get("dateNextRun").is_none(),
            "case {case:?}: expected no scheduled next run in {stored}"
        );
        assert!(
            stored.get("dateUpdated").is_none(),
            "case {case:?}: expected a pipeline nothing updated to report no dateUpdated in \
             {stored}"
        );
    }
}

/// A create is refused for what the model cannot say, and a reference it cannot resolve is
/// reported missing, naming the kind of resource the ARN identifies.
#[tokio::test]
async fn a_create_refuses_what_it_cannot_resolve() {
    let router = emulator();
    let component = create_component(&router, REGION, "refusal-component").await;
    let recipe = create_recipe(&router, REGION, "refusal-recipe", "1.0.0", &component).await;
    let configuration = create_configuration(&router, REGION, "refusal-infra").await;
    let absent_recipe = recipe_arn(REGION, "absent-recipe", "1.0.0");
    let absent_container =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:container-recipe/absent/1.0.0");
    let absent_configuration =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/absent");
    let absent_distribution =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:distribution-configuration/absent");
    let absent_workflow =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/build/absent/1.0.0/1");

    // Each row names the one thing wrong with it; everything else about the request is
    // what the round-trip case above stores.
    for (position, (case, extra, expected)) in [
        (
            "neither recipe named",
            json!({}),
            Err((
                400,
                "MissingParametersException",
                "Required parameters are missing from the request: 'imageRecipeArn' or \
                 'containerRecipeArn'. Verify that you have entered all of the required \
                 parameters in your request and try again."
                    .to_string(),
            )),
        ),
        (
            "both recipes named",
            json!({ "imageRecipeArn": recipe, "containerRecipeArn": absent_container }),
            Err((
                400,
                "InvalidRequestException",
                "The request must specify exactly one of imageRecipeArn and containerRecipeArn."
                    .to_string(),
            )),
        ),
        (
            "an image recipe nothing stored",
            json!({ "imageRecipeArn": absent_recipe }),
            Ok(("ImageRecipe", absent_recipe.clone())),
        ),
        (
            "a container recipe, which this store holds none of",
            json!({ "containerRecipeArn": absent_container }),
            Ok(("ContainerRecipe", absent_container.clone())),
        ),
        (
            "a distribution configuration, which this store holds none of",
            json!({
                "imageRecipeArn": recipe,
                "distributionConfigurationArn": absent_distribution,
            }),
            Ok(("DistributionConfiguration", absent_distribution.clone())),
        ),
        (
            "a workflow, which this store holds none of",
            json!({
                "imageRecipeArn": recipe,
                "executionRole": "ImageBuilderWorkflowRole",
                "workflows": [{ "workflowArn": absent_workflow }],
            }),
            Ok(("Workflow", absent_workflow.clone())),
        ),
        (
            "workflows without an execution role",
            json!({
                "imageRecipeArn": recipe,
                "workflows": [{ "workflowArn": absent_workflow }],
            }),
            Err((
                400,
                "InvalidRequestException",
                "The request must specify an executionRole when it specifies workflows."
                    .to_string(),
            )),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut body = Map::new();
        body.insert("name".to_string(), json!(format!("refused-pipeline-{position}")));
        body.insert("clientToken".to_string(), json!(format!("refused-{position}")));
        body.insert(
            "infrastructureConfigurationArn".to_string(),
            json!(configuration),
        );
        for (member, value) in extra.as_object().expect("an object of extra members") {
            body.insert(member.clone(), value.clone());
        }
        let (status, answer) = create_pipeline_with(&router, REGION, &Value::Object(body)).await;
        match expected {
            Ok((resource_type, named)) => {
                assert_missing(case, status, &answer, resource_type, &named);
            }
            Err((code, shape, message)) => {
                assert_refused(case, status, &answer, code, shape, &message);
            }
        }
    }

    // An infrastructure configuration nothing stored is reported the same way, and it is
    // the member a create cannot omit at all.
    let body = json!({
        "name": "refused-infrastructure",
        "clientToken": "refused-infrastructure",
        "imageRecipeArn": recipe,
        "infrastructureConfigurationArn": absent_configuration,
    });
    let (status, answer) = create_pipeline_with(&router, REGION, &body).await;
    assert_missing(
        "an infrastructure configuration nothing stored",
        status,
        &answer,
        "InfrastructureConfiguration",
        &absent_configuration,
    );

    // A pipeline whose name is already taken in this Region is refused as existing.
    let name = "refusal taken";
    for case in ["the first create of one name", "the second create of one name"] {
        let body = json!({
            "name": name,
            "clientToken": format!("taken-{case}"),
            "imageRecipeArn": recipe,
            "infrastructureConfigurationArn": configuration,
        });
        let (status, answer) = create_pipeline_with(&router, REGION, &body).await;
        if case == "the first create of one name" {
            assert_ok(case, status, answer);
        } else {
            assert_refused(
                case,
                status,
                &answer,
                400,
                "ResourceAlreadyExistsException",
                // The refusal names the LABEL AWS gives this kind — a `Pipeline`, not the
                // `ImagePipeline` its ARN spells — and the ARN of the pipeline that already
                // carries the normalized name.
                &format!(
                    "The following resource 'Pipeline' already exists: \
                     'arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/refusal-taken'."
                ),
            );
        }
    }
}

/// The build versions a pipeline's runs filed are what the listing reports: scoped to the
/// image version the request names, filtered on the members the operation documents, and
/// paged with a token of its own.
#[tokio::test]
async fn the_runs_a_pipeline_filed_are_listed() {
    let router = emulator();
    let pipeline = pipeline_over_a_recipe(&router, "listed").await;

    // Three runs, the middle one tagged, so the listing has something to order, page and
    // report tags from.
    let mut filed = Vec::new();
    for (position, tags) in [&[][..], &[("Environment", "dev")][..], &[][..]]
        .into_iter()
        .enumerate()
    {
        let case = format!("the run {position}");
        let (status, body) =
            start_pipeline(&router, REGION, &pipeline, &format!("listed-{position}"), tags).await;
        let body = assert_ok(&case, status, body);
        filed.push(text(&case, &body, "imageBuildVersionArn"));
    }
    let version_arn = format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/listed-recipe/1.0.0");

    // What ONE entry reports is read off the record the run filed.
    let (status, listed) = list_build_versions(
        &router,
        REGION,
        &json!({ "imageVersionArn": version_arn }),
    )
    .await;
    let listed = assert_ok("every build version of one image version", status, listed);
    assert_eq!(
        listed_arns("every build version of one image version", &listed),
        filed,
        "case \"every build version of one image version\": expected the three runs in \
         build-version order"
    );
    let summaries = listed
        .get("imageSummaryList")
        .and_then(Value::as_array)
        .expect("a summary list")
        .clone();
    for (position, summary) in summaries.iter().enumerate() {
        let case = format!("the summary of run {position}");
        for (member, expected) in [
            ("name", "listed-recipe"),
            ("version", "1.0.0"),
            ("platform", "Linux"),
            ("type", "AMI"),
            ("buildType", "USER_INITIATED"),
            ("owner", ACCOUNT),
        ] {
            let actual = text(&case, summary, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?}"
            );
        }
        let state = summary
            .get("state")
            .unwrap_or_else(|| panic!("case {case:?}: expected a state in {summary}"));
        assert_eq!(
            text(&case, state, "status"),
            QUEUED_STATUS,
            "case {case:?}: expected the queued status in {summary}"
        );
        assert!(
            !text(&case, summary, "dateCreated").is_empty(),
            "case {case:?}: expected the date the run was queued in {summary}"
        );
        // Only the middle run carried tags, and a build version reports exactly the ones
        // its own run named.
        let expected_tags = if position == 1 {
            tag_map(&[("Environment", "dev")])
        } else {
            BTreeMap::new()
        };
        assert_eq!(
            reported_tags(&case, summary),
            expected_tags,
            "case {case:?}: expected the run's own tags in {summary}"
        );
    }

    // The query rows: what each one reaches is decided by the stored records alone.
    let absent_version = format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/listed-recipe/9.9.9");
    for (case, query, expected) in [
        (
            "no image version named, so every build version of the Region",
            json!({}),
            filed.clone(),
        ),
        (
            "an image version nothing was built under",
            json!({ "imageVersionArn": absent_version }),
            Vec::new(),
        ),
        (
            "a filter on the name the recipe carries",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "name", "values": ["listed-recipe"] }],
            }),
            filed.clone(),
        ),
        (
            "a filter on a name nothing carries",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "name", "values": ["other-recipe"] }],
            }),
            Vec::new(),
        ),
        (
            "a filter on the platform the recipe's components target",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "platform", "values": ["Linux"] }],
            }),
            filed.clone(),
        ),
        (
            "a filter on the platform nothing targets",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "platform", "values": ["Windows"] }],
            }),
            Vec::new(),
        ),
        (
            "a filter on the image type a recipe build produces",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "type", "values": ["AMI"] }],
            }),
            filed.clone(),
        ),
        (
            "a filter on the other image type",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "type", "values": ["DOCKER"] }],
            }),
            Vec::new(),
        ),
        (
            "a filter on the version the recipe carries",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "version", "values": ["1.0.0"] }],
            }),
            filed.clone(),
        ),
        (
            "a filter on a version nothing carries",
            json!({
                "imageVersionArn": version_arn,
                "filters": [{ "name": "version", "values": ["2.0.0"] }],
            }),
            Vec::new(),
        ),
    ] {
        let (status, listed) = list_build_versions(&router, REGION, &query).await;
        let listed = assert_ok(case, status, listed);
        let actual = listed_arns(case, &listed);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // One page at a time, with the token carrying the position in this listing.
    let (status, first) = list_build_versions(
        &router,
        REGION,
        &json!({ "imageVersionArn": version_arn, "maxResults": 2 }),
    )
    .await;
    let first = assert_ok("the first page of two", status, first);
    assert_eq!(
        listed_arns("the first page of two", &first),
        filed[..2].to_vec(),
        "case \"the first page of two\": expected the first two build versions"
    );
    let token = text("the first page of two", &first, "nextToken");
    let (status, second) = list_build_versions(
        &router,
        REGION,
        &json!({ "imageVersionArn": version_arn, "maxResults": 2, "nextToken": token }),
    )
    .await;
    let second = assert_ok("the page after the token", status, second);
    assert_eq!(
        listed_arns("the page after the token", &second),
        filed[2..].to_vec(),
        "case \"the page after the token\": expected the build version the first page left"
    );
    assert!(
        second.get("nextToken").is_none(),
        "case \"the page after the token\": expected no token past the last entry in {second}"
    );

    // A filter this operation does not accept, and one carrying no values, are refused
    // rather than ignored.
    for (case, filter) in [
        (
            "a filter name the operation does not accept",
            json!({ "name": "owner", "values": ["Self"] }),
        ),
        (
            "a filter carrying no values",
            json!({ "name": "name", "values": [] }),
        ),
    ] {
        let (status, body) = list_build_versions(
            &router,
            REGION,
            &json!({ "imageVersionArn": version_arn, "filters": [filter] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }
}
