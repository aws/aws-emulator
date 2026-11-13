// SPDX-License-Identifier: CC0-1.0

//! Region scoping, end to end over HTTP: which resources a call signed for one Region is
//! answered from.
//!
//! RULE. kafkaconnect is a REGIONAL service — its `aws.api#service` trait gives it the
//! `kafkaconnect` endpoint prefix with standard regional endpoints, and `ListConnectors` /
//! `ListCustomPlugins` are documented "all the connectors/custom plugins in this account and
//! Region" — so every resource belongs to exactly ONE Region, the one the request that
//! created it was signed for, and every call is answered from the resources of its own
//! Region alone. Three consequences, each pinned below:
//!
//! 1. a listing reports its own Region's resources and nothing else, so a Region nothing was
//!    created in answers an empty page;
//! 2. a name is unique PER REGION, so the same name created through two Regions' endpoints
//!    is two independent resources rather than a `ConflictException`;
//! 3. an ARN is resolved only through the endpoint of the Region that minted it — through
//!    any other it is as absent as an ARN the service never minted, for a read, a write, a
//!    delete and the tag seam alike, and such a call changes nothing.
//!
//! A service quota is likewise a per-Region quota, which the last test pins.
//!
//! Every case drives the emulator the way a client does — a buffered `restJson1` request
//! through the same router the production binary assembles, with the Region carried in the
//! SigV4 credential scope — so a handler that reads the store without consulting the
//! request's Region fails here.
//!
//! The tables span what the rule generalises over: four Regions (the emulator's default
//! `us-east-1`, two commercial Regions in different partitions of the globe and a GovCloud
//! Region), all three named resource kinds (custom plugin, connector, worker configuration),
//! each resource's own name reused across every Region, and — for the cross-Region
//! refusals — every by-ARN route of the surface, read and write.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{RESOURCE_QUOTA, TRANSITION_SPAN};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// The Regions every table below spans: the emulator's own default Region, two commercial
/// Regions far apart in the partition, and a GovCloud Region — four distinct credential
/// scopes, which is what makes each row a different Region rather than a different name.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-northeast-1", "us-gov-west-1"];

/// A Region no test below creates anything in, so its endpoint must answer every listing
/// with an empty page.
const EMPTY_REGION: &str = "sa-east-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The one name every Region's resources are created under, so a conflict across Regions
/// would show up as a refused create and a cross-Region read would show up as the wrong
/// Region's ARN.
const SHARED_NAME: &str = "demo";

/// Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`: a legal
/// `propertiesFileContent` for every row, since no row here is about the properties file.
const PROPERTIES: &str =
    "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=";

/// The status a successful `TagResource`/`UntagResource` answers in this emulator.
const TAG_WRITE_STATUS: u16 = 204;

/// A SigV4 credential scope naming this service and `region`. No authenticator is
/// registered, so the signature is never checked; the scope is what routes the request and
/// names the Region it is signed for.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kafkaconnect/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Long enough that every change the clock owes after one call is due. The span itself is
/// the service's own constant, so this cannot drift from it.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// One emulator: a single shared engine behind the production router. ONE store serves every
/// Region, which is exactly what makes the scoping worth asserting.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request through `region`'s endpoint and read back `(status, body)`.
async fn call(
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

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS SDK
/// encodes a label: every character a path segment reserves (including the ARN's own `:` and
/// `/`) is escaped.
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

/// A response member that is a list, or an empty slice.
fn list<'body>(body: &'body Value, member: &str) -> &'body [Value] {
    body.get(member).and_then(Value::as_array).map_or(&[], |items| items.as_slice())
}

/// The `CreateCustomPlugin` body for `name`. No S3 is mounted beside this service here, so
/// the plugin file is unresolvable and therefore not judged (see `custom_plugin_file_location`).
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": "arn:aws:s3:::kafkaconnect-plugin-artifacts",
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The `CreateWorkerConfiguration` body for `name`.
fn worker_request(name: &str) -> Value {
    json!({"name": name, "propertiesFileContent": PROPERTIES})
}

/// The `CreateConnector` body for `name`, backed by the plugin `plugin_arn` names.
fn connector_request(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
            "topics": format!("{name}-topic"),
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": "b-1.demo.kafka.amazonaws.com:9092",
            "vpc": {"subnets": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
    })
}

/// The three ARNs one Region holds after the whole shared-name set has been created in it.
struct RegionResources {
    plugin_arn: String,
    connector_arn: String,
    worker_arn: String,
}

/// Create a custom plugin, a worker configuration and a connector — all three under
/// [`SHARED_NAME`] — through `region`'s endpoint, asserting each create succeeds.
///
/// Every Region gets the SAME names, so a store that did not scope names per Region would
/// refuse the second Region's creates with the modelled 409.
async fn create_the_set(emulator: &MultiServiceRouter, region: &str) -> RegionResources {
    let (status, plugin) = call(
        emulator,
        region,
        "POST",
        "/v1/custom-plugins",
        plugin_request(SHARED_NAME),
    )
    .await;
    assert_eq!(
        status, 200,
        "region {region}: creating the custom plugin {SHARED_NAME:?} must succeed, got \
         {status} {plugin}"
    );

    let (status, worker) = call(
        emulator,
        region,
        "POST",
        "/v1/worker-configurations",
        worker_request(SHARED_NAME),
    )
    .await;
    assert_eq!(
        status, 200,
        "region {region}: creating the worker configuration {SHARED_NAME:?} must succeed, got \
         {status} {worker}"
    );

    let plugin_arn = text(&plugin, "customPluginArn");
    // A connector may only reference an ACTIVE plugin, and a freshly created plugin is
    // CREATING until the clock settles it.
    past_the_transition().await;

    let (status, connector) = call(
        emulator,
        region,
        "POST",
        "/v1/connectors",
        connector_request(SHARED_NAME, &plugin_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "region {region}: creating the connector {SHARED_NAME:?} must succeed, got \
         {status} {connector}"
    );

    RegionResources {
        plugin_arn,
        connector_arn: text(&connector, "connectorArn"),
        worker_arn: text(&worker, "workerConfigurationArn"),
    }
}

/// Whether `arn` is an ARN of `region`: the Region field of
/// `arn:aws:kafkaconnect:<region>:<account>:<resource>`.
fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}

#[tokio::test]
async fn a_listing_reports_only_the_resources_of_the_region_it_is_answered_through() {
    let emulator = emulator();
    let mut created = Vec::new();
    for region in REGIONS {
        created.push((*region, create_the_set(&emulator, region).await));
    }

    for (region, resources) in &created {
        // (the listing's route, the response member it answers under, the ARN member of a
        // summary, the ARN this Region's single resource was minted with)
        let listings: &[(&str, &str, &str, &str)] = &[
            (
                "/v1/connectors",
                "connectors",
                "connectorArn",
                &resources.connector_arn,
            ),
            (
                "/v1/custom-plugins",
                "customPlugins",
                "customPluginArn",
                &resources.plugin_arn,
            ),
        ];

        for (path, member, arn_member, want_arn) in listings {
            let (status, listed) = call(&emulator, region, "GET", path, Value::Null).await;
            assert_eq!(
                status, 200,
                "case ({region}, {path}): the listing must succeed, got {status} {listed}"
            );
            let items = list(&listed, member);
            assert_eq!(
                items.len(),
                1,
                "case ({region}, {path}): a Region reports its OWN one resource, expected 1 \
                 entry, got {} in {listed}",
                items.len()
            );
            let reported = text(&items[0], arn_member);
            assert_eq!(
                &reported, want_arn,
                "case ({region}, {path}): expected this Region's ARN {want_arn:?}, got \
                 {reported:?}"
            );
            assert_eq!(
                arn_region(&reported),
                *region,
                "case ({region}, {path}): every reported ARN must carry the Region it was \
                 listed through, got {reported:?}"
            );
            assert!(
                listed.get("nextToken").is_none()
                    || listed.get("nextToken") == Some(&Value::Null),
                "case ({region}, {path}): one entry is a whole page, so no nextToken, got \
                 {listed}"
            );

            // The same listing narrowed by a name prefix every Region's resource shares:
            // the prefix selects within the Region, it does not widen past it.
            let prefix_member = if *path == "/v1/connectors" {
                "connectorNamePrefix"
            } else {
                "namePrefix"
            };
            let prefix = &SHARED_NAME[..1];
            let (status, filtered) = call(
                &emulator,
                region,
                "GET",
                &format!("{path}?{prefix_member}={prefix}"),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case ({region}, {path}, prefix {prefix:?}): the listing must succeed, got \
                 {status} {filtered}"
            );
            assert_eq!(
                list(&filtered, member).len(),
                1,
                "case ({region}, {path}, prefix {prefix:?}): a prefix every Region's resource \
                 shares still reports one entry, got {filtered}"
            );
        }

        // The operations OF a connector are listed through that connector's own Region only;
        // a Region holds the operations of the connectors it holds.
        let (status, operations) = call(
            &emulator,
            region,
            "GET",
            &format!("/v1/connectors/{}/operations", label(&resources.connector_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case ({region}, ListConnectorOperations): a connector of this Region must \
             resolve, got {status} {operations}"
        );
        assert!(
            list(&operations, "connectorOperations").is_empty(),
            "case ({region}, ListConnectorOperations): nothing has been restarted or updated, \
             so no operations, got {operations}"
        );
    }

    // A Region nothing was created in is an EMPTY page, not another Region's state.
    for (path, member) in [
        ("/v1/connectors", "connectors"),
        ("/v1/custom-plugins", "customPlugins"),
    ] {
        let (status, listed) = call(&emulator, EMPTY_REGION, "GET", path, Value::Null).await;
        assert_eq!(
            status, 200,
            "case ({EMPTY_REGION}, {path}): the listing must succeed, got {status} {listed}"
        );
        assert!(
            list(&listed, member).is_empty(),
            "case ({EMPTY_REGION}, {path}): a Region holding nothing reports an empty page, \
             got {listed}"
        );
    }
}

#[tokio::test]
async fn one_name_names_one_resource_per_region_and_each_is_independent() {
    let emulator = emulator();
    let mut created = Vec::new();
    for region in REGIONS {
        created.push((*region, create_the_set(&emulator, region).await));
    }

    // Every Region minted its own three ARNs under the one shared name: distinct ARNs, each
    // carrying its own Region.
    let mut seen: Vec<String> = Vec::new();
    for (region, resources) in &created {
        for (kind, arn) in [
            ("custom plugin", &resources.plugin_arn),
            ("connector", &resources.connector_arn),
            ("worker configuration", &resources.worker_arn),
        ] {
            assert_eq!(
                arn_region(arn),
                *region,
                "case ({region}, {kind}): the minted ARN must carry the Region the create was \
                 signed for, got {arn:?}"
            );
            assert!(
                !seen.contains(arn),
                "case ({region}, {kind}): each Region's resource is its own, so no ARN repeats, \
                 got {arn:?} twice"
            );
            seen.push(arn.clone());
        }
    }

    // A repeat of the name WITHIN one Region is still the modelled 409: the rule is one
    // resource per name per Region, not "no uniqueness at all".
    for (region, _) in &created {
        // (the create's route, the request body, the member the refusal names)
        let repeats: &[(&str, Value)] = &[
            ("/v1/custom-plugins", plugin_request(SHARED_NAME)),
            ("/v1/worker-configurations", worker_request(SHARED_NAME)),
        ];
        for (path, body) in repeats {
            let (status, answered) = call(&emulator, region, "POST", path, body.clone()).await;
            assert_eq!(
                status, 409,
                "case ({region}, {path}): a repeated name within one Region is a conflict, got \
                 {status} {answered}"
            );
            assert!(
                error_code(&answered).contains("ConflictException"),
                "case ({region}, {path}): expected ConflictException, got {answered}"
            );
        }
    }

    // Each Region's resources carry their own tags: a write through one Region's endpoint is
    // invisible to the resource of the same name in every other Region.
    for (region, resources) in &created {
        let (status, answered) = call(
            &emulator,
            region,
            "POST",
            &format!("/v1/tags/{}", label(&resources.connector_arn)),
            json!({"tags": {"region-tag": region}}),
        )
        .await;
        assert_eq!(
            status, TAG_WRITE_STATUS,
            "case ({region}, TagResource): the write must succeed, got {status} {answered}"
        );
    }
    for (region, resources) in &created {
        let (status, tags) = call(
            &emulator,
            region,
            "GET",
            &format!("/v1/tags/{}", label(&resources.connector_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case ({region}, ListTagsForResource): the read must succeed, got {status} {tags}"
        );
        let reported = tags
            .get("tags")
            .and_then(|tags| tags.get("region-tag"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reported, *region,
            "case ({region}, ListTagsForResource): a resource carries the tag written through \
             ITS Region, got {tags}"
        );
    }
}

#[tokio::test]
async fn an_arn_is_resolved_only_through_the_endpoint_of_its_own_region() {
    let emulator = emulator();
    let home = REGIONS[1];
    let resources = create_the_set(&emulator, home).await;
    // A restart records one connector operation, so the operation ARN below is a real one.
    past_the_transition().await;
    let (status, restarted) = call(
        &emulator,
        home,
        "POST",
        &format!("/v1/connectors/{}/restart", label(&resources.connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the restart that records an operation must succeed, got {status} {restarted}"
    );
    let operation_arn = text(&restarted, "connectorOperationArn");

    let plugin = label(&resources.plugin_arn);
    let connector = label(&resources.connector_arn);
    let worker = label(&resources.worker_arn);
    let operation = label(&operation_arn);
    let capacity = json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 2}}});

    // Every by-ARN route of the surface, read and write: through a FOREIGN Region's endpoint
    // each is the modelled 404, exactly as an ARN this service never minted. `UpdateConnector`
    // is the one exception and is checked below it: it decides its `@required`
    // `currentVersion` before it resolves the connector, so a foreign Region's endpoint
    // refuses it on that member instead — still without resolving the connector.
    let routes: Vec<(&str, &str, String, Value)> = vec![
        ("DescribeConnector", "GET", format!("/v1/connectors/{connector}"), Value::Null),
        (
            "ListConnectorOperations",
            "GET",
            format!("/v1/connectors/{connector}/operations"),
            Value::Null,
        ),
        (
            "RestartConnector",
            "POST",
            format!("/v1/connectors/{connector}/restart"),
            Value::Null,
        ),
        ("DeleteConnector", "DELETE", format!("/v1/connectors/{connector}"), Value::Null),
        (
            "DescribeConnectorOperation",
            "GET",
            format!("/v1/connectorOperations/{operation}"),
            Value::Null,
        ),
        (
            "DescribeCustomPlugin",
            "GET",
            format!("/v1/custom-plugins/{plugin}"),
            Value::Null,
        ),
        (
            "DeleteCustomPlugin",
            "DELETE",
            format!("/v1/custom-plugins/{plugin}"),
            Value::Null,
        ),
        (
            "DescribeWorkerConfiguration",
            "GET",
            format!("/v1/worker-configurations/{worker}"),
            Value::Null,
        ),
        (
            "DeleteWorkerConfiguration",
            "DELETE",
            format!("/v1/worker-configurations/{worker}"),
            Value::Null,
        ),
        ("ListTagsForResource", "GET", format!("/v1/tags/{connector}"), Value::Null),
        (
            "TagResource",
            "POST",
            format!("/v1/tags/{connector}"),
            json!({"tags": {"from": "another-region"}}),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/v1/tags/{connector}?tagKeys=from"),
            Value::Null,
        ),
    ];

    for foreign in REGIONS.iter().filter(|region| **region != home) {
        for (operation_name, method, path, body) in &routes {
            let (status, answered) =
                call(&emulator, foreign, method, path, body.clone()).await;
            assert_eq!(
                status, 404,
                "case ({foreign}, {operation_name}): an ARN of {home} must not resolve through \
                 another Region's endpoint, got {status} {answered}"
            );
            assert!(
                error_code(&answered).contains("NotFoundException"),
                "case ({foreign}, {operation_name}): expected NotFoundException, got {answered}"
            );
        }
    }

    // `UpdateConnector` through a foreign Region's endpoint: the connector is no more
    // resolvable there than for an ARN this service never minted, and the version the
    // request named is not the current version of any connector that endpoint holds, so
    // the refusal is that member's — the modelled 400 — and nothing is updated.
    for foreign in REGIONS.iter().filter(|region| **region != home) {
        let (status, answered) = call(
            &emulator,
            foreign,
            "PUT",
            &format!("/v1/connectors/{connector}?currentVersion=1"),
            capacity.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case ({foreign}, UpdateConnector): an ARN of {home} must not resolve through \
             another Region's endpoint, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "case ({foreign}, UpdateConnector): expected BadRequestException, got {answered}"
        );
        assert_eq!(
            text(&answered, "message"),
            "Invalid parameter currentVersion.",
            "case ({foreign}, UpdateConnector): expected the refusal of the version the \
             request named, got {answered}"
        );
    }

    // None of those refusals wrote anything: the home Region's resources read back exactly as
    // they were, untagged, undeleted and with the one operation the restart recorded.
    let reads: &[(&str, String, &str, &str)] = &[
        (
            "DescribeConnector",
            format!("/v1/connectors/{connector}"),
            "connectorArn",
            &resources.connector_arn,
        ),
        (
            "DescribeCustomPlugin",
            format!("/v1/custom-plugins/{plugin}"),
            "customPluginArn",
            &resources.plugin_arn,
        ),
        (
            "DescribeWorkerConfiguration",
            format!("/v1/worker-configurations/{worker}"),
            "workerConfigurationArn",
            &resources.worker_arn,
        ),
        (
            "DescribeConnectorOperation",
            format!("/v1/connectorOperations/{operation}"),
            "connectorOperationArn",
            &operation_arn,
        ),
    ];
    for (operation_name, path, member, want) in reads {
        let (status, answered) = call(&emulator, home, "GET", path, Value::Null).await;
        assert_eq!(
            status, 200,
            "case ({home}, {operation_name}): the resource must still read back, got {status} \
             {answered}"
        );
        assert_eq!(
            &text(&answered, member),
            want,
            "case ({home}, {operation_name}): expected {want:?}, got {answered}"
        );
    }
    let (status, state) = call(
        &emulator,
        home,
        "GET",
        &format!("/v1/connectors/{connector}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "the connector must still read back, got {state}");
    assert_ne!(
        text(&state, "connectorState"),
        "DELETING",
        "case ({home}, DescribeConnector): a foreign Region's delete must not have moved this \
         connector, got {state}"
    );
    let (status, tags) = call(
        &emulator,
        home,
        "GET",
        &format!("/v1/tags/{connector}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "the tags must still read back, got {tags}");
    assert_eq!(
        tags.get("tags"),
        Some(&json!({})),
        "case ({home}, ListTagsForResource): a foreign Region's TagResource must have written \
         nothing, got {tags}"
    );
    let (status, operations) = call(
        &emulator,
        home,
        "GET",
        &format!("/v1/connectors/{connector}/operations"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "the operations must still list, got {operations}");
    assert_eq!(
        list(&operations, "connectorOperations").len(),
        1,
        "case ({home}, ListConnectorOperations): only the one restart this Region asked for \
         was recorded, got {operations}"
    );
}

#[tokio::test]
async fn the_resource_quota_is_counted_per_region() {
    // The cap is the service's own constant, so this cannot drift from it.
    let emulator = emulator();
    let first = REGIONS[0];
    let second = REGIONS[2];

    for index in 0..RESOURCE_QUOTA {
        let (status, created) = call(
            &emulator,
            first,
            "POST",
            "/v1/custom-plugins",
            plugin_request(&format!("quota-plugin-{index}")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case ({first}, plugin {index}): a plugin within the quota must be created, got \
             {status} {created}"
        );
    }

    let (status, answered) = call(
        &emulator,
        first,
        "POST",
        "/v1/custom-plugins",
        plugin_request("quota-plugin-over"),
    )
    .await;
    assert_eq!(
        status, 429,
        "case ({first}, the plugin past the quota): expected a 429, got {status} {answered}"
    );
    assert!(
        error_code(&answered).contains("TooManyRequestsException"),
        "case ({first}, the plugin past the quota): expected TooManyRequestsException, got \
         {answered}"
    );

    // Another Region's own count is untouched by the Region that is at its quota.
    let (status, created) = call(
        &emulator,
        second,
        "POST",
        "/v1/custom-plugins",
        plugin_request("quota-plugin-0"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case ({second}, the first plugin of a second Region): a quota is per Region, so this \
         must be created, got {status} {created}"
    );
    assert_eq!(
        arn_region(&text(&created, "customPluginArn")),
        second,
        "case ({second}): the created plugin must live in the Region it was created through, \
         got {created}"
    );
}
