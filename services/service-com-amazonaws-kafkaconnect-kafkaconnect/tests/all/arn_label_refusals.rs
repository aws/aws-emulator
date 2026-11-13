// SPDX-License-Identifier: CC0-1.0

//! What this service answers for a member it documents as carrying an ARN, when the value
//! is not an ARN of this service at all.
//!
//! The rule, over every ARN-carrying member of the surface rather than the one route an
//! issue cited: a value that is not a well-formed ARN of the service owning the resource
//! it is meant to name names NO resource, so it is incorrect INPUT — the modelled
//! `BadRequestException` (HTTP 400) blaming that member with the service's `Invalid ARN.`
//! reason. A WELL-FORMED ARN the account holds no resource at is a different answer: the
//! resource is absent, so that one stays the modelled `NotFoundException` (HTTP 404) with
//! the service's `The requested resource doesn't exist.` reason — except where an earlier
//! gate of the operation has already decided the request, which is `UpdateConnector` and
//! its `@required` `currentVersion`. Both errors are declared for every one of these
//! operations, and each label is an `@httpLabel __string` with no `@pattern`, so the
//! handler is the only place the distinction can be drawn.
//!
//! The tables span what the rule generalises over:
//!
//! * every route whose request carries an ARN — the four path labels (`connectorArn`,
//!   `connectorOperationArn`, `customPluginArn`, `workerConfigurationArn`), the shared
//!   `resourceArn` of the three tag operations, and the two ARNs a `CreateConnector` body
//!   holds — read and write alike;
//! * every way a value can fail the ARN grammar
//!   `arn:<partition>:<vendor>:<region>:<account>:<resource>`: no fields at all, a
//!   well-formed ARN of another vendor, too few fields, an empty partition, an empty
//!   vendor, an empty resource, and a first field that is not `arn`.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, and the response read back as
//! JSON.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

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

/// The reason this service gives for a member that must carry an ARN and does not, as its
/// own recorded rejections write it.
const INVALID_ARN: &str = "Invalid ARN.";

/// The reason this service gives for a member naming a resource it does not hold. The
/// apostrophe is U+2019, as the service writes it.
const DOES_NOT_EXIST: &str = "The requested resource doesn\u{2019}t exist.";

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
/// SDK encodes a label: every character a path segment reserves (the ARN's own `:` and
/// `/` included) is escaped.
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

/// The `message` a response carries, or `""`.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One route of the surface whose request carries an ARN in its URI path.
struct Route {
    /// The operation, for the case name.
    operation: &'static str,
    /// The member the refusal must blame.
    member: &'static str,
    method: &'static str,
    /// The URI, with `{}` where the percent-encoded ARN goes.
    path: &'static str,
    /// The resource kind a well-formed ARN for this route names.
    kind: &'static str,
}

impl Route {
    /// This route's URI with `arn` in its label position.
    fn uri(&self, arn: &str) -> String {
        self.path.replace("{}", &label(arn))
    }

    /// The body this route's request carries, if any.
    fn body(&self) -> Value {
        match self.operation {
            "UpdateConnector" => json!({"connectorConfiguration": {"tasks.max": "2"}}),
            "TagResource" => json!({"tags": {"env": "probe"}}),
            _ => Value::Null,
        }
    }
}

/// Every route of this service whose request names a resource by ARN in its URI path: the
/// four by-ARN resource families, read and write, and the three tag operations that share
/// one `resourceArn`.
fn routes() -> Vec<Route> {
    vec![
        Route {
            operation: "DescribeConnector",
            member: "connectorArn",
            method: "GET",
            path: "/v1/connectors/{}",
            kind: "connector",
        },
        Route {
            operation: "DeleteConnector",
            member: "connectorArn",
            method: "DELETE",
            path: "/v1/connectors/{}",
            kind: "connector",
        },
        Route {
            operation: "UpdateConnector",
            member: "connectorArn",
            method: "PUT",
            path: "/v1/connectors/{}?currentVersion=1",
            kind: "connector",
        },
        Route {
            operation: "RestartConnector",
            member: "connectorArn",
            method: "POST",
            path: "/v1/connectors/{}/restart",
            kind: "connector",
        },
        Route {
            operation: "ListConnectorOperations",
            member: "connectorArn",
            method: "GET",
            path: "/v1/connectors/{}/operations",
            kind: "connector",
        },
        Route {
            operation: "DescribeConnectorOperation",
            member: "connectorOperationArn",
            method: "GET",
            path: "/v1/connectorOperations/{}",
            kind: "connector-operation",
        },
        Route {
            operation: "DescribeCustomPlugin",
            member: "customPluginArn",
            method: "GET",
            path: "/v1/custom-plugins/{}",
            kind: "custom-plugin",
        },
        Route {
            operation: "DeleteCustomPlugin",
            member: "customPluginArn",
            method: "DELETE",
            path: "/v1/custom-plugins/{}",
            kind: "custom-plugin",
        },
        Route {
            operation: "DescribeWorkerConfiguration",
            member: "workerConfigurationArn",
            method: "GET",
            path: "/v1/worker-configurations/{}",
            kind: "worker-configuration",
        },
        Route {
            operation: "DeleteWorkerConfiguration",
            member: "workerConfigurationArn",
            method: "DELETE",
            path: "/v1/worker-configurations/{}",
            kind: "worker-configuration",
        },
        Route {
            operation: "ListTagsForResource",
            member: "resourceArn",
            method: "GET",
            path: "/v1/tags/{}",
            kind: "connector",
        },
        Route {
            operation: "TagResource",
            member: "resourceArn",
            method: "POST",
            path: "/v1/tags/{}",
            kind: "connector",
        },
        Route {
            operation: "UntagResource",
            member: "resourceArn",
            method: "DELETE",
            path: "/v1/tags/{}?tagKeys=env",
            kind: "connector",
        },
    ]
}

/// Every way a value fails the ARN grammar
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`, where the first field is
/// literally `arn` and the partition, the vendor and the resource are non-empty: no
/// fields at all, a well-formed ARN of another vendor, too few fields, an empty resource,
/// an empty partition, an empty vendor, and a first field that is not `arn`.
fn malformed_arns() -> Vec<(&'static str, String)> {
    vec![
        ("no ARN fields at all", "not-an-arn".to_string()),
        (
            "a well-formed ARN of another vendor",
            "arn:aws:s3:::plugin-bucket".to_string(),
        ),
        (
            "another vendor, region and account filled",
            format!("arn:aws:iam::{ACCOUNT}:role/not-a-kafkaconnect-resource"),
        ),
        (
            "truncated after the vendor",
            "arn:aws:kafkaconnect".to_string(),
        ),
        (
            "five fields, so no resource field at all",
            format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:"),
        ),
        (
            "an empty partition",
            format!("arn::kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/1"),
        ),
        (
            "an empty vendor",
            format!("arn:aws::{REGION}:{ACCOUNT}:connector/ghost/1"),
        ),
        (
            "a first field that is not `arn`",
            format!("ARN:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/1"),
        ),
    ]
}

/// A well-formed kafkaconnect ARN of `kind` that this emulator holds no resource at.
fn unheld_arn(kind: &str) -> String {
    format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:{kind}/ghost/00000000-0000-4000-8000-000000000000"
    )
}

/// A worker configuration this emulator really holds, carrying one tag, with its ARN.
/// `CreateWorkerConfiguration` answers `ACTIVE` straight away, so nothing has to be
/// waited out before it reads back.
async fn held_worker_configuration(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/worker-configurations",
        json!({
            "name": name,
            // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
            "propertiesFileContent": "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
            "tags": {"env": "held"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the worker configuration must be created, got {status} {created}"
    );
    created["workerConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: a workerConfigurationArn in {created}"))
        .to_string()
}

#[tokio::test]
async fn a_label_that_is_not_an_arn_is_the_modelled_bad_request_naming_the_member() {
    let emulator = emulator();

    // A resource the store really holds, so the table below can be shown to have written
    // nothing: its tag set and its readability are the same afterwards.
    let held = held_worker_configuration(&emulator, "arn-label-bystander").await;

    for route in routes() {
        for (shape, arn) in malformed_arns() {
            let case = format!("{} with {shape}: {arn:?}", route.operation);
            let (status, answered) = call(
                &emulator,
                route.method,
                &route.uri(&arn),
                route.body(),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {answered}"
            );
            assert!(
                error_code(&answered).contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {answered}"
            );
            let expected = format!("Invalid parameter {}: {INVALID_ARN}", route.member);
            let actual = message(&answered);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the message {expected:?}, got {actual:?}"
            );
        }
    }

    // A refusal on the input writes nothing: the stored configuration is still there, in
    // the same state, with exactly the tags its create carried.
    let (status, described) = call(
        &emulator,
        "GET",
        &format!("/v1/worker-configurations/{}", label(&held)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the refused calls must leave the held configuration readable, got {status} {described}"
    );
    assert_eq!(
        described["workerConfigurationState"].as_str(),
        Some("ACTIVE"),
        "the refused calls must not move the held configuration's state, got {described}"
    );
    let (status, tags) = call(
        &emulator,
        "GET",
        &format!("/v1/tags/{}", label(&held)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the held configuration's tags must still read, got {status} {tags}"
    );
    assert_eq!(
        tags["tags"],
        json!({"env": "held"}),
        "the refused calls must not have written a tag, got {tags}"
    );
}

#[tokio::test]
async fn a_well_formed_arn_the_store_does_not_hold_stays_the_modelled_not_found() {
    let emulator = emulator();

    // The contrast the rule turns on: the SAME routes, given a well-formed kafkaconnect
    // ARN of the kind each one addresses, answer that the resource is absent.
    for route in routes() {
        // `UpdateConnector` is the one route that answers an unheld ARN with something
        // else, because its `@required` `currentVersion` is judged BEFORE the connector is
        // resolved: no connector that endpoint holds is at the version the request named,
        // so that member is what the refusal blames. Asserted right after this table.
        if route.operation == "UpdateConnector" {
            continue;
        }
        let arn = unheld_arn(route.kind);
        let case = format!("{} on an unheld {} ARN", route.operation, route.kind);
        let (status, answered) =
            call(&emulator, route.method, &route.uri(&arn), route.body()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
        let expected = format!("Invalid parameter {}: {DOES_NOT_EXIST}", route.member);
        let actual = message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }

    // And the one route whose earlier gate has already decided the request: the version
    // it names is not the current version of any connector this endpoint holds, so it is
    // the modelled 400 on that member and the connector is never resolved.
    let update = routes()
        .into_iter()
        .find(|route| route.operation == "UpdateConnector")
        .expect("the surface binds UpdateConnector");
    let arn = unheld_arn(update.kind);
    let (status, answered) = call(&emulator, update.method, &update.uri(&arn), update.body()).await;
    assert_eq!(
        status, 400,
        "case \"UpdateConnector on an unheld connector ARN\": expected HTTP 400, got {status} \
         {answered}"
    );
    assert!(
        error_code(&answered).contains("BadRequestException"),
        "case \"UpdateConnector on an unheld connector ARN\": expected BadRequestException, got \
         {answered}"
    );
    let actual = message(&answered);
    assert_eq!(
        actual, "Invalid parameter currentVersion.",
        "case \"UpdateConnector on an unheld connector ARN\": expected the refusal of the version \
         the request named, got {actual:?}"
    );
}

#[tokio::test]
async fn an_arn_a_create_connector_body_carries_is_read_the_same_way() {
    let emulator = emulator();

    // The rule is about ARN-carrying MEMBERS, not about URI labels: the two ARNs a
    // `CreateConnector` body names are refused exactly as the path labels are.
    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/custom-plugins",
        json!({
            "name": "body-arn-plugin",
            "contentType": "ZIP",
            "location": {"s3Location": {
                "bucketArn": "arn:aws:s3:::plugin-bucket",
                "fileKey": "plugins/p.zip",
            }},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the backing plugin must be created, got {status} {created}"
    );
    let plugin = created["customPluginArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a customPluginArn in {created}"))
        .to_string();

    // The plugin has to be ACTIVE before a connector may reference it, and only the clock
    // moves it there.
    tokio::time::sleep(
        service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN
            + std::time::Duration::from_millis(400),
    )
    .await;

    for (index, (shape, arn)) in malformed_arns().into_iter().enumerate() {
        // `plugins[].customPlugin.customPluginArn`: refused before the plugin is looked
        // up, so the reference naming no ARN at all never reaches the store.
        let case = format!("CreateConnector customPluginArn with {shape}: {arn:?}");
        let mut body = connector_body(&format!("body-plugin-{index}"), &plugin);
        body["plugins"] = json!([{"customPlugin": {"customPluginArn": arn, "revision": 1}}]);
        let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {answered}"
        );
        let expected = format!("Invalid parameter customPluginArn: {INVALID_ARN}");
        let actual = message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );

        // `workerConfiguration.workerConfigurationArn`: the same, on a request whose
        // plugin reference does resolve.
        let case = format!("CreateConnector workerConfigurationArn with {shape}: {arn:?}");
        let mut body = connector_body(&format!("body-worker-{index}"), &plugin);
        body["workerConfiguration"] = json!({"workerConfigurationArn": arn, "revision": 1});
        let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {answered}"
        );
        let expected = format!("Invalid parameter workerConfigurationArn: {INVALID_ARN}");
        let actual = message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }

    // And a well-formed reference the account holds nothing at is still the 404.
    let mut body = connector_body("body-unheld-plugin", &plugin);
    let unheld = unheld_arn("custom-plugin");
    body["plugins"] = json!([{"customPlugin": {"customPluginArn": unheld, "revision": 1}}]);
    let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 404,
        "a well-formed plugin ARN the account does not hold is a 404, got {status} {answered}"
    );
    let expected = format!("Invalid parameter customPluginArn: {DOES_NOT_EXIST}");
    let actual = message(&answered);
    assert_eq!(
        actual, expected,
        "the unheld plugin reference must report that the resource is absent, got {actual:?}"
    );
}

/// A `CreateConnector` body on `plugin_arn` that every other rule of the operation
/// accepts, so only the member a row alters decides the answer.
fn connector_body(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {"tasks.max": "1"},
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": "b-1.example:9098",
            "vpc": {"subnets": ["subnet-1", "subnet-2"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "IAM"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/connector-role"),
    })
}
