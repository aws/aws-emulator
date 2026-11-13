// SPDX-License-Identifier: CC0-1.0

//! Whose authority a RUNNING handler's own configuration read is judged by, end to end over
//! the wire: AWS Lambda, the AppConfig control plane, the AppConfig data plane, IAM and the
//! token service on ONE engine, and a `python3.x` function whose code does `import boto3` and
//! opens a configuration session.
//!
//! RULE: a call a handler makes is a call made BY the function's EXECUTION ROLE, so the
//! service serving it decides it against the identity-based policies in force on that role: a
//! role no readable statement grants the action — because none grants it, because the grant
//! names another configuration, or because a statement explicitly denies it — is refused
//! `AccessDeniedException` (HTTP 403) carrying IAM's own denial sentence, and the same read is
//! served once the grant is in force. That refusal is a different claim from the modelled
//! `ResourceNotFoundException` (HTTP 404) an identifier that resolves to nothing gets, which
//! is what lets a handler tell a missing GRANT from a wrong IDENTIFIER — and what the two
//! faults an operator must be able to distinguish come down to.
//!
//! The rows differ along what the rule generalises over: which of the data plane's two actions
//! the role reaches (both, neither, only the session), whether the grant names THIS
//! configuration or another one, and whether the deciding statement allows, states nothing or
//! explicitly denies. The sibling suite `handler_appconfig_sdk.rs` pins what a handler reads
//! when it IS authorized — the deployed document, byte for byte, over both of AppConfig's APIs
//! — and the data plane's own crate pins the same decision taken for a client
//! (`service-com-amazonaws-appconfigdata-appconfigdata/tests/all/identity_authorization.rs`);
//! what this suite adds is that the identity a handler's call is decided as is its function's
//! role.
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

use crate::invoke_request_id::one_invocation_stream_at_a_time;
// The one .zip writer this crate's suites package a handler with, exported by the suite that
// first needed it rather than written again here.
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for, and the one the function's own invocation —
/// and therefore the client its handler builds — runs in.
const REGION: &str = "us-east-1";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// The access-key id standing for the account's own published credential, which is what every
/// request this suite makes on its own behalf presents.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The name of the inline identity policy a row writes on the execution role.
const INLINE_POLICY: &str = "inline";

/// The error code and status AWS states an authorization failure with.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: i64 = 403;

/// The two actions AppConfig publishes the data plane's operations under, which is how a
/// policy statement spells them.
const START_SESSION_ACTION: &str = "appconfig:StartConfigurationSession";
const GET_CONFIGURATION_ACTION: &str = "appconfig:GetLatestConfiguration";

/// The names the hierarchy is created under, and the document the deployment puts in force.
const APPLICATION_NAME: &str = "orders-grants";
const ENVIRONMENT_NAME: &str = "production";
const PROFILE_NAME: &str = "backend-rollout";
const DEPLOYED: &str = r#"{"useNewBackend": true}"#;

/// The predefined deployment strategy that reaches every target at once, so the deployment's
/// rollout has begun the instant `StartDeployment` answers.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// Lambda, both of AppConfig's APIs, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router and signed as the session of its own execution role, so the policies that
/// decide it are the ones this suite wrote with IAM's own operations.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The handler's OWN call is signed by the runtime, with the credential it was given.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
    /// the request and where the Region comes from. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One `restJson1` call to the service answering at the endpoint host `host_prefix`: its
    /// status, its headers and its parsed body.
    async fn rest(
        &self,
        signing_name: &str,
        host_prefix: &str,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> (u16, http::HeaderMap, Value) {
        let host = format!("{host_prefix}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", Self::authorization(signing_name))
            .header("content-type", content_type)
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        let parsed: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
        };
        (parts.status.as_u16(), parts.headers, parsed)
    }

    /// One AppConfig control-plane call that must be served, answering its body.
    async fn appconfig(&self, what: &str, path: &str, content_type: &str, body: &[u8]) -> Value {
        let (status, _headers, parsed) = self
            .rest("appconfig", "appconfig", "POST", path, content_type, body)
            .await;
        assert_eq!(
            status, 201,
            "setup {what}: expected HTTP 201, got {status} ({parsed})"
        );
        parsed
    }

    /// The `Id` a control-plane create minted.
    async fn created_id(&self, what: &str, path: &str, body: &Value) -> String {
        let created = self
            .appconfig(
                what,
                path,
                "application/json",
                &serde_json::to_vec(body).expect("the request body serializes"),
            )
            .await;
        created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("setup {what}: expected a minted Id, got {created}"))
            .to_string()
    }

    /// Read one deployment through AppConfig's control plane until its rollout has reached
    /// every target.
    ///
    /// Every stage of a deployment after the one `StartDeployment` answers — the rollout
    /// reaching its targets, the final bake, the completion — is owed by the CLOCK, so this
    /// looks again until the condition holds instead of sleeping for a fixed span. Reaching
    /// every target is the condition a handler's read depends on: from then on the version
    /// the deployment put in service is the one AppConfig serves every caller.
    async fn await_rollout_reaching_every_target(
        &self,
        application: &str,
        environment: &str,
        number: i64,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let (status, _headers, parsed) = self
                .rest(
                    "appconfig",
                    "appconfig",
                    "GET",
                    &format!(
                        "/applications/{application}/environments/{environment}\
                         /deployments/{number}"
                    ),
                    "application/json",
                    &[],
                )
                .await;
            assert_eq!(
                status, 200,
                "setup GetDeployment: expected HTTP 200, got {status} ({parsed})"
            );
            let reached = parsed["PercentageComplete"].as_f64().unwrap_or_default();
            if reached >= 100.0 {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "setup GetDeployment: deployment {number} reached only {reached}% of its \
                 targets, last read {parsed}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// One awsQuery call to IAM that must be served.
    async fn iam(&self, what: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "setup {what}: IAM {action} must succeed, got {status} {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// Create the execution role `role_name`, trusted by Lambda, carrying `policy` as its
    /// inline identity policy — written with IAM's own operations, so the role a session is
    /// minted of is the record `iam:CreateRole` wrote.
    async fn execution_role(&self, role_name: &str, policy: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            "the execution role",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.role_policy("the execution role", role_name, policy)
            .await;
        format!(
            "arn:aws:iam::{}:role/{role_name}",
            engine::emulator_account_id()
        )
    }

    /// The identity policy in force on `role_name` from now on, so a row is about the grant in
    /// force when the handler calls rather than the one in force when the role was made.
    async fn role_policy(&self, what: &str, role_name: &str, document: &str) {
        self.iam(
            what,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
                ("PolicyDocument", document.to_string()),
            ],
        )
        .await;
    }

    /// Create a function whose code is `source`, running as `role_arn`, and wait out the
    /// provisioning window.
    async fn function(&self, name: &str, role_arn: &str, source: &str) {
        let (status, _headers, created) = self
            .rest(
                "lambda",
                "lambda",
                "POST",
                FUNCTIONS_URI,
                "application/json",
                &serde_json::to_vec(&json!({
                    "FunctionName": name,
                    "Role": role_arn,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Timeout": 25,
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD
                            .encode(package(&[("lambda_function.py", source)])),
                    },
                }))
                .expect("the create body serializes"),
            )
            .await;
        assert_eq!(
            status, 201,
            "setup: expected HTTP 201 from CreateFunction, got {status} ({created})"
        );
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    }

    /// Invoke the function once with the triple it is to read, and answer what the handler
    /// reported — asserting the invocation itself was served and the handler ran to
    /// completion, which is what a refusal the handler CAUGHT looks like from outside.
    async fn read(&self, case: &str, function: &str, event: &Value) -> Value {
        let (status, headers, payload) = self
            .rest(
                "lambda",
                "lambda",
                "POST",
                &format!("{FUNCTIONS_URI}/{function}/invocations"),
                "application/json",
                event.to_string().as_bytes(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        payload
    }
}

/// `application/x-www-form-urlencoded` for one awsQuery parameter value, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive the
/// round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One identity policy document granting `actions` on `resource`.
fn allow(actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// The ARN of the configuration a session over this triple is served from, as AppConfig
/// addresses one — stated here independently of the crates under test, so the sentence a
/// refusal carries is compared against an ARN this suite built.
fn configuration_arn(application: &str, environment: &str, profile: &str) -> String {
    format!(
        "arn:aws:appconfig:{REGION}:{}:application/{application}/environment/{environment}\
         /configuration/{profile}",
        engine::emulator_account_id()
    )
}

/// The assumed-role session a function's own calls are made as:
/// `arn:aws:sts::<account>:assumed-role/<role name>/<function name>`, the identity AWS reports
/// for a function's calls.
fn session_principal(role_name: &str, function_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{function_name}",
        engine::emulator_account_id()
    )
}

/// The sentence IAM refuses a request with, built here from the principal, the action and the
/// resource rather than read off the crates under test.
fn denial(principal: &str, action: &str, resource: &str, explicit_deny: bool) -> String {
    let reason = if explicit_deny {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         {reason}"
    )
}

/// The handler every row runs: it opens a configuration session over the three identifiers the
/// event names, polls it once, and on ANY refusal reports the error code and falls back to the
/// default compiled into it — the shape a function that reads its own feature flags has.
const HANDLER_SOURCE: &str = r#"
import json

import boto3
import botocore.exceptions

CLIENT = boto3.client("appconfigdata")


def lambda_handler(event, context):
    try:
        session = CLIENT.start_configuration_session(
            ApplicationIdentifier=event["application"],
            EnvironmentIdentifier=event["environment"],
            ConfigurationProfileIdentifier=event["profile"],
        )
        answered = CLIENT.get_latest_configuration(
            ConfigurationToken=session["InitialConfigurationToken"]
        )
    except botocore.exceptions.ClientError as error:
        return {
            "source": "default",
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
            "config": {"useNewBackend": False},
        }
    read = answered["Configuration"].read()
    return {"source": "appconfig", "config": json.loads(read.decode("utf-8"))}
"#;

/// The hierarchy the handler reads, as the control plane minted it.
struct Hierarchy {
    application: String,
    environment: String,
    profile: String,
}

/// Create an application, an environment and a hosted configuration profile and deploy
/// [`DEPLOYED`] into it, all through AppConfig's own control plane — so what the handler reads
/// is what a client deployed.
async fn deploy(emulator: &Emulator) -> Hierarchy {
    let application = emulator
        .created_id(
            "CreateApplication",
            "/applications",
            &json!({"Name": APPLICATION_NAME}),
        )
        .await;
    let environment = emulator
        .created_id(
            "CreateEnvironment",
            &format!("/applications/{application}/environments"),
            &json!({"Name": ENVIRONMENT_NAME}),
        )
        .await;
    let profile = emulator
        .created_id(
            "CreateConfigurationProfile",
            &format!("/applications/{application}/configurationprofiles"),
            &json!({
                "Name": PROFILE_NAME,
                "LocationUri": "hosted",
                "Type": "AWS.Freeform",
            }),
        )
        .await;
    emulator
        .appconfig(
            "CreateHostedConfigurationVersion",
            &format!(
                "/applications/{application}/configurationprofiles/{profile}\
                 /hostedconfigurationversions"
            ),
            "application/json",
            DEPLOYED.as_bytes(),
        )
        .await;
    let started = emulator
        .appconfig(
            "StartDeployment",
            &format!("/applications/{application}/environments/{environment}/deployments"),
            "application/json",
            &serde_json::to_vec(&json!({
                "DeploymentStrategyId": ALL_AT_ONCE,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }))
            .expect("the request body serializes"),
        )
        .await;
    // The rollout is a transition AppConfig performs after the call that asked for it, so
    // the fixture waits for it to reach every target — which is when the version it
    // deployed is the one AppConfig serves every reader.
    emulator
        .await_rollout_reaching_every_target(
            &application,
            &environment,
            started["DeploymentNumber"].as_i64().unwrap_or_default(),
        )
        .await;
    Hierarchy {
        application,
        environment,
        profile,
    }
}

/// One row of the grant table: the policy in force on the execution role, and what the
/// handler's read must answer under it.
struct Row {
    case: &'static str,
    /// The inline policy the role carries, built from the ARN of the configuration the
    /// handler's call addresses.
    policy: fn(&str) -> String,
    /// The action the refusal must name, or `None` when the read must be served.
    refused_action: Option<&'static str>,
    /// Whether the refusal names an explicit deny rather than an absent grant.
    explicit_deny: bool,
}

/// The rows, each read under its own policy: which of the two actions the role reaches, a role
/// granted none of AppConfig's actions at all, a grant naming this configuration and one
/// naming another, and an explicit deny over a wildcard grant.
const ROWS: &[Row] = &[
    Row {
        case: "a role granted both of the data plane's actions",
        policy: |_arn| allow(&[START_SESSION_ACTION, GET_CONFIGURATION_ACTION], "*"),
        refused_action: None,
        explicit_deny: false,
    },
    Row {
        case: "a role granted no AppConfig action at all",
        policy: |_arn| {
            allow(
                &[
                    "logs:CreateLogGroup",
                    "logs:CreateLogStream",
                    "logs:PutLogEvents",
                ],
                "*",
            )
        },
        refused_action: Some(START_SESSION_ACTION),
        explicit_deny: false,
    },
    Row {
        case: "a role granted the session but not the poll",
        policy: |_arn| allow(&[START_SESSION_ACTION], "*"),
        refused_action: Some(GET_CONFIGURATION_ACTION),
        explicit_deny: false,
    },
    Row {
        case: "a role whose grant names this configuration",
        policy: |arn| allow(&[START_SESSION_ACTION, GET_CONFIGURATION_ACTION], arn),
        refused_action: None,
        explicit_deny: false,
    },
    Row {
        case: "a role whose grant names another configuration",
        policy: |_arn| {
            allow(
                &[START_SESSION_ACTION, GET_CONFIGURATION_ACTION],
                &configuration_arn("other", "other", "other"),
            )
        },
        refused_action: Some(START_SESSION_ACTION),
        explicit_deny: false,
    },
    Row {
        case: "an explicit deny over a wildcard grant",
        policy: |_arn| {
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "appconfig:*", "Resource": "*"},
                    {"Effect": "Deny", "Action": START_SESSION_ACTION, "Resource": "*"},
                ],
            })
            .to_string()
        },
        refused_action: Some(START_SESSION_ACTION),
        explicit_deny: true,
    },
];

/// RULE: a handler's configuration session is opened and polled as its EXECUTION ROLE, so the
/// data plane decides it against the policies in force on that role — a role whose
/// identity-based policies do not allow the action is refused `AccessDeniedException` (HTTP
/// 403) carrying IAM's own sentence, never the not-found refusal a wrong identifier gets, and
/// the same read is served once the grant is in force.
#[tokio::test]
async fn a_handlers_configuration_read_is_decided_by_its_execution_role() {
    let _stream = one_invocation_stream_at_a_time().await;
    let role_name = "lambda-appconfig-grants";
    let function_name = "appconfig-grant-reader";
    let emulator = Emulator::new();
    let hierarchy = deploy(&emulator).await;
    let resource = configuration_arn(
        &hierarchy.application,
        &hierarchy.environment,
        &hierarchy.profile,
    );
    let role = emulator
        .execution_role(role_name, &(ROWS[0].policy)(&resource))
        .await;
    emulator
        .function(function_name, &role, HANDLER_SOURCE)
        .await;
    let principal = session_principal(role_name, function_name);
    let event = json!({
        "application": hierarchy.application,
        "environment": hierarchy.environment,
        "profile": hierarchy.profile,
    });

    for row in ROWS {
        let case = row.case;
        emulator
            .role_policy(case, role_name, &(row.policy)(&resource))
            .await;
        let answer = emulator.read(case, function_name, &event).await;
        let Some(action) = row.refused_action else {
            assert_eq!(
                answer["source"],
                json!("appconfig"),
                "case {case:?}: expected the authorized read to be served, got {answer}"
            );
            assert_eq!(
                answer["config"],
                json!({"useNewBackend": true}),
                "case {case:?}: expected the deployed flag document, got {answer}"
            );
            continue;
        };
        assert_eq!(
            answer["source"],
            json!("default"),
            "case {case:?}: expected the handler to fall back to its default, got {answer}"
        );
        assert_eq!(
            answer["code"],
            json!(ACCESS_DENIED),
            "case {case:?}: expected the authorization refusal rather than a not-found one, \
             got {answer}"
        );
        assert_eq!(
            answer["status"],
            json!(ACCESS_DENIED_STATUS),
            "case {case:?}: expected HTTP 403 for an unauthorized caller, got {answer}"
        );
        assert_eq!(
            answer["message"],
            json!(denial(&principal, action, &resource, row.explicit_deny)),
            "case {case:?}: expected IAM's own denial sentence, got {answer}"
        );
        assert_eq!(
            answer["config"],
            json!({"useNewBackend": false}),
            "case {case:?}: expected the handler's own default document, got {answer}"
        );
    }
}
