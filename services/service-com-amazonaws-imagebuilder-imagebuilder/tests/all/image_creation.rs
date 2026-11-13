// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreateImage`: the build a caller asks for by hand.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers), so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * `CreateImage` "creates a new image along with all configured output resources" and
//!   "the response returns as soon as Image Builder creates the new image resource": it
//!   resolves the ONE recipe the request names against the stored recipes of the calling
//!   Region, files an image build version whose name and three assigned version nodes come
//!   from that recipe and whose fourth node counts up per name+version pair, and reports
//!   that build version's ARN beside the wildcard references that reach its latest
//!   version. A later `ListImageBuildVersions` reports exactly the build versions it
//!   filed.
//! * The image build process "runs asynchronously": the status a build version reports
//!   moves from queued to available with the CLOCK, and nothing has to be polled for it.
//! * "You must specify exactly one recipe for your image, using either a
//!   containerRecipeArn or an imageRecipeArn", and "you must also provide an
//!   executionRole" when the request names workflows: a request that satisfies neither
//!   rule is refused as invalid and nothing is filed.
//! * A reference naming a resource this Region holds nothing under is answered with the
//!   not-found error this service models (`ResourceNotFoundException`, HTTP 404), whose
//!   message names the kind of resource the ARN identifies and the ARN the request
//!   carried, verbatim.
//! * `clientToken` is an idempotency token: a retry "returns the original response without
//!   running the operation again", and the same token carrying different parameters is a
//!   mismatch.
//!
//! The rows below span what each rule generalises over: which version nodes a recipe
//! reference pins and which it wildcards, how many builds one name+version pair has had,
//! which path queued them, how many tags a build carries, and how a reference resolves to
//! nothing (never created, another Region, a resource kind this store holds none of).

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

/// A request signed for `imagebuilder` in `region`: the signal the router routes on and
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

/// Assert the answer is a refusal carrying `expected` status, `code` and `message`.
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

/// `CreateImage` with `body`, in `region`.
async fn create_image_with(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "PUT", "/CreateImage", Some(body)).await
}

/// `CreateImage` naming `recipe_reference` and `configuration_arn`, with `token` and
/// `tags`, in `region`.
async fn create_image(
    router: &MultiServiceRouter,
    region: &str,
    recipe_reference: &str,
    configuration_arn: &str,
    token: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("imageRecipeArn".to_string(), json!(recipe_reference));
    body.insert(
        "infrastructureConfigurationArn".to_string(),
        json!(configuration_arn),
    );
    body.insert("clientToken".to_string(), json!(token));
    if !tags.is_empty() {
        body.insert("tags".to_string(), tag_object(tags));
    }
    create_image_with(router, region, &Value::Object(body)).await
}

/// `CreateImagePipeline` for `name` in `region`, answering the pipeline's own ARN.
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
    let (status, answer) = call_in(router, region, "PUT", "/CreateImagePipeline", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imagePipelineArn")
}

/// `StartImagePipelineExecution` for `arn` in `region`, with `token`.
async fn start_pipeline(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    token: &str,
) -> (u16, Value) {
    let body = json!({ "imagePipelineArn": arn, "clientToken": token });
    call_in(
        router,
        region,
        "PUT",
        "/StartImagePipelineExecution",
        Some(&body),
    )
    .await
}

/// The `imageSummaryList` the build versions of `image_version_arn` are listed as, in
/// `region`.
async fn list_build_versions(
    router: &MultiServiceRouter,
    region: &str,
    image_version_arn: &str,
) -> Vec<Value> {
    let body = json!({ "imageVersionArn": image_version_arn });
    let case = format!("list the build versions of {image_version_arn} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "POST",
        "/ListImageBuildVersions",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    answer
        .get("imageSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageSummaryList in {answer}"))
        .clone()
}

/// The ARNs the build versions of `image_version_arn` are listed under.
async fn listed_arns(
    router: &MultiServiceRouter,
    region: &str,
    image_version_arn: &str,
) -> Vec<String> {
    let case = format!("the build versions of {image_version_arn}");
    list_build_versions(router, region, image_version_arn)
        .await
        .iter()
        .map(|summary| text(&case, summary, "arn"))
        .collect()
}

/// `ListImagePackages` for `arn`, in `region`.
async fn list_packages(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    let body = json!({ "imageBuildVersionArn": arn });
    call_in(router, region, "POST", "/ListImagePackages", Some(&body)).await
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
                    .unwrap_or_else(|| panic!("case {case:?}: expected text in the tag {key}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The ARN of one image build version, as this service derives it.
fn image_arn(region: &str, name: &str, version: &str, build: u64) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image/{name}/{version}/{build}")
}

/// The ARN of one image version, which a build-version listing is asked for by.
fn image_version_arn(region: &str, name: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image/{name}/{version}")
}

/// The ARN of one image recipe reference, which may carry trailing `x` wildcards.
fn recipe_reference(region: &str, name: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:image-recipe/{name}/{version}")
}

/// A create resolves the recipe its reference reaches, and the build version it files
/// carries that recipe's name and assigned version with the next build number of the pair.
///
/// The rows span the reference forms a recipe ARN takes: an exact version, and a version
/// whose trailing nodes are `x` wildcards at each depth, which "always reference the latest
/// version of the resource".
#[tokio::test]
async fn a_create_queues_a_build_of_the_recipe_its_reference_reaches() {
    let router = emulator();
    let component = create_component(&router, REGION, "created-image-component").await;
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        create_recipe(&router, REGION, "created-image", version, &component).await;
    }
    let configuration = create_configuration(&router, REGION, "created-image-infra").await;

    // (the reference the request names, the recipe version it reaches, the build number
    // the build version gets, since the fourth node counts up per name+version pair)
    let cases: [(&str, &str, u64); 5] = [
        ("1.0.0", "1.0.0", 1),
        ("1.x.x", "1.2.0", 1),
        ("x.x.x", "1.2.0", 2),
        ("1.1.x", "1.1.0", 1),
        ("1.0.0", "1.0.0", 2),
    ];
    for (position, (reference, resolved, build)) in cases.into_iter().enumerate() {
        let case = format!("the reference {reference}");
        let token = format!("created-image-{position}");
        let (status, answer) = create_image(
            &router,
            REGION,
            &recipe_reference(REGION, "created-image", reference),
            &configuration,
            &token,
            &[],
        )
        .await;
        let answer = assert_ok(&case, status, answer);

        let expected = image_arn(REGION, "created-image", resolved, build);
        let actual = text(&case, &answer, "imageBuildVersionArn");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the build version {expected:?}, got {actual:?}"
        );
        let echoed = text(&case, &answer, "clientToken");
        assert_eq!(
            echoed, token,
            "case {case:?}: expected the request's client token {token:?}, got {echoed:?}"
        );
        assert!(
            !text(&case, &answer, "requestId").is_empty(),
            "case {case:?}: expected a request id in {answer}"
        );

        // "A set of wildcard version ARNs that always reference the latest version of the
        // resource", for the latest overall and within this image's major, minor and patch
        // levels: the build number is deliberately not carried.
        let [major, minor, patch] = ["", "", ""].map(String::from);
        let mut nodes = resolved.split('.');
        let (major, minor, patch) = (
            nodes.next().unwrap_or(&major).to_string(),
            nodes.next().unwrap_or(&minor).to_string(),
            nodes.next().unwrap_or(&patch).to_string(),
        );
        let references = answer
            .get("latestVersionReferences")
            .unwrap_or_else(|| panic!("case {case:?}: expected latestVersionReferences in {answer}"));
        for (member, version) in [
            ("latestVersionArn", "x.x.x".to_string()),
            ("latestMajorVersionArn", format!("{major}.x.x")),
            ("latestMinorVersionArn", format!("{major}.{minor}.x")),
            ("latestPatchVersionArn", format!("{major}.{minor}.{patch}")),
        ] {
            let expected = image_version_arn(REGION, "created-image", &version);
            let actual = text(&case, references, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?}"
            );
        }

        // The build version is really filed: the listing of the image version it belongs
        // to reports it, and the summary reports what the recipe and this build say.
        let listed = list_build_versions(&router, REGION, &image_version_arn(REGION, "created-image", resolved)).await;
        let summary = listed
            .iter()
            .find(|summary| text(&case, summary, "arn") == expected)
            .unwrap_or_else(|| panic!("case {case:?}: expected {expected:?} among {listed:?}"));
        for (member, value) in [
            ("name", "created-image"),
            ("version", resolved),
            ("platform", "Linux"),
            ("type", "AMI"),
            ("buildType", "USER_INITIATED"),
            ("owner", ACCOUNT),
        ] {
            let actual = text(&case, summary, member);
            assert_eq!(
                actual, value,
                "case {case:?}: expected {member} {value:?}, got {actual:?}"
            );
        }
        let state = summary
            .get("state")
            .unwrap_or_else(|| panic!("case {case:?}: expected a state in {summary}"));
        assert_eq!(
            text(&case, state, "status"),
            QUEUED_STATUS,
            "case {case:?}: expected the just-queued status in {summary}"
        );
        assert!(
            summary.get("sourcePipelineArn").is_none(),
            "case {case:?}: expected no sourcePipelineArn on a build no pipeline ran, in {summary}"
        );
        assert!(
            !text(&case, summary, "dateCreated").is_empty(),
            "case {case:?}: expected the date the build was queued in {summary}"
        );

        // The build only just started, so nothing has been inventoried on a build
        // instance: the build version reports an empty package inventory rather than one
        // nothing took.
        let (status, packages) = list_packages(&router, REGION, &expected).await;
        let packages = assert_ok(&case, status, packages);
        let inventory = packages
            .get("imagePackageList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected an imagePackageList in {packages}"));
        assert!(
            inventory.is_empty(),
            "case {case:?}: expected no packages for a build that only just started, got \
             {inventory:?}"
        );
    }

    // Each name+version pair counts its own builds, and a build version belongs to exactly
    // one of them.
    for (version, builds) in [("1.0.0", 2_u64), ("1.1.0", 1), ("1.2.0", 2)] {
        let case = format!("the builds of version {version}");
        let expected: Vec<String> = (1..=builds)
            .map(|build| image_arn(REGION, "created-image", version, build))
            .collect();
        let actual = listed_arns(&router, REGION, &image_version_arn(REGION, "created-image", version)).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the build versions {expected:?}, got {actual:?}"
        );
    }
}

/// The fourth node counts up across BOTH paths that queue a build of one image: the one a
/// caller asks for by hand and the one a pipeline execution starts.
#[tokio::test]
async fn builds_of_one_image_count_up_whichever_path_queued_them() {
    let router = emulator();
    let component = create_component(&router, REGION, "shared-count-component").await;
    let recipe = create_recipe(&router, REGION, "shared-count", "2.3.4", &component).await;
    let configuration = create_configuration(&router, REGION, "shared-count-infra").await;
    let pipeline = create_pipeline(
        &router,
        REGION,
        "shared-count-pipeline",
        &recipe,
        &configuration,
    )
    .await;

    let mut expected = Vec::new();
    for (position, queued_by_pipeline) in [false, true, false, true].into_iter().enumerate() {
        let case = format!("the build at position {position}");
        let token = format!("shared-count-{position}");
        let (status, answer) = if queued_by_pipeline {
            start_pipeline(&router, REGION, &pipeline, &token).await
        } else {
            create_image(&router, REGION, &recipe, &configuration, &token, &[]).await
        };
        let answer = assert_ok(&case, status, answer);
        let build = u64::try_from(position).expect("a build number") + 1;
        let wanted = image_arn(REGION, "shared-count", "2.3.4", build);
        let actual = text(&case, &answer, "imageBuildVersionArn");
        assert_eq!(
            actual, wanted,
            "case {case:?}: expected the build version {wanted:?}, got {actual:?}"
        );
        expected.push(wanted);
    }

    let listed = listed_arns(
        &router,
        REGION,
        &image_version_arn(REGION, "shared-count", "2.3.4"),
    )
    .await;
    assert_eq!(
        listed, expected,
        "case \"every build of the image\": expected {expected:?}, got {listed:?}"
    );
}

/// The build a create queues is OUTSTANDING: the status it reports moves to available with
/// the clock, and nothing has to be polled for it.
#[tokio::test]
async fn a_queued_build_becomes_available_with_the_clock() {
    let router = emulator();
    let component = create_component(&router, REGION, "clocked-component").await;
    let recipe = create_recipe(&router, REGION, "clocked", "1.0.0", &component).await;
    let configuration = create_configuration(&router, REGION, "clocked-infra").await;

    let (status, answer) =
        create_image(&router, REGION, &recipe, &configuration, "clocked", &[]).await;
    let answer = assert_ok("the queued build", status, answer);
    let arn = text("the queued build", &answer, "imageBuildVersionArn");
    let version = image_version_arn(REGION, "clocked", "1.0.0");

    for (case, expected, settled) in [
        ("just after the create", QUEUED_STATUS, false),
        ("once the build is due", AVAILABLE_STATUS, true),
    ] {
        if settled {
            tokio::time::sleep(std::time::Duration::from_millis(3_200)).await;
        }
        let listed = list_build_versions(&router, REGION, &version).await;
        let summary = listed
            .iter()
            .find(|summary| text(case, summary, "arn") == arn)
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn:?} among {listed:?}"));
        let state = summary
            .get("state")
            .unwrap_or_else(|| panic!("case {case:?}: expected a state in {summary}"));
        let actual = text(case, state, "status");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the status {expected:?}, got {actual:?}"
        );
    }
}

/// The image a create files carries exactly the tags that request named, and a later read
/// of its tags reports them.
#[tokio::test]
async fn a_created_image_carries_the_tags_its_request_named() {
    let router = emulator();
    let component = create_component(&router, REGION, "tagged-build-component").await;
    let recipe = create_recipe(&router, REGION, "tagged-build", "1.0.0", &component).await;
    let configuration = create_configuration(&router, REGION, "tagged-build-infra").await;

    let cases: [&[(&str, &str)]; 3] = [
        &[],
        &[("Environment", "dev")],
        &[("Environment", "prod"), ("Owner", "platform"), ("Tier", "1")],
    ];
    for (position, tags) in cases.into_iter().enumerate() {
        let case = format!("a build carrying {} tag(s)", tags.len());
        let (status, answer) = create_image(
            &router,
            REGION,
            &recipe,
            &configuration,
            &format!("tagged-build-{position}"),
            tags,
        )
        .await;
        let answer = assert_ok(&case, status, answer);
        let arn = text(&case, &answer, "imageBuildVersionArn");

        let (status, listed) = list_tags(&router, REGION, &arn).await;
        let listed = assert_ok(&case, status, listed);
        let actual = reported_tags(&case, &listed);
        let expected = tag_map(tags);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the tags {expected:?}, got {actual:?}"
        );
    }
}

/// A retry of one request answers the original response and queues NO second build, while
/// the same token carrying different parameters is a mismatch.
#[tokio::test]
async fn one_client_token_queues_one_build() {
    let router = emulator();
    let component = create_component(&router, REGION, "token-build-component").await;
    let recipe = create_recipe(&router, REGION, "token-build", "1.0.0", &component).await;
    let other = create_recipe(&router, REGION, "token-build-other", "1.0.0", &component).await;
    let configuration = create_configuration(&router, REGION, "token-build-infra").await;
    let version = image_version_arn(REGION, "token-build", "1.0.0");

    let (status, first) =
        create_image(&router, REGION, &recipe, &configuration, "one-token", &[]).await;
    let first = assert_ok("the first create", status, first);

    let (status, retried) =
        create_image(&router, REGION, &recipe, &configuration, "one-token", &[]).await;
    let retried = assert_ok("the retry", status, retried);
    for member in ["imageBuildVersionArn", "clientToken", "requestId"] {
        let original = text("the first create", &first, member);
        let repeated = text("the retry", &retried, member);
        assert_eq!(
            repeated, original,
            "case \"the retry\": expected the original {member} {original:?}, got {repeated:?}"
        );
    }
    let listed = listed_arns(&router, REGION, &version).await;
    assert_eq!(
        listed,
        vec![text("the first create", &first, "imageBuildVersionArn")],
        "case \"the retry\": expected the one build the first call queued, got {listed:?}"
    );

    // The same token with different creation parameters is the mismatch this service
    // models, and still nothing more is queued.
    let (status, mismatch) =
        create_image(&router, REGION, &other, &configuration, "one-token", &[]).await;
    assert_refused(
        "a mismatched retry",
        status,
        &mismatch,
        400,
        "IdempotentParameterMismatchException",
        "The request parameters do not match those in the original idempotent request.",
    );
    let listed = listed_arns(&router, REGION, &version).await;
    assert_eq!(
        listed.len(),
        1,
        "case \"a mismatched retry\": expected the one build already queued, got {listed:?}"
    );

    // A token of its own queues a build of its own.
    let (status, second) = create_image(
        &router,
        REGION,
        &recipe,
        &configuration,
        "another-token",
        &[],
    )
    .await;
    let second = assert_ok("a second token", status, second);
    let expected = image_arn(REGION, "token-build", "1.0.0", 2);
    let actual = text("a second token", &second, "imageBuildVersionArn");
    assert_eq!(
        actual, expected,
        "case \"a second token\": expected the build version {expected:?}, got {actual:?}"
    );
}

/// A request naming an ARN this Region holds no resource under is reported MISSING, and
/// nothing is filed for it.
#[tokio::test]
async fn a_reference_naming_nothing_is_reported_missing() {
    let router = emulator();
    let component = create_component(&router, REGION, "missing-case-component").await;
    let recipe = create_recipe(&router, REGION, "missing-case", "1.0.0", &component).await;
    let configuration = create_configuration(&router, REGION, "missing-case-infra").await;
    // The same resources in another Region, which this Region's endpoint cannot reach.
    let elsewhere_component = create_component(&router, OTHER_REGION, "elsewhere-component").await;
    let elsewhere_recipe = create_recipe(
        &router,
        OTHER_REGION,
        "elsewhere",
        "1.0.0",
        &elsewhere_component,
    )
    .await;
    let elsewhere_configuration =
        create_configuration(&router, OTHER_REGION, "elsewhere-infra").await;

    // (what the request names, which ARN it is reported missing by, that ARN's kind)
    let cases: [(&str, Value, String, &str); 6] = [
        (
            "an infrastructure configuration nothing created",
            json!({
                "imageRecipeArn": recipe.clone(),
                "infrastructureConfigurationArn":
                    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/absent"),
                "clientToken": "missing-configuration",
            }),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/absent"),
            "InfrastructureConfiguration",
        ),
        (
            "an infrastructure configuration of another Region",
            json!({
                "imageRecipeArn": recipe.clone(),
                "infrastructureConfigurationArn": elsewhere_configuration.clone(),
                "clientToken": "other-region-configuration",
            }),
            elsewhere_configuration.clone(),
            "InfrastructureConfiguration",
        ),
        (
            "a recipe nothing created",
            json!({
                "imageRecipeArn": recipe_reference(REGION, "absent-recipe", "1.0.0"),
                "infrastructureConfigurationArn": configuration.clone(),
                "clientToken": "missing-recipe",
            }),
            recipe_reference(REGION, "absent-recipe", "1.0.0"),
            "ImageRecipe",
        ),
        (
            "a recipe version no stored recipe carries",
            json!({
                "imageRecipeArn": recipe_reference(REGION, "missing-case", "9.x.x"),
                "infrastructureConfigurationArn": configuration.clone(),
                "clientToken": "missing-recipe-version",
            }),
            recipe_reference(REGION, "missing-case", "9.x.x"),
            "ImageRecipe",
        ),
        (
            "a recipe of another Region",
            json!({
                "imageRecipeArn": elsewhere_recipe.clone(),
                "infrastructureConfigurationArn": configuration.clone(),
                "clientToken": "other-region-recipe",
            }),
            elsewhere_recipe.clone(),
            "ImageRecipe",
        ),
        (
            "a container recipe, a resource kind this store holds none of",
            json!({
                "containerRecipeArn":
                    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:container-recipe/boxed/1.0.0"),
                "infrastructureConfigurationArn": configuration.clone(),
                "clientToken": "container-recipe",
            }),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:container-recipe/boxed/1.0.0"),
            "ContainerRecipe",
        ),
    ];
    for (case, body, arn, resource_type) in cases {
        let (status, answer) = create_image_with(&router, REGION, &body).await;
        assert_missing(case, status, &answer, resource_type, &arn);
    }

    // A distribution configuration is a resource of this service too, and this store holds
    // none under this ARN, so a reference to it names nothing either.
    let distribution =
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:distribution-configuration/spread");
    let (status, answer) = create_image_with(
        &router,
        REGION,
        &json!({
            "imageRecipeArn": recipe.clone(),
            "infrastructureConfigurationArn": configuration.clone(),
            "distributionConfigurationArn": distribution.clone(),
            "clientToken": "distribution-configuration",
        }),
    )
    .await;
    assert_missing(
        "a distribution configuration nothing created",
        status,
        &answer,
        "DistributionConfiguration",
        &distribution,
    );

    // Not one of those requests queued a build: a refusal files nothing, and the token it
    // carried stays free.
    let listed = listed_arns(
        &router,
        REGION,
        &image_version_arn(REGION, "missing-case", "1.0.0"),
    )
    .await;
    assert!(
        listed.is_empty(),
        "case \"after every refusal\": expected no build version filed, got {listed:?}"
    );
}

/// A request that does not name exactly one recipe, or names workflows without the role
/// they run under, is refused as INVALID rather than filed.
#[tokio::test]
async fn a_request_the_model_cannot_bound_is_refused_as_invalid() {
    let router = emulator();
    let component = create_component(&router, REGION, "invalid-case-component").await;
    let recipe = create_recipe(&router, REGION, "invalid-case", "1.0.0", &component).await;
    let configuration = create_configuration(&router, REGION, "invalid-case-infra").await;
    let container = format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:container-recipe/boxed/1.0.0");
    let workflow = format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/build/flow/1.0.0");

    let cases: [(&str, Value, &str, &str); 3] = [
        (
            "neither recipe",
            json!({
                "infrastructureConfigurationArn": configuration.clone(),
                "clientToken": "no-recipe",
            }),
            // A request that named NONE of the two is missing a required parameter, which
            // is a different report from one that named both.
            "MissingParametersException",
            "Required parameters are missing from the request: 'imageRecipeArn' or \
             'containerRecipeArn'. Verify that you have entered all of the required \
             parameters in your request and try again.",
        ),
        (
            "both recipes",
            json!({
                "imageRecipeArn": recipe.clone(),
                "containerRecipeArn": container.clone(),
                "infrastructureConfigurationArn": configuration.clone(),
                "clientToken": "both-recipes",
            }),
            "InvalidRequestException",
            "The request must specify exactly one of imageRecipeArn and containerRecipeArn.",
        ),
        (
            "workflows without the role they run under",
            json!({
                "imageRecipeArn": recipe.clone(),
                "infrastructureConfigurationArn": configuration.clone(),
                "workflows": [{ "workflowArn": workflow.clone() }],
                "clientToken": "workflows-without-role",
            }),
            "InvalidRequestException",
            "The request must specify an executionRole when it specifies workflows.",
        ),
    ];
    for (case, body, code, message) in cases {
        let (status, answer) = create_image_with(&router, REGION, &body).await;
        assert_refused(case, status, &answer, 400, code, message);
    }

    let listed = listed_arns(
        &router,
        REGION,
        &image_version_arn(REGION, "invalid-case", "1.0.0"),
    )
    .await;
    assert!(
        listed.is_empty(),
        "case \"after every refusal\": expected no build version filed, got {listed:?}"
    );
}

/// A regional endpoint reaches only its own Region's builds: the same recipe name built in
/// two Regions is two images, each listed by its own endpoint alone.
#[tokio::test]
async fn a_build_belongs_to_the_region_that_queued_it() {
    let router = emulator();
    for region in [REGION, OTHER_REGION] {
        let component = create_component(&router, region, "regional-component").await;
        let recipe = create_recipe(&router, region, "regional", "1.0.0", &component).await;
        let configuration = create_configuration(&router, region, "regional-infra").await;
        let case = format!("the build queued in {region}");
        let (status, answer) = create_image(
            &router,
            region,
            &recipe,
            &configuration,
            "regional-token",
            &[],
        )
        .await;
        let answer = assert_ok(&case, status, answer);
        let expected = image_arn(region, "regional", "1.0.0", 1);
        let actual = text(&case, &answer, "imageBuildVersionArn");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the build version {expected:?}, got {actual:?}"
        );
    }

    for (region, other) in [(REGION, OTHER_REGION), (OTHER_REGION, REGION)] {
        let case = format!("what {region} reaches");
        let listed = listed_arns(
            &router,
            region,
            &image_version_arn(region, "regional", "1.0.0"),
        )
        .await;
        assert_eq!(
            listed,
            vec![image_arn(region, "regional", "1.0.0", 1)],
            "case {case:?}: expected only its own Region's build, got {listed:?}"
        );
        let across = listed_arns(
            &router,
            region,
            &image_version_arn(other, "regional", "1.0.0"),
        )
        .await;
        assert!(
            across.is_empty(),
            "case {case:?}: expected no build of {other} to be reachable, got {across:?}"
        );
    }
}
