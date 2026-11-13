// SPDX-License-Identifier: CC0-1.0

//! The message every kafkaconnect parameter rejection carries, and the tags a create
//! stores, end to end over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, and the response body read
//! back as JSON.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A rejection kafkaconnect blames on a request member answers with a message that
//!    NAMES that member: `Invalid parameter <member>`, optionally followed by
//!    `: <reason>`. The model states no message text, so the form is the service's own,
//!    as its recorded rejections write it (`Invalid parameter nextToken.`,
//!    `Invalid parameter bucketArn: Invalid ARN.`). The table below spans every
//!    operation and member this holds for, not just the one the issue cited.
//! 2. A member that must carry an ARN for the service to use the resource it names is
//!    refused as a bad request when it is not an ARN of that resource — not an ARN at all,
//!    an ARN of another service, or an `s3` ARN naming something other than a bucket — and
//!    accepted for every partition, so the rule is the ARN grammar and the resource it
//!    names, never one literal prefix.
//! 3. `ListTagsForResource` reports exactly the tags the create stored, for either
//!    taggable resource, and an empty tag set is a different answer from a resource that
//!    is not there.

use std::collections::BTreeMap;
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
/// SDK encodes a label.
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

/// The `message` a failure response carries, or `""`.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `CreateCustomPlugin` body naming `bucket_arn` as the plugin file's bucket.
fn plugin_body(name: &str, bucket_arn: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {"bucketArn": bucket_arn, "fileKey": "plugins/p.zip"}},
    })
}

/// A `CreateConnector` body on `plugin_arn`, running under `role_arn`.
fn connector_body(name: &str, plugin_arn: &str, role_arn: &str) -> Value {
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
        "serviceExecutionRoleArn": role_arn,
    })
}

/// A well-formed IAM role ARN, as every accepted `CreateConnector` carries.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/connector-role")
}

/// A well-formed kafkaconnect ARN this emulator holds no resource at.
fn unheld_arn(kind: &str) -> String {
    format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:{kind}/ghost/00000000-0000-0000-0000-000000000000")
}

/// Assert that `body`'s message names `member`, and names `reason` too when one is
/// expected: the `Invalid parameter <member>[: <reason>]` form.
///
/// `reason` is given only where the SERVICE's own wording is known (its recorded
/// rejections carry `Invalid ARN.` and `The requested resource doesn't exist.`), or where
/// the rule says the message carries no reason at all. Where the reason sentence is this
/// emulator's own wording, [`assert_names_member_with_a_reason`] asserts the part the rule
/// fixes — that the message names the member — rather than re-stating a literal the
/// handler itself carries.
fn assert_names_member(case: &str, body: &Value, member: &str, reason: Option<&str>) {
    let actual = message(body);
    let expected = match reason {
        None => format!("Invalid parameter {member}."),
        Some(reason) => format!("Invalid parameter {member}: {reason}"),
    };
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
}

/// Assert that `body`'s message names `member` and goes on to give a reason, whatever the
/// reason sentence says: the `Invalid parameter <member>: <reason>` form.
fn assert_names_member_with_a_reason(case: &str, body: &Value, member: &str) {
    let actual = message(body);
    let expected_prefix = format!("Invalid parameter {member}: ");
    assert!(
        actual.starts_with(&expected_prefix) && actual.len() > expected_prefix.len(),
        "case {case:?}: expected a message starting {expected_prefix:?} and carrying a \
         reason, got {actual:?}"
    );
}

/// A plugin created and settled into ACTIVE, with its ARN.
async fn plugin_arn(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/custom-plugins",
        plugin_body(name, "arn:aws:s3:::plugin-bucket"),
    )
    .await;
    assert_eq!(status, 200, "creating {name} must succeed, got {created}");
    created["customPluginArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a customPluginArn in {created}"))
        .to_string()
}

#[tokio::test]
async fn every_parameter_rejection_names_the_parameter_it_refused() {
    let emulator = emulator();

    // Tokens no page of this service ever issued: one that is not even the token's
    // encoding, one that is hex but carries none of the service's marker, and one that is
    // hex of a plausible resume key.
    let forged_keyed = hex::encode(format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/page-0/x"
    ));
    let forged_tokens = ["not-a-token", "6161", forged_keyed.as_str()];

    // bucketArns that name no S3 bucket: empty, a bare name, a non-ARN sentence, too few
    // fields, an empty partition, an empty resource, an ARN of another service, and two
    // `s3` ARNs whose resource field names something other than a bucket — an object under
    // one, and an access point.
    let malformed_bucket_arns = [
        String::new(),
        "plugin-bucket".to_string(),
        "not-an-arn".to_string(),
        "arn:aws:s3::plugin-bucket".to_string(),
        "arn::s3:::plugin-bucket".to_string(),
        "arn:aws:s3:::".to_string(),
        "arn:aws:sqs:::plugin-bucket".to_string(),
        "arn:aws:s3:::bucket/prefix".to_string(),
        format!("arn:aws:s3:{REGION}:{ACCOUNT}:accesspoint/plugins"),
    ];

    // What a row expects of the message's reason part.
    #[derive(Clone, Copy)]
    enum Reason {
        /// No reason at all: `Invalid parameter <member>.`, as the recorded
        /// `Invalid parameter nextToken.` is written.
        Absent,
        /// Exactly this reason — the service's own recorded wording.
        Recorded(&'static str),
        /// Some reason, whose sentence is this emulator's own wording, so only the part
        /// the rule fixes (the member being named) is asserted.
        Any,
    }

    // (case, method, path, body, expected status, expected error code, member, reason)
    type Row = (
        String,
        &'static str,
        String,
        Value,
        u16,
        &'static str,
        &'static str,
        Reason,
    );
    let mut rows: Vec<Row> = Vec::new();

    for token in forged_tokens {
        rows.push((
            format!("ListConnectors with the unissued token {token:?}"),
            "GET",
            format!("/v1/connectors?maxResults=10&nextToken={}", label(token)),
            Value::Null,
            400,
            "BadRequestException",
            "nextToken",
            Reason::Absent,
        ));
        rows.push((
            format!("ListCustomPlugins with the unissued token {token:?}"),
            "GET",
            format!("/v1/custom-plugins?nextToken={}", label(token)),
            Value::Null,
            400,
            "BadRequestException",
            "nextToken",
            Reason::Absent,
        ));
    }

    for (index, bucket_arn) in malformed_bucket_arns.iter().enumerate() {
        rows.push((
            format!("CreateCustomPlugin with the bucketArn {bucket_arn:?}"),
            "POST",
            "/v1/custom-plugins".to_string(),
            plugin_body(&format!("malformed-bucket-{index}"), bucket_arn),
            400,
            "BadRequestException",
            "bucketArn",
            Reason::Recorded("Invalid ARN."),
        ));
    }

    for (index, bad_role) in ["", "connector-role", "arn:aws:s3:::bucket", "arn:aws:iam::"]
        .iter()
        .enumerate()
    {
        rows.push((
            format!("CreateConnector with the serviceExecutionRoleArn {bad_role:?}"),
            "POST",
            "/v1/connectors".to_string(),
            connector_body(
                &format!("bad-role-{index}"),
                &unheld_arn("custom-plugin"),
                bad_role,
            ),
            400,
            "BadRequestException",
            "serviceExecutionRoleArn",
            Reason::Recorded("Invalid ARN."),
        ));
    }

    // The by-ARN reads and deletes: a well-formed ARN naming no resource this emulator
    // holds names the member it could not resolve.
    for (member, kind, collection) in [
        ("customPluginArn", "custom-plugin", "/v1/custom-plugins"),
        ("connectorArn", "connector", "/v1/connectors"),
    ] {
        for method in ["GET", "DELETE"] {
            rows.push((
                format!("{method} {collection} on an ARN no resource carries"),
                method,
                format!("{collection}/{}", label(&unheld_arn(kind))),
                Value::Null,
                404,
                "NotFoundException",
                member,
                Reason::Recorded("The requested resource doesn\u{2019}t exist."),
            ));
        }
    }
    for kind in ["custom-plugin", "connector"] {
        rows.push((
            format!("ListTagsForResource on an unheld {kind} ARN"),
            "GET",
            format!("/v1/tags/{}", label(&unheld_arn(kind))),
            Value::Null,
            404,
            "NotFoundException",
            "resourceArn",
            Reason::Recorded("The requested resource doesn\u{2019}t exist."),
        ));
    }

    // CreateConnector's own member-level refusals.
    let mut no_plugins = connector_body("no-plugins", &unheld_arn("custom-plugin"), &role_arn());
    no_plugins["plugins"] = json!([]);
    rows.push((
        "CreateConnector with an empty plugins list".to_string(),
        "POST",
        "/v1/connectors".to_string(),
        no_plugins,
        400,
        "BadRequestException",
        "plugins",
        Reason::Any,
    ));
    let mut no_capacity = connector_body("no-capacity", &unheld_arn("custom-plugin"), &role_arn());
    no_capacity["capacity"] = json!({});
    rows.push((
        "CreateConnector with a capacity naming neither form".to_string(),
        "POST",
        "/v1/connectors".to_string(),
        no_capacity,
        400,
        "BadRequestException",
        "capacity",
        Reason::Any,
    ));
    rows.push((
        "CreateConnector on a plugin ARN no plugin carries".to_string(),
        "POST",
        "/v1/connectors".to_string(),
        connector_body(
            "unknown-plugin",
            &unheld_arn("custom-plugin"),
            &role_arn(),
        ),
        404,
        "NotFoundException",
        "customPluginArn",
        Reason::Recorded("The requested resource doesn\u{2019}t exist."),
    ));

    for (case, method, path, body, status, code, member, reason) in rows {
        let (answered_status, answered) = call(&emulator, method, &path, body).await;
        assert_eq!(
            answered_status, status,
            "case {case:?}: expected HTTP {status}, got {answered_status} {answered}"
        );
        assert!(
            error_code(&answered).contains(code),
            "case {case:?}: expected {code}, got {answered}"
        );
        match reason {
            Reason::Absent => assert_names_member(&case, &answered, member, None),
            Reason::Recorded(reason) => {
                assert_names_member(&case, &answered, member, Some(reason))
            }
            Reason::Any => assert_names_member_with_a_reason(&case, &answered, member),
        }
    }
}

/// Look again until `plugin_arn` reads back ACTIVE — the state a connector may reference
/// it in — or a generous deadline passes. Only the clock moves a plugin there, so the wait
/// is for that condition and not for a span of time.
async fn poll_until_plugin_active(emulator: &MultiServiceRouter, plugin_arn: &str) {
    // A multiple of the service's own transition span, so this cannot drift from it, and
    // far longer than one transition needs; nothing is asserted about how long it takes.
    let deadline = service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN * 60;
    let started = std::time::Instant::now();
    loop {
        let (status, described) = call(
            emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(plugin_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeCustomPlugin must succeed, got {status} {described}"
        );
        if described["customPluginState"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            started.elapsed() < deadline,
            "the plugin must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_rejection_against_stored_state_names_its_parameter_too() {
    let emulator = emulator();
    let plugin = plugin_arn(&emulator, "stateful-rejections").await;

    // A plugin that has not settled into ACTIVE yet cannot back a connector. This runs
    // first, while the transition the clock owes is still outstanding.
    let (status, answered) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_body("on-creating-plugin", &plugin, &role_arn()),
    )
    .await;
    assert_eq!(
        status, 400,
        "a connector on a CREATING plugin is a 400, got {status} {answered}"
    );
    assert_names_member_with_a_reason(
        "a plugin that is still CREATING",
        &answered,
        "customPluginArn",
    );

    // Once the plugin is ACTIVE the rows below are refused on the member each one names
    // rather than on the plugin's state. Only the clock moves it there, so this looks
    // again until a read reports it rather than waiting a span of time.
    poll_until_plugin_active(&emulator, &plugin).await;

    // A revision the stored plugin does not have: the member that could not be resolved
    // is `revision`, not the ARN, which did resolve.
    let mut wrong_revision = connector_body("wrong-revision", &plugin, &role_arn());
    wrong_revision["plugins"] = json!([{"customPlugin": {"customPluginArn": plugin, "revision": 9}}]);
    let (status, answered) = call(&emulator, "POST", "/v1/connectors", wrong_revision).await;
    assert_eq!(
        status, 404,
        "a revision the plugin does not have is a 404, got {status} {answered}"
    );
    assert_names_member(
        "a revision the stored plugin does not have",
        &answered,
        "revision",
        Some("The requested resource doesn\u{2019}t exist."),
    );

    // A stale currentVersion on a delete.
    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_body("versioned", &plugin, &role_arn()),
    )
    .await;
    assert_eq!(status, 200, "the connector must be created, got {created}");
    let connector = created["connectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a connectorArn in {created}"))
        .to_string();

    for stale in ["9", "", "1.0"] {
        let (status, answered) = call(
            &emulator,
            "DELETE",
            &format!(
                "/v1/connectors/{}?currentVersion={}",
                label(&connector),
                label(stale)
            ),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {stale:?}: a currentVersion that is not the connector's is a 400, got \
             {status} {answered}"
        );
        assert_names_member(
            &format!("the stale currentVersion {stale:?}"),
            &answered,
            "currentVersion",
            None,
        );
    }
}

#[tokio::test]
async fn a_bucket_arn_in_any_partition_is_accepted() {
    let emulator = emulator();

    // Every row is a well-formed S3 BUCKET ARN: the three partitions AWS publishes
    // (`aws`, `aws-cn`, `aws-us-gov`), a bucket name at one character, and one carrying
    // the characters a bucket name may hold — the ARN grammar is what decides, never one
    // literal prefix. A value whose resource field names something other than a bucket is
    // refused instead, which `custom_plugin_bucket_arn_names_a_bucket.rs` pins.
    let accepted = [
        ("the commercial partition", "arn:aws:s3:::plugin-bucket"),
        ("the China partition", "arn:aws-cn:s3:::plugin-bucket"),
        ("the GovCloud partition", "arn:aws-us-gov:s3:::plugin-bucket"),
        ("a one-character bucket name", "arn:aws:s3:::b"),
        (
            "a bucket name carrying dots and digits",
            "arn:aws:s3:::plugin.bucket-2024",
        ),
    ];

    for (index, (case, bucket_arn)) in accepted.iter().enumerate() {
        let name = format!("accepted-{index}");
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/custom-plugins",
            plugin_body(&name, bucket_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for the bucketArn {bucket_arn:?}, got \
             {status} {created}"
        );

        // And the accepted ARN is what the plugin reads back with, from state.
        let arn = created["customPluginArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a customPluginArn in {created}"));
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeCustomPlugin must succeed, got {status} {described}"
        );
        assert_eq!(
            described["latestRevision"]["location"]["s3Location"]["bucketArn"]
                .as_str()
                .unwrap_or_default(),
            *bucket_arn,
            "case {case:?}: the stored bucketArn must read back, got {described}"
        );
    }
}

/// One row of the tag table: the case name and the tags the create carries.
type TagRow<'a> = (&'a str, &'a [(&'a str, &'a str)]);

#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_the_create_stored() {
    let emulator = emulator();

    // The rows differ along what the rule generalises over: how many tags (none, one,
    // several), and what the keys and values hold — the `tags` member is
    // `map<string,string>, len 0..200`, so an empty map is a tag set too.
    let rows: &[TagRow<'_>] = &[
        ("no tags at all", &[]),
        ("one tag", &[("team", "data")]),
        (
            "several tags",
            &[("team", "data"), ("env", "prod"), ("cost-centre", "42")],
        ),
        (
            "keys and values outside ASCII",
            &[
                ("\u{e9}quipe", "donn\u{e9}es"),
                ("\u{30c4}\u{30fc}\u{30eb}", "\u{30ab}\u{30d5}\u{30ab}"),
            ],
        ),
        ("an empty tag value", &[("marker", "")]),
    ];

    for (index, (case, tags)) in rows.iter().enumerate() {
        let expected: BTreeMap<String, String> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        // The same tag set on each of the two taggable resources: the reader must not
        // care which kind the ARN names.
        let mut plugin = plugin_body(&format!("tagged-plugin-{index}"), "arn:aws:s3:::plugin-bucket");
        if !tags.is_empty() {
            plugin["tags"] = json!(expected);
        }
        let (status, created) = call(&emulator, "POST", "/v1/custom-plugins", plugin).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the tagged plugin must be created, got {status} {created}"
        );
        let plugin_arn = created["customPluginArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a customPluginArn in {created}"))
            .to_string();

        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v1/tags/{}", label(&plugin_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed}"
        );
        let reported: BTreeMap<String, String> =
            serde_json::from_value(listed["tags"].clone()).unwrap_or_else(|error| {
                panic!("case {case:?}: a tags map in {listed}: {error}")
            });
        assert_eq!(
            reported, expected,
            "case {case:?}: the plugin's tags must be exactly the ones the create stored, \
             got {listed}"
        );
    }

    // The same, for a connector — and the tag set read back is the one THAT create
    // carried, not the plugin's.
    let plugin = plugin_arn(&emulator, "connector-tag-plugin").await;
    tokio::time::sleep(
        service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN
            + std::time::Duration::from_millis(400),
    )
    .await;
    let connector_tags: BTreeMap<String, String> = [("owner", "streams"), ("env", "staging")]
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let mut body = connector_body("tagged-connector", &plugin, &role_arn());
    body["tags"] = json!(connector_tags);
    let (status, created) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 200,
        "the tagged connector must be created, got {status} {created}"
    );
    let connector_arn = created["connectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a connectorArn in {created}"))
        .to_string();
    let (status, listed) = call(
        &emulator,
        "GET",
        &format!("/v1/tags/{}", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource on the connector must succeed, got {status} {listed}"
    );
    let reported: BTreeMap<String, String> = serde_json::from_value(listed["tags"].clone())
        .unwrap_or_else(|error| panic!("a tags map in {listed}: {error}"));
    assert_eq!(
        reported, connector_tags,
        "the connector's tags must be exactly the ones its create stored, got {listed}"
    );

    // A deleted resource is not a resource with no tags: once the clock has dropped it,
    // the read is a 404 rather than an empty map.
    let (status, _) = call(
        &emulator,
        "DELETE",
        &format!("/v1/connectors/{}", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "the delete must be accepted");
    tokio::time::sleep(
        service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN
            + std::time::Duration::from_millis(400),
    )
    .await;
    let (status, answered) = call(
        &emulator,
        "GET",
        &format!("/v1/tags/{}", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 404,
        "a dropped connector has no tag set to report, got {status} {answered}"
    );
    assert_names_member(
        "the tags of a dropped connector",
        &answered,
        "resourceArn",
        Some("The requested resource doesn\u{2019}t exist."),
    );
}
