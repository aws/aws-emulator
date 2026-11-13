// SPDX-License-Identifier: CC0-1.0

//! The AppConfigData configuration-polling loop, end to end over HTTP.
//!
//! Every test drives real requests through a `MultiServiceRouter` that mounts BOTH
//! AppConfigData and the AppConfig control plane over one shared engine, because the
//! data plane has nothing of its own to serve: the applications, environments,
//! configuration profiles, hosted configuration versions and deployments a session
//! resolves against are created here through AppConfig's own operations, exactly as a
//! client would create them, and the poll then reads them back. Nothing is written
//! into either store by hand except where a test has to move the CLOCK on (a token's
//! expiry, a deployment's start), which no operation exposes.
//!
//! The rules under test, none of which is specific to one payload or one identifier:
//!
//! * a session is started only for a triple the AppConfig control plane actually
//!   holds; an unresolvable level is `ResourceNotFoundException` (404) naming the
//!   modelled `ResourceType` and the request member that named it;
//! * a session is over a DEPLOYED configuration, so every session started here is
//!   started over a triple a deployment has already put a configuration into (the
//!   refusal owed to a triple with no deployment is asserted in
//!   `session_requires_a_deployment`);
//! * each identifier is "the ID or the name" of its level, so a session resolves
//!   either spelling;
//! * a poll returns the content of the configuration version the environment's
//!   in-force deployment deployed, with that version's content type and label, only
//!   when it differs from the version the session was last served; otherwise the
//!   payload is empty. Every poll returns a fresh token, usable for its own lifetime,
//!   and a non-zero poll interval;
//! * a token that resolves to no session, or whose lifetime has run out, is refused
//!   with `BadRequestException` (400) whose `Details.InvalidParameters` is keyed by the
//!   offending input MEMBER and names the modelled problem;
//! * an absent `@required` input member is `BadRequestException` (400) whose `Message`
//!   is the validation envelope naming that member and the constraint it failed —
//!   never a 501, and never a 400 with no message.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_appconfig_amazonappconfig::state::ComAmazonawsAppconfigAmazonappconfigState;
use service_com_amazonaws_appconfigdata_appconfigdata::state::{
    ComAmazonawsAppconfigdataAppconfigdataState, DEFAULT_POLL_INTERVAL_IN_SECONDS, MAX_SESSIONS,
};
use service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `appconfig`
/// signing name — the one both AppConfigData and the AppConfig control plane sign
/// with, per each model's `aws.auth#sigv4` trait. Request authentication is switched
/// off below, so only the scope matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/appconfig/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The AppConfigData endpoint host an SDK client addresses.
const ENDPOINT_HOST: &str = "appconfigdata.us-east-1.amazonaws.com";

/// The predefined deployment strategy that reaches every target at once
/// (`state::PREDEFINED_DEPLOYMENT_STRATEGIES`), so a deployment's rollout has begun
/// the instant `StartDeployment` answers.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// The `LocationUri` that selects AppConfig's own hosted configuration store.
const HOSTED: &str = "hosted";

/// One AppConfig hierarchy, as its control plane minted it.
#[derive(Debug, Clone)]
pub(crate) struct Hierarchy {
    pub(crate) application_id: String,
    pub(crate) application_name: String,
    pub(crate) environment_id: String,
    pub(crate) environment_name: String,
    pub(crate) profile_id: String,
    pub(crate) profile_name: String,
}

/// One emulator: the shared engine plus a router with the data plane and the control
/// plane mounted.
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { engine, router }
    }

    /// This service's session store, as the handlers reach it.
    pub(crate) fn state(
        &self,
    ) -> engine::ServiceState<ComAmazonawsAppconfigdataAppconfigdataState> {
        self.engine
            .state::<ComAmazonawsAppconfigdataAppconfigdataState>()
    }

    /// The AppConfig control plane's store, for the one thing no operation exposes:
    /// moving a deployment's start instant into the past.
    pub(crate) fn control_plane(
        &self,
    ) -> engine::ServiceState<ComAmazonawsAppconfigAmazonappconfigState> {
        self.engine
            .state::<ComAmazonawsAppconfigAmazonappconfigState>()
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        content_type: &str,
        extra_headers: &[(&str, &str)],
        body: Bytes,
    ) -> Reply {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", AUTHORIZATION)
            .header("content-type", content_type);
        for (name, value) in extra_headers {
            request = request.header(*name, *value);
        }
        let request = request.body(body).expect("the request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    pub(crate) async fn post_json(&self, path: &str, body: &str) -> Reply {
        self.send(
            "POST",
            path,
            "application/json",
            &[],
            Bytes::copy_from_slice(body.as_bytes()),
        )
        .await
    }

    async fn start_configuration_session(&self, body: &str) -> Reply {
        self.post_json("/configurationsessions", body).await
    }

    /// `GET /configuration` with whatever query string is given — `None` for a request
    /// that carries no `configuration_token` parameter at all.
    pub(crate) async fn get_configuration(&self, query: Option<&str>) -> Reply {
        let path = match query {
            Some(query) => format!("/configuration?{query}"),
            None => "/configuration".to_string(),
        };
        self.send("GET", &path, "application/json", &[], Bytes::new())
            .await
    }

    pub(crate) async fn get_latest_configuration(&self, token: &str) -> Reply {
        self.get_configuration(Some(&format!("configuration_token={token}")))
            .await
    }

    /// Create an application, an environment and a hosted configuration profile
    /// through AppConfig's own operations, and answer the ids and names it minted.
    pub(crate) async fn create_hierarchy(
        &self,
        application: &str,
        environment: &str,
        profile: &str,
    ) -> Hierarchy {
        let created = self
            .post_json("/applications", &format!(r#"{{"Name":"{application}"}}"#))
            .await;
        assert_eq!(
            created.status,
            201,
            "CreateApplication({application}): expected 201, got {} body {}",
            created.status,
            created.text()
        );
        let application_id = created.string("Id");

        let created = self
            .post_json(
                &format!("/applications/{application_id}/environments"),
                &format!(r#"{{"Name":"{environment}"}}"#),
            )
            .await;
        assert_eq!(
            created.status,
            201,
            "CreateEnvironment({environment}): expected 201, got {} body {}",
            created.status,
            created.text()
        );
        let environment_id = created.string("Id");

        let profile_id = self
            .create_typed_profile(&application_id, profile, None)
            .await;

        Hierarchy {
            application_id,
            application_name: application.to_string(),
            environment_id,
            environment_name: environment.to_string(),
            profile_id,
            profile_name: profile.to_string(),
        }
    }

    /// One more environment under an existing application, answered by its id.
    pub(crate) async fn create_environment(
        &self,
        application_id: &str,
        environment: &str,
    ) -> String {
        let created = self
            .post_json(
                &format!("/applications/{application_id}/environments"),
                &format!(r#"{{"Name":"{environment}"}}"#),
            )
            .await;
        assert_eq!(
            created.status,
            201,
            "CreateEnvironment({environment}): expected 201, got {} body {}",
            created.status,
            created.text()
        );
        created.string("Id")
    }

    /// One more hosted configuration profile under an existing application.
    pub(crate) async fn create_profile(&self, application_id: &str, profile: &str) -> String {
        self.create_typed_profile(application_id, profile, None)
            .await
    }

    /// One more hosted configuration profile of a declared `Type` — `None` for a create
    /// that declares none, which AppConfig stores as a freeform profile.
    pub(crate) async fn create_typed_profile(
        &self,
        application_id: &str,
        profile: &str,
        profile_type: Option<&str>,
    ) -> String {
        let declared_type = match profile_type {
            Some(profile_type) => format!(r#","Type":"{profile_type}""#),
            None => String::new(),
        };
        let created = self
            .post_json(
                &format!("/applications/{application_id}/configurationprofiles"),
                &format!(r#"{{"Name":"{profile}","LocationUri":"{HOSTED}"{declared_type}}}"#),
            )
            .await;
        assert_eq!(
            created.status,
            201,
            "CreateConfigurationProfile({profile}): expected 201, got {} body {}",
            created.status,
            created.text()
        );
        created.string("Id")
    }

    /// Write a hosted configuration version and deploy it, as a client sets a
    /// configuration up: the version number AppConfig minted is what the deployment
    /// names.
    pub(crate) async fn deploy(
        &self,
        hierarchy: &Hierarchy,
        profile_id: &str,
        content: &[u8],
        content_type: &str,
        version_label: Option<&str>,
    ) {
        self.deploy_numbered(hierarchy, profile_id, content, content_type, version_label)
            .await;
    }

    /// [`Self::deploy`], answering the deployment number — for a test that has to name
    /// the deployment afterwards.
    pub(crate) async fn deploy_numbered(
        &self,
        hierarchy: &Hierarchy,
        profile_id: &str,
        content: &[u8],
        content_type: &str,
        version_label: Option<&str>,
    ) -> i64 {
        self.start_a_deployment(hierarchy, profile_id, content, content_type, version_label, true)
            .await
    }

    /// [`Self::deploy_numbered`] without letting the stages the clock owes fall due, so
    /// the deployment it starts is one still IN FLIGHT: a rollout is a transition AppConfig
    /// performs after the call that asked for it, which is the state a test about a
    /// deployment in progress needs.
    pub(crate) async fn deploy_in_flight(
        &self,
        hierarchy: &Hierarchy,
        profile_id: &str,
        content: &[u8],
        content_type: &str,
        version_label: Option<&str>,
    ) -> i64 {
        self.start_a_deployment(hierarchy, profile_id, content, content_type, version_label, false)
            .await
    }

    /// Write a hosted configuration version and deploy it, letting the stages the clock
    /// owes the deployment fall due when `settled`.
    async fn start_a_deployment(
        &self,
        hierarchy: &Hierarchy,
        profile_id: &str,
        content: &[u8],
        content_type: &str,
        version_label: Option<&str>,
        settled: bool,
    ) -> i64 {
        let application_id = &hierarchy.application_id;
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(label) = version_label {
            headers.push(("VersionLabel", label));
        }
        let written = self
            .send(
                "POST",
                &format!(
                    "/applications/{application_id}/configurationprofiles/{profile_id}\
                     /hostedconfigurationversions"
                ),
                content_type,
                &headers,
                Bytes::copy_from_slice(content),
            )
            .await;
        assert_eq!(
            written.status,
            201,
            "CreateHostedConfigurationVersion: expected 201, got {} body {}",
            written.status,
            written.text()
        );
        let version = written
            .header("version-number")
            .unwrap_or_else(|| {
                panic!(
                    "no Version-Number header; headers {:?}",
                    written.headers
                )
            })
            .to_string();

        // Only one deployment may be in flight in an environment, so any earlier one
        // is first allowed to finish — see `settle_deployments`.
        self.settle_deployments(hierarchy);
        let environment_id = &hierarchy.environment_id;
        let started = self
            .post_json(
                &format!(
                    "/applications/{application_id}/environments/{environment_id}/deployments"
                ),
                &format!(
                    r#"{{"DeploymentStrategyId":"{ALL_AT_ONCE}",
                          "ConfigurationProfileId":"{profile_id}",
                          "ConfigurationVersion":"{version}"}}"#
                ),
            )
            .await;
        assert_eq!(
            started.status,
            201,
            "StartDeployment(version {version}): expected 201, got {} body {}",
            started.status,
            started.text()
        );
        // Every stage of a deployment after the one the start answers is owed by the
        // CLOCK: the rollout reaching its targets, the bake and the completion. A test
        // that needs the deployed version to be the one in service lets those stages fall
        // due the same way — see `settle_deployments` — rather than sleeping for them.
        if settled {
            self.settle_deployments(hierarchy);
        }
        started.json()["DeploymentNumber"]
            .as_i64()
            .unwrap_or_else(|| panic!("no DeploymentNumber in {}", started.text()))
    }

    /// Stop a deployment as a caller undoes one, with `AllowRevert` so a deployment that
    /// has already completed is reverted rather than refused: afterwards the
    /// configuration it deployed is no longer the one in force for its profile, while the
    /// deployment itself is still a deployment the environment holds.
    pub(crate) async fn stop_deployment(&self, hierarchy: &Hierarchy, number: i64) {
        let application_id = &hierarchy.application_id;
        let environment_id = &hierarchy.environment_id;
        let stopped = self
            .send(
                "DELETE",
                &format!(
                    "/applications/{application_id}/environments/{environment_id}\
                     /deployments/{number}"
                ),
                "application/json",
                &[("Allow-Revert", "true")],
                Bytes::new(),
            )
            .await;
        assert_eq!(
            stopped.status,
            202,
            "StopDeployment({number}): expected 202, got {} body {}",
            stopped.status,
            stopped.text()
        );
    }

    /// Move every deployment in the hierarchy's environment far enough into the past
    /// that its rollout window and final bake time have both elapsed.
    ///
    /// The transition is the CLOCK's — AppConfig completes a deployment once its
    /// strategy's durations have passed, whether or not anybody reads it — so a test
    /// that needs a finished deployment moves the start instant rather than sleeping
    /// for the strategy's bake time.
    pub(crate) fn settle_deployments(&self, hierarchy: &Hierarchy) {
        let state = self.control_plane();
        let mut store = state.lock();
        let Some(environment) = store
            .application_mut(&hierarchy.application_id)
            .and_then(|application| {
                application.environments.get_mut(&hierarchy.environment_id)
            })
        else {
            return;
        };
        for deployment in environment.deployments.values_mut() {
            deployment.started_at -= Duration::from_secs(24 * 60 * 60);
        }
        environment.materialize(SystemTime::now());
    }

    /// Start a session over a triple, named however the caller spells it.
    pub(crate) async fn session_token(
        &self,
        application: &str,
        environment: &str,
        profile: &str,
    ) -> String {
        let reply = self
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"{application}",
                      "EnvironmentIdentifier":"{environment}",
                      "ConfigurationProfileIdentifier":"{profile}"}}"#
            ))
            .await;
        assert_eq!(
            reply.status, 201,
            "StartConfigurationSession({application}/{environment}/{profile}): expected 201, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.initial_token()
    }
}

/// A dispatched response, reduced to what the assertions read.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        Self {
            status,
            headers,
            body: response.into_body(),
        }
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// One top-level string member of a JSON response body.
    pub(crate) fn string(&self, member: &str) -> String {
        self.json()[member]
            .as_str()
            .unwrap_or_else(|| panic!("no {member} in {}", self.text()))
            .to_string()
    }

    fn initial_token(&self) -> String {
        self.string("InitialConfigurationToken")
    }

    pub(crate) fn next_token(&self) -> String {
        self.header("next-poll-configuration-token")
            .unwrap_or_else(|| {
                panic!(
                    "no Next-Poll-Configuration-Token header; headers {:?}",
                    self.headers
                )
            })
            .to_string()
    }

    /// The error code a restJson1 error response carries.
    pub(crate) fn error_code(&self) -> String {
        self.string("__type")
    }

    /// `Details.InvalidParameters.ConfigurationToken.Problem`, as the modelled
    /// `BadRequestException` carries it: the map is keyed by the Smithy input member
    /// name, which is how a client finds the parameter it got wrong.
    fn token_problem(&self) -> String {
        let body = self.json();
        body["Details"]["InvalidParameters"]["ConfigurationToken"]["Problem"]
            .as_str()
            .unwrap_or_else(|| panic!("no token problem in {}", self.text()))
            .to_string()
    }
}

/// The token pattern the model declares for `Token`: `^\S{1,8192}$`.
pub(crate) fn is_modelled_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 8192
        && !token.chars().any(char::is_whitespace)
        && token.chars().all(|c| !c.is_control())
}

/// One deployed-configuration class: a poll must return whatever bytes and content
/// type are deployed, so the rows differ in payload form, size and content type.
struct ContentCase {
    name: &'static str,
    content_type: &'static str,
    first: &'static [u8],
    first_label: Option<&'static str>,
    second: &'static [u8],
    second_label: Option<&'static str>,
}

const CONTENT_CASES: &[ContentCase] = &[
    ContentCase {
        name: "json document",
        content_type: "application/json",
        first: br#"{"feature":"on"}"#,
        first_label: Some("v-one"),
        second: br#"{"feature":"off"}"#,
        second_label: Some("v-two"),
    },
    ContentCase {
        name: "yaml document",
        content_type: "application/x-yaml",
        first: b"feature: on\n",
        first_label: Some("release-1"),
        second: b"feature: off\nextra: 2\n",
        second_label: None,
    },
    ContentCase {
        name: "plain text",
        content_type: "text/plain",
        first: b"on",
        first_label: None,
        second: b"off",
        second_label: Some("flag-2"),
    },
    ContentCase {
        name: "binary blob with non-utf8 bytes",
        content_type: "application/octet-stream",
        first: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        first_label: None,
        second: &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
        second_label: Some("binary-2"),
    },
];

/// The whole polling loop, for every content class: the first poll serves the version
/// the environment's deployment deployed, the next poll serves an empty payload
/// because nothing changed, and a poll after a new deployment serves the new content
/// and label. Every poll rotates the token.
#[tokio::test]
async fn a_poll_returns_content_only_when_the_deployed_version_changed() {
    for case in CONTENT_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        // A session is over a deployed configuration, so the version is deployed before
        // the session that polls it is started.
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                case.first,
                case.content_type,
                case.first_label,
            )
            .await;
        let token = emulator
            .session_token(
                &hierarchy.application_name,
                &hierarchy.environment_name,
                &hierarchy.profile_name,
            )
            .await;
        assert!(
            is_modelled_token(&token),
            "case {name}: the initial token must satisfy the modelled token pattern, got {token:?}"
        );

        // FIRST POLL — the session has been served nothing yet, so the deployed
        // content comes back with its content type and version label.
        let first = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            first.status,
            200,
            "case {name}: first poll expected 200, got {} body {}",
            first.status,
            first.text()
        );
        assert_eq!(
            first.body.as_ref(),
            case.first,
            "case {name}: first poll expected the deployed content, got {:?}",
            first.body.as_ref()
        );
        assert_eq!(
            first.header("content-type"),
            Some(case.content_type),
            "case {name}: first poll expected the deployed content type, got {:?}",
            first.header("content-type")
        );
        assert_eq!(
            first.header("version-label"),
            case.first_label,
            "case {name}: first poll expected the deployed version label, got {:?}",
            first.header("version-label")
        );
        assert_eq!(
            first.header("next-poll-interval-in-seconds"),
            Some(DEFAULT_POLL_INTERVAL_IN_SECONDS.to_string().as_str()),
            "case {name}: a session with no minimum must be told the default interval, got {:?}",
            first.header("next-poll-interval-in-seconds")
        );
        let second_token = first.next_token();
        assert!(
            is_modelled_token(&second_token) && second_token != token,
            "case {name}: the rotated token must be fresh and well formed, got {second_token:?}"
        );

        // SECOND POLL — nothing was deployed in between, so the client already has
        // the latest version: an empty payload, no version label, a fresh token.
        let second = emulator.get_latest_configuration(&second_token).await;
        assert_eq!(
            second.status,
            200,
            "case {name}: second poll expected 200, got {} body {}",
            second.status,
            second.text()
        );
        assert!(
            second.body.is_empty(),
            "case {name}: an unchanged configuration must return an empty payload, got {:?}",
            second.body.as_ref()
        );
        assert_eq!(
            second.header("version-label"),
            None,
            "case {name}: an unchanged configuration reports no version label, got {:?}",
            second.header("version-label")
        );
        assert_eq!(
            second.header("content-type"),
            Some(case.content_type),
            "case {name}: the content type describes the session's configuration and is \
             reported even when the payload is empty, got {:?}",
            second.header("content-type")
        );
        let third_token = second.next_token();
        assert!(
            is_modelled_token(&third_token) && third_token != second_token,
            "case {name}: the second poll must rotate the token too, got {third_token:?}"
        );

        // THIRD POLL — after a new version is deployed, it and its label come back.
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                case.second,
                case.content_type,
                case.second_label,
            )
            .await;
        let third = emulator.get_latest_configuration(&third_token).await;
        assert_eq!(
            third.status,
            200,
            "case {name}: third poll expected 200, got {} body {}",
            third.status,
            third.text()
        );
        assert_eq!(
            third.body.as_ref(),
            case.second,
            "case {name}: a changed configuration must return the new content, got {:?}",
            third.body.as_ref()
        );
        assert_eq!(
            third.header("version-label"),
            case.second_label,
            "case {name}: a changed configuration must return its version label, got {:?}",
            third.header("version-label")
        );

        // FOURTH POLL — and the change is served exactly once.
        let fourth = emulator
            .get_latest_configuration(&third.next_token())
            .await;
        assert_eq!(
            fourth.status,
            200,
            "case {name}: fourth poll expected 200, got {} body {}",
            fourth.status,
            fourth.text()
        );
        assert!(
            fourth.body.is_empty(),
            "case {name}: re-polling after the change must return an empty payload, got {:?}",
            fourth.body.as_ref()
        );
    }
}

/// How a triple may be spelled: each identifier is documented as "the ID or the name"
/// of its level, so every mixture of the two resolves to the same session.
#[tokio::test]
async fn a_session_resolves_each_identifier_by_id_or_by_name() {
    let spellings: &[(&str, [bool; 3])] = &[
        ("all three by name", [false, false, false]),
        ("all three by id", [true, true, true]),
        ("application by id, the rest by name", [true, false, false]),
        ("profile by id, the rest by name", [false, false, true]),
        ("environment by id, the rest by name", [false, true, false]),
    ];

    for (name, by_id) in spellings {
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                b"served",
                "text/plain",
                Some("spelling"),
            )
            .await;
        let pick = |use_id: bool, id: &str, resource_name: &str| {
            if use_id {
                id.to_string()
            } else {
                resource_name.to_string()
            }
        };
        let token = emulator
            .session_token(
                &pick(by_id[0], &hierarchy.application_id, &hierarchy.application_name),
                &pick(by_id[1], &hierarchy.environment_id, &hierarchy.environment_name),
                &pick(by_id[2], &hierarchy.profile_id, &hierarchy.profile_name),
            )
            .await;
        let reply = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            reply.status, 200,
            "case {name}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.body.as_ref(),
            b"served",
            "case {name}: expected the deployed content, got {:?}",
            reply.text()
        );
    }
}

/// Which level of the hierarchy a session named that the control plane does not hold.
struct MissingCase {
    name: &'static str,
    /// Each identifier as the request spells it; `None` means "the one that exists".
    application: Option<&'static str>,
    environment: Option<&'static str>,
    profile: Option<&'static str>,
    /// The modelled `ResourceType` value and the request member `ReferencedBy` is
    /// keyed by. The `ResourceType` values are the five the model's `ResourceType`
    /// enum declares (Application, ConfigurationProfile, Deployment, Environment,
    /// Configuration), listed in `src/ops/types/shared_types.rs` as
    /// `RESOURCE_TYPE_VALUES`.
    resource_type: &'static str,
    referenced_by: &'static str,
}

const MISSING_CASES: &[MissingCase] = &[
    MissingCase {
        name: "unknown application",
        application: Some("zzzzzzz"),
        environment: None,
        profile: None,
        resource_type: "Application",
        referenced_by: "ApplicationIdentifier",
    },
    MissingCase {
        name: "unknown environment",
        application: None,
        environment: Some("Beta"),
        profile: None,
        resource_type: "Environment",
        referenced_by: "EnvironmentIdentifier",
    },
    MissingCase {
        name: "unknown configuration profile",
        application: None,
        environment: None,
        profile: Some("OtherProfile"),
        resource_type: "ConfigurationProfile",
        referenced_by: "ConfigurationProfileIdentifier",
    },
    MissingCase {
        name: "nothing in common",
        application: Some("a"),
        environment: Some("b"),
        profile: Some("c"),
        // Resolved outermost first, so the application is the level reported.
        resource_type: "Application",
        referenced_by: "ApplicationIdentifier",
    },
    MissingCase {
        name: "maximum-length identifiers",
        application: Some(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        environment: None,
        profile: None,
        resource_type: "Application",
        referenced_by: "ApplicationIdentifier",
    },
];

/// `StartConfigurationSession` resolves its three identifiers against the control
/// plane before it mints anything: a level that does not exist is refused with the
/// modelled `ResourceNotFoundException`, naming the level and the request member that
/// named it — never a 201 with a token for a hierarchy that is not there.
#[tokio::test]
async fn a_session_for_a_resource_the_control_plane_lacks_is_not_found() {
    for case in MISSING_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        let application = case.application.unwrap_or(&hierarchy.application_name);
        let environment = case.environment.unwrap_or(&hierarchy.environment_name);
        let profile = case.profile.unwrap_or(&hierarchy.profile_name);

        let reply = emulator
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"{application}",
                      "EnvironmentIdentifier":"{environment}",
                      "ConfigurationProfileIdentifier":"{profile}"}}"#
            ))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {name}: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {name}: expected ResourceNotFoundException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.header("x-amzn-errortype"),
            Some("ResourceNotFoundException"),
            "case {name}: expected the error type header, got {:?}",
            reply.header("x-amzn-errortype")
        );
        assert_eq!(
            reply.json()["ResourceType"].as_str(),
            Some(case.resource_type),
            "case {name}: expected ResourceType {}, got {}",
            case.resource_type,
            reply.text()
        );
        assert_eq!(
            reply.json()["ReferencedBy"][case.referenced_by].as_str(),
            Some(match case.referenced_by {
                "ApplicationIdentifier" => application,
                "EnvironmentIdentifier" => environment,
                _ => profile,
            }),
            "case {name}: expected ReferencedBy to name the request member, got {}",
            reply.text()
        );
        assert!(
            reply.json()["Message"]
                .as_str()
                .is_some_and(|message| message == format!("{} not found", case.resource_type)),
            "case {name}: expected a Message naming the level that is missing, got {}",
            reply.text()
        );
    }
}

/// A poll serves the configuration ITS OWN profile has in force: once the deployment a
/// session was started over has been undone, that profile has nothing in force in the
/// environment — a deployment of another profile there is not what this session polls —
/// so there is nothing to serve and the refusal is the modelled
/// `ResourceNotFoundException` naming the `Configuration`.
///
/// Having BEEN deployed (which is what lets the session be started at all) and having a
/// configuration in force (which is what a poll serves) are two different questions, and
/// this is the case that separates them.
#[tokio::test]
async fn a_poll_with_nothing_in_force_for_its_profile_is_not_found() {
    let emulator = Emulator::new();
    let hierarchy = emulator
        .create_hierarchy("MyApp", "Prod", "MyProfile")
        .await;
    let other_profile = emulator
        .create_profile(&hierarchy.application_id, "OtherProfile")
        .await;
    emulator
        .deploy(
            &hierarchy,
            &hierarchy.profile_id,
            b"only for MyProfile",
            "text/plain",
            None,
        )
        .await;
    // OtherProfile is deployed and then that deployment is undone, so the profile has a
    // deployment but no configuration in force, while MyProfile's deployment — another
    // profile's, in the same environment — stays in force throughout.
    let undone = emulator
        .deploy_numbered(
            &hierarchy,
            &other_profile,
            b"withdrawn from OtherProfile",
            "text/plain",
            None,
        )
        .await;
    emulator.stop_deployment(&hierarchy, undone).await;

    let cases: &[(&str, &str)] = &[
        ("a profile with nothing in force, named by name", "OtherProfile"),
        ("the same profile named by id", &other_profile),
    ];
    for (name, profile) in cases {
        let token = emulator
            .session_token(
                &hierarchy.application_name,
                &hierarchy.environment_name,
                profile,
            )
            .await;
        let reply = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            reply.status, 404,
            "case {name}: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {name}: expected ResourceNotFoundException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.json()["ResourceType"].as_str(),
            Some("Configuration"),
            "case {name}: expected the Configuration resource type, got {}",
            reply.text()
        );
        assert!(
            reply.json()["ReferencedBy"].is_null(),
            "case {name}: a poll names no resource parameter of its own, so ReferencedBy \
             must be absent, got {}",
            reply.text()
        );
    }
}

/// How a token is made unusable before the poll that uses it.
enum TokenState {
    /// A syntactically valid token this emulator never minted.
    NeverMinted(&'static str),
    /// A token whose 24-hour lifetime has passed.
    Expired,
}

/// A token resolves to exactly one session and stays usable for as long as its lifetime
/// lasts: anything else is `BadRequestException` (400) naming the modelled problem,
/// under the input member the client got wrong.
#[tokio::test]
async fn an_unusable_token_is_refused_with_the_modelled_problem() {
    // `Corrupted` / `Expired` are two of the three modelled
    // `InvalidParameterProblem` values (the third, `PollIntervalNotSatisfied`, is
    // covered by the poll-interval test below).
    let cases = [
        (
            "token this emulator never minted",
            TokenState::NeverMinted("not-a-real-token"),
            "Corrupted",
        ),
        (
            "hex-shaped token this emulator never minted",
            TokenState::NeverMinted(
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            "Corrupted",
        ),
        (
            "single-character token",
            TokenState::NeverMinted("x"),
            "Corrupted",
        ),
        ("token past its 24-hour lifetime", TokenState::Expired, "Expired"),
    ];

    for (name, state, expected_problem) in cases {
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        emulator
            .deploy(&hierarchy, &hierarchy.profile_id, b"{}", "application/json", None)
            .await;

        let token = match state {
            TokenState::NeverMinted(token) => token.to_string(),
            TokenState::Expired => {
                let token = emulator
                    .session_token(
                        &hierarchy.application_name,
                        &hierarchy.environment_name,
                        &hierarchy.profile_name,
                    )
                    .await;
                let state = emulator.state();
                let mut guard = state.lock();
                let session = guard
                    .sessions
                    .get_mut(&token)
                    .expect("the session was persisted under its token");
                session.expires_at = SystemTime::now() - Duration::from_secs(1);
                drop(guard);
                token
            }
        };

        let reply = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            reply.status, 400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {name}: expected BadRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.json()["Reason"].as_str(),
            Some("InvalidParameters"),
            "case {name}: expected the modelled Reason, got {}",
            reply.text()
        );
        assert_eq!(
            reply.token_problem(),
            expected_problem,
            "case {name}: expected problem {expected_problem}, got {}",
            reply.text()
        );
        // The message is the service's own terse sentence for that problem; the
        // problem above is where a client branches.
        let expected_message = match expected_problem {
            "Corrupted" => "Token not valid",
            _ => "Token expired",
        };
        assert_eq!(
            reply.json()["Message"].as_str(),
            Some(expected_message),
            "case {name}: expected the service's message for {expected_problem}, got {}",
            reply.text()
        );
    }
}

/// `RequiredMinimumPollIntervalInSeconds` is modelled `range 15..86400`: each
/// published bound is accepted and one step past it is refused with the modelled
/// `BadRequestException`.
#[tokio::test]
async fn the_requested_minimum_poll_interval_is_held_to_its_modelled_range() {
    let cases: &[(&str, Option<i32>, u16)] = &[
        ("one below the minimum", Some(14), 400),
        ("the minimum bound", Some(15), 201),
        ("inside the range", Some(600), 201),
        ("the maximum bound", Some(86_400), 201),
        ("one above the maximum", Some(86_401), 400),
        ("zero", Some(0), 400),
        ("negative", Some(-15), 400),
        ("absent (optional member)", None, 201),
    ];

    for (name, requested, expected_status) in cases {
        let emulator = Emulator::new();
        let hierarchy = emulator.create_hierarchy("MyApp", "Prod", "MyProfile").await;
        // The interval is what is under test, so the triple a session is started over
        // carries a deployed configuration: a session is over a deployed one.
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                b"{}",
                "application/json",
                None,
            )
            .await;
        let interval = requested
            .map(|seconds| format!(r#","RequiredMinimumPollIntervalInSeconds":{seconds}"#))
            .unwrap_or_default();
        let reply = emulator
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod",
                      "ConfigurationProfileIdentifier":"MyProfile"{interval}}}"#
            ))
            .await;
        assert_eq!(
            reply.status, *expected_status,
            "case {name}: expected {expected_status}, got {} body {}",
            reply.status,
            reply.text()
        );
        if *expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {name}: expected BadRequestException, got {}",
                reply.text()
            );
        } else {
            assert!(
                is_modelled_token(&reply.initial_token()),
                "case {name}: expected a well-formed token, got {}",
                reply.text()
            );
        }
    }
}

/// An absent `@required` input member, wherever the model binds it.
struct AbsentMemberCase {
    name: &'static str,
    /// The `StartConfigurationSession` body to send, or `None` for the one route whose
    /// required member is bound to the query string.
    body: Option<&'static str>,
    /// The whole `Message` the refusal must carry.
    message: &'static str,
}

const ABSENT_MEMBER_CASES: &[AbsentMemberCase] = &[
    AbsentMemberCase {
        name: "no ApplicationIdentifier",
        body: Some(
            r#"{"EnvironmentIdentifier":"Prod","ConfigurationProfileIdentifier":"MyProfile"}"#,
        ),
        message: "1 validation error detected: Value null at 'applicationIdentifier' failed \
                  to satisfy constraint: Member must not be null",
    },
    AbsentMemberCase {
        name: "no EnvironmentIdentifier",
        body: Some(
            r#"{"ApplicationIdentifier":"MyApp","ConfigurationProfileIdentifier":"MyProfile"}"#,
        ),
        message: "1 validation error detected: Value null at 'environmentIdentifier' failed \
                  to satisfy constraint: Member must not be null",
    },
    AbsentMemberCase {
        name: "no ConfigurationProfileIdentifier",
        body: Some(r#"{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod"}"#),
        message: "1 validation error detected: Value null at 'configurationProfileIdentifier' \
                  failed to satisfy constraint: Member must not be null",
    },
    AbsentMemberCase {
        name: "no configuration_token query parameter",
        body: None,
        // The one member bound to the query string, which earns AppConfigData's own
        // sentence rather than the body-member validation envelope.
        message: "ConfigurationToken must be provided. Use the StartConfigurationSession \
                  API to obtain an InitialConfigurationToken",
    },
];

/// A `@required` input member the request omits is refused with the operation's own
/// modelled `BadRequestException` (400), carrying a `Message` that says which member
/// was missing — including the one member a route binds to the query string, which
/// must not fall through to a 501.
#[tokio::test]
async fn an_absent_required_member_is_refused_with_the_modelled_bad_request() {
    for case in ABSENT_MEMBER_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        emulator.create_hierarchy("MyApp", "Prod", "MyProfile").await;
        let reply = match case.body {
            Some(body) => emulator.start_configuration_session(body).await,
            None => emulator.get_configuration(None).await,
        };
        assert_eq!(
            reply.status, 400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {name}: expected BadRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.header("x-amzn-errortype"),
            Some("BadRequestException"),
            "case {name}: expected the error type header, got {:?}",
            reply.header("x-amzn-errortype")
        );
        assert_eq!(
            reply.json()["Message"].as_str(),
            Some(case.message),
            "case {name}: expected Message {:?}, got {}",
            case.message,
            reply.text()
        );
    }
}

/// A value outside a modelled constraint is refused in the same modelled shape, with
/// the envelope naming the member and the bound it failed: the refusal class is one
/// class, whether the member was absent or merely out of range.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_with_the_modelled_bad_request() {
    let over_long = "i".repeat(129);
    // `Identifier` is modelled `@length 1..128` and `OptionalPollSeconds`
    // `@range 15..86400`, as `StartConfigurationSession`'s input members are listed in
    // the crate root's OPERATIONS.md.
    let cases: &[(&str, String, &str)] = &[
        (
            "empty ApplicationIdentifier",
            r#"{"ApplicationIdentifier":"","EnvironmentIdentifier":"Prod",
                "ConfigurationProfileIdentifier":"MyProfile"}"#
                .to_string(),
            "Value '' at 'applicationIdentifier' failed to satisfy constraint: Member must \
             have length greater than or equal to 1",
        ),
        (
            "over-long EnvironmentIdentifier",
            format!(
                r#"{{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"{over_long}",
                    "ConfigurationProfileIdentifier":"MyProfile"}}"#
            ),
            "at 'environmentIdentifier' failed to satisfy constraint: Member must have \
             length less than or equal to 128",
        ),
        (
            "poll interval one below the modelled minimum",
            r#"{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod",
                "ConfigurationProfileIdentifier":"MyProfile",
                "RequiredMinimumPollIntervalInSeconds":14}"#
                .to_string(),
            "Value '14' at 'requiredMinimumPollIntervalInSeconds' failed to satisfy \
             constraint: Member must have value greater than or equal to 15",
        ),
        (
            "poll interval one above the modelled maximum",
            r#"{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod",
                "ConfigurationProfileIdentifier":"MyProfile",
                "RequiredMinimumPollIntervalInSeconds":86401}"#
                .to_string(),
            "Value '86401' at 'requiredMinimumPollIntervalInSeconds' failed to satisfy \
             constraint: Member must have value less than or equal to 86400",
        ),
    ];

    for (name, body, expected_clause) in cases {
        let emulator = Emulator::new();
        emulator.create_hierarchy("MyApp", "Prod", "MyProfile").await;
        let reply = emulator.start_configuration_session(body).await;
        assert_eq!(
            reply.status, 400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {name}: expected BadRequestException, got {}",
            reply.text()
        );
        let message = reply.json()["Message"].as_str().unwrap_or("").to_string();
        assert!(
            message.starts_with("1 validation error detected: ")
                && message.contains(expected_clause),
            "case {name}: expected a Message containing {expected_clause:?}, got {message:?}"
        );
    }
}

/// The reported poll interval is the session's own minimum when it set one, and a
/// session that set one may not poll faster than it: the early poll is refused with
/// `PollIntervalNotSatisfied` and leaves its token usable.
#[tokio::test]
async fn a_sessions_minimum_poll_interval_is_reported_and_enforced() {
    let cases: &[(&str, Option<i32>, i32, bool)] = &[
        ("no minimum requested", None, DEFAULT_POLL_INTERVAL_IN_SECONDS, true),
        ("the minimum bound", Some(15), 15, false),
        ("a ten-minute minimum", Some(600), 600, false),
        ("the maximum bound", Some(86_400), 86_400, false),
    ];

    for (name, requested, expected_interval, immediate_repoll_allowed) in cases {
        let emulator = Emulator::new();
        let hierarchy = emulator
            .create_hierarchy("MyApp", "Prod", "MyProfile")
            .await;
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                br#"{"feature":"on"}"#,
                "application/json",
                Some("v-one"),
            )
            .await;
        let interval = requested
            .map(|seconds| format!(r#","RequiredMinimumPollIntervalInSeconds":{seconds}"#))
            .unwrap_or_default();
        let started = emulator
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod",
                      "ConfigurationProfileIdentifier":"MyProfile"{interval}}}"#
            ))
            .await;
        assert_eq!(
            started.status, 201,
            "case {name}: expected 201, got {} body {}",
            started.status,
            started.text()
        );

        // The first poll is always allowed: the session has not polled yet.
        let first = emulator
            .get_latest_configuration(&started.initial_token())
            .await;
        assert_eq!(
            first.status, 200,
            "case {name}: first poll expected 200, got {} body {}",
            first.status,
            first.text()
        );
        assert_eq!(
            first.header("next-poll-interval-in-seconds"),
            Some(expected_interval.to_string().as_str()),
            "case {name}: expected a {expected_interval}s poll interval, got {:?}",
            first.header("next-poll-interval-in-seconds")
        );

        let next = first.next_token();
        let immediate = emulator.get_latest_configuration(&next).await;
        if *immediate_repoll_allowed {
            assert_eq!(
                immediate.status, 200,
                "case {name}: a session with no minimum may poll again at once, got {} body {}",
                immediate.status,
                immediate.text()
            );
        } else {
            assert_eq!(
                immediate.status, 400,
                "case {name}: polling inside the minimum interval expected 400, got {} body {}",
                immediate.status,
                immediate.text()
            );
            assert_eq!(
                immediate.token_problem(),
                "PollIntervalNotSatisfied",
                "case {name}: expected PollIntervalNotSatisfied, got {}",
                immediate.text()
            );
            assert_eq!(
                immediate.json()["Message"].as_str(),
                Some("Request too early"),
                "case {name}: expected the service's too-early message, got {}",
                immediate.text()
            );
            let state = emulator.state();
            let session = state.lock().session(&next).cloned();
            assert_eq!(
                session.map(|session| session.has_served_a_poll()),
                Some(false),
                "case {name}: a refused poll must leave the token usable"
            );
        }
    }
}

/// A token is an opaque handle to stored state, not an encoding of the request: two
/// sessions over identical input get different tokens, and no token carries the
/// identifiers it was started with.
#[tokio::test]
async fn tokens_are_opaque_and_unique() {
    let cases = [
        ("short identifiers", ("ap", "en", "pr")),
        ("ordinary identifiers", ("MyApp", "Prod", "MyProfile")),
        (
            "identifiers that look like ids",
            ("abcd123", "wxyz789", "pqrs456"),
        ),
        (
            "maximum-length identifiers",
            (
                // `Name` is modelled `len 1..64` on the control plane, so the longest
                // name a session can be started over is the longest one AppConfig will
                // create.
                &"a".repeat(64)[..],
                &"b".repeat(64)[..],
                &"c".repeat(64)[..],
            ),
        ),
    ];

    let emulator = Emulator::new();
    let mut seen: Vec<String> = Vec::new();
    for (name, (application, environment, profile)) in cases {
        let hierarchy = emulator
            .create_hierarchy(application, environment, profile)
            .await;
        emulator
            .deploy(
                &hierarchy,
                &hierarchy.profile_id,
                b"content",
                "text/plain",
                None,
            )
            .await;
        let first = emulator
            .session_token(application, environment, profile)
            .await;
        let second = emulator
            .session_token(application, environment, profile)
            .await;
        assert_ne!(
            first, second,
            "case {name}: two sessions over identical input must get different tokens"
        );
        for token in [&first, &second] {
            assert!(
                is_modelled_token(token),
                "case {name}: {token:?} does not satisfy the modelled token pattern"
            );
            for identifier in [application, environment, profile] {
                // An identifier shorter than 8 characters can appear in 64 hex
                // characters by chance, so the echo check is applied to the rows
                // whose identifiers cannot: it is the token's independence from the
                // request that is under test, not hex arithmetic.
                if identifier.len() >= 8 {
                    assert!(
                        !token.contains(identifier),
                        "case {name}: the token must not echo the request, {token:?} contains {identifier:?}"
                    );
                }
            }
            assert!(
                !seen.contains(token),
                "case {name}: {token:?} was minted twice"
            );
            seen.push(token.clone());
        }
        // And both resolve to their OWN session: each is usable exactly once.
        for token in [&first, &second] {
            let reply = emulator.get_latest_configuration(token).await;
            assert_eq!(
                reply.status, 200,
                "case {name}: {token:?} must resolve to its session, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.body.as_ref(),
                b"content",
                "case {name}: {token:?} must serve its own session's configuration, got {:?}",
                reply.text()
            );
        }
    }
}

/// The session store is bounded: entries past their expiry are reclaimed when the
/// next session is stored, and the store never grows past `MAX_SESSIONS` however many
/// sessions are minted — while the token most recently handed out still resolves.
///
/// Driven against the state API rather than over HTTP: the bound is a property of the
/// store, and reaching it takes more sessions than an HTTP test should mint.
#[test]
fn the_session_store_is_bounded() {
    let mut state = ComAmazonawsAppconfigdataAppconfigdataState::default();
    let now = SystemTime::now();

    // An entry whose lifetime has passed is reclaimed, not kept for ever. Each session
    // is minted at the position in a rollout the store mints for it, exactly as
    // `StartConfigurationSession` does.
    let bucket = state.mint_rollout_bucket(now);
    let stale = state.start_session("MyApp", "Prod", "MyProfile", None, None, bucket, now);
    state
        .sessions
        .get_mut(&stale)
        .expect("the session was persisted under its token")
        .expires_at = now - Duration::from_secs(1);
    let bucket = state.mint_rollout_bucket(now);
    let fresh = state.start_session("MyApp", "Prod", "MyProfile", None, None, bucket, now);
    assert!(
        state.session(&stale).is_none(),
        "case expired entry: an entry past its expiry must be reclaimed"
    );
    assert!(
        state.session(&fresh).is_some(),
        "case expired entry: the live entry must survive the sweep"
    );

    // And minting far past the cap holds the store at the cap.
    let mut latest = fresh;
    for round in 0..(MAX_SESSIONS + 200) {
        let bucket = state.mint_rollout_bucket(now);
        latest = state.start_session("MyApp", "Prod", "MyProfile", None, None, bucket, now);
        assert!(
            state.sessions.len() <= MAX_SESSIONS,
            "case round {round}: the store must stay within {MAX_SESSIONS} entries, holds {}",
            state.sessions.len()
        );
    }
    assert!(
        state.session(&latest).is_some(),
        "case cap reached: the most recently minted token must still resolve"
    );
}
