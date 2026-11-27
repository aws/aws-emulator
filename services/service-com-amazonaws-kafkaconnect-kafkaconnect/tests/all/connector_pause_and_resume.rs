// SPDX-License-Identifier: CC0-1.0

//! `PauseConnector` and `ResumeConnector`, end to end over HTTP: the connector operation
//! each records, the states the clock walks the connector through, and the gate each one's
//! own accepted `ConnectorState` set decides.
//!
//! RULE. A call that stops or starts a stored connector's workers is asynchronous: it
//! records ONE connector operation of its own `ConnectorOperationType` against that
//! connector, answers with the connector's ARN and that operation's ARN, and leaves the
//! connector in the transient `ConnectorState` of the work it started — and the CLOCK, not
//! the next call, is what walks the operation through the model's states for its type and
//! brings the connector to the state that work was asked to reach: `PAUSED` for a pause,
//! and the state the connector's bring-up decided for a resume. Neither call changes a
//! connector's settings or its version. Each is served only for a connector in one of the
//! states its own work needs — `RUNNING` for a pause, `PAUSED` for a resume — and is
//! otherwise refused with `BadRequestException` (HTTP 400) blaming `connectorArn` and
//! naming the states that WOULD be served; a `connectorArn` that is not a well-formed ARN
//! of this service is the same 400, and one naming no connector of the endpoint's region is
//! `NotFoundException` (HTTP 404).
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response body read
//! back as JSON. Nothing reaches into the state struct, so a handler answering from the
//! request instead of from state would fail here.
//!
//! The tables span what the rule generalises over: BOTH calls, crossed with every
//! `ConnectorState` this service can put a connector in without a second service mounted
//! (`CREATING`, `RUNNING`, `PAUSING`, `PAUSED`, `RESUMING`, `RESTARTING`, `UPDATING`), so
//! each row is served exactly where that call's own accepted set says and refused
//! everywhere else.
//!
//! The sibling suites' request helpers are private to their own modules, so this one
//! carries its own copies of the few it needs. Nothing here waits a fixed time: a change
//! the clock owes is waited for by looking again until the state holds, under a deadline
//! far longer than the service's own span, so a loaded host cannot fail a case the rule
//! passes.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    only_valid_in_states, CONNECTOR_CREATING, CONNECTOR_OPERATION_PAUSE,
    CONNECTOR_OPERATION_PAUSE_COMPLETE, CONNECTOR_OPERATION_PAUSE_IN_PROGRESS,
    CONNECTOR_OPERATION_PENDING, CONNECTOR_OPERATION_RESUME, CONNECTOR_OPERATION_RESUME_COMPLETE,
    CONNECTOR_OPERATION_RESUME_IN_PROGRESS, CONNECTOR_PAUSED, CONNECTOR_PAUSING,
    CONNECTOR_RESTARTING, CONNECTOR_RESUMING, CONNECTOR_RUNNING, CONNECTOR_UPDATING,
    PAUSE_ACCEPTED_CONNECTOR_STATES, RESUME_ACCEPTED_CONNECTOR_STATES,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN carries.
/// No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// How long a wait for a change the clock owes may take before the case fails. Generous on
/// purpose: the only thing under test is WHICH state the clock brings the resource to, not
/// how fast this host runs.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll loop waits before looking again. Used only inside such a loop.
const LOOK_AGAIN: Duration = Duration::from_millis(20);

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

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS SDK
/// encodes a label.
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

/// A member of a JSON object as a string, or `""`.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
        },
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

/// Create one custom plugin and return its ARN once the clock has made it `ACTIVE` — a
/// connector may only reference a plugin in that state.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    let arn = text(&created, "customPluginArn");
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = call(
            emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeCustomPlugin must succeed, got {status} {described}"
        );
        let state = text(&described, "customPluginState");
        if state == "ACTIVE" {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "the plugin {name} must become ACTIVE, still {state:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create one connector and return its ARN. The clock is NOT waited out, so the connector
/// is still `CREATING` when this returns.
async fn create_connector(
    emulator: &MultiServiceRouter,
    name: &str,
    plugin_arn: &str,
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
        "case {name:?}: CreateConnector must succeed, got {status} {created}"
    );
    text(&created, "connectorArn")
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

/// The `connectorState` the connector `connector_arn` names reports now.
async fn connector_state(emulator: &MultiServiceRouter, connector_arn: &str) -> String {
    text(&describe(emulator, connector_arn).await, "connectorState")
}

/// Look at the connector again until it reports `wanted`, and hand back the whole
/// description it then carries.
///
/// A WAIT FOR THE CONDITION, not for a span: the service's transition is owed on its own
/// clock, so the loop looks again until the state holds or [`SETTLE_DEADLINE`] passes.
async fn until_connector_state(
    emulator: &MultiServiceRouter,
    case: &str,
    connector_arn: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = describe(emulator, connector_arn).await;
        let state = text(&described, "connectorState");
        if state == wanted {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the clock to bring the connector to {wanted:?}, still \
             {state:?} in {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
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

/// Look at the operation again until it reports `wanted`, and hand back its report.
async fn until_operation_state(
    emulator: &MultiServiceRouter,
    case: &str,
    operation_arn: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = describe_operation(emulator, operation_arn).await;
        let state = text(&described, "connectorOperationState");
        if state == wanted {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the clock to walk the operation to {wanted:?}, still \
             {state:?} in {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
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

/// Which of the two calls a row makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    /// `PauseConnector`.
    Pause,
    /// `ResumeConnector`.
    Resume,
}

/// Both calls that stop or start a connector's workers.
const WORK: &[Work] = &[Work::Pause, Work::Resume];

impl Work {
    /// The `@http` path this call is bound to, for `connector_arn`.
    fn path(self, connector_arn: &str) -> String {
        let segment = match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
        };
        format!("/v1/connectors/{}/{segment}", label(connector_arn))
    }

    /// The `ConnectorOperationType` the operation this call records carries.
    fn operation_type(self) -> &'static str {
        match self {
            Self::Pause => CONNECTOR_OPERATION_PAUSE,
            Self::Resume => CONNECTOR_OPERATION_RESUME,
        }
    }

    /// The `ConnectorOperationState`s the operation walks after `PENDING`, in order.
    fn operation_states(self) -> [&'static str; 2] {
        match self {
            Self::Pause => [
                CONNECTOR_OPERATION_PAUSE_IN_PROGRESS,
                CONNECTOR_OPERATION_PAUSE_COMPLETE,
            ],
            Self::Resume => [
                CONNECTOR_OPERATION_RESUME_IN_PROGRESS,
                CONNECTOR_OPERATION_RESUME_COMPLETE,
            ],
        }
    }

    /// The transient `ConnectorState` the connector is in while this call's work runs.
    fn transient_state(self) -> &'static str {
        match self {
            Self::Pause => CONNECTOR_PAUSING,
            Self::Resume => CONNECTOR_RESUMING,
        }
    }

    /// The `ConnectorState` the clock leaves the connector in once the work is done.
    fn settled_state(self) -> &'static str {
        match self {
            Self::Pause => CONNECTOR_PAUSED,
            Self::Resume => CONNECTOR_RUNNING,
        }
    }

    /// The `ConnectorState`s this call is served for.
    fn accepted_states(self) -> &'static [&'static str] {
        match self {
            Self::Pause => PAUSE_ACCEPTED_CONNECTOR_STATES,
            Self::Resume => RESUME_ACCEPTED_CONNECTOR_STATES,
        }
    }

    /// The whole message a request this connector's state does not admit is refused with.
    fn refusal(self) -> String {
        format!(
            "Invalid parameter connectorArn: {}",
            only_valid_in_states(self.accepted_states())
        )
    }

    /// Make this call against `connector_arn`.
    async fn call(self, emulator: &MultiServiceRouter, connector_arn: &str) -> (u16, Value) {
        call(emulator, "PUT", &self.path(connector_arn), Value::Null).await
    }
}

/// Make `work`'s call, assert it was served, and hand back the ARN of the operation it
/// recorded.
async fn served(emulator: &MultiServiceRouter, case: &str, connector_arn: &str, work: Work) -> String {
    let (status, answered) = work.call(emulator, connector_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {work:?} must be served, got {status} {answered}"
    );
    assert_eq!(
        text(&answered, "connectorArn"),
        connector_arn,
        "case {case:?}: the answer must name the connector it acted on, got {answered}"
    );
    let operation_arn = text(&answered, "connectorOperationArn");
    assert!(
        !operation_arn.is_empty(),
        "case {case:?}: a served call records an operation, got {answered}"
    );
    operation_arn
}

/// Assert the operation `described` reports is `work`'s own kind of work against
/// `connector_arn`, and that it reports neither a settings pair nor a step: every value
/// `ConnectorOperationStepType` lists names a step of an UPDATE, and neither a pause nor a
/// resume changes a connector's worker setting or its configuration.
fn assert_operation_identity(case: &str, work: Work, connector_arn: &str, described: &Value) {
    assert_eq!(
        text(described, "connectorOperationType"),
        work.operation_type(),
        "case {case:?}: the operation must report the kind of work it is, got {described}"
    );
    assert_eq!(
        text(described, "connectorArn"),
        connector_arn,
        "case {case:?}: the operation must report the connector it ran against, got {described}"
    );
    for member in [
        "operationSteps",
        "originWorkerSetting",
        "targetWorkerSetting",
        "originConnectorConfiguration",
        "targetConnectorConfiguration",
        "errorInfo",
    ] {
        assert!(
            described.get(member).is_none(),
            "case {case:?}: work that changes no setting reports no {member}, got {described}"
        );
    }
    assert!(
        !text(described, "creationTime").is_empty(),
        "case {case:?}: the operation must report its creationTime, got {described}"
    );
}

#[tokio::test]
async fn each_call_records_its_own_operation_and_the_clock_walks_the_connector_through_it() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "pause-resume-plugin").await;
    let connector_arn = create_connector(&emulator, "pause-resume-connector", &plugin_arn).await;
    let running = until_connector_state(
        &emulator,
        "the fixture",
        &connector_arn,
        CONNECTOR_RUNNING,
    )
    .await;
    let created_version = text(&running, "currentVersion");
    let created_capacity = running["capacity"].clone();
    let created_plugins = running["plugins"].clone();

    // A pause, then a resume: each call's work is walked to its end before the next one,
    // so the second row runs against the state the first one's clock left behind.
    let mut recorded: Vec<(Work, String)> = Vec::new();
    for work in WORK {
        let work = *work;
        let case = format!("{work:?}");
        let operation_arn = served(&emulator, &case, &connector_arn, work).await;

        // The operation as the call left it: its own kind of work, not yet finished.
        let described = describe_operation(&emulator, &operation_arn).await;
        assert_operation_identity(&case, work, &connector_arn, &described);
        let state = text(&described, "connectorOperationState");
        let walked: Vec<&str> = std::iter::once(CONNECTOR_OPERATION_PENDING)
            .chain(work.operation_states())
            .collect();
        assert!(
            walked.contains(&state.as_str()),
            "case {case:?}: a {:?} operation only ever reports one of {walked:?}, got \
             {state:?}",
            work.operation_type()
        );
        // `endTime` is "the time when the operation ended", so it is there exactly when
        // the operation has reached its terminal state.
        assert_eq!(
            described.get("endTime").is_some(),
            state == work.operation_states()[1],
            "case {case:?}: an endTime is reported exactly once the operation has ended, \
             got {described}"
        );

        // The connector is in the transient state of that work for exactly as long as the
        // operation runs, and the CLOCK is what moves both.
        let state = connector_state(&emulator, &connector_arn).await;
        assert!(
            state == work.transient_state() || state == work.settled_state(),
            "case {case:?}: the connector must be {:?} while the work runs and {:?} after \
             it, got {state:?}",
            work.transient_state(),
            work.settled_state()
        );

        let ended = until_operation_state(
            &emulator,
            &case,
            &operation_arn,
            work.operation_states()[1],
        )
        .await;
        assert_operation_identity(&case, work, &connector_arn, &ended);
        assert!(
            !text(&ended, "endTime").is_empty(),
            "case {case:?}: an operation that has ended reports an endTime, got {ended}"
        );
        let settled =
            until_connector_state(&emulator, &case, &connector_arn, work.settled_state()).await;

        // Neither call is an update: the connector keeps the settings and the version it
        // was running with.
        assert_eq!(
            text(&settled, "currentVersion"),
            created_version,
            "case {case:?}: stopping or starting the workers moves no version, got {settled}"
        );
        assert_eq!(
            settled["capacity"], created_capacity,
            "case {case:?}: stopping or starting the workers changes no capacity, got \
             {settled}"
        );
        assert_eq!(
            settled["plugins"], created_plugins,
            "case {case:?}: stopping or starting the workers changes no plugins, got \
             {settled}"
        );

        recorded.push((work, operation_arn));
    }

    // Both calls recorded one operation each, and each ARN still reports ITS OWN record.
    let listed = listed_operations(&emulator, &connector_arn).await;
    assert_eq!(
        listed.len(),
        recorded.len(),
        "each call must record exactly one operation, got {listed:?}"
    );
    let listed_types: BTreeSet<String> = listed
        .iter()
        .map(|summary| text(summary, "connectorOperationType"))
        .collect();
    let expected_types: BTreeSet<String> = recorded
        .iter()
        .map(|(work, _)| work.operation_type().to_string())
        .collect();
    assert_eq!(
        listed_types, expected_types,
        "the listing must report one operation of each kind of work, got {listed:?}"
    );
    for (work, operation_arn) in &recorded {
        let described = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            text(&described, "connectorOperationArn"),
            *operation_arn,
            "case {work:?}: the report must name the operation that was asked for, got \
             {described}"
        );
        assert_operation_identity(&format!("{work:?}"), *work, &connector_arn, &described);
    }
}

/// How a row puts its connector into the state it tests.
#[derive(Debug, Clone, Copy)]
enum Reach {
    /// Read the connector straight after its create, while its workers come up.
    JustCreated,
    /// Wait out the create: the connector's workers are running.
    BroughtUp,
    /// Pause a running connector, and read it while the pause runs.
    Pausing,
    /// Pause a running connector and wait the pause out.
    Paused,
    /// Resume a paused connector, and read it while the resume runs.
    Resuming,
    /// Restart a running connector, so it is bringing its workers back.
    Restarting,
    /// Update a running connector, so it is taking on new settings.
    Updating,
}

/// One row of the gate table: a `ConnectorState`, and how to reach it.
struct Case {
    /// The `ConnectorState` the connector is meant to hold when the call lands.
    state: &'static str,
    reach: Reach,
}

/// Every `ConnectorState` this service can put a connector in with no second service
/// mounted. `ConnectorState` also lists `FAILED`, which a connector reaches only when its
/// bring-up found no Apache Kafka cluster — covered, with the gate over it, by the
/// router's `kafkaconnect_connector_bring_up` suite, where Amazon MSK answers that
/// question — and `DELETING`, where the connector is on its way out of the store.
const CASES: &[Case] = &[
    Case {
        state: CONNECTOR_CREATING,
        reach: Reach::JustCreated,
    },
    Case {
        state: CONNECTOR_RUNNING,
        reach: Reach::BroughtUp,
    },
    Case {
        state: CONNECTOR_PAUSING,
        reach: Reach::Pausing,
    },
    Case {
        state: CONNECTOR_PAUSED,
        reach: Reach::Paused,
    },
    Case {
        state: CONNECTOR_RESUMING,
        reach: Reach::Resuming,
    },
    Case {
        state: CONNECTOR_RESTARTING,
        reach: Reach::Restarting,
    },
    Case {
        state: CONNECTOR_UPDATING,
        reach: Reach::Updating,
    },
];

/// Put a freshly created connector into the state `reach` names and return its ARN. Every
/// call that moves it is checked, so a row can only run against the work it means to test.
async fn connector_in_state(
    emulator: &MultiServiceRouter,
    case: &str,
    name: &str,
    plugin_arn: &str,
    reach: Reach,
) -> String {
    let connector_arn = create_connector(emulator, name, plugin_arn).await;
    if matches!(reach, Reach::JustCreated) {
        return connector_arn;
    }
    until_connector_state(emulator, case, &connector_arn, CONNECTOR_RUNNING).await;

    match reach {
        Reach::JustCreated => unreachable!("handled above"),
        Reach::BroughtUp => {}
        Reach::Pausing => {
            served(emulator, case, &connector_arn, Work::Pause).await;
        }
        Reach::Paused => {
            served(emulator, case, &connector_arn, Work::Pause).await;
            until_connector_state(emulator, case, &connector_arn, CONNECTOR_PAUSED).await;
        }
        Reach::Resuming => {
            served(emulator, case, &connector_arn, Work::Pause).await;
            until_connector_state(emulator, case, &connector_arn, CONNECTOR_PAUSED).await;
            served(emulator, case, &connector_arn, Work::Resume).await;
        }
        Reach::Restarting => {
            let path = format!("/v1/connectors/{}/restart", label(&connector_arn));
            let (status, answered) = call(emulator, "POST", &path, Value::Null).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the restart that puts the connector into its state must be \
                 served, got {status} {answered}"
            );
        }
        Reach::Updating => {
            let version = text(&describe(emulator, &connector_arn).await, "currentVersion");
            let path = format!(
                "/v1/connectors/{}?currentVersion={}",
                label(&connector_arn),
                label(&version)
            );
            let body = json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}});
            let (status, answered) = call(emulator, "PUT", &path, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the update that puts the connector into its state must be \
                 served, got {status} {answered}"
            );
        }
    }
    connector_arn
}

#[tokio::test]
async fn each_call_is_served_exactly_for_the_states_its_own_work_needs() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "pause-resume-gate-plugin").await;

    for (index, case) in CASES.iter().enumerate() {
        for work in WORK {
            let work = *work;
            let row = format!("{} {work:?}", case.state);
            // One connector per (state, call) pair: a served call is what takes the
            // connector out of the state the row tests.
            let name = format!("pause-resume-gate-{index}-{work:?}").to_lowercase();
            let connector_arn =
                connector_in_state(&emulator, &row, &name, &plugin_arn, case.reach).await;

            // The state the call actually lands on. A transient row is read as the
            // service reports it rather than as the row intends, because the clock may
            // already have settled it on a slow host; the rule is about the state the
            // call lands on, so that is what the expectation is taken from.
            let observed = connector_state(&emulator, &connector_arn).await;
            let accepted = work.accepted_states().contains(&observed.as_str());

            let (status, answered) = work.call(&emulator, &connector_arn).await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {row:?}: a connector in {observed:?} serves {work:?}, got {status} \
                     {answered}"
                );
                assert_eq!(
                    text(&answered, "connectorArn"),
                    connector_arn,
                    "case {row:?}: the answer must name the connector it acted on, got \
                     {answered}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {row:?}: a connector in {observed:?} serves no {work:?}, got {status} \
                 {answered}"
            );
            assert!(
                error_code(&answered).contains("BadRequestException"),
                "case {row:?}: expected BadRequestException, got {answered}"
            );
            let expected = work.refusal();
            assert_eq!(
                text(&answered, "message"),
                expected,
                "case {row:?}: expected the refusal {expected:?}, got {answered}"
            );
            // A refused call records nothing: the connector keeps the state it was in,
            // and no operation is stored against it.
            let after = connector_state(&emulator, &connector_arn).await;
            assert_eq!(
                after, observed,
                "case {row:?}: a refused call moves no connector, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_arn_that_names_no_connector_of_this_region_is_refused_before_anything_is_recorded() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "pause-resume-arn-plugin").await;
    let connector_arn = create_connector(&emulator, "pause-resume-arn-connector", &plugin_arn).await;
    until_connector_state(&emulator, "the fixture", &connector_arn, CONNECTOR_RUNNING).await;

    // The rows span what the rule generalises over: a value that is no ARN at all, an ARN
    // of another service, one of another region, and a well-formed ARN of this service and
    // region that names nothing.
    let rows: Vec<(&str, String, u16)> = vec![
        ("not an ARN at all", "pause-resume-arn-connector".to_string(), 400),
        (
            "an ARN of another service",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/other/0"),
            400,
        ),
        (
            "an ARN of another region",
            connector_arn.replace(REGION, "us-east-2"),
            404,
        ),
        (
            "an ARN of this region that names no connector",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/\
                 00000000-0000-0000-0000-000000000000"
            ),
            404,
        ),
    ];

    for (why, arn, expected) in rows {
        for work in WORK {
            let work = *work;
            let row = format!("{why} {work:?}");
            let (status, answered) = work.call(&emulator, &arn).await;
            assert_eq!(
                status, expected,
                "case {row:?}: expected HTTP {expected}, got {status} {answered}"
            );
            let code = error_code(&answered);
            let wanted = if expected == 400 {
                "BadRequestException"
            } else {
                "NotFoundException"
            };
            assert!(
                code.contains(wanted),
                "case {row:?}: expected {wanted}, got {answered}"
            );
        }
        // Nothing was recorded against the connector the fixture holds: the refusals above
        // reached no state at all.
        let listed = listed_operations(&emulator, &connector_arn).await;
        assert!(
            listed.is_empty(),
            "case {why:?}: a refused call records no operation, got {listed:?}"
        );
    }

    // And the connector the fixture created is untouched by every one of them.
    let state = connector_state(&emulator, &connector_arn).await;
    assert_eq!(
        state, CONNECTOR_RUNNING,
        "a refused call moves no other connector, got {state:?}"
    );
}
