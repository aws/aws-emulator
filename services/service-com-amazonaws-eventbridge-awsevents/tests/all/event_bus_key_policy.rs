// SPDX-License-Identifier: CC0-1.0

//! The KEY POLICY of the customer managed key an event bus is configured with decides
//! whether that configuration is accepted at all, end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter` with KMS mounted beside EventBridge on one engine — so the key is
//! read where its own handlers wrote it and the outcome is asserted where a client sees it:
//! in the refusal, and in what the bus reports back afterwards.
//!
//! The RULE asserted here: a bus that names a `KmsKeyIdentifier` has every event on it
//! encrypted under that key by EventBridge itself, as its own service principal acting for
//! that bus — "if you do not specify a customer managed key identifier, EventBridge uses an
//! Amazon Web Services owned key"
//! (`smithy_query(query="doc",shape="CreateEventBusRequest",member="KmsKeyIdentifier")`) —
//! and such a principal holds no identity-based policies, so the KEY's own policy is the
//! whole authority over the use. A key whose policy does not admit this service is a bus
//! that could never carry an event, so the configuration is refused with
//! `AccessDeniedException` and the bus is left exactly as it was: not created by a refused
//! `CreateEventBus`, and still on the key it had after a refused `UpdateEventBus`. Writing
//! the statement afterwards lets the identical call through with nothing else changed.
//!
//! The rows span the class rather than the one recorded case: they differ along what the
//! key's policy states — whether a statement about this service exists at all, which
//! principal it names, which actions it covers (all of the ones a bus needs, one of them,
//! a wildcard over the KMS namespace), which `Effect` it states, and which condition it is
//! scoped by (the service the use arrives through, the bus the data belongs to, the
//! account that owns it) — and along whether the bus names a key this account holds at all.
//! Every row is asserted for BOTH writers of a bus's key, which is where the rule holds.
//!
//! The harness (the router with EventBridge, KMS and the target services on one engine) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::SERVICE_PRINCIPAL;

use crate::target_dispatch::{emulator, events, rpc, Emulator, REGION};

/// The bus every row configures. One name for both writers, so a policy scoped to the
/// bus's own ARN is the same statement in both legs.
const BUS: &str = "encrypted-bus";

/// A second bus, named only as another statement's condition value — no call below
/// configures it.
const OTHER_BUS: &str = "another-bus";

/// The KMS action an event bus's key is used through, and the one a refusal names:
/// encrypting what is published onto the bus means having a data key generated under that
/// key.
const BUS_KEY_ACTION: &str = "kms:GenerateDataKey";

/// The actions the EventBridge user guide's key policy for an event bus lists, which is
/// the statement an account writes such a key with.
const BUS_KEY_ACTIONS: [&str; 3] = ["kms:DescribeKey", "kms:GenerateDataKey", "kms:Decrypt"];

/// Another service's principal, which is not the one a bus's key is used by.
const OTHER_SERVICE_PRINCIPAL: &str = "sqs.amazonaws.com";

/// The condition keys a key policy scopes a service's use of a key by: the service the use
/// arrives through, the encryption context the data is bound to, and the account the
/// resource acting belongs to.
const VIA_SERVICE_KEY: &str = "kms:ViaService";
const EVENT_BUS_CONTEXT_KEY: &str = "kms:EncryptionContext:aws:events:event-bus:arn";
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The account every ARN below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the bus `name` in the Region every call here is signed for.
fn bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// The regional endpoint name of the service a use of the key arrives through, which is
/// how a key policy's `kms:ViaService` spells one.
fn via_service(vendor: &str) -> String {
    format!("{vendor}.{REGION}.amazonaws.com")
}

/// One KMS call (`awsJson1_1`, `TrentService.<Operation>`) that must answer 200.
async fn kms(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(emulator, "kms", "TrentService", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: KMS {operation} answered {status}: {answered}"
    );
    answered
}

/// The statement every key policy carries beside the row's own: the account root's
/// authority over its own key, which is what `CreateKey` writes and what the lockout safety
/// check of `PutKeyPolicy` requires of a replacement.
fn root_statement() -> Value {
    json!({
        "Sid": "Enable IAM policies",
        "Effect": "Allow",
        "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A key policy document carrying the root statement and whatever `statements` adds.
fn key_policy(statements: Vec<Value>) -> String {
    let mut all = vec![root_statement()];
    all.extend(statements);
    json!({ "Version": "2012-10-17", "Id": "key-default", "Statement": all }).to_string()
}

/// One statement about a service principal's use of the key: `effect` of `actions` for
/// `principal`, scoped by `condition` when the row states one.
fn about_service(
    effect: &str,
    principal: &str,
    actions: Value,
    condition: Option<Value>,
) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Principal": { "Service": principal },
        "Action": actions,
        "Resource": "*",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// The statement an event bus key needs: this service, the actions a bus uses, unscoped.
fn admitting_events() -> Value {
    about_service("Allow", SERVICE_PRINCIPAL, json!(BUS_KEY_ACTIONS), None)
}

/// A customer managed key whose whole policy is `statements` beside the root statement,
/// answered as its ARN.
async fn given_key(emulator: &Emulator, case: &str, statements: Vec<Value>) -> String {
    let created = kms(emulator, case, "CreateKey", &json!({})).await;
    let arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no KeyMetadata.Arn in {created}"))
        .to_string();
    kms(
        emulator,
        case,
        "PutKeyPolicy",
        &json!({
            "KeyId": arn,
            "PolicyName": "default",
            "Policy": key_policy(statements),
        }),
    )
    .await;
    arn
}

/// The sentence a configuration whose key withholds the use is refused with: the call
/// EventBridge could not make, and KMS's own reason for a denial about a key it can report
/// nothing further about — one clause for every way a key policy can withhold a service
/// principal's use, which is why a policy that states nothing and one that denies
/// explicitly read alike.
fn denied_message(key: &str) -> String {
    format!(
        "Access to call {BUS_KEY_ACTION} is denied. Reason: {SERVICE_PRINCIPAL} is not \
         authorized to perform: {BUS_KEY_ACTION} on {key} because the resource does not exist \
         in this Region, no resource-based policies allow access, or a resource-based policy \
         explicitly denies access"
    )
}

/// What key a row configures its bus with.
enum Key {
    /// A key this account holds, carrying the statements the row states beside the root's.
    Own(Vec<Value>),
    /// A key ARN this account holds no key for, which is evidence of nothing.
    Unknown,
}

/// One row of the table.
struct Row {
    case: &'static str,
    key: Key,
    /// Whether the configuration is refused.
    refused: bool,
}

/// The two writers of a bus's `KmsKeyIdentifier`, which is where the rule holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Create,
    Update,
}

impl Writer {
    fn operation(self) -> &'static str {
        match self {
            Self::Create => "CreateEventBus",
            Self::Update => "UpdateEventBus",
        }
    }
}

/// The rows, built once and used by each writer: each carries the statements its key's
/// policy holds and the verdict the configuration earns.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a key whose policy admits this service the actions a bus needs",
            key: Key::Own(vec![admitting_events()]),
            refused: false,
        },
        Row {
            case: "a key carrying only the account root's own authority",
            key: Key::Own(Vec::new()),
            refused: true,
        },
        Row {
            case: "a key whose statement names another service",
            key: Key::Own(vec![about_service(
                "Allow",
                OTHER_SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTIONS),
                None,
            )]),
            refused: true,
        },
        Row {
            case: "a key admitting this service only the decryption",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!("kms:Decrypt"),
                None,
            )]),
            refused: true,
        },
        Row {
            case: "a key admitting this service only the data key a published event needs",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTION),
                None,
            )]),
            refused: false,
        },
        Row {
            case: "a key admitting this service a wildcard over the KMS namespace",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!("kms:*"),
                None,
            )]),
            refused: false,
        },
        Row {
            case: "a key whose policy denies this service beside the allow",
            key: Key::Own(vec![
                admitting_events(),
                about_service("Deny", SERVICE_PRINCIPAL, json!(BUS_KEY_ACTIONS), None),
            ]),
            refused: true,
        },
        Row {
            case: "a key scoped to uses arriving through this service",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTIONS),
                Some(json!({ "StringEquals": { VIA_SERVICE_KEY: via_service("events") } })),
            )]),
            refused: false,
        },
        Row {
            case: "a key scoped to uses arriving through another service",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTIONS),
                Some(json!({ "StringEquals": { VIA_SERVICE_KEY: via_service("sqs") } })),
            )]),
            refused: true,
        },
        Row {
            case: "a key scoped to this bus by its encryption context",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTIONS),
                Some(json!({ "StringEquals": { EVENT_BUS_CONTEXT_KEY: bus_arn(BUS) } })),
            )]),
            refused: false,
        },
        Row {
            case: "a key scoped to another bus by its encryption context",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTIONS),
                Some(json!({ "StringEquals": { EVENT_BUS_CONTEXT_KEY: bus_arn(OTHER_BUS) } })),
            )]),
            refused: true,
        },
        Row {
            case: "a key scoped to this account's own resources",
            key: Key::Own(vec![about_service(
                "Allow",
                SERVICE_PRINCIPAL,
                json!(BUS_KEY_ACTIONS),
                Some(json!({ "StringEquals": { SOURCE_ACCOUNT_KEY: account() } })),
            )]),
            refused: false,
        },
        Row {
            case: "a key this account holds none of, which is evidence of nothing",
            key: Key::Unknown,
            refused: false,
        },
    ]
}

/// A bus is configured on a customer managed key only where that key's own policy lets
/// this service use it — asserted for every writer of a bus's key, over a table of key
/// policies.
#[tokio::test]
async fn a_bus_is_configured_on_a_key_only_where_its_policy_admits_this_service() {
    for writer in [Writer::Create, Writer::Update] {
        for row in rows() {
            let case = format!("{} with {}", writer.operation(), row.case);
            let case = case.as_str();
            let emulator = emulator();
            let key = match row.key {
                Key::Own(statements) => given_key(&emulator, case, statements).await,
                Key::Unknown => format!(
                    "arn:aws:kms:{REGION}:{}:key/00000000-0000-0000-0000-000000000000",
                    account()
                ),
            };

            // The bus the update leg writes exists already and is on the Amazon Web
            // Services owned key, so a refused update has a bus to read the absence of a
            // key off.
            if writer == Writer::Update {
                let (status, created) =
                    events(&emulator, "CreateEventBus", &json!({ "Name": BUS })).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the unencrypted bus answered {status}: {created}"
                );
            }

            let request = json!({ "Name": BUS, "KmsKeyIdentifier": key });
            let (status, answered) = events(&emulator, writer.operation(), &request).await;
            let reported = described_key(&emulator, case).await;
            if row.refused {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the configuration must be refused, got {status}: {answered}"
                );
                assert_eq!(
                    answered["__type"],
                    json!("AccessDeniedException"),
                    "case {case:?}: expected AccessDeniedException, got {answered}"
                );
                let expected = denied_message(&key);
                assert_eq!(
                    answered["message"],
                    json!(expected),
                    "case {case:?}: expected the message {expected:?}, got {answered}"
                );
                assert!(
                    reported.is_none(),
                    "case {case:?}: a refused configuration must leave the bus on no customer \
                     managed key, got {reported:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the configuration must be accepted, got {status}: {answered}"
                );
                assert_eq!(
                    answered["KmsKeyIdentifier"],
                    json!(key),
                    "case {case:?}: the answer must report the key stored, got {answered}"
                );
                assert_eq!(
                    reported.as_deref(),
                    Some(key.as_str()),
                    "case {case:?}: the bus must read back on that key, got {reported:?}"
                );
            }
        }
    }
}

/// The key [`BUS`] reports itself on, or `None` for a bus on the Amazon Web Services owned
/// key — and for a bus a refused create never brought into being.
async fn described_key(emulator: &Emulator, case: &str) -> Option<String> {
    let (status, body) = events(emulator, "DescribeEventBus", &json!({ "Name": BUS })).await;
    if status != 200 {
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: expected the bus to be missing, got {status}: {body}"
        );
        return None;
    }
    body["KmsKeyIdentifier"].as_str().map(str::to_string)
}

/// Granting the key's use afterwards lets the IDENTICAL configuration through, with
/// nothing about the bus or the request changed in between — so the decision is a fact of
/// the key's policy and not of the call.
#[tokio::test]
async fn writing_the_key_policy_statement_lets_the_same_configuration_through() {
    let case = "a key policy that admits this service only after it is written";
    let emulator = emulator();
    let key = given_key(&emulator, case, Vec::new()).await;
    let request = json!({ "Name": BUS, "KmsKeyIdentifier": key });

    let (status, refused) = events(&emulator, "CreateEventBus", &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the first configuration must be refused, got {status}: {refused}"
    );
    assert!(
        described_key(&emulator, case).await.is_none(),
        "case {case:?}: the refused create must have created no bus"
    );

    kms(
        &emulator,
        case,
        "PutKeyPolicy",
        &json!({
            "KeyId": key,
            "PolicyName": "default",
            "Policy": key_policy(vec![admitting_events()]),
        }),
    )
    .await;

    let (status, created) = events(&emulator, "CreateEventBus", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the identical configuration must be accepted once the key admits \
         this service, got {status}: {created}"
    );
    let reported = described_key(&emulator, case).await;
    assert_eq!(
        reported.as_deref(),
        Some(key.as_str()),
        "case {case:?}: the bus must read back on that key, got {reported:?}"
    );
}
