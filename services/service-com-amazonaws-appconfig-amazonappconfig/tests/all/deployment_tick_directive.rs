// SPDX-License-Identifier: CC0-1.0

//! An `AT_DEPLOYMENT_TICK` extension STEERS the rollout it is invoked for: the directive
//! the action's target answers decides whether the deployment carries on or is aborted.
//!
//! The rule under test, which the table below is one slice of: while a deployment paced by
//! a non-instant strategy is in flight, every action an associated extension binds at
//! `AT_DEPLOYMENT_TICK` is invoked once per tick with the event of that deployment —
//! naming the deployment, the environment under rollout and the rollout position the tick
//! is asking about — and the
//! answer is read as a directive — the one directive that aborts a rollout
//! (`ROLL_BACK`, `state::extension_actions::DIRECTIVE_ROLL_BACK`) drives the deployment to
//! the terminal `ROLLED_BACK` state, logs `ROLLBACK_STARTED` and `ROLLBACK_COMPLETED`
//! instead of `DEPLOYMENT_COMPLETED`, carries the `Description` the target answered on the
//! `ROLLBACK_STARTED` entry and attributes that entry to the EXTENSION that directed it,
//! withdraws the configuration from every target so the data plane serves the previously
//! deployed version's exact bytes again, and leaves a deployment a stop can no longer
//! apply to; every other answer steers nothing, so the rollout completes.
//!
//! Only a Lambda function can ANSWER an action point, so the steering target of every row
//! is a function this emulator runs, and the rows differ along what its handler answers:
//! the abort directive with and without a reason, the directive that lets the rollout
//! stand, no directive at all, and a directive AppConfig defines no meaning for. Each row
//! also binds a SECOND extension whose tick action targets an Amazon SQS queue — the
//! target kind that cannot answer — because that is where the event handed to the action
//! point is observable as a client sees it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, send, Answer,
};
use crate::configuration_profile_validators::{activate_functions, lambda};
use crate::extension_action_points::{
    associate, create_version, drain, get_deployment, given_growing_strategy, given_queue,
    given_strategy, start_deployment,
};
use crate::extensions::create_extension;
use crate::resource_tags::arn;

/// The `DeploymentState` values a deployment can no longer leave
/// (`smithy_query(query="shape",shape="DeploymentState")`: the rollout ends in one of
/// these, and the suite waits for whichever one this row's directive leads to.
const TERMINAL_STATES: &[&str] = &["COMPLETE", "ROLLED_BACK", "REVERTED"];

/// The `DeploymentState` values of a deployment the service is still rolling out, which is
/// when the position a tick reports is the position the deployment itself reports.
const IN_FLIGHT_STATES: &[&str] = &["DEPLOYING", "BAKING"];

/// The share of targets this suite's paced rollout grows by per interval.
///
/// A factor that takes SEVERAL intervals to reach every target, so the ticks of one
/// deployment are asked about more than one position: "linear … AppConfig deploys the
/// configuration to the number of targets specified by the growth factor per interval"
/// (`smithy_query(query="shape",shape="GrowthType")`).
const GROWTH_FACTOR: f64 = 25.0;

/// Every share of targets a rollout growing by [`GROWTH_FACTOR`] per interval stands at:
/// the share it starts from, each cumulative step, and every target.
const GROWTH_SHARES: &[f64] = &[0.0, 25.0, 50.0, 75.0, 100.0];

/// How long a row waits for its deployment to reach a terminal state.
///
/// A ceiling on the WAIT, not a statement about how long a rollout takes: a deployment
/// still in flight here is one nothing is advancing, which is the defect this suite pins.
const SETTLED_BY: Duration = Duration::from_secs(120);

/// How often the deployment under observation is read while waiting for that condition.
///
/// Reading it is also what gives the ticks the clock has made due somewhere to run, the
/// way a client watching its own rollout does.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// AppConfig, Lambda and Amazon SQS on ONE engine: the steering target and the observation
/// target of an extension action both live in another service, and the invocation is
/// asserted where a client sees it.
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
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// A .zip deployment package holding `source` as the handler's module, written in memory
/// the way a caller uploads one and base64-encoded as `FunctionCode$ZipFile` carries it.
fn package(source: &str) -> String {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the archive entry starts");
        std::io::Write::write_all(&mut writer, source.as_bytes()).expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        buffer.into_inner(),
    )
}

/// The handler of a steering function: whatever the row asked it to answer, for every
/// event it is handed.
///
/// The answer is parsed from JSON text inside the handler, so a row states exactly the
/// document a tick target replies with and nothing of it is assembled here.
fn steering_source(answer: &str) -> String {
    format!(
        "import json\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   return json.loads({answer:?})\n"
    )
}

/// Create the function `name` running `source`, bring the activation Lambda's own clock
/// owes it forward so no row waits for that window, and answer the function's ARN.
async fn given_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    source: &str,
) -> String {
    let created = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{}:role/appconfig-tick", engine::emulator_account_id()),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 20,
            "Code": { "ZipFile": package(source) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateFunction answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    activate_functions(engine);
    created
        .string("FunctionArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateFunction answered no FunctionArn"))
}

/// An extension binding one action at `AT_DEPLOYMENT_TICK` pointing at `uri`. Answers its
/// minted id.
async fn given_tick_extension(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    uri: &str,
) -> String {
    let actions: BTreeMap<String, Value> = BTreeMap::from([(
        "AT_DEPLOYMENT_TICK".to_string(),
        json!([{ "Name": format!("{name}-action"), "Uri": uri }]),
    )]);
    let created = create_extension(router, &json!({ "Name": name, "Actions": actions }), None).await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateExtension answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateExtension answered no Id"))
}

/// Read the deployment until it reaches a state it can no longer leave, and answer the
/// document it was last reported with together with every `AT_DEPLOYMENT_TICK` event the
/// ticks of this deployment delivered to the observation queue.
///
/// The wait is on the CONDITION: the loop looks again until the state is terminal or
/// [`SETTLED_BY`] has passed, so nothing here depends on how fast the host runs. Reading
/// the deployment is also what runs the ticks the clock has made due, so the events a read
/// produced are drained right after it — which is what lets each tick's reported position
/// be checked against the position that same read reported for the deployment, while it is
/// still rolling out.
async fn settled(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    number: i32,
    queue_url: &str,
) -> (Value, Vec<Value>) {
    let deadline = Instant::now() + SETTLED_BY;
    let mut ticks: Vec<Value> = Vec::new();
    loop {
        let read = get_deployment(router, application, environment, number).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetDeployment answered {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let body = read.json();
        let state = body["State"].as_str().unwrap_or_default().to_string();
        let reported = body["PercentageComplete"].as_f64().unwrap_or_default();
        for event in drain(router, queue_url).await {
            if event["Type"] != json!("AtDeploymentTick") {
                continue;
            }
            let position = event["PercentageComplete"].as_f64().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a tick must report the rollout position it is asking \
                     about, got {event}"
                )
            });
            // Only while the rollout stands: a tick that asked for the abort is answered
            // by withdrawing the configuration from every target, so the position the read
            // reports afterwards is the one the ROLLBACK left, not the one the tick saw.
            if IN_FLIGHT_STATES.contains(&state.as_str()) {
                assert_eq!(
                    position, reported,
                    "case {case:?}: a tick reports the position the deployment stands at, \
                     got {position} where the same read reported {reported}"
                );
            }
            ticks.push(event);
        }
        if TERMINAL_STATES.contains(&state.as_str()) {
            return (body, ticks);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the deployment never reached a terminal state, it is still \
             {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `EventType` of every entry of a deployment document's event log.
fn event_types(document: &Value) -> Vec<String> {
    document["EventLog"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| event["EventType"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The newest event log entry of `event_type`, which is the first one: "the most recent
/// events are displayed first" (`smithy_query(query="shape",shape="Deployment")`).
fn newest_event(document: &Value, event_type: &str) -> Option<Value> {
    document["EventLog"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|event| event["EventType"] == json!(event_type))
}

/// The configuration AppConfig serves one client for a triple, as `(bytes, version)`.
async fn served(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    profile: &str,
) -> (Vec<u8>, Option<String>) {
    let read = send(
        router,
        "GET",
        &format!(
            "/applications/{application}/environments/{environment}/configurations/{profile}\
             ?client_id=tick-observer"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: GetConfiguration answered {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    (read.body.to_vec(), read.header("Configuration-Version"))
}

/// `StopDeployment` of one deployment, with no `AllowRevert`.
async fn stop_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    number: i32,
) -> Answer {
    json_call(
        router,
        "DELETE",
        &format!("/applications/{application}/environments/{environment}/deployments/{number}"),
        &Value::Null,
    )
    .await
}

/// What one row's tick target answers and what that makes of the rollout.
struct Row {
    /// The row's name, which every assertion of it reports.
    case: &'static str,
    /// The JSON document the row's handler answers, verbatim.
    answer: &'static str,
    /// The terminal `DeploymentState` the rollout ends in.
    expected_state: &'static str,
    /// The `Description` the answer asked AppConfig to record for the rollback, when it
    /// supplied one.
    expected_description: Option<&'static str>,
}

/// The rows: every shape of answer a tick target can give, and what each makes of the
/// rollout. Only `ROLL_BACK` aborts one.
const ROWS: &[Row] = &[
    Row {
        case: "an abort directive with a reason",
        answer: r#"{"Directive": "ROLL_BACK", "Description": "error rate above the canary limit"}"#,
        expected_state: "ROLLED_BACK",
        expected_description: Some("error rate above the canary limit"),
    },
    Row {
        case: "an abort directive with no reason",
        answer: r#"{"Directive": "ROLL_BACK"}"#,
        expected_state: "ROLLED_BACK",
        expected_description: None,
    },
    Row {
        case: "a directive to carry on",
        answer: r#"{"Directive": "CONTINUE", "Description": "the canary is healthy"}"#,
        expected_state: "COMPLETE",
        expected_description: None,
    },
    Row {
        case: "an answer carrying no directive",
        answer: "{}",
        expected_state: "COMPLETE",
        expected_description: None,
    },
    Row {
        case: "a directive AppConfig defines no meaning for",
        answer: r#"{"Directive": "PAUSE", "Description": "not a directive of the API"}"#,
        expected_state: "COMPLETE",
        expected_description: None,
    },
];

/// The whole chain a tick's directive drives, asserted per row through the front door.
#[tokio::test]
async fn an_at_deployment_tick_directive_steers_the_deployment_it_is_invoked_for() {
    let (router, engine) = emulator();
    // One instant strategy to put the PREVIOUS version in service, and one paced strategy
    // for the deployment under tick: an action point fires for a deployment that is in
    // FLIGHT, and a strategy with no rollout window leaves none in flight.
    let instant = given_strategy(&router, "tick-instant", 0, 0).await;
    let paced = given_growing_strategy(&router, "tick-paced", 1, 0, GROWTH_FACTOR).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let suffix = index + 1;
        let application = create_application(&router, &format!("tick-app-{suffix}")).await;
        let environment =
            create_environment(&router, &application, &format!("tick-env-{suffix}")).await;
        let profile = create_profile(&router, &application, &format!("tick-profile-{suffix}")).await;
        let first = format!(r#"{{"release":"first","row":{suffix}}}"#);
        let second = format!(r#"{{"release":"second","row":{suffix}}}"#);
        for content in [&first, &second] {
            let written = create_version(
                &router,
                &application,
                &profile,
                "application/json",
                content.as_bytes(),
            )
            .await;
            assert_eq!(
                written.status,
                201,
                "case {case:?}: CreateHostedConfigurationVersion answered {} / {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        // Version 1 in service first, with no extension associated yet: it is the
        // configuration a rolled-back deployment of version 2 has to leave behind.
        let seeding = start_deployment(
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
            seeding.json()["State"],
            json!("COMPLETE"),
            "case {case:?}: the instant deployment of version 1 must be complete, got {}",
            seeding.json()
        );
        let (seeded, seeded_version) =
            served(&router, case, &application, &environment, &profile).await;
        assert_eq!(
            String::from_utf8_lossy(&seeded),
            first,
            "case {case:?}: version 1 must be the configuration in service before the \
             rollout, got {seeded_version:?}"
        );

        // The steering extension, and beside it an observation extension on a queue: a
        // queue cannot answer, so it reports the event without steering anything.
        let function = given_function(
            &router,
            &engine,
            case,
            &format!("tick-steer-{suffix}"),
            &steering_source(row.answer),
        )
        .await;
        let (queue_url, queue_arn) = given_queue(&router, &format!("tick-observations-{suffix}")).await;
        let steering =
            given_tick_extension(&router, case, &format!("tick-steer-ext-{suffix}"), &function)
                .await;
        let watching = given_tick_extension(
            &router,
            case,
            &format!("tick-watch-ext-{suffix}"),
            &queue_arn,
        )
        .await;
        let environment_arn = arn(&format!("application/{application}/environment/{environment}"));
        for extension in [&steering, &watching] {
            associate(&router, case, extension, &environment_arn, &Value::Null).await;
        }

        let started = start_deployment(
            &router,
            &application,
            &environment,
            &json!({
                "DeploymentStrategyId": paced,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "2",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {case:?}: StartDeployment answered {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        assert_eq!(
            started.json()["State"],
            json!("DEPLOYING"),
            "case {case:?}: a paced deployment is still rolling out when StartDeployment \
             answers, got {}",
            started.json()
        );

        let (document, ticks) =
            settled(&router, case, &application, &environment, 2, &queue_url).await;
        let state = document["State"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            state, row.expected_state,
            "case {case:?}: expected the rollout to end {}, got {state:?} with event log {:?}",
            row.expected_state,
            event_types(&document)
        );

        // The action point FIRED: the observation target was handed the tick events of this
        // deployment, each naming the deployment, the environment under rollout and the
        // rollout position the tick is asking about — which `settled` already checked
        // against the position the deployment itself reported.
        let tick = ticks.first().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the AT_DEPLOYMENT_TICK action point must be invoked while \
                 the deployment is in flight, got no tick event"
            )
        });
        assert_eq!(
            tick["DeploymentNumber"],
            json!(2),
            "case {case:?}: a tick names the deployment it paces, got {tick}"
        );
        assert_eq!(
            tick["Environment"]["Id"],
            json!(environment),
            "case {case:?}: a tick names the environment under deployment, got {tick}"
        );
        // Every position is one of the shares this strategy's growth factor reaches, and a
        // rollout only ever grows, so the positions consecutive ticks of one deployment
        // report never go backwards.
        let mut reached = 0.0_f64;
        for tick in &ticks {
            let position = tick["PercentageComplete"].as_f64().unwrap_or_default();
            assert!(
                GROWTH_SHARES.contains(&position) && position >= reached,
                "case {case:?}: expected a rollout position among {GROWTH_SHARES:?} that \
                 does not go back from {reached}, got {position}"
            );
            reached = position;
        }
        // A rollout nothing aborted is ticked right through its growth steps, so its ticks
        // report a position that GREW: a tick reporting the share it started at for a
        // deployment that reached every target is reporting a stale position.
        if row.expected_state != "ROLLED_BACK" {
            assert!(
                reached > 0.0,
                "case {case:?}: the ticks of a rollout that reached every target must \
                 report the shares it grew through, the last reported {reached}"
            );
        }

        let types = event_types(&document);
        if row.expected_state == "ROLLED_BACK" {
            for owed in ["ROLLBACK_STARTED", "ROLLBACK_COMPLETED"] {
                assert!(
                    types.iter().any(|event| event == owed),
                    "case {case:?}: an aborted rollout logs {owed}, got {types:?}"
                );
            }
            assert!(
                !types.iter().any(|event| event == "DEPLOYMENT_COMPLETED"),
                "case {case:?}: an aborted rollout never completed, got {types:?}"
            );
            let rollback = newest_event(&document, "ROLLBACK_STARTED").unwrap_or_else(|| {
                panic!("case {case:?}: the rollback's own entry must be on the event log")
            });
            assert_eq!(
                rollback["TriggeredBy"],
                json!("EXTENSION"),
                "case {case:?}: the rollback was directed by the extension, so the entry \
                 attributes it to the extension, got {rollback}"
            );
            let description = rollback["Description"].as_str().unwrap_or_default();
            match row.expected_description {
                Some(expected) => assert_eq!(
                    description, expected,
                    "case {case:?}: the rollback entry carries the description the extension \
                     answered, got {description:?}"
                ),
                // The target named no reason, so AppConfig's own description stands —
                // whatever it says, the entry is not left without one.
                None => assert!(
                    !description.is_empty(),
                    "case {case:?}: the rollback entry must describe the rollback, got \
                     {description:?}"
                ),
            }
        } else {
            assert!(
                types.iter().any(|event| event == "DEPLOYMENT_COMPLETED"),
                "case {case:?}: a rollout nothing aborted completes, got {types:?}"
            );
            assert!(
                !types.iter().any(|event| event == "ROLLBACK_STARTED"),
                "case {case:?}: only a ROLL_BACK directive aborts a rollout, got {types:?}"
            );
        }

        // The data plane follows the deployment: an aborted rollout puts the previous
        // version's exact bytes back in service, and one that completed serves the new
        // ones.
        let (in_service, version) = served(&router, case, &application, &environment, &profile).await;
        let (expected_bytes, expected_version) = if row.expected_state == "ROLLED_BACK" {
            (&first, "1")
        } else {
            (&second, "2")
        };
        assert_eq!(
            String::from_utf8_lossy(&in_service),
            **expected_bytes,
            "case {case:?}: expected the bytes of version {expected_version} in service, the \
             served version was {version:?}"
        );
        assert_eq!(
            version.as_deref(),
            Some(expected_version),
            "case {case:?}: expected version {expected_version} in service, got {version:?}"
        );

        // A deployment the service has finished with is no longer stoppable, whichever
        // terminal state it reached.
        let stopped = stop_deployment(&router, &application, &environment, 2).await;
        assert_eq!(
            stopped.status,
            400,
            "case {case:?}: a terminal deployment cannot be stopped, got {} / {}",
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        assert_eq!(
            stopped.error_code().split('#').next_back(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {}",
            stopped.error_code()
        );
        let message = stopped.string("Message").unwrap_or_default();
        let expected = format!(
            "Deployment 2 for App {application} and Environment {environment} cannot be \
             stopped, it has a status of {state}."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }
}
