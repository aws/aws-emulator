// SPDX-License-Identifier: CC0-1.0

//! `DescribeConnectorOperation`, end to end over HTTP.
//!
//! RULE. `DescribeConnectorOperation` reports the ONE stored connector operation its
//! `connectorOperationArn` names and nothing else: the connector that operation ran
//! against, the kind of work it is, the state the CLOCK has walked it to, the steps and
//! the origin/target settings pair of that kind of work, its creation time, and an
//! `endTime` only once it has reached its terminal state. Being a read
//! (`smithy_query query="operation" operation="DescribeConnectorOperation"` reports it
//! `@readonly`) it changes nothing a later caller sees: it neither advances the operation
//! nor alters, adds or drops a record. An ARN this emulator holds no connector operation
//! of — including one naming a resource of another kind — is the modelled
//! `NotFoundException` (HTTP 404) blaming `connectorOperationArn`.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response body read
//! back as JSON. Nothing reaches into the state struct, so a handler answering from the
//! request, or from a fixed value, instead of from state would fail here.
//!
//! The tables span what the rule generalises over: every kind of call this service has
//! that records an operation (a restart, an update of either of a connector's two
//! updatable settings), all three stored side by side on ONE connector so that a handler
//! reporting "the connector's latest operation" rather than the one its ARN names fails;
//! the whole life of one operation, sampled far more often than the clock moves it; and
//! every form of an ARN that names no stored operation, including the ARNs of this
//! service's three other resource kinds.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
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

/// The states a restart's operation passes through, in order, after the `PENDING` every
/// operation starts in: the two restart states `ConnectorOperationState` lists that are
/// not a failure (`smithy_query query="shape" shape="ConnectorOperationState"`), the last
/// of which is terminal.
const RESTART_STATES: &[&str] = &["RESTART_IN_PROGRESS", "RESTART_COMPLETE"];

/// The states an update's operation passes through, in order, after `PENDING`: one per
/// step of the update, the last of which is terminal.
const UPDATE_STATES: &[&str] = &[
    "UPDATE_IN_PROGRESS",
    "UPDATE_IN_PROGRESS",
    "UPDATE_IN_PROGRESS",
    "UPDATE_COMPLETE",
];

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
/// `FINALIZE_UPDATE` — the three an update of either kind shares around the one that names
/// the change itself, which is spelled the same as the operation's own type.
fn update_step_types(operation_type: &str) -> Vec<String> {
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

/// Long enough that every state of a progression `states` long is due: one span per
/// state, with the span taken from the service rather than from a literal.
async fn past_a_whole_progression(states: usize) {
    let states = u32::try_from(states).expect("a small progression");
    tokio::time::sleep(TRANSITION_SPAN * states + Duration::from_millis(500)).await;
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
/// `:` and `/`) is escaped. Encoded with the same `percent-encoding` crate the server
/// side uses, rather than a second encoder.
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

/// Every number `value` carries, behind the path it sits at, so a comparison of two
/// capacities names the member that differs instead of printing two blobs. `CapacityUpdate`
/// and `CapacityDescription` name their members identically, so what an update asked for
/// and what a report carries compare directly.
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
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The capacity every connector in this suite is created with.
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

/// The capacity the capacity row updates to: the auto-scaled form, so the row is a change
/// of FORM and not only of value.
fn target_capacity() -> Value {
    json!({"autoScaling": {
        "maxWorkerCount": 4,
        "mcuCount": 1,
        "minWorkerCount": 1,
        "scaleInPolicy": {"cpuUtilizationPercentage": 20},
        "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
        // "Must be at least equal to maxWorkerCount"
        // (`AutoScalingUpdate$maxAutoscalingTaskCount`).
        "maxAutoscalingTaskCount": 4,
    }})
}

/// The configuration the configuration row updates to.
fn target_configuration() -> Value {
    json!({
        "connector.class": "io.confluent.connect.s3.S3SinkConnector",
        "tasks.max": "4",
        "flush.size": "1000",
    })
}

/// The `WorkerSetting` a connector this suite created runs with, at `capacity`.
///
/// `WorkerSetting` carries a connector's capacity, "which plugins the connector uses" and
/// "which worker configuration the connector uses", beside the flag that says whether it
/// uses the default one (`smithy_query(query="shape", shape="WorkerSetting")`). Every
/// connector here is created on one plugin at revision 1 and names no `workerConfiguration`,
/// so it runs on the default worker configuration.
fn worker_setting(capacity: Value, plugin_arn: &str) -> Value {
    json!({
        "capacity": capacity,
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "useDefaultWorkerConfiguration": true,
    })
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
    let (status, created) = call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    past_one_transition().await;
    text(&created, "customPluginArn")
}

/// Create one connector and return its ARN, once the clock has moved it to `RUNNING`.
async fn running_connector(
    emulator: &MultiServiceRouter,
    plugin_arn: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/connectors",
        connector_request(name, plugin_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnector {name} must succeed, got {status} {created}"
    );
    past_one_transition().await;
    text(&created, "connectorArn")
}

/// `DescribeConnector` for `connector_arn`.
async fn describe_connector(emulator: &MultiServiceRouter, connector_arn: &str) -> Value {
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

/// `DescribeConnectorOperation` for `operation_arn`, as `(status, body)`.
async fn describe_operation(emulator: &MultiServiceRouter, operation_arn: &str) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v1/connectorOperations/{}", label(operation_arn)),
        Value::Null,
    )
    .await
}

/// `DescribeConnectorOperation` for `operation_arn`, asserting it succeeded.
async fn described_operation(emulator: &MultiServiceRouter, operation_arn: &str) -> Value {
    let (status, described) = describe_operation(emulator, operation_arn).await;
    assert_eq!(
        status, 200,
        "DescribeConnectorOperation must succeed, got {status} {described}"
    );
    described
}

/// The summaries `ListConnectorOperations` answers with for `connector_arn`.
async fn listed_operations(emulator: &MultiServiceRouter, connector_arn: &str) -> Vec<Value> {
    let (status, listed) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}/operations", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListConnectorOperations must succeed, got {status} {listed}"
    );
    listed["connectorOperations"]
        .as_array()
        .unwrap_or_else(|| panic!("a connectorOperations array in {listed}"))
        .clone()
}

/// The `(stepType, stepState)` pairs a described operation reports, in order; empty when
/// it reports no `operationSteps` at all.
fn reported_steps(described: &Value) -> Vec<(String, String)> {
    described
        .get("operationSteps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .map(|step| (text(step, "stepType"), text(step, "stepState")))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the table: a kind of call that records a connector operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `RestartConnector`: changes neither of the connector's updatable settings.
    Restart,
    /// `UpdateConnector` naming `capacity`, which is the connector's worker setting.
    UpdateCapacity,
    /// `UpdateConnector` naming `connectorConfiguration`.
    UpdateConfiguration,
}

/// Every kind of call this service has that records a connector operation.
const KINDS: &[Kind] = &[Kind::Restart, Kind::UpdateCapacity, Kind::UpdateConfiguration];

impl Kind {
    /// The connector this row drives, one per row so a row's own operation is the only
    /// one its connector has.
    fn connector(self) -> &'static str {
        match self {
            Self::Restart => "described-restart",
            Self::UpdateCapacity => "described-capacity-update",
            Self::UpdateConfiguration => "described-configuration-update",
        }
    }

    /// The `connectorOperationType` the recorded operation must report.
    fn operation_type(self) -> &'static str {
        match self {
            Self::Restart => "RESTART_CONNECTOR",
            Self::UpdateCapacity => UPDATE_WORKER_SETTING,
            Self::UpdateConfiguration => UPDATE_CONFIGURATION,
        }
    }

    /// The states the operation walks after `PENDING`, in order.
    fn states(self) -> &'static [&'static str] {
        match self {
            Self::Restart => RESTART_STATES,
            Self::UpdateCapacity | Self::UpdateConfiguration => UPDATE_STATES,
        }
    }

    /// The `connectorOperationState` the operation ends in.
    fn terminal_state(self) -> &'static str {
        self.states().last().expect("a non-empty progression")
    }

    /// The `operationSteps` the operation must report, in order: a restart has no value
    /// of `ConnectorOperationStepType` to report, since every one of them names a step of
    /// an update.
    fn step_types(self) -> Vec<String> {
        match self {
            Self::Restart => Vec::new(),
            Self::UpdateCapacity | Self::UpdateConfiguration => {
                update_step_types(self.operation_type())
            }
        }
    }

    /// The `(origin, target)` settings pair the operation must report, behind the member
    /// each sits at: `None` for a kind of work that changes no setting.
    fn settings_pair(self, plugin_arn: &str) -> Option<(&'static str, &'static str, Value, Value)> {
        match self {
            Self::Restart => None,
            Self::UpdateCapacity => Some((
                "originWorkerSetting",
                "targetWorkerSetting",
                worker_setting(created_capacity(), plugin_arn),
                worker_setting(target_capacity(), plugin_arn),
            )),
            Self::UpdateConfiguration => Some((
                "originConnectorConfiguration",
                "targetConnectorConfiguration",
                created_configuration(),
                target_configuration(),
            )),
        }
    }

    /// Start this kind of work against `connector_arn` and return the ARN of the operation
    /// it recorded.
    async fn start(self, emulator: &MultiServiceRouter, connector_arn: &str) -> String {
        let (status, started) = match self {
            Self::Restart => {
                call(
                    emulator,
                    "POST",
                    &format!("/v1/connectors/{}/restart", label(connector_arn)),
                    Value::Null,
                )
                .await
            }
            Self::UpdateCapacity | Self::UpdateConfiguration => {
                let current_version =
                    text(&describe_connector(emulator, connector_arn).await, "currentVersion");
                let body = if self == Self::UpdateCapacity {
                    json!({"capacity": target_capacity()})
                } else {
                    json!({"connectorConfiguration": target_configuration()})
                };
                call(
                    emulator,
                    "PUT",
                    &format!(
                        "/v1/connectors/{}?currentVersion={}",
                        label(connector_arn),
                        label(&current_version)
                    ),
                    body,
                )
                .await
            }
        };
        assert_eq!(
            status, 200,
            "case {self:?}: the call that records an operation must succeed, got {status} \
             {started}"
        );
        let operation_arn = text(&started, "connectorOperationArn");
        assert!(
            !operation_arn.is_empty(),
            "case {self:?}: the call must answer with the ARN of the operation it recorded, \
             got {started}"
        );
        operation_arn
    }
}

/// Assert `described` is the report of a `kind` operation against `connector_arn`, with
/// the pair of settings and the step types that kind of work carries.
fn assert_identity(kind: Kind, connector_arn: &str, plugin_arn: &str, described: &Value) {
    assert_eq!(
        text(described, "connectorOperationType"),
        kind.operation_type(),
        "case {kind:?}: the operation must report the kind of work it is, got {described}"
    );
    assert_eq!(
        text(described, "connectorArn"),
        connector_arn,
        "case {kind:?}: the operation must report the connector it ran against, got \
         {described}"
    );
    assert_eq!(
        reported_steps(described)
            .iter()
            .map(|(step_type, _)| step_type.clone())
            .collect::<Vec<String>>(),
        kind.step_types(),
        "case {kind:?}: the operation must report the steps of its kind of work, got \
         {described}"
    );
    assert!(
        described.get("errorInfo").is_none(),
        "case {kind:?}: an operation nothing failed reports no errorInfo, got {described}"
    );

    match kind.settings_pair(plugin_arn) {
        None => {
            for member in [
                "originWorkerSetting",
                "targetWorkerSetting",
                "originConnectorConfiguration",
                "targetConnectorConfiguration",
            ] {
                assert!(
                    described.get(member).is_none(),
                    "case {kind:?}: work that changes no setting reports no {member}, got \
                     {described}"
                );
            }
        }
        Some((origin_member, target_member, origin, target)) => {
            for (member, expected) in [(origin_member, &origin), (target_member, &target)] {
                let reported = described
                    .get(member)
                    .unwrap_or_else(|| panic!("case {kind:?}: a {member} in {described}"));
                assert_eq!(
                    numbers(reported),
                    numbers(expected),
                    "case {kind:?}: {member} must carry the settings of its side of the \
                     change, got {reported}"
                );
                assert_eq!(
                    strings(reported),
                    strings(expected),
                    "case {kind:?}: {member} must carry the settings of its side of the \
                     change, got {reported}"
                );
            }
            // The pair belongs to the kind of setting that changes: the other kind's pair
            // is not reported at all.
            for member in [
                "originWorkerSetting",
                "targetWorkerSetting",
                "originConnectorConfiguration",
                "targetConnectorConfiguration",
            ] {
                if member != origin_member && member != target_member {
                    assert!(
                        described.get(member).is_none(),
                        "case {kind:?}: only the pair of the setting that changes is \
                         reported, got {member} in {described}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn every_kind_of_stored_operation_describes_from_its_own_record() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "described-operation-plugin").await;

    // Start one operation of every kind, each on its own connector, and read each back
    // while it is still running.
    let mut started: Vec<(Kind, String, String, String)> = Vec::new();
    for kind in KINDS {
        let kind = *kind;
        let connector_arn = running_connector(&emulator, &plugin_arn, kind.connector()).await;
        let operation_arn = kind.start(&emulator, &connector_arn).await;

        let described = described_operation(&emulator, &operation_arn).await;
        assert_eq!(
            text(&described, "connectorOperationArn"),
            operation_arn,
            "case {kind:?}: the report must name the operation that was asked for, got \
             {described}"
        );
        assert_identity(kind, &connector_arn, &plugin_arn, &described);

        let state = text(&described, "connectorOperationState");
        assert!(
            state == "PENDING" || state == kind.states()[0],
            "case {kind:?}: a just-started operation is PENDING or in its first state \
             {:?}, got {state:?}",
            kind.states()[0]
        );
        assert!(
            described.get("endTime").is_none(),
            "case {kind:?}: an operation that has not reached its terminal state carries no \
             endTime, got {described}"
        );
        let creation_time = text(&described, "creationTime");
        assert!(
            !creation_time.is_empty(),
            "case {kind:?}: the operation must report its creationTime, got {described}"
        );
        // No step has completed out of turn: nothing has begun at all while the operation
        // is still PENDING.
        if state == "PENDING" {
            assert!(
                reported_steps(&described)
                    .iter()
                    .all(|(_, step_state)| step_state == "PENDING"),
                "case {kind:?}: an operation that has not left PENDING is working no step, \
                 got {described}"
            );
        }

        started.push((kind, connector_arn, operation_arn, creation_time));
    }

    past_a_whole_progression(UPDATE_STATES.len()).await;

    // With all three stored side by side, each ARN still reports ITS OWN record: a
    // handler answering with the newest operation, or with the one of the connector the
    // last call named, reports the wrong kind of work here.
    for (kind, connector_arn, operation_arn, creation_time) in &started {
        let described = described_operation(&emulator, operation_arn).await;
        assert_eq!(
            text(&described, "connectorOperationArn"),
            *operation_arn,
            "case {kind:?}: the report must name the operation that was asked for, got \
             {described}"
        );
        assert_identity(*kind, connector_arn, &plugin_arn, &described);
        assert_eq!(
            text(&described, "connectorOperationState"),
            kind.terminal_state(),
            "case {kind:?}: the clock must walk the operation to its terminal state, got \
             {described}"
        );
        assert_eq!(
            text(&described, "creationTime"),
            *creation_time,
            "case {kind:?}: an operation's creationTime must not move, got {described}"
        );
        let end_time = text(&described, "endTime");
        assert!(
            !end_time.is_empty() && end_time >= *creation_time,
            "case {kind:?}: an operation that has ended reports an endTime at or after its \
             creationTime {creation_time:?}, got {end_time:?}"
        );
        assert!(
            reported_steps(&described)
                .iter()
                .all(|(_, step_state)| step_state == "COMPLETED"),
            "case {kind:?}: every step of an operation that has ended has completed, got \
             {described}"
        );
    }

    // Every ARN is its own: three distinct records, one per call.
    let arns: std::collections::BTreeSet<&String> =
        started.iter().map(|(_, _, arn, _)| arn).collect();
    assert_eq!(
        arns.len(),
        started.len(),
        "each call must record its own operation, got {arns:?}"
    );
}

#[tokio::test]
async fn describing_an_operation_changes_nothing_a_later_caller_sees() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "read-only-plugin").await;
    let connector_arn = running_connector(&emulator, &plugin_arn, "read-only-connector").await;

    let started_at = Instant::now();
    let operation_arn = Kind::Restart.start(&emulator, &connector_arn).await;

    // Poll the operation far more often than the clock moves it. Reads neither advance it
    // nor move it backwards: the state only ever walks the progression forwards, and the
    // terminal state is not reached before the clock owes it.
    let progression: Vec<&str> = std::iter::once("PENDING")
        .chain(RESTART_STATES.iter().copied())
        .collect();
    let rank = |state: &str| -> usize {
        progression
            .iter()
            .position(|known| *known == state)
            .unwrap_or_else(|| panic!("a state of the restart progression, got {state:?}"))
    };

    let mut highest = 0;
    let mut polls = 0_u32;
    let mut seen: Vec<String> = Vec::new();
    let deadline = TRANSITION_SPAN * 12;
    loop {
        let described = described_operation(&emulator, &operation_arn).await;
        let state = text(&described, "connectorOperationState");
        let at = rank(&state);
        assert!(
            at >= highest,
            "poll {polls}: an operation's state only moves forwards, saw {seen:?} then \
             {state:?}"
        );
        highest = at;
        seen.push(state.clone());
        polls += 1;

        let ended = state == Kind::Restart.terminal_state();
        assert_eq!(
            described.get("endTime").is_some(),
            ended,
            "poll {polls}: endTime is reported exactly once the operation has reached its \
             terminal state (state {state:?}), got {described}"
        );
        if ended {
            break;
        }
        assert!(
            started_at.elapsed() < deadline,
            "the restart must reach its terminal state within {deadline:?}, saw {seen:?}"
        );
        tokio::time::sleep(TRANSITION_SPAN / 8).await;
    }

    assert!(
        polls > u32::try_from(RESTART_STATES.len()).expect("a small progression"),
        "the operation must be sampled more often than the clock moves it, got {polls} polls"
    );
    assert!(
        started_at.elapsed()
            >= TRANSITION_SPAN * u32::try_from(RESTART_STATES.len()).expect("a small progression"),
        "reading an operation must not bring its end forward: it ended after {:?}, sooner \
         than the clock owes it, having seen {seen:?}",
        started_at.elapsed()
    );

    // Two further reads answer exactly the same bytes, and the record is still the one
    // record the connector has.
    let once = described_operation(&emulator, &operation_arn).await;
    let twice = described_operation(&emulator, &operation_arn).await;
    assert_eq!(
        once, twice,
        "reading an ended operation twice must answer the same report, got {once} then \
         {twice}"
    );
    let summaries = listed_operations(&emulator, &connector_arn).await;
    assert_eq!(
        summaries.len(),
        1,
        "reading an operation must neither add nor drop one, got {summaries:?}"
    );
    assert_eq!(
        text(&summaries[0], "connectorOperationArn"),
        operation_arn,
        "the one listed operation must be the one that was read, got {summaries:?}"
    );
    assert_eq!(
        text(&summaries[0], "connectorOperationState"),
        text(&once, "connectorOperationState"),
        "both read paths must report the same stored state, got {summaries:?} and {once}"
    );
    assert_eq!(
        text(&summaries[0], "endTime"),
        text(&once, "endTime"),
        "both read paths must report the same stored endTime, got {summaries:?} and {once}"
    );
}

#[tokio::test]
async fn an_arn_naming_no_stored_operation_is_the_modelled_not_found() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "not-found-plugin").await;
    let connector_arn = running_connector(&emulator, &plugin_arn, "not-found-connector").await;
    let operation_arn = Kind::Restart.start(&emulator, &connector_arn).await;

    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/worker-configurations",
        json!({"name": "not-found-worker-configuration", "propertiesFileContent": "QUJD"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWorkerConfiguration must succeed, got {status} {created}"
    );
    let worker_configuration_arn = text(&created, "workerConfigurationArn");

    // The id in an operation ARN's last position, replaced with one of the same form that
    // was never minted: the UUID AWS gives the random part of a kafkaconnect ARN.
    let (prefix, _) = operation_arn
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("an operation ARN with its own id, got {operation_arn:?}"));
    let never_minted = format!("{prefix}/00000000-0000-4000-8000-000000000000");

    // Every form of a `connectorOperationArn` this emulator holds no operation of: the ARN
    // of each of this service's other resource kinds, an operation ARN of the right shape
    // that was never minted, the same ARN in another region and another account, and an
    // operation ARN whose resource field names another kind. Every row is a WELL-FORMED
    // kafkaconnect ARN, so each names a resource that is absent; a value that is not an
    // ARN at all is incorrect input instead, which `arn_label_refusals` pins.
    let absent = [
        ("a connector's own ARN", connector_arn.clone()),
        ("a custom plugin's ARN", plugin_arn.clone()),
        (
            "a worker configuration's ARN",
            worker_configuration_arn.clone(),
        ),
        ("an operation id never minted", never_minted),
        (
            "the operation's ARN in another region",
            operation_arn.replace(REGION, "us-west-2"),
        ),
        (
            "the operation's ARN in another account",
            operation_arn.replace(ACCOUNT, "123456789012"),
        ),
        (
            "a kafkaconnect ARN of no resource kind this service mints",
            format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:not-an-operation"),
        ),
    ];

    for (case, arn) in &absent {
        let (status, answered) = describe_operation(&emulator, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an ARN naming no stored operation is a 404, got {status} \
             {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
        // `Invalid parameter <member>: <reason>`, the form this service's own recorded
        // rejections are written in, blaming the member the request carried.
        assert_eq!(
            text(&answered, "message"),
            "Invalid parameter connectorOperationArn: The requested resource doesn\u{2019}t \
             exist.",
            "case {case:?}: the message must blame connectorOperationArn, got {answered}"
        );
        // A refusal reports no resource: not the one asked for, and not another kind's.
        for member in ["connectorOperationArn", "connectorArn", "operationSteps"] {
            assert!(
                answered.get(member).is_none(),
                "case {case:?}: a refusal must report no {member}, got {answered}"
            );
        }
    }

    // The refusal is not blanket: the operation this emulator does hold still describes.
    let described = described_operation(&emulator, &operation_arn).await;
    assert_eq!(
        text(&described, "connectorArn"),
        connector_arn,
        "the stored operation must still describe, got {described}"
    );
}
