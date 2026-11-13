// SPDX-License-Identifier: CC0-1.0

//! `UpdateConnector`, end to end over HTTP: the settings an update leaves a connector
//! holding, and the connector operation it records.
//!
//! RULE. An update names exactly one of a connector's two updatable settings — its
//! capacity (the worker setting) or its configuration — against the version the
//! connector is at, and is asynchronous: the connector answers in its updating state,
//! keeps reporting the settings and the version it is still running with, and the CLOCK
//! is what leaves it running on exactly the settings the request named, at the next
//! version. The same call records ONE connector operation against that connector, typed
//! for the setting that changes, which reports the settings on each side of the update
//! and walks the model's update states and steps until it completes. A request that
//! names neither setting or both, a capacity that names neither of its forms or both, a
//! capacity whose counts are not ones the service allocates (an `mcuCount` outside the
//! value set its member documents, or auto-scaling worker and task counts that do not
//! bound each other), a
//! version that is not the connector's, and a connector that is not running are each
//! refused with nothing stored.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response body read
//! back as JSON. Nothing reaches into the state struct, so a handler answering from the
//! request instead of from state would fail here.
//!
//! The tables span what the rule generalises over — both capacity forms (including a
//! switch from one form to the other), each published bound of the two bounded capacity
//! integers and a value past each, a configuration that drops, changes and adds keys, and
//! each state a connector cannot be updated in — rather than a single recorded call.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{MCU_COUNT_VALUES, TRANSITION_SPAN};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN must
/// carry. No authenticator is registered, so the signature is never checked; the scope
/// is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `ConnectorOperationType` an update to a connector's capacity runs under, and the
/// `ConnectorOperationStepType` of the step that applies it: the model spells both the
/// same (`smithy_query query="shape" shape="ConnectorOperationType,ConnectorOperationStepType"`).
const UPDATE_WORKER_SETTING: &str = "UPDATE_WORKER_SETTING";

/// The same, for an update to a connector's configuration.
const UPDATE_CONFIGURATION: &str = "UPDATE_CONNECTOR_CONFIGURATION";

/// The steps an update of `operation_type` walks, in order.
///
/// `ConnectorOperationStepType` lists exactly five values — `INITIALIZE_UPDATE`,
/// `VALIDATE_UPDATE`, `UPDATE_WORKER_SETTING`, `UPDATE_CONNECTOR_CONFIGURATION` and
/// `FINALIZE_UPDATE` — the three an update of either kind shares around the one that
/// names the change itself, which is spelled the same as the operation's own type.
fn expected_step_types(operation_type: &str) -> Vec<String> {
    [
        "INITIALIZE_UPDATE",
        "VALIDATE_UPDATE",
        operation_type,
        "FINALIZE_UPDATE",
    ]
    .iter()
    .map(|step| (*step).to_string())
    .collect()
}

/// Long enough that one change the clock owes after a call is due. The span itself is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// Long enough that every step of an update is due: one span per step of the
/// progression, both the step count and the span taken from the rule rather than from a
/// literal.
async fn past_the_whole_update() {
    let steps = u32::try_from(expected_step_types(UPDATE_WORKER_SETTING).len())
        .expect("a small progression");
    tokio::time::sleep(TRANSITION_SPAN * steps + Duration::from_millis(500)).await;
}

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS
/// SDK encodes a label: every character a path segment reserves (including the ARN's own
/// `:` and `/`) is escaped.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every number `value` carries, behind the path it sits at.
///
/// A capacity is a small tree of integers, and `CapacityUpdate` and `CapacityDescription`
/// name their members identically, so the numbers an update asked for and the numbers a
/// description reports compare directly — and a comparison of these maps names the
/// member that differs instead of printing two blobs.
fn numbers(value: &Value) -> BTreeMap<String, i64> {
    fn walk(at: &str, value: &Value, into: &mut BTreeMap<String, i64>) {
        match value {
            Value::Object(members) => {
                for (member, value) in members {
                    walk(&format!("{at}/{member}"), value, into);
                }
            }
            Value::Number(number) => {
                into.insert(at.to_string(), number.as_i64().unwrap_or_default());
            }
            _ => {}
        }
    }
    let mut collected = BTreeMap::new();
    walk("", value, &mut collected);
    collected
}

/// A JSON object of strings as a map, for a comparison that names the key that differs.
fn strings(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `CreateCustomPlugin` body for `name`.
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The capacity every connector in this suite is created with: the provisioned form, so
/// a row updating to the auto-scaled form is a change of FORM and not only of value.
fn created_capacity() -> Value {
    json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}})
}

/// The configuration every connector in this suite is created with.
fn created_configuration() -> Value {
    json!({
        "connector.class": "io.confluent.connect.s3.S3SinkConnector",
        "tasks.max": "2",
    })
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`.
fn connector_request(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": created_capacity(),
        "connectorConfiguration": created_configuration(),
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{name}.kafka.{REGION}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
    })
}

/// Create one ACTIVE custom plugin and return its ARN.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) =
        call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    past_one_transition().await;
    text(&created, "customPluginArn")
}

/// Create the named connectors against `plugin_arn` and return their ARNs, in the same
/// order, once the clock has moved every one of them to `RUNNING`.
async fn running_connectors(
    emulator: &MultiServiceRouter,
    plugin_arn: &str,
    names: &[String],
) -> Vec<String> {
    let mut arns = Vec::new();
    for name in names {
        let (status, created) = call(
            emulator,
            "POST",
            "/v1/connectors",
            connector_request(name, plugin_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnector must succeed, got {status} {created}"
        );
        arns.push(text(&created, "connectorArn"));
    }
    past_one_transition().await;
    arns
}

/// `DescribeConnector` for `connector_arn`.
async fn describe(emulator: &MultiServiceRouter, connector_arn: &str) -> Value {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeConnector must succeed, got {status} {described}"
    );
    described
}

/// `UpdateConnector` against `connector_arn` at `current_version`, with `body` as the
/// request body.
async fn update(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    current_version: &str,
    body: Value,
) -> (u16, Value) {
    let path = format!(
        "/v1/connectors/{}?currentVersion={}",
        label(connector_arn),
        label(current_version)
    );
    call(emulator, "PUT", &path, body).await
}

/// `DescribeConnectorOperation` for `operation_arn`.
async fn describe_operation(emulator: &MultiServiceRouter, operation_arn: &str) -> Value {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectorOperations/{}", label(operation_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeConnectorOperation must succeed, got {status} {described}"
    );
    described
}

/// The `(stepType, stepState)` pairs a described operation reports, in order.
fn reported_steps(described: &Value) -> Vec<(String, String)> {
    described["operationSteps"]
        .as_array()
        .unwrap_or_else(|| panic!("an operationSteps array in {described}"))
        .iter()
        .map(|step| (text(step, "stepType"), text(step, "stepState")))
        .collect()
}

/// Which of a connector's two updatable settings one update names.
enum Settings {
    /// `capacity`: the connector's worker setting.
    Capacity(Value),
    /// `connectorConfiguration`: the whole configuration map the connector takes on.
    Configuration(Value),
}

impl Settings {
    /// The `UpdateConnector` request body naming these settings.
    fn body(&self) -> Value {
        match self {
            Self::Capacity(capacity) => json!({"capacity": capacity}),
            Self::Configuration(configuration) => {
                json!({"connectorConfiguration": configuration})
            }
        }
    }

    /// The `ConnectorOperationType` the update runs under.
    fn operation_type(&self) -> &'static str {
        match self {
            Self::Capacity(_) => UPDATE_WORKER_SETTING,
            Self::Configuration(_) => UPDATE_CONFIGURATION,
        }
    }
}

/// One row of the update table: what it names, and why that row is in the class.
struct UpdateCase {
    why: &'static str,
    settings: Settings,
}

/// The accepted updates, spanning what the rule generalises over.
///
/// `ProvisionedCapacityUpdate$mcuCount` and `AutoScalingUpdate$mcuCount` are
/// `__integerMin1Max8` (`range 1..8`) and both scaling policies'
/// `cpuUtilizationPercentage` is `__integerMin1Max100` (`range 1..100`), so each
/// published bound of each is a row; a value past either bound is in the refusal table.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            why: "the provisioned form, at the smallest mcuCount the model allows",
            settings: Settings::Capacity(
                json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 1}}),
            ),
        },
        UpdateCase {
            why: "the provisioned form, at the largest mcuCount the model allows",
            settings: Settings::Capacity(
                json!({"provisionedCapacity": {"mcuCount": 8, "workerCount": 7}}),
            ),
        },
        UpdateCase {
            why: "a switch to the auto-scaled form, at both policy bounds",
            settings: Settings::Capacity(json!({"autoScaling": {
                "maxWorkerCount": 8,
                "mcuCount": 2,
                "minWorkerCount": 2,
                "scaleInPolicy": {"cpuUtilizationPercentage": 1},
                "scaleOutPolicy": {"cpuUtilizationPercentage": 100},
                // "Must be at least equal to maxWorkerCount"
                // (`AutoScalingUpdate$maxAutoscalingTaskCount`).
                "maxAutoscalingTaskCount": 8,
            }})),
        },
        UpdateCase {
            why: "a configuration that drops a key, changes one and adds one",
            settings: Settings::Configuration(json!({
                "connector.class": "io.confluent.connect.s3.S3SinkConnector",
                "errors.tolerance": "all",
                "flush.size": "1000",
            })),
        },
        UpdateCase {
            why: "a configuration of one key, carrying a value outside ASCII",
            settings: Settings::Configuration(json!({
                "topics.regex": "\u{e9}v\u{e9}nements-.*",
            })),
        },
    ]
}

#[tokio::test]
async fn an_update_records_an_operation_and_the_clock_leaves_the_connector_on_the_new_settings() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "update-plugin").await;
    let cases = update_cases();
    let names: Vec<String> = (0..cases.len())
        .map(|index| format!("updated-connector-{index}"))
        .collect();
    let connector_arns = running_connectors(&emulator, &plugin_arn, &names).await;

    // Every update is issued first, then one wait covers every one of them: the clock,
    // not the order of the calls, is what finishes each.
    let mut started: Vec<(&UpdateCase, &String, Value, String)> = Vec::new();
    for ((case, connector_arn), name) in cases.iter().zip(&connector_arns).zip(&names) {
        let before = describe(&emulator, connector_arn).await;
        let version = text(&before, "currentVersion");
        assert_eq!(
            text(&before, "connectorState"),
            "RUNNING",
            "case {:?}: the connector must be running before the update, got {before}",
            case.why
        );

        let (status, updated) = update(&emulator, connector_arn, &version, case.settings.body())
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateConnector must succeed, got {status} {updated}",
            case.why
        );
        assert_eq!(
            text(&updated, "connectorArn"),
            *connector_arn,
            "case {:?}: the answer must name the connector that was updated, got {updated}",
            case.why
        );
        assert_eq!(
            text(&updated, "connectorState"),
            "UPDATING",
            "case {:?}: an updating connector is UPDATING, got {updated}",
            case.why
        );

        // The operation's ARN belongs to the connector's own scope, with its own minted
        // id appended.
        let operation_arn = text(&updated, "connectorOperationArn");
        let prefix = format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/{name}/");
        assert!(
            operation_arn.starts_with(&prefix),
            "case {:?}: expected a connectorOperationArn starting {prefix}, got \
             {operation_arn:?}",
            case.why
        );

        // Read the connector straight back: it is updating, and still reports the
        // settings and the version it is RUNNING with.
        let during = describe(&emulator, connector_arn).await;
        assert_eq!(
            text(&during, "connectorState"),
            "UPDATING",
            "case {:?}: the stored connector must be UPDATING, got {during}",
            case.why
        );
        assert_eq!(
            text(&during, "currentVersion"),
            version,
            "case {:?}: the version must not move before the update lands, got {during}",
            case.why
        );
        assert_eq!(
            numbers(&during["capacity"]),
            numbers(&created_capacity()),
            "case {:?}: the capacity must not move before the update lands, got {during}",
            case.why
        );
        assert_eq!(
            strings(&during["connectorConfiguration"]),
            strings(&created_configuration()),
            "case {:?}: the configuration must not move before the update lands, got \
             {during}",
            case.why
        );

        // And the operation it recorded reports the update: typed for the setting that
        // changes, still running, with the settings on each side and no step finished
        // out of turn.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            text(&operation, "connectorArn"),
            *connector_arn,
            "case {:?}: the operation must report the connector it runs against, got \
             {operation}",
            case.why
        );
        assert_eq!(
            text(&operation, "connectorOperationType"),
            case.settings.operation_type(),
            "case {:?}: the operation must be typed for the setting that changes, got \
             {operation}",
            case.why
        );
        let state = text(&operation, "connectorOperationState");
        assert!(
            state == "PENDING" || state == "UPDATE_IN_PROGRESS",
            "case {:?}: a just-started update must still be running, got {state:?}",
            case.why
        );
        assert!(
            operation.get("endTime").is_none(),
            "case {:?}: a running operation carries no endTime, got {operation}",
            case.why
        );
        let steps = reported_steps(&operation);
        let expected_types = expected_step_types(case.settings.operation_type());
        assert_eq!(
            steps
                .iter()
                .map(|(step_type, _)| step_type.clone())
                .collect::<Vec<String>>(),
            expected_types,
            "case {:?}: the operation must report the steps of its kind of update, got \
             {operation}",
            case.why
        );
        assert!(
            steps
                .iter()
                .all(|(_, step_state)| step_state != "COMPLETED"),
            "case {:?}: no step of a just-started update has completed, got {steps:?}",
            case.why
        );
        if state == "PENDING" {
            assert!(
                steps
                    .iter()
                    .all(|(_, step_state)| step_state == "PENDING"),
                "case {:?}: an operation that has not left PENDING is working no step, got \
                 {steps:?}",
                case.why
            );
        }

        match &case.settings {
            Settings::Capacity(capacity) => {
                assert_eq!(
                    numbers(&operation["originWorkerSetting"]["capacity"]),
                    numbers(&created_capacity()),
                    "case {:?}: the origin worker setting is the capacity the connector \
                     held, got {operation}",
                    case.why
                );
                assert_eq!(
                    numbers(&operation["targetWorkerSetting"]["capacity"]),
                    numbers(capacity),
                    "case {:?}: the target worker setting is the capacity the update \
                     named, got {operation}",
                    case.why
                );
                assert!(
                    operation.get("originConnectorConfiguration").is_none()
                        && operation.get("targetConnectorConfiguration").is_none(),
                    "case {:?}: an update to the worker setting reports no configuration \
                     pair, got {operation}",
                    case.why
                );
            }
            Settings::Configuration(configuration) => {
                assert_eq!(
                    strings(&operation["originConnectorConfiguration"]),
                    strings(&created_configuration()),
                    "case {:?}: the origin configuration is the one the connector held, \
                     got {operation}",
                    case.why
                );
                assert_eq!(
                    strings(&operation["targetConnectorConfiguration"]),
                    strings(configuration),
                    "case {:?}: the target configuration is the one the update named, got \
                     {operation}",
                    case.why
                );
                assert!(
                    operation.get("originWorkerSetting").is_none()
                        && operation.get("targetWorkerSetting").is_none(),
                    "case {:?}: an update to the configuration reports no worker setting \
                     pair, got {operation}",
                    case.why
                );
            }
        }

        started.push((case, connector_arn, before, operation_arn));
    }

    past_the_whole_update().await;

    for (case, connector_arn, before, operation_arn) in started {
        let after = describe(&emulator, connector_arn).await;
        assert_eq!(
            text(&after, "connectorState"),
            "RUNNING",
            "case {:?}: the clock must leave the connector running, got {after}",
            case.why
        );

        // The version the settings are at has moved exactly once.
        let was: u64 = text(&before, "currentVersion").parse().unwrap_or_else(|_| {
            panic!(
                "case {:?}: the created connector's version must be a count, got {before}",
                case.why
            )
        });
        assert_eq!(
            text(&after, "currentVersion"),
            (was + 1).to_string(),
            "case {:?}: an update that landed moves the connector one version on, got \
             {after}",
            case.why
        );

        // Exactly the settings the update named — and only the setting it named.
        match &case.settings {
            Settings::Capacity(capacity) => {
                assert_eq!(
                    numbers(&after["capacity"]),
                    numbers(capacity),
                    "case {:?}: the connector must read back with the capacity the update \
                     named, got {after}",
                    case.why
                );
                assert_eq!(
                    strings(&after["connectorConfiguration"]),
                    strings(&created_configuration()),
                    "case {:?}: an update to the capacity leaves the configuration alone, \
                     got {after}",
                    case.why
                );
            }
            Settings::Configuration(configuration) => {
                assert_eq!(
                    strings(&after["connectorConfiguration"]),
                    strings(configuration),
                    "case {:?}: the connector's configuration must be exactly the map the \
                     update named, got {after}",
                    case.why
                );
                assert_eq!(
                    numbers(&after["capacity"]),
                    numbers(&created_capacity()),
                    "case {:?}: an update to the configuration leaves the capacity alone, \
                     got {after}",
                    case.why
                );
            }
        }

        // The operation has completed, every step with it, and it now carries an end.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            text(&operation, "connectorOperationState"),
            "UPDATE_COMPLETE",
            "case {:?}: an update that landed is UPDATE_COMPLETE, got {operation}",
            case.why
        );
        assert!(
            !text(&operation, "endTime").is_empty(),
            "case {:?}: an operation that ended reports its endTime, got {operation}",
            case.why
        );
        let steps = reported_steps(&operation);
        assert!(
            steps
                .iter()
                .all(|(_, step_state)| step_state == "COMPLETED"),
            "case {:?}: every step of a completed update has completed, got {steps:?}",
            case.why
        );

        // And the list path reports the same stored record.
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors/{}/operations", label(connector_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListConnectorOperations must succeed, got {status} {listed}",
            case.why
        );
        let summaries = listed["connectorOperations"]
            .as_array()
            .unwrap_or_else(|| panic!("a connectorOperations array in {listed}"))
            .clone();
        assert_eq!(
            summaries.len(),
            1,
            "case {:?}: exactly the one update must be listed, got {listed}",
            case.why
        );
        assert_eq!(
            text(&summaries[0], "connectorOperationArn"),
            operation_arn,
            "case {:?}: the summary must name the stored operation, got {listed}",
            case.why
        );
        assert_eq!(
            text(&summaries[0], "connectorOperationType"),
            case.settings.operation_type(),
            "case {:?}: the summary must carry the stored type, got {listed}",
            case.why
        );
    }
}

#[tokio::test]
async fn successive_updates_each_move_the_connector_one_version_on() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "successive-plugin").await;
    let names = vec!["successively-updated".to_string()];
    let connector_arn = running_connectors(&emulator, &plugin_arn, &names)
        .await
        .remove(0);

    // The version a client holds is the one the service handed it, so each update names
    // the version the previous one left — and the one before that is refused.
    let updates = [
        Settings::Capacity(json!({"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}})),
        Settings::Configuration(json!({"tasks.max": "4"})),
        Settings::Capacity(json!({"provisionedCapacity": {"mcuCount": 4, "workerCount": 4}})),
    ];

    for (index, settings) in updates.iter().enumerate() {
        let version = text(&describe(&emulator, &connector_arn).await, "currentVersion");
        assert_eq!(
            version,
            (index + 1).to_string(),
            "update {index}: the connector must be at version {}, got {version:?}",
            index + 1
        );

        let (status, updated) = update(&emulator, &connector_arn, &version, settings.body()).await;
        assert_eq!(
            status, 200,
            "update {index}: UpdateConnector must succeed, got {status} {updated}"
        );
        past_the_whole_update().await;

        let after = describe(&emulator, &connector_arn).await;
        assert_eq!(
            text(&after, "currentVersion"),
            (index + 2).to_string(),
            "update {index}: the landed update must move the version on, got {after}"
        );
        match settings {
            Settings::Capacity(capacity) => assert_eq!(
                numbers(&after["capacity"]),
                numbers(capacity),
                "update {index}: the named capacity must read back, got {after}"
            ),
            Settings::Configuration(configuration) => assert_eq!(
                strings(&after["connectorConfiguration"]),
                strings(configuration),
                "update {index}: the named configuration must read back, got {after}"
            ),
        }

        // The version this update was called with is now stale.
        let (status, refused) = update(
            &emulator,
            &connector_arn,
            &version,
            Settings::Configuration(json!({"tasks.max": "9"})).body(),
        )
        .await;
        assert_eq!(
            status, 400,
            "update {index}: a replay at the spent version must be refused, got {status} \
             {refused}"
        );
    }

    // Every update is stored against the connector, one per call.
    let (status, listed) = call(
        &emulator,
        "GET",
        &format!("/v1/connectors/{}/operations", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListConnectorOperations must succeed, got {status} {listed}"
    );
    let summaries = listed["connectorOperations"]
        .as_array()
        .unwrap_or_else(|| panic!("a connectorOperations array in {listed}"))
        .len();
    assert_eq!(
        summaries,
        updates.len(),
        "one operation per update that was accepted, got {listed}"
    );
}

#[tokio::test]
async fn an_update_that_names_no_single_valid_setting_is_refused_with_nothing_stored() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "refusal-plugin").await;
    let names = vec!["refusing-connector".to_string()];
    let connector_arn = running_connectors(&emulator, &plugin_arn, &names)
        .await
        .remove(0);
    let before = describe(&emulator, &connector_arn).await;
    let version = text(&before, "currentVersion");

    // A capacity naming a value past a published bound: `mcuCount` is
    // `__integerMin1Max8` and `cpuUtilizationPercentage` is `__integerMin1Max100`, so one
    // past each bound of each is refused rather than stored.
    let rows: Vec<(&str, Value)> = vec![
        ("neither setting", json!({})),
        (
            "both settings at once",
            json!({
                "capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 2}},
                "connectorConfiguration": {"tasks.max": "3"},
            }),
        ),
        (
            "a capacity naming neither of its forms",
            json!({"capacity": {}}),
        ),
        (
            "a capacity naming both of its forms",
            json!({"capacity": {
                "provisionedCapacity": {"mcuCount": 2, "workerCount": 2},
                "autoScaling": {
                    "maxWorkerCount": 4,
                    "mcuCount": 2,
                    "minWorkerCount": 1,
                    "scaleInPolicy": {"cpuUtilizationPercentage": 20},
                    "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
                    "maxAutoscalingTaskCount": 4,
                },
            }}),
        ),
        (
            "an mcuCount below the smallest the model allows",
            json!({"capacity": {"provisionedCapacity": {"mcuCount": 0, "workerCount": 2}}}),
        ),
        (
            "an mcuCount above the largest the model allows",
            json!({"capacity": {"provisionedCapacity": {"mcuCount": 9, "workerCount": 2}}}),
        ),
        (
            "a scale-in percentage below the smallest the model allows",
            json!({"capacity": {"autoScaling": {
                "maxWorkerCount": 4,
                "mcuCount": 2,
                "minWorkerCount": 1,
                "scaleInPolicy": {"cpuUtilizationPercentage": 0},
                "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
                "maxAutoscalingTaskCount": 4,
            }}}),
        ),
        (
            "a scale-out percentage above the largest the model allows",
            json!({"capacity": {"autoScaling": {
                "maxWorkerCount": 4,
                "mcuCount": 2,
                "minWorkerCount": 1,
                "scaleInPolicy": {"cpuUtilizationPercentage": 20},
                "scaleOutPolicy": {"cpuUtilizationPercentage": 101},
                "maxAutoscalingTaskCount": 4,
            }}}),
        ),
    ];

    for (case, body) in rows {
        let (status, refused) = update(&emulator, &connector_arn, &version, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused}"
        );
        let unchanged = describe(&emulator, &connector_arn).await;
        assert_eq!(
            text(&unchanged, "connectorState"),
            "RUNNING",
            "case {case:?}: a refused update leaves the connector running, got {unchanged}"
        );
        assert_eq!(
            text(&unchanged, "currentVersion"),
            version,
            "case {case:?}: a refused update stores nothing, got {unchanged}"
        );
        assert_eq!(
            numbers(&unchanged["capacity"]),
            numbers(&created_capacity()),
            "case {case:?}: a refused update changes no capacity, got {unchanged}"
        );
    }

    // A version that is not the connector's own, whatever form it takes, and an ARN this
    // emulator holds no connector at: both are the one gate this operation decides before
    // it resolves the connector, so both carry the same refusal.
    //
    // `update_connector_version_gate.rs` is where that rule is spanned; these rows keep
    // the refusal beside the stored-nothing assertions of this suite.
    let unheld = format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/00000000-0000-0000-0000-000000000000"
    );
    let version_rows: Vec<(String, &str, &str)> = vec![
        ("a version the connector has moved past".to_string(), connector_arn.as_str(), "9"),
        ("an empty version".to_string(), connector_arn.as_str(), ""),
        ("a version in another form".to_string(), connector_arn.as_str(), "1.0"),
        ("the version before the first".to_string(), connector_arn.as_str(), "0"),
        ("an ARN no connector carries".to_string(), unheld.as_str(), "1"),
    ];
    for (case, arn, named) in &version_rows {
        let (status, refused) = update(
            &emulator,
            arn,
            named,
            json!({"connectorConfiguration": {"tasks.max": "5"}}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a currentVersion that is not a connector's current version is \
             a 400, got {status} {refused}"
        );
        assert!(
            error_code(&refused).contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            text(&refused, "message"),
            "Invalid parameter currentVersion.",
            "case {case:?}: the refusal must name the member it refused, got {refused}"
        );
    }

    // And a connector that is not running: one still updating, and one still being
    // created. Both are refused on the connector, with no second operation recorded.
    let (status, accepted) = update(
        &emulator,
        &connector_arn,
        &version,
        json!({"connectorConfiguration": {"tasks.max": "6"}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first update must be accepted, got {status} {accepted}"
    );
    let (status, refused) = update(
        &emulator,
        &connector_arn,
        &version,
        json!({"connectorConfiguration": {"tasks.max": "7"}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a connector that is already updating takes no update, got {status} {refused}"
    );
    assert_eq!(
        text(&refused, "message"),
        "Invalid parameter connectorArn: The connector is not RUNNING.",
        "the refusal must name the connector and why, got {refused}"
    );

    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_request("still-creating", &plugin_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "the connector must be created, got {status} {created}"
    );
    let creating = text(&created, "connectorArn");
    let (status, refused) = update(
        &emulator,
        &creating,
        "1",
        json!({"connectorConfiguration": {"tasks.max": "8"}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a connector that is still being created takes no update, got {status} {refused}"
    );
    let (status, listed) = call(
        &emulator,
        "GET",
        &format!("/v1/connectors/{}/operations", label(&creating)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListConnectorOperations must succeed, got {status} {listed}"
    );
    assert!(
        listed["connectorOperations"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "a refused update records no operation, got {listed}"
    );
}

/// What the service must answer one capacity's counts with.
#[derive(Clone, Copy)]
enum CountVerdict {
    /// Accepted: every count is one the service allocates.
    Accepted,
    /// Refused by the service, on the member it blames.
    RefusedNaming(&'static str),
    /// Refused by the modelled range the member targets, before the handler runs, so the
    /// message is the framework's rather than the service's member-naming one.
    RefusedByTheModel,
}

/// One row of the capacity-count table.
struct CountCase {
    why: String,
    capacity: Value,
    verdict: CountVerdict,
}

/// An auto-scaled capacity with these counts. Both scaling policies are required on
/// `AutoScalingUpdate`, so every row carries them at a legal percentage.
fn auto_scaling(mcu: i32, minimum: i32, maximum: i32, tasks: Option<i32>) -> Value {
    let mut scaling = json!({
        "mcuCount": mcu,
        "minWorkerCount": minimum,
        "maxWorkerCount": maximum,
        "scaleInPolicy": {"cpuUtilizationPercentage": 20},
        "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
    });
    if let Some(tasks) = tasks {
        scaling["maxAutoscalingTaskCount"] = json!(tasks);
    }
    json!({"autoScaling": scaling})
}

/// The capacity-count table, spanning what the rule generalises over.
///
/// `AutoScalingUpdate$mcuCount` and `ProvisionedCapacityUpdate$mcuCount` document "The
/// valid values are 1,2,4,8" inside a modelled `__integerMin1Max8` range that also admits
/// 3, 5, 6 and 7, so every integer from one below that range to one above it is a row, in
/// each of the two capacity forms. The auto-scaled rows then span the relations the other
/// members document: a worker window with its minimum below, equal to and above its
/// maximum, and a `maxAutoscalingTaskCount` absent, equal to `maxWorkerCount`, above it
/// and below it ("Must be at least equal to maxWorkerCount").
fn count_cases() -> Vec<CountCase> {
    let mut rows = Vec::new();
    for mcu in 0..=9 {
        let verdict = if MCU_COUNT_VALUES.contains(&mcu) {
            CountVerdict::Accepted
        } else if (1..=8).contains(&mcu) {
            CountVerdict::RefusedNaming("mcuCount")
        } else {
            CountVerdict::RefusedByTheModel
        };
        rows.push(CountCase {
            why: format!("the provisioned form at mcuCount {mcu}"),
            capacity: json!({"provisionedCapacity": {"mcuCount": mcu, "workerCount": 2}}),
            verdict,
        });
        rows.push(CountCase {
            why: format!("the auto-scaled form at mcuCount {mcu}"),
            capacity: auto_scaling(mcu, 1, 4, None),
            verdict,
        });
    }

    for (minimum, maximum, tasks, verdict) in [
        (1, 4, None, CountVerdict::Accepted),
        (2, 2, None, CountVerdict::Accepted),
        (1, 4, Some(4), CountVerdict::Accepted),
        (1, 4, Some(40), CountVerdict::Accepted),
        (8, 2, None, CountVerdict::RefusedNaming("minWorkerCount")),
        (5, 4, Some(10), CountVerdict::RefusedNaming("minWorkerCount")),
        (
            1,
            4,
            Some(3),
            CountVerdict::RefusedNaming("maxAutoscalingTaskCount"),
        ),
        (
            2,
            2,
            Some(1),
            CountVerdict::RefusedNaming("maxAutoscalingTaskCount"),
        ),
    ] {
        rows.push(CountCase {
            why: format!("minWorkerCount {minimum}, maxWorkerCount {maximum}, tasks {tasks:?}"),
            capacity: auto_scaling(2, minimum, maximum, tasks),
            verdict,
        });
    }
    rows
}

#[tokio::test]
async fn an_update_s_capacity_counts_must_be_ones_the_service_allocates() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "capacity-count-plugin").await;
    let rows = count_cases();

    // Every refused row is tried against ONE connector: a refusal stores nothing, so the
    // connector is still at its created capacity and version for the next row.
    let refusing = running_connectors(
        &emulator,
        &plugin_arn,
        &["count-refusals".to_string()],
    )
    .await
    .remove(0);
    let version = text(&describe(&emulator, &refusing).await, "currentVersion");

    for row in &rows {
        let member = match row.verdict {
            CountVerdict::Accepted => continue,
            CountVerdict::RefusedNaming(member) => Some(member),
            CountVerdict::RefusedByTheModel => None,
        };
        let (status, refused) = update(
            &emulator,
            &refusing,
            &version,
            json!({"capacity": row.capacity}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: counts the service does not allocate must be refused, got {status} \
             {refused}",
            row.why
        );
        assert!(
            error_code(&refused).contains("BadRequestException"),
            "case {:?}: expected BadRequestException, got {refused}",
            row.why
        );
        if let Some(member) = member {
            let expected = format!("Invalid parameter {member}: ");
            let actual = text(&refused, "message");
            assert!(
                actual.starts_with(&expected),
                "case {:?}: expected a message starting {expected:?}, got {actual:?}",
                row.why
            );
        }
        let unchanged = describe(&emulator, &refusing).await;
        assert_eq!(
            text(&unchanged, "currentVersion"),
            version,
            "case {:?}: a refused update stores nothing, got {unchanged}",
            row.why
        );
        assert_eq!(
            numbers(&unchanged["capacity"]),
            numbers(&created_capacity()),
            "case {:?}: a refused update changes no capacity, got {unchanged}",
            row.why
        );
    }

    // Each accepted row gets its own connector, so one wait covers every update the clock
    // then has to land.
    let accepted: Vec<&CountCase> = rows
        .iter()
        .filter(|row| matches!(row.verdict, CountVerdict::Accepted))
        .collect();
    let names: Vec<String> = (0..accepted.len())
        .map(|index| format!("count-accepted-{index}"))
        .collect();
    let connectors = running_connectors(&emulator, &plugin_arn, &names).await;
    for (row, connector) in accepted.iter().zip(&connectors) {
        let version = text(&describe(&emulator, connector).await, "currentVersion");
        let (status, updated) = update(
            &emulator,
            connector,
            &version,
            json!({"capacity": row.capacity}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: counts the service allocates must be accepted, got {status} {updated}",
            row.why
        );
    }
    past_the_whole_update().await;

    for (row, connector) in accepted.iter().zip(&connectors) {
        let after = describe(&emulator, connector).await;
        let reported = numbers(&after["capacity"]);
        // Every number the request named must read back from state. A member the request
        // left out takes the modelled default, which the description reports as zero, so
        // the requested numbers are a subset rather than the whole map.
        for (member, value) in numbers(&row.capacity) {
            assert_eq!(
                reported.get(&member),
                Some(&value),
                "case {:?}: the accepted {member} must read back as {value}, got {after}",
                row.why
            );
        }
    }
}
