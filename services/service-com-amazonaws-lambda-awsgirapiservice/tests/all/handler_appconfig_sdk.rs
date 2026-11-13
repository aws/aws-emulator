// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler reads its own CONFIGURATION through, end to end over the
//! wire: AWS Lambda, the AppConfig control plane and the AppConfig data plane on ONE engine,
//! a `python3.x` function whose code does `import boto3`, and what the handler read back
//! where a client reads it.
//!
//! RULE: a managed runtime's bundled SDK can build a client for, and call, any service this
//! emulator serves, over that service's own protocol — so what a handler reads back is that
//! service's own answer (the operation's output members on a success, its own error code and
//! message raised as `botocore.exceptions.ClientError` on a refusal) and never a client-side
//! report that the service is unknown. And where an output member is bound to the WHOLE
//! response body and its shape is a `blob`, the body IS that member's bytes, so the SDK hands
//! it over as the readable stream its own client answers one with, carrying those bytes
//! verbatim rather than the text they happened to decode to.
//!
//! The rows differ along what the rule generalises over: WHICH of AppConfig's two APIs the
//! call goes to (the data plane's session and poll, the control plane's read and write of a
//! hosted version), whether each identifier is given as the ID or as the NAME of its level
//! ("the application ID or the application name", `StartConfigurationSessionRequest`), which
//! level is given an identifier that resolves to nothing, whether the document is JSON text
//! or bytes that are not UTF-8 at all, and whether the call is served or refused — and, for a
//! refusal, which of the service's own error codes the handler reports. Each served row is
//! asserted on BOTH sides: the bytes the guest read, and what the service itself holds, read
//! through its own API as a client reads it.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. Each transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// The signing name BOTH of AppConfig's APIs sign with (each model's `aws.auth#sigv4` name),
/// which is what routes a request of either to the service whose own paths it addresses.
const APPCONFIG_SIGNING_NAME: &str = "appconfig";

/// The `LocationUri` that selects AppConfig's own hosted configuration store.
const HOSTED: &str = "hosted";

/// The predefined deployment strategy that reaches every target at once, so the deployment's
/// rollout has begun the instant `StartDeployment` answers.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// The names the hierarchy is created under.
const APPLICATION_NAME: &str = "orders-flags";
const ENVIRONMENT_NAME: &str = "production";
const PROFILE_NAME: &str = "backend-rollout";

/// The document the deployment puts in force: the flag document a feature-flagged handler
/// branches on.
const DEPLOYED: &[u8] = br#"{"useNewBackend": true}"#;

/// The media type the documents here are written and served with.
const JSON_CONTENT_TYPE: &str = "application/json";
const BINARY_CONTENT_TYPE: &str = "application/octet-stream";

/// A document that is NOT text at all: these bytes are not valid UTF-8 (0xff never appears in
/// a UTF-8 encoding), so a value carried as the text its bytes decoded to could not survive
/// the round trip.
const BINARY_DOCUMENT: &[u8] = &[0x00, 0x9f, 0x92, 0x96, 0xff, 0xfe, 0x41];

/// The bytes the handler itself writes as a hosted configuration version, binary for the same
/// reason.
const HANDLER_WRITTEN: &[u8] = &[0xff, 0x00, 0x7b, 0x22, 0x61, 0x22, 0x7d, 0x80];

/// The modelled refusal AppConfig answers for an identifier that names nothing
/// (`ResourceNotFoundException`, HTTP 404 in both models).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The fallback document the handler serves when it cannot reach AppConfig at all.
fn fallback() -> Value {
    json!({ "useNewBackend": false })
}

/// Lambda, the AppConfig control plane and the AppConfig data plane over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the configuration it reads is the configuration this suite deployed.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// What one dispatched request answered.
struct Answered {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answered {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&self.body).to_string()))
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// One response header, as text.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// The SigV4 signing name the service answering at the endpoint host `service` signs with:
/// both of AppConfig's APIs sign as `appconfig`, whichever endpoint a client addresses, and
/// every other service here signs under its own name.
fn signing_name(service: &str) -> &str {
    match service {
        "appconfig" | "appconfigdata" => APPCONFIG_SIGNING_NAME,
        other => other,
    }
}

/// One request dispatched at the endpoint host of `service`, signed for it.
async fn dispatch(
    router: &MultiServiceRouter,
    service: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answered {
    let host = format!("{service}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header("authorization", authorization(signing_name(service)))
        .header("content-type", content_type);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One Lambda `restJson1` call.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answered {
    dispatch(router, "lambda", method, uri, JSON_CONTENT_TYPE, &[], body).await
}

/// One AppConfig CONTROL-plane call, as a client makes one.
async fn control_plane(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    content_type: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answered {
    dispatch(router, "appconfig", method, uri, content_type, headers, body).await
}

/// One AppConfig DATA-plane call, as a client makes one.
async fn data_plane(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Answered {
    dispatch(
        router,
        "appconfigdata",
        method,
        uri,
        JSON_CONTENT_TYPE,
        &[],
        body,
    )
    .await
}

/// A control-plane call that must answer `expected`, with its parsed body.
async fn served(
    router: &MultiServiceRouter,
    case: &str,
    expected: u16,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Value {
    let answered = control_plane(router, method, uri, JSON_CONTENT_TYPE, &[], body).await;
    assert_eq!(
        answered.status,
        expected,
        "case {case:?}: expected HTTP {expected} from {method} {uri}, got {} ({})",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// The `Id` a control-plane create minted.
fn minted_id(case: &str, created: &Value) -> String {
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted Id, got {created}"))
        .to_string()
}

/// The AppConfig hierarchy this suite deploys into, as the control plane minted it.
struct Hierarchy {
    application_id: String,
    environment_id: String,
    profile_id: String,
    /// The version number of the JSON document the deployment put in force.
    deployed_version: i64,
    /// The version number of the binary document, which is stored but never deployed.
    binary_version: i64,
}

/// Create the application, environment and hosted configuration profile, write the two
/// hosted configuration versions, and deploy the JSON one — all through AppConfig's own
/// operations, exactly as a client sets a configuration up.
async fn deployed_hierarchy(router: &MultiServiceRouter) -> Hierarchy {
    let case = "the hierarchy the handler reads";
    let application = served(
        router,
        case,
        201,
        "POST",
        "/applications",
        json!({ "Name": APPLICATION_NAME }).to_string().as_bytes(),
    )
    .await;
    let application_id = minted_id(case, &application);

    let environment = served(
        router,
        case,
        201,
        "POST",
        &format!("/applications/{application_id}/environments"),
        json!({ "Name": ENVIRONMENT_NAME }).to_string().as_bytes(),
    )
    .await;
    let environment_id = minted_id(case, &environment);

    let profile = served(
        router,
        case,
        201,
        "POST",
        &format!("/applications/{application_id}/configurationprofiles"),
        json!({ "Name": PROFILE_NAME, "LocationUri": HOSTED })
            .to_string()
            .as_bytes(),
    )
    .await;
    let profile_id = minted_id(case, &profile);

    let deployed_version =
        write_version(router, case, &application_id, &profile_id, DEPLOYED, JSON_CONTENT_TYPE).await;
    let binary_version = write_version(
        router,
        case,
        &application_id,
        &profile_id,
        BINARY_DOCUMENT,
        BINARY_CONTENT_TYPE,
    )
    .await;

    let started = served(
        router,
        case,
        201,
        "POST",
        &format!("/applications/{application_id}/environments/{environment_id}/deployments"),
        json!({
            "DeploymentStrategyId": ALL_AT_ONCE,
            "ConfigurationProfileId": profile_id,
            "ConfigurationVersion": deployed_version.to_string(),
        })
        .to_string()
        .as_bytes(),
    )
    .await;
    assert!(
        started["DeploymentNumber"].as_i64().is_some(),
        "case {case:?}: expected StartDeployment to mint a deployment, got {started}"
    );
    // The rollout is a transition AppConfig performs after the call that asked for it, so
    // the fixture waits for the CONDITION — the rollout having reached every target, which
    // is when the version it deployed is the one AppConfig serves — rather than for a span.
    await_rollout_reaching_every_target(
        router,
        case,
        &application_id,
        &environment_id,
        started["DeploymentNumber"].as_i64().unwrap_or_default(),
    )
    .await;

    Hierarchy {
        application_id,
        environment_id,
        profile_id,
        deployed_version,
        binary_version,
    }
}

/// Read one deployment through the control plane until AppConfig reports its rollout has
/// reached every target.
///
/// Every stage of a deployment after the one `StartDeployment` answers — the rollout
/// reaching its targets, the final bake, the completion — is owed by the CLOCK, so this
/// looks again until the condition holds instead of sleeping for a fixed span. Reaching
/// every target is the condition the handler's read depends on: from then on the version
/// this deployment put in service is the one AppConfig serves every caller, whether the
/// deployment is still monitoring its bake time or already complete.
async fn await_rollout_reaching_every_target(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    environment_id: &str,
    number: i64,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let read = control_plane(
            router,
            "GET",
            &format!(
                "/applications/{application_id}/environments/{environment_id}/deployments/{number}"
            ),
            JSON_CONTENT_TYPE,
            &[],
            &[],
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected HTTP 200 from GetDeployment, got {} ({})",
            read.status,
            read.text()
        );
        let document = read.json();
        let reached = document["PercentageComplete"].as_f64().unwrap_or_default();
        if reached >= 100.0 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: deployment {number} reached only {reached}% of its targets, last \
             read {document}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Write one hosted configuration version through the control plane and answer the version
/// number AppConfig minted for it.
async fn write_version(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    profile_id: &str,
    content: &[u8],
    content_type: &str,
) -> i64 {
    let uri = format!(
        "/applications/{application_id}/configurationprofiles/{profile_id}\
         /hostedconfigurationversions"
    );
    let written = control_plane(router, "POST", &uri, content_type, &[], content).await;
    assert_eq!(
        written.status,
        201,
        "case {case:?}: expected HTTP 201 from CreateHostedConfigurationVersion, got {} ({})",
        written.status,
        written.text()
    );
    written
        .header("version-number")
        .and_then(|number| number.parse::<i64>().ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a Version-Number header, got {:?}",
                written.headers
            )
        })
}

/// The bytes a client reads back for one stored hosted configuration version.
async fn stored_version(
    router: &MultiServiceRouter,
    case: &str,
    hierarchy: &Hierarchy,
    version: i64,
) -> Vec<u8> {
    let read = control_plane(
        router,
        "GET",
        &format!(
            "/applications/{}/configurationprofiles/{}/hostedconfigurationversions/{version}",
            hierarchy.application_id, hierarchy.profile_id
        ),
        JSON_CONTENT_TYPE,
        &[],
        &[],
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: expected HTTP 200 from GetHostedConfigurationVersion, got {} ({})",
        read.status,
        read.text()
    );
    read.body.to_vec()
}

/// The bytes a CLIENT's own session and poll serve for the deployed configuration — the
/// control every served row is compared against, so what the handler read is what the data
/// plane serves and not something the runtime composed.
async fn client_poll(router: &MultiServiceRouter, case: &str, hierarchy: &Hierarchy) -> Vec<u8> {
    let session = data_plane(
        router,
        "POST",
        "/configurationsessions",
        json!({
            "ApplicationIdentifier": hierarchy.application_id,
            "EnvironmentIdentifier": hierarchy.environment_id,
            "ConfigurationProfileIdentifier": hierarchy.profile_id,
        })
        .to_string()
        .as_bytes(),
    )
    .await;
    assert_eq!(
        session.status,
        201,
        "case {case:?}: expected HTTP 201 from StartConfigurationSession, got {} ({})",
        session.status,
        session.text()
    );
    let token = session.json()["InitialConfigurationToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a session token, got {}",
                session.text()
            )
        })
        .to_string();
    let polled = data_plane(
        router,
        "GET",
        &format!("/configuration?configuration_token={token}"),
        &[],
    )
    .await;
    assert_eq!(
        polled.status,
        200,
        "case {case:?}: expected HTTP 200 from GetLatestConfiguration, got {} ({})",
        polled.status,
        polled.text()
    );
    polled.body.to_vec()
}

/// The handler every row runs: it reads the configuration the event names through the SDK the
/// runtime supplies and reports the bytes it read — or, when AppConfig refuses the call, falls
/// back to its own default document and reports the refusal's own error code, which is what a
/// feature-flagged handler does.
const HANDLER_SOURCE: &str = r#"
import base64
import json

import boto3
import botocore.exceptions

# The document a handler that cannot reach its configuration has to fall back on.
DEFAULT_CONFIG = {"useNewBackend": False}


def _answered(event):
    """What AppConfig answered the call the event names."""
    if event["call"] == "session":
        client = boto3.client("appconfigdata")
        session = client.start_configuration_session(
            ApplicationIdentifier=event["application"],
            EnvironmentIdentifier=event["environment"],
            ConfigurationProfileIdentifier=event["profile"],
        )
        return client.get_latest_configuration(
            ConfigurationToken=session["InitialConfigurationToken"]
        )
    if event["call"] == "hosted":
        return boto3.client("appconfig").get_hosted_configuration_version(
            ApplicationId=event["application"],
            ConfigurationProfileId=event["profile"],
            VersionNumber=event["version"],
        )
    return boto3.client("appconfig").create_hosted_configuration_version(
        ApplicationId=event["application"],
        ConfigurationProfileId=event["profile"],
        Content=base64.b64decode(event["content"]),
        ContentType=event["content_type"],
    )


def lambda_handler(event, context):
    try:
        answered = _answered(event)
    except botocore.exceptions.ClientError as error:
        return {
            "source": "default",
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "config": DEFAULT_CONFIG,
        }
    except botocore.exceptions.BotoCoreError as error:
        return {
            "source": "default",
            "raised": type(error).__name__,
            "sentence": str(error),
            "config": DEFAULT_CONFIG,
        }
    carried = answered["Configuration"] if "Configuration" in answered else answered["Content"]
    read = carried.read()
    reported = {
        "source": "appconfig",
        "is_bytes": isinstance(read, (bytes, bytearray)),
        "bytes": base64.b64encode(read).decode("ascii"),
        "content_type": answered.get("ContentType"),
        "version": answered.get("VersionNumber"),
    }
    try:
        reported["config"] = json.loads(read.decode("utf-8"))
    except ValueError:
        reported["config"] = None
    return reported
"#;

/// What must come of one row's call.
enum Expect {
    /// AppConfig served it, and these are the bytes the handler must have read.
    Served {
        bytes: Vec<u8>,
        content_type: &'static str,
    },
    /// AppConfig refused it with this error code of its own, so the handler falls back.
    Refused(&'static str),
}

/// One call the handler makes, and what must come of it.
struct Row {
    case: &'static str,
    /// The event the handler is invoked with.
    event: Value,
    expect: Expect,
}

/// The rows, spanning what the rule generalises over.
///
/// A logical-id-shaped identifier is a string a template's `Ref` resolves to for a resource
/// whose own id it never is: it names no application, environment or configuration profile, so
/// each level that is given one is unresolvable.
fn rows(hierarchy: &Hierarchy) -> Vec<Row> {
    let application_id = hierarchy.application_id.as_str();
    let environment_id = hierarchy.environment_id.as_str();
    let profile_id = hierarchy.profile_id.as_str();
    let logical = "OrdersFlagsApplicationE2B4F2A1";
    vec![
        Row {
            case: "the data plane, with the three real ids",
            event: json!({
                "call": "session",
                "application": application_id,
                "environment": environment_id,
                "profile": profile_id,
            }),
            expect: Expect::Served {
                bytes: DEPLOYED.to_vec(),
                content_type: JSON_CONTENT_TYPE,
            },
        },
        Row {
            case: "the data plane, with each level given its NAME instead of its id",
            event: json!({
                "call": "session",
                "application": APPLICATION_NAME,
                "environment": ENVIRONMENT_NAME,
                "profile": PROFILE_NAME,
            }),
            expect: Expect::Served {
                bytes: DEPLOYED.to_vec(),
                content_type: JSON_CONTENT_TYPE,
            },
        },
        Row {
            case: "the data plane, with a logical-id-shaped application identifier",
            event: json!({
                "call": "session",
                "application": logical,
                "environment": environment_id,
                "profile": profile_id,
            }),
            expect: Expect::Refused(NOT_FOUND),
        },
        Row {
            case: "the data plane, with a logical-id-shaped environment identifier",
            event: json!({
                "call": "session",
                "application": application_id,
                "environment": "OrdersFlagsEnvironment7C1D",
                "profile": profile_id,
            }),
            expect: Expect::Refused(NOT_FOUND),
        },
        Row {
            case: "the data plane, with a logical-id-shaped profile identifier",
            event: json!({
                "call": "session",
                "application": application_id,
                "environment": environment_id,
                "profile": "OrdersFlagsProfile9A3B",
            }),
            expect: Expect::Refused(NOT_FOUND),
        },
        Row {
            case: "the data plane, with logical-id-shaped identifiers for all three levels",
            event: json!({
                "call": "session",
                "application": logical,
                "environment": "OrdersFlagsEnvironment7C1D",
                "profile": "OrdersFlagsProfile9A3B",
            }),
            expect: Expect::Refused(NOT_FOUND),
        },
        Row {
            case: "the control plane, reading the deployed document's own stored version",
            event: json!({
                "call": "hosted",
                "application": application_id,
                "profile": profile_id,
                "version": hierarchy.deployed_version,
            }),
            expect: Expect::Served {
                bytes: DEPLOYED.to_vec(),
                content_type: JSON_CONTENT_TYPE,
            },
        },
        Row {
            case: "the control plane, reading a stored version that is not text at all",
            event: json!({
                "call": "hosted",
                "application": application_id,
                "profile": profile_id,
                "version": hierarchy.binary_version,
            }),
            expect: Expect::Served {
                bytes: BINARY_DOCUMENT.to_vec(),
                content_type: BINARY_CONTENT_TYPE,
            },
        },
        Row {
            case: "the control plane, reading a version number nothing stored",
            event: json!({
                "call": "hosted",
                "application": application_id,
                "profile": profile_id,
                "version": 99,
            }),
            expect: Expect::Refused(NOT_FOUND),
        },
        Row {
            case: "the control plane, writing a version whose content is not text at all",
            event: json!({
                "call": "write",
                "application": application_id,
                "profile": profile_id,
                "content": base64::engine::general_purpose::STANDARD.encode(HANDLER_WRITTEN),
                "content_type": BINARY_CONTENT_TYPE,
            }),
            expect: Expect::Served {
                bytes: HANDLER_WRITTEN.to_vec(),
                content_type: BINARY_CONTENT_TYPE,
            },
        },
    ]
}

/// Invoke `function` with `event`, as `(the FunctionError signal, the payload)`.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let answered = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} ({})",
        answered.status,
        answered.text()
    );
    let signal = answered.header("x-amz-function-error");
    (signal, answered.json())
}

/// The bytes the handler reported reading, decoded from the base64 it carried them in.
fn reported_bytes(case: &str, payload: &Value) -> Vec<u8> {
    let encoded = payload["bytes"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the bytes the handler read, got {payload}"));
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected base64 bytes, got {encoded:?} ({error})")
        })
}

#[tokio::test]
async fn a_handler_reads_its_configuration_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = emulator();
    let hierarchy = deployed_hierarchy(&router).await;

    // The suite's own control: a CLIENT's session and poll serve the deployed document, so a
    // row that reads the same bytes read them from the data plane.
    let control = "a client's own session and poll";
    let polled = client_poll(&router, control, &hierarchy).await;
    assert_eq!(
        polled,
        DEPLOYED.to_vec(),
        "case {control:?}: expected the deployed document to be served to a client, got \
         {polled:?}"
    );

    let function = "appconfig-sdk-caller";
    let created = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        json!({
            "FunctionName": function,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 25,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("lambda_function.py", HANDLER_SOURCE)])),
            },
        })
        .to_string()
        .as_bytes(),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "expected HTTP 201 from CreateFunction, got {} ({})",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    for row in rows(&hierarchy) {
        let case = row.case;
        let (signal, payload) = invoke(&router, case, function, &row.event).await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        assert_eq!(
            payload["raised"],
            Value::Null,
            "case {case:?}: the runtime's SDK must reach the service rather than raise a \
             client-side failure, got {payload}"
        );

        match &row.expect {
            Expect::Served {
                bytes,
                content_type,
            } => {
                assert_eq!(
                    payload["source"],
                    json!("appconfig"),
                    "case {case:?}: expected the handler to be served its configuration, got \
                     {payload}"
                );
                assert_eq!(
                    payload["is_bytes"],
                    json!(true),
                    "case {case:?}: expected the SDK to hand the payload over as bytes, got \
                     {payload}"
                );
                let read = reported_bytes(case, &payload);
                assert_eq!(
                    read,
                    *bytes,
                    "case {case:?}: expected the document's own bytes {:?}, got {read:?}",
                    bytes
                );
                assert_eq!(
                    payload["content_type"],
                    json!(content_type),
                    "case {case:?}: expected the document's own content type, got {payload}"
                );
                if *content_type == JSON_CONTENT_TYPE {
                    assert_eq!(
                        payload["config"],
                        json!({ "useNewBackend": true }),
                        "case {case:?}: expected the deployed flag document, got {payload}"
                    );
                }
            }
            Expect::Refused(code) => {
                assert_eq!(
                    payload["source"],
                    json!("default"),
                    "case {case:?}: expected the handler to fall back on its own default, got \
                     {payload}"
                );
                assert_eq!(
                    payload["code"],
                    json!(code),
                    "case {case:?}: expected the service's own error code, got {payload}"
                );
                assert_eq!(
                    payload["config"],
                    fallback(),
                    "case {case:?}: expected the handler's own default document, got {payload}"
                );
                let message = payload["message"].as_str().unwrap_or_default();
                assert!(
                    !message.is_empty(),
                    "case {case:?}: expected the service's own message, got {payload}"
                );
            }
        }

        // What the service itself holds for the row that WROTE a version: the bytes the
        // handler passed, read back through the control plane's own API.
        if case == "the control plane, writing a version whose content is not text at all" {
            let version = payload["version"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: expected a version number, got {payload}"));
            let held = stored_version(&router, case, &hierarchy, version).await;
            assert_eq!(
                held,
                HANDLER_WRITTEN.to_vec(),
                "case {case:?}: expected AppConfig to hold the bytes the handler wrote, got \
                 {held:?}"
            );
        }
    }

    // And the two documents the SUITE wrote are still exactly what AppConfig stored, so no
    // read above changed them.
    let case = "the service's own view of the documents the handler read";
    for (version, expected) in [
        (hierarchy.deployed_version, DEPLOYED.to_vec()),
        (hierarchy.binary_version, BINARY_DOCUMENT.to_vec()),
    ] {
        let held = stored_version(&router, case, &hierarchy, version).await;
        assert_eq!(
            held, expected,
            "case {case:?}: expected version {version} to hold {expected:?}, got {held:?}"
        );
    }
}
