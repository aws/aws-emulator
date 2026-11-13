// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM AUTHORIZATION of this data plane's two operations.
//!
//! RULE: a configuration session is opened and polled under the CALLER's own credentials, and
//! AWS decides such a call against that principal's identity-based IAM policies before the
//! service reads the request — the principal being the identity the access key id the request
//! presents speaks for, and for an `sts:AssumeRole` credential the session, whose permissions
//! are the role's. The decision is IAM's own: an explicit `Deny` about the request wins over
//! every `Allow`, an `Allow` with none permits, and a principal with neither falls to the
//! default deny. The action is `appconfig:<OperationName>` — this service signs as
//! `appconfig` — and the resource is the CONFIGURATION the call addresses, built from the
//! identifiers it carried. A refusal is `AccessDeniedException` with HTTP 403, carrying IAM's
//! own sentence, and it persists nothing: no session is minted and no token is consumed.
//!
//! That status and code are what makes a MISSING GRANT distinguishable from an identifier
//! that resolves to nothing, which is the modelled `ResourceNotFoundException` with HTTP 404 —
//! the two faults an operator has to tell apart.
//!
//! The rows span what the rule generalises over: which of the two operations is asked for,
//! whether the principal holds no policy at all / a broad grant / a grant scoped to THIS
//! configuration / a grant scoped to ANOTHER one / a grant plus an explicit deny, and — for
//! the poll, whose own input names only its token — whether the token resolves to a session
//! at all. Beside the table: the account's own published credential, which holds no IAM
//! policies, keeps being served.
//!
//! The principals are made with IAM's and STS's OWN operations over the same engine, and the
//! hierarchy with AppConfig's, so the documents this service reads are the ones those
//! services wrote.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state::ComAmazonawsAppconfigAmazonappconfigState;
use service_com_amazonaws_appconfigdata_appconfigdata::state::ComAmazonawsAppconfigdataAppconfigdataState;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the ARNs a decision is taken
/// against therefore carry.
const REGION: &str = "us-east-1";

/// The two operations under test, by the names their own model gives them.
const START_SESSION: &str = "StartConfigurationSession";
const GET_CONFIGURATION: &str = "GetLatestConfiguration";

/// The namespace AppConfig publishes this data plane's actions under, which is also this
/// service's SigV4 signing name.
const ACTION_PREFIX: &str = "appconfig";

/// The code and status an authorization refusal carries.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The code and status an identifier that resolves to nothing carries — the modelled
/// refusal, which an authorization failure must never be confused with.
const NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's ambient identity, which holds no IAM
/// policies — the position AWS account root credentials are in. Every fixture is made with it.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names as the principal.
const SESSION: &str = "probe";

/// The predefined deployment strategy that reaches every target in one growth interval, so
/// a deployment's rollout has begun the instant `StartDeployment` answers.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// The document the hierarchy below deploys, which an authorized poll must serve.
const DEPLOYED: &[u8] = br#"{"useNewBackend": true}"#;

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// This data plane beside the AppConfig control plane that holds the hierarchy and the IAM and
/// STS services its callers are made with, all on ONE shared engine.
///
/// `super::configuration_session_poll::Emulator` mounts the two AppConfig services alone and
/// signs every call with one key, which is right for a suite about what this service serves; a
/// suite about WHO is calling needs the two services that hold principals and their policies,
/// and a credential scope naming a different key per call, so it assembles the router here.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here signs: the `Authorization` header carries only the credential scope,
        // which is where a service reads the caller's access key id and Region from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(
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
                Arc::clone(&engine),
            ),
        );
        Self { engine, router }
    }

    /// This service's session store, as the handlers reach it — read to prove a refused call
    /// minted nothing.
    fn sessions(&self) -> usize {
        self.engine
            .state::<ComAmazonawsAppconfigdataAppconfigdataState>()
            .lock()
            .sessions
            .len()
    }

    /// Let the stages the clock owes the environment's deployments fall due.
    ///
    /// A rollout reaching its targets, the final bake and the completion are transitions
    /// AppConfig performs AFTER the call that started the deployment, so a fixture that
    /// needs the deployed configuration in force moves the start instant into the past and
    /// brings the records up to what the clock then owes them — rather than sleeping for
    /// the stages.
    fn settle_deployments(&self, application_id: &str, environment_id: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsAppconfigAmazonappconfigState>();
        let mut store = state.lock();
        let Some(environment) = store
            .application_mut(application_id)
            .and_then(|application| application.environments.get_mut(environment_id))
        else {
            return;
        };
        for deployment in environment.deployments.values_mut() {
            deployment.started_at -= Duration::from_secs(24 * 60 * 60);
        }
        environment.materialize(SystemTime::now());
    }

    /// One `restJson1` call to the service signing as `signing_name`, signed for [`REGION`]
    /// with `access_key_id` in the credential scope: its status, its body as bytes and its
    /// body parsed as JSON.
    async fn call_as(
        &self,
        access_key_id: &str,
        signing_name: &str,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> (u16, Vec<u8>, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", content_type)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, \
                     Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a valid test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body().to_vec();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, payload, parsed)
    }

    /// `StartConfigurationSession` over one triple, as `principal`.
    async fn start_session(
        &self,
        access_key_id: &str,
        application: &str,
        environment: &str,
        profile: &str,
    ) -> (u16, Value) {
        let (status, _bytes, parsed) = self
            .call_as(
                access_key_id,
                "appconfig",
                "POST",
                "/configurationsessions",
                "application/json",
                &serde_json::to_vec(&json!({
                    "ApplicationIdentifier": application,
                    "EnvironmentIdentifier": environment,
                    "ConfigurationProfileIdentifier": profile,
                }))
                .expect("the request body serializes"),
            )
            .await;
        (status, parsed)
    }

    /// `GetLatestConfiguration` with one token, as `principal`.
    async fn poll(&self, access_key_id: &str, token: &str) -> (u16, Vec<u8>, Value) {
        self.call_as(
            access_key_id,
            "appconfig",
            "GET",
            &format!("/configuration?configuration_token={token}"),
            "application/json",
            &[],
        )
        .await
    }

    /// One AppConfig control-plane call, made with the account's own credential, that must be
    /// served.
    async fn appconfig(&self, what: &str, path: &str, content_type: &str, body: &[u8]) -> Value {
        let (status, _bytes, parsed) = self
            .call_as(ACCOUNT_KEY, "appconfig", "POST", path, content_type, body)
            .await;
        assert_eq!(
            status, 201,
            "setup {what}: expected HTTP 201, got {status} with {parsed}"
        );
        parsed
    }

    /// One awsQuery call to IAM or STS, made with the account's own credential, that must be
    /// served.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "setup {action}: the call must succeed, got {status} with {text}"
        );
        text
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
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

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create an IAM role carrying `policies` as its inline policies, assume it through STS, and
/// answer the session's access key id with the session ARN.
async fn assumed_role(aws: &Emulator, role_name: &str, policies: &[String]) -> Principal {
    let account = engine::emulator_account_id();
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    aws.query(
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    for (index, policy) in policies.iter().enumerate() {
        aws.query(
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", format!("inline-{index}")),
                ("PolicyDocument", policy.clone()),
            ],
        )
        .await;
    }
    let assumed = aws
        .query(
            "sts",
            "2011-06-15",
            "AssumeRole",
            &[
                ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
                ("RoleSessionName", SESSION.to_string()),
            ],
        )
        .await;
    Principal {
        access_key_id: xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
        arn: xml_element(&assumed, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
    }
}

/// One identity-based policy document carrying one statement.
fn statement(effect: &str, actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": effect, "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// The hierarchy every case is decided against: the ids the control plane minted, and the ARN
/// of the configuration a session over them addresses.
struct Deployed {
    application: String,
    environment: String,
    profile: String,
}

impl Deployed {
    /// The configuration ARN this triple addresses, spelled here independently of the crate
    /// under test — as AppConfig's Service Authorization Reference spells the `configuration`
    /// resource type.
    fn arn(&self) -> String {
        format!(
            "arn:aws:{ACTION_PREFIX}:{REGION}:{}:application/{}/environment/{}/configuration/{}",
            engine::emulator_account_id(),
            self.application,
            self.environment,
            self.profile
        )
    }
}

/// Create an application, an environment and a hosted configuration profile and deploy
/// [`DEPLOYED`] into it, all through AppConfig's own control plane.
async fn deploy(aws: &Emulator) -> Deployed {
    let created = aws
        .appconfig(
            "CreateApplication",
            "/applications",
            "application/json",
            br#"{"Name":"orders"}"#,
        )
        .await;
    let application = minted(&created, "CreateApplication");
    let created = aws
        .appconfig(
            "CreateEnvironment",
            &format!("/applications/{application}/environments"),
            "application/json",
            br#"{"Name":"prod"}"#,
        )
        .await;
    let environment = minted(&created, "CreateEnvironment");
    let created = aws
        .appconfig(
            "CreateConfigurationProfile",
            &format!("/applications/{application}/configurationprofiles"),
            "application/json",
            br#"{"Name":"flags","LocationUri":"hosted","Type":"AWS.Freeform"}"#,
        )
        .await;
    let profile = minted(&created, "CreateConfigurationProfile");
    aws.appconfig(
        "CreateHostedConfigurationVersion",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}\
             /hostedconfigurationversions"
        ),
        "application/json",
        DEPLOYED,
    )
    .await;
    aws.appconfig(
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
    aws.settle_deployments(&application, &environment);
    Deployed {
        application,
        environment,
        profile,
    }
}

/// The `Id` a control-plane create answered with.
fn minted(created: &Value, what: &str) -> String {
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {what}: expected an Id, got {created}"))
        .to_string()
}

/// One row of the grant table: the policies in force on the caller's role, and what each of
/// the two operations must answer under them.
struct Row {
    case: &'static str,
    /// The documents the role carries, built from the configuration ARN the call addresses.
    policies: fn(&str) -> Vec<String>,
    /// The action the refusal of `StartConfigurationSession` must name, or `None` when the
    /// session must be minted.
    session_refused: Option<&'static str>,
    /// The same for the poll, which runs with the session the first leg minted (or, where the
    /// session was refused, with a token minted for an authorized caller).
    poll_refused: Option<&'static str>,
    /// Whether the refusal names an explicit deny rather than an absent grant.
    explicit_deny: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a role carrying no policy at all",
        policies: |_arn| Vec::new(),
        session_refused: Some(START_SESSION),
        poll_refused: Some(GET_CONFIGURATION),
        explicit_deny: false,
    },
    Row {
        case: "a role granted every AppConfig action",
        policies: |_arn| vec![statement("Allow", &["appconfig:*"], "*")],
        session_refused: None,
        poll_refused: None,
        explicit_deny: false,
    },
    Row {
        case: "a role granted both actions on this configuration",
        policies: |arn| {
            vec![statement(
                "Allow",
                &["appconfig:StartConfigurationSession", "appconfig:GetLatestConfiguration"],
                arn,
            )]
        },
        session_refused: None,
        poll_refused: None,
        explicit_deny: false,
    },
    Row {
        case: "a role granted both actions on another configuration",
        policies: |_arn| {
            vec![statement(
                "Allow",
                &["appconfig:StartConfigurationSession", "appconfig:GetLatestConfiguration"],
                "arn:aws:appconfig:us-east-1:000000000000:application/other/environment/other\
                 /configuration/other",
            )]
        },
        session_refused: Some(START_SESSION),
        poll_refused: Some(GET_CONFIGURATION),
        explicit_deny: false,
    },
    Row {
        case: "a role granted the session but not the poll",
        policies: |_arn| {
            vec![statement(
                "Allow",
                &["appconfig:StartConfigurationSession"],
                "*",
            )]
        },
        session_refused: None,
        poll_refused: Some(GET_CONFIGURATION),
        explicit_deny: false,
    },
    Row {
        case: "a role granted every action with the poll explicitly denied",
        policies: |_arn| {
            vec![
                statement("Allow", &["appconfig:*"], "*"),
                statement("Deny", &["appconfig:GetLatestConfiguration"], "*"),
            ]
        },
        session_refused: None,
        poll_refused: Some(GET_CONFIGURATION),
        explicit_deny: true,
    },
];

/// The sentence IAM refuses a request with, stated here rather than read off the crate under
/// test: who was refused, which action, on which resource, and which of IAM's rules refused.
fn denial(principal: &str, action: &str, resource: &str, explicit_deny: bool) -> String {
    let reason = if explicit_deny {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {ACTION_PREFIX}:{action} action")
    };
    format!(
        "User: {principal} is not authorized to perform: {ACTION_PREFIX}:{action} on \
         resource: {resource} {reason}"
    )
}

/// What a refusal of this service carries on the wire: the code in the header and in the
/// document, and the `Message` its own error shapes spell.
fn refusal(status: u16, parsed: &Value, case: &str, expected_code: &str, expected_status: u16) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with {parsed}"
    );
    assert_eq!(
        parsed["__type"].as_str().map(|named| named
            .split('#')
            .next_back()
            .unwrap_or(named)
            .to_string()),
        Some(expected_code.to_string()),
        "case {case:?}: expected the {expected_code} code, got {parsed}"
    );
}

/// RULE: both operations of this data plane are decided against the calling principal's own
/// identity-based policies before anything is read, under the action
/// `appconfig:<OperationName>` and the configuration the call addresses — so a caller no
/// readable statement grants, or one an explicit deny refuses, is answered
/// `AccessDeniedException` with HTTP 403 and IAM's own sentence, while the same call is served
/// once the grant is in force; and a refused session mints nothing.
#[tokio::test]
async fn both_operations_are_decided_against_the_callers_identity_policies() {
    let aws = Emulator::new();
    let deployed = deploy(&aws).await;
    let arn = deployed.arn();
    // The authorized caller every row's poll leg borrows a token from, so a row whose SESSION
    // is refused can still have its poll decided.
    let permitted = assumed_role(
        &aws,
        "appconfig-permitted",
        &[statement("Allow", &["appconfig:*"], "*")],
    )
    .await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let caller = assumed_role(&aws, &format!("appconfig-row-{index}"), &(row.policies)(&arn))
            .await;

        let minted_before = aws.sessions();
        let (status, parsed) = aws
            .start_session(
                &caller.access_key_id,
                &deployed.application,
                &deployed.environment,
                &deployed.profile,
            )
            .await;
        let token = match row.session_refused {
            Some(action) => {
                refusal(status, &parsed, case, ACCESS_DENIED, ACCESS_DENIED_STATUS);
                assert_eq!(
                    parsed["Message"].as_str(),
                    Some(denial(&caller.arn, action, &arn, row.explicit_deny).as_str()),
                    "case {case:?}: expected IAM's own sentence, got {parsed}"
                );
                assert_eq!(
                    aws.sessions(),
                    minted_before,
                    "case {case:?}: a refused session must mint nothing"
                );
                // The poll leg still needs a live token, which only an authorized caller can
                // mint.
                let (status, minted) = aws
                    .start_session(
                        &permitted.access_key_id,
                        &deployed.application,
                        &deployed.environment,
                        &deployed.profile,
                    )
                    .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: the authorized caller must be served, got {status} {minted}"
                );
                minted["InitialConfigurationToken"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a token, got {minted}"))
                    .to_string()
            }
            None => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: an authorized session must be minted, got {status} {parsed}"
                );
                parsed["InitialConfigurationToken"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a token, got {parsed}"))
                    .to_string()
            }
        };

        let (status, body, parsed) = aws.poll(&caller.access_key_id, &token).await;
        match row.poll_refused {
            Some(action) => {
                refusal(status, &parsed, case, ACCESS_DENIED, ACCESS_DENIED_STATUS);
                assert_eq!(
                    parsed["Message"].as_str(),
                    Some(denial(&caller.arn, action, &arn, row.explicit_deny).as_str()),
                    "case {case:?}: expected IAM's own sentence for the poll, got {parsed}"
                );
                // The refusal consumed nothing: the same token still polls for a caller that
                // may.
                let (status, served, _parsed) = aws.poll(&permitted.access_key_id, &token).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a refused poll must consume no token, got {status}"
                );
                assert_eq!(
                    served, DEPLOYED,
                    "case {case:?}: expected the deployed document, got {served:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an authorized poll must be served, got {status} {parsed}"
                );
                assert_eq!(
                    body, DEPLOYED,
                    "case {case:?}: expected the deployed document, got {body:?}"
                );
            }
        }
    }
}

/// RULE, the other half of the poll's resource: a poll's own input names only its token, so a
/// token that resolves to no session addresses no configuration — the decision is taken on the
/// action alone, against every resource. An unauthorized caller is therefore refused
/// `AccessDeniedException` whether or not its token exists, which is what keeps the refusal
/// from telling it which, while an authorized caller gets the modelled token verdict.
#[tokio::test]
async fn a_poll_whose_token_resolves_to_nothing_is_decided_on_the_action_alone() {
    let aws = Emulator::new();
    let deployed = deploy(&aws).await;
    let permitted = assumed_role(
        &aws,
        "appconfig-token-permitted",
        &[statement("Allow", &["appconfig:*"], "*")],
    )
    .await;
    let denied = assumed_role(&aws, "appconfig-token-denied", &[]).await;
    let (status, minted) = aws
        .start_session(
            &permitted.access_key_id,
            &deployed.application,
            &deployed.environment,
            &deployed.profile,
        )
        .await;
    assert_eq!(
        status, 201,
        "setup: the authorized caller must be served, got {status} {minted}"
    );
    let live = minted["InitialConfigurationToken"]
        .as_str()
        .expect("setup: a minted token")
        .to_string();

    // (case, the token presented, the resource the refusal must name)
    let rows: &[(&str, String, String)] = &[
        ("a token this service minted", live, deployed.arn()),
        (
            "a token no session answers to",
            "0".repeat(64),
            "*".to_string(),
        ),
    ];
    for (case, token, resource) in rows {
        let (status, _body, parsed) = aws.poll(&denied.access_key_id, token).await;
        refusal(status, &parsed, case, ACCESS_DENIED, ACCESS_DENIED_STATUS);
        assert_eq!(
            parsed["Message"].as_str(),
            Some(denial(&denied.arn, GET_CONFIGURATION, resource, false).as_str()),
            "case {case:?}: expected IAM's own sentence naming {resource}, got {parsed}"
        );
    }
    // The authorized caller reaches the token verdict instead, which is a different claim from
    // the refusal above.
    let (status, _body, parsed) = aws.poll(&permitted.access_key_id, &"0".repeat(64)).await;
    refusal(
        status,
        &parsed,
        "an authorized caller presenting a token no session answers to",
        "BadRequestException",
        400,
    );
}

/// RULE: an identifier that resolves to nothing is the modelled `ResourceNotFoundException`
/// with HTTP 404, and a missing grant is `AccessDeniedException` with HTTP 403 — the two
/// faults an operator must be able to tell apart. The account's own published credential holds
/// no IAM policies, so it is served either way.
#[tokio::test]
async fn an_unresolvable_identifier_and_a_missing_grant_are_different_refusals() {
    let aws = Emulator::new();
    let deployed = deploy(&aws).await;
    let denied = assumed_role(&aws, "appconfig-unresolvable", &[]).await;

    // The account's own credential: no IAM policies, so the triple decides the answer.
    let (status, parsed) = aws
        .start_session(
            ACCOUNT_KEY,
            &deployed.application,
            &deployed.environment,
            &deployed.profile,
        )
        .await;
    assert_eq!(
        status, 201,
        "case \"the account's own credential\": expected a session, got {status} {parsed}"
    );
    let (status, parsed) = aws
        .start_session(
            ACCOUNT_KEY,
            &deployed.application,
            "OrdersEnvironment4e5f6a7b",
            &deployed.profile,
        )
        .await;
    refusal(
        status,
        &parsed,
        "an identifier that resolves to nothing",
        NOT_FOUND,
        NOT_FOUND_STATUS,
    );

    // The same unresolvable triple, for a caller holding no grant: the authorization refusal
    // comes first and says nothing about the identifiers.
    let (status, parsed) = aws
        .start_session(
            &denied.access_key_id,
            &deployed.application,
            "OrdersEnvironment4e5f6a7b",
            &deployed.profile,
        )
        .await;
    let case = "an unresolvable triple for a caller holding no grant";
    refusal(status, &parsed, case, ACCESS_DENIED, ACCESS_DENIED_STATUS);
    assert!(
        parsed["ResourceType"].is_null() && parsed["ReferencedBy"].is_null(),
        "case {case:?}: an authorization refusal must report no resource, got {parsed}"
    );
}
