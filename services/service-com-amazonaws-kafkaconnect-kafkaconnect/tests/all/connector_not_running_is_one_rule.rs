// SPDX-License-Identifier: CC0-1.0

//! The state gate every call that starts work on a connector runs, driven end to
//! end through BOTH operations that start one: `RestartConnector` and
//! `UpdateConnector`.
//!
//! RULE. A call that only a running connector can serve is refused, for a
//! connector in ANY `ConnectorState` the operation does not accept, with
//! `BadRequestException` (HTTP 400) blaming `connectorArn` and naming the states
//! that WOULD be served — `Invalid parameter connectorArn: This operation is
//! only valid for resources that are in one of the following states: [RUNNING,
//! PAUSED]` — the same status, code, blamed member and message whichever of
//! those calls made it, and is served only while the connector is in one of
//! them.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, with the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler running a gate of its own would fail here.
//!
//! The table spans what the rule generalises over: the `ConnectorState` the
//! connector is in when the call lands — the states this service puts a
//! connector in that the gate does not accept (`CREATING` while its workers come
//! up, `RESTARTING`, `UPDATING` and `DELETING`) — crossed with each call the gate
//! guards, plus the `RUNNING` row where both calls are served. `ConnectorState`
//! also lists `FAILED` (`smithy_query query="shape" shape="ConnectorState"`),
//! which a connector reaches only when its bring-up found no Apache Kafka
//! cluster — asserted, with the gate over it, in the router's
//! `kafkaconnect_connector_bring_up` suite, where Amazon MSK is mounted to
//! answer that question — and the `PAUSING`, `PAUSED` and `RESUMING` states the
//! two calls that stop and start a connector's workers put it in, which the
//! `connector_pause_and_resume` suite drives across both those calls' own
//! accepted sets.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    only_valid_in_states, CALL_ACCEPTED_CONNECTOR_STATES, CONNECTOR_CREATING, CONNECTOR_DELETING,
    CONNECTOR_RESTARTING, CONNECTOR_RUNNING, CONNECTOR_UPDATING, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// carries. No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The whole wire message the gate answers with, pinned as a client reads it:
/// the member the refusal blames and the states the operation accepts, which is
/// what tells a caller whether waiting would help.
const NOT_RUNNING: &str = "Invalid parameter connectorArn: This operation is only valid for \
                           resources that are in one of the following states: [RUNNING, PAUSED]";

/// Long enough that one change the clock owes after a call is due. The span is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
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

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label.
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

/// Create one connector and return its ARN. The clock is NOT waited out, so the
/// connector is still `CREATING` when this returns.
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

/// Which call the gate guards a row with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guarded {
    /// `RestartConnector`.
    Restart,
    /// `UpdateConnector` naming a new `capacity`.
    UpdateCapacity,
    /// `UpdateConnector` naming a new `connectorConfiguration`.
    UpdateConfiguration,
}

/// Every call this service has that only a running connector serves, i.e. one
/// per former copy of the gate, with each form the update takes.
const GUARDED: &[Guarded] = &[
    Guarded::Restart,
    Guarded::UpdateCapacity,
    Guarded::UpdateConfiguration,
];

/// Make the call `guarded` names against `connector_arn` and return its
/// `(status, body)`. An update names the connector's current version, so the
/// call reaches the state gate rather than the version check before it.
async fn guarded_call(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    guarded: Guarded,
) -> (u16, Value) {
    match guarded {
        Guarded::Restart => {
            let path = format!("/v1/connectors/{}/restart", label(connector_arn));
            call(emulator, "POST", &path, Value::Null).await
        }
        Guarded::UpdateCapacity | Guarded::UpdateConfiguration => {
            let version = text(&describe(emulator, connector_arn).await, "currentVersion");
            let body = if guarded == Guarded::UpdateCapacity {
                json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}})
            } else {
                json!({"connectorConfiguration": {
                    "connector.class": "io.confluent.connect.s3.S3SinkConnector",
                    "tasks.max": "4",
                }})
            };
            let path = format!(
                "/v1/connectors/{}?currentVersion={}",
                label(connector_arn),
                label(&version)
            );
            call(emulator, "PUT", &path, body).await
        }
    }
}

/// How a row puts its connector into the state it tests.
#[derive(Debug, Clone, Copy)]
enum Reach {
    /// Read the connector straight after its create, while its workers are
    /// still being provisioned.
    JustCreated,
    /// Restart a running connector, so it is bringing its workers back.
    Restarted,
    /// Update a running connector, so it is taking on new settings.
    Updated,
    /// Delete a running connector, so it is on its way out.
    Deleted,
}

/// One row of the table: a non-running `ConnectorState`, and how to reach it.
struct Case {
    /// The `ConnectorState` the connector holds when the guarded calls land.
    state: &'static str,
    reach: Reach,
}

/// Every non-running `ConnectorState` this service puts a connector in.
const CASES: &[Case] = &[
    Case {
        state: CONNECTOR_CREATING,
        reach: Reach::JustCreated,
    },
    Case {
        state: CONNECTOR_RESTARTING,
        reach: Reach::Restarted,
    },
    Case {
        state: CONNECTOR_UPDATING,
        reach: Reach::Updated,
    },
    Case {
        state: CONNECTOR_DELETING,
        reach: Reach::Deleted,
    },
];

/// Put a freshly created connector into the state `reach` names, and return its
/// ARN. Every call that moves it is checked, so a row can only run against the
/// state it means to test.
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

    // Every other row needs a RUNNING connector first: the clock, not a call,
    // is what brings one up.
    past_one_transition().await;
    let (status, answered) = match reach {
        Reach::JustCreated => unreachable!("handled above"),
        Reach::Restarted => guarded_call(emulator, &connector_arn, Guarded::Restart).await,
        Reach::Updated => guarded_call(emulator, &connector_arn, Guarded::UpdateCapacity).await,
        Reach::Deleted => {
            let path = format!("/v1/connectors/{}", label(&connector_arn));
            call(emulator, "DELETE", &path, Value::Null).await
        }
    };
    assert_eq!(
        status, 200,
        "case {case:?}: the call that puts the connector into its state must be served, \
         got {status} {answered}"
    );
    connector_arn
}

#[tokio::test]
async fn one_gate_refuses_every_call_a_connector_that_is_not_running_cannot_serve() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "not-running-gate-plugin").await;

    for (index, case) in CASES.iter().enumerate() {
        let name = format!("not-running-gate-{index}");
        let connector_arn =
            connector_in_state(&emulator, case.state, &name, &plugin_arn, case.reach).await;

        let described = describe(&emulator, &connector_arn).await;
        let state = text(&described, "connectorState");
        assert_eq!(
            state, case.state,
            "case {:?}: the row must run against the state it tests, got {state:?}",
            case.state
        );

        // Every guarded call, against the one connector in this state: the gate
        // is one rule, so each answers identically.
        let mut answers = Vec::new();
        for guarded in GUARDED {
            let row = format!("{} {guarded:?}", case.state);
            let (status, answered) = guarded_call(&emulator, &connector_arn, *guarded).await;
            assert_eq!(
                status, 400,
                "case {row:?}: a connector that is not RUNNING serves no such call, got \
                 {status} {answered}"
            );
            assert!(
                error_code(&answered).contains("BadRequestException"),
                "case {row:?}: expected BadRequestException, got {answered}"
            );
            let message = text(&answered, "message");
            assert_eq!(
                message, NOT_RUNNING,
                "case {row:?}: expected the refusal {NOT_RUNNING:?}, got {message:?}"
            );
            answers.push((row, message));
        }
        let Some((first_row, first)) = answers.first() else {
            panic!("case {:?}: the table names at least one guarded call", case.state);
        };
        for (row, message) in &answers {
            assert_eq!(
                message, first,
                "case {row:?}: every call the gate guards must answer as {first_row:?} does, \
                 got {message:?} against {first:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_running_connector_serves_every_call_the_gate_guards() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "running-gate-plugin").await;

    // One connector per guarded call: each is served exactly once, since a
    // served call is what takes the connector out of RUNNING.
    for (index, guarded) in GUARDED.iter().enumerate() {
        let name = format!("running-gate-{index}");
        let connector_arn = create_connector(&emulator, &name, &plugin_arn).await;
        past_one_transition().await;
        let described = describe(&emulator, &connector_arn).await;
        let state = text(&described, "connectorState");
        assert_eq!(
            state, CONNECTOR_RUNNING,
            "case {guarded:?}: the clock must bring the connector up before the call, got \
             {state:?}"
        );

        let (status, answered) = guarded_call(&emulator, &connector_arn, *guarded).await;
        assert_eq!(
            status, 200,
            "case {guarded:?}: a RUNNING connector serves the call, got {status} {answered}"
        );
        assert_eq!(
            text(&answered, "connectorArn"),
            connector_arn,
            "case {guarded:?}: the answer must name the connector it acted on, got {answered}"
        );
        let operation_arn = text(&answered, "connectorOperationArn");
        assert!(
            !operation_arn.is_empty(),
            "case {guarded:?}: a served call records an operation, got {answered}"
        );

        // The served call leaves the connector out of RUNNING, so the gate now
        // refuses the next one: the two halves of the one rule, back to back.
        let (status, refused) = guarded_call(&emulator, &connector_arn, *guarded).await;
        assert_eq!(
            status, 400,
            "case {guarded:?}: a connector already running that work serves no second call, \
             got {status} {refused}"
        );
        assert_eq!(
            text(&refused, "message"),
            NOT_RUNNING,
            "case {guarded:?}: expected the refusal {NOT_RUNNING:?}, got {refused}"
        );
    }
}

/// RULE (the sentence itself): the reason the gate renders is the published
/// sentence with ONE variable part, the bracketed list of the states the
/// operation accepts — so the message a client matches on cannot drift from the
/// set the gate decides by.
///
/// The rows span what that part generalises over: the set these two operations
/// accept (`RUNNING` and `PAUSED`, both states a connector's workers exist in), a
/// set of one state, and a set of three.
#[test]
fn the_reason_renders_the_states_the_operation_accepts() {
    let rows: &[(&str, &[&str], &str)] = &[
        (
            "the states a call against a connector accepts",
            CALL_ACCEPTED_CONNECTOR_STATES,
            "This operation is only valid for resources that are in one of the following states: \
             [RUNNING, PAUSED]",
        ),
        (
            "one state",
            &["ACTIVE"],
            "This operation is only valid for resources that are in one of the following states: \
             [ACTIVE]",
        ),
        (
            "three states",
            &["RUNNING", "PAUSED", "FAILED"],
            "This operation is only valid for resources that are in one of the following states: \
             [RUNNING, PAUSED, FAILED]",
        ),
    ];
    for (case, accepted, expected) in rows {
        let rendered = only_valid_in_states(accepted);
        assert_eq!(
            rendered, *expected,
            "case {case:?}: expected the reason {expected:?}, got {rendered:?}"
        );
    }

    // And the whole wire message the gate answers with is that reason behind the
    // member it blames, which is what the two tables above read back.
    let rendered = format!(
        "Invalid parameter connectorArn: {}",
        only_valid_in_states(CALL_ACCEPTED_CONNECTOR_STATES)
    );
    assert_eq!(
        rendered, NOT_RUNNING,
        "case \"the whole message\": expected {NOT_RUNNING:?}, got {rendered:?}"
    );
    assert!(
        CALL_ACCEPTED_CONNECTOR_STATES.contains(&CONNECTOR_RUNNING),
        "case \"the accepted set\": a running connector must be served, got \
         {CALL_ACCEPTED_CONNECTOR_STATES:?}"
    );
}
