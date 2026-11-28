// SPDX-License-Identifier: CC0-1.0

//! Cross-ACCOUNT replication of a Secrets Manager secret: the link a primary and the replica
//! of it another account holds carry, and the values that link replicates.
//!
//! The RULE pinned here: **a cross-account replica is a secret of its own that the replica
//! account creates with `CreateSecret` naming the primary it is to receive from
//! (`CrossAccountPrimaryARN`, which forbids every member the primary supplies instead and
//! requires a customer managed key), nothing replicates into it until the primary's owner
//! calls `AssociateCrossAccountReplica` — at most once per primary, and only for a secret in
//! the same Region and partition that names that primary — and from then on the replica
//! serves the primary's current value until the link is removed, which leaves the replica
//! standing with what it already received, reporting `Detached`.** "Links a primary secret in
//! your account to a replica secret in a different Amazon Web Services account and starts
//! replication to this replica" (`AssociateCrossAccountReplica`, the Secrets Manager Smithy
//! model); "Disassociating the replica from the primary does not delete the replica secret or
//! convert the replica secret to a standalone secret. The replica secret remains in the other
//! account and its replication status changes to Detached"
//! (`DisassociateCrossAccountReplica`).
//!
//! TWO crates have to agree for that to hold, which is why the suite lives here: a
//! cross-account replica must name a customer managed key, so KMS is the service that mints
//! the key and decides every use of it, while Secrets Manager holds the link and the values it
//! replicates. The owning crate also still roots its integration tests at a hand-declared
//! `tests/all.rs`, which a fix may not edit, and the router's `tests/all/` picks a new suite up
//! with nothing to declare.
//!
//! The rows differ along what the rule generalises over: the shape and size of the value the
//! primary holds, the form of the `SecretId` the association names the primary by (name, full
//! ARN, partial ARN), whether the association supplies a `ReplicationRoleARN`, and which
//! `ReplicaRestriction` it states — and, for the refusals, each pair and each member the rule
//! excludes.
//!
//! No row writes a key id, a secret ARN or a version id: every identifier asserted on is one
//! the services reported, because any single run's generated id is no part of the rule.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    primary_has_cross_account_replica_message, CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE,
    CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE, CROSS_ACCOUNT_REPLICA_NEEDS_KEY_MESSAGE,
    CROSS_ACCOUNT_REPLICA_NOT_FOUND_MESSAGE, CROSS_ACCOUNT_STATUS_DETACHED,
    CROSS_ACCOUNT_STATUS_PENDING, MARKED_FOR_DELETION_MESSAGE,
    NOT_PERMITTED_ON_CROSS_ACCOUNT_REPLICA_MESSAGE, ONE_CROSS_ACCOUNT_REPLICA_MESSAGE,
    REPLICATION_SUCCEEDED_MESSAGE, REPLICA_STATUS_IN_PROGRESS, REPLICA_STATUS_IN_SYNC,
    SECRET_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every secret below lives in: "The primary and replica secrets must be in the
/// same Amazon Web Services Region and partition" (`AssociateCrossAccountReplica`, the Secrets
/// Manager Smithy model), so one Region holds both sides of every link.
const REGION: &str = "us-west-2";

/// A Region no secret below is created in, for the pair the rule excludes.
const ANOTHER_REGION: &str = "eu-west-1";

/// How long a wait below looks for the state it waits on before giving up, and how often it
/// looks. The ceiling only stops the suite from hanging: what each wait looks for is the STATE
/// the store reaches, never a span passing.
const SETTLE_CEILING: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// Secrets Manager and KMS over ONE engine, the shape `core/router/src/lib.rs` mounts them in:
/// the key a cross-account replica names is minted in KMS, and the envelope calls Secrets
/// Manager makes reach it through the emulator's own in-process transport.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_1 call of `signing_name`, addressed by `target_prefix` and signed for
/// [`REGION`], with whatever status it answered.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Secrets Manager call, with whatever status it answered.
async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call(router, "secretsmanager", "secretsmanager", operation, body).await
}

/// One Secrets Manager call that must succeed, answered as its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = secretsmanager(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// One Secrets Manager call that must be refused, answered as `(error code, message)`.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = secretsmanager(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    let message = parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Message in error body {parsed}"))
        .to_string();
    (
        raw.split('#').next_back().unwrap_or(raw).to_string(),
        message,
    )
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A string member of a JSON object, or `None` when it is absent.
fn member(parsed: &Value, name: &str) -> Option<String> {
    parsed.get(name).and_then(Value::as_str).map(str::to_string)
}

/// A `ClientRequestToken` naming the one version a write stakes, padded to the modelled
/// 32-character minimum of `ClientRequestTokenType`: "If you generate a raw HTTP request to
/// the Secrets Manager service endpoint, then you must generate a ClientRequestToken and
/// include it in the request." (`CreateSecretRequest$ClientRequestToken`.)
fn version_token(label: &str) -> String {
    format!("{label:0<32}")
}

/// Mint a symmetric KMS key in [`REGION`] and answer its ARN: the customer managed key a
/// cross-account replica has to name.
async fn create_key(router: &MultiServiceRouter, case: &str) -> String {
    let (status, created) = call(router, "kms", "TrentService", "CreateKey", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from KMS CreateKey, got {status} with body {created}"
    );
    created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateKey reported no key ARN in {created}"))
        .to_string()
}

/// Create a secret holding `value` and answer its ARN.
async fn create_primary(
    router: &MultiServiceRouter,
    name: &str,
    value: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": value,
            "ClientRequestToken": version_token("primary-v1"),
        }),
        case,
    )
    .await;
    text(&created, "ARN", case)
}

/// Create a cross-account replica of `primary_arn` under a key this call mints, and answer its
/// ARN.
async fn create_replica(
    router: &MultiServiceRouter,
    name: &str,
    primary_arn: &str,
    case: &str,
) -> String {
    let key = create_key(router, case).await;
    let created = ok(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "KmsKeyId": key,
            "CrossAccountPrimaryARN": primary_arn,
            "ClientRequestToken": version_token("replica-create"),
        }),
        case,
    )
    .await;
    text(&created, "ARN", case)
}

/// How a row names the primary in its `SecretId`: "The ARN or name of the primary secret in
/// your account to replicate. For an ARN, we recommend that you specify a complete ARN rather
/// than a partial ARN" (`AssociateCrossAccountReplicaRequest$SecretId`, the Secrets Manager
/// Smithy model).
#[derive(Clone, Copy, Debug)]
enum IdForm {
    Name,
    FullArn,
    PartialArn,
}

/// The `SecretId` a row sends for the primary.
fn secret_id(form: IdForm, name: &str, arn: &str) -> String {
    match form {
        IdForm::Name => name.to_string(),
        IdForm::FullArn => arn.to_string(),
        // The partial ARN is the full ARN without the trailing hyphen and the six random
        // characters the service appends to the secret's name.
        IdForm::PartialArn => arn[..arn.len().saturating_sub(7)].to_string(),
    }
}

/// The same ARN with its Region field replaced, which is how a row names a secret outside the
/// primary's Region without inventing anything else about it.
fn in_another_region(arn: &str) -> String {
    let mut fields: Vec<&str> = arn.split(':').collect();
    fields[3] = ANOTHER_REGION;
    fields.join(":")
}

/// The one `CrossAccountReplicas` entry a read of the primary reports, or `None` while it
/// reports none.
fn replica_entry(described: &Value) -> Option<Value> {
    described
        .get("CrossAccountReplicas")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
}

/// The `CrossAccountPrimary` a read of the replica reports, or `None` while it reports none.
fn primary_link(described: &Value) -> Option<Value> {
    described.get("CrossAccountPrimary").cloned()
}

/// `DescribeSecret` on `secret_id`.
async fn describe(router: &MultiServiceRouter, secret_id: &str, case: &str) -> Value {
    ok(router, "DescribeSecret", json!({"SecretId": secret_id}), case).await
}

/// Look for the replication status of the primary's one entry until it is no longer
/// `InProgress`, and answer the entry.
///
/// What this looks for is the state the store OWES that link — the `InProgress` -> `InSync`
/// move is due at an instant the store recorded, and a request is what materialises it —
/// rather than waiting a span out.
async fn settled_entry(router: &MultiServiceRouter, primary_id: &str, case: &str) -> Value {
    let deadline = Instant::now() + SETTLE_CEILING;
    loop {
        let described = describe(router, primary_id, case).await;
        let entry = replica_entry(&described)
            .unwrap_or_else(|| panic!("case {case:?}: no CrossAccountReplicas in {described}"));
        if member(&entry, "ReplicationStatus").as_deref() != Some(REPLICA_STATUS_IN_PROGRESS) {
            return entry;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the cross-account link never settled, last entry {entry}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Look for `GetSecretValue` on `secret_id` to answer `expected`, which is what replication
/// into a replica carries.
async fn wait_for_value(
    router: &MultiServiceRouter,
    secret_id: &str,
    expected: &str,
    case: &str,
) {
    let deadline = Instant::now() + SETTLE_CEILING;
    loop {
        let (status, parsed) = secretsmanager(
            router,
            "GetSecretValue",
            &json!({"SecretId": secret_id}),
        )
        .await;
        if status == 200 && parsed.get("SecretString").and_then(Value::as_str) == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the replica never served the value its primary holds, last answer \
             {parsed}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// One row of the replication table: what the primary holds, how the association names it, and
/// what the association states about the replication itself.
struct Row {
    case: &'static str,
    value: String,
    updated: String,
    form: IdForm,
    role_arn: Option<String>,
    restriction: Option<&'static str>,
}

/// The table: one row per combination of what the rule generalises over.
fn rows() -> Vec<Row> {
    let role = format!(
        "arn:aws:iam::{}:role/cross-account-replication",
        ::engine::emulator_account_id()
    );
    vec![
        Row {
            case: "a short string, named by name, no role, no restriction stated",
            value: "s3cr3t".to_string(),
            updated: "s3cr3t-rotated".to_string(),
            form: IdForm::Name,
            role_arn: None,
            restriction: None,
        },
        Row {
            case: "a JSON document, named by full ARN, with a replication role, SameOrg",
            value: json!({"username": "admin", "password": "p@ssw0rd"}).to_string(),
            updated: json!({"username": "admin", "password": "r0tated"}).to_string(),
            form: IdForm::FullArn,
            role_arn: Some(role.clone()),
            restriction: Some("SameOrg"),
        },
        Row {
            case: "a long string, named by partial ARN, with a replication role, NoRestriction",
            value: "x".repeat(1_000),
            updated: "y".repeat(999),
            form: IdForm::PartialArn,
            role_arn: Some(role),
            restriction: Some("NoRestriction"),
        },
        Row {
            case: "a one-character value, named by name, no role, SameOrg",
            value: "x".to_string(),
            updated: "z".to_string(),
            form: IdForm::Name,
            role_arn: None,
            restriction: Some("SameOrg"),
        },
    ]
}

#[tokio::test]
async fn an_associated_cross_account_replica_serves_the_value_its_primary_holds() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let primary_name = format!("primary/{index}");
        let replica_name = format!("replica/{index}");
        let primary_arn = create_primary(&router, &primary_name, &row.value, case).await;
        let replica_arn = create_replica(&router, &replica_name, &primary_arn, case).await;

        // Before the association nothing replicates: the replica names its primary and
        // reports `Pending`, "which indicates that replication has not yet started".
        let described = describe(&router, &replica_name, case).await;
        let link = primary_link(&described)
            .unwrap_or_else(|| panic!("case {case:?}: no CrossAccountPrimary in {described}"));
        assert_eq!(
            (
                member(&link, "PrimarySecretARN"),
                member(&link, "ReplicationStatus")
            ),
            (
                Some(primary_arn.clone()),
                Some(CROSS_ACCOUNT_STATUS_PENDING.to_string())
            ),
            "case {case:?}: expected the replica to name its primary and report Pending, got \
             {described}"
        );
        assert!(
            replica_entry(&described).is_none(),
            "case {case:?}: a replica has no replica of its own to report, got {described}"
        );

        let mut request = json!({
            "SecretId": secret_id(row.form, &primary_name, &primary_arn),
            "CrossAccountReplicaARN": replica_arn.clone(),
        });
        let members = request
            .as_object_mut()
            .expect("the request body is an object");
        if let Some(role) = &row.role_arn {
            members.insert("ReplicationRoleARN".to_string(), json!(role));
        }
        if let Some(restriction) = row.restriction {
            members.insert("ReplicaRestriction".to_string(), json!(restriction));
        }
        let associated = ok(&router, "AssociateCrossAccountReplica", request, case).await;
        assert_eq!(
            (
                member(&associated, "ARN"),
                member(&associated, "CrossAccountReplicaARN"),
                member(&associated, "ReplicationStatus")
            ),
            (
                Some(primary_arn.clone()),
                Some(replica_arn.clone()),
                Some(REPLICA_STATUS_IN_PROGRESS.to_string())
            ),
            "case {case:?}: expected the association to report the pair it linked and that \
             replication has started, got {associated}"
        );

        // The clock owes the link its `InSync` move, and the entry the primary reports
        // carries the role the association stated and nothing it did not.
        let entry = settled_entry(&router, &primary_name, case).await;
        assert_eq!(
            (
                member(&entry, "ReplicaSecretARN"),
                member(&entry, "ReplicationStatus"),
                member(&entry, "StatusMessage"),
                member(&entry, "ReplicationRoleARN")
            ),
            (
                Some(replica_arn.clone()),
                Some(REPLICA_STATUS_IN_SYNC.to_string()),
                Some(REPLICATION_SUCCEEDED_MESSAGE.to_string()),
                row.role_arn.clone()
            ),
            "case {case:?}: expected the settled entry for the replica it replicates to, got \
             {entry}"
        );
        let described = describe(&router, &replica_name, case).await;
        let link = primary_link(&described)
            .unwrap_or_else(|| panic!("case {case:?}: no CrossAccountPrimary in {described}"));
        assert_eq!(
            member(&link, "ReplicationStatus"),
            Some(REPLICA_STATUS_IN_SYNC.to_string()),
            "case {case:?}: expected the replica to report replication complete, got {described}"
        );

        // What replication carries is the VALUE: the replica serves what the primary holds,
        // read through the replica's own name, and goes on carrying every later write.
        wait_for_value(&router, &replica_name, &row.value, case).await;
        ok(
            &router,
            "PutSecretValue",
            json!({
                "SecretId": primary_name.clone(),
                "SecretString": row.updated.clone(),
                "ClientRequestToken": version_token("primary-v2"),
            }),
            case,
        )
        .await;
        wait_for_value(&router, &replica_name, &row.updated, case).await;
    }
}

#[tokio::test]
async fn the_association_is_refused_for_every_pair_the_rule_excludes() {
    let case = "the pairs no association links";
    let router = emulator();
    let primary_arn = create_primary(&router, "primary", "s3cr3t", case).await;
    let replica_arn = create_replica(&router, "replica", &primary_arn, case).await;
    // A secret of this account that is no replica at all, and one that is the replica of
    // ANOTHER primary: neither names the primary this association is made on.
    let standalone_arn = create_primary(&router, "standalone", "stored", case).await;
    let other_primary_arn = create_primary(&router, "other-primary", "stored", case).await;
    let other_replica_arn =
        create_replica(&router, "other-replica", &other_primary_arn, case).await;

    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "a primary this Region holds no secret under",
            json!({"SecretId": "never-created", "CrossAccountReplicaARN": replica_arn.clone()}),
            "ResourceNotFoundException",
            SECRET_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a replica ARN no secret carries",
            json!({
                "SecretId": "primary",
                "CrossAccountReplicaARN": format!("{standalone_arn}-not-a-secret"),
            }),
            "ResourceNotFoundException",
            SECRET_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a replica outside the primary's Region",
            json!({
                "SecretId": "primary",
                "CrossAccountReplicaARN": in_another_region(&replica_arn),
            }),
            "InvalidParameterException",
            CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE.to_string(),
        ),
        (
            "a secret that is no cross-account replica",
            json!({"SecretId": "primary", "CrossAccountReplicaARN": standalone_arn.clone()}),
            "InvalidRequestException",
            format!("The secret {standalone_arn} is not a cross-account replica of the secret "),
        ),
        (
            "a replica that names another primary",
            json!({"SecretId": "primary", "CrossAccountReplicaARN": other_replica_arn.clone()}),
            "InvalidRequestException",
            format!("The secret {other_replica_arn} is not a cross-account replica of the \
                     secret "),
        ),
        (
            "the association made on the replica rather than the primary",
            json!({"SecretId": "replica", "CrossAccountReplicaARN": replica_arn.clone()}),
            "InvalidParameterException",
            NOT_PERMITTED_ON_CROSS_ACCOUNT_REPLICA_MESSAGE.to_string(),
        ),
    ];
    for (row, request, expected_code, expected_message) in rows {
        let (code, message) = refusal(
            &router,
            "AssociateCrossAccountReplica",
            request,
            row,
        )
        .await;
        assert_eq!(
            code, expected_code,
            "case {row:?}: expected {expected_code}, got {code} with message {message:?}"
        );
        assert!(
            message.starts_with(&expected_message),
            "case {row:?}: expected a message starting {expected_message:?}, got {message:?}"
        );
    }
    // Nothing above linked anything, so the primary still reports no replica at all.
    let described = describe(&router, "primary", case).await;
    assert!(
        replica_entry(&described).is_none(),
        "case {case:?}: a refused association links nothing, got {described}"
    );

    // One primary, at most one replica: a second replica of this primary is a quota refusal,
    // while the association of the replica already linked is idempotent and starts
    // replication over.
    ok(
        &router,
        "AssociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn.clone()}),
        case,
    )
    .await;
    let second_arn = create_replica(&router, "second-replica", &primary_arn, case).await;
    let (code, message) = refusal(
        &router,
        "AssociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": second_arn}),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "LimitExceededException",
            ONE_CROSS_ACCOUNT_REPLICA_MESSAGE
        ),
        "case {case:?}: expected the one-replica quota refusal, got {code} {message:?}"
    );
    let repaired = ok(
        &router,
        "AssociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn.clone()}),
        case,
    )
    .await;
    assert_eq!(
        member(&repaired, "ReplicationStatus"),
        Some(REPLICA_STATUS_IN_PROGRESS.to_string()),
        "case {case:?}: expected a repeat for the same replica to start replication over, got \
         {repaired}"
    );

    // And a secret on its way out takes no part in replication.
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "other-primary"}),
        case,
    )
    .await;
    let (code, message) = refusal(
        &router,
        "AssociateCrossAccountReplica",
        json!({
            "SecretId": "other-primary",
            "CrossAccountReplicaARN": other_replica_arn,
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("InvalidRequestException", MARKED_FOR_DELETION_MESSAGE),
        "case {case:?}: expected the deletion-state refusal, got {code} {message:?}"
    );
}

#[tokio::test]
async fn a_cross_account_replica_create_carries_only_what_a_replica_can_have() {
    let case = "the members a cross-account replica create cannot carry";
    let router = emulator();
    let primary_arn = create_primary(&router, "primary", "s3cr3t", case).await;
    let key = create_key(&router, case).await;

    // Each row is the create's own members beside `CrossAccountPrimaryARN`, and the sentence
    // the rule refuses it with. The members a replica receives from its primary instead:
    // "You can't include SecretString, SecretBinary, Description, AddReplicaRegions, or
    // Type" (`CreateSecretRequest$CrossAccountPrimaryARN`, the Secrets Manager Smithy model).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a value of its own",
            json!({"KmsKeyId": key.clone(), "SecretString": "its-own"}),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "a binary value of its own",
            json!({"KmsKeyId": key.clone(), "SecretBinary": "AAEC"}),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "a description of its own",
            json!({"KmsKeyId": key.clone(), "Description": "its own description"}),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "a type of its own",
            json!({"KmsKeyId": key.clone(), "Type": "SomeType"}),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "replica Regions of its own",
            json!({
                "KmsKeyId": key.clone(),
                "AddReplicaRegions": [{"Region": ANOTHER_REGION}],
            }),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "no KMS key at all",
            json!({}),
            CROSS_ACCOUNT_REPLICA_NEEDS_KEY_MESSAGE,
        ),
        (
            "the Amazon Web Services managed key",
            json!({"KmsKeyId": "alias/aws/secretsmanager"}),
            CROSS_ACCOUNT_REPLICA_NEEDS_KEY_MESSAGE,
        ),
        (
            "the empty key, which names the managed key too",
            json!({"KmsKeyId": ""}),
            CROSS_ACCOUNT_REPLICA_NEEDS_KEY_MESSAGE,
        ),
    ];
    for (index, (row, members, expected)) in rows.iter().enumerate() {
        let mut body = members.clone();
        let object = body.as_object_mut().expect("every row's body is an object");
        object.insert("Name".to_string(), json!(format!("refused/{index}")));
        object.insert(
            "CrossAccountPrimaryARN".to_string(),
            json!(primary_arn.clone()),
        );
        object.insert(
            "ClientRequestToken".to_string(),
            json!(version_token("refused-create")),
        );
        let (code, message) = refusal(&router, "CreateSecret", body, row).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", *expected),
            "case {row:?}: expected the parameter refusal {expected:?}, got {code} {message:?}"
        );
        let (status, _) = secretsmanager(
            &router,
            "DescribeSecret",
            &json!({"SecretId": format!("refused/{index}")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: a refused create stores nothing, so the name holds no secret"
        );
    }

    // A primary outside the replica's Region and partition is refused the same way, and the
    // create that names a primary in this Region under a customer managed key is served.
    let (code, message) = refusal(
        &router,
        "CreateSecret",
        json!({
            "Name": "refused/another-region",
            "KmsKeyId": key.clone(),
            "CrossAccountPrimaryARN": in_another_region(&primary_arn),
            "ClientRequestToken": version_token("refused-region"),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE
        ),
        "case {case:?}: expected the Region refusal, got {code} {message:?}"
    );
    create_replica(&router, "replica", &primary_arn, case).await;
}

#[tokio::test]
async fn disassociation_detaches_the_replica_and_frees_the_primary_for_deletion() {
    let case = "the link removed by the primary";
    let router = emulator();
    let primary_arn = create_primary(&router, "primary", "replicated", case).await;
    let replica_arn = create_replica(&router, "replica", &primary_arn, case).await;
    ok(
        &router,
        "AssociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn.clone()}),
        case,
    )
    .await;
    settled_entry(&router, "primary", case).await;
    wait_for_value(&router, "replica", "replicated", case).await;

    // While the replica is associated the primary cannot be deleted, and the refusal names
    // the secret and the replica in the way.
    let (code, message) = refusal(
        &router,
        "DeleteSecret",
        json!({"SecretId": "primary"}),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            primary_has_cross_account_replica_message(&primary_arn, &replica_arn).as_str()
        ),
        "case {case:?}: expected the associated-replica refusal, got {code} {message:?}"
    );

    // An ARN this primary has no association with names no association to remove.
    let (code, message) = refusal(
        &router,
        "DisassociateCrossAccountReplica",
        json!({
            "SecretId": "primary",
            "CrossAccountReplicaARN": in_another_region(&replica_arn),
        }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "ResourceNotFoundException",
            CROSS_ACCOUNT_REPLICA_NOT_FOUND_MESSAGE
        ),
        "case {case:?}: expected the missing-association refusal, got {code} {message:?}"
    );

    let disassociated = ok(
        &router,
        "DisassociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn.clone()}),
        case,
    )
    .await;
    assert_eq!(
        (
            member(&disassociated, "ARN"),
            member(&disassociated, "CrossAccountReplicaARN")
        ),
        (Some(primary_arn.clone()), Some(replica_arn.clone())),
        "case {case:?}: expected the disassociation to report the pair it unlinked, got \
         {disassociated}"
    );

    // The primary holds no replica any more; the replica remains, reporting `Detached` and
    // still serving the value already replicated into it.
    let described = describe(&router, "primary", case).await;
    assert!(
        replica_entry(&described).is_none(),
        "case {case:?}: expected no CrossAccountReplicas after the disassociation, got \
         {described}"
    );
    let described = describe(&router, "replica", case).await;
    let link = primary_link(&described)
        .unwrap_or_else(|| panic!("case {case:?}: no CrossAccountPrimary in {described}"));
    assert_eq!(
        (
            member(&link, "PrimarySecretARN"),
            member(&link, "ReplicationStatus")
        ),
        (
            Some(primary_arn.clone()),
            Some(CROSS_ACCOUNT_STATUS_DETACHED.to_string())
        ),
        "case {case:?}: expected the replica to report its primary and Detached, got {described}"
    );
    let read = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "replica"}),
        case,
    )
    .await;
    assert_eq!(
        member(&read, "SecretString"),
        Some("replicated".to_string()),
        "case {case:?}: expected the replica to keep the value replicated into it, got {read}"
    );

    // Nothing reaches the replica after the disassociation: a later write to the primary is
    // the primary's alone.
    ok(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": "primary",
            "SecretString": "after-the-disassociation",
            "ClientRequestToken": version_token("primary-v2"),
        }),
        case,
    )
    .await;
    let read = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "replica"}),
        case,
    )
    .await;
    assert_eq!(
        member(&read, "SecretString"),
        Some("replicated".to_string()),
        "case {case:?}: expected no update to reach a detached replica, got {read}"
    );

    // A second disassociation has nothing left to remove, and the primary is now deletable.
    let (code, _) = refusal(
        &router,
        "DisassociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn}),
        case,
    )
    .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected the second disassociation to find no association, got {code}"
    );
    let deleted = ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "primary"}),
        case,
    )
    .await;
    assert_eq!(
        member(&deleted, "ARN"),
        Some(primary_arn),
        "case {case:?}: expected the primary to be deletable once nothing replicates, got \
         {deleted}"
    );
}

#[tokio::test]
async fn stop_replication_to_replica_makes_a_cross_account_replica_standalone() {
    let case = "the link stopped by the replica";
    let router = emulator();
    let primary_arn = create_primary(&router, "primary", "replicated", case).await;
    let replica_arn = create_replica(&router, "replica", &primary_arn, case).await;
    ok(
        &router,
        "AssociateCrossAccountReplica",
        json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn.clone()}),
        case,
    )
    .await;
    settled_entry(&router, "primary", case).await;
    wait_for_value(&router, "replica", "replicated", case).await;

    // "To convert a detached replica into a standalone secret, call
    // StopReplicationToReplica on the replica secret."
    let stopped = ok(
        &router,
        "StopReplicationToReplica",
        json!({"SecretId": "replica"}),
        case,
    )
    .await;
    assert_eq!(
        member(&stopped, "ARN"),
        Some(replica_arn.clone()),
        "case {case:?}: expected the promotion to report the replica's own ARN, got {stopped}"
    );

    // The secret stands on its own: it reports no primary, keeps the value replicated into
    // it, and takes writes of its own.
    let described = describe(&router, "replica", case).await;
    assert!(
        primary_link(&described).is_none(),
        "case {case:?}: expected a standalone secret to report no CrossAccountPrimary, got \
         {described}"
    );
    let read = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": "replica"}),
        case,
    )
    .await;
    assert_eq!(
        member(&read, "SecretString"),
        Some("replicated".to_string()),
        "case {case:?}: expected the promoted secret to keep its value, got {read}"
    );
    ok(
        &router,
        "PutSecretValue",
        json!({
            "SecretId": "replica",
            "SecretString": "its-own-now",
            "ClientRequestToken": version_token("standalone-v2"),
        }),
        case,
    )
    .await;

    // The primary's entry records what became of the replica — replication was stopped by
    // the replica — and is no association, so the primary is deletable.
    let described = describe(&router, "primary", case).await;
    let entry = replica_entry(&described)
        .unwrap_or_else(|| panic!("case {case:?}: no CrossAccountReplicas in {described}"));
    assert_eq!(
        (
            member(&entry, "ReplicaSecretARN"),
            member(&entry, "ReplicationStatus")
        ),
        (
            Some(replica_arn),
            Some(CROSS_ACCOUNT_STATUS_DETACHED.to_string())
        ),
        "case {case:?}: expected the primary to report the replica as Detached, got {described}"
    );
    ok(
        &router,
        "DeleteSecret",
        json!({"SecretId": "primary"}),
        case,
    )
    .await;
}
