// SPDX-License-Identifier: CC0-1.0

//! An HTTP api route whose integration is a FIRST-CLASS APPCONFIG ACTION — `AWS_PROXY` naming
//! the `AppConfig-GetConfiguration` `IntegrationSubtype` — performed end to end: this service,
//! AppConfig, IAM and the token service on ONE engine, with the configuration deployed through
//! AppConfig's own operations rather than planted.
//!
//! The RULE every table below is one slice of: **an HTTP api route whose integration names the
//! `AppConfig-GetConfiguration` subtype performs that action on every matching request, with
//! each of the action's parameters taken from the integration's `RequestParameters` — one
//! mapping expression over the request, the invocation's context or the stage's variables, or a
//! static value — issued as a session of the role its `CredentialsArn` names; and the caller is
//! answered that action's own answer in the form AppConfig's api states one, which for this
//! operation binds the result to the response's own payload: `200` whose whole body is the
//! deployed configuration's bytes, whose `Content-Type` is the `ContentType` AppConfig stated
//! for them and whose `Configuration-Version` is the version they belong to — or the refusal
//! AppConfig itself stated, relayed.**
//!
//! Nothing asserted here is a value this emulator fills in: every assertion is either on what
//! AppConfig answered the gateway or on what AppConfig answers the SAME call made directly,
//! which is why it is mounted beside this one.
//!
//! The steps BEFORE the integration (the claim, stage resolution, the deployment gate, route
//! matching, the authorization gate and the payload quota) are owned by
//! `http_api_servable_stage.rs` and its siblings and are not restated here, and so are the
//! mapping-expression catalog and the role-session rules, owned by
//! `http_api_service_action_integration.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_form::form_encode;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The inline policy name this suite writes a role's grant under.
const INLINE_POLICY: &str = "inline";

/// The role every integration below is performed under, and the service principal it trusts,
/// which is the one API Gateway assumes a `CredentialsArn` role as.
const INTEGRATION_ROLE: &str = "appconfig-reader";
const GATEWAY_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The IAM action a statement on that role names for this subtype
/// (`<arnNamespace>:<OperationName>`).
const GET_CONFIGURATION_ACTION: &str = "appconfig:GetConfiguration";

/// The `IntegrationSubtype` under test, exactly as `Integration$IntegrationSubtype` spells the
/// action it names.
const GET_CONFIGURATION: &str = "AppConfig-GetConfiguration";

/// The `LocationUri` of a profile whose versions AppConfig itself hosts.
const HOSTED: &str = "hosted";

/// The status a performed action's answer is carried under, and the status AppConfig answers
/// every refusal of this operation with (`smithy_query(query="operation",
/// operation="GetConfiguration")` lists `BadRequestException` at HTTP 400).
const PERFORMED: u16 = 200;
const CLIENT_REFUSAL: u16 = 400;

/// The error shape AppConfig names for every refusal of this read.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The members a relayed refusal names the error SHAPE under, and the one it states the
/// service's own detail text under.
const SHAPE_MEMBER: &str = "__type";
const DETAIL_MEMBER: &str = "Message";

/// The status and sentence an HTTP api answers a request that does not carry what its route's
/// integration maps onto a parameter the action requires.
const BAD_REQUEST: u16 = 400;
const BAD_REQUEST_MESSAGE: &str = "Bad Request";

/// The header AppConfig's own api binds `Configuration$ConfigurationVersion` onto.
const CONFIGURATION_VERSION_HEADER: &str = "configuration-version";

/// The path an api's endpoint is published under on the address this emulator serves that
/// plane at: `/_aws/execute-api/{api-id}`, the api's own data-plane path.
const ENDPOINT_PATH_MARKER: &str = "/_aws/execute-api";


/// The state AppConfig reports for a deployment that has finished rolling out, and the longest
/// a poll waits for one to reach it.
///
/// Generous on purpose: what is being waited for is the CONDITION (the deployment's own
/// `State`), re-read until it holds, so a slow or loaded host only takes more turns of the loop.
const COMPLETE: &str = "COMPLETE";
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long one turn of that poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// This service, AppConfig, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the action an integration performs is dispatched back
/// into this same router, so the configuration it reads is the one this suite deployed.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one case deploys: the environment the configuration goes into, the hosted profile that
/// holds it, and its bytes with the media type AppConfig is told describes them.
struct Hosting<'h> {
    environment: &'h str,
    profile: &'h str,
    content: &'h [u8],
    content_type: &'h str,
}

/// One deployed configuration, as the triple a route's mapping addresses it by and the version
/// and bytes AppConfig holds for it.
#[derive(Debug, Clone)]
struct Deployed {
    application_id: String,
    application_name: String,
    environment_id: String,
    environment_name: String,
    profile_id: String,
    profile_name: String,
    /// The hosted version number the deployment put in force, as `Configuration-Version`
    /// spells it.
    version: String,
    /// The deployment this suite waits for, under the environment it was started in.
    deployment_number: i64,
}

/// One answer the deployed stage gave: the parts of it a client of this route reads.
struct Answered {
    status: u16,
    content_type: String,
    configuration_version: String,
    body: Vec<u8>,
}

impl Answered {
    /// One line about this answer for an assertion that failed.
    fn summary(&self) -> String {
        format!(
            "{} content-type={:?} configuration-version={:?} body={:?}",
            self.status,
            self.content_type,
            self.configuration_version,
            String::from_utf8_lossy(&self.body)
        )
    }

    /// This answer's body as the JSON document a relayed refusal is stated in.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&self.body).to_string()))
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        // The INTEGRATION's own call is signed by this emulator, with the session it minted.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(
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
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
    /// the request. The signature is never checked here.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{service}/\
             aws4_request, SignedHeaders=host, Signature=not-verified"
        )
    }

    /// One AppConfig call (`restJson1` under the service's own endpoint), as its status, the
    /// headers it answered and its body as the bytes it is.
    async fn appconfig(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let host = format!("appconfig.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("authorization", Self::authorization("appconfig"));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        (status, answered, response.into_body().to_vec())
    }

    /// One AppConfig call carrying a JSON document, which must be served; answers the document
    /// it answered.
    async fn appconfig_ok(&self, case: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
        let payload = body
            .as_ref()
            .map(|value| value.to_string().into_bytes())
            .unwrap_or_default();
        let (status, _headers, answered) = self
            .appconfig(method, uri, &[("content-type", "application/json")], payload)
            .await;
        let document = parsed(&answered);
        assert!(
            (200..300).contains(&status),
            "case {case:?}: expected AppConfig {method} {uri} to succeed, got {status} {document}"
        );
        document
    }

    /// One awsQuery call to IAM, as the account's own credential, which must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
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
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: IAM {action} must succeed, got {status} {answered}"
        );
    }

    /// One control-plane call against this service (`restJson1` under `/v2`), as its status and
    /// parsed body.
    async fn control(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(value.to_string()),
            None => bytes::Bytes::new(),
        };
        let host = format!("apigateway.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("apigateway"))
            .body(payload)
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(&response.into_body()))
    }

    /// The role every integration below is performed under: trusting API Gateway, carrying the
    /// grant for the action its subtype names, written with IAM's own operations. Answers its
    /// ARN.
    async fn role(&self, case: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": GATEWAY_PRINCIPAL},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", INTEGRATION_ROLE.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": GET_CONFIGURATION_ACTION,
                "Resource": "*",
            }],
        })
        .to_string();
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", INTEGRATION_ROLE.to_string()),
                ("PolicyName", INLINE_POLICY.to_string()),
                ("PolicyDocument", document),
            ],
        )
        .await;
        format!(
            "arn:aws:iam::{}:role/{INTEGRATION_ROLE}",
            engine::emulator_account_id()
        )
    }

    /// An AppConfig application, created through its own operation; answers its `(id, name)`.
    async fn application(&self, case: &str, name: &str) -> (String, String) {
        let created = self
            .appconfig_ok(case, "POST", "/applications", Some(json!({ "Name": name })))
            .await;
        (text(case, &created, "Id"), name.to_string())
    }

    /// The deployment strategy every deployment below uses: one growth interval reaching every
    /// target, no bake time, replicating nowhere.
    async fn strategy(&self, case: &str, name: &str) -> String {
        let created = self
            .appconfig_ok(
                case,
                "POST",
                "/deploymentstrategies",
                Some(json!({
                    "Name": name,
                    "DeploymentDurationInMinutes": 0,
                    "GrowthFactor": 100,
                    "FinalBakeTimeInMinutes": 0,
                    "ReplicateTo": "NONE",
                })),
            )
            .await;
        text(case, &created, "Id")
    }

    /// Deploy `hosting.content` into a fresh environment of `application`, under a hosted
    /// configuration profile of its own: the environment, the profile, the hosted version
    /// carrying those bytes and the deployment that puts it in force, each through AppConfig's
    /// own operation.
    ///
    /// The deployment is only STARTED here; [`Emulator::settled`] waits for it, so several
    /// deployments can be started together and waited for once.
    async fn deploy(
        &self,
        case: &str,
        application: &(String, String),
        strategy: &str,
        hosting: Hosting<'_>,
    ) -> Deployed {
        let Hosting {
            environment: environment_name,
            profile: profile_name,
            content,
            content_type,
        } = hosting;
        let (application_id, application_name) = application;
        let environment = self
            .appconfig_ok(
                case,
                "POST",
                &format!("/applications/{application_id}/environments"),
                Some(json!({ "Name": environment_name })),
            )
            .await;
        let environment_id = text(case, &environment, "Id");
        let profile = self
            .appconfig_ok(
                case,
                "POST",
                &format!("/applications/{application_id}/configurationprofiles"),
                Some(json!({ "Name": profile_name, "LocationUri": HOSTED })),
            )
            .await;
        let profile_id = text(case, &profile, "Id");
        // `Content` is this operation's `@httpPayload` and `ContentType` its `Content-Type`
        // header, so the bytes travel as the body a client posts and the media type as the
        // header describing them.
        let (status, headers, answered) = self
            .appconfig(
                "POST",
                &format!(
                    "/applications/{application_id}/configurationprofiles/{profile_id}\
                     /hostedconfigurationversions"
                ),
                &[("content-type", content_type)],
                content.to_vec(),
            )
            .await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}: CreateHostedConfigurationVersion answered {status} {:?}",
            String::from_utf8_lossy(&answered)
        );
        let version = header(&headers, "version-number");
        assert!(
            !version.is_empty(),
            "case {case:?}: expected a Version-Number header, got {headers:?}"
        );
        let deployment = self
            .appconfig_ok(
                case,
                "POST",
                &format!(
                    "/applications/{application_id}/environments/{environment_id}/deployments"
                ),
                Some(json!({
                    "DeploymentStrategyId": strategy,
                    "ConfigurationProfileId": profile_id,
                    "ConfigurationVersion": version,
                })),
            )
            .await;
        let deployment_number = deployment["DeploymentNumber"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: no DeploymentNumber in {deployment}"));
        Deployed {
            application_id: application_id.clone(),
            application_name: application_name.clone(),
            environment_id,
            environment_name: environment_name.to_string(),
            profile_id,
            profile_name: profile_name.to_string(),
            version,
            deployment_number,
        }
    }

    /// Wait for `deployed`'s deployment to finish rolling out, read back through AppConfig's
    /// own `GetDeployment`.
    ///
    /// The wait is for the CONDITION: the record is re-read until its `State` is the terminal
    /// `COMPLETE`, or until a deadline far beyond any span this emulator owes the rollout has
    /// passed.
    async fn settled(&self, case: &str, deployed: &Deployed) {
        let uri = format!(
            "/applications/{}/environments/{}/deployments/{}",
            deployed.application_id, deployed.environment_id, deployed.deployment_number
        );
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let read = self.appconfig_ok(case, "GET", &uri, None).await;
            if read["State"] == json!(COMPLETE) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the deployment to reach {COMPLETE}, got {read}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// An api carrying one integration per `(route key, integration)` pair and the `$default`
    /// stage with auto-deploy on; answers the host its `ApiEndpoint` names and its `ApiId`.
    async fn api(&self, case: &str, routes: &[(&str, Value)]) -> (String, String) {
        let (status, api) = self
            .control(
                "POST",
                "/v2/apis",
                Some(json!({ "name": "configuration-api", "protocolType": "HTTP" })),
            )
            .await;
        assert_eq!(status, 201, "case {case:?}: CreateApi answered {status} {api}");
        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
        let host = api["apiEndpoint"]
            .as_str()
            .unwrap_or_default()
            .split_once("://")
            .map(|(_scheme, rest)| rest.to_string())
            .unwrap_or_default();
        for (route_key, integration) in routes {
            let (status, created) = self
                .control(
                    "POST",
                    &format!("/v2/apis/{api_id}/integrations"),
                    Some(integration.clone()),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateIntegration answered {status} {created}"
            );
            let integration_id = created["integrationId"].as_str().unwrap_or_default();
            // The subtype must read back: it is what decides which action the route performs.
            assert_eq!(
                created["integrationSubtype"],
                integration["integrationSubtype"],
                "case {case:?}: expected the subtype to be recorded, got {created}"
            );
            let (status, route) = self
                .control(
                    "POST",
                    &format!("/v2/apis/{api_id}/routes"),
                    Some(json!({
                        "routeKey": route_key,
                        "target": format!("integrations/{integration_id}"),
                        "authorizationType": "AWS_IAM",
                    })),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateRoute answered {status} {route}"
            );
        }
        let (status, stage) = self
            .control(
                "POST",
                &format!("/v2/apis/{api_id}/stages"),
                Some(json!({ "stageName": "$default", "autoDeploy": true })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage answered {status} {stage}"
        );
        (host, api_id)
    }

    /// One client request to a route of the deployed stage: the invoke the integration is
    /// performed for, signed for the data plane's own signing name, because every route below
    /// declares `AWS_IAM`.
    async fn invoke(
        &self,
        method: &str,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Answered {
        let signed = Self::authorization("execute-api");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", signed);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let read = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let content_type = read("content-type");
        let configuration_version = read(CONFIGURATION_VERSION_HEADER);
        Answered {
            status,
            content_type,
            configuration_version,
            body: response.into_body().to_vec(),
        }
    }

    /// The same read made DIRECTLY against AppConfig, as a client holding the triple would
    /// make it: the independent answer a relayed one is compared with.
    async fn read_directly(
        &self,
        application: &str,
        environment: &str,
        configuration: &str,
        client_id: &str,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        self.appconfig(
            "GET",
            &format!(
                "/applications/{application}/environments/{environment}/configurations/\
                 {configuration}?client_id={client_id}"
            ),
            &[],
            Vec::new(),
        )
        .await
    }
}

/// One response body as the JSON it is, or the text it is for a body that is not JSON.
fn parsed(body: &[u8]) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).to_string()))
}

/// The string member `member` of `document`, which must be there.
fn text(case: &str, document: &Value, member: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member:?} in {document}"))
        .to_string()
}

/// The value of the header `name` among `headers`, or the empty string for one not answered.
fn header(headers: &[(String, String)], name: &str) -> String {
    headers
        .iter()
        .find(|(held, _)| held == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// The detail text a refusal states, under either spelling a service may state it in.
fn detail(document: &Value) -> String {
    for member in [DETAIL_MEMBER, "message"] {
        if let Some(text) = document.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

/// The integration a route below carries: the first-class subtype, performed under
/// `credentials_arn`, with `request_parameters` mapping the action's parameters.
fn subtype_integration(credentials_arn: &str, request_parameters: Value) -> Value {
    json!({
        "integrationType": "AWS_PROXY",
        "integrationSubtype": GET_CONFIGURATION,
        "payloadFormatVersion": "1.0",
        "credentialsArn": credentials_arn,
        "requestParameters": request_parameters,
    })
}

/// RULE: a route whose integration names `AppConfig-GetConfiguration` reads the configuration
/// in force for the triple its `RequestParameters` map, for the client the mapped `ClientId`
/// names, and answers it as AppConfig's own api states that answer — the deployed bytes as the
/// whole body, under the deployed `ContentType`, with the deployed version in
/// `Configuration-Version`.
///
/// The rows differ along what the rule generalises over: the CONTENT and the media type
/// describing it (a JSON document, a plain-text document, a YAML document, bytes that are not
/// text at all), how the triple is ADDRESSED (by the ids AppConfig minted and by the names the
/// caller chose), and WHERE each mapped parameter comes from (a static value, a query-string
/// parameter, a header, a member of the request body, a path variable and a context variable).
#[tokio::test]
async fn a_get_configuration_subtype_route_answers_the_deployed_configuration() {
    /// One case: the configuration deployed, the route, the request made to it, and the
    /// parameters the integration maps out of that request.
    struct Row {
        case: &'static str,
        environment: &'static str,
        profile: &'static str,
        content: &'static [u8],
        content_type: &'static str,
        route_key: &'static str,
        method: &'static str,
        path: &'static str,
        headers: &'static [(&'static str, &'static str)],
        body: &'static str,
        parameters: fn(&Deployed) -> Value,
    }
    let rows: &[Row] = &[
        Row {
            case: "a JSON document, the triple mapped as the ids AppConfig minted",
            environment: "production",
            profile: "features",
            content: br#"{"marker":"first","limits":{"retries":3}}"#,
            content_type: "application/json",
            route_key: "GET /config",
            method: "GET",
            path: "/config",
            headers: &[],
            body: "",
            parameters: |deployed| {
                json!({
                    "Application": deployed.application_id,
                    "Environment": deployed.environment_id,
                    "Configuration": deployed.profile_id,
                    "ClientId": "a-device",
                })
            },
        },
        Row {
            case: "a plain-text document, the triple mapped as the names the caller chose and \
                   the client id taken from the query string",
            environment: "staging",
            profile: "banner",
            content: b"maintenance = false\nbanner = none\n",
            content_type: "text/plain",
            route_key: "GET /banner",
            method: "GET",
            path: "/banner?client=browser-7",
            headers: &[],
            body: "",
            parameters: |deployed| {
                json!({
                    "Application": deployed.application_name,
                    "Environment": deployed.environment_name,
                    "Configuration": deployed.profile_name,
                    "ClientId": "$request.querystring.client",
                })
            },
        },
        Row {
            case: "a YAML document, the triple mapped out of a header, a body member and a \
                   path variable",
            environment: "canary",
            profile: "rollout",
            content: b"rollout:\n  enabled: true\n  share: 10\n",
            content_type: "application/x-yaml",
            route_key: "POST /config/{profile}",
            method: "POST",
            path: "/config/rollout",
            headers: &[("x-application", "mapped-from-a-header")],
            body: r#"{"environment":"canary"}"#,
            parameters: |_deployed| {
                // The header carries the application's id; the body names the environment and
                // the path variable the profile, each by the name the caller chose.
                json!({
                    "Application": "$request.header.x-application",
                    "Environment": "$request.body.environment",
                    "Configuration": "$request.path.profile",
                    "ClientId": "$context.httpMethod",
                })
            },
        },
        Row {
            case: "bytes that are not text, under the media type stated for them",
            environment: "devices",
            profile: "firmware",
            content: &[0x00, 0x01, 0xfe, 0xff, 0x10, 0x00, 0x7f],
            content_type: "application/octet-stream",
            route_key: "GET /firmware",
            method: "GET",
            path: "/firmware",
            headers: &[],
            body: "",
            parameters: |deployed| {
                json!({
                    "Application": deployed.application_id,
                    "Environment": deployed.environment_id,
                    "Configuration": deployed.profile_id,
                    "ClientId": "sensor-1",
                })
            },
        },
    ];

    let emulator = Emulator::new();
    let case = "the configurations under test";
    let role = emulator.role(case).await;
    let application = emulator.application(case, "configuration-fleet").await;
    let strategy = emulator.strategy(case, "all-at-once").await;
    // Every row's deployment is STARTED first and waited for afterwards: they are deployments
    // of different environments, so AppConfig rolls them out side by side.
    let mut deployed = Vec::new();
    for row in rows {
        deployed.push(
            emulator
                .deploy(
                    row.case,
                    &application,
                    &strategy,
                    Hosting {
                        environment: row.environment,
                        profile: row.profile,
                        content: row.content,
                        content_type: row.content_type,
                    },
                )
                .await,
        );
    }
    for (row, deployed) in rows.iter().zip(&deployed) {
        emulator.settled(row.case, deployed).await;
    }
    // The header row maps the application by the id AppConfig minted, which the request has to
    // carry: the row's own header value is replaced by it here.
    let mut headers_per_row: Vec<Vec<(String, String)>> = Vec::new();
    for (row, deployed) in rows.iter().zip(&deployed) {
        headers_per_row.push(
            row.headers
                .iter()
                .map(|(name, value)| {
                    let carried = if *name == "x-application" {
                        deployed.application_id.clone()
                    } else {
                        (*value).to_string()
                    };
                    ((*name).to_string(), carried)
                })
                .collect(),
        );
    }
    let routes: Vec<(&str, Value)> = rows
        .iter()
        .zip(&deployed)
        .map(|(row, deployed)| {
            (
                row.route_key,
                subtype_integration(&role, (row.parameters)(deployed)),
            )
        })
        .collect();
    let (host, _api_id) = emulator.api(case, &routes).await;

    for ((row, deployed), headers) in rows.iter().zip(&deployed).zip(&headers_per_row) {
        let case = row.case;
        let carried: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let answered = emulator
            .invoke(row.method, &host, row.path, &carried, row.body)
            .await;
        assert_eq!(
            answered.status,
            PERFORMED,
            "case {case:?}: expected the action's own answer, got {}",
            answered.summary()
        );
        assert_eq!(
            answered.body,
            row.content,
            "case {case:?}: expected the deployed content as the whole body, got {}",
            answered.summary()
        );
        assert_eq!(
            answered.content_type,
            row.content_type,
            "case {case:?}: expected the deployed ContentType, got {}",
            answered.summary()
        );
        assert_eq!(
            answered.configuration_version, deployed.version,
            "case {case:?}: expected the deployed version {:?} in {CONFIGURATION_VERSION_HEADER}, \
             got {}",
            deployed.version,
            answered.summary()
        );
        // And the same bytes AppConfig answers the SAME read made directly, which is what the
        // route exists to put an api in front of.
        let (status, _headers, directly) = emulator
            .read_directly(
                &deployed.application_id,
                &deployed.environment_id,
                &deployed.profile_id,
                "a-device",
            )
            .await;
        assert_eq!(
            (status, directly.as_slice()),
            (PERFORMED, row.content),
            "case {case:?}: expected the route's answer to be the one AppConfig gives the same \
             read, got {status} {:?}",
            String::from_utf8_lossy(&directly)
        );
    }
}

/// RULE: `ClientConfigurationVersion` is the one parameter of this action a mapping may leave
/// out, and the value a route maps for it travels to the action — so a client that says it
/// already holds the version in force is answered the empty content AppConfig answers it, and
/// one that says it holds another version, or says nothing, is answered the content.
#[tokio::test]
async fn the_client_configuration_version_a_route_maps_travels_to_the_action() {
    const CONTENT: &[u8] = br#"{"marker":"held"}"#;
    const CONTENT_TYPE: &str = "application/json";

    let case = "the version a client says it holds";
    let emulator = Emulator::new();
    let role = emulator.role(case).await;
    let application = emulator.application(case, "version-aware-fleet").await;
    let strategy = emulator.strategy(case, "all-at-once").await;
    let deployed = emulator
        .deploy(
            case,
            &application,
            &strategy,
            Hosting {
                environment: "production",
                profile: "features",
                content: CONTENT,
                content_type: CONTENT_TYPE,
            },
        )
        .await;
    emulator.settled(case, &deployed).await;
    let (host, _api_id) = emulator
        .api(
            case,
            &[(
                "GET /config",
                subtype_integration(
                    &role,
                    json!({
                        "Application": deployed.application_id,
                        "Environment": deployed.environment_id,
                        "Configuration": deployed.profile_id,
                        "ClientId": "a-device",
                        "ClientConfigurationVersion": "$request.querystring.held",
                    }),
                ),
            )],
        )
        .await;

    // Rows differ along WHICH version the request says the client holds: the one in force,
    // another one, and none at all (the mapping then resolves nothing and the parameter does
    // not travel).
    let in_force = deployed.version.clone();
    let rows: Vec<(&str, String, &[u8])> = vec![
        (
            "the version in force",
            format!("/config?held={in_force}"),
            b"",
        ),
        (
            "another version",
            format!("/config?held={}", in_force.parse::<i64>().unwrap_or(1) + 7),
            CONTENT,
        ),
        ("no version at all", "/config".to_string(), CONTENT),
    ];
    for (case, path, expected) in rows {
        let answered = emulator.invoke("GET", &host, &path, &[], "").await;
        assert_eq!(
            answered.status,
            PERFORMED,
            "case {case:?}: expected the action's own answer, got {}",
            answered.summary()
        );
        assert_eq!(
            answered.body,
            expected,
            "case {case:?}: expected the body {:?}, got {}",
            String::from_utf8_lossy(expected),
            answered.summary()
        );
        assert_eq!(
            answered.configuration_version, in_force,
            "case {case:?}: expected the version in force in {CONFIGURATION_VERSION_HEADER}, got {}",
            answered.summary()
        );
    }
}

/// RULE: a refusal the service serving the action stated is RELAYED to the caller — that
/// service's own status, the error shape it named and the text it stated — and never the
/// gateway's own generic text, so a client reads the same refusal it would reading the
/// configuration itself.
///
/// The rows differ along WHICH level of the triple the mapping addresses wrongly: the
/// application, the configuration profile and the environment — which AppConfig refuses
/// separately — each compared with the answer AppConfig gives the SAME read made directly.
#[tokio::test]
async fn a_refusal_appconfig_stated_is_relayed_to_the_caller() {
    const CONTENT: &[u8] = br#"{"marker":"deployed"}"#;

    let case = "the refusals AppConfig states";
    let emulator = Emulator::new();
    let role = emulator.role(case).await;
    let application = emulator.application(case, "refusing-fleet").await;
    let strategy = emulator.strategy(case, "all-at-once").await;
    let deployed = emulator
        .deploy(
            case,
            &application,
            &strategy,
            Hosting {
                environment: "production",
                profile: "features",
                content: CONTENT,
                content_type: "application/json",
            },
        )
        .await;
    emulator.settled(case, &deployed).await;

    /// One case: the triple the route's mapping addresses, and the route it is behind.
    struct Row {
        case: &'static str,
        route_key: &'static str,
        path: &'static str,
        triple: fn(&Deployed) -> (String, String, String),
    }
    let rows: &[Row] = &[
        Row {
            case: "an application nothing resolves",
            route_key: "GET /no-application",
            path: "/no-application",
            triple: |deployed| {
                (
                    "nothing-holds-this-application".to_string(),
                    deployed.environment_id.clone(),
                    deployed.profile_id.clone(),
                )
            },
        },
        Row {
            case: "a configuration profile the application does not hold",
            route_key: "GET /no-profile",
            path: "/no-profile",
            triple: |deployed| {
                (
                    deployed.application_id.clone(),
                    deployed.environment_id.clone(),
                    "nothing-holds-this-profile".to_string(),
                )
            },
        },
        Row {
            case: "an environment nothing resolves",
            route_key: "GET /no-environment",
            path: "/no-environment",
            triple: |deployed| {
                (
                    deployed.application_id.clone(),
                    "nothing-holds-this-environment".to_string(),
                    deployed.profile_id.clone(),
                )
            },
        },
    ];
    let client_id = "a-device";
    let routes: Vec<(&str, Value)> = rows
        .iter()
        .map(|row| {
            let (application, environment, configuration) = (row.triple)(&deployed);
            (
                row.route_key,
                subtype_integration(
                    &role,
                    json!({
                        "Application": application,
                        "Environment": environment,
                        "Configuration": configuration,
                        "ClientId": client_id,
                    }),
                ),
            )
        })
        .collect();
    let (host, _api_id) = emulator.api(case, &routes).await;

    for row in rows {
        let case = row.case;
        let (application, environment, configuration) = (row.triple)(&deployed);
        let (direct_status, _headers, direct_body) = emulator
            .read_directly(&application, &environment, &configuration, client_id)
            .await;
        let direct = parsed(&direct_body);
        assert_eq!(
            direct_status, CLIENT_REFUSAL,
            "case {case:?}: expected AppConfig itself to refuse the read, got {direct_status} \
             {direct}"
        );
        let answered = emulator.invoke("GET", &host, row.path, &[], "").await;
        let relayed = answered.document();
        assert_eq!(
            answered.status,
            direct_status,
            "case {case:?}: expected the status AppConfig stated, got {}",
            answered.summary()
        );
        assert_eq!(
            relayed[SHAPE_MEMBER],
            json!(BAD_REQUEST_EXCEPTION),
            "case {case:?}: expected the error shape AppConfig named, got {}",
            answered.summary()
        );
        assert_eq!(
            detail(&relayed),
            detail(&direct),
            "case {case:?}: expected the text AppConfig stated, got {}",
            answered.summary()
        );
    }
}

/// RULE: a request that carries no value for the expression its route's integration maps onto a
/// parameter the action REQUIRES is the request's own fault — `400` `{"message":"Bad Request"}`,
/// with no action performed — and that is a different class from the configuration being
/// unreadable.
///
/// The rows differ along WHICH required parameter the mapping selects nothing for: each of the
/// four in turn, from the catalogue's own set, mapped out of a header the request leaves out.
#[tokio::test]
async fn a_request_carrying_no_mapped_required_parameter_is_refused_as_a_bad_request() {
    /// The parameters this action requires, as the SUBTYPE catalogue states them — read from
    /// the owner of that set rather than restated here.
    const REQUIRED: &[&str] =
        service_com_amazonaws_apigatewayv2_apigatewayv2::state::integration_subtype
            ::APP_CONFIG_GET_CONFIGURATION
            .required_parameters;
    const CONTENT: &[u8] = br#"{"marker":"unreached"}"#;

    let case = "a request missing a required parameter";
    let emulator = Emulator::new();
    let role = emulator.role(case).await;
    let application = emulator.application(case, "strict-fleet").await;
    let strategy = emulator.strategy(case, "all-at-once").await;
    let deployed = emulator
        .deploy(
            case,
            &application,
            &strategy,
            Hosting {
                environment: "production",
                profile: "features",
                content: CONTENT,
                content_type: "application/json",
            },
        )
        .await;
    emulator.settled(case, &deployed).await;

    // One route per required parameter: every parameter but that one is a static value, and
    // that one is mapped out of a header no request below carries.
    let complete = |deployed: &Deployed| {
        vec![
            ("Application", deployed.application_id.clone()),
            ("Environment", deployed.environment_id.clone()),
            ("Configuration", deployed.profile_id.clone()),
            ("ClientId", "a-device".to_string()),
        ]
    };
    let route_key = |parameter: &str| format!("GET /without-{}", parameter.to_ascii_lowercase());
    let keys: Vec<String> = REQUIRED.iter().map(|parameter| route_key(parameter)).collect();
    let routes: Vec<(&str, Value)> = REQUIRED
        .iter()
        .zip(&keys)
        .map(|(unmapped, key)| {
            let mapping: serde_json::Map<String, Value> = complete(&deployed)
                .into_iter()
                .map(|(parameter, value)| {
                    let mapped = if parameter == *unmapped {
                        "$request.header.x-absent".to_string()
                    } else {
                        value
                    };
                    (parameter.to_string(), Value::String(mapped))
                })
                .collect();
            (
                key.as_str(),
                subtype_integration(&role, Value::Object(mapping)),
            )
        })
        .collect();
    let (host, _api_id) = emulator.api(case, &routes).await;

    for (unmapped, key) in REQUIRED.iter().zip(&keys) {
        let case = format!("a request carrying nothing for {unmapped}");
        let path = key.trim_start_matches("GET ");
        let answered = emulator.invoke("GET", &host, path, &[], "").await;
        let document = answered.document();
        assert_eq!(
            (answered.status, document["message"].clone()),
            (BAD_REQUEST, json!(BAD_REQUEST_MESSAGE)),
            "case {case:?}: expected {BAD_REQUEST} {BAD_REQUEST_MESSAGE:?}, got {}",
            answered.summary()
        );
    }

    // The control: the SAME api, reached on a route whose mapping the request does satisfy,
    // serves the configuration — so the refusals above are about the missing value and not
    // about the api.
    let (served, _control_api_id) = emulator
        .api(
            "the control",
            &[(
                "GET /config",
                subtype_integration(
                    &role,
                    json!({
                        "Application": deployed.application_id,
                        "Environment": deployed.environment_id,
                        "Configuration": deployed.profile_id,
                        "ClientId": "a-device",
                    }),
                ),
            )],
        )
        .await;
    let answered = emulator.invoke("GET", &served, "/config", &[], "").await;
    assert_eq!(
        (answered.status, answered.body.as_slice()),
        (PERFORMED, CONTENT),
        "case \"the control\": expected the deployed configuration, got {}",
        answered.summary()
    );
}

/// One answer read off the wire: its status line, its field lines as HTTP wrote them, and its
/// body as the bytes it is.
struct OnTheWire {
    status: String,
    fields: Vec<String>,
    body: Vec<u8>,
}

impl OnTheWire {
    /// One line about this answer for an assertion that failed.
    fn summary(&self) -> String {
        format!(
            "{:?} {:?} body={:?}",
            self.status,
            self.fields,
            String::from_utf8_lossy(&self.body)
        )
    }
}

/// One request written straight onto the published `authority` as the bytes of an HTTP/1.1
/// message, and the answer read back as the bytes it is.
///
/// Dialled at the LOOPBACK address the core binds every listener of this emulator on, with the
/// port the published authority names — nothing outside this machine is reached. The connection
/// is made IN THE CLEAR, which the published address serves beside the `https` a client dials
/// (`service_common::in_band_tls`), so what this reads is the answer as it goes out — no client
/// library normalises a field name on the way in.
async fn written_on_the_wire(authority: &str, target: &str) -> OnTheWire {
    let port: u16 = authority
        .rsplit_once(':')
        .and_then(|(_host, port)| port.parse().ok())
        .unwrap_or_else(|| panic!("the published authority {authority:?} names a port"));
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| {
            panic!("the published address {authority:?} accepts a connection: {error}")
        });
    let request = format!(
        "GET {target} HTTP/1.1\r\nhost: {authority}\r\nauthorization: {}\r\nconnection: \
         close\r\n\r\n",
        Emulator::authorization("execute-api")
    );
    socket
        .write_all(request.as_bytes())
        .await
        .expect("the request is written");
    let mut raw = Vec::new();
    socket
        .read_to_end(&mut raw)
        .await
        .expect("the answer is read");
    const SEPARATOR: &[u8] = b"\r\n\r\n";
    let at = raw
        .windows(SEPARATOR.len())
        .position(|window| window == SEPARATOR);
    let (head, body) = match at {
        Some(at) => (raw[..at].to_vec(), raw[at + SEPARATOR.len()..].to_vec()),
        None => (raw.clone(), Vec::new()),
    };
    let head = String::from_utf8_lossy(&head).to_string();
    let mut lines = head.lines().map(str::to_string);
    OnTheWire {
        status: lines.next().unwrap_or_default(),
        fields: lines.collect(),
        body,
    }
}

/// Assert `answered` carries `name: value` written EXACTLY so — the canonical spelling of the
/// field name — and carries that field under no other spelling.
///
/// A field name is case-insensitive (RFC 9110 §5.1), so this is not about the field's meaning:
/// it is about the bytes a client that reads a response's field names AS RECEIVED sees, which
/// is the only name such a client can look the value up under.
fn assert_field(case: &str, answered: &OnTheWire, name: &str, value: &str) {
    let carried: Vec<&String> = answered
        .fields
        .iter()
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(field, _value)| field.eq_ignore_ascii_case(name))
        })
        .collect();
    assert_eq!(
        carried,
        vec![&format!("{name}: {value}")],
        "case {case:?}: expected the answer to carry {name:?} as {value:?} under that exact \
         spelling, got {}",
        answered.summary()
    );
}

/// RULE: an api's `ApiEndpoint` is an address a CLIENT DIALS, and every field of an answer
/// read there is written under the canonical HTTP spelling of its name — so a client that
/// reads a response's field names AS RECEIVED finds the configuration's media type under
/// `Content-Type` and the version those bytes belong to under `Configuration-Version`, exactly
/// as it does calling AppConfig itself.
///
/// Driven over a REAL socket against the address the api PUBLISHES, on a router served on a
/// port the way this emulator's own entrypoint serves it: the field line as HTTP wrote it is
/// the only level at which "the field the client sees" is observable at all, so every
/// assertion below is on those bytes.
///
/// The rows differ along what the rule generalises over: the media type the answer carries (a
/// JSON document, plain text, a YAML document, bytes that are not text at all), and WHICH
/// answer it is — the action's own payload, and the gateway's own envelope for a path no route
/// of the api matches.
#[tokio::test]
async fn every_field_of_an_answer_at_the_published_endpoint_carries_its_canonical_name() {
    /// One deployed configuration and the route that serves it.
    struct Row {
        case: &'static str,
        environment: &'static str,
        profile: &'static str,
        content: &'static [u8],
        content_type: &'static str,
        route_key: &'static str,
        path: &'static str,
    }
    let rows: &[Row] = &[
        Row {
            case: "a JSON document",
            environment: "production",
            profile: "features",
            content: br#"{"marker":"on-the-wire"}"#,
            content_type: "application/json",
            route_key: "GET /config",
            path: "/config",
        },
        Row {
            case: "a plain-text document",
            environment: "staging",
            profile: "banner",
            content: b"maintenance = false\n",
            content_type: "text/plain",
            route_key: "GET /banner",
            path: "/banner",
        },
        Row {
            case: "a YAML document",
            environment: "canary",
            profile: "rollout",
            content: b"rollout:\n  enabled: true\n",
            content_type: "application/x-yaml",
            route_key: "GET /rollout",
            path: "/rollout",
        },
        Row {
            case: "bytes that are not text",
            environment: "devices",
            profile: "firmware",
            content: &[0x00, 0x01, 0xfe, 0xff, 0x10, 0x00, 0x7f],
            content_type: "application/octet-stream",
            route_key: "GET /firmware",
            path: "/firmware",
        },
    ];

    let case = "the answers as they go out on the wire";
    let emulator = Emulator::new();
    // SERVED on a port, as the emulator a client dials is: that is what makes this emulator
    // publish addresses at all, and so what gives the api's data plane an address of its own.
    // The port is published as the `https://` URL the core publishes its data port as, which
    // is the form a router records as the base it is served at.
    let (endpoint, _served) =
        emulator_server::serve_on_ephemeral_port(emulator.router.clone()).await;
    let endpoint = endpoint.replacen("http://", "https://", 1);
    emulator_server::EmulatorService::serving_at(&emulator.router, &endpoint);

    let role = emulator.role(case).await;
    let application = emulator.application(case, "wire-fleet").await;
    let strategy = emulator.strategy(case, "all-at-once").await;
    // Every row's deployment is STARTED first and waited for afterwards: they are deployments
    // of different environments, so AppConfig rolls them out side by side.
    let mut deployed = Vec::new();
    for row in rows {
        deployed.push(
            emulator
                .deploy(
                    row.case,
                    &application,
                    &strategy,
                    Hosting {
                        environment: row.environment,
                        profile: row.profile,
                        content: row.content,
                        content_type: row.content_type,
                    },
                )
                .await,
        );
    }
    for (row, deployed) in rows.iter().zip(&deployed) {
        emulator.settled(row.case, deployed).await;
    }
    let routes: Vec<(&str, Value)> = rows
        .iter()
        .zip(&deployed)
        .map(|(row, deployed)| {
            (
                row.route_key,
                subtype_integration(
                    &role,
                    json!({
                        "Application": deployed.application_id,
                        "Environment": deployed.environment_id,
                        "Configuration": deployed.profile_id,
                        "ClientId": "a-device",
                    }),
                ),
            )
        })
        .collect();
    let (announced, api_id) = emulator.api(case, &routes).await;
    let (authority, base_path) = announced
        .split_once('/')
        .map(|(authority, path)| (authority.to_string(), format!("/{path}")))
        .unwrap_or_else(|| (announced.clone(), String::new()));
    // The api is published on an address of its own DATA PLANE rather than on the endpoint the
    // control call reached, and the api it serves is named by the path it was announced under —
    // the addressing every reader of that value places an api by.
    let front_door = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, rest)| rest)
        .trim_end_matches('/')
        .to_string();
    assert_ne!(
        authority, front_door,
        "case {case:?}: expected the api to publish an address of its own data plane, got \
         {announced:?}"
    );
    assert_eq!(
        base_path,
        format!("{ENDPOINT_PATH_MARKER}/{api_id}"),
        "case {case:?}: expected the announced endpoint to name the api in its path, got \
         {announced:?}"
    );

    for (row, deployed) in rows.iter().zip(&deployed) {
        let case = row.case;
        let answered = written_on_the_wire(&authority, &format!("{base_path}{}", row.path)).await;
        assert!(
            answered.status.starts_with("HTTP/1.1 200"),
            "case {case:?}: expected the action's own answer, got {}",
            answered.summary()
        );
        // The three fields that say what the client just received: the media type AppConfig
        // stated for the bytes, the version they belong to, and how many of them there are.
        assert_field(case, &answered, "Content-Type", row.content_type);
        assert_field(case, &answered, "Configuration-Version", &deployed.version);
        assert_field(case, &answered, "Content-Length", &row.content.len().to_string());
        assert_eq!(
            answered.body, row.content,
            "case {case:?}: expected the deployed content as the whole body, got {}",
            answered.summary()
        );
    }

    // And the same holds of the gateway's OWN answer at that address: a path no route of the
    // api matches is the endpoint's `404`, whose JSON envelope is named canonically too.
    let case = "a path no route of the api matches";
    let answered = written_on_the_wire(&authority, &format!("{base_path}/nothing-here")).await;
    assert!(
        answered.status.starts_with("HTTP/1.1 404"),
        "case {case:?}: expected the endpoint's own refusal, got {}",
        answered.summary()
    );
    assert_field(case, &answered, "Content-Type", "application/json");
}
