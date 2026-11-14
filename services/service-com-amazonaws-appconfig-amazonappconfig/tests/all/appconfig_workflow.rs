// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppConfig's core workflow: organise an application, host a
//! configuration version in it, deploy that version to an environment, and read every
//! one of those resources back.
//!
//! Each suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body, bound headers) rather than about any handler's
//! internals. The clock-driven half of a deployment — the growth steps, the bake
//! period and the completion the strategy's documented durations schedule — is
//! exercised against `state::Deployment` directly, so the schedule can be asserted
//! across twenty minutes of deployment without any test sleeping.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    self, ComAmazonawsAppconfigAmazonappconfigState, Deployment, DeploymentStrategy, NewDeployment,
    PREDEFINED_DEPLOYMENT_STRATEGIES,
};
use service_com_amazonaws_appconfig_amazonappconfig::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `appconfig`. Neither the key id nor the signature is a
/// credential: this harness turns request authentication off below, exactly as the
/// emulator runs it unless started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/appconfig/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// One service mounted on its own engine, reached exactly the way the production
/// router reaches it.
pub(crate) fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // `emulator_server::authentication_enabled()` is off unless the launcher turns it
    // on, and these tests assert AppConfig's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// A router over a fresh engine, for a suite that needs no direct state access.
///
/// `pub(crate)` because the other suites in this target drive the same front door.
pub(crate) fn router() -> MultiServiceRouter {
    router_on(&Arc::new(engine::InMemoryEngine::new()))
}

/// The one account id a generated emulator presents, which every refusal whose template
/// carries an account clause must name. `pub(crate)` because more than one suite asserts
/// against it.
pub(crate) fn account() -> &'static str {
    engine::emulator_account_id()
}

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body, or `None` when absent or not a string.
    pub(crate) fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }

    /// A bound response header's value.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The wire error code (`__type`) of an error response.
    pub(crate) fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| self.header("x-amzn-errortype"))
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }
}

/// Dispatch one request with the given body and extra headers.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    headers: &[(&str, String)],
) -> Answer {
    dispatch(router, method, uri, Some(content_type), body, headers).await
}

/// Dispatch one request carrying NO `content-type` header at all, which is how a caller
/// omits the `@required` `Content-Type` member an operation binds to it.
pub(crate) async fn send_without_content_type(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Vec<u8>,
) -> Answer {
    dispatch(router, method, uri, None, body, &[]).await
}

/// Dispatch one request, with `content_type` as the `content-type` header when the caller
/// sends one.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    headers: &[(&str, String)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://appconfig.us-east-1.amazonaws.com{uri}"))
        .header("host", "appconfig.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `POST`/`GET` a JSON request, the shape every operation here except the hosted
/// configuration version uses.
pub(crate) async fn json_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &serde_json::Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    send(router, method, uri, "application/json", payload, &[]).await
}

/// A retrieval role of the shape the model's `RoleArn` pattern demands
/// (`^((arn):(aws|…):(iam)::\d{12}:role[/].*)$`).
///
/// Every `LocationUri` but the hosted store and a CodePipeline pipeline needs one: "A
/// retrieval role ARN is not required for configurations stored in CodePipeline or the
/// AppConfig hosted configuration store. It is required for all other sources that store
/// your configuration" (the model's `RetrievalRoleArn` documentation).
pub(crate) const RETRIEVAL_ROLE: &str = "arn:aws:iam::000000000000:role/appconfig-retrieval";

/// `body` with a `RetrievalRoleArn` added when its `LocationUri` needs one, so a row can
/// name a location without restating the rule.
pub(crate) fn with_required_retrieval_role(mut body: serde_json::Value) -> serde_json::Value {
    let needs_role = body
        .get("LocationUri")
        .and_then(serde_json::Value::as_str)
        .is_some_and(state::location_uri_requires_retrieval_role);
    if needs_role && body.get("RetrievalRoleArn").is_none() {
        body["RetrievalRoleArn"] = serde_json::Value::from(RETRIEVAL_ROLE);
    }
    body
}

/// The `DeploymentState` values a deployment stops at: "COMPLETE", a rollback that
/// finished, and the revert of one that had completed. What [`deployment_in_state`] waits
/// for when a caller is waiting for a deployment to be over.
pub(crate) const TERMINAL_DEPLOYMENT_STATES: &[&str] = &["COMPLETE", "ROLLED_BACK", "REVERTED"];

/// How long [`deployment_in_state`] waits for a deployment to reach the state it is
/// polling for.
///
/// A generous ceiling on the WAIT rather than a statement about the span any stage takes:
/// a deployment that has not got there by here is one nothing is moving.
const DEPLOYMENT_STATE_BY: Duration = Duration::from_secs(60);

/// How often [`deployment_in_state`] re-reads the deployment while waiting.
const DEPLOYMENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Poll `GetDeployment` until the deployment reports one of `states`, and answer the
/// document that reported it.
///
/// Every stage of a deployment after the one `StartDeployment` answers is owed by the
/// CLOCK, so a caller that needs a later stage waits for the CONDITION — this re-reads
/// until the state holds instead of sleeping for a span. Fails the case, naming it and the
/// last state read, when the deployment does not get there.
pub(crate) async fn deployment_in_state(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    environment_id: &str,
    number: i64,
    states: &[&str],
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + DEPLOYMENT_STATE_BY;
    loop {
        let read = json_call(
            router,
            "GET",
            &format!(
                "/applications/{application_id}/environments/{environment_id}/deployments/{number}"
            ),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetDeployment expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let document = read.json();
        let state = read.string("State").unwrap_or_default();
        if states.contains(&state.as_str()) {
            return document;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: deployment {number} did not report any of {states:?} within {}s; \
             last state {state:?}",
            DEPLOYMENT_STATE_BY.as_secs()
        );
        tokio::time::sleep(DEPLOYMENT_POLL_INTERVAL).await;
    }
}

/// Create an application and return its minted id.
pub(crate) async fn create_application(router: &MultiServiceRouter, name: &str) -> String {
    let answer = json_call(
        router,
        "POST",
        "/applications",
        &serde_json::json!({ "Name": name }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateApplication must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateApplication answered no Id"))
}

/// Create an environment under `application_id` and return its minted id.
pub(crate) async fn create_environment(
    router: &MultiServiceRouter,
    application_id: &str,
    name: &str,
) -> String {
    let answer = json_call(
        router,
        "POST",
        &format!("/applications/{application_id}/environments"),
        &serde_json::json!({ "Name": name }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateEnvironment must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateEnvironment answered no Id"))
}

/// Create a hosted configuration profile under `application_id` and return its id.
pub(crate) async fn create_profile(
    router: &MultiServiceRouter,
    application_id: &str,
    name: &str,
) -> String {
    let answer = json_call(
        router,
        "POST",
        &format!("/applications/{application_id}/configurationprofiles"),
        &serde_json::json!({ "Name": name, "LocationUri": "hosted" }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateConfigurationProfile must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// Create a hosted configuration profile that stores FEATURE FLAGS under `application_id`
/// and return its minted id.
///
/// [`create_profile`] declares no `Type`, so AppConfig stores that profile as
/// `AWS.Freeform` — and an experiment definition targets a feature flag, so it can only be
/// bound to a profile of `state::FEATURE_FLAGS_PROFILE_TYPE`. Every suite that creates a
/// definition builds its profile here.
pub(crate) async fn create_flags_profile(
    router: &MultiServiceRouter,
    application_id: &str,
    name: &str,
) -> String {
    let answer = create_profile_from(
        router,
        application_id,
        name,
        &serde_json::json!({
            "Name": name,
            "LocationUri": "hosted",
            "Type": service_com_amazonaws_appconfig_amazonappconfig::state::FEATURE_FLAGS_PROFILE_TYPE,
        }),
    )
    .await;
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// Create a configuration profile under `application_id` from `body` and answer what
/// `CreateConfigurationProfile` reported, so a suite can assert against the created
/// record rather than only against its id.
///
/// `case` names the row the call belongs to, the way every assertion in this target
/// does. [`create_profile`] is the shorthand for the hosted, name-only profile most
/// suites need.
pub(crate) async fn create_profile_from(
    router: &MultiServiceRouter,
    application_id: &str,
    case: &str,
    body: &serde_json::Value,
) -> Answer {
    let answer = json_call(
        router,
        "POST",
        &format!("/applications/{application_id}/configurationprofiles"),
        body,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateConfigurationProfile must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
}

/// Write one hosted configuration version of `profile`, answering the version number it was
/// stored under.
///
/// Shared because every suite that DEPLOYS a configuration needs the version it deploys to
/// exist: `StartDeployment` resolves `ConfigurationVersion` against the versions the named
/// profile's hosted store holds, so a suite that deploys without hosting is deploying
/// content AppConfig does not have.
pub(crate) async fn host_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    content: &[u8],
    version_label: Option<&str>,
) -> i64 {
    let headers: Vec<(&str, String)> = version_label
        .map(|label| vec![("VersionLabel", label.to_string())])
        .unwrap_or_default();
    let answer = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &headers,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .header("Version-Number")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("case {case:?}: no Version-Number on the create response"))
}

/// Whether `id` satisfies the model's `Id` shape, `^[a-z0-9]{4,7}$`.
pub(crate) fn is_modelled_id(id: &str) -> bool {
    (4..=7).contains(&id.chars().count())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// `CreateApplication` mints an id of the modelled shape, persists the record, and
/// `GetApplication` reads back exactly what was stored — across the whole range of
/// names and descriptions the model accepts.
#[tokio::test]
async fn applications_round_trip_across_the_modelled_name_and_description_range() {
    let router = router();

    // Rows differ along what the rule generalises over: the length of `Name` at both
    // published bounds (1..64), whether `Description` is present, and how many tags
    // ride along.
    let cases: Vec<(&str, String, Option<&str>, usize)> = vec![
        ("one-character name", "a".to_string(), None, 0),
        ("ordinary name", "MyMobileApp".to_string(), Some("mobile"), 1),
        ("64-character name", "n".repeat(64), Some(""), 0),
        (
            "name with the punctuation AppConfig allows",
            "my app_1.2-3".to_string(),
            Some("a description that is long enough to be interesting"),
            3,
        ),
    ];

    let mut minted = Vec::new();
    for (case, name, description, tag_count) in &cases {
        let mut body = serde_json::json!({ "Name": name });
        if let Some(description) = description {
            body["Description"] = serde_json::Value::from(*description);
        }
        if *tag_count > 0 {
            let tags: serde_json::Map<String, serde_json::Value> = (0..*tag_count)
                .map(|index| (format!("key{index}"), serde_json::Value::from("value")))
                .collect();
            body["Tags"] = serde_json::Value::Object(tags);
        }

        let created = json_call(&router, "POST", "/applications", &body).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));
        assert!(
            is_modelled_id(&id),
            "case {case:?}: expected an Id matching ^[a-z0-9]{{4,7}}$, got {id:?}"
        );
        assert_eq!(
            created.string("Name").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the stored Name, got {:?}",
            created.string("Name")
        );
        assert_eq!(
            created.string("Description").as_deref(),
            *description,
            "case {case:?}: expected the stored Description, got {:?}",
            created.string("Description")
        );

        // The round trip: the create is only real if a later read sees it.
        let fetched = json_call(&router, "GET", &format!("/applications/{id}"), &serde_json::Value::Null).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetApplication expected 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.json(),
            created.json(),
            "case {case:?}: GetApplication must report what CreateApplication stored, got {}",
            fetched.json()
        );

        minted.push((case, id));
    }

    for (index, (case, id)) in minted.iter().enumerate() {
        assert!(
            minted.iter().skip(index + 1).all(|(_, other)| other != id),
            "case {case:?}: minted id {id:?} was handed out twice"
        );
    }
}

/// A child resource cannot be created under an application that does not exist: both
/// child-creating operations answer 404 `ResourceNotFoundException`.
#[tokio::test]
async fn creating_a_child_under_an_unknown_application_is_not_found() {
    let router = router();
    let known = create_application(&router, "parent").await;

    // Rows differ along the shape of the unresolvable parent id, and the last row
    // proves the check is about existence rather than about the id's spelling.
    let near_miss = format!("{known}x");
    let unknown_ids = ["nosuchapp", "zzzzzzz", "a", "0000", near_miss.as_str()];

    for unknown in unknown_ids {
        for (case, uri, body) in [
            (
                "CreateEnvironment",
                format!("/applications/{unknown}/environments"),
                serde_json::json!({ "Name": "prod" }),
            ),
            (
                "CreateConfigurationProfile",
                format!("/applications/{unknown}/configurationprofiles"),
                serde_json::json!({ "Name": "config", "LocationUri": "hosted" }),
            ),
        ] {
            let answer = json_call(&router, "POST", &uri, &body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} with application {unknown:?}: expected 404, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?} with application {unknown:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }
}

/// An environment and a configuration profile are persisted under their parent and read
/// back through their own `Get`, including the optional members the caller supplied.
#[tokio::test]
async fn environments_and_profiles_round_trip_with_their_optional_members() {
    let router = router();
    let application = create_application(&router, "round-trip").await;

    // Rows differ along the optional members supplied: none, monitors, and validators
    // of each `ValidatorType` the model lists.
    let environment_cases = [
        ("no monitors", serde_json::json!([])),
        (
            "one monitor",
            serde_json::json!([{ "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:latency" }]),
        ),
        (
            "two monitors",
            serde_json::json!([
                { "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:a" },
                { "AlarmArn": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:b" },
            ]),
        ),
    ];

    for (case, monitors) in &environment_cases {
        let mut body = serde_json::json!({ "Name": format!("env-{case}"), "Description": *case });
        if !monitors.as_array().is_some_and(Vec::is_empty) {
            body["Monitors"] = monitors.clone();
        }
        let created = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments"),
            &body,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.string("State").as_deref(),
            Some(state::ENVIRONMENT_READY_FOR_DEPLOYMENT),
            "case {case:?}: a new environment has nothing deploying into it, got {:?}",
            created.string("State")
        );
        assert_eq!(
            created.string("ApplicationId").as_deref(),
            Some(application.as_str()),
            "case {case:?}: expected the parent application id, got {:?}",
            created.string("ApplicationId")
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        let fetched = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{id}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            fetched.json(),
            created.json(),
            "case {case:?}: GetEnvironment must report what CreateEnvironment stored, got {}",
            fetched.json()
        );
    }

    // `Type` and `Validators` are the profile's own optional members; `ValidatorType`
    // values come from the model's `ValidatorType` enum (JSON_SCHEMA | LAMBDA). A
    // request that declares no `Type` creates a freeform profile, so the expected
    // `Type` of such a row is "AWS.Freeform" (see the
    // `configuration_profile_type` test for that rule over the whole class).
    let profile_cases = [
        ("hosted, freeform", "hosted", Some("AWS.Freeform"), serde_json::Value::Null),
        (
            "hosted feature flags",
            "hosted",
            Some("AWS.AppConfig.FeatureFlags"),
            serde_json::Value::Null,
        ),
        (
            "ssm parameter, no type",
            "ssm-parameter://my-parameter",
            None,
            serde_json::Value::Null,
        ),
        ("hosted, no type", "hosted", None, serde_json::Value::Null),
        (
            "s3 object with a JSON schema validator",
            "s3://amzn-s3-demo-bucket/my-app/config.json",
            Some("AWS.Freeform"),
            serde_json::json!([{ "Type": "JSON_SCHEMA", "Content": "{\"type\":\"object\"}" }]),
        ),
        (
            "hosted with a lambda validator",
            "hosted",
            Some("AWS.Freeform"),
            serde_json::json!([{
                "Type": "LAMBDA",
                "Content": "arn:aws:lambda:us-east-1:000000000000:function:validate"
            }]),
        ),
    ];

    for (case, location_uri, profile_type, validators) in &profile_cases {
        let mut body = serde_json::json!({ "Name": format!("profile-{case}"), "LocationUri": location_uri });
        if let Some(profile_type) = profile_type {
            body["Type"] = serde_json::Value::from(*profile_type);
        }
        if !validators.is_null() {
            body["Validators"] = validators.clone();
        }
        let body = with_required_retrieval_role(body);
        let created = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/configurationprofiles"),
            &body,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.string("LocationUri").as_deref(),
            Some(*location_uri),
            "case {case:?}: expected the stored LocationUri, got {:?}",
            created.string("LocationUri")
        );
        assert_eq!(
            created.string("Type").as_deref(),
            Some(profile_type.unwrap_or("AWS.Freeform")),
            "case {case:?}: expected the stored Type, got {:?}",
            created.string("Type")
        );
        let id = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        let fetched = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/configurationprofiles/{id}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            fetched.json(),
            created.json(),
            "case {case:?}: GetConfigurationProfile must report what CreateConfigurationProfile stored, got {}",
            fetched.json()
        );
    }
}

/// Hosted configuration versions are numbered 1, 2, 3, … per configuration profile, and
/// every version's bytes come back exactly as they were written — whatever the payload
/// is.
#[tokio::test]
async fn hosted_configuration_versions_are_numbered_per_profile_and_keep_their_bytes() {
    let router = router();
    let application = create_application(&router, "hosting").await;
    let first_profile = create_profile(&router, &application, "first").await;
    let second_profile = create_profile(&router, &application, "second").await;

    // Rows differ along the payload's content type, size and byte values, including
    // bytes no text encoding can carry.
    let payloads: Vec<(&str, &str, Vec<u8>)> = vec![
        ("json object", "application/json", br#"{"enabled":true}"#.to_vec()),
        ("yaml document", "application/x-yaml", b"enabled: true\nlimit: 7\n".to_vec()),
        ("single byte", "text/plain", vec![b'x']),
        ("bytes with NULs and high bits", "application/octet-stream", vec![0, 1, 2, 255, 0, 128]),
        ("64 KiB of text", "text/plain", vec![b'a'; 64 * 1024]),
    ];

    for profile in [&first_profile, &second_profile] {
        for (index, (case, content_type, content)) in payloads.iter().enumerate() {
            let expected_version = i32::try_from(index + 1).expect("the case count fits in i32");
            let created = send(
                &router,
                "POST",
                &format!(
                    "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
                ),
                content_type,
                content.clone(),
                &[("Description", (*case).to_string())],
            )
            .await;
            assert_eq!(
                created.status,
                201,
                "case {case:?}: expected 201, got {} / {}",
                created.status,
                String::from_utf8_lossy(&created.body)
            );
            assert_eq!(
                created.header("Version-Number"),
                Some(expected_version.to_string()),
                "case {case:?}: version numbers must increase by one per profile, got {:?}",
                created.header("Version-Number")
            );
            assert_eq!(
                created.header("Configuration-Profile-Id").as_deref(),
                Some(profile.as_str()),
                "case {case:?}: expected the profile that holds the version, got {:?}",
                created.header("Configuration-Profile-Id")
            );
            assert_eq!(
                created.header("Content-Type").as_deref(),
                Some(*content_type),
                "case {case:?}: expected the stored ContentType, got {:?}",
                created.header("Content-Type")
            );
            assert_eq!(
                created.body.as_ref(),
                content.as_slice(),
                "case {case:?}: CreateHostedConfigurationVersion returned {} bytes, expected {}",
                created.body.len(),
                content.len()
            );

            let fetched = send(
                &router,
                "GET",
                &format!(
                    "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{expected_version}"
                ),
                "application/json",
                Vec::new(),
                &[],
            )
            .await;
            assert_eq!(
                fetched.status,
                200,
                "case {case:?}: GetHostedConfigurationVersion expected 200, got {} / {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert_eq!(
                fetched.body.as_ref(),
                content.as_slice(),
                "case {case:?}: the hosted configuration must read back byte for byte, got {} bytes",
                fetched.body.len()
            );
            assert_eq!(
                fetched.header("Version-Number"),
                Some(expected_version.to_string()),
                "case {case:?}: expected version {expected_version}, got {:?}",
                fetched.header("Version-Number")
            );
            assert_eq!(
                fetched.header("Description").as_deref(),
                Some(*case),
                "case {case:?}: expected the stored Description, got {:?}",
                fetched.header("Description")
            );
        }
    }

    // A version number that was never written is not found.
    let missing = send(
        &router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{first_profile}/hostedconfigurationversions/99"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "case \"unwritten version\": expected 404, got {} / {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );
}

/// `LatestVersionNumber` is the model's optimistic lock: a write that names a version
/// other than the profile's current latest is refused with 409 `ConflictException`.
#[tokio::test]
async fn latest_version_number_locks_a_hosted_configuration_write() {
    let router = router();
    let application = create_application(&router, "locking").await;
    let profile = create_profile(&router, &application, "locked").await;
    let uri = format!(
        "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
    );

    // Rows walk the lock across every state it can be in: no versions yet (latest 0),
    // one version, and both a too-low and a too-high claim at each point.
    let cases: [(&str, Option<i32>, u16); 6] = [
        ("empty profile, claims 0", Some(0), 201),
        ("one version, claims 1", Some(1), 201),
        ("two versions, claims 1", Some(1), 409),
        ("two versions, claims 5", Some(5), 409),
        ("two versions, claims 0", Some(0), 409),
        ("two versions, no claim", None, 201),
    ];

    for (row, (case, claim, expected)) in cases.into_iter().enumerate() {
        let headers: Vec<(&str, String)> = claim
            .map(|claim| vec![("Latest-Version-Number", claim.to_string())])
            .unwrap_or_default();
        // Content of its own per row: content byte-identical to the version a profile
        // already holds REUSES that version rather than allocating one, so rows that are
        // meant to move the lock's latest version forward write different bytes.
        let content = format!(r#"{{"enabled":false,"row":{row}}}"#).into_bytes();
        let answer = send(
            &router,
            "POST",
            &uri,
            "application/json",
            content,
            &headers,
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected == 409 {
            assert!(
                answer.error_code().contains("ConflictException"),
                "case {case:?}: expected ConflictException, got {:?}",
                answer.error_code()
            );
        }
    }
}

/// Every predefined `AppConfig.*` deployment strategy is deployable without a prior
/// `CreateDeploymentStrategy`, and the deployment records that strategy's own duration,
/// growth type, growth factor and bake time — read back through `GetDeployment`.
#[tokio::test]
async fn predefined_strategies_deploy_and_the_deployment_reads_back() {
    let router = router();
    let application = create_application(&router, "deploying").await;
    let profile = create_profile(&router, &application, "deployed-config").await;

    let version = send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        version.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        version.status,
        String::from_utf8_lossy(&version.body)
    );

    // Every row of the predefined catalog, including the strategies the issue did not
    // cite. Each gets its own environment, because only one deployment at a time can
    // be in progress for an environment.
    for predefined in PREDEFINED_DEPLOYMENT_STRATEGIES {
        let case = predefined.id;
        let environment = create_environment(&router, &application, &format!("env-{case}")).await;

        let started = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments/{environment}/deployments"),
            &serde_json::json!({
                "DeploymentStrategyId": predefined.id,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
                "Description": format!("deploying with {case}"),
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let body = started.json();
        assert_eq!(
            body.get("DeploymentNumber").and_then(serde_json::Value::as_i64),
            Some(1),
            "case {case:?}: an environment's first deployment is number 1, got {body}"
        );

        let fetched = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{environment}/deployments/1"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeployment expected 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        let read = fetched.json();

        for (member, expected) in [
            (
                "DeploymentDurationInMinutes",
                i64::from(predefined.deployment_duration_in_minutes),
            ),
            (
                "FinalBakeTimeInMinutes",
                i64::from(predefined.final_bake_time_in_minutes),
            ),
        ] {
            assert_eq!(
                read.get(member).and_then(serde_json::Value::as_i64),
                Some(expected),
                "case {case:?}: GetDeployment must report the persisted {member} {expected}, got {:?}",
                read.get(member)
            );
        }
        assert_eq!(
            read.get("GrowthFactor").and_then(serde_json::Value::as_f64),
            Some(f64::from(predefined.growth_factor)),
            "case {case:?}: GetDeployment must report the persisted GrowthFactor {}, got {:?}",
            predefined.growth_factor,
            read.get("GrowthFactor")
        );
        for (member, expected) in [
            ("GrowthType", predefined.growth_type),
            ("DeploymentStrategyId", predefined.id),
            ("ConfigurationProfileId", profile.as_str()),
            ("ConfigurationVersion", "1"),
            ("ConfigurationName", "deployed-config"),
            ("ConfigurationLocationUri", "hosted"),
        ] {
            assert_eq!(
                read.get(member).and_then(serde_json::Value::as_str),
                Some(expected),
                "case {case:?}: GetDeployment must report the persisted {member} {expected:?}, got {:?}",
                read.get(member)
            );
        }

        // A deployment that has only just started is not finished: AppConfig answers a
        // non-terminal state, and the event log already carries its start.
        let state_now = read
            .get("State")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            state_now == "DEPLOYING" || state_now == "BAKING",
            "case {case:?}: expected a deployment in progress, got State {state_now:?}"
        );
        let event_log = read
            .get("EventLog")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !event_log.is_empty(),
            "case {case:?}: expected a non-empty EventLog, got {read}"
        );
        assert_eq!(
            event_log
                .last()
                .and_then(|event| event.get("EventType"))
                .and_then(serde_json::Value::as_str),
            Some("DEPLOYMENT_STARTED"),
            "case {case:?}: the model says the most recent events come first, so the start \
             event is last; got {event_log:?}"
        );
        assert!(
            read.get("StartedAt").is_some(),
            "case {case:?}: expected StartedAt, got {read}"
        );
        assert!(
            read.get("CompletedAt").is_none(),
            "case {case:?}: a deployment in progress has no CompletedAt, got {read}"
        );

        // A second deployment into the same environment conflicts with the first.
        let conflicting = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments/{environment}/deployments"),
            &serde_json::json!({
                "DeploymentStrategyId": predefined.id,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            conflicting.status, 409,
            "case {case:?}: a second concurrent deployment must conflict, got {} / {}",
            conflicting.status,
            String::from_utf8_lossy(&conflicting.body)
        );
        assert!(
            conflicting.error_code().contains("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            conflicting.error_code()
        );

        // And the environment reports that it is deploying.
        let environment_state = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{environment}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            environment_state.string("State").as_deref(),
            Some(state::ENVIRONMENT_DEPLOYING),
            "case {case:?}: an environment with a deployment in flight is Deploying, got {:?}",
            environment_state.string("State")
        );
    }
}

/// `StartDeployment` resolves every resource it names, and answers 404 for each one it
/// cannot; `GetDeployment` answers 404 for a deployment number that was never issued.
#[tokio::test]
async fn deployments_are_not_found_when_a_named_resource_does_not_exist() {
    let router = router();
    let application = create_application(&router, "missing-parts").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_profile(&router, &application, "config").await;

    // Rows differ along WHICH of the four resources a deployment names is absent.
    let cases: Vec<(&str, String, serde_json::Value)> = vec![
        (
            "unknown application",
            format!("/applications/nosuchapp/environments/{environment}/deployments"),
            serde_json::json!({
                "DeploymentStrategyId": "AppConfig.AllAtOnce",
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        ),
        (
            "unknown environment",
            format!("/applications/{application}/environments/nosuchenv/deployments"),
            serde_json::json!({
                "DeploymentStrategyId": "AppConfig.AllAtOnce",
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        ),
        (
            "unknown configuration profile",
            format!("/applications/{application}/environments/{environment}/deployments"),
            serde_json::json!({
                "DeploymentStrategyId": "AppConfig.AllAtOnce",
                "ConfigurationProfileId": "zzzz9",
                "ConfigurationVersion": "1",
            }),
        ),
        (
            "unknown deployment strategy id",
            format!("/applications/{application}/environments/{environment}/deployments"),
            serde_json::json!({
                "DeploymentStrategyId": "zzzz9",
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        ),
        (
            "unknown predefined deployment strategy",
            format!("/applications/{application}/environments/{environment}/deployments"),
            serde_json::json!({
                "DeploymentStrategyId": "AppConfig.NoSuchStrategy",
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        ),
    ];

    for (case, uri, body) in &cases {
        let answer = json_call(&router, "POST", uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }

    // No deployment was ever started into this environment, so every number is absent.
    for number in [1, 2, 99] {
        let answer = json_call(
            &router,
            "GET",
            &format!(
                "/applications/{application}/environments/{environment}/deployments/{number}"
            ),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case \"deployment number {number}\": expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

/// A hosted configuration version only exists in AppConfig's own hosted store, and the
/// `Type` a profile declares is one of the two the model names: both rules are refused
/// with the modelled 400 `BadRequestException` rather than quietly accepted.
#[tokio::test]
async fn semantic_rules_the_schema_cannot_express_are_refused_with_bad_request() {
    let router = router();
    let application = create_application(&router, "semantics").await;

    // Rows differ along the source a profile points at: only the hosted sentinel keeps
    // its configuration inside AppConfig.
    let location_cases = [
        ("hosted", "hosted", 201),
        ("s3 object", "s3://amzn-s3-demo-bucket/my-app/config.json", 400),
        ("ssm parameter", "ssm-parameter://my-parameter", 400),
        ("secrets manager", "secretsmanager://my-secret", 400),
        ("code pipeline", "codepipeline://my-pipeline", 400),
    ];

    for (case, location_uri, expected) in location_cases {
        let created = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/configurationprofiles"),
            &with_required_retrieval_role(
                serde_json::json!({ "Name": format!("profile-{case}"), "LocationUri": location_uri }),
            ),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the profile itself is valid, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let profile = created
            .string("Id")
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {}", created.json()));

        let written = send(
            &router,
            "POST",
            &format!(
                "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
            ),
            "application/json",
            br#"{"enabled":true}"#.to_vec(),
            &[],
        )
        .await;
        assert_eq!(
            written.status, expected,
            "case {case:?}: expected {expected} for a hosted write against {location_uri}, got {} / {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        if expected == 400 {
            assert!(
                written.error_code().contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?}",
                written.error_code()
            );
        }
    }

    // `Type` accepts the two values the model's own documentation lists, and nothing
    // else. The rejected rows all satisfy the member's pattern, so only the value check
    // can refuse them.
    let type_cases = [
        ("freeform", "AWS.Freeform", 201),
        ("feature flags", "AWS.AppConfig.FeatureFlags", 201),
        ("lowercase freeform", "aws.freeform", 400),
        ("unknown type", "AWS.Something.Else", 400),
        ("empty-ish type", "x", 400),
    ];

    for (case, profile_type, expected) in type_cases {
        let created = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/configurationprofiles"),
            &serde_json::json!({
                "Name": format!("typed-{case}"),
                "LocationUri": "hosted",
                "Type": profile_type,
            }),
        )
        .await;
        assert_eq!(
            created.status, expected,
            "case {case:?}: expected {expected} for Type {profile_type}, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        if expected == 400 {
            assert!(
                created.error_code().contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?}",
                created.error_code()
            );
        }
    }
}

/// Every store this service grows one request at a time is bounded, and a write past a
/// bound is refused with the quota error the operation models — never accepted
/// silently, so no loop of create calls can grow the emulator's heap without limit.
#[tokio::test]
async fn writes_past_a_bound_are_refused_with_the_modelled_quota_error() {
    // Applications: the one bound AWS documents on this service's own quota error.
    let applications = router();
    for index in 0..state::MAX_APPLICATIONS {
        let answer = json_call(
            &applications,
            "POST",
            "/applications",
            &serde_json::json!({ "Name": format!("app-{index}") }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case \"application {index}\": expected 201 below the quota, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let over = json_call(
        &applications,
        "POST",
        "/applications",
        &serde_json::json!({ "Name": "one-too-many" }),
    )
    .await;
    assert_eq!(
        over.status,
        402,
        "case \"application past the quota\": expected 402, got {} / {}",
        over.status,
        String::from_utf8_lossy(&over.body)
    );
    assert!(
        over.error_code().contains("ServiceQuotaExceededException"),
        "case \"application past the quota\": expected ServiceQuotaExceededException, got {:?}",
        over.error_code()
    );

    // Environments and configuration profiles, each bounded per application.
    let children = router();
    let application = create_application(&children, "bounded").await;
    for (case, uri, bound, body) in [
        (
            "environments",
            format!("/applications/{application}/environments"),
            state::MAX_ENVIRONMENTS_PER_APPLICATION,
            serde_json::json!({ "Name": "child" }),
        ),
        (
            "configuration profiles",
            format!("/applications/{application}/configurationprofiles"),
            state::MAX_CONFIGURATION_PROFILES_PER_APPLICATION,
            serde_json::json!({ "Name": "child", "LocationUri": "hosted" }),
        ),
    ] {
        for index in 0..bound {
            let answer = json_call(&children, "POST", &uri, &body).await;
            assert_eq!(
                answer.status,
                201,
                "case {case:?} #{index}: expected 201 below the bound, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
        let over = json_call(&children, "POST", &uri, &body).await;
        assert_eq!(
            over.status, 402,
            "case {case:?} past the bound: expected 402, got {} / {}",
            over.status,
            String::from_utf8_lossy(&over.body)
        );
        assert!(
            over.error_code().contains("ServiceQuotaExceededException"),
            "case {case:?} past the bound: expected ServiceQuotaExceededException, got {:?}",
            over.error_code()
        );
    }

    // Hosted configuration versions: bounded in count per profile, and in the size of
    // one write.
    let sized = router();
    let application = create_application(&sized, "bounded-content").await;
    let profile = create_profile(&sized, &application, "bounded").await;
    let uri = format!(
        "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
    );

    // Rows walk the per-write size bound across its boundary: at the limit, and one
    // byte past it.
    for (case, length, expected) in [
        ("one byte", 1, 201),
        ("exactly the limit", state::MAX_HOSTED_CONFIGURATION_BYTES, 201),
        (
            "one byte past the limit",
            state::MAX_HOSTED_CONFIGURATION_BYTES + 1,
            413,
        ),
    ] {
        let answer = send(
            &sized,
            "POST",
            &uri,
            "application/octet-stream",
            vec![b'c'; length],
            &[],
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected} for {length} bytes, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected == 413 {
            assert!(
                answer.error_code().contains("PayloadTooLargeException"),
                "case {case:?}: expected PayloadTooLargeException, got {:?}",
                answer.error_code()
            );
        }
    }

    // A fresh profile, so the count below starts from no versions at all.
    let counted = router();
    let application = create_application(&counted, "bounded-count").await;
    let profile = create_profile(&counted, &application, "bounded").await;
    let uri = format!(
        "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
    );
    for index in 0..state::MAX_HOSTED_CONFIGURATION_VERSIONS_PER_PROFILE {
        let answer = send(
            &counted,
            "POST",
            &uri,
            "application/json",
            // Content of its own per write: bytes identical to the profile's newest
            // version reuse it instead of filling another slot of the bound.
            format!(r#"{{"enabled":true,"version":{index}}}"#).into_bytes(),
            &[],
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case \"hosted version {index}\": expected 201 below the bound, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let over = send(
        &counted,
        "POST",
        &uri,
        "application/json",
        br#"{"enabled":true,"version":"past the bound"}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        over.status,
        402,
        "case \"hosted version past the bound\": expected 402, got {} / {}",
        over.status,
        String::from_utf8_lossy(&over.body)
    );
    assert!(
        over.error_code().contains("ServiceQuotaExceededException"),
        "case \"hosted version past the bound\": expected ServiceQuotaExceededException, got {:?}",
        over.error_code()
    );
}

/// Deployment numbers increase by one per environment, independently of other
/// environments, and a deployment that is already finished does not block the next one.
#[tokio::test]
async fn deployment_numbers_increase_per_environment() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let application = create_application(&router, "numbering").await;
    let profile = create_profile(&router, &application, "config").await;
    // The version every deployment below names: a deployment resolves its
    // `ConfigurationVersion` against the versions the profile holds.
    host_version(
        &router,
        &application,
        &profile,
        "numbering",
        br#"{"enabled":true}"#,
        None,
    )
    .await;

    // A strategy with no deployment duration and no final bake time, so each of its
    // deployments is finished the moment it is started and this suite can deploy into the
    // same environment again without waiting out a predefined strategy's minutes. Seeded
    // through the same bounded entry point `CreateDeploymentStrategy` writes through, so
    // this suite exercises the deployment numbering rather than that operation.
    let immediate = "imm0001";
    assert!(
        state::service_state(&*engine)
            .expect("the test engine must carry this service's state")
            .lock()
            .try_insert_strategy(DeploymentStrategy {
                id: immediate.to_string(),
                name: "immediate".to_string(),
                description: None,
                deployment_duration_in_minutes: 0,
                growth_type: state::GROWTH_LINEAR.to_string(),
                growth_factor: 100.0,
                final_bake_time_in_minutes: 0,
                replicate_to: state::REPLICATE_TO_NONE.to_string(),
                tags: std::collections::HashMap::new(),
            }),
        "case \"seed the immediate strategy\": the first created strategy is inside the bound"
    );

    // Two environments: numbering runs 1, 2, 3 in the first and restarts at 1 in the
    // second, so it is shown to be per environment rather than per service.
    let first = create_environment(&router, &application, "first").await;
    let second = create_environment(&router, &application, "second").await;

    for expected_number in 1..=3 {
        let case = format!("first environment, deployment {expected_number}");
        let started = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments/{first}/deployments"),
            &serde_json::json!({
                "DeploymentStrategyId": immediate,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        assert_eq!(
            started
                .json()
                .get("DeploymentNumber")
                .and_then(serde_json::Value::as_i64),
            Some(i64::from(expected_number)),
            "case {case:?}: deployment numbers run 1, 2, 3 per environment, got {}",
            started.json()
        );
        assert_eq!(
            started.string("EnvironmentId").as_deref(),
            Some(first.as_str()),
            "case {case:?}: expected the environment deployed into, got {:?}",
            started.string("EnvironmentId")
        );

        // A strategy with no rollout window and no bake time paces no stage: AppConfig has
        // nothing to roll out gradually and nothing to monitor, so the start call itself
        // answers a COMPLETE deployment — and the next deployment into the same environment
        // is not blocked, with no caller asking for anything and no test sleeping.
        assert_eq!(
            started.string("State").as_deref(),
            Some("COMPLETE"),
            "case {case:?}: a deployment whose strategy paces no stage is over when the \
             start answers, got {}",
            started.json()
        );

        let finished = deployment_in_state(
            &router,
            &case,
            &application,
            &first,
            i64::from(expected_number),
            &["COMPLETE"],
        )
        .await;
        assert_eq!(
            finished.get("State").and_then(serde_json::Value::as_str),
            Some("COMPLETE"),
            "case {case:?}: expected the deployment to have completed, got {finished}"
        );
        assert_eq!(
            finished
                .get("PercentageComplete")
                .and_then(serde_json::Value::as_f64),
            Some(100.0),
            "case {case:?}: a completed deployment reached every target, got {finished}"
        );
        assert!(
            finished.get("CompletedAt").is_some(),
            "case {case:?}: a completed deployment reports CompletedAt, got {finished}"
        );

        // And the environment is ready for the next deployment again.
        let environment_state = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{first}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            environment_state.string("State").as_deref(),
            Some(state::ENVIRONMENT_READY_FOR_DEPLOYMENT),
            "case {case:?}: an environment with nothing in flight is ready, got {:?}",
            environment_state.string("State")
        );
    }

    // The second environment has had nothing deployed into it, so its own numbering
    // starts at 1 however many deployments the first environment took.
    let other = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{second}/deployments"),
        &serde_json::json!({
            "DeploymentStrategyId": immediate,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        other.status,
        201,
        "case \"second environment\": expected 201, got {} / {}",
        other.status,
        String::from_utf8_lossy(&other.body)
    );
    assert_eq!(
        other
            .json()
            .get("DeploymentNumber")
            .and_then(serde_json::Value::as_i64),
        Some(1),
        "case \"second environment\": deployment numbers are per environment, got {}",
        other.json()
    );
}

/// The created-deployment-strategy map is bounded like every other store this service
/// grows per request, and the bound cannot be reached around: `try_insert_strategy` is
/// the only way in.
#[test]
fn created_deployment_strategies_are_bounded() {
    let mut store = ComAmazonawsAppconfigAmazonappconfigState::default();

    let strategy = |id: String| DeploymentStrategy {
        id,
        name: "bounded".to_string(),
        description: None,
        deployment_duration_in_minutes: 1,
        growth_type: state::GROWTH_LINEAR.to_string(),
        growth_factor: 50.0,
        final_bake_time_in_minutes: 0,
        replicate_to: state::REPLICATE_TO_NONE.to_string(),
        tags: std::collections::HashMap::new(),
    };

    for index in 0..state::MAX_DEPLOYMENT_STRATEGIES {
        assert!(
            store.try_insert_strategy(strategy(format!("str{index:04}"))),
            "case \"strategy {index}\": expected a strategy inside the bound to be stored"
        );
    }
    assert!(
        !store.try_insert_strategy(strategy("overflow".to_string())),
        "case \"one past the bound\": expected the write to be refused"
    );
    // Replacing one that already exists is not growth, so it is still allowed.
    assert!(
        store.try_insert_strategy(strategy("str0000".to_string())),
        "case \"replace an existing strategy\": expected the write to be allowed"
    );
    assert_eq!(
        store.created_strategies().count(),
        state::MAX_DEPLOYMENT_STRATEGIES,
        "case \"after the refusal\": expected exactly {} strategies, got {}",
        state::MAX_DEPLOYMENT_STRATEGIES,
        store.created_strategies().count()
    );
    // And freeing a slot lets the next one in, the way DeleteDeploymentStrategy will.
    assert!(
        store.remove_strategy("str0000").is_some(),
        "case \"remove a strategy\": expected the stored strategy back"
    );
    assert!(
        store.try_insert_strategy(strategy("overflow".to_string())),
        "case \"after a removal\": expected the freed slot to accept a strategy"
    );
}

/// What `StartDeployment` persists about a deployment, for the schedule tests that
/// drive `state::Deployment` directly.
pub(crate) fn new_deployment() -> NewDeployment {
    NewDeployment {
        deployment_number: 1,
        application_id: "abcd123".to_string(),
        environment_id: "efgh456".to_string(),
        configuration_profile_id: "ijkl789".to_string(),
        configuration_name: "config".to_string(),
        configuration_location_uri: "hosted".to_string(),
        configuration_version: "1".to_string(),
        description: None,
        kms_key_identifier: None,
        version_label: None,
        // The schedule tests drive one deployment's clock, not the association store.
        applied_extensions: Vec::new(),
        tags: std::collections::HashMap::new(),
    }
}

/// A deployment's schedule terminates and stays inside the modelled percentage range
/// for ANY growth factor a stored strategy can carry — including values the model's
/// `Percentage` shape does not allow, which reach the store through
/// `CreateDeploymentStrategy` and which the JSON protocols can spell (`NaN`,
/// `Infinity`). A schedule that looped on such a value would hang the handler and grow
/// the event log without bound, and every later read of that environment would re-enter
/// it.
#[test]
fn a_deployment_schedule_terminates_for_any_stored_growth_factor() {
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Rows differ along the growth factor's kind: non-finite, out of range on each
    // side, at the modelled bounds, one past them, and fractional — for both growth
    // types.
    let cases: [(&str, f32, &str); 12] = [
        ("NaN, linear", f32::NAN, state::GROWTH_LINEAR),
        ("NaN, exponential", f32::NAN, state::GROWTH_EXPONENTIAL),
        ("infinity", f32::INFINITY, state::GROWTH_LINEAR),
        ("negative infinity", f32::NEG_INFINITY, state::GROWTH_LINEAR),
        ("zero", 0.0, state::GROWTH_LINEAR),
        ("negative", -25.0, state::GROWTH_LINEAR),
        ("the modelled minimum", 1.0, state::GROWTH_LINEAR),
        ("the modelled minimum, exponential", 1.0, state::GROWTH_EXPONENTIAL),
        ("just under the modelled minimum", 0.5, state::GROWTH_LINEAR),
        ("the modelled maximum", 100.0, state::GROWTH_LINEAR),
        ("past the modelled maximum", 1000.0, state::GROWTH_LINEAR),
        ("a fractional factor", 33.3, state::GROWTH_LINEAR),
    ];

    // 100 growth steps at the model's smallest percentage, plus the start, bake and
    // completion events.
    const MAX_EVENTS: usize = 103;

    for (case, growth_factor, growth_type) in cases {
        let strategy = DeploymentStrategy {
            id: "sched01".to_string(),
            name: "schedule".to_string(),
            description: None,
            deployment_duration_in_minutes: 10,
            growth_type: growth_type.to_string(),
            growth_factor,
            final_bake_time_in_minutes: 5,
            replicate_to: state::REPLICATE_TO_NONE.to_string(),
            tags: std::collections::HashMap::new(),
        };

        // Reaching this line at all is half the assertion: a schedule that did not
        // terminate would hang here rather than fail.
        let mut deployment = Deployment::start(new_deployment(), &strategy, started_at);
        for elapsed_minutes in [0, 1, 5, 10, 14, 15, 60] {
            deployment.materialize(started_at + Duration::from_secs(60 * elapsed_minutes));
            let reached = deployment.percentage_complete;
            assert!(
                reached.is_finite() && (0.0..=100.0).contains(&reached),
                "case {case:?} at {elapsed_minutes} minutes: expected a percentage inside the \
                 0..=100 range the modelled Percentage caps, got {reached}"
            );
            assert!(
                deployment.event_log.len() <= MAX_EVENTS,
                "case {case:?} at {elapsed_minutes} minutes: expected at most {MAX_EVENTS} \
                 events, got {}",
                deployment.event_log.len()
            );
        }
        assert_eq!(
            deployment.state,
            state::DEPLOYMENT_COMPLETE,
            "case {case:?}: expected COMPLETE once the rollout and bake time elapsed, got {:?}",
            deployment.state
        );
    }
}

/// A deployment's progress is owed by the CLOCK: for every predefined strategy, the
/// percentage of targets reached, the bake period and the completion fall due at
/// instants fixed when the deployment started, and `materialize` reports exactly the
/// ones that have passed.
///
/// Asserted against the state record rather than over HTTP so the whole schedule — up
/// to half an hour of deployment — can be checked without any test sleeping.
#[test]
fn a_deployments_progress_follows_the_strategys_documented_schedule() {
    let store = ComAmazonawsAppconfigAmazonappconfigState::default();
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    for predefined in PREDEFINED_DEPLOYMENT_STRATEGIES {
        let case = predefined.id;
        let strategy = store
            .resolve_strategy(predefined.id)
            .unwrap_or_else(|| panic!("case {case:?}: a predefined strategy must resolve"));
        let new = new_deployment();

        // Each growth interval of the rollout is paced by one span here
        // (`state::STAGE_DURATION`), and the final bake keeps the strategy's declared
        // minutes (`state::bake_duration`), so the schedule is walked in those spans
        // rather than in the rollout's declared minutes.
        let at_start = Deployment::start(new.clone(), &strategy, started_at);
        let rollout = at_start.rollout_duration();
        let bake = state::bake_duration(predefined.final_bake_time_in_minutes);

        // At the start instant: nothing has rolled out yet, whatever window the strategy
        // declares — reaching the targets is a transition of its own.
        assert_eq!(
            at_start.state,
            state::DEPLOYMENT_DEPLOYING,
            "case {case:?}: a deployment starts rolling out, got {:?}",
            at_start.state
        );
        assert_eq!(
            at_start.percentage_complete, 0.0,
            "case {case:?}: expected 0% of targets at the start instant, got {}",
            at_start.percentage_complete
        );
        assert!(
            at_start.completed_at.is_none(),
            "case {case:?}: a deployment in progress has no CompletedAt, got {:?}",
            at_start.completed_at
        );

        // Once the rollout window has elapsed: every target reached, and baking when
        // the strategy has a bake time.
        let mut after_rollout = Deployment::start(new.clone(), &strategy, started_at);
        after_rollout.materialize(started_at + rollout + Duration::from_secs(1));
        assert!(
            after_rollout.percentage_complete >= 100.0,
            "case {case:?}: the rollout window ends at 100% of targets, got {}",
            after_rollout.percentage_complete
        );
        let expected_after_rollout = if bake.is_zero() {
            state::DEPLOYMENT_COMPLETE
        } else {
            state::DEPLOYMENT_BAKING
        };
        assert_eq!(
            after_rollout.state, expected_after_rollout,
            "case {case:?}: expected {expected_after_rollout} after the rollout, got {:?}",
            after_rollout.state
        );

        // Once the bake time has elapsed too: complete, with a completion instant and a
        // completion event.
        let mut after_bake = Deployment::start(new.clone(), &strategy, started_at);
        after_bake.materialize(started_at + rollout + bake + Duration::from_secs(1));
        assert_eq!(
            after_bake.state, state::DEPLOYMENT_COMPLETE,
            "case {case:?}: expected COMPLETE once the rollout and bake time elapsed, got {:?}",
            after_bake.state
        );
        assert!(
            after_bake.percentage_complete >= 100.0,
            "case {case:?}: a completed deployment reached every target, got {}",
            after_bake.percentage_complete
        );
        assert!(
            after_bake.completed_at.is_some(),
            "case {case:?}: a completed deployment has a CompletedAt, got {:?}",
            after_bake.completed_at
        );
        assert!(
            !after_bake.is_in_flight(),
            "case {case:?}: a completed deployment is no longer in flight"
        );

        // The event log is the whole history, newest first, and it ends with the start.
        let log = after_bake.event_log_newest_first();
        let types: Vec<&str> = log
            .iter()
            .filter_map(|event| event.event_type.as_deref())
            .collect();
        assert_eq!(
            types.first().copied(),
            Some(state::EVENT_DEPLOYMENT_COMPLETED),
            "case {case:?}: the newest event of a completed deployment is its completion, got {types:?}"
        );
        assert_eq!(
            types.last().copied(),
            Some(state::EVENT_DEPLOYMENT_STARTED),
            "case {case:?}: the oldest event is the deployment's start, got {types:?}"
        );
        if !bake.is_zero() {
            assert!(
                types.contains(&state::EVENT_BAKE_TIME_STARTED),
                "case {case:?}: a strategy with a bake time logs BAKE_TIME_STARTED, got {types:?}"
            );
        }

        // Materialising again changes nothing: the clock owes no more than it owed.
        let before = after_bake.clone();
        after_bake.materialize(started_at + rollout + bake + Duration::from_secs(600));
        assert_eq!(
            after_bake.event_log.len(),
            before.event_log.len(),
            "case {case:?}: a second read must not append an event the clock did not owe"
        );
        assert_eq!(
            after_bake.state, before.state,
            "case {case:?}: a second read must not move a completed deployment"
        );

        // Halfway through the rollout, a rollout made of more than one growth interval has
        // reached part of its targets and no more; one made of a single interval has
        // reached none of them yet, because that interval has not elapsed.
        let intervals = rollout.as_secs_f64() / at_start.growth_interval().as_secs_f64();
        let floor_at_halfway = if intervals >= 2.0 {
            predefined.growth_factor
        } else {
            0.0
        };
        let mut halfway = Deployment::start(new.clone(), &strategy, started_at);
        halfway.materialize(started_at + rollout / 2);
        assert!(
            halfway.percentage_complete >= floor_at_halfway
                && halfway.percentage_complete <= 100.0,
            "case {case:?}: halfway through the rollout expected between {floor_at_halfway} \
             and 100, got {}",
            halfway.percentage_complete
        );
        assert!(
            halfway.percentage_complete >= at_start.percentage_complete,
            "case {case:?}: the percentage of targets reached never decreases, got {} then {}",
            at_start.percentage_complete,
            halfway.percentage_complete
        );
    }
}
