// SPDX-License-Identifier: CC0-1.0

//! The secret value's ENVELOPE: every read of a secret under a customer managed
//! KMS key decrypts through that key, and every write encrypts through it, so the
//! key's own state decides both.
//!
//! The rule these tables assert: a secret naming a customer managed key holds its
//! versions sealed under data keys that key wrapped, so a key KMS will not perform
//! a cryptographic operation with refuses the read with `DecryptionFailure` and the
//! write with `EncryptionFailure` — each relaying the KMS refusal it met — and the
//! write stores nothing; a CREATE naming such a key is refused with
//! `PreconditionNotMetException`, since the key it names is a prerequisite of
//! creating the secret at all; and a secret left on the Amazon Web Services managed
//! key names no key a caller can act on, so nothing stands between its reads and its
//! value.
//!
//! Pointing a secret at another key moves the versions the service's own staging
//! labels reach to that key, re-encrypting each under it, so the key they came from
//! is no longer needed to read them while a deprecated version keeps the key its own
//! data key was wrapped by.
//!
//! The rows span what the rule generalises over rather than one recorded request:
//! the FORM the secret names its key in (key ARN, bare key id, alias name, alias
//! ARN), the member and size of the value (a short string, a large JSON document, a
//! binary blob), the KEY STATE that refuses the call (disabled, pending deletion),
//! and the OPERATION that reads or writes the value (`GetSecretValue` by label and
//! by version id, `BatchGetSecretValue`, `PutSecretValue`, `UpdateSecret`,
//! `CreateSecret`).
//!
//! KMS is mounted beside Secrets Manager on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, so every key below is a real key minted
//! through KMS's own operations and every state change is made through them too.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::state::ComAmazonawsSecretsmanagerSecretsmanagerState;
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::secret_lifecycle::ValueCase;

/// The region every request in this suite signs for: a KMS key is regional, and
/// Secrets Manager calls the key in the region the request was addressed to, so the
/// keys and the secrets below live in the same one.
const REGION: &str = "us-west-2";

/// How a read refusal introduces the KMS refusal it relays: Secrets Manager states
/// what it could not do and then relays the wrapped failure, which is how the
/// refusal of a secret under an unusable key is worded on the wire.
pub(crate) const DECRYPTION_SENTENCE: &str = "Secrets Manager can't decrypt the secret value:";

/// The mirror of [`DECRYPTION_SENTENCE`] for a write refusal.
const ENCRYPTION_SENTENCE: &str = "Secrets Manager can't encrypt the secret value:";

/// What the relayed clause states about the wrapped KMS refusal beside its message:
/// the service that answered and the status its class is answered with, before the
/// code it was stated under.
const RELAY_FIELDS: &str = "(Service: AWSKMS; Status Code: 400; Error Code: ";

/// How the relayed clause ends: the identifier of the answer it relays, then the
/// proxy field, which no in-process answer came through.
const RELAY_TAIL: &str = "; Proxy: null)";

/// Secrets Manager and KMS over one shared engine, behind the same
/// `MultiServiceRouter` the production router builds.
///
/// `Clone` shares that one router and that one engine, so a concurrency case can
/// hand a copy to each task and every request still reaches the same store.
#[derive(Clone)]
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::build(true)
    }

    /// An emulator assembled WITHOUT KMS, which is the one deployment where a value
    /// under a customer managed key is held as sent: nothing answers the seal call,
    /// so nothing is said about the key.
    fn without_kms() -> Self {
        Self::build(false)
    }

    fn build(with_kms: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if with_kms {
            router.register_service(
                service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
            );
        }
        Self { router, engine }
    }

    /// POST one `awsJson1_1` operation of `service`, signed for [`REGION`], and
    /// answer its status and parsed body.
    async fn call(
        &self,
        service: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{service}.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
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

    /// One Secrets Manager operation.
    pub(crate) async fn sm(&self, operation: &str, body: &Value) -> (u16, Value) {
        // A write that stores a value creates a version the caller has to name;
        // see `secret_client_request_token::with_staked_version`.
        let body = crate::secret_client_request_token::with_staked_version(operation, body);
        self.call("secretsmanager", "secretsmanager", operation, &body)
            .await
    }

    /// One Secrets Manager operation that must succeed.
    pub(crate) async fn sm_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.sm(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
        );
        parsed
    }

    /// One Secrets Manager operation that must be refused, answered as
    /// `(error code, message)`.
    pub(crate) async fn sm_err(
        &self,
        operation: &str,
        body: &Value,
        case: &str,
    ) -> (String, String) {
        let (status, parsed) = self.sm(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 client error from {operation}, got {status} with \
             body {parsed}"
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

    /// One KMS operation that must succeed.
    pub(crate) async fn kms_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self
            .call("kms", "TrentService", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from KMS {operation}, got {status} with body {parsed}"
        );
        parsed
    }

    /// Mint a symmetric KMS key in [`REGION`] and answer it as `(key ARN, key id)`.
    pub(crate) async fn create_key(&self, case: &str) -> (String, String) {
        let created = self.kms_ok("CreateKey", &json!({}), case).await;
        let metadata = &created["KeyMetadata"];
        let arn = metadata["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateKey reported no Arn in {created}"))
            .to_string();
        let key_id = metadata["KeyId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateKey reported no KeyId in {created}"))
            .to_string();
        (arn, key_id)
    }

    /// Whether the version `AWSCURRENT` names of the secret `name` is held SEALED —
    /// read straight out of the shared store, because the one thing a response
    /// cannot show is that the stored value is not the plaintext.
    fn current_version_is_sealed(&self, name: &str, case: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSecretsmanagerSecretsmanagerState>();
        let store = state.lock();
        let secret = store
            .get(REGION, name)
            .unwrap_or_else(|| panic!("case {case:?}: the store holds no secret {name:?}"));
        secret
            .version_with_stage("AWSCURRENT")
            .unwrap_or_else(|| panic!("case {case:?}: {name:?} has no AWSCURRENT version"))
            .value
            .is_sealed()
    }
}

/// How many versions a secret holds, deprecated ones included — what a refused
/// write must leave unchanged.
pub(crate) async fn version_count(emulator: &Emulator, secret_id: &str, case: &str) -> usize {
    let listed = emulator
        .sm_ok(
            "ListSecretVersionIds",
            &json!({"SecretId": secret_id, "IncludeDeprecated": true}),
            case,
        )
        .await;
    listed["Versions"]
        .as_array()
        .map_or(0, |versions| versions.len())
}

/// A `CreateSecret` body naming `name`, the key `kms_key_id` and this value.
pub(crate) fn create_body(name: &str, kms_key_id: Option<&str>, value: &ValueCase) -> Value {
    let mut body = Map::new();
    body.insert("Name".to_string(), json!(name));
    if let Some(kms_key_id) = kms_key_id {
        body.insert("KmsKeyId".to_string(), json!(kms_key_id));
    }
    if let Value::Object(members) = value.wire_members() {
        body.extend(members);
    }
    Value::Object(body)
}

/// Assert a refusal is worded as this error class is: Secrets Manager's own clause
/// for what it could not do, then the wrapped KMS refusal — the message KMS stated,
/// the code it stated it under, and the fields that name the answer it came from.
pub(crate) fn assert_relays_kms(
    message: &str,
    sentence: &str,
    kms_code: &str,
    kms_phrase: &str,
    case: &str,
) {
    assert!(
        message.starts_with(sentence),
        "case {case:?}: expected the message to start {sentence:?}, got {message:?}"
    );
    let clause = format!("{RELAY_FIELDS}{kms_code}; Request ID: ");
    assert!(
        message.contains(&clause),
        "case {case:?}: expected the relayed clause {clause:?} in the message, got {message:?}"
    );
    assert!(
        message.contains(kms_phrase),
        "case {case:?}: expected the wrapped KMS refusal {kms_phrase:?} in the message, got \
         {message:?}"
    );
    // The clause ends with the identifier of the answer it relays, which is one
    // answer's own value and so is checked by its FORM: a non-empty identifier, and
    // nothing of the clause's own punctuation inside it.
    let request_id = message
        .split_once(&clause)
        .and_then(|(_, tail)| tail.strip_suffix(RELAY_TAIL))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the message to end {RELAY_TAIL:?}, got {message:?}")
        });
    assert!(
        !request_id.is_empty() && !request_id.contains(';') && !request_id.contains(' '),
        "case {case:?}: expected the relayed answer's identifier, got {request_id:?} in \
         {message:?}"
    );
}

/// The value table: the member and the size of what a caller stores, which the
/// envelope is the same for.
fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase::string("a short string", "s3cr3t"),
        ValueCase::string(
            "a JSON document",
            json!({"username": "admin", "password": "p@ssw0rd", "host": "db.internal"}).to_string(),
        ),
        // Larger than one AES block and not a multiple of it, so the ciphertext is
        // not a single block however the AEAD pads.
        ValueCase::string("a long string", "x".repeat(1_000)),
        ValueCase::binary("a binary blob", (0u8..=255).cycle().take(300).collect()),
        ValueCase::binary("a one-byte blob", vec![0x00]),
    ]
}

#[tokio::test]
async fn a_secret_under_a_customer_managed_key_is_sealed_under_it_and_reads_back() {
    let emulator = Emulator::new();
    // Every form `KmsKeyId` accepts: "the ARN, key ID, or alias of the KMS key"
    // (`CreateSecretRequest$KmsKeyId`, this service's Smithy model), and the alias
    // in both its spellings.
    for (form, by_arn, by_alias) in [
        ("a key ARN", true, false),
        ("a bare key id", false, false),
        ("an alias name", false, true),
        ("an alias ARN", false, true),
    ] {
        for (index, value) in value_cases().iter().enumerate() {
            let case = format!("{form}, {}", value.label);
            let (key_arn, key_id) = emulator.create_key(&case).await;
            let alias = format!("alias/envelope-{form}-{index}").replace(' ', "-");
            emulator
                .kms_ok(
                    "CreateAlias",
                    &json!({"AliasName": alias, "TargetKeyId": key_arn}),
                    &case,
                )
                .await;
            let named = match (by_arn, by_alias, form) {
                (true, _, _) => key_arn.clone(),
                (_, true, "an alias ARN") => {
                    format!("arn:aws:kms:{REGION}:{}:{alias}", engine::EMULATOR_ACCOUNT_ID)
                }
                (_, true, _) => alias.clone(),
                _ => key_id.clone(),
            };
            let name = format!("envelope-{}-{index}", form.replace(' ', "-"));

            emulator
                .sm_ok(
                    "CreateSecret",
                    &create_body(&name, Some(&named), value),
                    &case,
                )
                .await;

            // The stored value is not the plaintext: a customer managed key sealed
            // it, which is what makes a read depend on that key at all.
            assert!(
                emulator.current_version_is_sealed(&name, &case),
                "case {case:?}: expected the stored version to be sealed under the \
                 customer managed key, got a value held as sent"
            );

            let read = emulator
                .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
                .await;
            value.assert_round_trip(&read, &case);

            // The batched read opens the same envelope, so it answers the same value.
            let batch = emulator
                .sm_ok(
                    "BatchGetSecretValue",
                    &json!({"SecretIdList": [name]}),
                    &case,
                )
                .await;
            let entries = batch["SecretValues"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no SecretValues in {batch}"));
            assert_eq!(
                entries.len(),
                1,
                "case {case:?}: expected one batched entry, got {batch}"
            );
            value.assert_round_trip(&entries[0], &case);

            // And a new version goes through the key as well, so it reads back too.
            let put = ValueCase::string("a replacement", "rotated-value");
            let mut put_body = Map::new();
            put_body.insert("SecretId".to_string(), json!(name));
            if let Value::Object(members) = put.wire_members() {
                put_body.extend(members);
            }
            emulator
                .sm_ok("PutSecretValue", &Value::Object(put_body), &case)
                .await;
            let reread = emulator
                .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
                .await;
            put.assert_round_trip(&reread, &case);
        }
    }
}

#[tokio::test]
async fn reads_and_writes_are_refused_while_the_secret_key_cannot_be_used() {
    // The key states a cryptographic operation is refused for, each made through
    // KMS's own operation, with the refusal KMS answers under: a disabled key is
    // `DisabledException`, and any other unusable state is
    // `KMSInvalidStateException` naming the state the key is in.
    let unusable = [
        (
            "the key is disabled",
            "DisableKey",
            json!({}),
            "DisabledException",
            "is disabled.",
        ),
        (
            "the key is pending deletion",
            "ScheduleKeyDeletion",
            json!({"PendingWindowInDays": 7}),
            "KMSInvalidStateException",
            "is pending deletion.",
        ),
    ];

    for (label, kms_operation, extra, kms_code, kms_phrase) in unusable {
        for (index, value) in value_cases().iter().enumerate() {
            let emulator = Emulator::new();
            let case = format!("{label}, {}", value.label);
            let (key_arn, _) = emulator.create_key(&case).await;
            let name = format!("unusable-{index}");
            let created = emulator
                .sm_ok(
                    "CreateSecret",
                    &create_body(&name, Some(&key_arn), value),
                    &case,
                )
                .await;
            let version_id = created["VersionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no VersionId in {created}"))
                .to_string();
            let before = version_count(&emulator, &name, &case).await;

            // The secret is readable and writable while the key is usable, so the
            // refusals below are the key's state and nothing else.
            emulator
                .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
                .await;

            let mut request = Map::new();
            request.insert("KeyId".to_string(), json!(key_arn));
            if let Value::Object(members) = extra.clone() {
                request.extend(members);
            }
            emulator
                .kms_ok(kms_operation, &Value::Object(request), &case)
                .await;

            // Every way of addressing the stored value is refused, and every one of
            // them is refused with Secrets Manager's OWN code for a value it could
            // not decrypt.
            for (addressed, body) in [
                ("by AWSCURRENT", json!({"SecretId": name})),
                (
                    "by version id",
                    json!({"SecretId": name, "VersionId": version_id}),
                ),
                (
                    "by stage",
                    json!({"SecretId": name, "VersionStage": "AWSCURRENT"}),
                ),
            ] {
                let case = format!("{case}, read {addressed}");
                let (code, message) = emulator.sm_err("GetSecretValue", &body, &case).await;
                assert_eq!(
                    code, "DecryptionFailure",
                    "case {case:?}: expected DecryptionFailure, got {code} with {message:?}"
                );
                assert_relays_kms(&message, DECRYPTION_SENTENCE, kms_code, kms_phrase, &case);
                // KMS names the key its state refused the call for, by ARN, and
                // that is the wording the relay carries.
                assert!(
                    message.contains(&key_arn),
                    "case {case:?}: expected the relayed refusal to name the key {key_arn}, \
                     got {message:?}"
                );
            }

            // The batched read fails that secret alone, inside a 200: "you can see
            // the errors in Errors in the response".
            let batched_case = format!("{case}, batched read");
            let batch = emulator
                .sm_ok(
                    "BatchGetSecretValue",
                    &json!({"SecretIdList": [name]}),
                    &batched_case,
                )
                .await;
            assert_eq!(
                batch["SecretValues"].as_array().map(Vec::len),
                Some(0),
                "case {batched_case:?}: expected no batched value, got {batch}"
            );
            let errors = batch["Errors"]
                .as_array()
                .unwrap_or_else(|| panic!("case {batched_case:?}: no Errors in {batch}"));
            assert_eq!(
                errors.len(),
                1,
                "case {batched_case:?}: expected one batched error, got {batch}"
            );
            assert_eq!(
                errors[0]["ErrorCode"].as_str(),
                Some("DecryptionFailure"),
                "case {batched_case:?}: expected a DecryptionFailure entry, got {batch}"
            );
            assert_relays_kms(
                errors[0]["Message"].as_str().unwrap_or_default(),
                DECRYPTION_SENTENCE,
                kms_code,
                kms_phrase,
                &batched_case,
            );

            // Every write of a VALUE is refused with the mirror code, and stores
            // nothing.
            let replacement = ValueCase::string("a replacement", "written-under-a-dead-key");
            for operation in ["PutSecretValue", "UpdateSecret"] {
                let case = format!("{case}, {operation}");
                let mut body = Map::new();
                body.insert("SecretId".to_string(), json!(name));
                if let Value::Object(members) = replacement.wire_members() {
                    body.extend(members);
                }
                let (code, message) = emulator
                    .sm_err(operation, &Value::Object(body), &case)
                    .await;
                assert_eq!(
                    code, "EncryptionFailure",
                    "case {case:?}: expected EncryptionFailure, got {code} with {message:?}"
                );
                assert_relays_kms(&message, ENCRYPTION_SENTENCE, kms_code, kms_phrase, &case);
                assert_eq!(
                    version_count(&emulator, &name, &case).await,
                    before,
                    "case {case:?}: a refused write stored a version"
                );
            }

            // A metadata-only update encrypts nothing, so the key's state does not
            // reach it: "If you include SecretString or SecretBinary, then Secrets
            // Manager creates a new version".
            let metadata_case = format!("{case}, metadata-only UpdateSecret");
            emulator
                .sm_ok(
                    "UpdateSecret",
                    &json!({"SecretId": name, "Description": "still describable"}),
                    &metadata_case,
                )
                .await;

            // And a create naming the same key stores no secret at all. A create
            // names the key the secret is to be held under, and the account
            // prepares that key before the secret exists: a key KMS will not
            // encrypt with is a prerequisite of the create that was not
            // completed, which is this operation's own
            // `PreconditionNotMetException` — "The request failed because you did
            // not complete all the prerequisite steps." — carrying the KMS
            // refusal that says which step.
            let create_case = format!("{case}, CreateSecret");
            let fresh = format!("{name}-fresh");
            let (code, message) = emulator
                .sm_err(
                    "CreateSecret",
                    &create_body(&fresh, Some(&key_arn), value),
                    &create_case,
                )
                .await;
            assert_eq!(
                code, "PreconditionNotMetException",
                "case {create_case:?}: expected PreconditionNotMetException, got {code} with \
                 {message:?}"
            );
            assert_relays_kms(
                &message,
                ENCRYPTION_SENTENCE,
                kms_code,
                kms_phrase,
                &create_case,
            );
            let (status, parsed) = emulator
                .sm("DescribeSecret", &json!({"SecretId": fresh}))
                .await;
            assert_eq!(
                status, 400,
                "case {create_case:?}: expected the refused create to store nothing, got \
                 {status} with {parsed}"
            );
            assert_eq!(
                parsed["__type"]
                    .as_str()
                    .and_then(|raw| raw.split('#').next_back()),
                Some("ResourceNotFoundException"),
                "case {create_case:?}: expected the refused create to store nothing, got \
                 {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn the_identical_read_succeeds_again_once_the_key_is_enabled() {
    for (index, value) in value_cases().iter().enumerate() {
        let emulator = Emulator::new();
        let case = format!("re-enabled key, {}", value.label);
        let (key_arn, _) = emulator.create_key(&case).await;
        let name = format!("re-enabled-{index}");
        emulator
            .sm_ok(
                "CreateSecret",
                &create_body(&name, Some(&key_arn), value),
                &case,
            )
            .await;

        emulator
            .kms_ok("DisableKey", &json!({"KeyId": key_arn}), &case)
            .await;
        let (code, _) = emulator
            .sm_err("GetSecretValue", &json!({"SecretId": name}), &case)
            .await;
        assert_eq!(
            code, "DecryptionFailure",
            "case {case:?}: expected the read to be refused while the key is disabled, got \
             {code}"
        );

        emulator
            .kms_ok("EnableKey", &json!({"KeyId": key_arn}), &case)
            .await;
        // The SAME read, answering the value that was stored before the key was
        // disabled: the version was never lost, only unopenable.
        let read = emulator
            .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
            .await;
        value.assert_round_trip(&read, &case);

        // And the write path recovers with it.
        let replacement = ValueCase::string("a replacement", "written-after-re-enabling");
        let mut body = Map::new();
        body.insert("SecretId".to_string(), json!(name));
        if let Value::Object(members) = replacement.wire_members() {
            body.extend(members);
        }
        emulator
            .sm_ok("PutSecretValue", &Value::Object(body), &case)
            .await;
        let reread = emulator
            .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
            .await;
        replacement.assert_round_trip(&reread, &case);
    }
}

#[tokio::test]
async fn a_secret_on_the_service_managed_key_needs_no_customer_key() {
    let emulator = Emulator::new();
    // Every spelling of "the Amazon Web Services managed key": naming none, the
    // empty string ("If you set this to an empty string, Secrets Manager uses the
    // Amazon Web Services managed key aws/secretsmanager",
    // `UpdateSecretRequest$KmsKeyId`), and that key's alias in both spellings.
    for (form_index, (form, kms_key_id)) in [
        ("no KmsKeyId", None),
        ("an empty KmsKeyId", Some("")),
        ("the managed key alias", Some("alias/aws/secretsmanager")),
        ("the managed key alias, unprefixed", Some("aws/secretsmanager")),
    ]
    .into_iter()
    .enumerate()
    {
        for (index, value) in value_cases().iter().enumerate() {
            let case = format!("{form}, {}", value.label);
            let name = format!("managed-{form_index}-{index}");
            emulator
                .sm_ok("CreateSecret", &create_body(&name, kms_key_id, value), &case)
                .await;
            assert!(
                !emulator.current_version_is_sealed(&name, &case),
                "case {case:?}: expected a secret on the service-managed key to hold its \
                 value as sent, got a sealed value"
            );
            let read = emulator
                .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
                .await;
            value.assert_round_trip(&read, &case);
        }
    }
}

#[tokio::test]
async fn a_key_change_moves_the_staged_versions_to_the_new_key() {
    // "The ARN, key ID, or alias of the KMS key that Secrets Manager uses to
    // encrypt new secret versions as well as any existing versions with the
    // staging labels AWSCURRENT, AWSPENDING, or AWSPREVIOUS."
    // (`UpdateSecretRequest$KmsKeyId`, this service's Smithy model.) So pointing a
    // secret at another key re-encrypts the versions those labels reach, and the
    // key they came from is no longer needed to read them; a DEPRECATED version
    // carries none of those labels, so it keeps opening under the key its own data
    // key was wrapped by.
    //
    // The rows span the key the update names — another customer managed key, and
    // the Amazon Web Services managed key, which the empty string names — and
    // whether the same call also stores a value, since the key change is what
    // moves the versions either way.
    for (form, names_a_customer_key) in [
        ("another customer managed key", true),
        ("the Amazon Web Services managed key", false),
    ] {
        for carries_a_value in [false, true] {
            let case = format!("{form}, carrying a value: {carries_a_value}");
            let emulator = Emulator::new();
            let (first_key, _) = emulator.create_key(&case).await;
            let (second_key, _) = emulator.create_key(&case).await;
            let name = format!(
                "re-keyed-{}-{carries_a_value}",
                if names_a_customer_key { "cmk" } else { "managed" }
            );

            // Three versions under the FIRST key, which leaves the oldest
            // deprecated: AWSCURRENT on the third, AWSPREVIOUS on the second, no
            // label at all on the first.
            let deprecated = ValueCase::string("the deprecated value", "value-one");
            let created = emulator
                .sm_ok(
                    "CreateSecret",
                    &create_body(&name, Some(&first_key), &deprecated),
                    &case,
                )
                .await;
            let deprecated_id = version_id_of(&created, &case);
            let previous = ValueCase::string("the previous value", "value-two");
            let previous_id = version_id_of(
                &emulator
                    .sm_ok("PutSecretValue", &value_body(&name, &previous, &[]), &case)
                    .await,
                &case,
            );
            let current = ValueCase::string("the current value", "value-three");
            let current_id = version_id_of(
                &emulator
                    .sm_ok("PutSecretValue", &value_body(&name, &current, &[]), &case)
                    .await,
                &case,
            );

            // The key change, with or without a value of its own.
            let written = ValueCase::string("the written value", "value-four");
            let named_key = if names_a_customer_key {
                second_key.clone()
            } else {
                String::new()
            };
            let mut body = Map::new();
            body.insert("SecretId".to_string(), json!(name.clone()));
            body.insert("KmsKeyId".to_string(), json!(named_key));
            if carries_a_value {
                if let Value::Object(members) = written.wire_members() {
                    body.extend(members);
                }
            }
            emulator
                .sm_ok("UpdateSecret", &Value::Object(body), &case)
                .await;
            // Only a value creates a version, so a key change stages nothing: the
            // label a rotation would stage resolves nothing at all.
            let (code, message) = emulator
                .sm_err(
                    "GetSecretValue",
                    &json!({"SecretId": name.clone(), "VersionStage": "AWSPENDING"}),
                    &case,
                )
                .await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected the key change to stage no version, got {code} with \
                 {message:?}"
            );
            assert_eq!(
                message,
                "Secrets Manager can't find the specified secret value for staging label: \
                 AWSPENDING",
                "case {case:?}: expected the staging-label sentence, got {message:?}"
            );
            // The secret is on the named key, so its current version is held as
            // that key holds a value: sealed under a customer managed key, and as
            // sent on the key no caller can act on.
            assert_eq!(
                emulator.current_version_is_sealed(&name, &case),
                names_a_customer_key,
                "case {case:?}: expected the current version to be sealed only under a customer \
                 managed key"
            );

            // With the FIRST key disabled, every version the labels reached is
            // still readable: the change moved them off that key.
            emulator
                .kms_ok("DisableKey", &json!({"KeyId": first_key.clone()}), &case)
                .await;
            let expected_current = if carries_a_value { &written } else { &current };
            expected_current.assert_round_trip(
                &emulator
                    .sm_ok("GetSecretValue", &json!({"SecretId": name.clone()}), &case)
                    .await,
                &case,
            );
            for (labelled, value) in [(&current_id, &current), (&previous_id, &previous)] {
                value.assert_round_trip(
                    &emulator
                        .sm_ok(
                            "GetSecretValue",
                            &json!({"SecretId": name.clone(), "VersionId": labelled.clone()}),
                            &case,
                        )
                        .await,
                    &case,
                );
            }

            // The deprecated version carried none of those labels, so it was left
            // under the key it was sealed with and that key's state refuses it.
            let (code, message) = emulator
                .sm_err(
                    "GetSecretValue",
                    &json!({"SecretId": name.clone(), "VersionId": deprecated_id.clone()}),
                    &case,
                )
                .await;
            assert_eq!(
                code, "DecryptionFailure",
                "case {case:?}: expected the version the change left under the disabled key to \
                 be refused, got {code} with {message:?}"
            );
            assert_relays_kms(
                &message,
                DECRYPTION_SENTENCE,
                "DisabledException",
                "is disabled.",
                &case,
            );
            assert!(
                message.contains(&first_key),
                "case {case:?}: expected the relayed refusal to name the key {first_key}, got \
                 {message:?}"
            );

            // And the key the versions moved TO is now the one their reads depend
            // on — while a secret moved to the managed key names no key a caller
            // can act on, so nothing stands between its reads and its value.
            emulator
                .kms_ok("DisableKey", &json!({"KeyId": second_key.clone()}), &case)
                .await;
            if names_a_customer_key {
                let (code, message) = emulator
                    .sm_err("GetSecretValue", &json!({"SecretId": name.clone()}), &case)
                    .await;
                assert_eq!(
                    code, "DecryptionFailure",
                    "case {case:?}: expected the read to depend on the new key, got {code} with \
                     {message:?}"
                );
                assert!(
                    message.contains(&second_key),
                    "case {case:?}: expected the relayed refusal to name the new key \
                     {second_key}, got {message:?}"
                );
            } else {
                expected_current.assert_round_trip(
                    &emulator
                        .sm_ok("GetSecretValue", &json!({"SecretId": name.clone()}), &case)
                        .await,
                    &case,
                );
            }
        }
    }
}

/// The `VersionId` a write reported.
pub(crate) fn version_id_of(body: &Value, case: &str) -> String {
    body["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no VersionId in {body}"))
        .to_string()
}

/// A write body naming `name`, carrying this value, and claiming the staging
/// labels `stages` — none of them leaving the write on the default, `AWSCURRENT`
/// ("If you don't include VersionStages, then Secrets Manager automatically moves
/// the staging label AWSCURRENT to this version",
/// `PutSecretValueRequest$VersionStages`, this service's Smithy model).
pub(crate) fn value_body(name: &str, value: &ValueCase, stages: &[&str]) -> Value {
    let mut body = Map::new();
    body.insert("SecretId".to_string(), json!(name));
    if !stages.is_empty() {
        body.insert("VersionStages".to_string(), json!(stages));
    }
    if let Value::Object(members) = value.wire_members() {
        body.extend(members);
    }
    Value::Object(body)
}

#[tokio::test]
async fn an_emulator_without_kms_holds_the_value_as_sent() {
    let emulator = Emulator::without_kms();
    // A well-formed customer managed key ARN that no KMS in this emulator can
    // answer for, because this emulator holds none: the seal call is refused by the
    // transport itself, which says nothing about the key, so the value is stored
    // and read back as sent rather than every write of a value being refused.
    let key_arn = format!(
        "arn:aws:kms:{REGION}:{}:key/9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d",
        engine::EMULATOR_ACCOUNT_ID
    );
    for (index, value) in value_cases().iter().enumerate() {
        let case = format!("no KMS mounted, {}", value.label);
        let name = format!("no-kms-{index}");
        emulator
            .sm_ok(
                "CreateSecret",
                &create_body(&name, Some(&key_arn), value),
                &case,
            )
            .await;
        assert!(
            !emulator.current_version_is_sealed(&name, &case),
            "case {case:?}: expected the value to be held as sent where no KMS answers, got a \
             sealed value nothing could open"
        );
        let read = emulator
            .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
            .await;
        value.assert_round_trip(&read, &case);

        let replacement = ValueCase::string("a replacement", "written-with-no-kms-mounted");
        let mut body = Map::new();
        body.insert("SecretId".to_string(), json!(name));
        if let Value::Object(members) = replacement.wire_members() {
            body.extend(members);
        }
        emulator
            .sm_ok("PutSecretValue", &Value::Object(body), &case)
            .await;
        let reread = emulator
            .sm_ok("GetSecretValue", &json!({"SecretId": name}), &case)
            .await;
        replacement.assert_round_trip(&reread, &case);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_under_one_token_all_answer_the_same_version() {
    // The envelope releases the store's guard to call KMS, so the refusals a write
    // is subject to are decided again when it lands. "This value helps ensure
    // idempotency" (`CreateSecretRequest$ClientRequestToken`): identical creates
    // carrying one token are ONE request however they interleave, so each is
    // answered from the version that token named rather than refused for a name a
    // racing copy of itself took.
    let emulator = Emulator::new();
    let case = "concurrent identical creates under one token";
    let (key_arn, _) = emulator.create_key(case).await;
    // `ClientRequestTokenType` is modelled with a length bound of 32..64.
    let token = "f0b3e2a1-7c4d-4e5f-8a9b-0c1d2e3f4a5b";
    let value = ValueCase::string("one value", "the-one-value");
    let name = "concurrent-token";

    let mut body = create_body(name, Some(&key_arn), &value);
    if let Value::Object(members) = &mut body {
        members.insert("ClientRequestToken".to_string(), json!(token));
    }

    let mut running = Vec::new();
    for _ in 0..6 {
        let emulator = emulator.clone();
        let body = body.clone();
        running.push(tokio::spawn(async move {
            emulator.sm("CreateSecret", &body).await
        }));
    }
    for (attempt, task) in running.into_iter().enumerate() {
        let case = format!("{case}, attempt {attempt}");
        let (status, parsed) = task
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the request task failed: {error}"));
        assert_eq!(
            status, 200,
            "case {case:?}: expected every identical create to succeed, got {status} with \
             body {parsed}"
        );
        assert_eq!(
            parsed["VersionId"].as_str(),
            Some(token),
            "case {case:?}: expected the token's own version back, got {parsed}"
        );
    }

    // One secret, one version, and the value readable through the key it was
    // sealed under.
    assert_eq!(
        version_count(&emulator, name, case).await,
        1,
        "case {case:?}: six identical creates must leave one version"
    );
    let read = emulator
        .sm_ok("GetSecretValue", &json!({"SecretId": name}), case)
        .await;
    value.assert_round_trip(&read, case);
}

/// The actions a key policy grants the caller, and what a secret under that key may then
/// be read and written for.
struct KeyPolicyRow {
    case: &'static str,
    /// The cryptographic actions the statement names, beside the admin ones every row
    /// keeps so the key stays describable.
    granted: &'static [&'static str],
    /// Whether a READ of the secret's value may be served — `kms:Decrypt` on the key.
    reads: bool,
    /// Whether a WRITE of a value may be stored — `kms:GenerateDataKey` on the key.
    writes: bool,
}

/// The rows span what the rule generalises over: which of the two cryptographic actions a
/// secret's key grants, each on its own, both together, the wildcard that covers them, and
/// neither.
const KEY_POLICY_ROWS: &[KeyPolicyRow] = &[
    KeyPolicyRow {
        case: "the key grants only kms:Decrypt",
        granted: &["kms:Decrypt"],
        reads: true,
        writes: false,
    },
    KeyPolicyRow {
        case: "the key grants only kms:GenerateDataKey",
        granted: &["kms:GenerateDataKey"],
        reads: false,
        writes: true,
    },
    KeyPolicyRow {
        case: "the key grants both cryptographic actions",
        granted: &["kms:Decrypt", "kms:GenerateDataKey"],
        reads: true,
        writes: true,
    },
    KeyPolicyRow {
        case: "the key grants every action by wildcard",
        granted: &["kms:*"],
        reads: true,
        writes: true,
    },
    KeyPolicyRow {
        case: "the key grants the pattern covering the data-key actions",
        granted: &["kms:GenerateDataKey*"],
        reads: false,
        writes: true,
    },
    KeyPolicyRow {
        case: "the key grants neither cryptographic action",
        granted: &[],
        reads: false,
        writes: false,
    },
];

/// The actions a key policy must keep granting whatever else it states, so the key can be
/// described and its policy replaced: the service asks the key about itself through
/// `kms:DescribeKey`, and the suite replaces the policy again between rows.
const KEY_ADMIN_ACTIONS: [&str; 3] = ["kms:DescribeKey", "kms:GetKeyPolicy", "kms:PutKeyPolicy"];

/// A key policy granting `granted` plus [`KEY_ADMIN_ACTIONS`] to the principal this
/// emulator authenticates every request as.
pub(crate) fn key_policy(granted: &[&str]) -> String {
    let mut actions: Vec<&str> = KEY_ADMIN_ACTIONS.to_vec();
    actions.extend_from_slice(granted);
    json!({
        "Version": "2012-10-17",
        "Id": "key-under-test",
        "Statement": [{
            "Sid": "StatementUnderTest",
            "Effect": "Allow",
            "Principal": {"AWS": service_com_amazonaws_kms_trentservice::state::caller_identity_arn()},
            "Action": actions,
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Assert an authorization refusal of the secret's key is worded as that class is: Secrets
/// Manager's own sentence for it and nothing else.
///
/// The table for that wording, and for what it must NOT carry, is
/// [`crate::secret_kms_access_denied`]; this suite asserts the sentence so a key-policy row
/// below is answered the same way as every other refusal of the access a secret's key
/// withholds.
fn assert_denies_key_use(message: &str, case: &str) {
    assert_eq!(
        message,
        crate::secret_kms_access_denied::KMS_ACCESS_DENIED_MESSAGE,
        "case {case:?}: expected the refusal of the access the key withheld to read {:?}, got \
         {message:?}",
        crate::secret_kms_access_denied::KMS_ACCESS_DENIED_MESSAGE
    );
}

/// RULE: a read of a secret under a customer managed key performs `kms:Decrypt` on that key
/// as the CALLER, and a write performs `kms:GenerateDataKey` — so the authority in force on
/// the key decides each of them on its own, and a caller the key does not permit the action
/// is NOT AUTHORIZED (HTTP 400 `AccessDeniedException`) rather than served or told the value
/// could not be encrypted.
#[tokio::test]
async fn the_keys_own_grants_decide_each_read_and_each_write() {
    for (index, row) in KEY_POLICY_ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let (key_arn, _) = emulator.create_key(case).await;
        let name = format!("key-policy-{index}");
        let value = ValueCase::string("the initial value", "under-a-granting-key");

        // The secret is created while the key still carries its default policy, so the
        // refusals below are the policy under test and nothing else.
        emulator
            .sm_ok(
                "CreateSecret",
                &create_body(&name, Some(&key_arn), &value),
                case,
            )
            .await;
        emulator
            .kms_ok(
                "PutKeyPolicy",
                &json!({
                    "KeyId": key_arn,
                    "Policy": key_policy(row.granted),
                    // Every document here is the one statement under test, so it leaves no
                    // path to a later PutKeyPolicy that the lockout safety check accepts.
                    "BypassPolicyLockoutSafetyCheck": true,
                }),
                case,
            )
            .await;

        // The READ: `kms:Decrypt` on the key.
        let read_case = format!("{case}, GetSecretValue");
        if row.reads {
            let read = emulator
                .sm_ok("GetSecretValue", &json!({"SecretId": name}), &read_case)
                .await;
            value.assert_round_trip(&read, &read_case);
        } else {
            let (code, message) = emulator
                .sm_err("GetSecretValue", &json!({"SecretId": name}), &read_case)
                .await;
            assert_eq!(
                code, "AccessDeniedException",
                "case {read_case:?}: expected AccessDeniedException, got {code} with {message:?}"
            );
            assert_denies_key_use(&message, &read_case);
        }

        // The WRITE: `kms:GenerateDataKey` on the key.
        let replacement = ValueCase::string("a replacement", "written-under-a-granting-key");
        let mut write_body = Map::new();
        write_body.insert("SecretId".to_string(), json!(name));
        if let Value::Object(members) = replacement.wire_members() {
            write_body.extend(members);
        }
        let write_body = Value::Object(write_body);
        let write_case = format!("{case}, PutSecretValue");
        let before = version_count(&emulator, &name, &write_case).await;
        if row.writes {
            emulator
                .sm_ok("PutSecretValue", &write_body, &write_case)
                .await;
            assert_eq!(
                version_count(&emulator, &name, &write_case).await,
                before + 1,
                "case {write_case:?}: a permitted write stored no version"
            );
        } else {
            let (code, message) = emulator
                .sm_err("PutSecretValue", &write_body, &write_case)
                .await;
            assert_eq!(
                code, "AccessDeniedException",
                "case {write_case:?}: expected AccessDeniedException, got {code} with {message:?}"
            );
            assert_denies_key_use(&message, &write_case);
            assert_eq!(
                version_count(&emulator, &name, &write_case).await,
                before,
                "case {write_case:?}: a refused write stored a version"
            );
        }

        // And a CREATE under that key is the same write: the secret is held under the key
        // it names, so a key that does not permit the data key stores no secret.
        let create_case = format!("{case}, CreateSecret");
        let fresh = format!("{name}-fresh");
        let created = create_body(&fresh, Some(&key_arn), &value);
        if row.writes {
            emulator.sm_ok("CreateSecret", &created, &create_case).await;
        } else {
            let (code, message) = emulator
                .sm_err("CreateSecret", &created, &create_case)
                .await;
            assert_eq!(
                code, "AccessDeniedException",
                "case {create_case:?}: expected AccessDeniedException, got {code} with \
                 {message:?}"
            );
            assert_denies_key_use(&message, &create_case);
            let (status, parsed) = emulator
                .sm("DescribeSecret", &json!({"SecretId": fresh}))
                .await;
            assert_eq!(
                status, 400,
                "case {create_case:?}: expected the refused create to store nothing, got \
                 {status} with {parsed}"
            );
        }
    }
}
