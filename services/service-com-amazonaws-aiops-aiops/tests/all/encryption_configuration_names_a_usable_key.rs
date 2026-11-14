// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `encryptionConfiguration` a write may store on an
//! investigation group.
//!
//! The rule under test: **an investigation group's `encryptionConfiguration` has to name
//! a key selection the service can honour, so every write of the member refuses — with
//! the modelled `ValidationException` (HTTP 400), storing nothing — a configuration whose
//! two members disagree (the customer managed key type naming no `kmsKeyId`, the Amazon
//! Web Services owned key type naming one) and a `kmsKeyId` that KMS itself reports it
//! could not encrypt with.** The structure is "Use this structure to specify a customer
//! managed KMS key to use to encrypt investigation data" and `kmsKeyId` is "if the
//! investigation group uses a customer managed key for encryption, this field displays
//! the ID of that key" (`smithy_query(query="shape",shape="EncryptionConfiguration")`),
//! so the type selects whether a customer managed key is used and the key id says WHICH
//! key: a selection naming no key, naming two different keys at once, or naming a key
//! that does not exist is not a configuration the group's data could be encrypted under.
//!
//! This service and KMS are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so every key a row names is a key KMS itself minted, in the
//! state KMS itself reports, and no part of this suite tells this service anything about
//! it. A key nothing can be shown about is NOT refused, which the last test pins: with no
//! KMS to ask, a refusal would not be grounded in anything, exactly as the role
//! resolution (`tests/all/investigation_group_role_resolution.rs`) refuses nothing
//! without IAM.
//!
//! Both writers of the member apply the rule, so every row is driven through
//! `CreateInvestigationGroup` AND through `UpdateInvestigationGroup`, and every refusal is
//! checked to have left the store exactly as it was. An account holds one investigation
//! group per Region and a KMS key is regional, so each leg of each row gets its own
//! emulator rather than its own Region.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::{
    AWS_OWNED_KEY_TYPE, CUSTOMER_MANAGED_KMS_KEY_TYPE, KMS_KEY_ID_NOT_ACCEPTED,
    KMS_KEY_ID_REQUIRED, KMS_KEY_NOT_USABLE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account's.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The Region every request below signs for.
const REGION: &str = "us-west-2";

/// Another Region, for the row naming a key of a Region this endpoint does not serve.
const OTHER_REGION: &str = "us-east-1";

/// The alias every seeded key is given, so one row can name the key by an alias KMS
/// resolves rather than by the key's own ARN.
const ALIAS_NAME: &str = "alias/investigations";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`). No IAM is
/// mounted here, so the role resolution refuses nothing and the encryption rules are what
/// decide every row.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error a refused write reports
/// (`smithy_query(query="shape",shape="ValidationException")`: `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// This service and, optionally, KMS on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// This service with KMS mounted beside it, which is what lets the key resolution
    /// answer at all.
    fn with_kms() -> Self {
        Self::new(true)
    }

    /// This service alone: nothing can answer about a key.
    fn without_kms() -> Self {
        Self::new(false)
    }

    fn new(with_kms: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the
        // credential scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
            Arc::clone(&engine),
        ));
        if with_kms {
            router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
                engine,
            ));
        }
        Self { router }
    }

    /// One restJson1 call to this service: its status and parsed body.
    async fn aiops(&self, method: &str, uri: &str, body: Option<&Value>) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("aiops.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        let payload = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
            }
            None => Bytes::new(),
        };
        let request = builder.body(payload).expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut parsed = parse(&response.into_body());
        // restJson1 carries the error shape in `__type`, and the framework also sets the
        // `x-amzn-errortype` header; fold the header in so one accessor reads either.
        if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
            map.entry("__type")
                .or_insert_with(|| Value::String(error_type));
        }
        (status, parsed)
    }

    /// One awsJson1_1 call to KMS that must succeed.
    async fn kms_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kms.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/kms/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = parse(&response.into_body());
        assert_eq!(
            status, 200,
            "case {case:?}: KMS {operation} {body} must succeed, got {status} / {parsed:?}"
        );
        parsed
    }

    /// A key KMS itself minted in [`REGION`], answered by the ARN KMS reports for it —
    /// read off the response rather than constructed, so a row names the key KMS holds.
    async fn created_key(&self, case: &str) -> String {
        let created = self.kms_ok(case, "CreateKey", &json!({})).await;
        created["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created key carries an Arn, got {created}"))
            .to_string()
    }

    /// `POST` an investigation group carrying `configuration` (`None` omits the member).
    async fn create(&self, configuration: Option<&Value>) -> (u16, Value) {
        let mut body = json!({ "name": "encryption-probe", "roleArn": ROLE_ARN });
        if let Some(configuration) = configuration {
            body["encryptionConfiguration"] = configuration.clone();
        }
        self.aiops("POST", "/investigationGroups", Some(&body))
            .await
    }

    /// Create an investigation group that must be minted, answering its ARN.
    async fn create_ok(&self, case: &str, configuration: Option<&Value>) -> String {
        let (status, created) = self.create(configuration).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateInvestigationGroup, got {status} / {created:?}"
        );
        created
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create carried no arn, got {created:?}"))
            .to_string()
    }

    /// `PATCH` the group `identifier` with `configuration` (`None` omits the member).
    async fn update(&self, identifier: &str, configuration: Option<&Value>) -> (u16, Value) {
        let mut body = json!({});
        if let Some(configuration) = configuration {
            body["encryptionConfiguration"] = configuration.clone();
        }
        self.aiops(
            "PATCH",
            &format!("/investigationGroups/{}", label(identifier)),
            Some(&body),
        )
        .await
    }

    /// The `encryptionConfiguration` a read of the group reports.
    async fn stored_configuration(&self, case: &str, identifier: &str) -> Value {
        let (status, body) = self
            .aiops(
                "GET",
                &format!("/investigationGroups/{}", label(identifier)),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetInvestigationGroup, got {status} / {body:?}"
        );
        body.get("encryptionConfiguration")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the read reported no encryptionConfiguration, got {body:?}")
            })
    }

    /// Assert this Region holds no investigation group: a refused create must have
    /// persisted nothing, and must not have consumed the Region's one-group quota either.
    async fn assert_no_group(&self, case: &str) {
        let (status, listed) = self.aiops("GET", "/investigationGroups", None).await;
        let groups = listed
            .get("investigationGroups")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(
            (status, groups),
            (200, 0),
            "case {case:?}: a refused create must store no group, got {status} / {listed:?}"
        );
    }
}

/// A response body as JSON; an empty body reads as `Value::Null`.
fn parse(raw: &Bytes) -> Value {
    if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(raw).to_string()))
    }
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` a failed call carried.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// The configuration a group reports when no customer managed key is involved, which the
/// member's own documentation states ("If you omit this parameter, CloudWatch
/// investigations will use an Amazon Web Services key to encrypt the data").
fn aws_owned() -> Value {
    json!({ "type": AWS_OWNED_KEY_TYPE })
}

/// The configuration a group reports when it is encrypted with the customer managed key
/// `key`.
fn customer_managed(key: &str) -> Value {
    json!({ "type": CUSTOMER_MANAGED_KMS_KEY_TYPE, "kmsKeyId": key })
}

/// A well-formed key ARN of this account and Region that KMS holds no key under.
fn absent_key() -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/8d6a0b2e-19cc-4ff6-9a6f-2d0f6b5b1cf3")
}

/// What a write must do with one configuration.
enum Expected {
    /// It is served, and a later read reports this configuration.
    Stored(Value),
    /// It is refused with the modelled `ValidationException` carrying this sentence, and
    /// nothing is written.
    Refused(String),
}

/// `CreateInvestigationGroup` carrying `supplied` must answer as `expected` says, and a
/// refused create must leave the Region holding no group at all.
async fn assert_create(
    emulator: &Emulator,
    case: &str,
    supplied: Option<&Value>,
    expected: &Expected,
) {
    let (status, created) = emulator.create(supplied).await;
    match expected {
        Expected::Stored(want) => {
            assert_eq!(
                status, 201,
                "case {case:?} (create): expected 201 for {supplied:?}, got {status} / {created:?}"
            );
            let arn = created
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?} (create): the create carried no arn, got {created:?}")
                })
                .to_string();
            let got = emulator.stored_configuration(case, &arn).await;
            assert_eq!(
                &got, want,
                "case {case:?} (create): a create carrying {supplied:?} must store {want}, got \
                 {got:?}"
            );
            // Reporting the configuration does not change it.
            let again = emulator.stored_configuration(case, &arn).await;
            assert_eq!(
                again, got,
                "case {case:?} (create): a second read must report the same configuration, got \
                 {again:?}"
            );
        }
        Expected::Refused(sentence) => {
            assert_eq!(
                (status, error_code(&created), message(&created)),
                (400, VALIDATION.to_string(), sentence.clone()),
                "case {case:?} (create): expected a 400 {VALIDATION} refusal saying {sentence:?} \
                 for {supplied:?}, got {status} / {created:?}"
            );
            emulator.assert_no_group(case).await;
        }
    }
}

/// `UpdateInvestigationGroup` carrying `supplied`, against a group created with `before`,
/// must answer as `expected` says — and a refused update must leave `before` stored.
async fn assert_update(
    emulator: &Emulator,
    case: &str,
    before: &Value,
    supplied: Option<&Value>,
    expected: &Expected,
) {
    let arn = emulator.create_ok(case, Some(before)).await;
    let (status, updated) = emulator.update(&arn, supplied).await;
    match expected {
        Expected::Stored(want) => {
            assert_eq!(
                status, 200,
                "case {case:?} (update): expected 200 for {supplied:?}, got {status} / {updated:?}"
            );
            let got = emulator.stored_configuration(case, &arn).await;
            assert_eq!(
                &got, want,
                "case {case:?} (update): an update carrying {supplied:?} must store {want}, got \
                 {got:?}"
            );
        }
        Expected::Refused(sentence) => {
            assert_eq!(
                (status, error_code(&updated), message(&updated)),
                (400, VALIDATION.to_string(), sentence.clone()),
                "case {case:?} (update): expected a 400 {VALIDATION} refusal saying {sentence:?} \
                 for {supplied:?}, got {status} / {updated:?}"
            );
            let got = emulator.stored_configuration(case, &arn).await;
            assert_eq!(
                &got, before,
                "case {case:?} (update): a refused update must leave the stored configuration as \
                 it was ({before}), got {got:?}"
            );
        }
    }
}

/// One row of the AGREEMENT table: both halves are built from the key the row's own
/// emulator minted, so an accepted row is accepted because the configuration agrees with
/// itself AND names a key KMS holds.
struct AgreementRow {
    case: &'static str,
    /// The `encryptionConfiguration` the write carries; `None` omits the member.
    supplied: fn(&str) -> Option<Value>,
    /// What the write must do with it.
    expected: fn(&str) -> Expected,
}

/// Every form the structure admits, since neither of its members is `@required`: the
/// member absent, a structure carrying neither member, a structure carrying only a key
/// id, and each member of the modelled `EncryptionConfigurationType` value set with and
/// without a key id.
fn agreement_rows() -> Vec<AgreementRow> {
    vec![
        AgreementRow {
            case: "the member omitted entirely",
            supplied: |_| None,
            expected: |_| Expected::Stored(aws_owned()),
        },
        AgreementRow {
            case: "a structure carrying neither member",
            supplied: |_| Some(json!({})),
            expected: |_| Expected::Stored(aws_owned()),
        },
        AgreementRow {
            case: "the Amazon Web Services owned key, named",
            supplied: |_| Some(aws_owned()),
            expected: |_| Expected::Stored(aws_owned()),
        },
        AgreementRow {
            case: "a key id with no type, which asks for that key",
            supplied: |key| Some(json!({ "kmsKeyId": key })),
            expected: |key| Expected::Stored(customer_managed(key)),
        },
        AgreementRow {
            case: "a customer managed key with its key id",
            supplied: |key| Some(customer_managed(key)),
            expected: |key| Expected::Stored(customer_managed(key)),
        },
        AgreementRow {
            case: "a customer managed key naming no key id",
            supplied: |_| Some(json!({ "type": CUSTOMER_MANAGED_KMS_KEY_TYPE })),
            expected: |_| Expected::Refused(KMS_KEY_ID_REQUIRED.to_string()),
        },
        AgreementRow {
            case: "the Amazon Web Services owned key naming a key id",
            supplied: |key| Some(json!({ "type": AWS_OWNED_KEY_TYPE, "kmsKeyId": key })),
            expected: |_| Expected::Refused(KMS_KEY_ID_NOT_ACCEPTED.to_string()),
        },
        AgreementRow {
            // The agreement is decided from the request alone, so it reports before the
            // key is resolved: this key does not exist either, and the sentence is still
            // the disagreement's.
            case: "the Amazon Web Services owned key naming a key that does not exist",
            supplied: |_| {
                Some(json!({ "type": AWS_OWNED_KEY_TYPE, "kmsKeyId": absent_key() }))
            },
            expected: |_| Expected::Refused(KMS_KEY_ID_NOT_ACCEPTED.to_string()),
        },
    ]
}

/// The two members of the structure have to agree, whichever write carries them.
#[tokio::test]
async fn both_writes_require_the_configuration_to_agree_with_itself() {
    for row in agreement_rows() {
        let creating = Emulator::with_kms();
        let create_key = creating.created_key(row.case).await;
        assert_create(
            &creating,
            row.case,
            (row.supplied)(&create_key).as_ref(),
            &(row.expected)(&create_key),
        )
        .await;

        let updating = Emulator::with_kms();
        let update_key = updating.created_key(row.case).await;
        // The group the update addresses starts out encrypted with that same key, so an
        // accepted row visibly changes the stored configuration and a refused one visibly
        // leaves it alone.
        assert_update(
            &updating,
            row.case,
            &customer_managed(&update_key),
            (row.supplied)(&update_key).as_ref(),
            &(row.expected)(&update_key),
        )
        .await;
    }
}

/// The state KMS reports a seeded key in.
#[derive(Clone, Copy)]
enum KeyState {
    Enabled,
    Disabled,
    PendingDeletion,
}

/// The key one leg of a resolution row is built around, as KMS reports it.
struct Key {
    /// `KeyMetadata$Arn`, the `key/<key id>` form.
    arn: String,
    /// The key id alone, which is that ARN's resource id — what a row naming the same key
    /// under another Region or account builds its ARN around.
    id: String,
    /// The ARN of [`ALIAS_NAME`], which KMS resolves to this key.
    alias: String,
}

/// A fresh emulator holding one key, aliased by [`ALIAS_NAME`], in the state `state`
/// names — every state put there through KMS's own API, so what the rule is judged on is
/// what KMS reports.
async fn seeded(case: &str, state: KeyState) -> (Emulator, Key) {
    let emulator = Emulator::with_kms();
    let arn = emulator.created_key(case).await;
    let id = arn
        .rsplit_once("key/")
        .expect("a key ARN carries a key id")
        .1
        .to_string();
    emulator
        .kms_ok(
            case,
            "CreateAlias",
            &json!({ "AliasName": ALIAS_NAME, "TargetKeyId": arn }),
        )
        .await;
    match state {
        KeyState::Enabled => {}
        KeyState::Disabled => {
            emulator
                .kms_ok(case, "DisableKey", &json!({ "KeyId": arn }))
                .await;
        }
        // A key SCHEDULED FOR DELETION is reported in a state it cannot be used in, the
        // same as a disabled one: "when KeyState is Enabled this value is true, otherwise
        // it is false" (`KeyMetadata$Enabled`).
        KeyState::PendingDeletion => {
            emulator
                .kms_ok(
                    case,
                    "ScheduleKeyDeletion",
                    &json!({ "KeyId": arn, "PendingWindowInDays": 7 }),
                )
                .await;
        }
    }
    let alias = format!("arn:aws:kms:{REGION}:{ACCOUNT}:{ALIAS_NAME}");
    (emulator, Key { arn, id, alias })
}

/// One row of the RESOLUTION table.
struct KeyRow {
    case: &'static str,
    /// The state KMS reports the row's key in.
    state: KeyState,
    /// The `kmsKeyId` the write names, built from the key KMS minted for this leg.
    value: fn(&Key) -> String,
    /// Whether the write must be served.
    admitted: bool,
}

/// The rows differ along what KMS reports for the value: a key it holds enabled, the
/// identifier FORMS that name that same key (its own ARN, an alias ARN), the states it
/// reports a key in it cannot encrypt with, and the values it reports no key of this
/// endpoint for (a key ARN it holds nothing under, that same key id in another Region, an
/// ARN of another account, an ARN naming no resource, another vendor's ARN).
///
/// A BARE key id is not among them: the member's modelled pattern is `^arn:.*$`
/// (`smithy_query(query="shape",shape="EncryptionConfiguration")`), so the constraint walk
/// refuses one before any resolution runs — which
/// `tests/all/encryption_configuration_keeps_the_named_key.rs` pins.
fn key_rows() -> Vec<KeyRow> {
    vec![
        KeyRow {
            case: "the ARN of an enabled key",
            state: KeyState::Enabled,
            value: |key| key.arn.clone(),
            admitted: true,
        },
        KeyRow {
            case: "an alias ARN KMS resolves to that same key",
            state: KeyState::Enabled,
            value: |key| key.alias.clone(),
            admitted: true,
        },
        KeyRow {
            case: "the ARN of a disabled key",
            state: KeyState::Disabled,
            value: |key| key.arn.clone(),
            admitted: false,
        },
        KeyRow {
            case: "the ARN of a key pending deletion",
            state: KeyState::PendingDeletion,
            value: |key| key.arn.clone(),
            admitted: false,
        },
        KeyRow {
            case: "a well-formed key ARN KMS holds no key for",
            state: KeyState::Enabled,
            value: |_| absent_key(),
            admitted: false,
        },
        KeyRow {
            case: "that same key id in a Region this endpoint does not serve",
            state: KeyState::Enabled,
            value: |key| format!("arn:aws:kms:{OTHER_REGION}:{ACCOUNT}:key/{}", key.id),
            admitted: false,
        },
        KeyRow {
            case: "that same key id under another account",
            state: KeyState::Enabled,
            value: |key| format!("arn:aws:kms:{REGION}:{FOREIGN_ACCOUNT}:key/{}", key.id),
            admitted: false,
        },
        KeyRow {
            case: "the shortest value the member admits, which names no key",
            state: KeyState::Enabled,
            value: |_| "arn:".to_string(),
            admitted: false,
        },
        KeyRow {
            case: "an ARN of another vendor entirely",
            state: KeyState::Enabled,
            value: |_| format!("arn:aws:s3:::{ACCOUNT}-investigation-data"),
            admitted: false,
        },
    ]
}

/// A write stores the key it names only when KMS reports a key it could encrypt with, and
/// a refused write leaves the store as it was — for both writers of the member.
#[tokio::test]
async fn both_writes_resolve_the_key_against_kms() {
    for row in key_rows() {
        let (creating, create_key) = seeded(row.case, row.state).await;
        let create_value = (row.value)(&create_key);
        let create_expected = if row.admitted {
            Expected::Stored(customer_managed(&create_value))
        } else {
            Expected::Refused(KMS_KEY_NOT_USABLE.to_string())
        };
        assert_create(
            &creating,
            row.case,
            Some(&json!({ "kmsKeyId": create_value })),
            &create_expected,
        )
        .await;

        let (updating, update_key) = seeded(row.case, row.state).await;
        let update_value = (row.value)(&update_key);
        let update_expected = if row.admitted {
            Expected::Stored(customer_managed(&update_value))
        } else {
            Expected::Refused(KMS_KEY_NOT_USABLE.to_string())
        };
        // The group starts out on the Amazon Web Services owned key, which names no key to
        // resolve, so the key id a later read reports can only have come from the update.
        assert_update(
            &updating,
            row.case,
            &aws_owned(),
            Some(&json!({ "kmsKeyId": update_value })),
            &update_expected,
        )
        .await;
    }
}

/// The STATE of the key is read on every write rather than once: the very same key is
/// refused while KMS reports it disabled and accepted again once KMS reports it enabled,
/// and the configuration the refusal did not store is not there in between.
#[tokio::test]
async fn a_disabled_key_is_refused_until_kms_reports_it_enabled_again() {
    let case = "a key disabled and then enabled again";
    let (emulator, key) = seeded(case, KeyState::Disabled).await;
    let arn = emulator.create_ok(case, Some(&aws_owned())).await;
    let supplied = customer_managed(&key.arn);

    let (status, refused) = emulator.update(&arn, Some(&supplied)).await;
    assert_eq!(
        (status, error_code(&refused), message(&refused)),
        (400, VALIDATION.to_string(), KMS_KEY_NOT_USABLE.to_string()),
        "case {case:?}: a disabled key must be refused, got {status} / {refused:?}"
    );
    let between = emulator.stored_configuration(case, &arn).await;
    assert_eq!(
        between,
        aws_owned(),
        "case {case:?}: the refused update must have stored nothing, got {between:?}"
    );

    emulator
        .kms_ok(case, "EnableKey", &json!({ "KeyId": key.arn }))
        .await;
    let (status, served) = emulator.update(&arn, Some(&supplied)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the same key must be accepted once KMS reports it enabled, got {status} / \
         {served:?}"
    );
    let after = emulator.stored_configuration(case, &arn).await;
    assert_eq!(
        after, supplied,
        "case {case:?}: the group must report the key the accepted update named, got {after:?}"
    );
}

/// A key nothing can be shown about is NOT refused: with no KMS mounted beside this
/// service nothing answers about the key, and a refusal has to be grounded in what KMS
/// answered — so the write is served and the group reports the key it named.
#[tokio::test]
async fn a_key_no_kms_answered_about_is_not_refused() {
    let emulator = Emulator::without_kms();
    let case = "a key ARN with no KMS mounted to ask";
    let supplied = customer_managed(&absent_key());
    let arn = emulator.create_ok(case, Some(&supplied)).await;
    let got = emulator.stored_configuration(case, &arn).await;
    assert_eq!(
        got, supplied,
        "case {case:?}: with nothing to ask about the key, the write must be served and store the \
         key it named, got {got:?}"
    );
}
