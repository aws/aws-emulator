// SPDX-License-Identifier: CC0-1.0

//! `UpdateConnector`'s `currentVersion` gate, end to end over HTTP: which answer the
//! operation gives a version it does not recognise, and when it gives it.
//!
//! RULE. `UpdateConnector` decides its `@required` `currentVersion` BEFORE it resolves the
//! connector, so a request whose version is not the current version of a connector the
//! endpoint's Region holds is the modelled `BadRequestException` (HTTP 400,
//! `x-amzn-errortype: BadRequestException`) blaming that member in this service's
//! no-reason parameter form — `Invalid parameter currentVersion.`, whose only variable
//! part is the member name — and nothing is written. Whether the version has moved on, the
//! value was never a version this service mints, or the ARN names no connector at all
//! (never minted, minted in another Region, or already removed) makes no difference: once
//! the ARN is a well-formed ARN of this service, the connector is never resolved, so this
//! operation does not report a connector as absent. The version the connector itself
//! reports is served, and once an update lands it is the NEW version that is served and
//! the old one that is refused.
//!
//! The gate the rule leaves in front of it is the ARN's own GRAMMAR: a value that is not
//! an ARN of this service names no member of the version comparison and keeps its own
//! refusal (`Invalid parameter connectorArn: Invalid ARN.`), which is the contrast every
//! table below ends on.
//!
//! The same no-reason form is what `DeleteConnector` — the other operation of this service
//! whose request can name a `currentVersion` — words its own refusal of that member in.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response read back as
//! JSON. Nothing reaches into the state struct, and every wait polls for the condition it
//! waits on rather than for a span of time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// The Region every request of this suite is signed for, unless a case names another.
const REGION: &str = "eu-west-1";

/// A Region of the same partition no resource of this suite is created through, for the
/// row that asks one Region's endpoint about another Region's connector.
const OTHER_REGION: &str = "eu-central-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error code every refusal in this suite carries: the shape this service's model
/// declares for incorrect input (`smithy_query(query="operation",
/// operation="UpdateConnector")` lists `BadRequestException` @httpError 400).
const BAD_REQUEST: &str = "BadRequestException";

/// The whole message a refusal of the version a request named carries.
const INVALID_CURRENT_VERSION: &str = "Invalid parameter currentVersion.";

/// Generous room for a change the clock owes: a multiple of the service's own span, so
/// this cannot drift from it, and far longer than any one transition needs. Nothing is
/// asserted about how long a transition takes — only that the condition arrives.
fn settle_deadline() -> Duration {
    TRANSITION_SPAN * 60
}

/// How long a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope naming this service and `region`. No authenticator is
/// registered, so the signature is never checked; the scope is what names the Region.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kafkaconnect/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Send one request through `region`'s endpoint and read back `(status, body)`.
async fn call_in(
    emulator: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{region}.amazonaws.com{path}"))
        .header("authorization", authorization(region))
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

/// Send one request through the suite's own Region.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    call_in(emulator, REGION, method, path, body).await
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS
/// SDK encodes a label: every character a path segment reserves (the ARN's own `:` and `/`
/// included) is escaped.
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

/// The capacity every connector of this suite is created with.
fn created_capacity() -> Value {
    json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}})
}

/// The configuration every connector of this suite is created with.
fn created_configuration() -> Value {
    json!({"connector.class": "io.confluent.connect.s3.S3SinkConnector", "tasks.max": "2"})
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`.
fn connector_request(name: &str, plugin_arn: &str, region: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": created_capacity(),
        "connectorConfiguration": created_configuration(),
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{name}.kafka.{region}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
    })
}

/// Create one custom plugin in `region` and return its ARN once a read reports it ACTIVE,
/// which is the state a connector may reference it in.
async fn active_plugin(emulator: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, created) =
        call_in(emulator, region, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the plugin must be created, got {status} {created}"
    );
    let arn = text(&created, "customPluginArn");
    let started = Instant::now();
    loop {
        let (status, described) = call_in(
            emulator,
            region,
            "GET",
            &format!("/v1/custom-plugins/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeCustomPlugin must succeed, got {status} {described}"
        );
        if text(&described, "customPluginState") == "ACTIVE" {
            return arn;
        }
        assert!(
            started.elapsed() < settle_deadline(),
            "case {name:?}: the plugin must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `DescribeConnector` for `connector_arn` through `region`'s endpoint.
async fn describe_in(emulator: &MultiServiceRouter, region: &str, connector_arn: &str) -> Value {
    let (status, described) = call_in(
        emulator,
        region,
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

/// `DescribeConnector` through the suite's own Region.
async fn describe(emulator: &MultiServiceRouter, connector_arn: &str) -> Value {
    describe_in(emulator, REGION, connector_arn).await
}

/// Create a connector in `region` and return its ARN once a read reports it RUNNING, the
/// state an update is served in.
async fn running_connector(
    emulator: &MultiServiceRouter,
    region: &str,
    name: &str,
    plugin_arn: &str,
) -> String {
    let (status, created) = call_in(
        emulator,
        region,
        "POST",
        "/v1/connectors",
        connector_request(name, plugin_arn, region),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateConnector must succeed, got {status} {created}"
    );
    let arn = text(&created, "connectorArn");
    poll_until_running(emulator, region, &arn).await;
    arn
}

/// Look again until `connector_arn` reads back RUNNING, or a generous deadline passes.
async fn poll_until_running(emulator: &MultiServiceRouter, region: &str, connector_arn: &str) {
    let started = Instant::now();
    loop {
        let described = describe_in(emulator, region, connector_arn).await;
        if text(&described, "connectorState") == "RUNNING" {
            return;
        }
        assert!(
            started.elapsed() < settle_deadline(),
            "the connector must come to rest RUNNING, last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `UpdateConnector` against `connector_arn` at `current_version`, through `region`.
async fn update_in(
    emulator: &MultiServiceRouter,
    region: &str,
    connector_arn: &str,
    current_version: &str,
    body: Value,
) -> (u16, Value) {
    let path = format!(
        "/v1/connectors/{}?currentVersion={}",
        label(connector_arn),
        label(current_version)
    );
    call_in(emulator, region, "PUT", &path, body).await
}

/// `UpdateConnector` through the suite's own Region.
async fn update(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    current_version: &str,
    body: Value,
) -> (u16, Value) {
    update_in(emulator, REGION, connector_arn, current_version, body).await
}

/// How many connector operations `connector_arn` has recorded.
async fn recorded_operations(emulator: &MultiServiceRouter, connector_arn: &str) -> usize {
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
        .len()
}

/// A well-formed kafkaconnect connector ARN in `partition`/`region` naming `name` that no
/// connector of this suite carries.
fn unheld_arn(partition: &str, region: &str, name: &str, suffix: &str) -> String {
    format!("arn:{partition}:kafkaconnect:{region}:{ACCOUNT}:connector/{name}/{suffix}")
}

/// Both body forms `UpdateConnector` accepts, each one the operation would serve: the
/// refusal cannot depend on which setting the request names.
fn accepted_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a provisioned capacity",
            json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}}),
        ),
        (
            "an auto-scaled capacity",
            json!({"capacity": {"autoScaling": {
                "maxWorkerCount": 4,
                "mcuCount": 2,
                "minWorkerCount": 1,
                "scaleInPolicy": {"cpuUtilizationPercentage": 20},
                "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
                "maxAutoscalingTaskCount": 4,
            }}}),
        ),
        (
            "a configuration",
            json!({"connectorConfiguration": {"tasks.max": "7"}}),
        ),
    ]
}

/// Every value that is not the current version of a connector the Region holds, spanning
/// what the rule generalises over: a version the connector has moved past, versions in
/// forms this service never mints (empty, decimal, zero-padded, signed, whitespace around
/// the digits, a word, a very long digit string), and the version after the current one,
/// which no connector is at yet.
fn not_current_versions() -> Vec<(&'static str, String)> {
    vec![
        ("a version past the connector's", "9".to_string()),
        ("the version before the first", "0".to_string()),
        ("the version after the connector's", "2".to_string()),
        ("no version at all", String::new()),
        ("a decimal version", "1.0".to_string()),
        ("a zero-padded version", "01".to_string()),
        ("a signed version", "+1".to_string()),
        ("a version with leading space", " 1".to_string()),
        ("a version with trailing space", "1 ".to_string()),
        ("a version that is a word", "latest".to_string()),
        ("a version of absurd length", "1".repeat(300)),
    ]
}

/// Assert that `answered` is the modelled refusal of the version the request named.
fn assert_refused_on_current_version(case: &str, status: u16, answered: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {answered}"
    );
    let code = error_code(answered);
    assert!(
        code.contains(BAD_REQUEST),
        "case {case:?}: expected {BAD_REQUEST}, got {code:?} in {answered}"
    );
    let message = text(answered, "message");
    assert_eq!(
        message, INVALID_CURRENT_VERSION,
        "case {case:?}: expected the message {INVALID_CURRENT_VERSION:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_version_no_connector_of_the_region_is_at_is_refused_on_that_member() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, REGION, "version-gate-plugin").await;
    let held = running_connector(&emulator, REGION, "version-gate-held", &plugin).await;
    let current = text(&describe(&emulator, &held).await, "currentVersion");

    // A connector of ANOTHER Region: its ARN is well-formed, and the endpoint under test
    // holds no connector at it, so it belongs to the same class as an ARN never minted.
    let foreign_plugin = active_plugin(&emulator, OTHER_REGION, "version-gate-foreign-plugin").await;
    let foreign = running_connector(
        &emulator,
        OTHER_REGION,
        "version-gate-foreign",
        &foreign_plugin,
    )
    .await;
    let foreign_version = text(
        &describe_in(&emulator, OTHER_REGION, &foreign).await,
        "currentVersion",
    );

    // Each ARN this endpoint holds no connector at, over the ARN shapes a well-formed
    // connector ARN of this service takes.
    let unheld: Vec<(String, String)> = vec![
        (
            "an ARN naming a connector that was never created".to_string(),
            unheld_arn("aws", REGION, "version-gate-ghost", "11111111-2222-3333-4444-555555555555-1"),
        ),
        (
            "an ARN whose name differs from a held connector's by one character".to_string(),
            unheld_arn("aws", REGION, "version-gate-helda", "11111111-2222-3333-4444-555555555555-2"),
        ),
        (
            "an ARN of another account".to_string(),
            format!(
                "arn:aws:kafkaconnect:{REGION}:999999999999:connector/version-gate-ghost/\
                 11111111-2222-3333-4444-555555555555-3"
            ),
        ),
        (
            "an ARN of another partition".to_string(),
            unheld_arn("aws-cn", REGION, "version-gate-ghost", "11111111-2222-3333-4444-555555555555-4"),
        ),
        (
            "an ARN of a connector this Region does not hold".to_string(),
            foreign.clone(),
        ),
    ];

    // The table: every (version, ARN, body) triple that is not a connector's current
    // version here. The version rows run against the connector this Region DOES hold, so
    // the only thing wrong with them is the version; the ARN rows run with the version
    // each ARN's own connector would be at, so the only thing wrong with them is that no
    // connector of this Region is at it.
    let mut rows: Vec<(String, String, String, Value)> = Vec::new();
    for (shape, body) in accepted_bodies() {
        for (case, version) in not_current_versions() {
            assert_ne!(
                version, current,
                "case {case:?}: this row must name a version the connector is NOT at"
            );
            rows.push((
                format!("{case} on a held connector, naming {shape}"),
                held.clone(),
                version,
                body.clone(),
            ));
        }
        for (case, arn) in &unheld {
            let version = if arn == &foreign {
                foreign_version.clone()
            } else {
                current.clone()
            };
            rows.push((
                format!("{case}, naming {shape}"),
                arn.clone(),
                version,
                body.clone(),
            ));
        }
    }

    for (case, arn, version, body) in &rows {
        let (status, answered) = update(&emulator, arn, version, body.clone()).await;
        assert_refused_on_current_version(case, status, &answered);
        // The absent connector is never reported as absent by this operation: the gate in
        // front of the lookup decided the request.
        assert!(
            !error_code(&answered).contains("NotFoundException"),
            "case {case:?}: the version gate runs before the connector is resolved, so this \
             is not a resource refusal, got {answered}"
        );
    }

    // None of those refusals wrote anything: the held connector is still RUNNING at the
    // version it reported, on the settings it was created with, with no operation
    // recorded — and the other Region's connector is untouched too.
    let unchanged = describe(&emulator, &held).await;
    assert_eq!(
        text(&unchanged, "connectorState"),
        "RUNNING",
        "a refused update leaves the connector running, got {unchanged}"
    );
    assert_eq!(
        text(&unchanged, "currentVersion"),
        current,
        "a refused update leaves the version where it was, got {unchanged}"
    );
    assert_eq!(
        unchanged["capacity"]["provisionedCapacity"]["workerCount"],
        created_capacity()["provisionedCapacity"]["workerCount"],
        "a refused update changes no capacity, got {unchanged}"
    );
    assert_eq!(
        unchanged["connectorConfiguration"]["tasks.max"],
        created_configuration()["tasks.max"],
        "a refused update changes no configuration, got {unchanged}"
    );
    assert_eq!(
        recorded_operations(&emulator, &held).await,
        0,
        "a refused update records no connector operation"
    );
    let foreign_unchanged = describe_in(&emulator, OTHER_REGION, &foreign).await;
    assert_eq!(
        text(&foreign_unchanged, "currentVersion"),
        foreign_version,
        "another Region's connector must be untouched, got {foreign_unchanged}"
    );
}

#[tokio::test]
async fn the_version_the_connector_reports_is_the_one_served_and_it_moves_on() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, REGION, "version-gate-served-plugin").await;
    let connector = running_connector(&emulator, REGION, "version-gate-served", &plugin).await;

    // The version the connector itself reports is served, whichever body form names the
    // change; once the update has landed the connector is at the NEXT version, so the
    // version that was just served is now refused and the new one is served.
    for (shape, body) in accepted_bodies() {
        let before = text(&describe(&emulator, &connector).await, "currentVersion");
        let (status, accepted) = update(&emulator, &connector, &before, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {shape:?}: the version the connector reports must be served, got {status} \
             {accepted}"
        );
        assert_eq!(
            text(&accepted, "connectorArn"),
            connector,
            "case {shape:?}: the answer names the connector it updated, got {accepted}"
        );

        // Only the clock lands the update, and the version moves on with it: the gate
        // reads the connector's own state, so the version it serves changes when that
        // state does.
        poll_until_running(&emulator, REGION, &connector).await;
        let after = text(&describe(&emulator, &connector).await, "currentVersion");
        assert_ne!(
            after, before,
            "case {shape:?}: a landed update moves the version on, got {after:?}"
        );

        let (status, refused) = update(&emulator, &connector, &before, body.clone()).await;
        assert_refused_on_current_version(
            &format!("the version {before:?} the connector has moved past, naming {shape}"),
            status,
            &refused,
        );
    }
}

#[tokio::test]
async fn a_connector_arn_that_is_no_arn_of_this_service_keeps_its_own_refusal() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, REGION, "version-gate-grammar-plugin").await;
    let connector = running_connector(&emulator, REGION, "version-gate-grammar", &plugin).await;
    let current = text(&describe(&emulator, &connector).await, "currentVersion");

    // The one gate the rule leaves in FRONT of the version comparison: a label that is not
    // a well-formed ARN of this service. Spanned over the ways a value fails that grammar,
    // and with the version both right and wrong, so the precedence is the ARN's grammar
    // and not the version's correctness.
    let malformed = [
        ("no ARN fields at all", "version-gate-not-an-arn".to_string()),
        (
            "a well-formed ARN of another vendor",
            "arn:aws:s3:::version-gate-bucket".to_string(),
        ),
        (
            "truncated after the vendor",
            "arn:aws:kafkaconnect".to_string(),
        ),
        (
            "an empty resource field",
            format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:"),
        ),
    ];
    for (shape, arn) in &malformed {
        for version in [current.as_str(), "9", ""] {
            let case = format!("{shape} at currentVersion={version:?}");
            let (status, answered) =
                update(&emulator, arn, version, json!({"connectorConfiguration": {"tasks.max": "3"}}))
                    .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {answered}"
            );
            let message = text(&answered, "message");
            assert_eq!(
                message, "Invalid parameter connectorArn: Invalid ARN.",
                "case {case:?}: a value that is no ARN of this service keeps the ARN's own \
                 refusal, got {message:?}"
            );
        }
    }

    // And the connector is still there, at the version it was at, with nothing recorded.
    let unchanged = describe(&emulator, &connector).await;
    assert_eq!(
        text(&unchanged, "currentVersion"),
        current,
        "the refused calls must leave the version where it was, got {unchanged}"
    );
    assert_eq!(
        recorded_operations(&emulator, &connector).await,
        0,
        "the refused calls must record no connector operation"
    );
}

#[tokio::test]
async fn delete_connector_words_the_same_member_the_same_way() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, REGION, "version-gate-delete-plugin").await;
    let connector = running_connector(&emulator, REGION, "version-gate-delete", &plugin).await;
    let current = text(&describe(&emulator, &connector).await, "currentVersion");

    // `DeleteConnector` is the other operation of this service whose request can name a
    // `currentVersion`, and it words a value it does not recognise in the one no-reason
    // form too.
    for (case, version) in not_current_versions() {
        let (status, answered) = call(
            &emulator,
            "DELETE",
            &format!(
                "/v1/connectors/{}?currentVersion={}",
                label(&connector),
                label(&version)
            ),
            Value::Null,
        )
        .await;
        assert_refused_on_current_version(case, status, &answered);
    }

    // The connector is still there to be deleted at the version it reports, and the
    // version it reports is served.
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!(
            "/v1/connectors/{}?currentVersion={}",
            label(&connector),
            label(&current)
        ),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the version the connector reports must be served, got {status} {deleted}"
    );
    assert_eq!(
        text(&deleted, "connectorState"),
        "DELETING",
        "a deleted connector is DELETING, got {deleted}"
    );
}
