// SPDX-License-Identifier: CC0-1.0

//! `UpdateConnector`'s worker setting, end to end over HTTP: which members name one, what
//! an update that names some of them leaves the connector running with, and what the
//! connector operation reports on each side of the change.
//!
//! RULE. An update names EITHER the connector's worker setting — one or more of `capacity`,
//! `plugins`, `workerConfiguration` and `useDefaultWorkerConfiguration` — OR its
//! `connectorConfiguration`, and not both in the same request; a part of the worker setting
//! the request names replaces that part, and a part it does not name is kept, so the
//! connector ends up running with its current setting under the named parts. The operation
//! the call records is typed `UPDATE_WORKER_SETTING` and reports the WHOLE setting on each
//! side of the change, a connector that holds no custom worker configuration being one
//! that uses the default one. A request naming both kinds of change or neither, naming both
//! `workerConfiguration` and `useDefaultWorkerConfiguration`, carrying a `plugins` list
//! that is not a single-element one, or naming a plugin or worker configuration this
//! region does not hold as ACTIVE at that revision, is refused with nothing stored; and a
//! `PAUSED` connector is updated only when the request asks for the health checks to be
//! skipped.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response body read
//! back as JSON. Nothing reaches into the state struct, so a handler answering from the
//! request instead of from state would fail here.
//!
//! The tables span what the rule generalises over: each worker-setting member on its own,
//! several at once, the flag that drops a custom configuration, and every way the request
//! can name a setting the service does not accept. Nothing here waits a fixed time: a
//! change the clock owes is waited for by looking again until the state holds, under a
//! deadline far longer than the service's own span.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    invalid_parameter_because, CONNECTOR_OPERATION_UPDATE_WORKER_SETTING, CONNECTOR_PAUSED,
    CONNECTOR_RUNNING, ONE_UPDATE_TARGET, ONE_WORKER_CONFIGURATION_FORM,
    PAUSED_NEEDS_SKIPPED_HEALTH_VALIDATION, RESOURCE_DOES_NOT_EXIST,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// How long a wait for a change the clock owes may take before the case fails.
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

/// The capacity every connector in this suite is created with: the provisioned form, so a
/// row updating to the auto-scaled form is a change of FORM and not only of value.
fn created_capacity() -> Value {
    json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}})
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`.
fn connector_request(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": created_capacity(),
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

/// Look at the resource at `path` again until `member` reports `wanted`.
async fn until_state(
    emulator: &MultiServiceRouter,
    case: &str,
    path: &str,
    member: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = call(emulator, "GET", path, Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read of {path} must succeed, got {status} {described}"
        );
        let state = text(&described, member);
        if state == wanted {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the clock to bring {member} to {wanted:?}, still \
             {state:?} in {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create one custom plugin and return its ARN once the clock has made it `ACTIVE`.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    let arn = text(&created, "customPluginArn");
    until_state(
        emulator,
        name,
        &format!("/v1/custom-plugins/{}", label(&arn)),
        "customPluginState",
        "ACTIVE",
    )
    .await;
    arn
}

/// Create one worker configuration and return its ARN. `CreateWorkerConfiguration` answers
/// with the only state the model gives the shape besides `DELETING`, so there is no
/// transition to wait for.
async fn worker_configuration(emulator: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        // base64 `connect-distributed.properties` content, as the member requires.
        "propertiesFileContent":
            "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
    });
    let (status, created) = call(emulator, "POST", "/v1/worker-configurations", body).await;
    assert_eq!(
        status, 200,
        "the worker configuration {name} must be created, got {status} {created}"
    );
    text(&created, "workerConfigurationArn")
}

/// Create one connector on `plugin_arn` and return its ARN once its workers are running.
async fn running_connector(
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
        "CreateConnector {name} must succeed, got {status} {created}"
    );
    let arn = text(&created, "connectorArn");
    until_state(
        emulator,
        name,
        &format!("/v1/connectors/{}", label(&arn)),
        "connectorState",
        CONNECTOR_RUNNING,
    )
    .await;
    arn
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

/// `UpdateConnector` against `connector_arn` at its current version, with `body`.
async fn update(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    body: Value,
) -> (u16, Value) {
    let version = text(&describe(emulator, connector_arn).await, "currentVersion");
    let path = format!(
        "/v1/connectors/{}?currentVersion={}",
        label(connector_arn),
        label(&version)
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

/// The worker setting a connector reports, read off `DescribeConnector`: the three parts
/// `WorkerSetting` carries, beside the flag that says whether the connector runs on the
/// default worker configuration — which is what holding no custom one means.
fn reported_worker_setting(described: &Value) -> Value {
    let worker_configuration = described.get("workerConfiguration").cloned();
    let mut setting = json!({
        "capacity": described["capacity"].clone(),
        "plugins": described["plugins"].clone(),
        "useDefaultWorkerConfiguration": worker_configuration.is_none(),
    });
    if let Some(configuration) = worker_configuration {
        setting["workerConfiguration"] = configuration;
    }
    setting
}

/// One plugin reference, as both a request and a report name it.
fn plugin_reference(plugin_arn: &str) -> Value {
    json!([{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}])
}

/// One worker-configuration reference, as both a request and a report name it.
fn worker_reference(worker_arn: &str) -> Value {
    json!({"workerConfigurationArn": worker_arn, "revision": 1})
}

/// The auto-scaled capacity the capacity rows update to: a change of FORM, not only of
/// value.
fn target_capacity() -> Value {
    json!({"autoScaling": {
        "maxWorkerCount": 4,
        "mcuCount": 2,
        "minWorkerCount": 1,
        "scaleInPolicy": {"cpuUtilizationPercentage": 20},
        "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
        // "Must be at least equal to maxWorkerCount"
        // (`AutoScalingUpdate$maxAutoscalingTaskCount`).
        "maxAutoscalingTaskCount": 4,
    }})
}

#[tokio::test]
async fn a_named_part_of_the_worker_setting_replaces_it_and_an_unnamed_one_is_kept() {
    let emulator = emulator();
    let first_plugin = active_plugin(&emulator, "worker-setting-plugin-one").await;
    let second_plugin = active_plugin(&emulator, "worker-setting-plugin-two").await;
    let worker_arn = worker_configuration(&emulator, "worker-setting-configuration").await;
    let connector_arn =
        running_connector(&emulator, "worker-setting-connector", &first_plugin).await;

    // Each row is one update of the SAME connector, so every row runs against the setting
    // the row before it left behind: the rule is that an unnamed part is kept.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "capacity alone",
            json!({"capacity": target_capacity()}),
            json!({
                "capacity": target_capacity(),
                "plugins": plugin_reference(&first_plugin),
                "useDefaultWorkerConfiguration": true,
            }),
        ),
        (
            "plugins alone",
            json!({"plugins": plugin_reference(&second_plugin)}),
            json!({
                "capacity": target_capacity(),
                "plugins": plugin_reference(&second_plugin),
                "useDefaultWorkerConfiguration": true,
            }),
        ),
        (
            "a worker configuration alone",
            json!({"workerConfiguration": worker_reference(&worker_arn)}),
            json!({
                "capacity": target_capacity(),
                "plugins": plugin_reference(&second_plugin),
                "workerConfiguration": worker_reference(&worker_arn),
                "useDefaultWorkerConfiguration": false,
            }),
        ),
        (
            "the default worker configuration, which drops the custom one",
            json!({"useDefaultWorkerConfiguration": true}),
            json!({
                "capacity": target_capacity(),
                "plugins": plugin_reference(&second_plugin),
                "useDefaultWorkerConfiguration": true,
            }),
        ),
        (
            "capacity, plugins and a worker configuration at once",
            json!({
                "capacity": created_capacity(),
                "plugins": plugin_reference(&first_plugin),
                "workerConfiguration": worker_reference(&worker_arn),
            }),
            json!({
                "capacity": created_capacity(),
                "plugins": plugin_reference(&first_plugin),
                "workerConfiguration": worker_reference(&worker_arn),
                "useDefaultWorkerConfiguration": false,
            }),
        ),
    ];

    for (case, body, expected) in rows {
        let before = describe(&emulator, &connector_arn).await;
        let origin_setting = reported_worker_setting(&before);
        let version_before = text(&before, "currentVersion");

        let (status, answered) = update(&emulator, &connector_arn, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must be served, got {status} {answered}"
        );
        let operation_arn = text(&answered, "connectorOperationArn");
        assert!(
            !operation_arn.is_empty(),
            "case {case:?}: a served update records an operation, got {answered}"
        );

        // The operation reports the WHOLE worker setting on each side of the change: what
        // the connector was running with, and what it is to run with.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            text(&operation, "connectorOperationType"),
            CONNECTOR_OPERATION_UPDATE_WORKER_SETTING,
            "case {case:?}: an update of the worker setting runs under its own type, got \
             {operation}"
        );
        assert_eq!(
            operation["originWorkerSetting"], origin_setting,
            "case {case:?}: originWorkerSetting must be the setting the connector was \
             running with, got {operation}"
        );
        assert_eq!(
            operation["targetWorkerSetting"], expected,
            "case {case:?}: targetWorkerSetting must be the setting the update leads to, \
             got {operation}"
        );
        for member in ["originConnectorConfiguration", "targetConnectorConfiguration"] {
            assert!(
                operation.get(member).is_none(),
                "case {case:?}: an update of the worker setting reports no {member}, got \
                 {operation}"
            );
        }

        // The CLOCK is what leaves the connector running on the new setting, at the next
        // version — and not before: the connector keeps reporting the setting it is still
        // running with while the update runs.
        let settled = until_state(
            &emulator,
            case,
            &format!("/v1/connectors/{}", label(&connector_arn)),
            "connectorState",
            CONNECTOR_RUNNING,
        )
        .await;
        assert_eq!(
            reported_worker_setting(&settled),
            expected,
            "case {case:?}: the connector must end up running with the setting the update \
             named, got {settled}"
        );
        let version_after = text(&settled, "currentVersion");
        assert_ne!(
            version_after, version_before,
            "case {case:?}: an update that landed moves the connector's version, got \
             {settled}"
        );
    }
}

#[tokio::test]
async fn a_request_the_worker_setting_rules_refuse_stores_nothing() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "worker-setting-refusal-plugin").await;
    let worker_arn = worker_configuration(&emulator, "worker-setting-refusal-configuration").await;
    let connector_arn =
        running_connector(&emulator, "worker-setting-refusal-connector", &plugin_arn).await;

    let unheld_plugin = format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:custom-plugin/ghost/\
         00000000-0000-0000-0000-000000000000"
    );
    let unheld_worker = format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/ghost/\
         00000000-0000-0000-0000-000000000000"
    );
    let configuration = json!({"tasks.max": "4"});

    // Each row: the body, the status, and the whole message the refusal carries.
    let rows: Vec<(&str, Value, u16, String)> = vec![
        (
            "neither kind of change",
            json!({}),
            400,
            invalid_parameter_because("connectorConfiguration", ONE_UPDATE_TARGET),
        ),
        (
            "a worker-setting member and the connector configuration at once",
            json!({"capacity": target_capacity(), "connectorConfiguration": configuration.clone()}),
            400,
            invalid_parameter_because("connectorConfiguration", ONE_UPDATE_TARGET),
        ),
        (
            "plugins and the connector configuration at once",
            json!({
                "plugins": plugin_reference(&plugin_arn),
                "connectorConfiguration": configuration.clone(),
            }),
            400,
            invalid_parameter_because("connectorConfiguration", ONE_UPDATE_TARGET),
        ),
        (
            "the default worker configuration and a custom one at once",
            json!({
                "workerConfiguration": worker_reference(&worker_arn),
                "useDefaultWorkerConfiguration": true,
            }),
            400,
            invalid_parameter_because("workerConfiguration", ONE_WORKER_CONFIGURATION_FORM),
        ),
        (
            "an empty plugins list",
            json!({"plugins": []}),
            400,
            invalid_parameter_because(
                "plugins",
                "Specify a single-element list containing one customPlugin object.",
            ),
        ),
        (
            "two plugins at once",
            json!({"plugins": [
                {"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}},
                {"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}},
            ]}),
            400,
            invalid_parameter_because(
                "plugins",
                "Specify a single-element list containing one customPlugin object.",
            ),
        ),
        (
            "a plugin reference that is no ARN at all",
            json!({"plugins": [{"customPlugin": {"customPluginArn": "plugin", "revision": 1}}]}),
            400,
            invalid_parameter_because("customPluginArn", "Invalid ARN."),
        ),
        (
            "a plugin this region does not hold",
            json!({"plugins": [
                {"customPlugin": {"customPluginArn": unheld_plugin, "revision": 1}},
            ]}),
            404,
            invalid_parameter_because("customPluginArn", RESOURCE_DOES_NOT_EXIST),
        ),
        (
            "a plugin revision the plugin is not at",
            json!({"plugins": [
                {"customPlugin": {"customPluginArn": plugin_arn, "revision": 9}},
            ]}),
            404,
            invalid_parameter_because("revision", RESOURCE_DOES_NOT_EXIST),
        ),
        (
            "a worker configuration this region does not hold",
            json!({"workerConfiguration": {
                "workerConfigurationArn": unheld_worker,
                "revision": 1,
            }}),
            404,
            invalid_parameter_because("workerConfigurationArn", RESOURCE_DOES_NOT_EXIST),
        ),
        (
            "a worker configuration revision it is not at",
            json!({"workerConfiguration": {
                "workerConfigurationArn": worker_arn,
                "revision": 7,
            }}),
            404,
            invalid_parameter_because("revision", RESOURCE_DOES_NOT_EXIST),
        ),
    ];

    let before = describe(&emulator, &connector_arn).await;
    for (case, body, expected, message) in rows {
        let (status, refused) = update(&emulator, &connector_arn, body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {refused}"
        );
        let wanted = if expected == 400 {
            "BadRequestException"
        } else {
            "NotFoundException"
        };
        assert!(
            error_code(&refused).contains(wanted),
            "case {case:?}: expected {wanted}, got {refused}"
        );
        assert_eq!(
            text(&refused, "message"),
            message,
            "case {case:?}: expected the refusal {message:?}, got {refused}"
        );

        // Nothing was stored: the connector is still running the setting it was, at the
        // version it was, and no operation was recorded against it.
        let after = describe(&emulator, &connector_arn).await;
        assert_eq!(
            reported_worker_setting(&after),
            reported_worker_setting(&before),
            "case {case:?}: a refused update changes no worker setting, got {after}"
        );
        assert_eq!(
            text(&after, "currentVersion"),
            text(&before, "currentVersion"),
            "case {case:?}: a refused update moves no version, got {after}"
        );
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors/{}/operations", label(&connector_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListConnectorOperations must succeed, got {status} {listed}"
        );
        let operations = listed["connectorOperations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            operations, 0,
            "case {case:?}: a refused update records no operation, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_paused_connector_is_updated_only_when_the_health_checks_are_skipped() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "paused-update-plugin").await;
    let connector_arn = running_connector(&emulator, "paused-update-connector", &plugin_arn).await;

    let (status, paused) = call(
        &emulator,
        "PUT",
        &format!("/v1/connectors/{}/pause", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the pause must be served, got {status} {paused}"
    );
    until_state(
        &emulator,
        "the fixture",
        &format!("/v1/connectors/{}", label(&connector_arn)),
        "connectorState",
        CONNECTOR_PAUSED,
    )
    .await;

    // The rows differ only in what the request says about the health checks: absent, false
    // and true. "To update a connector that is in the PAUSED state, you must set this
    // parameter to true" (`UpdateConnector$skipHealthValidation`).
    let refused: Vec<(&str, Value)> = vec![
        (
            "no skipHealthValidation at all",
            json!({"capacity": target_capacity()}),
        ),
        (
            "skipHealthValidation false",
            json!({"capacity": target_capacity(), "skipHealthValidation": false}),
        ),
    ];
    let expected =
        invalid_parameter_because("skipHealthValidation", PAUSED_NEEDS_SKIPPED_HEALTH_VALIDATION);
    for (case, body) in refused {
        let (status, answered) = update(&emulator, &connector_arn, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a paused connector takes no such update, got {status} {answered}"
        );
        assert_eq!(
            text(&answered, "message"),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {answered}"
        );
        let state = text(&describe(&emulator, &connector_arn).await, "connectorState");
        assert_eq!(
            state, CONNECTOR_PAUSED,
            "case {case:?}: a refused update leaves the connector paused, got {state:?}"
        );
    }

    let (status, answered) = update(
        &emulator,
        &connector_arn,
        json!({"capacity": target_capacity(), "skipHealthValidation": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"skipHealthValidation true\": the update must be served, got {status} \
         {answered}"
    );

    // An update neither stops nor starts a connector's workers: the clock leaves a paused
    // connector paused, running the capacity the update named.
    let settled = until_state(
        &emulator,
        "skipHealthValidation true",
        &format!("/v1/connectors/{}", label(&connector_arn)),
        "connectorState",
        CONNECTOR_PAUSED,
    )
    .await;
    assert_eq!(
        settled["capacity"], target_capacity(),
        "case \"skipHealthValidation true\": the connector must end up on the capacity the \
         update named, got {settled}"
    );
}
