// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for cross-region replication as a REGIONAL resource, driven
//! over the wire through the multi-service router.
//!
//! The rules under test:
//!
//! * `ReplicateSecretToRegions` creates a separate secret in each Region it adds:
//!   it carries an ARN of its own — the primary's with the Region changed — it is
//!   read in its own Region, and it is not written there, because it tracks the
//!   primary;
//! * a secret NAME identifies at most one secret in a Region, so the same name in
//!   another Region is another secret;
//! * a primary with live replicas is not deleted until they are removed;
//! * a replica's `ReplicationStatusType` reports the status its creation reached
//!   AND the `StatusMessage` that status says about it: one the service created
//!   reports that replication succeeded, and one it could not reports `Failed`
//!   with the reason — a destination Region that already holds a secret of this
//!   name, which `ForceOverwriteReplicaSecret` overwrites, or a KMS key that
//!   cannot be used;
//! * `RemoveRegionsFromReplication` deletes the replica, and
//!   `StopReplicationToReplica`, called in the replica's Region, unlinks it and
//!   leaves the secret it had become — while the same call made where the secret
//!   is NOT a replica is refused with the sentence that secret's replication state
//!   calls for;
//! * `PrimaryRegion` belongs to a secret that takes part in replication, so the
//!   `primary-region` filter selects replicas and replicated primaries and never a
//!   secret this service reports no `PrimaryRegion` for;
//! * cross-ACCOUNT replication is the same resource seen from two secrets of their
//!   own: the replica account creates its secret naming the primary
//!   (`CreateSecretRequest$CrossAccountPrimaryARN`), nothing replicates until the
//!   primary's owner associates it, and from then on the replica serves the
//!   primary's current value until the link is removed, which leaves it `Detached`
//!   with what it already received.
//!
//! Every test is a TABLE over the class its rule generalises: the Region a
//! replica is placed in and how many, the form of the `SecretId`, the form of the
//! replica's KMS key, which operation writes to the replica, and whether the
//! caller asked for an overwrite.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::{
    primary_has_cross_account_replica_message, primary_is_replicated_message,
    ComAmazonawsSecretsmanagerSecretsmanagerState, CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE,
    CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE, CROSS_ACCOUNT_REPLICA_NEEDS_KEY_MESSAGE,
    CROSS_ACCOUNT_STATUS_DETACHED, CROSS_ACCOUNT_STATUS_PENDING,
    NOT_PERMITTED_ON_CROSS_ACCOUNT_REPLICA_MESSAGE, NOT_PERMITTED_ON_REPLICA_MESSAGE,
    NOT_PERMITTED_ON_REPLICATED_PRIMARY_MESSAGE, NOT_PERMITTED_ON_STANDALONE_MESSAGE,
    ONE_CROSS_ACCOUNT_REPLICA_MESSAGE, REPLICATION_FAILED_PREFIX, REPLICATION_SUCCEEDED_MESSAGE,
    REPLICA_NAME_COLLISION_MESSAGE, REPLICA_STATUS_FAILED, REPLICA_STATUS_IN_PROGRESS,
    REPLICA_STATUS_IN_SYNC, SECRET_QUOTA,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The harness that mounts KMS beside this service and mints a real key through KMS's
// own operations, written ONCE for this test binary rather than per suite: a
// cross-account replica has to name a customer managed key.
use crate::secret_kms_key_state::Emulator as KeyedEmulator;

/// The Region the primary of every secret below is created in.
const PRIMARY_REGION: &str = "us-west-2";

/// How long a wait below looks for the state it waits on before giving up, and how
/// often it looks. The ceiling only stops a suite from hanging: what each wait
/// looks for is the STATE the store reaches, never a span passing.
const REPLICA_WAIT: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// Secrets Manager and KMS over one shared engine, behind the same
/// `MultiServiceRouter` the production router builds: a replica names a KMS key
/// in its own Region, and whether that key can be used is KMS's answer.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// Fill the store until it holds `held` secrets, by storing records straight
    /// into it: what the bound under test is about is how many secrets the store
    /// HOLDS, and reaching the bound through the API would say the same thing a
    /// thousand requests more slowly.
    fn fill_store_to(&self, held: usize) {
        let state = self
            .engine
            .state::<ComAmazonawsSecretsmanagerSecretsmanagerState>();
        let mut store = state.lock();
        let mut index = 0;
        while store.secrets.len() < held {
            let name = format!("filler/{index}");
            index += 1;
            store.insert(
                service_com_amazonaws_secretsmanager_secretsmanager::state::Secret::new(
                    name.clone(),
                    format!(
                        "arn:aws:secretsmanager:{PRIMARY_REGION}:{}:secret:{name}-AbCdEf",
                        engine::EMULATOR_ACCOUNT_ID
                    ),
                    PRIMARY_REGION.to_string(),
                    std::time::SystemTime::now(),
                ),
            );
        }
    }

    /// POST one `awsJson1_1` Secrets Manager operation signed for `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let body = crate::secret_client_request_token::with_staked_version(operation, body);
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://secretsmanager.{region}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("secretsmanager.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{region}/secretsmanager/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One operation signed for `region` that must succeed.
    async fn ok_in(&self, region: &str, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call_in(region, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {operation} in {region}, got {status} with body \
             {parsed}"
        );
        parsed
    }

    /// One operation signed for `region` that must be refused, answered as
    /// `(error code, message)`.
    async fn err_in(
        &self,
        region: &str,
        operation: &str,
        body: Value,
        case: &str,
    ) -> (String, String) {
        let (status, parsed) = self.call_in(region, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 client error from {operation} in {region}, got \
             {status} with body {parsed}"
        );
        let raw = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
        let code = raw.split('#').next_back().unwrap_or(raw).to_string();
        let message = parsed
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (code, message)
    }

    /// One operation in the primary's Region that must succeed.
    async fn ok(&self, operation: &str, body: Value, case: &str) -> Value {
        self.ok_in(PRIMARY_REGION, operation, body, case).await
    }

    /// Create a secret in the primary Region holding `value`, and answer its ARN.
    async fn create(&self, name: &str, value: &str, case: &str) -> String {
        let created = self
            .ok(
                "CreateSecret",
                json!({"Name": name, "SecretString": value}),
                case,
            )
            .await;
        text(&created, "ARN", case)
    }

    /// Replicate `name` into every `(Region, KmsKeyId)` of `replicas`, and answer
    /// the `ReplicationStatus` the call reports.
    async fn replicate(
        &self,
        name: &str,
        replicas: &[(&str, Option<&str>)],
        force: bool,
        case: &str,
    ) -> Vec<Value> {
        let requested: Vec<Value> = replicas
            .iter()
            .map(|(region, key)| match key {
                Some(key) => json!({"Region": region, "KmsKeyId": key}),
                None => json!({"Region": region}),
            })
            .collect();
        let replicated = self
            .ok(
                "ReplicateSecretToRegions",
                json!({
                    "SecretId": name,
                    "AddReplicaRegions": requested,
                    "ForceOverwriteReplicaSecret": force,
                }),
                case,
            )
            .await;
        replicas_of(&replicated, case)
    }

    /// The replica entry for `region` once its creation has settled, read back
    /// through `DescribeSecret` on the primary.
    ///
    /// Looks for the status the store OWES that replica until it has been
    /// reported, rather than waiting a span out: the `InProgress` -> `InSync`
    /// move is due at an instant the store recorded, and a request is what
    /// materialises it. A replica the service could not create has nothing left
    /// owed to it, so its `Failed` entry ends the wait too.
    async fn settled_replica(&self, name: &str, region: &str, case: &str) -> Value {
        let deadline = Instant::now() + REPLICA_WAIT;
        loop {
            let described = self
                .ok("DescribeSecret", json!({"SecretId": name}), case)
                .await;
            let entry = replica_for(&described, region, case);
            if member(&entry, "Status").as_deref() != Some(REPLICA_STATUS_IN_PROGRESS) {
                return entry;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the replica in {region} never settled, last entry {entry}"
            );
            tokio::time::sleep(LOOK_INTERVAL).await;
        }
    }
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
    parsed
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `ReplicationStatus` entries a response reports.
fn replicas_of(parsed: &Value, case: &str) -> Vec<Value> {
    parsed
        .get("ReplicationStatus")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no ReplicationStatus in {parsed}"))
        .clone()
}

/// The one `ReplicationStatus` entry for `region`.
fn replica_for(parsed: &Value, region: &str, case: &str) -> Value {
    replicas_of(parsed, case)
        .into_iter()
        .find(|entry| member(entry, "Region").as_deref() == Some(region))
        .unwrap_or_else(|| panic!("case {case:?}: no replica for {region} in {parsed}"))
}

/// How a row names the replica it addresses in its own Region: "The name of the
/// secret or the replica ARN" (`StopReplicationToReplicaRequest$SecretId`, this
/// service's Smithy model), plus the partial ARN every `SecretId` accepts.
#[derive(Clone, Copy, Debug)]
enum IdForm {
    Name,
    FullArn,
    PartialArn,
}

/// The `SecretId` a row sends for the replica.
fn secret_id(form: IdForm, name: &str, arn: &str) -> String {
    match form {
        IdForm::Name => name.to_string(),
        IdForm::FullArn => arn.to_string(),
        IdForm::PartialArn => arn[..arn.len().saturating_sub(7)].to_string(),
    }
}

#[tokio::test]
async fn a_replica_is_a_separate_secret_in_its_own_region() {
    // The rows run over the Region the replica is placed in and the form of the
    // `SecretId` a read in that Region names it by: the rule is about the
    // replica being its own regional secret, not about which Region or which
    // form of id.
    let rows: &[(&str, &str, IdForm)] = &[
        ("a replica in us-east-1, by name", "us-east-1", IdForm::Name),
        (
            "a replica in eu-west-1, by its own ARN",
            "eu-west-1",
            IdForm::FullArn,
        ),
        (
            "a replica in ap-northeast-1, by its own partial ARN",
            "ap-northeast-1",
            IdForm::PartialArn,
        ),
    ];
    for (index, (case, replica_region, form)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("regional/replica-{index}");
        let value = format!("value-{index}");
        let primary_arn = emulator.create(&name, &value, case).await;
        emulator
            .replicate(&name, &[(replica_region, None)], false, case)
            .await;
        let entry = emulator.settled_replica(&name, replica_region, case).await;
        assert_eq!(
            member(&entry, "Status"),
            Some(REPLICA_STATUS_IN_SYNC.to_string()),
            "case {case:?}: expected the replica to have been created, got {entry}"
        );

        // The replica is a secret of its own in its Region: its ARN is the
        // primary's with the Region changed, so it is neither the primary's ARN
        // nor in the primary's Region.
        let described = emulator
            .ok_in(replica_region, "DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        let replica_arn = text(&described, "ARN", case);
        assert_ne!(
            replica_arn, primary_arn,
            "case {case:?}: the replica must carry an ARN of its own"
        );
        assert!(
            replica_arn.contains(&format!(":{replica_region}:")),
            "case {case:?}: expected a {replica_region}-qualified ARN, got {replica_arn:?}"
        );
        assert_eq!(
            replica_arn,
            primary_arn.replace(PRIMARY_REGION, replica_region),
            "case {case:?}: the replica ARN is the primary's with the Region changed"
        );
        assert_eq!(
            member(&described, "PrimaryRegion"),
            Some(PRIMARY_REGION.to_string()),
            "case {case:?}: the replica reports the Region its primary is in, got {described}"
        );

        // It is READ in its own Region, by any of the documented forms of id,
        // and it holds the primary's value.
        let sent = secret_id(*form, &name, &replica_arn);
        let read = emulator
            .ok_in(replica_region, "GetSecretValue", json!({"SecretId": sent}), case)
            .await;
        assert_eq!(
            member(&read, "SecretString"),
            Some(value.clone()),
            "case {case:?}: expected the replica to hold the primary's value, got {read}"
        );
        assert_eq!(
            text(&read, "ARN", case),
            replica_arn,
            "case {case:?}: a read in the replica's Region answers with the replica's ARN"
        );

        // And it is NOT written there: every write goes to the primary.
        let writes: &[(&str, Value)] = &[
            (
                "PutSecretValue",
                json!({"SecretId": &name, "SecretString": "written to the replica"}),
            ),
            (
                "UpdateSecret",
                json!({"SecretId": &name, "Description": "written to the replica"}),
            ),
            (
                "TagResource",
                json!({"SecretId": &name, "Tags": [{"Key": "k", "Value": "v"}]}),
            ),
            ("DeleteSecret", json!({"SecretId": &name})),
            (
                "RemoveRegionsFromReplication",
                json!({"SecretId": &name, "RemoveReplicaRegions": [PRIMARY_REGION]}),
            ),
        ];
        for (operation, body) in writes {
            let (code, message) = emulator
                .err_in(replica_region, operation, body.clone(), case)
                .await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected {operation} on the replica to be refused, got {code} \
                 {message:?}"
            );
            assert_eq!(
                message, NOT_PERMITTED_ON_REPLICA_MESSAGE,
                "case {case:?}: expected the replica refusal sentence from {operation}"
            );
        }

        // The write that DID go to the primary reaches the replica.
        emulator
            .ok(
                "PutSecretValue",
                json!({"SecretId": &name, "SecretString": "rotated on the primary"}),
                case,
            )
            .await;
        let propagated = emulator
            .ok_in(replica_region, "GetSecretValue", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            member(&propagated, "SecretString"),
            Some("rotated on the primary".to_string()),
            "case {case:?}: expected the primary's new value to reach the replica, got \
             {propagated}"
        );

        // The primary cannot be deleted while the replica is live.
        let (code, message) = emulator
            .err_in(PRIMARY_REGION, "DeleteSecret", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the primary delete to be refused, got {code} {message:?}"
        );
        assert_eq!(
            message,
            primary_is_replicated_message(&primary_arn, &[(*replica_region).to_string()]),
            "case {case:?}: expected the refusal to name the secret and the Region in the way"
        );

        // Removing the Region deletes the replica, and then the primary goes.
        emulator
            .ok(
                "RemoveRegionsFromReplication",
                json!({"SecretId": &name, "RemoveReplicaRegions": [replica_region]}),
                case,
            )
            .await;
        let (code, _) = emulator
            .err_in(replica_region, "DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected the replica to be gone from {replica_region}"
        );
        emulator
            .ok("DeleteSecret", json!({"SecretId": &name}), case)
            .await;
    }
}

#[tokio::test]
async fn a_secret_name_is_free_in_every_other_region() {
    // The rows run over how many Regions hold a secret of the one name: the
    // namespace is per Region, so each of them holds a secret of its own with
    // its own ARN and its own value.
    let rows: &[(&str, &[&str])] = &[
        ("one other Region", &["us-east-1"]),
        ("two other Regions", &["us-east-1", "eu-central-1"]),
        (
            "three other Regions",
            &["us-east-1", "eu-central-1", "sa-east-1"],
        ),
    ];
    for (index, (case, regions)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("per-region/name-{index}");
        let primary_arn = emulator.create(&name, "the primary's value", case).await;
        let mut seen = vec![primary_arn.clone()];
        for region in *regions {
            let value = format!("the {region} value");
            let created = emulator
                .ok_in(
                    region,
                    "CreateSecret",
                    json!({"Name": &name, "SecretString": &value}),
                    case,
                )
                .await;
            let arn = text(&created, "ARN", case);
            assert!(
                !seen.contains(&arn),
                "case {case:?}: every Region's secret of this name is its own, got {arn:?} twice"
            );
            seen.push(arn);
            let read = emulator
                .ok_in(region, "GetSecretValue", json!({"SecretId": &name}), case)
                .await;
            assert_eq!(
                member(&read, "SecretString"),
                Some(value),
                "case {case:?}: expected {region} to answer its own secret, got {read}"
            );
        }
        // The primary is untouched by every one of them.
        let read = emulator
            .ok("GetSecretValue", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            member(&read, "SecretString"),
            Some("the primary's value".to_string()),
            "case {case:?}: expected the primary's own value, got {read}"
        );
    }
}

#[tokio::test]
async fn a_replica_reports_the_status_message_of_the_status_it_reached() {
    // A key that does not exist is asked about through KMS, so the row names the
    // forms of `ReplicaRegionType$KmsKeyId` — an alias and a key id — beside the
    // absent member, which means `aws/secretsmanager` and always exists.
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("no KMS key named", None, true),
        ("the managed key's own alias", Some("alias/aws/secretsmanager"), true),
        ("an alias no key carries", Some("alias/not-a-key"), false),
        (
            "a key id nothing was minted under",
            Some("9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d"),
            false,
        ),
    ];
    for (index, (case, key, usable)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("replica-status/{index}");
        emulator.create(&name, "s3cret", case).await;
        let replica_region = "us-east-1";
        emulator
            .replicate(&name, &[(replica_region, *key)], false, case)
            .await;
        let entry = emulator.settled_replica(&name, replica_region, case).await;
        if *usable {
            assert_eq!(
                member(&entry, "Status"),
                Some(REPLICA_STATUS_IN_SYNC.to_string()),
                "case {case:?}: expected the replica to be created, got {entry}"
            );
            assert_eq!(
                member(&entry, "StatusMessage"),
                Some(REPLICATION_SUCCEEDED_MESSAGE.to_string()),
                "case {case:?}: expected the created replica to report that replication \
                 succeeded, got {entry}"
            );
            emulator
                .ok_in(replica_region, "DescribeSecret", json!({"SecretId": &name}), case)
                .await;
            continue;
        }
        assert_eq!(
            member(&entry, "Status"),
            Some(REPLICA_STATUS_FAILED.to_string()),
            "case {case:?}: expected a replica under an unusable key to fail, got {entry}"
        );
        let message = member(&entry, "StatusMessage").unwrap_or_default();
        assert!(
            message.starts_with(REPLICATION_FAILED_PREFIX)
                && message.contains("can't encrypt the secret value:"),
            "case {case:?}: a failed replica states that replication failed and what it could \
             not do with the key, got {entry}"
        );
        assert!(
            message.contains("Error Code: NotFoundException;"),
            "case {case:?}: the reason relays the refusal KMS stated about the key, got {entry}"
        );
        // Nothing exists in that Region: a replica that failed was not created.
        let (code, _) = emulator
            .err_in(replica_region, "DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: a failed replica is no secret in its Region, got {code}"
        );

        // And it is recoverable: remove the Region and replicate again under a
        // key that can be used.
        emulator
            .ok(
                "RemoveRegionsFromReplication",
                json!({"SecretId": &name, "RemoveReplicaRegions": [replica_region]}),
                case,
            )
            .await;
        emulator
            .replicate(&name, &[(replica_region, None)], false, case)
            .await;
        let recovered = emulator.settled_replica(&name, replica_region, case).await;
        assert_eq!(
            member(&recovered, "Status"),
            Some(REPLICA_STATUS_IN_SYNC.to_string()),
            "case {case:?}: expected the replica to be recoverable, got {recovered}"
        );
    }
}

#[tokio::test]
async fn a_name_the_destination_region_already_holds_fails_until_it_is_overwritten() {
    // The rows run over whether the caller asks for the overwrite, which is the
    // one thing that decides this: "Specifies whether to overwrite a secret with
    // the same name in the destination Region."
    let rows: &[(&str, bool)] = &[
        ("without ForceOverwriteReplicaSecret", false),
        ("with ForceOverwriteReplicaSecret", true),
    ];
    for (index, (case, force)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("collide/{index}");
        emulator.create(&name, "the primary's value", case).await;
        let replica_region = "us-east-1";
        // A secret of its own in the destination Region, which is what the
        // replica would overwrite.
        emulator
            .ok_in(
                replica_region,
                "CreateSecret",
                json!({"Name": &name, "SecretString": "the other Region's own value"}),
                case,
            )
            .await;

        emulator
            .replicate(&name, &[(replica_region, None)], *force, case)
            .await;
        let entry = emulator.settled_replica(&name, replica_region, case).await;
        let read = emulator
            .ok_in(replica_region, "GetSecretValue", json!({"SecretId": &name}), case)
            .await;
        if *force {
            assert_eq!(
                member(&entry, "Status"),
                Some(REPLICA_STATUS_IN_SYNC.to_string()),
                "case {case:?}: expected the overwrite to replicate, got {entry}"
            );
            assert_eq!(
                member(&read, "SecretString"),
                Some("the primary's value".to_string()),
                "case {case:?}: expected the replica to have overwritten the secret that was \
                 there, got {read}"
            );
        } else {
            assert_eq!(
                member(&entry, "Status"),
                Some(REPLICA_STATUS_FAILED.to_string()),
                "case {case:?}: expected the name collision to fail, got {entry}"
            );
            assert_eq!(
                member(&entry, "StatusMessage"),
                Some(REPLICA_NAME_COLLISION_MESSAGE.to_string()),
                "case {case:?}: expected the collision sentence, got {entry}"
            );
            assert_eq!(
                member(&read, "SecretString"),
                Some("the other Region's own value".to_string()),
                "case {case:?}: a failed replica overwrites nothing, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_primary_names_the_secret_and_every_region_in_the_way() {
    // The rows run over how many Regions the secret still has replicas in: the
    // refusal names the secret and all of them, in the order the secret holds
    // them, which is the order they were first replicated to.
    let rows: &[(&str, &[&str])] = &[
        ("one replica Region", &["us-east-1"]),
        ("two replica Regions", &["us-east-1", "eu-west-1"]),
        (
            "three replica Regions",
            &["eu-central-1", "us-east-1", "ap-northeast-1"],
        ),
    ];
    for (index, (case, regions)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("undeletable/{index}");
        let primary_arn = emulator.create(&name, "s3cret", case).await;
        let requested: Vec<(&str, Option<&str>)> =
            regions.iter().map(|region| (*region, None)).collect();
        emulator.replicate(&name, &requested, false, case).await;

        let (code, message) = emulator
            .err_in(PRIMARY_REGION, "DeleteSecret", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the delete to be refused, got {code} {message:?}"
        );
        let held: Vec<String> = regions.iter().map(|region| (*region).to_string()).collect();
        assert_eq!(
            message,
            primary_is_replicated_message(&primary_arn, &held),
            "case {case:?}: expected the refusal to name the secret and every Region in the way"
        );

        // Removing them all makes it deletable, one Region at a time.
        for (removed, region) in regions.iter().enumerate() {
            emulator
                .ok(
                    "RemoveRegionsFromReplication",
                    json!({"SecretId": &name, "RemoveReplicaRegions": [region]}),
                    case,
                )
                .await;
            if removed + 1 < regions.len() {
                let (_, message) = emulator
                    .err_in(PRIMARY_REGION, "DeleteSecret", json!({"SecretId": &name}), case)
                    .await;
                assert_eq!(
                    message,
                    primary_is_replicated_message(&primary_arn, &held[removed + 1..]),
                    "case {case:?}: expected the refusal to name only the Regions left"
                );
            }
        }
        emulator
            .ok("DeleteSecret", json!({"SecretId": &name}), case)
            .await;
    }
}

#[tokio::test]
async fn replication_is_held_to_the_bound_the_store_puts_on_secrets() {
    // A replica is a secret of the store, so replication grows it and is bounded
    // by the same quota a create is. The rows run over how much room is left and
    // how many Regions the request names, so the bound is pinned at it, one
    // inside it and one past it.
    let rows: &[(&str, usize, &[&str], bool)] = &[
        (
            "one Region with room for it",
            SECRET_QUOTA - 1,
            &["us-east-1"],
            true,
        ),
        (
            "one Region with no room left",
            SECRET_QUOTA,
            &["us-east-1"],
            false,
        ),
        (
            "two Regions with room for one",
            SECRET_QUOTA - 1,
            &["us-east-1", "eu-west-1"],
            false,
        ),
        (
            "two Regions with room for both",
            SECRET_QUOTA - 2,
            &["us-east-1", "eu-west-1"],
            true,
        ),
    ];
    for (index, (case, held, regions, fits)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("bounded/{index}");
        emulator.create(&name, "s3cret", case).await;
        emulator.fill_store_to(*held);

        let requested: Vec<Value> = regions
            .iter()
            .map(|region| json!({"Region": region}))
            .collect();
        let body = json!({"SecretId": &name, "AddReplicaRegions": requested});
        if *fits {
            let replicated = emulator
                .ok("ReplicateSecretToRegions", body, case)
                .await;
            assert_eq!(
                replicas_of(&replicated, case).len(),
                regions.len(),
                "case {case:?}: expected every Region to be replicated to, got {replicated}"
            );
            continue;
        }
        let (code, message) = emulator
            .err_in(PRIMARY_REGION, "ReplicateSecretToRegions", body, case)
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the bound to refuse the request, got {code} {message:?}"
        );
        assert!(
            message.contains(&SECRET_QUOTA.to_string()),
            "case {case:?}: expected the refusal to state the bound, got {message:?}"
        );
        // Nothing was replicated: the request is refused whole.
        let described = emulator
            .ok("DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        assert!(
            described.get("ReplicationStatus").is_none(),
            "case {case:?}: a refused request replicates nothing, got {described}"
        );
    }
}

#[tokio::test]
async fn stop_replication_to_replica_promotes_the_replica_it_is_called_on() {
    // The rows run over the form of the `SecretId` the promotion names the
    // replica by, and the Region it is in.
    let rows: &[(&str, &str, IdForm)] = &[
        ("by name", "us-east-1", IdForm::Name),
        ("by the replica ARN", "eu-west-1", IdForm::FullArn),
        ("by the partial replica ARN", "sa-east-1", IdForm::PartialArn),
    ];
    for (index, (case, replica_region, form)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("promote/{index}");
        let value = format!("promoted-{index}");
        emulator.create(&name, &value, case).await;
        emulator
            .replicate(&name, &[(replica_region, None)], false, case)
            .await;
        emulator.settled_replica(&name, replica_region, case).await;
        let described = emulator
            .ok_in(replica_region, "DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        let replica_arn = text(&described, "ARN", case);

        // Called anywhere but the replica's Region there is no replica to
        // promote: the primary's Region holds a primary, and a Region holding
        // nothing of this secret holds nothing to promote.
        let (code, message) = emulator
            .err_in(
                PRIMARY_REGION,
                "StopReplicationToReplica",
                json!({"SecretId": &name}),
                case,
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the primary to be refused, got {code} {message:?}"
        );
        assert_eq!(
            message, NOT_PERMITTED_ON_REPLICATED_PRIMARY_MESSAGE,
            "case {case:?}: expected the replicated-primary refusal sentence"
        );
        let (code, _) = emulator
            .err_in(
                "ap-south-1",
                "StopReplicationToReplica",
                json!({"SecretId": &name}),
                case,
            )
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected a Region holding nothing of this secret to find nothing"
        );

        let sent = secret_id(*form, &name, &replica_arn);
        let promoted = emulator
            .ok_in(
                replica_region,
                "StopReplicationToReplica",
                json!({"SecretId": sent}),
                case,
            )
            .await;
        assert_eq!(
            text(&promoted, "ARN", case),
            replica_arn,
            "case {case:?}: the promoted secret keeps the ARN the replica had, got {promoted}"
        );

        // The primary no longer replicates to that Region.
        let primary = emulator
            .ok("DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        assert!(
            primary.get("ReplicationStatus").is_none(),
            "case {case:?}: the primary stops replicating to a promoted replica, got {primary}"
        );

        // And the promoted secret is a secret of its own: it tracks nothing, it
        // still holds the value it mirrored, and it is WRITTEN in its Region now.
        let standalone = emulator
            .ok_in(replica_region, "DescribeSecret", json!({"SecretId": &name}), case)
            .await;
        assert!(
            standalone.get("PrimaryRegion").is_none()
                && standalone.get("ReplicationStatus").is_none(),
            "case {case:?}: a promoted secret takes no part in replication, got {standalone}"
        );
        let read = emulator
            .ok_in(replica_region, "GetSecretValue", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            member(&read, "SecretString"),
            Some(value.clone()),
            "case {case:?}: the promoted secret keeps the value it mirrored, got {read}"
        );
        emulator
            .ok_in(
                replica_region,
                "PutSecretValue",
                json!({"SecretId": &name, "SecretString": "written after promotion"}),
                case,
            )
            .await;
        let written = emulator
            .ok_in(replica_region, "GetSecretValue", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            member(&written, "SecretString"),
            Some("written after promotion".to_string()),
            "case {case:?}: a promoted secret is written in its own Region, got {written}"
        );
        // The primary is untouched by the write to the promoted secret, and is
        // now deletable: nothing replicates it any more.
        let primary_read = emulator
            .ok("GetSecretValue", json!({"SecretId": &name}), case)
            .await;
        assert_eq!(
            member(&primary_read, "SecretString"),
            Some(value),
            "case {case:?}: the primary keeps its own value, got {primary_read}"
        );
        emulator
            .ok("DeleteSecret", json!({"SecretId": &name}), case)
            .await;
    }
}

/// The names `ListSecrets` answers in `region` for `filters`, in the order it
/// answers them.
async fn listed_names(emulator: &Emulator, region: &str, filters: Value, case: &str) -> Vec<String> {
    let listed = emulator
        .ok_in(region, "ListSecrets", json!({"Filters": filters}), case)
        .await;
    listed
        .get("SecretList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no SecretList in {listed}"))
        .iter()
        .map(|entry| text(entry, "Name", case))
        .collect()
}

/// A `name` filter scoping a listing to the secrets of one test, beside one more
/// filter: separate filters are ANDed, so the pair selects what both hold for.
fn scoped(prefix: &str, key: &str, value: &str) -> Value {
    json!([
        {"Key": "name", "Values": [prefix]},
        {"Key": key, "Values": [value]},
    ])
}

#[tokio::test]
async fn a_filter_matches_only_against_an_attribute_the_secret_carries() {
    // A filter selects the secrets that CARRY what it names, so a secret this
    // service reports no value of that attribute for matches no value of its key.
    // `PrimaryRegion` is "the Region the secret is in" for a secret that takes
    // part in replication — a replica reports its primary's Region and a
    // replicated primary its own — and a secret with no replication has none, so
    // it matches no `primary-region` value, the Region it happens to live in
    // included.
    //
    // The rows run over the three classes of secret (a replica, the primary it
    // tracks, and two never-replicated secrets), over the Region the listing is
    // made in, over the Region the filter names (each of the two, and a prefix of
    // one), and over a second key the same rule holds for: `owning-service`,
    // which only a secret another service manages carries.
    const AWAY_REGION: &str = "us-east-1";
    const PREFIX: &str = "filtered/";
    let case = "a replicated pair beside two standalone secrets";
    let emulator = Emulator::new();
    let replicated = format!("{PREFIX}replicated");
    let home_standalone = format!("{PREFIX}home-standalone");
    let away_standalone = format!("{PREFIX}away-standalone");
    emulator.create(&replicated, "mirrored", case).await;
    emulator.create(&home_standalone, "home", case).await;
    emulator
        .ok_in(
            AWAY_REGION,
            "CreateSecret",
            json!({"Name": &away_standalone, "SecretString": "away"}),
            case,
        )
        .await;
    emulator
        .replicate(&replicated, &[(AWAY_REGION, None)], false, case)
        .await;
    let settled = emulator
        .settled_replica(&replicated, AWAY_REGION, case)
        .await;
    assert_eq!(
        member(&settled, "Status"),
        Some(REPLICA_STATUS_IN_SYNC.to_string()),
        "case {case:?}: expected the replica to have settled, got {settled}"
    );

    // The secret with no replication reports no `PrimaryRegion` at all, which is
    // the value the filter is matched against.
    let described = emulator
        .ok_in(
            AWAY_REGION,
            "DescribeSecret",
            json!({"SecretId": &away_standalone}),
            case,
        )
        .await;
    assert!(
        described.get("PrimaryRegion").is_none(),
        "case {case:?}: a never-replicated secret reports no PrimaryRegion, got {described}"
    );

    let rows: Vec<(String, &str, &str, &str, Vec<String>)> = vec![
        (
            "primary-region = the home Region, listed away".to_string(),
            AWAY_REGION,
            "primary-region",
            PRIMARY_REGION,
            vec![replicated.clone()],
        ),
        (
            "primary-region = the away Region, listed away".to_string(),
            AWAY_REGION,
            "primary-region",
            AWAY_REGION,
            Vec::new(),
        ),
        (
            "primary-region = the home Region, listed at home".to_string(),
            PRIMARY_REGION,
            "primary-region",
            PRIMARY_REGION,
            vec![replicated.clone()],
        ),
        (
            "primary-region = the away Region, listed at home".to_string(),
            PRIMARY_REGION,
            "primary-region",
            AWAY_REGION,
            Vec::new(),
        ),
        (
            "primary-region as a prefix of the home Region, listed away".to_string(),
            AWAY_REGION,
            "primary-region",
            "us-w",
            vec![replicated.clone()],
        ),
        (
            "owning-service, which no secret here carries".to_string(),
            PRIMARY_REGION,
            "owning-service",
            "rds",
            Vec::new(),
        ),
    ];
    for (row, region, key, value, expected) in rows {
        let listed = listed_names(&emulator, region, scoped(PREFIX, key, value), &row).await;
        assert_eq!(
            listed, expected,
            "case {row:?}: expected {expected:?} in {region}, got {listed:?}"
        );
    }

    // Promoting the replica ends the replication, so neither secret carries a
    // `PrimaryRegion` any more and neither matches any value of the filter.
    let promoted = "the promoted secret and the primary it left";
    emulator
        .ok_in(
            AWAY_REGION,
            "StopReplicationToReplica",
            json!({"SecretId": &replicated}),
            promoted,
        )
        .await;
    for (region, value) in [
        (AWAY_REGION, PRIMARY_REGION),
        (AWAY_REGION, AWAY_REGION),
        (PRIMARY_REGION, PRIMARY_REGION),
    ] {
        let row = format!("after the promotion: primary-region = {value}, listed in {region}");
        let listed =
            listed_names(&emulator, region, scoped(PREFIX, "primary-region", value), &row).await;
        assert!(
            listed.is_empty(),
            "case {row:?}: expected no secret to match, got {listed:?}"
        );
    }
    // The secrets themselves are still there: it is the ATTRIBUTE that is gone.
    let still_held = listed_names(
        &emulator,
        AWAY_REGION,
        json!([{"Key": "name", "Values": [PREFIX]}]),
        promoted,
    )
    .await;
    assert_eq!(
        still_held,
        vec![away_standalone.clone(), replicated.clone()],
        "case {promoted:?}: expected both secrets of the away Region to be listed, got \
         {still_held:?}"
    );
}

#[tokio::test]
async fn the_promotion_refusal_states_which_class_of_secret_it_was_called_on() {
    // `StopReplicationToReplica` is the replica-side promotion, and the refusal it
    // answers where the secret is NOT a replica says which class of secret that
    // is: a primary that still HAS a replica is sent to the replica's Region, and
    // a secret with no replica at all is told its class has no promotion to
    // perform — there is no Region to send it to. The ladder below walks one
    // secret through every state it reaches: standalone, replicated primary
    // (before and after the clock settles the replica), the replica itself, and
    // the two standalone secrets the promotion leaves behind.
    const AWAY_REGION: &str = "us-east-1";
    let case = "one secret through every replication state";
    let emulator = Emulator::new();
    let name = "promotion/ladder";
    emulator.create(name, "stored", case).await;

    let standalone = format!("{case}: never replicated");
    let (code, message) = emulator
        .err_in(
            PRIMARY_REGION,
            "StopReplicationToReplica",
            json!({"SecretId": name}),
            &standalone,
        )
        .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            NOT_PERMITTED_ON_STANDALONE_MESSAGE
        ),
        "case {standalone:?}: expected the standalone refusal, got {code} {message:?}"
    );

    emulator
        .replicate(name, &[(AWAY_REGION, None)], false, case)
        .await;
    for state in ["a replica still InProgress", "a replica settled InSync"] {
        if state.ends_with("InSync") {
            let settled = emulator.settled_replica(name, AWAY_REGION, case).await;
            assert_eq!(
                member(&settled, "Status"),
                Some(REPLICA_STATUS_IN_SYNC.to_string()),
                "case {state:?}: expected the replica to have settled, got {settled}"
            );
        }
        let row = format!("{case}: {state}");
        let (code, message) = emulator
            .err_in(
                PRIMARY_REGION,
                "StopReplicationToReplica",
                json!({"SecretId": name}),
                &row,
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                NOT_PERMITTED_ON_REPLICATED_PRIMARY_MESSAGE
            ),
            "case {row:?}: expected the replicated-primary refusal, got {code} {message:?}"
        );
    }

    // The call made where the rule sends it IS served, and both secrets it leaves
    // are standalone: each answers the standalone sentence from then on.
    emulator
        .ok_in(
            AWAY_REGION,
            "StopReplicationToReplica",
            json!({"SecretId": name}),
            case,
        )
        .await;
    for (region, state) in [
        (AWAY_REGION, "the promoted secret"),
        (PRIMARY_REGION, "the primary its replica left"),
    ] {
        let row = format!("{case}: {state}");
        let (code, message) = emulator
            .err_in(
                region,
                "StopReplicationToReplica",
                json!({"SecretId": name}),
                &row,
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                NOT_PERMITTED_ON_STANDALONE_MESSAGE
            ),
            "case {row:?}: expected the standalone refusal, got {code} {message:?}"
        );
    }
}

// --- cross-ACCOUNT replication -----------------------------------------------

/// The one `CrossAccountReplicas` entry a read of a primary reports, or `None`
/// while it reports none.
fn cross_account_entry(described: &Value) -> Option<Value> {
    described
        .get("CrossAccountReplicas")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
}

/// The `CrossAccountPrimary` a read of a replica reports, or `None` while it
/// reports none.
fn cross_account_primary(described: &Value) -> Option<Value> {
    described.get("CrossAccountPrimary").cloned()
}

/// Create a secret holding `value` in the keyed harness's Region, and answer its
/// ARN.
async fn create_in(emulator: &KeyedEmulator, name: &str, value: &str, case: &str) -> String {
    let created = emulator
        .sm_ok(
            "CreateSecret",
            &json!({"Name": name, "SecretString": value}),
            case,
        )
        .await;
    text(&created, "ARN", case)
}

/// Create a cross-account replica of `primary_arn` under a customer managed key
/// KMS mints for this call, and answer its ARN.
async fn create_cross_account_replica(
    emulator: &KeyedEmulator,
    name: &str,
    primary_arn: &str,
    case: &str,
) -> String {
    let (key_arn, _) = emulator.create_key(case).await;
    let created = emulator
        .sm_ok(
            "CreateSecret",
            &json!({
                "Name": name,
                "KmsKeyId": key_arn,
                "CrossAccountPrimaryARN": primary_arn,
            }),
            case,
        )
        .await;
    text(&created, "ARN", case)
}

/// The primary's one cross-account entry once its link has settled.
///
/// Looks for the status the store OWES that link until it has been reported,
/// rather than waiting a span out: the `InProgress` -> `InSync` move is due at an
/// instant the store recorded, and a request is what materialises it.
async fn settled_cross_account_entry(
    emulator: &KeyedEmulator,
    primary_id: &str,
    case: &str,
) -> Value {
    let deadline = Instant::now() + REPLICA_WAIT;
    loop {
        let described = emulator
            .sm_ok("DescribeSecret", &json!({"SecretId": primary_id}), case)
            .await;
        let entry = cross_account_entry(&described)
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

/// Look for `GetSecretValue` on `secret_id` to answer `expected`, which is what
/// replication into a cross-account replica carries.
async fn wait_for_replicated_value(
    emulator: &KeyedEmulator,
    secret_id: &str,
    expected: &str,
    case: &str,
) {
    let deadline = Instant::now() + REPLICA_WAIT;
    loop {
        let (status, parsed) = emulator
            .sm("GetSecretValue", &json!({"SecretId": secret_id}))
            .await;
        if status == 200 && member(&parsed, "SecretString").as_deref() == Some(expected) {
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

#[tokio::test]
async fn an_associated_cross_account_replica_serves_what_its_primary_holds() {
    // The rows run over the shape and size of the value the primary holds, the form
    // of the `SecretId` the association names the primary by, and whether the
    // association states a `ReplicationRoleARN`: the rule is about the link
    // carrying the primary's value, not about any of those.
    let role = format!(
        "arn:aws:iam::{}:role/cross-account-replication",
        engine::EMULATOR_ACCOUNT_ID
    );
    let rows: Vec<(&str, String, String, IdForm, Option<String>)> = vec![
        (
            "a short value, named by name, no replication role",
            "s3cr3t".to_string(),
            "s3cr3t-rotated".to_string(),
            IdForm::Name,
            None,
        ),
        (
            "a JSON document, named by full ARN, with a replication role",
            json!({"username": "admin", "password": "p@ssw0rd"}).to_string(),
            json!({"username": "admin", "password": "r0tated"}).to_string(),
            IdForm::FullArn,
            Some(role.clone()),
        ),
        (
            "a long value, named by partial ARN, with a replication role",
            "x".repeat(1_000),
            "y".repeat(999),
            IdForm::PartialArn,
            Some(role),
        ),
    ];
    for (index, (case, value, updated, form, role_arn)) in rows.iter().enumerate() {
        let emulator = KeyedEmulator::new();
        let primary_name = format!("cross-account/primary/{index}");
        let replica_name = format!("cross-account/replica/{index}");
        let primary_arn = create_in(&emulator, &primary_name, value, case).await;
        let replica_arn =
            create_cross_account_replica(&emulator, &replica_name, &primary_arn, case).await;

        // Before the association nothing replicates: the replica names its primary
        // and reports `Pending`, "which indicates that replication has not yet
        // started" (`StatusType`, this service's Smithy model).
        let described = emulator
            .sm_ok("DescribeSecret", &json!({"SecretId": &replica_name}), case)
            .await;
        let link = cross_account_primary(&described)
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
        let described = emulator
            .sm_ok("DescribeSecret", &json!({"SecretId": &primary_name}), case)
            .await;
        assert!(
            cross_account_entry(&described).is_none() && cross_account_primary(&described).is_none(),
            "case {case:?}: expected an unassociated primary to report no cross-account link, \
             got {described}"
        );

        let mut request = json!({
            "SecretId": secret_id(*form, &primary_name, &primary_arn),
            "CrossAccountReplicaARN": replica_arn.clone(),
        });
        if let Some(role) = role_arn {
            request
                .as_object_mut()
                .expect("the request body is an object")
                .insert("ReplicationRoleARN".to_string(), json!(role));
        }
        let associated = emulator
            .sm_ok("AssociateCrossAccountReplica", &request, case)
            .await;
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

        // The clock owes the link its `InSync` move, and the entry the primary
        // reports carries the role the association stated and nothing it did not.
        let entry = settled_cross_account_entry(&emulator, &primary_name, case).await;
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
                role_arn.clone()
            ),
            "case {case:?}: expected the settled entry for the replica it replicates to, got \
             {entry}"
        );

        // What replication carries is the VALUE: the replica serves what the primary
        // holds, read through the replica's own name, and goes on carrying every
        // later write — until the link is removed, after which the replica keeps
        // what it has and the primary's writes are its own.
        wait_for_replicated_value(&emulator, &replica_name, value, case).await;
        emulator
            .sm_ok(
                "PutSecretValue",
                &json!({"SecretId": &primary_name, "SecretString": updated}),
                case,
            )
            .await;
        wait_for_replicated_value(&emulator, &replica_name, updated, case).await;

        // While the replica is associated the primary cannot be deleted: "You must
        // disassociate a cross-account replica before you call DeleteSecret for the
        // primary secret."
        let (code, message) = emulator
            .sm_err("DeleteSecret", &json!({"SecretId": &primary_name}), case)
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (
                "InvalidParameterException",
                primary_has_cross_account_replica_message(&primary_arn, &replica_arn).as_str()
            ),
            "case {case:?}: expected the associated-replica refusal, got {code} {message:?}"
        );

        let disassociated = emulator
            .sm_ok(
                "DisassociateCrossAccountReplica",
                &json!({
                    "SecretId": secret_id(*form, &primary_name, &primary_arn),
                    "CrossAccountReplicaARN": replica_arn.clone(),
                }),
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
        let described = emulator
            .sm_ok("DescribeSecret", &json!({"SecretId": &replica_name}), case)
            .await;
        let link = cross_account_primary(&described)
            .unwrap_or_else(|| panic!("case {case:?}: no CrossAccountPrimary in {described}"));
        assert_eq!(
            member(&link, "ReplicationStatus"),
            Some(CROSS_ACCOUNT_STATUS_DETACHED.to_string()),
            "case {case:?}: expected the detached replica to report Detached, got {described}"
        );
        emulator
            .sm_ok(
                "PutSecretValue",
                &json!({"SecretId": &primary_name, "SecretString": "after-the-disassociation"}),
                case,
            )
            .await;
        let read = emulator
            .sm_ok("GetSecretValue", &json!({"SecretId": &replica_name}), case)
            .await;
        assert_eq!(
            member(&read, "SecretString").as_deref(),
            Some(updated.as_str()),
            "case {case:?}: expected a detached replica to keep what it received and take no \
             update, got {read}"
        );
        // And the primary, replicating to nothing, is deletable.
        emulator
            .sm_ok("DeleteSecret", &json!({"SecretId": &primary_name}), case)
            .await;
    }
}

#[tokio::test]
async fn a_cross_account_link_is_refused_for_every_pair_and_member_the_rule_excludes() {
    let case = "the cross-account links this service does not make";
    let emulator = KeyedEmulator::new();
    let primary_arn = create_in(&emulator, "primary", "s3cr3t", case).await;
    let standalone_arn = create_in(&emulator, "standalone", "stored", case).await;
    let replica_arn = create_cross_account_replica(&emulator, "replica", &primary_arn, case).await;
    let (key_arn, _) = emulator.create_key(case).await;

    // A create that names a primary to receive from carries nothing the primary
    // supplies instead, and has to name a customer managed key: "You must specify a
    // customer managed KMS key in KmsKeyId. You can't include SecretString,
    // SecretBinary, Description, AddReplicaRegions, or Type."
    // (`CreateSecretRequest$CrossAccountPrimaryARN`, this service's Smithy model.)
    let creates: Vec<(&str, Value, &str)> = vec![
        (
            "a value of its own",
            json!({"KmsKeyId": key_arn.clone(), "SecretString": "its-own"}),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "a description of its own",
            json!({"KmsKeyId": key_arn.clone(), "Description": "its own"}),
            CROSS_ACCOUNT_REPLICA_MEMBERS_MESSAGE,
        ),
        (
            "replica Regions of its own",
            json!({
                "KmsKeyId": key_arn.clone(),
                "AddReplicaRegions": [{"Region": "us-east-1"}],
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
    ];
    for (index, (row, members, expected)) in creates.iter().enumerate() {
        let mut body = members.clone();
        let object = body.as_object_mut().expect("every row's body is an object");
        object.insert("Name".to_string(), json!(format!("refused/{index}")));
        object.insert(
            "CrossAccountPrimaryARN".to_string(),
            json!(primary_arn.clone()),
        );
        let (code, message) = emulator.sm_err("CreateSecret", &body, row).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("InvalidParameterException", *expected),
            "case {row:?}: expected the parameter refusal {expected:?}, got {code} {message:?}"
        );
        let (status, _) = emulator
            .sm(
                "DescribeSecret",
                &json!({"SecretId": format!("refused/{index}")}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {row:?}: a refused create stores nothing, so the name holds no secret"
        );
    }

    // And the pairs no association links: a secret that is no replica of this
    // primary, a replica outside its Region, the call made on the replica, and a
    // second replica of one primary.
    let other_region_arn = {
        let mut fields: Vec<&str> = replica_arn.split(':').collect();
        fields[3] = "us-east-1";
        fields.join(":")
    };
    let associations: Vec<(&str, Value, &str, String)> = vec![
        (
            "a secret that is no cross-account replica",
            json!({"SecretId": "primary", "CrossAccountReplicaARN": standalone_arn.clone()}),
            "InvalidRequestException",
            format!("The secret {standalone_arn} is not a cross-account replica of the secret "),
        ),
        (
            "a replica outside the primary's Region",
            json!({"SecretId": "primary", "CrossAccountReplicaARN": other_region_arn}),
            "InvalidParameterException",
            CROSS_ACCOUNT_REGION_MISMATCH_MESSAGE.to_string(),
        ),
        (
            "the association made on the replica rather than the primary",
            json!({"SecretId": "replica", "CrossAccountReplicaARN": replica_arn.clone()}),
            "InvalidParameterException",
            NOT_PERMITTED_ON_CROSS_ACCOUNT_REPLICA_MESSAGE.to_string(),
        ),
    ];
    for (row, request, expected_code, expected_message) in associations {
        let (code, message) = emulator
            .sm_err("AssociateCrossAccountReplica", &request, row)
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
    let described = emulator
        .sm_ok("DescribeSecret", &json!({"SecretId": "primary"}), case)
        .await;
    assert!(
        cross_account_entry(&described).is_none(),
        "case {case:?}: a refused association links nothing, got {described}"
    );

    emulator
        .sm_ok(
            "AssociateCrossAccountReplica",
            &json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn.clone()}),
            case,
        )
        .await;
    let second_arn =
        create_cross_account_replica(&emulator, "second-replica", &primary_arn, case).await;
    let (code, message) = emulator
        .sm_err(
            "AssociateCrossAccountReplica",
            &json!({"SecretId": "primary", "CrossAccountReplicaARN": second_arn}),
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
    // The association of the replica already linked is idempotent: it starts
    // replication over rather than being refused.
    let repaired = emulator
        .sm_ok(
            "AssociateCrossAccountReplica",
            &json!({"SecretId": "primary", "CrossAccountReplicaARN": replica_arn}),
            case,
        )
        .await;
    assert_eq!(
        member(&repaired, "ReplicationStatus"),
        Some(REPLICA_STATUS_IN_PROGRESS.to_string()),
        "case {case:?}: expected a repeat for the same replica to start replication over, got \
         {repaired}"
    );
}

#[tokio::test]
async fn the_listed_resource_policy_names_follow_the_document_a_secret_carries() {
    // `ListResourcePolicies` "lists the resource-based policies attached to a
    // secret. The response contains only the names of the policies" (this service's
    // Smithy model), and a secret carries the one `default` policy — so the listing
    // is that name exactly while a document is attached and empty while none is.
    // The rows run over the form of the `SecretId` and the page size asked for.
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)},
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*",
        }],
    })
    .to_string();
    for form in [IdForm::Name, IdForm::FullArn, IdForm::PartialArn] {
        for page in [json!({}), json!({"MaxResults": 1}), json!({"MaxResults": 100})] {
            let case = format!("listed by {form:?}, page {page}");
            let emulator = KeyedEmulator::new();
            let name = "listed-policies";
            let arn = create_in(&emulator, name, "s3cr3t", &case).await;
            let id = secret_id(form, name, &arn);
            let listing = || {
                let mut body = page.clone();
                body.as_object_mut()
                    .expect("a request body is an object")
                    .insert("SecretId".to_string(), json!(id.clone()));
                body
            };

            let listed = emulator
                .sm_ok("ListResourcePolicies", &listing(), &case)
                .await;
            assert_eq!(
                policy_names(&listed, &case),
                Vec::<String>::new(),
                "case {case:?}: expected no names for a secret carrying no policy, got {listed}"
            );
            assert!(
                listed.get("NextToken").is_none(),
                "case {case:?}: expected no cursor where there is no further page, got {listed}"
            );

            emulator
                .sm_ok(
                    "PutResourcePolicy",
                    &json!({"SecretId": id.clone(), "ResourcePolicy": document.clone()}),
                    &case,
                )
                .await;
            let listed = emulator
                .sm_ok("ListResourcePolicies", &listing(), &case)
                .await;
            assert_eq!(
                policy_names(&listed, &case),
                vec!["default".to_string()],
                "case {case:?}: expected the attached policy's name, got {listed}"
            );
            assert!(
                listed.get("NextToken").is_none(),
                "case {case:?}: the whole listing fits one page, so no cursor is reported, got \
                 {listed}"
            );

            emulator
                .sm_ok(
                    "DeleteResourcePolicy",
                    &json!({"SecretId": id.clone()}),
                    &case,
                )
                .await;
            let listed = emulator
                .sm_ok("ListResourcePolicies", &listing(), &case)
                .await;
            assert_eq!(
                policy_names(&listed, &case),
                Vec::<String>::new(),
                "case {case:?}: expected the detached name to be gone, got {listed}"
            );
        }
    }
}

/// The `PolicyName` values a `ListResourcePolicies` answer reports, in order.
fn policy_names(listed: &Value, case: &str) -> Vec<String> {
    listed
        .get("ResourcePolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no ResourcePolicies in {listed}"))
        .iter()
        .map(|entry| text(entry, "PolicyName", case))
        .collect()
}
