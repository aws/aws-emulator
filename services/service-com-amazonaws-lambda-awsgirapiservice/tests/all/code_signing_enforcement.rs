// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CODE SIGNING as a deployment control, and for the members the
//! code-signing surface reports.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A deployment to a function attached to a code-signing configuration is verified
//!    against that configuration's `AllowedPublishers` ("A signing profile defines a trusted
//!    user who can sign a code package"): the package's OWN signature is read out of it and
//!    verified against the signing service that made it, and a package signed by a profile
//!    the configuration allows is deployed. An artifact this emulator cannot attribute to an
//!    allowed publisher — one bearing no signature, one whose signature covers other
//!    contents, one made under another publisher's key — fails the checks, and
//!    `CodeSigningPolicies$UntrustedArtifactOnDeployment` decides what happens then: "If you
//!    set the policy to `Enforce`, Lambda blocks the deployment request if signature
//!    validation checks fail. If you set the policy to `Warn`, Lambda allows the deployment".
//!    So such an artifact is BLOCKED under `Enforce` with the modelled
//!    `CodeVerificationFailedException` (HTTP 400) and stores nothing, and lands under
//!    `Warn`. It applies to each operation the model binds that error to: `CreateFunction`
//!    and `UpdateFunctionCode` for the function's own deployment package, and
//!    `UpdateFunctionConfiguration` for the layer packages a configuration change mounts.
//!    The refusal names the artifact whose signature was checked: the function for a
//!    deployment of its own package, the layer version for a layer attach.
//!    A configuration change that mounts no layer deploys nothing and is admitted.
//! 2. Every READ of a code-signing configuration renders `CodeSigningConfig$Description`
//!    (`@length 0..256`), as the empty string for a configuration nobody described —
//!    `GetCodeSigningConfig`, `UpdateCodeSigningConfig` and every entry of
//!    `ListCodeSigningConfigs` alike — while `CreateCodeSigningConfig` ECHOES the
//!    configuration it was asked for and carries the member only for a request that stated
//!    one.
//! 3. `GetFunctionCodeSigningConfig` reports the ATTACHMENT a function holds: the
//!    configuration ARN beside the name of the function it covers for a function that holds
//!    an association, and neither of the two members for a function that holds none.
//!
//! Every row drives real HTTP requests through the router and reads the function or the
//! configuration back, so a refusal that nevertheless changed the store, or a deployment that
//! was answered but never persisted, fails here.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound
/// (`smithy_query(query="operation", operation="CreateFunction")`:
/// `POST /2015-03-31/functions`), and the prefix `UpdateFunctionCode`
/// (`PUT .../{FunctionName}/code`) and `UpdateFunctionConfiguration`
/// (`PUT .../{FunctionName}/configuration`) are bound under.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateCodeSigningConfig` (`POST /2020-04-22/code-signing-configs`) and
/// `ListCodeSigningConfigs` (`GET /2020-04-22/code-signing-configs`) are bound, and the
/// prefix `GetCodeSigningConfig` and `UpdateCodeSigningConfig` are bound under.
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `PublishLayerVersion` is bound
/// (`POST /2018-10-31/layers/{LayerName}/versions`).
const LAYERS_URI: &str = "/2018-10-31/layers";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and which region the ARNs carry. Nothing in it is
/// verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `CodeSigningPolicy` value that BLOCKS a deployment whose signature checks failed,
/// from `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`).
const POLICY_ENFORCE: &str = "Enforce";

/// The other `CodeSigningPolicy` value, which "allows the deployment", and the documented
/// default of `CodeSigningPolicies$UntrustedArtifactOnDeployment`.
const POLICY_WARN: &str = "Warn";

/// The modelled error a blocked deployment answers, from
/// `smithy_query(query="shape", shape="CodeVerificationFailedException")` (`httpError 400`).
const CODE_VERIFICATION_FAILED: &str = "CodeVerificationFailedException";

/// The upper bound the model puts on `CodeSigningConfig$Description` (`@length 0..256`).
const DESCRIPTION_MAX: usize = 256;

/// The refusal Lambda words for a deployment its code-signing checks rejected under the
/// `Enforce` policy: two fixed sentences, and the ARTIFACT whose signature was checked —
/// the function for a deployment of its own package, the layer version for a layer attach.
fn blocked_deployment_message(resource: &str) -> String {
    format!(
        "Lambda cannot deploy the function. The function or layer might be signed using a \
         signature that the client is not configured to accept. Check the provided signature \
         for {resource}."
    )
}

/// The unqualified ARN of the function `name` names, as the model's `FunctionArn` pattern
/// spells one.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// A signing profile version ARN satisfying the model's `Arn` pattern, in the form the
/// signing service vends one.
fn signing_profile(index: usize) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile{index}/{index:08x}")
}

/// The bytes of a .zip archive holding `entries` as `(name, content)`, written in memory.
///
/// The ONE writer every package below is assembled with, so a package and the contents a
/// signature was taken over carry the same entry names: an archive this writes and reads back
/// is what [`service_common::code_signing_signature::content_digest`] states.
pub(crate) fn archive_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, content) in entries {
            writer.start_file(*name, options).expect("a new entry");
            writer.write_all(content).expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The bytes of a .zip archive holding `entries`, written in memory; each entry's content is
/// a run of one byte.
fn archive(entries: &[(&str, usize)]) -> Vec<u8> {
    let named: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(name, length)| (*name, vec![b'x'; *length]))
        .collect();
    archive_of(&named)
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A string member of the body, when it carries one.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `CodeSigningConfig` of a configuration answer.
    fn config(&self) -> Value {
        self.json()
            .get("CodeSigningConfig")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it —
/// with the SIGNING service beside it, because a package's signature is verified against the
/// key pair that service holds for the signing profile version it was signed under.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router.register_service(
        service_com_amazonaws_signer_wallabyservice::EmulatedService::new(Arc::clone(engine)),
    );
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Move every stored function's activation instant into the past, so the next call
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// `CreateCodeSigningConfig`, answering the stored configuration's ARN.
async fn create_config(
    router: &MultiServiceRouter,
    label: &str,
    policy: &str,
    publishers: usize,
) -> String {
    let profiles: Vec<String> = (0..publishers).map(signing_profile).collect();
    create_config_allowing(router, label, policy, &profiles).await
}

/// `CreateCodeSigningConfig` allowing exactly `publishers`, answering the stored
/// configuration's ARN.
async fn create_config_allowing(
    router: &MultiServiceRouter,
    label: &str,
    policy: &str,
    publishers: &[String],
) -> String {
    let profiles: Vec<Value> = publishers.iter().map(|arn| json!(arn)).collect();
    let answer = call(
        router,
        "POST",
        CODE_SIGNING_CONFIGS_URI,
        &json!({
            "AllowedPublishers": { "SigningProfileVersionArns": profiles },
            "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": policy },
            "Description": format!("configuration for {label}"),
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {label:?}: CreateCodeSigningConfig must answer 201, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .config()
        .get("CodeSigningConfigArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {label:?}: CreateCodeSigningConfig must answer an ARN, got {}", answer.text())
        })
        .to_string()
}

/// `UpdateCodeSigningConfig`, replacing the policy of the configuration `arn` names.
async fn set_policy(
    router: &MultiServiceRouter,
    label: &str,
    arn: &str,
    policy: &str,
) {
    let answer = call(
        router,
        "PUT",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        &json!({ "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": policy } }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {label:?}: UpdateCodeSigningConfig must answer 200, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `CreateFunction` with the deployment package `package`, optionally naming the
/// code-signing configuration to deploy under.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    package: &[u8],
    config_arn: Option<&str>,
) -> Answer {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(package) },
    });
    if let Some(arn) = config_arn {
        body["CodeSigningConfigArn"] = json!(arn);
    }
    call(router, "POST", FUNCTIONS_URI, &body).await
}

/// `CreateFunction` that must succeed, answering the created configuration.
async fn create_active_function(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    package: &[u8],
    config_arn: Option<&str>,
) -> Value {
    let answer = create_function(router, name, package, config_arn).await;
    assert_eq!(
        answer.status,
        201,
        "case {label:?}: CreateFunction {name:?} must answer 201, got {} {}",
        answer.status,
        answer.text()
    );
    make_activation_due(engine);
    answer.json()
}

/// `UpdateFunctionCode` on `name`, carrying `body`.
async fn update_code(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{FUNCTIONS_URI}/{name}/code"), body).await
}

/// `UpdateFunctionConfiguration` on `name`, carrying `body`.
async fn update_configuration(
    router: &MultiServiceRouter,
    name: &str,
    body: &Value,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        body,
    )
    .await
}

/// `GetFunctionConfiguration` on `name`.
async fn get_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await
}

/// `GetFunction` on `name`.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("{FUNCTIONS_URI}/{name}"), &Value::Null).await
}

/// `PutFunctionCodeSigningConfig` on `name`, which must succeed.
async fn put_association(router: &MultiServiceRouter, label: &str, name: &str, arn: &str) {
    let answer = call(
        router,
        "PUT",
        &format!("/2020-06-30/functions/{name}/code-signing-config"),
        &json!({ "CodeSigningConfigArn": arn }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {label:?}: PutFunctionCodeSigningConfig must answer 200, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `GetFunctionCodeSigningConfig` on `name`.
async fn get_association(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2020-06-30/functions/{name}/code-signing-config"),
        &Value::Null,
    )
    .await
}

/// `PublishLayerVersion` of one layer holding `package`, answering its version ARN.
async fn publish_layer(
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    package: &[u8],
) -> String {
    let answer = call(
        router,
        "POST",
        &format!("{LAYERS_URI}/{name}/versions"),
        &json!({ "Content": { "ZipFile": zip_file(package) } }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {label:?}: PublishLayerVersion must answer 201, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .string("LayerVersionArn")
        .unwrap_or_else(|| panic!("case {label:?}: expected a LayerVersionArn, got {}", answer.text()))
}

/// Assert that `answer` is the modelled refusal of a deployment code signing blocked,
/// naming `resource`.
fn assert_blocked(label: &str, operation: &str, resource: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {label:?}: expected 400 from {operation} under an enforcing configuration, got \
         {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.error_code(),
        CODE_VERIFICATION_FAILED,
        "case {label:?}: expected {CODE_VERIFICATION_FAILED} from {operation}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.string("Message"),
        Some(blocked_deployment_message(resource)),
        "case {label:?}: expected {operation} to answer {:?}, got {:?}",
        blocked_deployment_message(resource),
        answer.string("Message")
    );
}

/// One row of the create table: the policy the configuration carries, how many publishers it
/// allows, the deployment package the create submits, and whether the deployment lands.
struct CreateCase {
    name: &'static str,
    function: &'static str,
    policy: &'static str,
    publishers: usize,
    package: Vec<u8>,
    blocked: bool,
}

/// The rows differ along what the rule generalises over: the policy, how many publishers the
/// configuration allows, and the form and size of the package the create deploys.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "enforce, one publisher, a one-entry archive",
            function: "csc-enf-create-one",
            policy: POLICY_ENFORCE,
            publishers: 1,
            package: archive(&[("index.py", 32)]),
            blocked: true,
        },
        CreateCase {
            name: "enforce, several publishers, a larger archive",
            function: "csc-enf-create-many",
            policy: POLICY_ENFORCE,
            publishers: 5,
            package: archive(&[("index.py", 4_096), ("lib/helper.py", 256)]),
            blocked: true,
        },
        CreateCase {
            name: "enforce, bytes that are no archive at all",
            function: "csc-enf-create-junk",
            policy: POLICY_ENFORCE,
            publishers: 2,
            package: vec![0x5au8; 97],
            blocked: true,
        },
        CreateCase {
            name: "warn, one publisher, a one-entry archive",
            function: "csc-warn-create-one",
            policy: POLICY_WARN,
            publishers: 1,
            package: archive(&[("index.py", 32)]),
            blocked: false,
        },
        CreateCase {
            name: "warn, several publishers, bytes that are no archive at all",
            function: "csc-warn-create-junk",
            policy: POLICY_WARN,
            publishers: 3,
            package: vec![0x17u8; 64],
            blocked: false,
        },
    ]
}

/// RULE 1, on `CreateFunction`: a create naming a configuration whose policy is `Enforce`
/// deploys a package no allowed publisher signed, so Lambda blocks it and NO function is
/// created; the same create lands under `Warn`.
#[tokio::test]
async fn creating_a_function_under_an_enforcing_configuration_is_blocked_and_stores_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in create_cases() {
        let label = case.name;
        let config = create_config(&router, label, case.policy, case.publishers).await;
        let created = create_function(&router, case.function, &case.package, Some(&config)).await;

        if case.blocked {
            assert_blocked(label, "CreateFunction", case.function, &created);
            // Nothing was created: the function the create was refused for is not there,
            // which is also what its teardown has to see.
            let read = get_function(&router, case.function).await;
            assert_eq!(
                read.status,
                404,
                "case {label:?}: a blocked create must leave no function behind, but \
                 GetFunction answered {} {}",
                read.status,
                read.text()
            );
            continue;
        }

        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateFunction under the {} policy, got {} {}",
            case.policy,
            created.status,
            created.text()
        );
        make_activation_due(&engine);
        // The function landed WITH the association, which is what every later deployment to
        // it is verified against.
        let association = get_association(&router, case.function).await;
        assert_eq!(
            association.string("CodeSigningConfigArn"),
            Some(config.clone()),
            "case {label:?}: expected the function to report the configuration {config:?}, got \
             {:?}",
            association.text()
        );
    }
}

/// How a function comes to be attached to the configuration a row's update is verified
/// against.
#[derive(Debug, Clone, Copy)]
enum Attach {
    /// Named on `CreateFunction` while the configuration still carried the admitting policy,
    /// and RAISED to the row's policy afterwards by `UpdateCodeSigningConfig` — the path a
    /// client takes to tighten an existing deployment control, and the only way a function
    /// comes to hold a create-time association at all, since the create's own deployment is
    /// verified the same way.
    AtCreateThenRaised,
    /// Set by `PutFunctionCodeSigningConfig` after the function exists, against a
    /// configuration that already carries the row's policy: associating a configuration
    /// deploys nothing, so it is not itself verified.
    ByPut,
}

/// One row of the code-update table.
struct UpdateCase {
    name: &'static str,
    function: &'static str,
    policy: &'static str,
    attach: Attach,
    /// The package the function is created with.
    created_with: Vec<u8>,
    /// The `UpdateFunctionCode` body, less the package member this row submits.
    extra: Value,
    /// The package the update submits.
    submitted: Vec<u8>,
    blocked: bool,
}

/// The rows differ along what the rule generalises over: the policy, how the association was
/// made, the package submitted, and the other members the update carries (publishing a
/// version, and a dry run that only validates).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "enforce, associated at create and raised afterwards",
            function: "csc-enf-update-at-create",
            policy: POLICY_ENFORCE,
            attach: Attach::AtCreateThenRaised,
            created_with: archive(&[("index.py", 32)]),
            extra: json!({}),
            submitted: archive(&[("index.py", 64)]),
            blocked: true,
        },
        UpdateCase {
            name: "enforce, associated by a put, publishing a version",
            function: "csc-enf-update-by-put",
            policy: POLICY_ENFORCE,
            attach: Attach::ByPut,
            created_with: archive(&[("index.py", 32)]),
            extra: json!({ "Publish": true }),
            submitted: archive(&[("index.py", 2_048), ("lib/helper.py", 16)]),
            blocked: true,
        },
        UpdateCase {
            name: "enforce, submitting bytes that are no archive at all",
            function: "csc-enf-update-junk",
            policy: POLICY_ENFORCE,
            attach: Attach::ByPut,
            created_with: archive(&[("index.py", 32)]),
            extra: json!({}),
            submitted: vec![0x4bu8; 128],
            blocked: true,
        },
        UpdateCase {
            name: "enforce, a dry run that only validates the package",
            function: "csc-enf-update-dry-run",
            policy: POLICY_ENFORCE,
            attach: Attach::AtCreateThenRaised,
            created_with: archive(&[("index.py", 32)]),
            extra: json!({ "DryRun": true }),
            submitted: archive(&[("index.py", 512)]),
            blocked: true,
        },
        UpdateCase {
            name: "warn, associated at create",
            function: "csc-warn-update-at-create",
            policy: POLICY_WARN,
            attach: Attach::AtCreateThenRaised,
            created_with: archive(&[("index.py", 32)]),
            extra: json!({}),
            submitted: archive(&[("index.py", 1_024)]),
            blocked: false,
        },
        UpdateCase {
            name: "warn, associated by a put, publishing a version",
            function: "csc-warn-update-by-put",
            policy: POLICY_WARN,
            attach: Attach::ByPut,
            created_with: vec![0x2au8; 48],
            extra: json!({ "Publish": true }),
            submitted: archive(&[("index.py", 256)]),
            blocked: false,
        },
    ]
}

/// RULE 1, on `UpdateFunctionCode`: an update of a function attached to a configuration whose
/// policy is `Enforce` deploys a package no allowed publisher signed, so Lambda blocks it and
/// the STORED package is untouched; the same update lands under `Warn`.
#[tokio::test]
async fn updating_code_under_an_enforcing_configuration_is_blocked_and_keeps_the_stored_package() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in update_cases() {
        let label = case.name;
        // A create naming an ENFORCING configuration is itself a blocked deployment (the
        // create table above), so a create-time association starts from the admitting
        // policy and the configuration is raised once the function exists; a put needs no
        // such step.
        let at_create_policy = match case.attach {
            Attach::AtCreateThenRaised => POLICY_WARN,
            Attach::ByPut => case.policy,
        };
        let config = create_config(&router, label, at_create_policy, 2).await;
        let named_at_create = match case.attach {
            Attach::AtCreateThenRaised => Some(config.as_str()),
            Attach::ByPut => None,
        };
        let created = create_active_function(
            &engine,
            &router,
            label,
            case.function,
            &case.created_with,
            named_at_create,
        )
        .await;
        if matches!(case.attach, Attach::ByPut) {
            put_association(&router, label, case.function, &config).await;
        }
        if matches!(case.attach, Attach::AtCreateThenRaised) {
            set_policy(&router, label, &config, case.policy).await;
        }

        let stored_before = created["CodeSha256"].clone();
        let mut body = case.extra.clone();
        body["ZipFile"] = zip_file(&case.submitted);
        let updated = update_code(&router, case.function, &body).await;

        if case.blocked {
            assert_blocked(
                label,
                "UpdateFunctionCode",
                &function_arn(case.function),
                &updated,
            );
        } else {
            assert_eq!(
                updated.status,
                200,
                "case {label:?}: expected 200 from UpdateFunctionCode under the {} policy, got \
                 {} {}",
                case.policy,
                updated.status,
                updated.text()
            );
        }

        // What the function now holds: the package the blocked update never replaced, or the
        // one the admitted update stored.
        let configuration = get_configuration(&router, case.function).await;
        let reported = configuration.json()["CodeSha256"].clone();
        if case.blocked {
            assert_eq!(
                reported, stored_before,
                "case {label:?}: a blocked update must leave the stored package alone, but the \
                 function reports {reported:?} where it held {stored_before:?}"
            );
            let versions = call(
                &router,
                "GET",
                &format!("{FUNCTIONS_URI}/{}/versions", case.function),
                &Value::Null,
            )
            .await;
            let published: Vec<Value> = versions.json()["Versions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry["Version"].as_str() != Some("$LATEST"))
                .collect();
            assert!(
                published.is_empty(),
                "case {label:?}: a blocked update must publish no version, got {published:?}"
            );
        } else {
            assert_ne!(
                reported, stored_before,
                "case {label:?}: the admitted update must store the package it submitted, but \
                 the function still reports {reported:?}"
            );
            assert_eq!(
                reported,
                updated.json()["CodeSha256"].clone(),
                "case {label:?}: expected the update's own answer and the function's \
                 configuration to report one package, got {:?} and {reported:?}",
                updated.json()["CodeSha256"]
            );
        }
    }
}

/// RULE 1, on `UpdateFunctionConfiguration`: the layer packages a configuration change mounts
/// are deployed to the function, so an `Enforce` configuration blocks a change that mounts
/// one — naming the layer version whose signature was checked — and admits a change that
/// mounts none.
#[tokio::test]
async fn mounting_a_layer_under_an_enforcing_configuration_is_blocked() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Each row: the policy, and whether the change mounts a layer.
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        ("enforce, mounting one layer", "csc-enf-layer", POLICY_ENFORCE, true),
        ("enforce, mounting no layer", "csc-enf-no-layer", POLICY_ENFORCE, false),
        ("warn, mounting one layer", "csc-warn-layer", POLICY_WARN, true),
        ("warn, mounting no layer", "csc-warn-no-layer", POLICY_WARN, false),
    ];

    for (label, function, policy, mounts_layer) in rows {
        let config = create_config(&router, label, POLICY_WARN, 1).await;
        create_active_function(
            &engine,
            &router,
            label,
            function,
            &archive(&[("index.py", 32)]),
            Some(&config),
        )
        .await;
        set_policy(&router, label, &config, policy).await;

        let layer = publish_layer(&router, label, function, &archive(&[("python/lib.py", 64)])).await;
        let body = if mounts_layer {
            json!({ "Layers": [layer.clone()] })
        } else {
            json!({ "Description": "no layer mounted here" })
        };
        let answer = update_configuration(&router, function, &body).await;

        let blocked = mounts_layer && policy == POLICY_ENFORCE;
        if blocked {
            // The artifact whose signature was checked is the LAYER VERSION's own package,
            // so that version is what the refusal names — not the function it would have
            // been mounted into.
            assert_blocked(label, "UpdateFunctionConfiguration", &layer, &answer);
        } else {
            assert_eq!(
                answer.status,
                200,
                "case {label:?}: expected 200 from UpdateFunctionConfiguration, got {} {}",
                answer.status,
                answer.text()
            );
        }

        // The layers the function holds afterwards: none for a blocked change, the mounted
        // one for an admitted change that mounted it.
        let configuration = get_configuration(&router, function).await;
        let mounted: Vec<String> = configuration.json()["Layers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry["Arn"].as_str().map(str::to_string))
            .collect();
        let expected: Vec<String> = if blocked || !mounts_layer {
            Vec::new()
        } else {
            vec![layer.clone()]
        };
        assert_eq!(
            mounted, expected,
            "case {label:?}: expected the function to hold the layers {expected:?}, got \
             {mounted:?}"
        );
    }
}

/// RULE 2: every READER of a stored code-signing configuration renders `Description`, as the
/// empty string for a configuration nobody described, while the create echoes the
/// description its own request stated and carries no such member without one.
#[tokio::test]
async fn every_code_signing_config_reader_renders_description() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Each row: the `Description` the create states (`None` for a request that states none),
    // and the value every read must then report. The values span the member's own modelled
    // length range (`@length 0..256`): absent, empty, a short one and the longest admitted.
    let rows: Vec<(&str, Option<String>, String)> = vec![
        ("no Description member at all", None, String::new()),
        (
            "an empty Description",
            Some(String::new()),
            String::new(),
        ),
        (
            "a short Description",
            Some("signs the payments handlers".to_string()),
            "signs the payments handlers".to_string(),
        ),
        (
            "the longest Description the model admits",
            Some("d".repeat(DESCRIPTION_MAX)),
            "d".repeat(DESCRIPTION_MAX),
        ),
    ];

    let mut expected_by_arn: Vec<(String, String)> = Vec::new();
    for (label, stated, expected) in &rows {
        let mut request = json!({
            "AllowedPublishers": { "SigningProfileVersionArns": [signing_profile(0)] },
        });
        if let Some(description) = stated {
            request["Description"] = json!(description);
        }
        let created = call(&router, "POST", CODE_SIGNING_CONFIGS_URI, &request).await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: expected 201 from CreateCodeSigningConfig, got {} {}",
            created.status,
            created.text()
        );
        let arn = created
            .config()
            .get("CodeSigningConfigArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: expected an ARN, got {}", created.text()))
            .to_string();

        // The create ECHOES what the request described: the member is there when the
        // request stated one and absent when it did not.
        assert_eq!(
            created.config().get("Description").and_then(Value::as_str),
            stated.as_deref(),
            "case {label:?}: expected CreateCodeSigningConfig to echo the Description {:?}, \
             got {}",
            stated,
            created.text()
        );

        // Every READ of the stored record renders the member: the read, an update that
        // states no description, and every entry of the listing below.
        let read = call(
            &router,
            "GET",
            &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
            &Value::Null,
        )
        .await;
        let updated = call(
            &router,
            "PUT",
            &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
            &json!({ "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE } }),
        )
        .await;
        for (reader, answer) in [
            ("GetCodeSigningConfig", &read),
            ("UpdateCodeSigningConfig", &updated),
        ] {
            assert_eq!(
                answer.config().get("Description").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {label:?}: expected {reader} to report Description {expected:?}, got {}",
                answer.text()
            );
        }
        expected_by_arn.push((arn, expected.clone()));
    }

    // ... and so does every entry of the listing, which reports the same records.
    let listed = call(&router, "GET", CODE_SIGNING_CONFIGS_URI, &Value::Null).await;
    assert_eq!(
        listed.status,
        200,
        "expected 200 from ListCodeSigningConfigs, got {} {}",
        listed.status,
        listed.text()
    );
    let entries = listed.json()["CodeSigningConfigs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (arn, expected) in &expected_by_arn {
        let entry = entries
            .iter()
            .find(|entry| entry["CodeSigningConfigArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!("expected the listing to carry the configuration {arn:?}, got {}", listed.text())
            });
        assert_eq!(
            entry.get("Description").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {arn:?}: expected the listed entry to report Description {expected:?}, got \
             {entry}"
        );
    }
}

/// RULE 3: `GetFunctionCodeSigningConfig` reports the ATTACHMENT a function holds as the pair
/// of members that describes it, and carries neither of them for a function that holds none.
#[tokio::test]
async fn get_function_code_signing_config_reports_the_association_as_a_whole() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let config = create_config(&router, "member set", POLICY_WARN, 1).await;
    create_active_function(
        &engine,
        &router,
        "member set",
        "csc-members-held",
        &archive(&[("index.py", 32)]),
        Some(&config),
    )
    .await;
    create_active_function(
        &engine,
        &router,
        "member set",
        "csc-members-none",
        &archive(&[("index.py", 32)]),
        None,
    )
    .await;

    // Each row: the function, and the members its body must carry — the configuration ARN
    // and the function it covers for a function that holds an attachment, and nothing at all
    // for one that holds none.
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a function holding an association",
            "csc-members-held",
            vec!["CodeSigningConfigArn", "FunctionName"],
        ),
        ("a function holding none", "csc-members-none", Vec::new()),
    ];

    for (label, function, expected_members) in rows {
        let answer = get_association(&router, function).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200 from GetFunctionCodeSigningConfig, got {} {}",
            answer.status,
            answer.text()
        );
        let mut members: Vec<String> = answer
            .json()
            .as_object()
            .map(|body| body.keys().cloned().collect())
            .unwrap_or_default();
        members.sort();
        let mut expected: Vec<String> = expected_members
            .iter()
            .map(|member| (*member).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            members, expected,
            "case {label:?}: expected the body to carry exactly {expected:?}, got {}",
            answer.text()
        );
        // A 200 carries a JSON object either way: "no attachment" is an object with no
        // members at all, never a value standing in for one.
        assert!(
            answer.json().is_object(),
            "case {label:?}: expected a JSON object body, got {:?}",
            answer.text()
        );
        // The members the function holds come from the stored association, so the name
        // reported beside the ARN is the function's own.
        assert_eq!(
            answer.string("FunctionName"),
            (!expected.is_empty()).then(|| function.to_string()),
            "case {label:?}: expected the body to name the function exactly while it \
             reports an attachment covering it, got {}",
            answer.text()
        );
        if !expected.is_empty() {
            assert_eq!(
                answer.string("CodeSigningConfigArn"),
                Some(config.clone()),
                "case {label:?}: expected the stored association, got {}",
                answer.text()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// RULE 1, the ADMITTING half: a package an allowed publisher signed is deployed.
//
// The signature is the package's own, so a case makes one the way the signing service does:
// a signing profile is created through that service's front door, and the envelope is
// written over the digest of the contents it travels with, under that profile version's key
// pair ([`signed_by`]). Nothing here reaches into Lambda's store.
// ---------------------------------------------------------------------------------------

/// The key pair of one signing profile version, as the signing service holds it.
pub(crate) type SigningKey =
    service_com_amazonaws_signer_wallabyservice::state::payload_signing::PayloadSigningKey;

/// A signing platform of the code-signing workflow the signing service publishes, and the
/// certificate ARN a profile on it signs with.
const SIGNING_PLATFORM: &str = "AWSLambda-SHA384-ECDSA";
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-east-1:000000000000:certificate/abc";

/// A SigV4 credential scope naming the SIGNING service in the same region, which is what
/// routes a profile create there.
const SIGNER_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/signer/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The signing job id a case's envelope names. The signing service holds no job of this id
/// here — these packages are signed without one — so a function deploying such a package
/// reports its publisher and no `SigningJobArn`; the job half is what the cross-service suite
/// that signs through a real `StartSigningJob` pins.
pub(crate) const JOB_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The status a function's `LastUpdateStatus` reports while the update's completion is still
/// owed by the clock, and the one it settles to.
const UPDATE_IN_PROGRESS: &str = "InProgress";
const UPDATE_SUCCESSFUL: &str = "Successful";

/// One signing profile of the signing service: the version ARN a signature of it names, the
/// profile ARN that version belongs to, and the key pair it signs with.
pub(crate) struct Publisher {
    pub(crate) version_arn: String,
    pub(crate) profile_arn: String,
    pub(crate) key: SigningKey,
}

/// `PutSigningProfile` on the signing service, then the key pair that profile version signs
/// with, read out of that service's own store.
pub(crate) async fn publisher(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
) -> Publisher {
    let host = format!("signer.{REGION}.amazonaws.com");
    let body = json!({
        "platformId": SIGNING_PLATFORM,
        "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
    });
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("http://{host}/signing-profiles/{name}"))
        .header("host", host)
        .header("authorization", SIGNER_AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the profile request body must serialize"),
        ))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, payload) = response.into_parts();
    assert_eq!(
        parts.status.as_u16(),
        200,
        "case {label:?}: PutSigningProfile {name:?} must answer 200, got {} {}",
        parts.status.as_u16(),
        String::from_utf8_lossy(&payload)
    );
    let answered: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    let version_arn = answered["profileVersionArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {label:?}: PutSigningProfile must answer a profileVersionArn, got {}",
                String::from_utf8_lossy(&payload)
            )
        })
        .to_string();
    let profile_arn = answered["arn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {label:?}: PutSigningProfile must answer the profile's ARN, got {}",
                String::from_utf8_lossy(&payload)
            )
        })
        .to_string();
    // The key pair that version signs with, taken out of the signing service's store the way
    // a signing job takes it: it is that service's own material, so nothing here invents a
    // key a verifier would then have to be told about.
    let state = service_com_amazonaws_signer_wallabyservice::state::service_state(&**engine)
        .expect("the engine carries the signing service's store");
    let key = {
        let mut store = state.lock();
        store
            .profile_mut(REGION, name)
            .and_then(service_com_amazonaws_signer_wallabyservice::state::SigningProfileRecord::signing_key_copy)
            .unwrap_or_else(|| panic!("case {label:?}: the profile {name:?} must hold a key pair"))
    };
    Publisher {
        version_arn,
        profile_arn,
        key,
    }
}

/// A package `key` signed: the entries `carried`, plus the signature envelope over the
/// material the digest of the entries `declared` and the stated attribution make up, made
/// under that key.
///
/// The four ways a row varies from a package signed as it stands:
///
/// * `declared` differing from `carried` — contents changed after they were signed;
/// * `attributed_to` naming a publisher other than the one whose `key` signed — an envelope
///   claiming a signer that did not make it;
/// * `stated_job` differing from `signed_job` — an attribution rewritten after signing;
/// * `key` being another publisher's.
fn signed_by(
    key: &SigningKey,
    attributed_to: &str,
    (signed_job, stated_job): (&str, &str),
    declared: &[(&str, usize)],
    carried: &[(&str, usize)],
) -> Vec<u8> {
    let digest = service_common::code_signing_signature::content_digest(&archive(declared))
        .expect("the declared contents digest");
    let (signature, public_key) = key
        .signature_and_public_key(&service_common::code_signing_signature::signed_material(
            attributed_to,
            signed_job,
            &digest,
        ))
        .expect("the signing profile version signs");
    let envelope = service_common::code_signing_signature::render_envelope(
        &digest,
        &signature,
        &public_key,
        attributed_to,
        stated_job,
    );
    let mut entries: Vec<(&str, Vec<u8>)> = carried
        .iter()
        .map(|(name, length)| (*name, vec![b'x'; *length]))
        .collect();
    entries.push((
        service_common::code_signing_signature::SIGNATURE_ENTRY_NAME,
        envelope,
    ));
    archive_of(&entries)
}

/// The `CodeSha256` a stored package reports: "The SHA256 hash of the function's deployment
/// package", base64-encoded as the member documents.
fn code_sha256(package: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(<sha2::Sha256 as sha2::Digest>::digest(package))
}

/// Read the function's configuration until `LastUpdateStatus` has settled, and answer the
/// status it settled to.
///
/// The move out of `InProgress` is owed by the CLOCK, so the case looks again until it holds
/// rather than waiting a fixed time, and gives up only on a deadline generous enough that a
/// loaded machine does not fail it.
async fn settled_update_status(
    router: &MultiServiceRouter,
    label: &str,
    function: &str,
) -> String {
    let deadline = SystemTime::now() + Duration::from_secs(120);
    loop {
        let answer = get_configuration(router, function).await;
        let status = answer.string("LastUpdateStatus").unwrap_or_default();
        if status != UPDATE_IN_PROGRESS {
            return status;
        }
        assert!(
            SystemTime::now() < deadline,
            "case {label:?}: the update of {function} never settled: {}",
            answer.text()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One row of the admitted-deployment table.
struct AdmittedCase {
    name: &'static str,
    function: &'static str,
    /// Whether the configuration allows other publishers beside the one that signed.
    other_publishers: usize,
    /// The entries of the package the update deploys.
    entries: Vec<(&'static str, usize)>,
    /// The other members the update carries.
    extra: Value,
}

/// The rows differ along what the rule generalises over: how many other publishers the
/// configuration allows, the shape and size of the signed package, and the other members the
/// update carries.
fn admitted_cases() -> Vec<AdmittedCase> {
    vec![
        AdmittedCase {
            name: "the only publisher the configuration allows",
            function: "csc-signed-version",
            other_publishers: 0,
            entries: vec![("index.py", 64)],
            extra: json!({}),
        },
        AdmittedCase {
            name: "allowed among several publishers, publishing a version",
            function: "csc-signed-among-many",
            other_publishers: 3,
            entries: vec![("index.py", 2_048), ("lib/helper.py", 16)],
            extra: json!({ "Publish": true }),
        },
        AdmittedCase {
            name: "allowed beside one other publisher, an archive with an empty entry",
            function: "csc-signed-empty-entry",
            other_publishers: 1,
            entries: vec![("index.py", 32), ("data/empty", 0)],
            extra: json!({}),
        },
    ]
}

/// RULE 1, the half the refusal is the other side of: a package whose own signature verifies
/// under one of the configuration's allowed publishers is DEPLOYED under the `Enforce`
/// policy — answered 200, stored, settling to `Successful`, and reporting the publisher that
/// signed it.
#[tokio::test]
async fn updating_code_with_a_package_an_allowed_publisher_signed_is_deployed() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let release = publisher(&engine, &router, "signed deployments", "Release_Profile").await;

    for case in admitted_cases() {
        let label = case.name;
        let mut publishers: Vec<String> = (0..case.other_publishers).map(signing_profile).collect();
        publishers.push(release.version_arn.clone());
        let config = create_config_allowing(&router, label, POLICY_ENFORCE, &publishers).await;

        // The function starts from an UNSIGNED package, which is why it is created before the
        // enforcing configuration is attached to it: that create would itself be a blocked
        // deployment.
        let created = create_active_function(
            &engine,
            &router,
            label,
            case.function,
            &archive(&[("index.py", 32)]),
            None,
        )
        .await;
        put_association(&router, label, case.function, &config).await;
        assert_eq!(
            created["SigningProfileVersionArn"],
            Value::Null,
            "case {label:?}: a function created from an unsigned package must report no \
             publisher, got {:?}",
            created["SigningProfileVersionArn"]
        );

        let package = signed_by(
            &release.key,
            &release.version_arn,
            (JOB_ID, JOB_ID),
            &case.entries,
            &case.entries,
        );
        let mut body = case.extra.clone();
        body["ZipFile"] = zip_file(&package);
        let updated = update_code(&router, case.function, &body).await;
        assert_eq!(
            updated.status,
            200,
            "case {label:?}: expected 200 from UpdateFunctionCode for a package the \
             configuration's allowed publisher signed, got {} {}",
            updated.status,
            updated.text()
        );
        // The digest of the very bytes that were deployed.
        assert_eq!(
            updated.string("CodeSha256"),
            Some(code_sha256(&package)),
            "case {label:?}: expected the deployed package's own digest, got {:?}",
            updated.string("CodeSha256")
        );
        // ... and WHO signed them, read back off the stored configuration.
        assert_eq!(
            updated.string("SigningProfileVersionArn"),
            Some(release.version_arn.clone()),
            "case {label:?}: expected the update to report the publisher that signed the \
             package, got {}",
            updated.text()
        );

        let settled = settled_update_status(&router, label, case.function).await;
        assert_eq!(
            settled, UPDATE_SUCCESSFUL,
            "case {label:?}: expected the update to settle to {UPDATE_SUCCESSFUL}, got \
             {settled:?}"
        );

        // The round trip: an independent read reports the package that was deployed and the
        // publisher it was signed by.
        let read = get_function(&router, case.function).await;
        let configuration = read.json()["Configuration"].clone();
        assert_eq!(
            configuration["CodeSha256"].as_str(),
            Some(code_sha256(&package).as_str()),
            "case {label:?}: expected GetFunction to report the deployed package, got {:?}",
            configuration["CodeSha256"]
        );
        assert_eq!(
            configuration["SigningProfileVersionArn"].as_str(),
            Some(release.version_arn.as_str()),
            "case {label:?}: expected GetFunction to report the publisher, got {:?}",
            configuration["SigningProfileVersionArn"]
        );
    }
}

/// How a row's package fails to be one an allowed publisher signed.
#[derive(Debug, Clone, Copy)]
enum NotSignedBy {
    /// Signed, as it claims, by a profile the configuration does not allow.
    AnotherPublisher,
    /// An envelope naming the allowed publisher, made under another profile's key.
    AnotherPublishersKey,
    /// The allowed publisher's own envelope, lifted onto other contents.
    OtherContents,
    /// The allowed publisher's own signature, with the job it attributes the package to
    /// rewritten afterwards.
    ARewrittenAttribution,
    /// A version ARN of the allowed profile that no signature was made under.
    AnotherVersionOfTheProfile,
    /// Signed by the allowed PROFILE, where the configuration allows the profile's own ARN
    /// and not the version a signature names.
    AProfileWithNoVersion,
    /// No signature at all.
    Nothing,
}

/// RULE 1, the refusing half, on the cases a VERIFIED signature tells apart: only a package
/// whose signature verifies under one of the signing profile VERSIONS the configuration
/// allows is deployed, so a package signed by someone else, one whose signature was made
/// under another key, one whose contents or attribution changed after they were signed, one
/// naming a version nothing signed, one whose configuration names a profile rather than a
/// version of it, and one bearing no signature at all are each blocked under `Enforce` — with
/// the stored package left alone.
#[tokio::test]
async fn updating_code_with_a_package_no_allowed_publisher_signed_is_blocked() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let release = publisher(&engine, &router, "refused deployments", "Release_Profile").await;
    let other = publisher(&engine, &router, "refused deployments", "Other_Profile").await;

    let deployed: Vec<(&str, usize)> = vec![("index.py", 64)];
    let elsewhere: Vec<(&str, usize)> = vec![("index.py", 65)];
    // (case, the function, how the package fails to be signed by an allowed publisher)
    let rows: Vec<(&str, &str, NotSignedBy)> = vec![
        (
            "signed by a publisher the configuration does not allow",
            "csc-unsigned-other",
            NotSignedBy::AnotherPublisher,
        ),
        (
            "an envelope naming the allowed publisher, signed with another key",
            "csc-unsigned-forged",
            NotSignedBy::AnotherPublishersKey,
        ),
        (
            "the allowed publisher's envelope, lifted onto other contents",
            "csc-unsigned-lifted",
            NotSignedBy::OtherContents,
        ),
        (
            "the signing job attribution rewritten after signing",
            "csc-unsigned-rewritten",
            NotSignedBy::ARewrittenAttribution,
        ),
        (
            "a version of the allowed profile that nothing signed",
            "csc-unsigned-version",
            NotSignedBy::AnotherVersionOfTheProfile,
        ),
        (
            "a configuration naming the signing profile rather than the version that signed",
            "csc-unsigned-profile",
            NotSignedBy::AProfileWithNoVersion,
        ),
        ("no signature at all", "csc-unsigned-none", NotSignedBy::Nothing),
    ];

    for (label, function, failure) in rows {
        // The publisher the configuration allows: the version ARN a signature names, except
        // for the row whose configuration names the profile itself.
        let allowed = match failure {
            NotSignedBy::AProfileWithNoVersion => release.profile_arn.clone(),
            _ => release.version_arn.clone(),
        };
        let config = create_config_allowing(&router, label, POLICY_ENFORCE, &[allowed]).await;
        let created = create_active_function(
            &engine,
            &router,
            label,
            function,
            &archive(&[("index.py", 32)]),
            None,
        )
        .await;
        put_association(&router, label, function, &config).await;
        let stored_before = created["CodeSha256"].clone();

        let package = match failure {
            NotSignedBy::AnotherPublisher => signed_by(
                &other.key,
                &other.version_arn,
                (JOB_ID, JOB_ID),
                &deployed,
                &deployed,
            ),
            NotSignedBy::AnotherPublishersKey => signed_by(
                &other.key,
                &release.version_arn,
                (JOB_ID, JOB_ID),
                &deployed,
                &deployed,
            ),
            NotSignedBy::OtherContents => signed_by(
                &release.key,
                &release.version_arn,
                (JOB_ID, JOB_ID),
                &elsewhere,
                &deployed,
            ),
            NotSignedBy::ARewrittenAttribution => signed_by(
                &release.key,
                &release.version_arn,
                (JOB_ID, "99999999-8888-7777-6666-555555555555"),
                &deployed,
                &deployed,
            ),
            NotSignedBy::AnotherVersionOfTheProfile => signed_by(
                &release.key,
                &format!("{}/0000000000", release.profile_arn),
                (JOB_ID, JOB_ID),
                &deployed,
                &deployed,
            ),
            // The package IS signed by the allowed profile's version; what fails is the
            // configuration, which names the profile rather than that version.
            NotSignedBy::AProfileWithNoVersion => signed_by(
                &release.key,
                &release.version_arn,
                (JOB_ID, JOB_ID),
                &deployed,
                &deployed,
            ),
            NotSignedBy::Nothing => archive(&deployed),
        };
        let updated = update_code(&router, function, &json!({ "ZipFile": zip_file(&package) }))
            .await;
        assert_blocked(label, "UpdateFunctionCode", &function_arn(function), &updated);

        // Nothing was deployed: the function still serves the package it held, and reports no
        // publisher for it.
        let configuration = get_configuration(&router, function).await;
        assert_eq!(
            configuration.json()["CodeSha256"], stored_before,
            "case {label:?}: a blocked update must leave the stored package alone, got {}",
            configuration.text()
        );
        assert_eq!(
            configuration.json()["SigningProfileVersionArn"],
            Value::Null,
            "case {label:?}: expected the function to report no publisher, got {}",
            configuration.text()
        );
    }
}

/// RULE 1, on `CreateFunction` and on the LAYER packages a configuration change mounts: the
/// same verification admits a create whose package an allowed publisher signed, and a change
/// mounting a layer version whose own package one signed.
#[tokio::test]
async fn creating_and_mounting_signed_packages_under_an_enforcing_configuration_lands() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let release = publisher(&engine, &router, "signed create", "Release_Profile").await;
    let config = create_config_allowing(
        &router,
        "signed create",
        POLICY_ENFORCE,
        std::slice::from_ref(&release.version_arn),
    )
    .await;
    // A create naming the enforcing configuration, deploying a package that publisher signed.
    let function = "csc-signed-create";
    let entries: Vec<(&str, usize)> = vec![("index.py", 128)];
    let package = signed_by(
        &release.key,
        &release.version_arn,
        (JOB_ID, JOB_ID),
        &entries,
        &entries,
    );
    let created = create_function(&router, function, &package, Some(&config)).await;
    assert_eq!(
        created.status,
        201,
        "expected 201 from CreateFunction for a package the allowed publisher signed, got {} {}",
        created.status,
        created.text()
    );
    make_activation_due(&engine);
    assert_eq!(
        created.string("SigningProfileVersionArn"),
        Some(release.version_arn.clone()),
        "expected the create to report the publisher that signed the package, got {}",
        created.text()
    );
    assert_eq!(
        created.string("CodeSha256"),
        Some(code_sha256(&package)),
        "expected the create to report the deployed package's own digest, got {}",
        created.text()
    );

    // A layer version whose own package that publisher signed, mounted into the same
    // function: each layer is a package of its own, verified on its own when it is published.
    let layer_entries: Vec<(&str, usize)> = vec![("python/lib.py", 96)];
    let layer_package = signed_by(
        &release.key,
        &release.version_arn,
        (JOB_ID, JOB_ID),
        &layer_entries,
        &layer_entries,
    );
    let layer = publish_layer(&router, "signed layer", function, &layer_package).await;
    let mounted = update_configuration(&router, function, &json!({ "Layers": [layer.clone()] })).await;
    assert_eq!(
        mounted.status,
        200,
        "expected 200 from UpdateFunctionConfiguration mounting a signed layer version, got {} {}",
        mounted.status,
        mounted.text()
    );
    let configuration = get_configuration(&router, function).await;
    let held: Vec<String> = configuration.json()["Layers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["Arn"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        held,
        vec![layer.clone()],
        "expected the function to hold the signed layer version, got {held:?}"
    );

    // And the other half of the same rule on the same function: a layer version no allowed
    // publisher signed is still blocked, named by that version's own ARN. The mount above is
    // an update in flight, so the case waits for the clock to settle it before making
    // another change.
    let settled = settled_update_status(&router, "signed layer", function).await;
    assert_eq!(
        settled, UPDATE_SUCCESSFUL,
        "expected the mount to settle to {UPDATE_SUCCESSFUL}, got {settled:?}"
    );
    let unsigned_layer =
        publish_layer(&router, "unsigned layer", function, &archive(&layer_entries)).await;
    let refused = update_configuration(
        &router,
        function,
        &json!({ "Layers": [unsigned_layer.clone()] }),
    )
    .await;
    assert_blocked(
        "unsigned layer",
        "UpdateFunctionConfiguration",
        &unsigned_layer,
        &refused,
    );
}
