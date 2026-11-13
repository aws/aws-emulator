// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CODE SIGNING as a deployment control, and for the members the
//! code-signing surface reports.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A deployment to a function attached to a code-signing configuration is verified
//!    against that configuration's `AllowedPublishers` ("A signing profile defines a trusted
//!    user who can sign a code package"), and
//!    `CodeSigningPolicies$UntrustedArtifactOnDeployment` decides what happens when the
//!    verification fails: "If you set the policy to `Enforce`, Lambda blocks the deployment
//!    request if signature validation checks fail. If you set the policy to `Warn`, Lambda
//!    allows the deployment". An artifact this emulator cannot attribute to an allowed
//!    publisher — every artifact it is handed, since no signing service stamps one here — is
//!    therefore BLOCKED under `Enforce` with the modelled
//!    `CodeVerificationFailedException` (HTTP 400) and stores nothing, and lands under
//!    `Warn`. It applies to each operation the model binds that error to: `CreateFunction`
//!    and `UpdateFunctionCode` for the function's own deployment package, and
//!    `UpdateFunctionConfiguration` for the layer packages a configuration change mounts.
//!    A configuration change that mounts no layer deploys nothing and is admitted.
//! 2. Every READ of a code-signing configuration renders `CodeSigningConfig$Description`
//!    (`@length 0..256`), as the empty string for a configuration nobody described —
//!    `GetCodeSigningConfig`, `UpdateCodeSigningConfig` and every entry of
//!    `ListCodeSigningConfigs` alike — while `CreateCodeSigningConfig` ECHOES the
//!    configuration it was asked for and carries the member only for a request that stated
//!    one.
//! 3. `GetFunctionCodeSigningConfig` reports the association as a WHOLE: the configuration
//!    ARN and the function holding it for a function that holds one, and NEITHER member for
//!    a function that holds none.
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
/// `Enforce` policy: two fixed sentences, and the function the deployment addressed.
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

/// The bytes of a .zip archive holding `entries`, written in memory; each entry's content is
/// a run of one byte.
fn archive(entries: &[(&str, usize)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, length) in entries {
            writer.start_file(*name, options).expect("a new entry");
            writer
                .write_all(&vec![b'x'; *length])
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
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

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
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
    let profiles: Vec<Value> = (0..publishers).map(|index| json!(signing_profile(index))).collect();
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
/// one and admits a change that mounts none.
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
            assert_blocked(
                label,
                "UpdateFunctionConfiguration",
                &function_arn(function),
                &answer,
            );
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

/// RULE 3: `GetFunctionCodeSigningConfig` reports the association as a whole — the
/// configuration ARN and the function holding it, or neither member at all.
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
    // and the function holding it for a function that holds an association, and nothing at
    // all for one that holds none.
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
        // A 200 carries a JSON object either way: "no association" is an object with no
        // member in it, never a value standing in for one.
        assert!(
            answer.json().is_object(),
            "case {label:?}: expected a JSON object body, got {:?}",
            answer.text()
        );
        // The members the function holds come from the stored association, so the name
        // reported is the function's own.
        if !expected.is_empty() {
            assert_eq!(
                (answer.string("CodeSigningConfigArn"), answer.string("FunctionName")),
                (Some(config.clone()), Some(function.to_string())),
                "case {label:?}: expected the stored association, got {}",
                answer.text()
            );
        }
    }
}
