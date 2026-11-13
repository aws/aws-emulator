// SPDX-License-Identifier: CC0-1.0

//! Extension action points FIRE: an extension association is not metadata, it makes
//! AppConfig invoke the action's target at the point of its own workflow the action is
//! bound to, hand it the documented event, and act on what comes back.
//!
//! The rule under test, which every table below is one slice of: when a call (or the
//! deployment that call started) reaches an action point, every action bound there by an
//! extension associated to the application, the environment or the configuration profile
//! the workflow is running on is invoked at its `Uri`, with a JSON event naming the
//! action point in PascalCase (`Type`), the resources the workflow is running on, the
//! configuration content base64-encoded, the `ContentVersion` as a string, the
//! association's parameter values and a system-generated `InvocationId`; an action point
//! AppConfig consults BEFORE performing the call gates it — a target that does not answer
//! refuses the call and leaves nothing behind.
//!
//! The suite mounts Amazon SQS and AWS Lambda beside AppConfig on one engine, because an
//! invocation is asserted where a client sees it: on the queue the action's `Uri` names,
//! and — for the gate — against the function it names. A queue is the target kind whose
//! event is easiest to read back; it is also one of the four kinds the `Uri` member
//! documents, and one that cannot ANSWER, so a `Content` transform and a `ROLL_BACK`
//! directive (which only a Lambda target can give) are asserted where they are decided,
//! in `crate::state::extension_actions`' own unit tables.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, send, Answer,
};
use crate::extensions::create_extension;
use crate::resource_tags::{arn, pct};

/// The Region every request below is signed for, which is also the Region of the queue a
/// target ARN names.
const REGION: &str = "us-east-1";

/// AppConfig, SQS and Lambda on ONE engine: an extension action's target lives in another
/// service, and the invocation is asserted there.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One `awsJson1_0` RPC against the Amazon SQS emulator; the credential scope is what
/// routes it.
pub(crate) async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/sqs/aws4_request, SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    assert_eq!(
        status,
        200,
        "case {operation:?}: SQS answered {status} / {}",
        String::from_utf8_lossy(&payload)
    );
    if payload.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&payload).unwrap_or_else(|error| {
        panic!(
            "case {operation:?}: SQS answered a body that is not JSON ({error}): {}",
            String::from_utf8_lossy(&payload)
        )
    })
}

/// A queue to observe invocations on; answers `(url, arn)`.
pub(crate) async fn given_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateQueue answered no QueueUrl: {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no QueueArn: {attributes}"))
        .to_string();
    (url, queue_arn)
}

/// Every event waiting on the queue, parsed, oldest first.
pub(crate) async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("an action-point event must be a JSON document ({error}): {body:?}")
            })
        })
        .collect()
}

/// An extension binding one action at each of `action_points`, all of them pointing at
/// `uri`, and declaring the parameters `parameters` names. Answers its `(id, name)`.
async fn given_extension(
    router: &MultiServiceRouter,
    name: &str,
    action_points: &[&str],
    uri: &str,
    parameters: &[&str],
) -> (String, String) {
    let actions: BTreeMap<String, Value> = action_points
        .iter()
        .map(|point| {
            (
                (*point).to_string(),
                json!([{ "Name": format!("{name}-action"), "Uri": uri }]),
            )
        })
        .collect();
    let declared: BTreeMap<String, Value> = parameters
        .iter()
        .map(|parameter| {
            (
                (*parameter).to_string(),
                json!({ "Required": true, "Dynamic": false }),
            )
        })
        .collect();
    let mut body = json!({ "Name": name, "Actions": actions });
    if !declared.is_empty() {
        body["Parameters"] = json!(declared);
    }
    let created = create_extension(router, &body, None).await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateExtension answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    (
        created
            .string("Id")
            .unwrap_or_else(|| panic!("case {name:?}: CreateExtension answered no Id")),
        name.to_string(),
    )
}

/// Associate `extension` to `resource_arn`, supplying `parameters`. Answers the
/// association's id.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    extension: &str,
    resource_arn: &str,
    parameters: &Value,
) -> String {
    let mut body = json!({
        "ExtensionIdentifier": extension,
        "ResourceIdentifier": resource_arn,
    });
    if !parameters.is_null() {
        body["Parameters"] = parameters.clone();
    }
    let created = json_call(router, "POST", "/extensionassociations", &body).await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateExtensionAssociation answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: the association answered no Id"))
}

/// Write a hosted configuration version holding `content` of `content_type`.
pub(crate) async fn create_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content_type: &str,
    content: &[u8],
) -> Answer {
    send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        content_type,
        content.to_vec(),
        &[],
    )
    .await
}

/// Read a hosted configuration version's STORED bytes back.
async fn stored_bytes(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: i32,
) -> Vec<u8> {
    let read = send(
        router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        read.status, 200,
        "GetHostedConfigurationVersion({version}) answered {}",
        read.status
    );
    read.body.to_vec()
}

/// A deployment strategy of this account's own, so a suite can pick the pacing it needs
/// rather than the bake time a predefined strategy carries. Answers its id.
///
/// [`given_strategy`] is the shorthand for one that reaches every target in a single
/// growth step; a suite that needs a rollout to pass THROUGH shares names its growth
/// factor here.
pub(crate) async fn given_growing_strategy(
    router: &MultiServiceRouter,
    name: &str,
    duration_in_minutes: i32,
    final_bake_time_in_minutes: i32,
    growth_factor: f64,
) -> String {
    let created = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": duration_in_minutes,
            "FinalBakeTimeInMinutes": final_bake_time_in_minutes,
            "GrowthFactor": growth_factor,
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

/// The same, reaching every target in one growth step — the pacing most suites need.
pub(crate) async fn given_strategy(
    router: &MultiServiceRouter,
    name: &str,
    duration_in_minutes: i32,
    final_bake_time_in_minutes: i32,
) -> String {
    given_growing_strategy(
        router,
        name,
        duration_in_minutes,
        final_bake_time_in_minutes,
        100.0,
    )
    .await
}

/// `StartDeployment`.
pub(crate) async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        body,
    )
    .await
}

/// `GetDeployment`.
pub(crate) async fn get_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i32,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        &Value::Null,
    )
    .await
}

/// The event's `Content` member, decoded: the standard base64 the member carries, read
/// back through the `base64` crate rather than through the service's own encoder.
fn decoded(text: &str) -> Vec<u8> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text)
        .unwrap_or_else(|error| panic!("{text:?} is not base64 ({error})"))
}

/// Whether `id` satisfies the model's `Id` shape, `^[a-z0-9]{4,7}$`, which
/// `ActionInvocation$InvocationId` targets.
fn is_modelled_invocation_id(id: &str) -> bool {
    (4..=7).contains(&id.chars().count())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// A `PRE_CREATE_HOSTED_CONFIGURATION_VERSION` action is consulted on every write to the
/// profile it is associated to, and is handed the documented event: `Type`
/// `PreCreateHostedConfigurationVersion`, the content base64-encoded whatever bytes it
/// holds, `ContentVersion` as a STRING, and — from the second version on —
/// `PreviousContent` carrying the bytes the previously STORED version holds.
///
/// The rows differ along what the rule generalises over: the content's media type, its
/// size and whether it is text at all, and whether a previous version exists. A target
/// that is a queue cannot answer, so the write is also asserted to stay byte-faithful.
#[tokio::test]
async fn the_pre_create_hosted_configuration_version_action_is_handed_the_documented_event() {
    let router = emulator();
    let (queue_url, queue_arn) = given_queue(&router, "pre-create-observations").await;
    let application = create_application(&router, "pre-create-app").await;
    let profile = create_profile(&router, &application, "pre-create-profile").await;
    let (extension, extension_name) = given_extension(
        &router,
        "pre-create-stamp",
        &["PRE_CREATE_HOSTED_CONFIGURATION_VERSION"],
        &queue_arn,
        &["team"],
    )
    .await;

    // The control: with no association the action point is not consulted at all, so
    // nothing reaches the queue and the bytes are stored exactly as sent.
    let control = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"control":true}"#,
    )
    .await;
    assert_eq!(
        control.status,
        201,
        "case \"no association\": expected 201, got {} / {}",
        control.status,
        String::from_utf8_lossy(&control.body)
    );
    let events = drain(&router, &queue_url).await;
    assert!(
        events.is_empty(),
        "case \"no association\": an unassociated extension must not be invoked, got {events:?}"
    );
    assert_eq!(
        stored_bytes(&router, &application, &profile, 1).await,
        br#"{"control":true}"#.to_vec(),
        "case \"no association\": the stored bytes must be the bytes that were sent"
    );

    associate(
        &router,
        "the configuration profile",
        &extension,
        &arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        )),
        &json!({ "team": "platform" }),
    )
    .await;

    // Rows: the media type, the shape and size of the content, and bytes that are not
    // text at all — each written in turn, so every row but the first also has a previous
    // version behind it.
    let rows: Vec<(&str, &str, Vec<u8>)> = vec![
        (
            "a JSON document",
            "application/json",
            br#"{"colour":"blue"}"#.to_vec(),
        ),
        ("plain text", "text/plain", b"a=1\nb=2\n".to_vec()),
        (
            "bytes that are not text",
            "application/octet-stream",
            vec![0x00, 0x01, 0xfe, 0xff, 0x7f],
        ),
        ("an empty document", "text/plain", Vec::new()),
        (
            "a document larger than one base64 line",
            "application/json",
            format!(r#"{{"padding":"{}"}}"#, "p".repeat(300)).into_bytes(),
        ),
    ];

    // Version 1 above is the control's, so the first row writes version 2.
    let mut previous: Option<(String, i32, Vec<u8>)> =
        Some(("application/json".to_string(), 1, br#"{"control":true}"#.to_vec()));
    for (index, (case, content_type, content)) in rows.iter().enumerate() {
        let version = i32::try_from(index).expect("the row count fits in i32") + 2;
        let created = create_version(&router, &application, &profile, content_type, content).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let events = drain(&router, &queue_url).await;
        assert_eq!(
            events.len(),
            1,
            "case {case:?}: expected exactly one action-point event, got {events:?}"
        );
        let event = &events[0];
        assert_eq!(
            event["Type"],
            json!("PreCreateHostedConfigurationVersion"),
            "case {case:?}: expected the action point's PascalCase Type, got {event}"
        );
        assert_eq!(
            event["ContentVersion"],
            json!(version.to_string()),
            "case {case:?}: expected ContentVersion as a string, got {event}"
        );
        assert_eq!(
            event["ContentType"],
            json!(*content_type),
            "case {case:?}: expected the request's ContentType, got {event}"
        );
        let carried = decoded(event["Content"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: Content must be a base64 string, got {event}")
        }));
        assert_eq!(
            carried, *content,
            "case {case:?}: expected the submitted bytes in Content, got {carried:?}"
        );
        assert_eq!(
            event["Application"],
            json!({ "Id": application, "Name": "pre-create-app" }),
            "case {case:?}: expected the application the write runs on, got {event}"
        );
        assert_eq!(
            event["ConfigurationProfile"],
            json!({ "Id": profile, "Name": "pre-create-profile" }),
            "case {case:?}: expected the profile the write runs on, got {event}"
        );
        assert_eq!(
            event["Parameters"],
            json!({ "team": "platform" }),
            "case {case:?}: expected the association's parameter values, got {event}"
        );
        let invocation_id = event["InvocationId"].as_str().unwrap_or_default();
        assert!(
            is_modelled_invocation_id(invocation_id),
            "case {case:?}: expected a system-generated InvocationId of the modelled shape, \
             got {invocation_id:?}"
        );
        assert!(
            event.get("Environment").is_none(),
            "case {case:?}: a hosted version is written against no environment, got {event}"
        );

        match &previous {
            Some((previous_type, previous_version, previous_content)) => {
                let carried = &event["PreviousContent"];
                assert_eq!(
                    carried["ContentType"],
                    json!(previous_type),
                    "case {case:?}: expected the previously stored ContentType, got {carried}"
                );
                assert_eq!(
                    carried["ContentVersion"],
                    json!(previous_version.to_string()),
                    "case {case:?}: expected the previously stored ContentVersion as a string, \
                     got {carried}"
                );
                let previous_bytes = decoded(carried["Content"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: PreviousContent.Content must be base64, got {carried}")
                }));
                assert_eq!(
                    previous_bytes, *previous_content,
                    "case {case:?}: expected the previously STORED bytes, got {previous_bytes:?}"
                );
            }
            None => assert!(
                event.get("PreviousContent").is_none(),
                "case {case:?}: the first version has no previous content, got {event}"
            ),
        }

        // A queue cannot answer, so nothing replaced what was stored.
        assert_eq!(
            stored_bytes(&router, &application, &profile, version).await,
            *content,
            "case {case:?}: a target that cannot answer must leave the stored bytes alone"
        );
        previous = Some((content_type.to_string(), version, content.clone()));
    }

    // Removing the association stops the dispatch again: nothing is consulted and the
    // bytes stay byte-faithful.
    let associations = json_call(&router, "GET", "/extensionassociations", &Value::Null).await;
    for association in associations.json()["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let id = association["Id"].as_str().unwrap_or_default().to_string();
        let removed = json_call(
            &router,
            "DELETE",
            &format!("/extensionassociations/{}", pct(&id)),
            &Value::Null,
        )
        .await;
        assert_eq!(
            removed.status, 204,
            "case \"association removed\": DeleteExtensionAssociation answered {}",
            removed.status
        );
    }
    let after = create_version(
        &router,
        &application,
        &profile,
        "text/plain",
        b"after removal",
    )
    .await;
    assert_eq!(
        after.status,
        201,
        "case \"association removed\": expected 201, got {} / {}",
        after.status,
        String::from_utf8_lossy(&after.body)
    );
    let events = drain(&router, &queue_url).await;
    assert!(
        events.is_empty(),
        "case \"association removed\": nothing may be invoked any more, got {events:?}"
    );
    assert!(
        !extension_name.is_empty(),
        "the extension's name is what a refusal would name"
    );
}

/// A `PRE_START_DEPLOYMENT` action is consulted on every deployment into the environment
/// it is associated to, through an association bound at ANY of the three levels a
/// deployment runs on, and is handed the deployment's event: the candidate configuration
/// base64-encoded, the deployment numbers, and the dynamic parameter values the call
/// supplied for it.
#[tokio::test]
async fn the_pre_start_deployment_action_is_handed_the_documented_event() {
    // Rows differ along WHICH of the three levels the association is bound to, which is
    // what the binding rule generalises over.
    let levels: [&str; 3] = ["application", "environment", "configurationprofile"];
    for level in levels {
        let router = emulator();
        let (queue_url, queue_arn) = given_queue(&router, "pre-start-observations").await;
        let application = create_application(&router, "pre-start-app").await;
        let environment = create_environment(&router, &application, "pre-start-env").await;
        let profile = create_profile(&router, &application, "pre-start-profile").await;
        for payload in [br#"{"v":1}"#.to_vec(), br#"{"v":2}"#.to_vec()] {
            let created =
                create_version(&router, &application, &profile, "application/json", &payload).await;
            assert_eq!(
                created.status,
                201,
                "case {level:?}: seeding a version answered {}",
                created.status
            );
        }
        let (extension, extension_name) = given_extension(
            &router,
            "pre-start-gate",
            &["PRE_START_DEPLOYMENT"],
            &queue_arn,
            &["limit"],
        )
        .await;
        let resource = match level {
            "application" => arn(&format!("application/{application}")),
            "environment" => arn(&format!("application/{application}/environment/{environment}")),
            _ => arn(&format!(
                "application/{application}/configurationprofile/{profile}"
            )),
        };
        // A strategy that reaches every target at once and bakes for nothing, so the
        // first deployment is finished before the second one starts.
        let strategy = given_strategy(&router, "pre-start-instant", 0, 0).await;
        associate(
            &router,
            level,
            &extension,
            &resource,
            &json!({ "limit": "10" }),
        )
        .await;

        // Two deployments in a row, so the rows also differ along the deployment numbers
        // the event reports and the version it carries.
        for (number, version) in [(1, "1"), (2, "2")] {
            let started = start_deployment(
                &router,
                &application,
                &environment,
                &json!({
                    "DeploymentStrategyId": strategy,
                    "ConfigurationProfileId": profile,
                    "ConfigurationVersion": version,
                    // "A map of dynamic extension parameter names to values to pass to
                    // associated extensions with PRE_START_DEPLOYMENT actions", keyed
                    // `<extension name>#<parameter name>`.
                    "DynamicExtensionParameters": { format!("{extension_name}#limit"): "99" },
                }),
            )
            .await;
            assert_eq!(
                started.status,
                201,
                "case {level:?} deployment {number}: expected 201, got {} / {}",
                started.status,
                String::from_utf8_lossy(&started.body)
            );

            let events = drain(&router, &queue_url).await;
            let pre_start: Vec<&Value> = events
                .iter()
                .filter(|event| event["Type"] == json!("PreStartDeployment"))
                .collect();
            assert_eq!(
                pre_start.len(),
                1,
                "case {level:?} deployment {number}: expected one PreStartDeployment event, \
                 got {events:?}"
            );
            let event = pre_start[0];
            assert_eq!(
                event["Application"],
                json!({ "Id": application, "Name": "pre-start-app" }),
                "case {level:?} deployment {number}: expected the application, got {event}"
            );
            assert_eq!(
                event["Environment"],
                json!({ "Id": environment, "Name": "pre-start-env" }),
                "case {level:?} deployment {number}: expected the environment, got {event}"
            );
            assert_eq!(
                event["ConfigurationProfile"],
                json!({ "Id": profile, "Name": "pre-start-profile" }),
                "case {level:?} deployment {number}: expected the profile, got {event}"
            );
            assert_eq!(
                event["ContentVersion"],
                json!(version),
                "case {level:?} deployment {number}: expected ContentVersion as a string, \
                 got {event}"
            );
            let carried = decoded(event["Content"].as_str().unwrap_or_else(|| {
                panic!("case {level:?} deployment {number}: Content must be base64, got {event}")
            }));
            assert_eq!(
                carried,
                format!(r#"{{"v":{version}}}"#).into_bytes(),
                "case {level:?} deployment {number}: expected the candidate bytes, got {carried:?}"
            );
            assert_eq!(
                event["DeploymentNumber"],
                json!(number),
                "case {level:?} deployment {number}: expected the deployment number, got {event}"
            );
            assert_eq!(
                event["PreviousDeploymentNumber"],
                json!(number - 1),
                "case {level:?} deployment {number}: expected the previous deployment number, \
                 got {event}"
            );
            assert_eq!(
                event["Parameters"],
                json!({ "limit": "99" }),
                "case {level:?} deployment {number}: a dynamic value overrides the \
                 association's, got {event}"
            );

            // The invocation is recorded on the event that says the deployment started.
            let read = get_deployment(&router, &application, &environment, number).await;
            let log = read.json();
            let started_event = log["EventLog"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|event| event["EventType"] == json!("DEPLOYMENT_STARTED"))
                .unwrap_or_else(|| {
                    panic!("case {level:?} deployment {number}: no DEPLOYMENT_STARTED in {log}")
                });
            let invocations = started_event["ActionInvocations"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                invocations.iter().any(|invocation| {
                    invocation["ExtensionIdentifier"] == json!(extension_name)
                        && invocation["Uri"] == json!(queue_arn)
                        && invocation["ErrorCode"].is_null()
                }),
                "case {level:?} deployment {number}: expected the action invocation on the \
                 DEPLOYMENT_STARTED event, got {started_event}"
            );
        }
    }
}

/// An association bound to ANOTHER environment or ANOTHER configuration profile is not
/// bound to this deployment's workflow, so its actions are not invoked.
#[tokio::test]
async fn an_association_bound_elsewhere_is_not_invoked() {
    let router = emulator();
    let (queue_url, queue_arn) = given_queue(&router, "unbound-observations").await;
    let application = create_application(&router, "unbound-app").await;
    let environment = create_environment(&router, &application, "unbound-env").await;
    let other_environment = create_environment(&router, &application, "other-env").await;
    let profile = create_profile(&router, &application, "unbound-profile").await;
    let other_profile = create_profile(&router, &application, "other-profile").await;
    let other_application = create_application(&router, "other-app").await;
    let created = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"v":1}"#,
    )
    .await;
    assert_eq!(created.status, 201, "seeding a version answered {}", created.status);
    let (extension, _) = given_extension(
        &router,
        "unbound-gate",
        &["PRE_START_DEPLOYMENT"],
        &queue_arn,
        &[],
    )
    .await;

    // Rows: each binding that does NOT cover this deployment.
    let rows = [
        (
            "another environment",
            arn(&format!(
                "application/{application}/environment/{other_environment}"
            )),
        ),
        (
            "another configuration profile",
            arn(&format!(
                "application/{application}/configurationprofile/{other_profile}"
            )),
        ),
        (
            "another application",
            arn(&format!("application/{other_application}")),
        ),
    ];
    for (case, resource) in rows {
        associate(&router, case, &extension, &resource, &Value::Null).await;
    }

    let started = start_deployment(
        &router,
        &application,
        &environment,
        &json!({
            "DeploymentStrategyId": "AppConfig.AllAtOnce",
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "case \"bound elsewhere\": expected 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let events = drain(&router, &queue_url).await;
    assert!(
        events.is_empty(),
        "case \"bound elsewhere\": no action bound to another resource may be invoked, \
         got {events:?}"
    );
}

/// An action point AppConfig consults before performing the call GATES it: a target that
/// cannot be invoked refuses the call with 400 `BadRequestException` carrying the
/// service's own `Error invoking extension <name>: …` template, and leaves nothing behind
/// — no deployment record, and no stored configuration version.
#[tokio::test]
async fn a_consulted_action_point_whose_target_does_not_answer_refuses_the_call() {
    let router = emulator();
    let application = create_application(&router, "gated-app").await;
    let environment = create_environment(&router, &application, "gated-env").await;
    let profile = create_profile(&router, &application, "gated-profile").await;
    let seeded = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"v":1}"#,
    )
    .await;
    assert_eq!(seeded.status, 201, "seeding a version answered {}", seeded.status);

    // A function nothing created: the invocation is refused by Lambda itself, which is
    // the invocation failure AppConfig reports. Each row is one action point that gates
    // its call, with the call it gates.
    let missing_function =
        format!("arn:aws:lambda:{REGION}:{}:function:nobody-created-me", engine::emulator_account_id());
    let (deployment_gate, deployment_gate_name) = given_extension(
        &router,
        "deployment-gate",
        &["PRE_START_DEPLOYMENT"],
        &missing_function,
        &[],
    )
    .await;
    let (write_gate, write_gate_name) = given_extension(
        &router,
        "write-gate",
        &["PRE_CREATE_HOSTED_CONFIGURATION_VERSION"],
        &missing_function,
        &[],
    )
    .await;
    associate(
        &router,
        "PRE_START_DEPLOYMENT",
        &deployment_gate,
        &arn(&format!("application/{application}/environment/{environment}")),
        &Value::Null,
    )
    .await;
    associate(
        &router,
        "PRE_CREATE_HOSTED_CONFIGURATION_VERSION",
        &write_gate,
        &arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        )),
        &Value::Null,
    )
    .await;

    let refused_deployment = start_deployment(
        &router,
        &application,
        &environment,
        &json!({
            "DeploymentStrategyId": "AppConfig.AllAtOnce",
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    let refused_write = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"v":2}"#,
    )
    .await;

    for (case, answer, extension_name) in [
        (
            "PRE_START_DEPLOYMENT",
            &refused_deployment,
            &deployment_gate_name,
        ),
        (
            "PRE_CREATE_HOSTED_CONFIGURATION_VERSION",
            &refused_write,
            &write_gate_name,
        ),
    ] {
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
        let message = answer.string("Message").unwrap_or_default();
        assert!(
            message.starts_with(&format!("Error invoking extension {extension_name}: ")),
            "case {case:?}: expected the service's refusal template, got {message:?}"
        );
    }

    // Nothing was left behind: no deployment record, and no second version.
    let deployments = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await;
    let items = deployments.json()["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        items.is_empty(),
        "case \"vetoed deployment\": no deployment record may exist, got {items:?}"
    );
    let versions = json_call(
        &router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        &Value::Null,
    )
    .await;
    let numbers: Vec<i64> = versions.json()["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["VersionNumber"].as_i64())
        .collect();
    assert_eq!(
        numbers,
        vec![1],
        "case \"vetoed write\": only the version written before the association may exist, \
         got {numbers:?}"
    );
}

/// The action points of a deployment's OWN lifecycle fire as the deployment reaches each
/// stage: `ON_DEPLOYMENT_START` when it starts, `ON_DEPLOYMENT_COMPLETE` when the clock
/// completes it, and `AT_DEPLOYMENT_TICK` repeatedly while it is in flight — each exactly
/// once per stage, however often the deployment is read.
#[tokio::test]
async fn the_deployment_lifecycle_action_points_fire_as_the_deployment_reaches_each_stage() {
    let router = emulator();
    let (queue_url, queue_arn) = given_queue(&router, "lifecycle-observations").await;
    let application = create_application(&router, "lifecycle-app").await;
    let environment = create_environment(&router, &application, "lifecycle-env").await;
    let profile = create_profile(&router, &application, "lifecycle-profile").await;
    let seeded = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"v":1}"#,
    )
    .await;
    assert_eq!(seeded.status, 201, "seeding a version answered {}", seeded.status);
    let (extension, _) = given_extension(
        &router,
        "lifecycle-watcher",
        &[
            "ON_DEPLOYMENT_START",
            "ON_DEPLOYMENT_COMPLETE",
            "AT_DEPLOYMENT_TICK",
        ],
        &queue_arn,
        &[],
    )
    .await;
    associate(
        &router,
        "the environment",
        &extension,
        &arn(&format!("application/{application}/environment/{environment}")),
        &Value::Null,
    )
    .await;

    // A strategy with no rollout window and no bake time has nothing left to wait for,
    // so the deployment is complete the moment it starts: both of its lifecycle stages
    // are owed by the time `StartDeployment` answers, and no tick is ever owed.
    let instant = given_strategy(&router, "lifecycle-instant", 0, 0).await;
    let started = start_deployment(
        &router,
        &application,
        &environment,
        &json!({
            "DeploymentStrategyId": instant,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "case \"instant strategy\": expected 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    assert_eq!(
        started.json()["State"],
        json!("COMPLETE"),
        "case \"instant strategy\": expected a completed deployment, got {}",
        started.json()
    );
    let types: Vec<String> = drain(&router, &queue_url)
        .await
        .iter()
        .map(|event| event["Type"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        types,
        vec![
            "OnDeploymentStart".to_string(),
            "OnDeploymentComplete".to_string()
        ],
        "case \"instant strategy\": expected both lifecycle stages, in the order they \
         happened, got {types:?}"
    );

    // Reading the deployment again owes nothing: each stage happened once.
    let read = get_deployment(&router, &application, &environment, 1).await;
    assert_eq!(
        read.status, 200,
        "case \"instant strategy\": GetDeployment answered {}",
        read.status
    );
    let again = drain(&router, &queue_url).await;
    assert!(
        again.is_empty(),
        "case \"instant strategy\": a stage must not fire twice, got {again:?}"
    );

    // A deployment with a rollout window is still in flight, so its ticks fall due while
    // nobody is watching and are invoked the next time it is read.
    let paced = given_strategy(&router, "lifecycle-paced", 1, 0).await;
    let paced = start_deployment(
        &router,
        &application,
        &environment,
        &json!({
            "DeploymentStrategyId": paced,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        paced.status,
        201,
        "case \"paced strategy\": expected 201, got {} / {}",
        paced.status,
        String::from_utf8_lossy(&paced.body)
    );
    assert_eq!(
        paced.json()["State"],
        json!("DEPLOYING"),
        "case \"paced strategy\": expected a deployment still rolling out, got {}",
        paced.json()
    );
    let start_types: Vec<String> = drain(&router, &queue_url)
        .await
        .iter()
        .map(|event| event["Type"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        start_types,
        vec!["OnDeploymentStart".to_string()],
        "case \"paced strategy\": expected only the start stage so far, got {start_types:?}"
    );

    // Past the first tick's due instant: the tick is the one action point owed by an
    // interval rather than by a stage.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let read = get_deployment(&router, &application, &environment, 2).await;
    assert_eq!(
        read.status, 200,
        "case \"paced strategy\": GetDeployment answered {}",
        read.status
    );
    let ticks = drain(&router, &queue_url).await;
    assert_eq!(
        ticks
            .iter()
            .filter(|event| event["Type"] == json!("AtDeploymentTick"))
            .count(),
        1,
        "case \"paced strategy\": expected one tick invocation, got {ticks:?}"
    );
    let tick = ticks
        .iter()
        .find(|event| event["Type"] == json!("AtDeploymentTick"))
        .unwrap_or_else(|| panic!("case \"paced strategy\": no tick in {ticks:?}"));
    assert_eq!(
        tick["DeploymentNumber"],
        json!(2),
        "case \"paced strategy\": a tick names the deployment it paces, got {tick}"
    );
    assert_eq!(
        tick["Environment"],
        json!({ "Id": environment, "Name": "lifecycle-env" }),
        "case \"paced strategy\": a tick names the environment, got {tick}"
    );
}
