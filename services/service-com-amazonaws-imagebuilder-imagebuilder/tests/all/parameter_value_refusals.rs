// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service refuses a parameter whose value its model
//! does not admit.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door and the modelled-constraint walk behind it — so
//! what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: such a value is refused as the parameter value it is — HTTP 400
//! `InvalidParameterValueException`, in `$.__type` and in the `x-amzn-errortype` header,
//! with one sentence naming the parameter as the API addresses it (a list element by its
//! zero-based index) and the requirement its value had to satisfy, and with the request the
//! response answers named in the body's `requestId`.
//!
//! The rows span what the rule generalises over: the constraint class the value broke (a
//! pattern, either `@range` bound, an enumerated value set, a list below its `@length`
//! minimum), how deep in the request the
//! parameter sits (a top-level member, a member of a structure, a member inside a list
//! element, a list element's own structure member), which operation carries it, and — for
//! the expression an ARN member's value had to match — whether the value was read out of the
//! request body or out of the request line.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// A component document with one build phase, so a `CreateComponent` row is refused for the
/// parameter it is about rather than for its document.
const BUILD_DOCUMENT: &str = "name: Probe\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: run\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hi\n";

/// One row of the refusal table: the case, the request (method, URI, body), the parameter
/// the refusal names and the requirement it states for it.
type RefusalRow<'a> = (&'a str, &'a str, String, Option<Value>, &'a str, String);

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

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on.
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

/// What one call answered, as a client reads it.
struct Answer {
    status: u16,
    /// The error shape the BODY names (`$.__type`), reduced to its shape name.
    code: String,
    /// The error shape the HEADER names, reduced to its shape name.
    error_type: String,
    message: String,
    /// The `requestId` member of the body, or the empty string when it carries none.
    request_id: String,
    body: Value,
}

/// The shape name a discriminator value names, without its namespace or locator.
fn shape_name(raw: &str) -> String {
    raw.rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Call the emulator and read the answer.
async fn call(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(shape_name)
        .unwrap_or_default();
    let raw = response.into_body();
    let parsed: Value = serde_json::from_slice(&raw)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {raw:?}"));
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(shape_name)
        .unwrap_or_default();
    let message = parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request_id = parsed
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        code,
        error_type,
        message,
        request_id,
        body: parsed,
    }
}

/// The sentence this service states for `parameter`, whose value had to satisfy
/// `requirement`.
fn expected_message(parameter: &str, requirement: &str) -> String {
    format!("The value supplied for parameter '{parameter}' is not valid. {requirement}")
}

/// Assert one refused call: the status, the error in both places it travels, the sentence,
/// and the request the response names.
fn assert_refused(case: &str, answered: &Answer, expected: &str) {
    assert_refused_as(case, answered, INVALID_PARAMETER_VALUE, expected);
}

/// [`assert_refused`] for a refusal this service answers in another of its modelled error
/// classes: the class is asserted in both places it travels, and everything else is the
/// same.
fn assert_refused_as(case: &str, answered: &Answer, code: &str, expected: &str) {
    assert_eq!(
        answered.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.code, code,
        "case {case:?}: expected $.__type {code:?}, got {:?}",
        answered.code
    );
    assert_eq!(
        answered.error_type, code,
        "case {case:?}: expected x-amzn-errortype {code:?}, got {:?}",
        answered.error_type
    );
    assert_eq!(
        answered.message, expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        answered.message
    );
    assert!(
        is_request_id(&answered.request_id),
        "case {case:?}: expected the refusal to name the request it answers, got {:?}",
        answered.request_id
    );
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// A `CreateComponent` body: the required members plus whatever `extra` overlays.
fn component_body(name: &str, version: &str, token: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "semanticVersion": version,
        "platform": "Linux",
        "clientToken": token,
        "data": BUILD_DOCUMENT,
    });
    if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
        for (member, value) in extra {
            body.insert(member, value);
        }
    }
    body
}

#[tokio::test]
async fn a_value_the_model_does_not_admit_is_refused_as_that_parameters_value() {
    let router = emulator();

    // The modelled requirements each row's value breaks, from
    // `smithy_query(query="shape",shape="VersionNumber,Ownership,Platform,ImageTestsConfiguration,AmiNameString,RestrictedInteger,EbsInstanceBlockDeviceSpecification")`:
    // the version pattern, the two enumerated sets, the test-timeout range 60..1440, the AMI
    // name pattern, the page-size range 1..100 and the volume-size range 1..16000.
    let version_pattern = r"^[0-9]+\.[0-9]+\.[0-9]+$";
    let ami_name_pattern = r"^[-_A-Za-z0-9{][-_A-Za-z0-9\s:{}\.]+[-_A-Za-z0-9}]$";
    let rows: Vec<RefusalRow> = vec![
        (
            "a two-part semantic version",
            "PUT",
            "/CreateComponent".to_string(),
            Some(component_body("probe-component", "1.0", "ptok-0001", json!({}))),
            "semanticVersion",
            format!("semanticVersion must match pattern {version_pattern}"),
        ),
        (
            "a four-part semantic version",
            "PUT",
            "/CreateComponent".to_string(),
            Some(component_body(
                "probe-component",
                "1.0.0.0",
                "ptok-0002",
                json!({}),
            )),
            "semanticVersion",
            format!("semanticVersion must match pattern {version_pattern}"),
        ),
        (
            "a platform outside the modelled set",
            "PUT",
            "/CreateComponent".to_string(),
            Some(component_body(
                "probe-component",
                "1.0.0",
                "ptok-0003",
                json!({"platform": "linux"}),
            )),
            "platform",
            "platform must be one of ['Windows', 'Linux', 'macOS']".to_string(),
        ),
        (
            "an owner outside the modelled set",
            "POST",
            "/ListComponents".to_string(),
            Some(json!({"owner": "self"})),
            "owner",
            "owner must be one of ['Self', 'Shared', 'Amazon', 'ThirdParty', 'AWSMarketplace']"
                .to_string(),
        ),
        (
            "a page size above the modelled maximum",
            "POST",
            "/ListComponents".to_string(),
            Some(json!({"maxResults": 101})),
            "maxResults",
            "maxResults must be smaller than or equal to 100".to_string(),
        ),
        (
            "a page size below the modelled minimum",
            "POST",
            "/ListComponents".to_string(),
            Some(json!({"maxResults": 0})),
            "maxResults",
            "maxResults must be bigger than or equal to 1".to_string(),
        ),
        (
            "a test timeout above the modelled maximum, inside a structure member",
            "PUT",
            "/CreateImagePipeline".to_string(),
            Some(json!({
                "name": "probe-pipeline",
                "infrastructureConfigurationArn": format!(
                    "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/probe"
                ),
                "clientToken": "ptok-0004",
                "imageTestsConfiguration": {"imageTestsEnabled": true, "timeoutMinutes": 1441},
            })),
            "imageTestsConfiguration.timeoutMinutes",
            "imageTestsConfiguration.timeoutMinutes must be smaller than or equal to 1440"
                .to_string(),
        ),
        (
            "a test timeout below the modelled minimum, inside a structure member",
            "PUT",
            "/CreateImagePipeline".to_string(),
            Some(json!({
                "name": "probe-pipeline",
                "infrastructureConfigurationArn": format!(
                    "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/probe"
                ),
                "clientToken": "ptok-0005",
                "imageTestsConfiguration": {"timeoutMinutes": 59},
            })),
            "imageTestsConfiguration.timeoutMinutes",
            "imageTestsConfiguration.timeoutMinutes must be bigger than or equal to 60"
                .to_string(),
        ),
        (
            "an AMI name pattern broken in the first list element",
            "PUT",
            "/CreateDistributionConfiguration".to_string(),
            Some(json!({
                "name": "probe-distribution",
                "clientToken": "ptok-0006",
                "distributions": [{
                    "region": REGION,
                    "amiDistributionConfiguration": {"name": "custom al2 awscli v2!"},
                }],
            })),
            "distributions[0].amiDistributionConfiguration.name",
            format!(
                "distributions[0].amiDistributionConfiguration.name must match pattern \
                 {ami_name_pattern}"
            ),
        ),
        (
            "the same pattern broken in the SECOND list element",
            "PUT",
            "/CreateDistributionConfiguration".to_string(),
            Some(json!({
                "name": "probe-distribution",
                "clientToken": "ptok-0007",
                "distributions": [
                    {"region": REGION},
                    {
                        "region": "us-east-1",
                        "amiDistributionConfiguration": {"name": "another bad name!"},
                    },
                ],
            })),
            "distributions[1].amiDistributionConfiguration.name",
            format!(
                "distributions[1].amiDistributionConfiguration.name must match pattern \
                 {ami_name_pattern}"
            ),
        ),
        (
            "a volume size above the modelled maximum, inside a list element's structure",
            "PUT",
            "/CreateImageRecipe".to_string(),
            Some(json!({
                "name": "probe-recipe",
                "semanticVersion": "1.0.0",
                "parentImage": "ami-0123456789abcdef0",
                "clientToken": "ptok-0008",
                "blockDeviceMappings": [{
                    "deviceName": "/dev/xvda",
                    "ebs": {"volumeSize": 16001},
                }],
            })),
            "blockDeviceMappings[0].ebs.volumeSize",
            "blockDeviceMappings[0].ebs.volumeSize must be smaller than or equal to 16000"
                .to_string(),
        ),
    ];

    for (case, method, uri, body, parameter, requirement) in &rows {
        let answered = call(&router, case, method, uri, body.as_ref()).await;
        assert_refused(case, &answered, &expected_message(parameter, requirement));
    }
}

#[tokio::test]
async fn an_arn_expression_is_stated_in_the_form_of_the_layer_that_read_the_value() {
    let router = emulator();

    // The modelled `*Arn` patterns, from
    // `smithy_query(query="shape",shape="ImagePipelineArn,ImageRecipeArn,ComponentVersionArnOrBuildVersionArn,ImageBuildVersionArn")`,
    // with the partition and Region fields spelled as each layer states them: a value read
    // out of the request LINE in the character-class form, one read out of the BODY in the
    // Region-code form. Every other field is the model's.
    let pipeline_from_the_line = "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$";
    let pipeline_from_the_body = "^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:(?:[0-9]{12}|aws):image-pipeline/[a-z0-9-_]+$";
    let recipe_from_the_line = "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\\.(?:[0-9]+|x)\\.(?:[0-9]+|x)$";
    let component_from_the_line = "^arn:aws(?:-[a-z0-9]+)*:imagebuilder:[a-z0-9-]*[a-z0-9]:(?:[0-9]{12}|aws(?:-[a-z-]+)?|third-party):component/[a-z0-9-_]+/(?:(?:([0-9]+|x)\\.([0-9]+|x)\\.([0-9]+|x))|(?:[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+))$";
    let image_from_the_body = "^arn:aws(?:-[a-z]+)*:imagebuilder:[a-z]{2,}(?:-[a-z]+)+-[0-9]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?):image/[a-z0-9-_]+/[0-9]+\\.[0-9]+\\.[0-9]+/[0-9]+$";
    let rows: Vec<RefusalRow> = vec![
        (
            "a pipeline ARN read out of a query parameter",
            "GET",
            "/GetImagePipeline?imagePipelineArn=probe-pipeline".to_string(),
            None,
            "imagePipelineArn",
            format!("imagePipelineArn must match pattern {pipeline_from_the_line}"),
        ),
        (
            // The same member, read out of the BODY by another operation: the expression is
            // stated in the other form, which is why the wording follows the layer and not
            // the member.
            "the same pipeline ARN read out of a body member",
            "PUT",
            "/StartImagePipelineExecution".to_string(),
            Some(json!({"imagePipelineArn": "probe-pipeline", "clientToken": "atok-0001"})),
            "imagePipelineArn",
            format!("imagePipelineArn must match pattern {pipeline_from_the_body}"),
        ),
        (
            // An operation the issue did not cite, read out of the request line.
            "a pipeline ARN read out of a DELETE's query parameter",
            "DELETE",
            "/DeleteImagePipeline?imagePipelineArn=probe-pipeline".to_string(),
            None,
            "imagePipelineArn",
            format!("imagePipelineArn must match pattern {pipeline_from_the_line}"),
        ),
        (
            // And one the issue did not cite, read out of the body.
            "a pipeline ARN read out of an update's body member",
            "PUT",
            "/UpdateImagePipeline".to_string(),
            Some(json!({
                "imagePipelineArn": "probe-pipeline",
                "infrastructureConfigurationArn": format!(
                    "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/probe"
                ),
                "clientToken": "atok-0002",
            })),
            "imagePipelineArn",
            format!("imagePipelineArn must match pattern {pipeline_from_the_body}"),
        ),
        (
            "a recipe ARN read out of a query parameter",
            "GET",
            format!(
                "/GetImageRecipe?imageRecipeArn=arn:aws:imagebuilder:{REGION}:{ACCOUNT}:\
                 component/probe-component/1.0.0/1"
            ),
            None,
            "imageRecipeArn",
            format!("imageRecipeArn must match pattern {recipe_from_the_line}"),
        ),
        (
            "a component ARN read out of a query parameter",
            "GET",
            "/GetComponent?componentBuildVersionArn=sim2real-probe-ibp-comp-xxxx".to_string(),
            None,
            "componentBuildVersionArn",
            format!("componentBuildVersionArn must match pattern {component_from_the_line}"),
        ),
        (
            "an image build version ARN read out of a body member",
            "POST",
            "/ListImagePackages".to_string(),
            Some(json!({
                "imageBuildVersionArn":
                    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/probe-image/1.0.0"),
            })),
            "imageBuildVersionArn",
            format!("imageBuildVersionArn must match pattern {image_from_the_body}"),
        ),
    ];

    for (case, method, uri, body, parameter, requirement) in &rows {
        let answered = call(&router, case, method, uri, body.as_ref()).await;
        assert_refused(case, &answered, &expected_message(parameter, requirement));
    }

    // The one operation whose own check reports the requirement without echoing the
    // expression at all, over values that miss the pattern in different ways.
    for value in [
        "probe-distribution".to_string(),
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/probe"),
        format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:distribution-configuration/Probe"),
    ] {
        let case = format!("a distribution configuration ARN {value:?}");
        let answered = call(
            &router,
            &case,
            "GET",
            &format!("/GetDistributionConfiguration?distributionConfigurationArn={value}"),
            None,
        )
        .await;
        assert_refused(
            &case,
            &answered,
            &expected_message(
                "distributionConfigurationArn",
                "distributionConfigurationArn does not match the required pattern",
            ),
        );
    }
}

#[tokio::test]
async fn a_requirement_over_two_parameters_together_names_both() {
    let router = emulator();

    // "The component is based on a YAML document that you specify using exactly one of the
    // following methods" — so the requirement holds over `data` and `uri` together, and
    // neither value is wrong on its own. The two halves of that requirement are different
    // failures, each in its own class: a request naming BOTH carries two mutually exclusive
    // parameters (`InvalidParameterCombinationException`, which `CreateComponent`'s own
    // error list declares) and the sentence names both members, while one naming NEITHER is
    // missing a required parameter and is told which members it could have chosen from.
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a component naming both data and uri",
            component_body(
                "probe-component",
                "1.0.0",
                "ctok-0001",
                json!({"uri": "s3://probe-bucket/component.yaml"}),
            ),
            "InvalidParameterCombinationException",
            "The value supplied for parameter 'data' and 'uri' is not valid. You must specify \
             either data or a uri.",
        ),
        (
            "a component naming neither data nor uri",
            json!({
                "name": "probe-component",
                "semanticVersion": "1.0.0",
                "platform": "Linux",
                "clientToken": "ctok-0002",
            }),
            "MissingParametersException",
            "Required parameters are missing from the request: 'data' or 'uri'. Verify that you \
             have entered all of the required parameters in your request and try again.",
        ),
    ];

    for (case, body, code, expected) in &rows {
        let answered = call(&router, case, "PUT", "/CreateComponent", Some(body)).await;
        assert_refused_as(case, &answered, code, expected);
    }
}

/// One row of the collection-minimum table: the case, the request (method, URI, body), the
/// parameter the refusal names and the requirement it states for it.
type CollectionRow<'a> = (&'a str, &'a str, &'a str, Option<Value>, &'a str, &'a str);

#[tokio::test]
async fn a_list_shorter_than_its_minimum_is_refused_as_that_parameters_value() {
    let router = emulator();

    // The modelled minimums each row breaks, from
    // `smithy_query(query="shape",shape="ComponentConfigurationList,OsVersionList,TagMap")`:
    // a recipe's `components` is a list of at least 1, `supportedOsVersions` a list of 1..25,
    // and `tags` a MAP of 1..50.
    let rows: Vec<CollectionRow> = vec![
        (
            "an empty components list on an image recipe",
            "PUT",
            "/CreateImageRecipe",
            Some(json!({
                "name": "probe-recipe",
                "semanticVersion": "1.0.0",
                "parentImage": "ami-0123456789abcdef0",
                "clientToken": "ltok-0001",
                "components": [],
            })),
            "components",
            "components must contain at least 1 items",
        ),
        (
            // The same member on another operation: the clause follows the constraint, not
            // the operation.
            "an empty components list on a container recipe",
            "PUT",
            "/CreateContainerRecipe",
            Some(json!({
                "name": "probe-container-recipe",
                "semanticVersion": "1.0.0",
                "containerType": "DOCKER",
                "parentImage": "amazonlinux:latest",
                "targetRepository": {"service": "ECR", "repositoryName": "probe"},
                "clientToken": "ltok-0002",
                "components": [],
            })),
            "components",
            "components must contain at least 1 items",
        ),
        (
            "an empty supportedOsVersions list on a component",
            "PUT",
            "/CreateComponent",
            Some(component_body(
                "probe-component",
                "1.0.0",
                "ltok-0003",
                json!({"supportedOsVersions": []}),
            )),
            "supportedOsVersions",
            "supportedOsVersions must contain at least 1 items",
        ),
    ];

    for (case, method, uri, body, parameter, requirement) in &rows {
        let answered = call(&router, case, method, uri, body.as_ref()).await;
        assert_refused(case, &answered, &expected_message(parameter, requirement));
    }

    // A MAP under its own minimum is a different finding — the walk counts its ENTRIES, not
    // items — so it keeps the wording the framework states for it rather than this sentence.
    let case = "an empty tags map on a component";
    let answered = call(
        &router,
        case,
        "PUT",
        "/CreateComponent",
        Some(&component_body(
            "probe-component",
            "1.0.0",
            "ltok-0004",
            json!({"tags": {}}),
        )),
    )
    .await;
    assert_eq!(
        answered.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {}",
        answered.status, answered.body
    );
    assert!(
        !answered.message.contains("must contain at least"),
        "case {case:?}: expected the framework's own wording for a map, got {:?}",
        answered.message
    );
}

#[tokio::test]
async fn a_success_states_the_request_id_its_own_output_models() {
    let router = emulator();

    // The report names the request on a FAILURE: a success states the id through the
    // `requestId` member its output shape models, and nothing is added to it.
    let created = call(
        &router,
        "a component the model admits",
        "PUT",
        "/CreateComponent",
        Some(&component_body(
            "probe-component",
            "1.0.0",
            "stok-0001",
            json!({}),
        )),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"a component the model admits\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    assert!(
        is_request_id(&created.request_id),
        "case \"a component the model admits\": expected the output's own requestId, got {:?}",
        created.request_id
    );
    assert!(
        created.body.get("componentBuildVersionArn").is_some(),
        "case \"a component the model admits\": expected the created ARN in {}",
        created.body
    );
}
