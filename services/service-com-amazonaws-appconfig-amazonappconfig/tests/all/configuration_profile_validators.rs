// SPDX-License-Identifier: CC0-1.0

//! A configuration profile's validators GATE the calls that put a configuration version
//! into service: they are not metadata that round-trips.
//!
//! The rule under test, which every table below is one slice of: "a validator provides a
//! syntactic or semantic check to ensure the configuration that you want to deploy
//! functions as intended. To validate your application configuration data, you provide a
//! schema or an Amazon Web Services Lambda function that runs against the configuration.
//! The configuration deployment or update can only proceed when the configuration data is
//! valid" (`smithy_query(query="doc",shape="Validator")`). So every operation AppConfig
//! documents as validating a configuration runs EVERY validator the profile carries
//! against the candidate version's content, and a validator that reports the content
//! non-compliant refuses that call with `BadRequestException` / HTTP 400, leaving no
//! deployment record behind and consuming no deployment number — while a validator that
//! produces NO verdict vetoes nothing, because a refusal reported for it would be a verdict
//! no validator gave.
//!
//! A `JSON_SCHEMA` validator's verdict is the subject of `json_schema_validator_gate`; here
//! it appears as one entry beside a `LAMBDA` one, to pin that the list is run whatever the
//! types in it are.
//!
//! The suite mounts AWS Lambda beside AppConfig on one engine, because a `LAMBDA`
//! validator's verdict comes from a function that lives in another service and is invoked
//! over the in-process callback.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, create_environment, json_call, send, Answer};

/// The Region every request below is signed for, which is also the Region of the function
/// a `LAMBDA` validator names.
const REGION: &str = "us-east-1";

/// AppConfig and Lambda on ONE engine: a `LAMBDA` validator's function lives in the other
/// service, and the verdict is whatever that service answers for the invocation.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One `restJson1` request against the AWS Lambda emulator; the credential scope is what
/// routes it there rather than to AppConfig.
///
/// `pub(crate)` because every suite here whose fixture is a FUNCTION — a `LAMBDA`
/// validator's, and an extension action's steering target
/// (`crate::deployment_tick_directive`) — reaches Lambda through this one door.
pub(crate) async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/lambda/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the test request body must serialize"),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// A function this emulator holds, in `Active` state: created through `CreateFunction`,
/// with the activation the clock owes brought forward so no test waits for it. Answers its
/// ARN.
async fn given_active_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> String {
    let created = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{}:role/validator", engine::emulator_account_id()),
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": "" },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateFunction answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    activate_functions(engine);
    created
        .string("FunctionArn")
        .unwrap_or_else(|| panic!("case {name:?}: CreateFunction answered no FunctionArn"))
}

/// Bring every function's activation forward: the transition into `Active` is owed by
/// Lambda's OWN clock, so no suite here sleeps on its behalf.
///
/// `pub(crate)` for the same reason [`lambda`] is.
pub(crate) fn activate_functions(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// The ARN of a function nothing created, which Lambda itself refuses the invocation of —
/// the verdict AppConfig relays as the validator's.
fn missing_function(name: &str) -> String {
    format!(
        "arn:aws:lambda:{REGION}:{}:function:{name}",
        engine::emulator_account_id()
    )
}

/// One entry of a profile's `Validators` list.
pub(crate) fn validator(validator_type: &str, content: &str) -> Value {
    json!({ "Type": validator_type, "Content": content })
}

/// A hosted configuration profile carrying `validators`. Answers its minted id.
async fn given_profile(
    router: &MultiServiceRouter,
    application: &str,
    name: &str,
    validators: &[Value],
) -> String {
    let created = json_call(
        router,
        "POST",
        &format!("/applications/{application}/configurationprofiles"),
        &json!({ "Name": name, "LocationUri": "hosted", "Validators": validators }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateConfigurationProfile answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// Write one hosted configuration version and answer the version number it was given.
///
/// Asserted to be accepted: `CreateHostedConfigurationVersion` stores the bytes it is
/// given, so a version a validator would reject is stored verbatim and only the calls that
/// put it into service are gated.
async fn given_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    content: &[u8],
) -> String {
    let created = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: a hosted version must be stored whatever a validator thinks of it, \
         got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .unwrap_or_else(|| panic!("case {case:?}: no Version-Number was answered"))
}

/// A deployment strategy with nothing to wait for, so one environment can take a second
/// deployment without the first still being in flight. Answers its id.
async fn given_strategy(router: &MultiServiceRouter, name: &str) -> String {
    let created = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": 0,
            "FinalBakeTimeInMinutes": 0,
            "GrowthFactor": 100.0,
            "GrowthType": "LINEAR",
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateDeploymentStrategy answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: the strategy answered no Id"))
}

/// `StartDeployment` of one configuration version.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    version: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
        }),
    )
    .await
}

/// The deployment numbers one environment's history holds.
async fn deployment_numbers(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Vec<i64> {
    let listed = json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await;
    listed.json()["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["DeploymentNumber"].as_i64())
        .collect()
}

/// `ValidateConfiguration` of one configuration version.
async fn validate_configuration(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/validators\
             ?configuration_version={version}"
        ),
        &Value::Null,
    )
    .await
}

/// Assert `answer` is the modelled 400 refusal of a call a validator did not pass.
pub(crate) fn assert_bad_request(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected the modelled 400 refusal, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code().split('#').next_back(),
        Some("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {}",
        answer.error_code()
    );
}

/// Assert `answer` is the modelled refusal of a call a validator did not pass, and that
/// its `Message` is derived from the request rather than from any one recorded run.
pub(crate) fn assert_refused(case: &str, answer: &Answer, profile: &str, version: &str, kind: &str) {
    assert_bad_request(case, answer);
    let message = answer.string("Message").unwrap_or_default();
    let expected = format!(
        "Configuration version '{version}' of configuration profile '{profile}' did not pass \
         the {kind} validator: "
    );
    assert!(
        message.starts_with(&expected),
        "case {case:?}: expected a refusal opening {expected:?}, got {message:?}"
    );
}

/// The configuration documents every table below runs its validators against.
///
/// Rows differ along what a `LAMBDA` gate must be indifferent to: the document's size, its
/// shape, its media type and whether it is JSON at all. A validator's verdict is about the
/// content, so the gate has to hold for every form content takes. The last column says
/// whether the document is a JSON OBJECT, which is what the `JSON_SCHEMA` entry below is
/// asked about.
pub(crate) fn contents() -> Vec<(&'static str, &'static [u8], bool)> {
    vec![
        ("a small JSON object", br#"{"allow_caching":true}"#, true),
        ("a JSON array", br#"["alpha","beta"]"#, false),
        (
            "a nested JSON object",
            br#"{"db":{"host":"h","port":5432}}"#,
            true,
        ),
        ("a JSON scalar", b"42", false),
        ("text that is not JSON", b"allow_caching = true", false),
        ("an empty document", b" ", false),
    ]
}

/// A deployment of a version a `LAMBDA` validator rejects is refused, leaves the
/// environment's history and its deployment number untouched, and does not stop the next
/// deployment from being the environment's first.
///
/// The verdict here is the one the invocation produces: the function the validator names
/// was never created, so Lambda itself refuses the invocation, which is the failed
/// validator AppConfig relays. The rows differ along the content the validator ran against,
/// which the gate must be indifferent to.
#[tokio::test]
async fn a_lambda_validator_that_rejects_refuses_the_deployment_and_leaves_nothing_behind() {
    let (router, _engine) = emulator();
    let application = create_application(&router, "lambda-gated").await;
    let strategy = given_strategy(&router, "lambda-gate-instant").await;
    let rejecting = missing_function("nobody-created-me");

    for (index, (case, content, _is_json_object)) in contents().into_iter().enumerate() {
        // One environment per row, so each row's deployment numbering starts at the same
        // place and the refusal's effect on it is readable. Resources are named by the
        // row's index, because `Name` is `length 1..64`.
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        let gated = given_profile(
            &router,
            &application,
            &format!("gated-{index}"),
            &[validator("LAMBDA", &rejecting)],
        )
        .await;
        let version = given_version(&router, &application, &gated, case, content).await;

        let refused = start_deployment(
            &router,
            &application,
            &environment,
            &gated,
            &strategy,
            &version,
        )
        .await;
        assert_refused(case, &refused, &gated, &version, "LAMBDA");
        assert!(
            deployment_numbers(&router, &application, &environment)
                .await
                .is_empty(),
            "case {case:?}: a refused deployment leaves no record, got {:?}",
            deployment_numbers(&router, &application, &environment).await
        );

        // The deployment number the refusal did not consume: the first deployment this
        // environment accepts is still number 1, from a profile nothing gates.
        let ungated = given_profile(
            &router,
            &application,
            &format!("ungated-{index}"),
            &[],
        )
        .await;
        let ungated_version = given_version(&router, &application, &ungated, case, content).await;
        let accepted = start_deployment(
            &router,
            &application,
            &environment,
            &ungated,
            &strategy,
            &ungated_version,
        )
        .await;
        assert_eq!(
            accepted.status,
            201,
            "case {case:?}: nothing gates this profile, so the deployment must proceed, got {} / {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        assert_eq!(
            accepted.json()["DeploymentNumber"].as_i64(),
            Some(1),
            "case {case:?}: the refusal must consume no deployment number, got {}",
            accepted.json()
        );
        assert_eq!(
            deployment_numbers(&router, &application, &environment).await,
            vec![1],
            "case {case:?}: only the accepted deployment belongs in the history"
        );
    }
}

/// Every validator the profile carries is run, and the first one that rejects the content
/// decides — wherever it sits in the list.
///
/// `ValidatorList` is `length 0..2`, so a profile carries at most two validators; the rows
/// differ along which position rejects and along what sits beside it.
#[tokio::test]
async fn every_validator_in_the_list_is_run_and_the_first_rejection_decides() {
    let (router, engine) = emulator();
    let application = create_application(&router, "two-validators").await;
    let strategy = given_strategy(&router, "two-validators-instant").await;
    let rejecting = validator("LAMBDA", &missing_function("nobody-created-me"));
    // A function this emulator holds and cannot run yields no verdict, so it neither
    // rejects nor shadows a validator beside it.
    let silent = validator(
        "LAMBDA",
        &given_active_function(&router, &engine, "silent-validator").await,
    );
    // The content every row below deploys is a JSON object, which this schema allows, so
    // this entry is run and reports nothing — its own verdicts are pinned by
    // `json_schema_validator_gate`.
    let schema = validator("JSON_SCHEMA", r#"{"type":"object"}"#);

    let rows = [
        ("the only validator rejects", vec![rejecting.clone()], true),
        (
            "the first of two rejects",
            vec![rejecting.clone(), silent.clone()],
            true,
        ),
        (
            "the second of two rejects",
            vec![silent.clone(), rejecting.clone()],
            true,
        ),
        (
            "a schema validator the content satisfies sits before the one that rejects",
            vec![schema.clone(), rejecting.clone()],
            true,
        ),
        (
            "neither validator reports the content non-compliant",
            vec![schema.clone(), silent.clone()],
            false,
        ),
    ];

    for (index, (case, validators, rejects)) in rows.into_iter().enumerate() {
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        let profile =
            given_profile(&router, &application, &format!("profile-{index}"), &validators).await;
        let version = given_version(&router, &application, &profile, case, br#"{"v":1}"#).await;
        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        if rejects {
            assert_refused(case, &answer, &profile, &version, "LAMBDA");
            assert!(
                deployment_numbers(&router, &application, &environment)
                    .await
                    .is_empty(),
                "case {case:?}: a refused deployment leaves no record"
            );
        } else {
            assert_eq!(
                answer.status,
                201,
                "case {case:?}: no verdict was produced, so nothing may veto the deployment, \
                 got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                deployment_numbers(&router, &application, &environment).await,
                vec![1],
                "case {case:?}: the deployment that proceeded belongs in the history"
            );
        }
    }
}

/// A `LAMBDA` validator rejects on the verdict the invocation PRODUCED, and on nothing
/// else: an invocation the Lambda service refuses is a rejection, and an invocation this
/// emulator cannot MAKE is no verdict at all, so it vetoes nothing.
///
/// The rows differ along what the validator's `Content` names: a function this emulator does
/// not hold, a `Content` that names no function at all, and a function it holds and cannot
/// run. The last is the second kind: running a deployment package is not implemented
/// (`service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`, filed as
/// `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`),
/// so nothing is known about what the function would have reported — and a refusal reported
/// for it would be a verdict no validator gave.
#[tokio::test]
async fn a_lambda_validator_rejects_only_on_a_verdict_the_invocation_produced() {
    let (router, engine) = emulator();
    let application = create_application(&router, "lambda-verdicts").await;
    let strategy = given_strategy(&router, "lambda-verdicts-instant").await;
    let active = given_active_function(&router, &engine, "configuration-validator").await;

    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a function nothing created",
            missing_function("nobody-created-me"),
            true,
        ),
        (
            "a Content naming no function at all",
            "not a function reference".to_string(),
            true,
        ),
        ("a function this emulator cannot run", active, false),
    ];

    for (index, (case, content, rejects)) in rows.into_iter().enumerate() {
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        let profile = given_profile(
            &router,
            &application,
            &format!("profile-{index}"),
            &[validator("LAMBDA", &content)],
        )
        .await;
        let version = given_version(&router, &application, &profile, case, br#"{"v":1}"#).await;
        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        if rejects {
            assert_refused(case, &answer, &profile, &version, "LAMBDA");
            assert!(
                deployment_numbers(&router, &application, &environment)
                    .await
                    .is_empty(),
                "case {case:?}: a refused deployment leaves no record"
            );
        } else {
            assert_eq!(
                answer.status,
                201,
                "case {case:?}: no verdict was produced, so nothing may veto the deployment, \
                 got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                deployment_numbers(&router, &application, &environment).await,
                vec![1],
                "case {case:?}: the deployment that proceeded belongs in the history"
            );
        }
    }
}

/// `ValidateConfiguration` "uses the validators in a configuration profile to validate a
/// configuration": it answers the modelled empty 204 when no validator reports the content
/// non-compliant, the 400 refusal when one does, 404 for a version the profile does not
/// hold — and it changes nothing either way, as a report of a verdict must not.
#[tokio::test]
async fn validate_configuration_reports_the_verdict_and_changes_nothing() {
    let (router, engine) = emulator();
    let application = create_application(&router, "validated").await;
    let environment = create_environment(&router, &application, "validated-env").await;
    let strategy = given_strategy(&router, "validated-instant").await;
    let silent = given_active_function(&router, &engine, "silent-validator").await;

    // Rows differ along the validators the profile carries — none, one that gives no
    // verdict, one whose verdict is about the content, and one that rejects — and the
    // content they run against. `Verdict` says what the row's validators report for one
    // content document.
    enum Verdict {
        /// Nothing reports the content non-compliant, whatever it is.
        Silent,
        /// The `LAMBDA` validator's invocation is refused, whatever the content is.
        Lambda,
        /// The `JSON_SCHEMA` validator allows a JSON object and nothing else, so the
        /// verdict follows the content.
        JsonObjectOnly,
    }

    let rows: Vec<(&str, Vec<Value>, Verdict)> = vec![
        ("no validators", Vec::new(), Verdict::Silent),
        (
            "a schema validator allowing only an object",
            vec![validator("JSON_SCHEMA", r#"{"type":"object"}"#)],
            Verdict::JsonObjectOnly,
        ),
        (
            "a lambda validator this emulator cannot run",
            vec![validator("LAMBDA", &silent)],
            Verdict::Silent,
        ),
        (
            "a lambda validator whose invocation is refused",
            vec![validator("LAMBDA", &missing_function("nobody-created-me"))],
            Verdict::Lambda,
        ),
    ];

    for (index, (case, validators, verdict)) in rows.into_iter().enumerate() {
        let profile =
            given_profile(&router, &application, &format!("checked-{index}"), &validators).await;
        for (content_case, content, is_json_object) in contents() {
            let case = format!("{case}, {content_case}");
            let version = given_version(&router, &application, &profile, &case, content).await;
            let answer = validate_configuration(&router, &application, &profile, &version).await;
            let refusing_kind = match verdict {
                Verdict::Silent => None,
                Verdict::Lambda => Some("LAMBDA"),
                Verdict::JsonObjectOnly => (!is_json_object).then_some("JSON_SCHEMA"),
            };
            match refusing_kind {
                Some("LAMBDA") => assert_refused(&case, &answer, &profile, &version, "LAMBDA"),
                // The schema's refusal carries AppConfig's own schema-violation template,
                // which `json_schema_validator_gate` pins in full; here only the modelled
                // 400 answer is asserted.
                Some(_) => assert_bad_request(&case, &answer),
                None => {
                    assert_eq!(
                        answer.status,
                        204,
                        "case {case:?}: no validator reported this content non-compliant, so \
                         the modelled 204 is owed, got {} / {}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert!(
                        answer.body.is_empty(),
                        "case {case:?}: the 204 carries no body, got {}",
                        String::from_utf8_lossy(&answer.body)
                    );
                }
            }
        }
        // A version the profile does not hold is not found, whatever its validators are.
        let unknown = validate_configuration(&router, &application, &profile, "9999").await;
        assert_eq!(
            unknown.status,
            404,
            "case {case:?}: a version nothing holds is not found, got {} / {}",
            unknown.status,
            String::from_utf8_lossy(&unknown.body)
        );
    }

    // Nothing above wrote anything: the first deployment this environment accepts is still
    // its first, and it carries the number a fresh environment issues.
    let profile = given_profile(&router, &application, "unchanged", &[]).await;
    let version = given_version(&router, &application, &profile, "unchanged", br#"{"v":1}"#).await;
    let accepted = start_deployment(
        &router,
        &application,
        &environment,
        &profile,
        &strategy,
        &version,
    )
    .await;
    assert_eq!(
        accepted.json()["DeploymentNumber"].as_i64(),
        Some(1),
        "validating a configuration consumes no deployment number, got {}",
        accepted.json()
    );
    assert_eq!(
        deployment_numbers(&router, &application, &environment).await,
        vec![1],
        "validating a configuration writes no deployment record"
    );
}
