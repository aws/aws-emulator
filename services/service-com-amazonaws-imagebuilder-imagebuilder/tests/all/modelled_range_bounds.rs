// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service answers an integer outside the `@range` its
//! member's modelled target declares.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door and the modelled-constraint walk behind it — so
//! what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a value outside a member's modelled range is refused as that
//! parameter's value — HTTP 400 `InvalidParameterValueException`, with one sentence naming
//! the parameter as the API addresses it and the bound its value had to satisfy — whichever
//! operation carries the member and whichever end of the range the value missed; a value AT
//! either published bound is admitted, and an operation that stores it reports it back
//! unchanged.
//!
//! The rows span what the rule generalises over: which member carries the range (a test
//! timeout, a page size, three EBS device settings), which operation the request addresses
//! (two creates, an update, two lists), how deep in the request the member sits (a top-level
//! member, a member of a structure, a member inside a list element, at two different list
//! indices), and which end of the range the value missed — each range tested at both
//! published bounds and at one step past each of them.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// A component document with one build phase, so the recipe the setup creates reports a
/// platform.
const BUILD_DOCUMENT: &str = "name: Probe\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: run\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hi\n";

// The modelled ranges under test, from
// `smithy_query(query="shape",shape="ImageTestsTimeoutMinutes,RestrictedInteger,EbsInstanceBlockDeviceSpecification")`:
// the test timeout, the page size, and the EBS volume size, IOPS and throughput.
/// `ImageTestsConfiguration.timeoutMinutes` -> `ImageTestsTimeoutMinutes`.
const TIMEOUT_RANGE: (i64, i64) = (60, 1440);
/// A list operation's `maxResults` -> `RestrictedInteger`.
const PAGE_SIZE_RANGE: (i64, i64) = (1, 100);
/// `EbsInstanceBlockDeviceSpecification.volumeSize` -> `EbsVolumeSizeInteger`.
const VOLUME_SIZE_RANGE: (i64, i64) = (1, 16000);
/// `EbsInstanceBlockDeviceSpecification.iops` -> `EbsIopsInteger`.
const IOPS_RANGE: (i64, i64) = (100, 64000);
/// `EbsInstanceBlockDeviceSpecification.throughput` -> `EbsVolumeThroughput`.
const THROUGHPUT_RANGE: (i64, i64) = (125, 1000);

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

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on and
/// the handlers read the Region from.
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

/// Status and parsed body of one call, with the error shape the header names folded into
/// `__type` so one accessor reads either place it travels.
async fn call(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
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
            .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {raw:?}"))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape an answer names, without its namespace, or the empty string when it
/// names none.
fn error_code(body: &Value) -> String {
    let rendered = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    rendered.rsplit('#').next().unwrap_or(rendered).to_string()
}

/// Read `name` out of an answer, panicking with the case when it is absent.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// The sentence this service states for `parameter`, whose value had to satisfy
/// `requirement`.
fn expected_message(parameter: &str, requirement: &str) -> String {
    format!("The value supplied for parameter '{parameter}' is not valid. {requirement}")
}

/// A request body carrying one `value` at the member under test, under one client token.
type BodyFor<'a> = Box<dyn Fn(i64, &str) -> Value + 'a>;

/// How one operation carries the member under test: the request it is addressed with, built
/// around the `value` that member holds and the client token the call signs for.
struct Carrier<'a> {
    /// The operation the case addresses, for the case label.
    operation: &'static str,
    method: &'static str,
    uri: &'static str,
    /// The body carrying `value` at the member under test, under one client token.
    body: BodyFor<'a>,
    /// Whether this operation SERVES a request whose value the model admits (an
    /// implemented operation answers it 200); an operation still scaffolded only has to get
    /// past the parameter-value refusal.
    serves: bool,
}

/// One member's modelled range and every operation that carries it.
struct RangeCase<'a> {
    /// The parameter as the API addresses it — the path the refusal names.
    parameter: &'static str,
    range: (i64, i64),
    carriers: Vec<Carrier<'a>>,
}

/// `CreateComponent` for `name`, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let case = format!("create the component {name}");
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let (status, answer) = call(router, &case, "PUT", "/CreateComponent", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answer}"
    );
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name`, referencing `component_arn`, answering the recipe's ARN.
async fn create_recipe(router: &MultiServiceRouter, name: &str, component_arn: &str) -> String {
    let case = format!("create the image recipe {name}");
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{name}"),
        "components": [{ "componentArn": component_arn }],
    });
    let (status, answer) = call(router, &case, "PUT", "/CreateImageRecipe", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answer}"
    );
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let case = format!("create the infrastructure configuration {name}");
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let (status, answer) = call(
        router,
        &case,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answer}"
    );
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// A `CreateImageRecipe` body whose first block device mapping is one the model admits and
/// whose SECOND one holds `value` at `member` — so an EBS case is checked inside a list
/// element, at an index the element before it does not occupy.
fn recipe_with_device_setting(member: &str, value: i64, token: &str) -> Value {
    json!({
        "name": format!("device-{member}-{token}"),
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": token,
        "blockDeviceMappings": [
            { "deviceName": "/dev/xvda", "ebs": { "volumeSize": 8 } },
            { "deviceName": "/dev/xvdb", "ebs": { member: value } },
        ],
    })
}

/// Every member whose modelled range is under test, with the operations that carry it. The
/// setup resources are the ones a create that the model admits has to reference.
fn range_cases<'a>(recipe: &'a str, configuration: &'a str, pipeline: &'a str) -> Vec<RangeCase<'a>> {
    vec![
        RangeCase {
            parameter: "imageTestsConfiguration.timeoutMinutes",
            range: TIMEOUT_RANGE,
            carriers: vec![
                Carrier {
                    operation: "CreateImagePipeline",
                    method: "PUT",
                    uri: "/CreateImagePipeline",
                    body: Box::new(move |value, token| {
                        json!({
                            "name": format!("pipeline-{token}"),
                            "clientToken": token,
                            "imageRecipeArn": recipe,
                            "infrastructureConfigurationArn": configuration,
                            "imageTestsConfiguration": {
                                "imageTestsEnabled": true,
                                "timeoutMinutes": value,
                            },
                        })
                    }),
                    serves: true,
                },
                Carrier {
                    operation: "CreateImage",
                    method: "PUT",
                    uri: "/CreateImage",
                    body: Box::new(move |value, token| {
                        json!({
                            "clientToken": token,
                            "imageRecipeArn": recipe,
                            "infrastructureConfigurationArn": configuration,
                            "imageTestsConfiguration": { "timeoutMinutes": value },
                        })
                    }),
                    serves: true,
                },
                Carrier {
                    // Still scaffolded, so an admitted value is not served — but the
                    // refusal of one the model does not admit is the door's, ahead of it.
                    operation: "UpdateImagePipeline",
                    method: "PUT",
                    uri: "/UpdateImagePipeline",
                    body: Box::new(move |value, token| {
                        json!({
                            "imagePipelineArn": pipeline,
                            "clientToken": token,
                            "infrastructureConfigurationArn": configuration,
                            "imageTestsConfiguration": { "timeoutMinutes": value },
                        })
                    }),
                    serves: false,
                },
            ],
        },
        RangeCase {
            parameter: "maxResults",
            range: PAGE_SIZE_RANGE,
            carriers: vec![
                Carrier {
                    operation: "ListImagePipelines",
                    method: "POST",
                    uri: "/ListImagePipelines",
                    body: Box::new(|value, _| json!({ "maxResults": value })),
                    serves: true,
                },
                Carrier {
                    operation: "ListImageRecipes",
                    method: "POST",
                    uri: "/ListImageRecipes",
                    body: Box::new(|value, _| json!({ "maxResults": value })),
                    serves: true,
                },
            ],
        },
        RangeCase {
            parameter: "blockDeviceMappings[1].ebs.volumeSize",
            range: VOLUME_SIZE_RANGE,
            carriers: vec![Carrier {
                operation: "CreateImageRecipe",
                method: "PUT",
                uri: "/CreateImageRecipe",
                body: Box::new(|value, token| {
                    recipe_with_device_setting("volumeSize", value, token)
                }),
                serves: true,
            }],
        },
        RangeCase {
            parameter: "blockDeviceMappings[1].ebs.iops",
            range: IOPS_RANGE,
            carriers: vec![Carrier {
                operation: "CreateImageRecipe",
                method: "PUT",
                uri: "/CreateImageRecipe",
                body: Box::new(|value, token| recipe_with_device_setting("iops", value, token)),
                serves: true,
            }],
        },
        RangeCase {
            parameter: "blockDeviceMappings[1].ebs.throughput",
            range: THROUGHPUT_RANGE,
            carriers: vec![Carrier {
                operation: "CreateImageRecipe",
                method: "PUT",
                uri: "/CreateImageRecipe",
                body: Box::new(|value, token| {
                    recipe_with_device_setting("throughput", value, token)
                }),
                serves: true,
            }],
        },
    ]
}

/// The setup every case references: a recipe, an infrastructure configuration and one stored
/// pipeline (for the update that names one).
async fn setup(router: &MultiServiceRouter) -> (String, String, String) {
    let component = create_component(router, "range-component").await;
    let recipe = create_recipe(router, "range-recipe", &component).await;
    let configuration = create_configuration(router, "range-infra").await;
    let case = "create the pipeline an update names";
    let body = json!({
        "name": "range-pipeline",
        "clientToken": "pipeline-range",
        "imageRecipeArn": recipe,
        "infrastructureConfigurationArn": configuration,
    });
    let (status, answer) = call(router, case, "PUT", "/CreateImagePipeline", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answer}"
    );
    let pipeline = text(case, &answer, "imagePipelineArn");
    (recipe, configuration, pipeline)
}

#[tokio::test]
async fn a_value_past_a_modelled_range_bound_is_refused_as_that_parameters_value() {
    let router = emulator();
    let (recipe, configuration, pipeline) = setup(&router).await;

    let mut token = 0usize;
    for case in range_cases(&recipe, &configuration, &pipeline) {
        let (min, max) = case.range;
        // One step past each published bound, at both ends of the range.
        let past = [
            (
                min - 1,
                format!("{} must be bigger than or equal to {min}", case.parameter),
            ),
            (
                max + 1,
                format!("{} must be smaller than or equal to {max}", case.parameter),
            ),
        ];
        for carrier in &case.carriers {
            for (value, requirement) in &past {
                token += 1;
                let label = format!(
                    "{} = {value}, one step past the bound, on {}",
                    case.parameter, carrier.operation
                );
                let body = (carrier.body)(*value, &format!("range-{token:04}"));
                let (status, answer) =
                    call(&router, &label, carrier.method, carrier.uri, Some(&body)).await;
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected HTTP 400, got {status} with {answer}"
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, INVALID_PARAMETER_VALUE,
                    "case {label:?}: expected {INVALID_PARAMETER_VALUE}, got {code:?} with \
                     {answer}"
                );
                let expected = expected_message(case.parameter, requirement);
                let message = answer
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert_eq!(
                    message, expected,
                    "case {label:?}: expected the message {expected:?}, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_value_at_a_published_range_bound_is_admitted() {
    let router = emulator();
    let (recipe, configuration, pipeline) = setup(&router).await;

    let mut token = 0usize;
    for case in range_cases(&recipe, &configuration, &pipeline) {
        let (min, max) = case.range;
        for value in [min, max] {
            for carrier in &case.carriers {
                token += 1;
                let label = format!(
                    "{} = {value}, a published bound, on {}",
                    case.parameter, carrier.operation
                );
                let body = (carrier.body)(value, &format!("bound-{token:04}"));
                let (status, answer) =
                    call(&router, &label, carrier.method, carrier.uri, Some(&body)).await;
                let code = error_code(&answer);
                assert_ne!(
                    code, INVALID_PARAMETER_VALUE,
                    "case {label:?}: expected the bound itself to be admitted, got \
                     {status} with {answer}"
                );
                if carrier.serves {
                    assert_eq!(
                        status, 200,
                        "case {label:?}: expected HTTP 200, got {status} with {answer}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_test_timeout_at_either_bound_reads_back_unchanged() {
    let router = emulator();
    let (recipe, configuration, _) = setup(&router).await;

    // Both ends of the modelled range, stored by the create and reported by the read.
    let (min, max) = TIMEOUT_RANGE;
    for value in [min, 720, max] {
        let label = format!("a pipeline whose test timeout is {value}");
        let body = json!({
            "name": format!("roundtrip-{value}-pipeline"),
            "clientToken": format!("roundtrip-{value}"),
            "imageRecipeArn": recipe,
            "infrastructureConfigurationArn": configuration,
            "imageTestsConfiguration": { "imageTestsEnabled": false, "timeoutMinutes": value },
        });
        let (status, answer) = call(&router, &label, "PUT", "/CreateImagePipeline", Some(&body))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {answer}"
        );
        let arn = text(&label, &answer, "imagePipelineArn");
        let uri = format!(
            "/GetImagePipeline?imagePipelineArn={}",
            percent_encoding::utf8_percent_encode(&arn, percent_encoding::NON_ALPHANUMERIC)
        );
        let (status, read) = call(&router, &label, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from the read, got {status} with {read}"
        );
        let stored = read
            .get("imagePipeline")
            .unwrap_or_else(|| panic!("case {label:?}: expected an imagePipeline member in {read}"));
        let expected = json!({ "imageTestsEnabled": false, "timeoutMinutes": value });
        assert_eq!(
            stored.get("imageTestsConfiguration"),
            Some(&expected),
            "case {label:?}: expected the stored test configuration {expected}, got {stored}"
        );
    }
}
