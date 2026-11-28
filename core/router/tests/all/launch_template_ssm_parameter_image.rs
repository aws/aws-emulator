// SPDX-License-Identifier: CC0-1.0

//! A launch template's `resolve:ssm:` `ImageId` IS STORED VERBATIM and resolved on the LAUNCH
//! path, against the account's own Parameter Store.
//!
//! The RULE pinned here: **"Alternatively, you can specify a Systems Manager parameter, using
//! one of the following formats. The Systems Manager parameter WILL RESOLVE TO AN AMI ID ON
//! LAUNCH" (`RequestLaunchTemplateData$ImageId`) — so a launch-template writer stores such a
//! value exactly as submitted, whichever documented form it takes (a parameter name, that name
//! with a `:<version>` or a `:<label>` selector, and the parameter ARN in each of those three
//! shapes) and resolves nothing, while the LAUNCH turns the reference into the AMI id the
//! parameter's selected version holds — reading the store the service that OWNS parameters
//! writes, so a parameter a client created resolves and one it deleted stops resolving. A
//! reference that store produces no value for is still STORED, and it is the launch that
//! refuses it: HTTP 400 `SsmInvalidParameter`, "The following aliases are invalid: " followed
//! by the reference the request wrote.**
//!
//! Two crates have to agree for the rule to hold — Amazon EC2 stores the reference and
//! resolves it when it launches, and Systems Manager owns the parameter it names and the
//! version its selector selects — so the suite lives here rather than in either of them. Every
//! value asserted comes from the other service's own surface: the AMI ids are ones
//! `ec2:DescribeImages` reports for this account, the parameter versions are the ones
//! `ssm:PutParameter` allocated and the label is the one `ssm:LabelParameterVersion` attached.
//!
//! The rows differ along exactly what the rule generalises over: WHICH documented form the
//! reference takes, WHICH version of the parameter its selector selects (the latest, an older
//! one by number, one by label), and — for the refusals — WHY the store produces no value for
//! it (no parameter of that name, a version number it does not carry, a label nothing
//! attached, an endpoint of another account, and a parameter that has been deleted).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode};

/// The Region every request below is signed for, and the Region the parameter ARNs name.
const REGION: &str = "us-east-1";

/// The account every generated emulator presents, which a parameter ARN of this account names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The prefix an `ImageId` carries when it names a Systems Manager parameter instead of an AMI
/// — written out here as the member documents it, so a wrong constant in the implementation
/// cannot make this suite pass.
const ALIAS_PREFIX: &str = "resolve:ssm:";

/// The class and the FIXED text Amazon EC2 refuses an image reference it can resolve no AMI
/// for with, likewise written out rather than imported.
const INVALID_ALIAS_CODE: &str = "SsmInvalidParameter";
const INVALID_ALIASES_PREFIX: &str = "The following aliases are invalid: ";

/// The data type a parameter whose value is an AMI id is created with — "when you create a
/// String parameter and specify aws:ec2:image, Amazon Web Services Systems Manager validates
/// the parameter value is in the required format" (`PutParameter$DataType`).
const IMAGE_DATA_TYPE: &str = "aws:ec2:image";

/// The hierarchical parameter name every reference below names, so the ARN rows carry a real
/// path after `parameter`.
const PARAMETER: &str = "/emulator/launch-template/golden-image";

/// The label a `<name>:<label>` reference selects a version by.
const LABEL: &str = "released";

/// Amazon EC2 and Systems Manager over ONE engine, as `core/router/src/lib.rs` mounts them:
/// the launch resolves its image reference by calling Parameter Store in process, so without
/// both mounted there is no parameter for it to reach.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope, which
    // is where a service reads the Region from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is what
/// routes the request; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One ec2Query action, answered with its HTTP status and its XML body — a refusal included,
/// which is what the rows reading Amazon EC2's own wording off its answer ask for.
async fn ec2_answered(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The same action, which must answer 200.
async fn ec2(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = ec2_answered(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon EC2 {action}, got {status} with body \
         {body}"
    );
    body
}

/// One Systems Manager call (`awsJson1_1`, `AmazonSSM.<Operation>`), which must answer 200.
async fn ssm(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ssm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonSSM.{operation}"))
        .header("authorization", authorization("ssm"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Systems Manager {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// The processor architectures a launch of this suite can place, each with the instance type
/// that runs it: an instance runs the machine code its AMI holds, so the pair has to agree.
///
/// Source: `smithy_query(query="shape", shape="ArchitectureValues")` for the architectures an
/// image declares, narrowed to the two current ones, since the types below are current
/// generation.
const LAUNCHABLE_ARCHITECTURES: [(&str, &str); 2] = [("x86_64", "t3.micro"), ("arm64", "t4g.micro")];

/// Two AMI ids of ONE processor architecture out of the account's own image catalog, with the
/// instance type that architecture runs.
///
/// Read through `ec2:DescribeImages` narrowed by its own `architecture` filter, the operation
/// that owns the account's images, so the ids the parameters below hold are ones Amazon EC2
/// handed out rather than ones this suite made up — and two of them, so a reference that PINS
/// a version names a different AMI than the one the parameter's latest version holds.
async fn two_images_of_one_architecture(
    router: &MultiServiceRouter,
) -> (String, String, &'static str) {
    for (architecture, instance_type) in LAUNCHABLE_ARCHITECTURES {
        let body = ec2(
            router,
            architecture,
            "DescribeImages",
            &[
                ("Filter.1.Name", "architecture"),
                ("Filter.1.Value.1", architecture),
            ],
        )
        .await;
        let images = elements(&body, "imageId");
        if let [first, second, ..] = images.as_slice() {
            return (first.clone(), second.clone(), instance_type);
        }
    }
    panic!(
        "the account's image catalog must carry two AMIs of one architecture among \
         {LAUNCHABLE_ARCHITECTURES:?}"
    );
}

/// The ARN of the parameter `PARAMETER` names at this endpoint, as Systems Manager's own read
/// of it reports — taken off `GetParameter` rather than written here, so the ARN rows address
/// the parameter by the ARN that service answers for it.
async fn parameter_arn(router: &MultiServiceRouter) -> String {
    let answer = ssm(
        router,
        "the parameter's own ARN",
        "GetParameter",
        &json!({ "Name": PARAMETER }),
    )
    .await;
    answer
        .get("Parameter")
        .and_then(|parameter| parameter.get("ARN"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no Parameter.ARN in {answer}"))
        .to_string()
}

/// The `imageId` the only version of `launch_template_id` reports with no `ResolveAlias` —
/// "if false, and if a Systems Manager parameter is specified for `ImageId`, the parameter is
/// displayed in the response for `imageId`. Default: false".
async fn stored_image_id(
    router: &MultiServiceRouter,
    case: &str,
    launch_template_id: &str,
) -> String {
    let body = ec2(
        router,
        case,
        "DescribeLaunchTemplateVersions",
        &[("LaunchTemplateId", launch_template_id)],
    )
    .await;
    element(&body, "imageId")
}

/// Create a launch template named `name` whose `ImageId` is `submitted`, and answer its id.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    submitted: &str,
    instance_type: &str,
) -> String {
    let body = ec2(
        router,
        case,
        "CreateLaunchTemplate",
        &[
            ("LaunchTemplateName", name),
            ("LaunchTemplateData.ImageId", submitted),
            ("LaunchTemplateData.InstanceType", instance_type),
        ],
    )
    .await;
    element(&body, "launchTemplateId")
}

/// Put `value` as the next version of `PARAMETER`, and answer the version number Systems
/// Manager allocated.
async fn put_image_parameter(router: &MultiServiceRouter, case: &str, value: &str) -> i64 {
    let answer = ssm(
        router,
        case,
        "PutParameter",
        &json!({
            "Name": PARAMETER,
            "Value": value,
            "Type": "String",
            "DataType": IMAGE_DATA_TYPE,
            "Overwrite": true,
        }),
    )
    .await;
    answer
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: no Version in {answer}"))
}

/// RULE, first half: every documented form of the reference is STORED exactly as submitted and
/// resolved on the launch path to the AMI the version its selector selects holds.
#[tokio::test]
async fn every_documented_reference_form_is_stored_verbatim_and_resolved_on_launch() {
    let router = build_router();
    let (older_image, newer_image, instance_type) = two_images_of_one_architecture(&router).await;

    let older_version = put_image_parameter(&router, "the first version", &older_image).await;
    let newer_version = put_image_parameter(&router, "the second version", &newer_image).await;
    assert!(
        newer_version > older_version,
        "the second put must allocate a later version than the first, got {newer_version} \
         after {older_version}"
    );
    ssm(
        &router,
        "the label on the older version",
        "LabelParameterVersion",
        &json!({
            "Name": PARAMETER,
            "ParameterVersion": older_version,
            "Labels": [LABEL],
        }),
    )
    .await;
    let arn = parameter_arn(&router).await;

    // (case, the reference the `ImageId` carries, the AMI it resolves to)
    let rows: Vec<(&str, String, &str)> = vec![
        ("a parameter name", PARAMETER.to_string(), &newer_image),
        (
            "that name with an older version number",
            format!("{PARAMETER}:{older_version}"),
            &older_image,
        ),
        (
            "that name with the latest version number",
            format!("{PARAMETER}:{newer_version}"),
            &newer_image,
        ),
        (
            "that name with a label",
            format!("{PARAMETER}:{LABEL}"),
            &older_image,
        ),
        ("the parameter ARN", arn.clone(), &newer_image),
        (
            "that ARN with an older version number",
            format!("{arn}:{older_version}"),
            &older_image,
        ),
        (
            "that ARN with a label",
            format!("{arn}:{LABEL}"),
            &older_image,
        ),
    ];

    for (index, (case, reference, resolves_to)) in rows.iter().enumerate() {
        let submitted = format!("{ALIAS_PREFIX}{reference}");
        let name = format!("ssm.reference.{index}");
        let launch_template_id =
            create_template(&router, case, &name, &submitted, instance_type).await;

        let stored = stored_image_id(&router, case, &launch_template_id).await;
        assert_eq!(
            stored, submitted,
            "case {case:?}: expected the stored version to report {submitted:?} unresolved, got \
             {stored:?}"
        );

        let launched = ec2(
            &router,
            case,
            "RunInstances",
            &[
                ("LaunchTemplate.LaunchTemplateId", &launch_template_id),
                ("MinCount", "1"),
                ("MaxCount", "1"),
            ],
        )
        .await;
        let reported = element(&launched, "imageId");
        assert_eq!(
            &reported, resolves_to,
            "case {case:?}: expected the launch to run from {resolves_to}, got {reported:?} in \
             {launched}"
        );

        // And the template is untouched by the launch: the reference it stored is still the
        // one submitted.
        let after = stored_image_id(&router, case, &launch_template_id).await;
        assert_eq!(
            after, submitted,
            "case {case:?}: expected the stored version to still report {submitted:?}, got \
             {after:?}"
        );
    }
}

/// RULE, second half: a reference Parameter Store produces no value for is STORED by the
/// writer all the same, and the LAUNCH is what refuses it — `SsmInvalidParameter` whose fixed
/// sentence names the reference the request wrote.
#[tokio::test]
async fn a_reference_nothing_resolves_is_stored_and_refuses_the_launch() {
    let router = build_router();
    let (image, _, instance_type) = two_images_of_one_architecture(&router).await;
    let version = put_image_parameter(&router, "the parameter under test", &image).await;
    let arn = parameter_arn(&router).await;

    // A parameter of another Region's endpoint and one of another account's: a reference
    // naming either addresses no parameter this endpoint holds.
    let other_account = "999999999999";
    let elsewhere = arn.replacen(ACCOUNT, other_account, 1);
    // A version number this parameter does not carry, derived from the one it does rather than
    // written down.
    let absent_version = version + 7;

    // (case, the reference the `ImageId` carries)
    let rows: Vec<(&str, String)> = vec![
        ("a name no parameter of the account has", "/emulator/no/such/parameter".to_string()),
        (
            "a version number the parameter does not carry",
            format!("{PARAMETER}:{absent_version}"),
        ),
        (
            "a label nothing attached to any version",
            format!("{PARAMETER}:never-attached"),
        ),
        ("the parameter ARN of another account", elsewhere),
        (
            "a value that is no parameter reference at all",
            "arn:aws:s3:::a-bucket".to_string(),
        ),
    ];

    for (index, (case, reference)) in rows.iter().enumerate() {
        let submitted = format!("{ALIAS_PREFIX}{reference}");
        // The WRITER takes it: the reference "will resolve to an AMI ID on launch", so the
        // template is created and holds it exactly as submitted.
        let name = format!("ssm.unresolved.{index}");
        let launch_template_id =
            create_template(&router, case, &name, &submitted, instance_type).await;
        let stored = stored_image_id(&router, case, &launch_template_id).await;
        assert_eq!(
            stored, submitted,
            "case {case:?}: expected the writer to store {submitted:?} unresolved, got \
             {stored:?}"
        );

        // And the LAUNCH refuses it, naming the reference.
        let (status, body) = ec2_answered(
            &router,
            "RunInstances",
            &[
                ("LaunchTemplate.LaunchTemplateId", &launch_template_id),
                ("MinCount", "1"),
                ("MaxCount", "1"),
            ],
        )
        .await;
        assert_eq!(
            (status, element(&body, "Code")),
            (400, INVALID_ALIAS_CODE.to_string()),
            "case {case:?}: expected HTTP 400 {INVALID_ALIAS_CODE} from the launch, got \
             {status} with body {body}"
        );
        let expected = format!("{INVALID_ALIASES_PREFIX}{reference}");
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {case:?}: expected the refusal to read {expected:?}, got body {body}"
        );
    }

    // The parameter that DOES resolve launches, and stops launching once it is deleted: the
    // launch reads Parameter Store at the instant of the launch rather than once at the write.
    let submitted = format!("{ALIAS_PREFIX}{PARAMETER}");
    let launch_template_id = create_template(
        &router,
        "the parameter that resolves",
        "ssm.deleted.parameter",
        &submitted,
        instance_type,
    )
    .await;
    let launched = ec2(
        &router,
        "the parameter that resolves",
        "RunInstances",
        &[
            ("LaunchTemplate.LaunchTemplateId", &launch_template_id),
            ("MinCount", "1"),
            ("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        element(&launched, "imageId"),
        image,
        "expected the launch to run from {image}, got {launched}"
    );

    ssm(
        &router,
        "the delete",
        "DeleteParameter",
        &json!({ "Name": PARAMETER }),
    )
    .await;
    let (status, body) = ec2_answered(
        &router,
        "RunInstances",
        &[
            ("LaunchTemplate.LaunchTemplateId", &launch_template_id),
            ("MinCount", "1"),
            ("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        (status, element(&body, "Code")),
        (400, INVALID_ALIAS_CODE.to_string()),
        "once the parameter is deleted the launch must be refused {INVALID_ALIAS_CODE}, got \
         {status} with body {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("{INVALID_ALIASES_PREFIX}{PARAMETER}"),
        "expected the refusal to name the parameter the template stored, got body {body}"
    );
}
