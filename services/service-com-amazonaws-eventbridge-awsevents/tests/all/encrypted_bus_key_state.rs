// SPDX-License-Identifier: CC0-1.0

//! The STATE of the customer managed key an event bus is encrypted with decides
//! whether a `PutEvents` entry onto that bus is published at all, end to end over
//! the wire through the real `MultiServiceRouter` with KMS and Amazon SQS mounted on
//! the same engine — so the key is read where its own handlers wrote it and the
//! outcome is asserted where a client sees it: in `FailedEntries`, and on the rule's
//! target.
//!
//! The RULE asserted here: "if you do not specify a customer managed key,
//! EventBridge uses an Amazon Web Services owned key"
//! (`smithy_query(query="operation",operation="CreateEventBus")`'s
//! `KmsKeyIdentifier`), so a bus that DOES name one has every event published onto it
//! encrypted under that key and the key's own state decides the publication: while
//! the key cannot be used in a cryptographic operation — it is disabled, or awaiting
//! deletion — each entry onto that bus fails on its OWN row with
//! `KMSEncryptionException` carrying the sentence KMS words for that key state, and
//! nothing of it is routed: no rule of the bus sees it and no target is handed it.
//! The key is a kill switch over the bus, so putting it back into service lets the
//! next publication through with nothing else changed, and an entry onto a bus on the
//! Amazon Web Services owned key is never affected.
//!
//! The table spans the class rather than the one recorded case: the rows differ along
//! the key state the bus's key is in (usable, disabled, awaiting deletion), along
//! whether the bus names a key at all, and along whether the key it names is one this
//! account holds — and the sequence test drives the same bus through disable and
//! enable, which is the kill switch itself. Every expected sentence is composed in
//! the test from the key's own ARN rather than imported from the implementation.
//!
//! The harness (the router with EventBridge, Amazon SQS, KMS, IAM and CloudWatch on
//! one engine, and the queue fixture) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::SERVICE_PRINCIPAL;

use crate::target_dispatch::{
    drain, emulator, events, given_queue, rpc, Emulator, DETAIL_TYPE, SOURCE,
};

/// The per-entry code EventBridge answers for an event it could not encrypt under the
/// key of the bus the entry names, stated here as the literal a client branches on.
const KMS_ENCRYPTION_EXCEPTION: &str = "KMSEncryptionException";

/// The rule every bus below carries, and the id of its one target.
const RULE: &str = "encrypted-bus-rule";
const TARGET_ID: &str = "queue";

/// One KMS call (`awsJson1_1`, `TrentService.<Operation>`) that must answer 200.
async fn kms(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = rpc(emulator, "kms", "TrentService", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: KMS {operation} answered {status}: {answered}"
    );
    answered
}

/// A customer managed key this service may use for an event bus, answered as its ARN.
///
/// The key carries the key-policy statement an event bus key needs: the account root's
/// own authority over the key — which is what `CreateKey` writes and what the lockout
/// safety check of `PutKeyPolicy` requires of a replacement — and the use this service
/// makes of the key as its OWN service principal. Without that second statement the bus
/// could not be configured on the key at all (the configuration is refused at the call
/// that asks for it, which is what `tests/all/event_bus_key_policy.rs` asserts), and what
/// these tests vary is the key's STATE: nothing about its policy may withhold the use.
async fn given_key(emulator: &Emulator, case: &str) -> String {
    let created = kms(emulator, case, "CreateKey", &json!({})).await;
    let arn = created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no KeyMetadata.Arn in {created}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "Enable IAM policies",
                "Effect": "Allow",
                "Principal": {
                    "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
                },
                "Action": "kms:*",
                "Resource": "*",
            },
            {
                "Sid": "Allow EventBridge to use the key for this account's buses",
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": ["kms:DescribeKey", "kms:GenerateDataKey", "kms:Decrypt"],
                "Resource": "*",
            },
        ],
    })
    .to_string();
    kms(
        emulator,
        case,
        "PutKeyPolicy",
        &json!({ "KeyId": arn, "PolicyName": "default", "Policy": policy }),
    )
    .await;
    arn
}

/// An event bus called `bus`, encrypted with `key` when the row names one, carrying a
/// rule that matches the published event and delivers it to `target_arn`.
async fn given_encrypted_bus(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    key: Option<&str>,
    target_arn: &str,
) {
    let mut request = json!({ "Name": bus });
    if let Some(key) = key {
        request["KmsKeyIdentifier"] = json!(key);
    }
    let (status, created) = events(emulator, "CreateEventBus", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status}: {created}"
    );
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, written) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventBusName": bus,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule answered {status}: {written}"
    );
    let (status, targeted) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": bus,
            "Targets": [{ "Id": TARGET_ID, "Arn": target_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status}: {targeted}"
    );
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {targeted}"
    );
}

/// Publish one matching event onto `bus`, and answer the whole `PutEvents` body.
async fn publish_onto(emulator: &Emulator, case: &str, bus: &str) -> Value {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "orderId": "A-1" }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status}: {body}"
    );
    body
}

/// What a row does to the bus's key before publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyCondition {
    /// The key is left as `CreateKey` made it, carrying the statement that admits this
    /// service ([`given_key`]).
    Usable,
    /// `DisableKey` was called on it.
    Disabled,
    /// `ScheduleKeyDeletion` was called on it.
    AwaitingDeletion,
    /// The bus names no key at all.
    NoKey,
    /// The bus names a key ARN this account holds no key for.
    UnknownKey,
}

/// One row: the case, what the bus's key is, and — when the publication is refused —
/// the `KeyState` the refusal's sentence names as KMS words it.
struct Row {
    case: &'static str,
    condition: KeyCondition,
    /// `None` is an entry that must be published and delivered.
    refused_state: Option<&'static str>,
}

/// An entry onto an encrypted bus is published only while that bus's key can be used,
/// over a table of key conditions.
#[tokio::test]
async fn an_entry_onto_an_encrypted_bus_is_refused_while_its_key_cannot_be_used() {
    let rows = [
        Row {
            case: "a bus on a key that is in service",
            condition: KeyCondition::Usable,
            refused_state: None,
        },
        Row {
            case: "a bus whose key was disabled",
            condition: KeyCondition::Disabled,
            refused_state: Some("disabled"),
        },
        Row {
            case: "a bus whose key is awaiting deletion",
            condition: KeyCondition::AwaitingDeletion,
            refused_state: Some("pending deletion"),
        },
        Row {
            case: "a bus on the Amazon Web Services owned key",
            condition: KeyCondition::NoKey,
            refused_state: None,
        },
        Row {
            case: "a bus naming a key this account does not hold",
            condition: KeyCondition::UnknownKey,
            refused_state: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (queue_url, queue_arn) =
            given_queue(&emulator, &format!("encrypted-bus-queue-{index}")).await;
        let key = match row.condition {
            KeyCondition::NoKey => None,
            KeyCondition::UnknownKey => Some(format!(
                "arn:aws:kms:{}:{}:key/00000000-0000-0000-0000-000000000000",
                crate::target_dispatch::REGION,
                engine::emulator_account_id()
            )),
            _ => Some(given_key(&emulator, case).await),
        };
        let bus = format!("encrypted-bus-{index}");
        given_encrypted_bus(&emulator, case, &bus, key.as_deref(), &queue_arn).await;
        match row.condition {
            KeyCondition::Disabled => {
                kms(
                    &emulator,
                    case,
                    "DisableKey",
                    &json!({ "KeyId": key.clone().unwrap_or_default() }),
                )
                .await;
            }
            KeyCondition::AwaitingDeletion => {
                kms(
                    &emulator,
                    case,
                    "ScheduleKeyDeletion",
                    &json!({
                        "KeyId": key.clone().unwrap_or_default(),
                        "PendingWindowInDays": 7,
                    }),
                )
                .await;
            }
            _ => {}
        }

        let published = publish_onto(&emulator, case, &bus).await;
        let entry = &published["Entries"][0];
        let delivered = drain(&emulator, &queue_url).await;
        match row.refused_state {
            None => {
                assert_eq!(
                    published["FailedEntryCount"],
                    json!(0),
                    "case {case:?}: the entry must be published, got {published}"
                );
                assert!(
                    entry["EventId"].as_str().is_some_and(|id| !id.is_empty()),
                    "case {case:?}: a published entry carries an EventId, got {published}"
                );
                assert_eq!(
                    delivered.len(),
                    1,
                    "case {case:?}: the rule's target must receive the event, got {delivered:?}"
                );
            }
            Some(state) => {
                assert_eq!(
                    published["FailedEntryCount"],
                    json!(1),
                    "case {case:?}: the entry must fail on its own row, got {published}"
                );
                assert_eq!(
                    entry["ErrorCode"],
                    json!(KMS_ENCRYPTION_EXCEPTION),
                    "case {case:?}: expected {KMS_ENCRYPTION_EXCEPTION}, got {published}"
                );
                let expected = format!(
                    "{} is {state}.",
                    key.clone().unwrap_or_default()
                );
                assert_eq!(
                    entry["ErrorMessage"],
                    json!(expected),
                    "case {case:?}: expected the message {expected:?}, got {published}"
                );
                assert!(
                    entry["EventId"].as_str().is_none_or(str::is_empty),
                    "case {case:?}: a refused entry carries no EventId, got {published}"
                );
                assert!(
                    delivered.is_empty(),
                    "case {case:?}: nothing of a refused entry may reach the target, got \
                     {delivered:?}"
                );
            }
        }
    }
}

/// The key is a KILL SWITCH over the bus: disabling it stops the publications and
/// enabling it lets them resume, with nothing else about the bus, the rule or the
/// target changed in between.
#[tokio::test]
async fn disabling_and_enabling_the_bus_key_stops_and_resumes_its_publications() {
    let case = "a bus key disabled and then enabled again";
    let emulator = emulator();
    let (queue_url, queue_arn) = given_queue(&emulator, "kill-switch-queue").await;
    let key = given_key(&emulator, case).await;
    let bus = "kill-switch-bus";
    given_encrypted_bus(&emulator, case, bus, Some(&key), &queue_arn).await;

    let published = publish_onto(&emulator, case, bus).await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "case {case:?}: the first publication must go through, got {published}"
    );
    let delivered = drain(&emulator, &queue_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: the target must receive the first event, got {delivered:?}"
    );

    kms(&emulator, case, "DisableKey", &json!({ "KeyId": key })).await;
    let refused = publish_onto(&emulator, case, bus).await;
    assert_eq!(
        refused["FailedEntryCount"],
        json!(1),
        "case {case:?}: a publication onto a bus whose key is disabled must fail, got {refused}"
    );
    assert_eq!(
        refused["Entries"][0]["ErrorMessage"],
        json!(format!("{key} is disabled.")),
        "case {case:?}: expected KMS's own sentence for the key state, got {refused}"
    );
    let withheld = drain(&emulator, &queue_url).await;
    assert!(
        withheld.is_empty(),
        "case {case:?}: nothing must reach the target while the key is disabled, got \
         {withheld:?}"
    );

    kms(&emulator, case, "EnableKey", &json!({ "KeyId": key })).await;
    let resumed = publish_onto(&emulator, case, bus).await;
    assert_eq!(
        resumed["FailedEntryCount"],
        json!(0),
        "case {case:?}: publications must resume once the key is enabled, got {resumed}"
    );
    let again = drain(&emulator, &queue_url).await;
    assert_eq!(
        again.len(),
        1,
        "case {case:?}: the target must receive the event again, got {again:?}"
    );
}

/// The refusal is about the BUS the entry names, not about the request: in one batch,
/// an entry onto a bus whose key is disabled fails on its own row while an entry onto
/// a bus on the Amazon Web Services owned key is published and delivered.
#[tokio::test]
async fn one_batch_refuses_only_the_entries_onto_the_bus_whose_key_is_disabled() {
    let case = "one batch spanning an encrypted bus and an unencrypted one";
    let emulator = emulator();
    let (encrypted_url, encrypted_queue) = given_queue(&emulator, "batch-encrypted-queue").await;
    let (plain_url, plain_queue) = given_queue(&emulator, "batch-plain-queue").await;
    let key = given_key(&emulator, case).await;
    given_encrypted_bus(
        &emulator,
        case,
        "batch-encrypted-bus",
        Some(&key),
        &encrypted_queue,
    )
    .await;
    given_encrypted_bus(&emulator, case, "batch-plain-bus", None, &plain_queue).await;
    kms(&emulator, case, "DisableKey", &json!({ "KeyId": key })).await;

    let detail = json!({ "orderId": "A-1" }).to_string();
    let (status, body) = events(
        &emulator,
        "PutEvents",
        &json!({
            "Entries": [
                {
                    "EventBusName": "batch-encrypted-bus",
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": detail,
                },
                {
                    "EventBusName": "batch-plain-bus",
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": detail,
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status}: {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(1),
        "case {case:?}: exactly the encrypted bus's entry must fail, got {body}"
    );
    // "For each record, the index of the response element is the same as the index in
    // the request array": the refusal is the FIRST row and the second is published.
    assert_eq!(
        body["Entries"][0]["ErrorCode"],
        json!(KMS_ENCRYPTION_EXCEPTION),
        "case {case:?}: the first entry must carry the encryption refusal, got {body}"
    );
    assert!(
        body["Entries"][1]["EventId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "case {case:?}: the second entry must be published, got {body}"
    );
    let encrypted_delivered = drain(&emulator, &encrypted_url).await;
    assert!(
        encrypted_delivered.is_empty(),
        "case {case:?}: the encrypted bus's target must receive nothing, got \
         {encrypted_delivered:?}"
    );
    let plain_delivered = drain(&emulator, &plain_url).await;
    assert_eq!(
        plain_delivered.len(),
        1,
        "case {case:?}: the other bus's target must receive its event, got {plain_delivered:?}"
    );
}
