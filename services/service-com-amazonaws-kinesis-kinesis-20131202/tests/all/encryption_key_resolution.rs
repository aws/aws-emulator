// SPDX-License-Identifier: CC0-1.0

//! The KMS key a `StartStreamEncryption` request names is RESOLVED before the stream is
//! reported as encrypted under it, end to end over the wire: `StartStreamEncryption` ->
//! `DescribeStreamSummary` / `DescribeStream`, with KMS mounted beside Kinesis on one
//! engine exactly as the assembled emulator mounts it.
//!
//! RULE: an operation that applies a caller-named KMS key to a stream resolves that
//! identifier against KMS before it mutates anything — which is why it models
//! `KMSNotFoundException` ("The request was rejected because the specified entity or
//! resource can't be found", HTTP 400) at all. A `KeyId` KMS resolves is stored and
//! reported, in every form `KeyId` takes ("a globally unique identifier, a fully specified
//! Amazon Resource Name (ARN) to either an alias or a key, or an alias name prefixed by
//! \"alias/\"", plus the service's own predefined alias); a `KeyId` KMS holds no key for is
//! refused 400 `KMSNotFoundException` with the fixed sentence "Key '<KeyId>' does not
//! exist" — whose only variable part is the identifier echoed from the request — and the
//! stream keeps the encryption configuration it had, so a stream that was unencrypted stays
//! `NONE`. A lookup KMS does not answer says nothing about the key, so it refuses nothing.
//!
//! The recorded flow this was derived from calls `StartStreamEncryption(EncryptionType=KMS,
//! KeyId=<a key ARN in the caller's own account and region that nothing created>)` once on
//! one single-shard stream, which is one row of the tables below. The rows differ along what
//! the rule generalises over: the FORM of the identifier (key ARN, key id, alias ARN, alias
//! name, the service's predefined alias), whether KMS holds a key for it, the encryption the
//! stream carries when the call arrives, the identifier the request addresses the stream by,
//! and whether KMS is there to answer at all.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! <prefix>.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses, bodies
//! and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    ENCRYPTION_TYPE_KMS, ENCRYPTION_TYPE_NONE, STREAM_ACTIVE,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::stream_tags::{error_code, error_message, stream_arn, REGION};

/// The `X-Amz-Target` prefix of each service a case drives (`OPERATIONS.md`, "Wire
/// protocol", and the KMS model's service shape).
const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";
pub(crate) const KMS_TARGET_PREFIX: &str = "TrentService";

/// A `KeyId` KMS resolves to no key: a UUID of the shape `KeyId` documents ("Globally
/// unique key ID example: 12345678-1234-1234-1234-123456789012") that no `CreateKey` of
/// this emulator minted.
const ABSENT_KEY_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The predefined alias of the Amazon Web Services managed key this service owns: "You can
/// also use a master key owned by Kinesis Data Streams by specifying the alias aws/kinesis"
/// (`smithy_query(query="doc",shape="StartStreamEncryptionInput",member="KeyId")`). KMS
/// holds such an alias in every account and Region, so it resolves.
const SERVICE_ALIAS: &str = "alias/aws/kinesis";

/// A router carrying Kinesis and (unless a case is about KMS being absent) KMS, over one
/// shared engine: the same assembly `core/router` performs.
///
/// `pub(crate)` because the DATA-plane half of the same contract is driven over the same two
/// services (`crate::encrypted_data_plane`), and one harness for one assembly is better than
/// two.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// Kinesis with KMS beside it, which is how the assembled emulator runs.
    pub(crate) fn new() -> Self {
        Self::build(true)
    }

    /// Kinesis mounted ALONE, so the handler's in-process lookup reaches no KMS at all and
    /// learns nothing about the key it asked about.
    fn without_kms() -> Self {
        Self::build(false)
    }

    fn build(with_kms: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated;
        // the router still routes by the SigV4 credential scope, which is where the
        // request's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if with_kms {
            router.register_service(
                service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
            );
        }
        Self { router }
    }

    /// Call `operation` of the service `target_prefix` names, signed for `REGION`.
    pub(crate) async fn send(
        &self,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let signing_name = if target_prefix == KMS_TARGET_PREFIX {
            "kms"
        } else {
            "kinesis"
        };
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One Kinesis call.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.send(KINESIS_TARGET_PREFIX, operation, body).await
    }

    /// One Kinesis call that must be answered 200.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// Create an ACTIVE stream named `name`: the create, then LOOKING until the stream
    /// reports the status its create owes it, through the crate's one poll loop
    /// ([`crate::stream_tags::poll_until`]).
    pub(crate) async fn active_stream(&self, case: &str, name: &str) {
        self.ok(case, "CreateStream", &json!({ "StreamName": name, "ShardCount": 1 }))
            .await;
        crate::stream_tags::poll_until(
            case,
            &format!("the stream {name} reports {STREAM_ACTIVE}"),
            || async {
                let (status, response) = self
                    .call("DescribeStreamSummary", &json!({ "StreamName": name }))
                    .await;
                status == 200
                    && response["StreamDescriptionSummary"]["StreamStatus"].as_str()
                        == Some(STREAM_ACTIVE)
            },
        )
        .await;
    }

    /// Mint a key through the mounted KMS and answer its `(KeyId, Arn)`, so an identifier a
    /// case means to be REAL is one KMS can resolve.
    pub(crate) async fn create_key(&self) -> (String, String) {
        let (status, response) = self.send(KMS_TARGET_PREFIX, "CreateKey", &json!({})).await;
        assert_eq!(
            status, 200,
            "CreateKey must mint a key for the rows that name a real one, got {status} with \
             body {response}"
        );
        let metadata = &response["KeyMetadata"];
        (
            metadata["KeyId"]
                .as_str()
                .expect("CreateKey reports the new key's KeyId")
                .to_string(),
            metadata["Arn"]
                .as_str()
                .expect("CreateKey reports the new key's Arn")
                .to_string(),
        )
    }

    /// Point `alias_name` at `key_id` through the mounted KMS.
    async fn create_alias(&self, alias_name: &str, key_id: &str) {
        let (status, response) = self
            .send(
                KMS_TARGET_PREFIX,
                "CreateAlias",
                &json!({ "AliasName": alias_name, "TargetKeyId": key_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateAlias must point {alias_name} at {key_id}, got {status} with body \
             {response}"
        );
    }

    /// The `EncryptionType` and `KeyId` `DescribeStreamSummary` reports for `name`.
    pub(crate) async fn reported_encryption(
        &self,
        case: &str,
        name: &str,
    ) -> (String, Option<String>) {
        let described = self
            .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        let summary = &described["StreamDescriptionSummary"];
        let encryption_type = summary["EncryptionType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the read must carry EncryptionType, got {described}"))
            .to_string();
        (
            encryption_type,
            summary["KeyId"].as_str().map(str::to_string),
        )
    }
}

/// What a refusal states for an identifier KMS holds no key for: fixed text, with the
/// identifier echoed from the request as its only variable part.
fn key_does_not_exist_message(key_id: &str) -> String {
    format!("Key '{key_id}' does not exist")
}

/// The alias ARN of `alias_name` at this account and Region, which is the other form an
/// alias is addressed by.
fn alias_arn(alias_name: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:{alias_name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Start encryption of `name` with `key_id`, addressing the stream the way `by_arn` says.
async fn start_encryption(
    emulator: &Emulator,
    name: &str,
    key_id: &str,
    by_arn: bool,
) -> (u16, Value) {
    let mut body = json!({ "EncryptionType": ENCRYPTION_TYPE_KMS, "KeyId": key_id });
    if by_arn {
        body["StreamARN"] = json!(stream_arn(REGION, name));
    } else {
        body["StreamName"] = json!(name);
    }
    emulator.call("StartStreamEncryption", &body).await
}

// 1. Every form of a KeyId KMS resolves is stored and reported
#[tokio::test]
async fn a_key_kms_resolves_is_accepted_in_every_form_the_member_documents() {
    let emulator = Emulator::new();
    let (key_id, key_arn) = emulator.create_key().await;
    let alias_name = "alias/kinesis-suite-key";
    emulator.create_alias(alias_name, &key_id).await;

    // The rows are the forms `KeyId` itself catalogs
    // (`smithy_query(query="doc",shape="StartStreamEncryptionInput",member="KeyId")`): a
    // key id, a key ARN, an alias name, an alias ARN, and this service's predefined alias.
    let rows = [
        ("the key id", key_id.clone()),
        ("the key ARN", key_arn.clone()),
        ("an alias name", alias_name.to_string()),
        ("an alias ARN", alias_arn(alias_name)),
        ("the service's predefined alias", SERVICE_ALIAS.to_string()),
    ];

    for (index, (label, identifier)) in rows.iter().enumerate() {
        let stream = format!("resolvable-{index}");
        emulator.active_stream(label, &stream).await;
        let by_arn = index % 2 == 1;
        let (status, response) = start_encryption(&emulator, &stream, identifier, by_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a KeyId KMS resolves must be accepted, got {status} with body \
             {response}"
        );
        let reported = emulator.reported_encryption(label, &stream).await;
        assert_eq!(
            reported,
            (ENCRYPTION_TYPE_KMS.to_string(), Some(identifier.clone())),
            "case {label:?}: expected the stream to report KMS under {identifier:?}, got \
             {reported:?}"
        );
    }
}

// 2. A KeyId KMS resolves to nothing is refused, and nothing is stored for it
#[tokio::test]
async fn a_key_kms_cannot_resolve_is_refused_and_the_stream_stays_unencrypted() {
    let emulator = Emulator::new();

    // The same forms as the accepted rows, each naming nothing KMS holds: a key id, a key
    // ARN of this account and Region, an alias name, and an alias ARN.
    let rows = [
        ("a key id nothing created", ABSENT_KEY_UUID.to_string()),
        (
            "a key ARN nothing created",
            format!(
                "arn:aws:kms:{REGION}:{}:key/{ABSENT_KEY_UUID}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "an alias name nothing created",
            "alias/no-such-alias".to_string(),
        ),
        (
            "an alias ARN nothing created",
            alias_arn("alias/no-such-alias"),
        ),
    ];

    for (index, (label, identifier)) in rows.iter().enumerate() {
        let stream = format!("unresolvable-{index}");
        emulator.active_stream(label, &stream).await;
        let by_arn = index % 2 == 1;
        let (status, response) = start_encryption(&emulator, &stream, identifier, by_arn).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a KeyId KMS resolves to nothing must be refused with 400, got \
             {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "KMSNotFoundException",
            "case {label:?}: expected KMSNotFoundException, got {response}"
        );
        let expected = key_does_not_exist_message(identifier);
        assert_eq!(
            error_message(&response),
            expected,
            "case {label:?}: expected message {expected:?}, got {:?}",
            error_message(&response)
        );
        let reported = emulator.reported_encryption(label, &stream).await;
        assert_eq!(
            reported,
            (ENCRYPTION_TYPE_NONE.to_string(), None),
            "case {label:?}: a refused request must leave the stream unencrypted, got \
             {reported:?}"
        );
    }
}

// 3. A refusal leaves an already encrypted stream under the key it had
#[tokio::test]
async fn a_refused_update_leaves_the_key_the_stream_already_carries() {
    let emulator = Emulator::new();
    let stream = "already-encrypted";
    emulator.active_stream("already encrypted", stream).await;
    let (_, held_key) = emulator.create_key().await;

    let (status, response) = start_encryption(&emulator, stream, &held_key, false).await;
    assert_eq!(
        status, 200,
        "case \"the first key\": a KeyId KMS resolves must be accepted, got {status} with \
         body {response}"
    );
    let encrypted = (ENCRYPTION_TYPE_KMS.to_string(), Some(held_key.clone()));
    let reported = emulator.reported_encryption("the first key", stream).await;
    assert_eq!(
        reported, encrypted,
        "case \"the first key\": expected {encrypted:?}, got {reported:?}"
    );

    let (status, response) = start_encryption(&emulator, stream, ABSENT_KEY_UUID, false).await;
    assert_eq!(
        status, 400,
        "case \"an update to a key nothing created\": expected a refusal, got {status} with \
         body {response}"
    );
    let reported = emulator
        .reported_encryption("an update to a key nothing created", stream)
        .await;
    assert_eq!(
        reported, encrypted,
        "case \"an update to a key nothing created\": expected the stream to keep \
         {encrypted:?}, got {reported:?}"
    );

    // The refusal spent nothing: the stream still takes a key KMS resolves.
    let (_, second_key) = emulator.create_key().await;
    let (status, response) = start_encryption(&emulator, stream, &second_key, false).await;
    assert_eq!(
        status, 200,
        "case \"a second resolvable key\": expected the update to be served, got {status} \
         with body {response}"
    );
    let reported = emulator
        .reported_encryption("a second resolvable key", stream)
        .await;
    assert_eq!(
        reported,
        (ENCRYPTION_TYPE_KMS.to_string(), Some(second_key.clone())),
        "case \"a second resolvable key\": expected the stream to report {second_key:?}, \
         got {reported:?}"
    );
}

// 4. A lookup nothing answered refuses nothing
#[tokio::test]
async fn a_lookup_kms_never_answered_refuses_nothing() {
    let emulator = Emulator::without_kms();
    let stream = "no-kms-mounted";
    emulator.active_stream("no KMS mounted", stream).await;

    let (status, response) = start_encryption(&emulator, stream, ABSENT_KEY_UUID, false).await;
    assert_eq!(
        status, 200,
        "case \"no KMS mounted\": a lookup that answered nothing about the key must not \
         refuse the call, got {status} with body {response}"
    );
    let reported = emulator.reported_encryption("no KMS mounted", stream).await;
    assert_eq!(
        reported,
        (
            ENCRYPTION_TYPE_KMS.to_string(),
            Some(ABSENT_KEY_UUID.to_string())
        ),
        "case \"no KMS mounted\": expected the accepted key to be stored, got {reported:?}"
    );
}

// 5. The refusals the stored streams already carry are answered before any key is resolved
#[tokio::test]
async fn a_request_the_stored_streams_refuse_is_refused_whatever_the_key_is() {
    let emulator = Emulator::new();
    let (_, key_arn) = emulator.create_key().await;

    // A stream this account and region do not hold is `ResourceNotFoundException` whether
    // the key resolves or not: the key is not what is wrong with the request.
    for (label, key) in [
        ("a resolvable key", key_arn.as_str()),
        ("a key nothing created", ABSENT_KEY_UUID),
    ] {
        let (status, response) = start_encryption(&emulator, "never-created", key, false).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a stream nothing created must be refused, got {status} with \
             body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {response}"
        );
    }
}
