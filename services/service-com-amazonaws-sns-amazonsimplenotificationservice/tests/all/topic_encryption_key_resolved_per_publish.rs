// SPDX-License-Identifier: CC0-1.0

//! The key a topic encrypts its messages under is resolved, and its condition read, on EVERY
//! write of a message to that topic — driven over the wire through the real
//! `MultiServiceRouter` with KMS mounted on the same engine, so what a client sees is what
//! this suite asserts.
//!
//! The RULE asserted here: a topic carrying `KmsMasterKeyId` has every message published to it
//! encrypted under the key that identifier names AT THE MOMENT OF THAT PUBLISH, so the key's
//! condition then — and the key the identifier resolves to then — decide that one call and
//! nothing else. A refusal is therefore not a state the topic is left in and an acceptance is
//! not a verdict the topic keeps: while the key cannot be used in a cryptographic operation
//! every publish is refused in the class KMS answered for that state, and while it can be used
//! every publish is accepted with a message id of its own, however many times the key's
//! condition changes between them.
//!
//! The rows differ along what the rule generalises over: how many messages are written per leg
//! and in which order the legs come (enabled, disabled, enabled again, disabled again), the
//! FORM the topic names its key in (a key id, a key ARN, an alias name), which operation does
//! the writing (`Publish` and `PublishBatch`, both of which write a message to the topic and
//! both of which model the `KMS*` family), and — for a topic on an alias — WHICH key the alias
//! targets when the publish arrives.
//!
//! The single-shot legs of the same rule (one publish per key condition, each condition a row)
//! are `topic_encryption_key_state.rs`; what this suite adds is the repetition and the
//! re-resolution.

use std::collections::HashSet;

use serde_json::json;

use crate::sqs_subscription_delivery::{element, emulator, kms, sns_call, Emulator};
use crate::topic_encryption_key_state::{
    assert_relayed, given_key, given_topic, set_key_enabled, RELAY_TAIL,
};
use crate::topics_subscriptions_publish::xml_unescape;

/// The awsQuery error code a publish under a merely disabled key is refused with: "`KMSDisabled`
/// (HTTP 400) — The request was rejected because the specified Amazon Web Services KMS key
/// isn't enabled" (`smithy_query(query="operation",operation="Publish")`).
const DISABLED_CODE: &str = "KMSDisabled";

/// KMS's own error code behind that refusal, which the relayed sentence reports.
const KMS_DISABLED_CODE: &str = "DisabledException";

/// The two operations that write a message to a topic, each of which resolves the topic's key
/// before it accepts anything.
const WRITERS: &[&str] = &["Publish", "PublishBatch"];

/// Every `<MessageId>` an awsQuery answer reports, in the order the answer lists them.
fn message_ids(body: &str) -> Vec<String> {
    body.split("<MessageId>")
        .skip(1)
        .map(|rest| {
            rest.split("</MessageId>")
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Write ONE message to `topic` through the operation `writer` names: its HTTP status, the
/// awsQuery error code (`""` on a success envelope), the message ids the answer reports and the
/// refusal sentence it carries (empty on a success envelope).
async fn write_message(
    emulator: &Emulator,
    writer: &str,
    topic: &str,
    body: &str,
) -> (u16, String, Vec<String>, String) {
    let params: Vec<(&str, &str)> = match writer {
        "Publish" => vec![("TopicArn", topic), ("Message", body)],
        "PublishBatch" => vec![
            ("TopicArn", topic),
            ("PublishBatchRequestEntries.member.1.Id", "only"),
            ("PublishBatchRequestEntries.member.1.Message", body),
        ],
        other => panic!("no writer called {other:?}"),
    };
    let (status, text) = sns_call(emulator, writer, &params).await;
    let (code, sentence) = if status == 200 {
        (String::new(), String::new())
    } else {
        (
            xml_unescape(&element(&text, "Code")),
            xml_unescape(&element(&text, "Message")),
        )
    };
    (status, code, message_ids(&text), sentence)
}

/// One leg of a case: the key's condition, how it reads, and how many messages are written
/// while the key is in it. Several messages per leg is what tells a condition read per publish
/// apart from one read once and remembered.
struct Leg {
    case: &'static str,
    enabled: bool,
    writes: usize,
}

/// The legs every case below runs, in order: the key's condition changes under a topic that is
/// never touched in between, and the publishes on each side of each change are what the rule is
/// asserted on.
const LEGS: &[Leg] = &[
    Leg {
        case: "while the key is enabled",
        enabled: true,
        writes: 3,
    },
    Leg {
        case: "while the key is disabled",
        enabled: false,
        writes: 4,
    },
    Leg {
        case: "after the key is enabled again",
        enabled: true,
        writes: 2,
    },
    Leg {
        case: "after the key is disabled again",
        enabled: false,
        writes: 3,
    },
    Leg {
        case: "after the key is enabled once more",
        enabled: true,
        writes: 2,
    },
];

#[tokio::test]
async fn every_write_to_an_encrypted_topic_is_judged_by_the_key_condition_of_that_write() {
    let emulator = emulator();

    // The FORM the topic names its key in: KMS resolves a key id, a key ARN and an alias name
    // to the same key, so each must follow that key's condition identically.
    let forms: &[&str] = &["the key id", "the key ARN", "an alias name"];

    for (index, form) in forms.iter().enumerate() {
        for writer in WRITERS {
            let (key_id, key_arn) = given_key(&emulator).await;
            let alias = format!("alias/per-publish-{index}-{writer}");
            kms(
                &emulator,
                "CreateAlias",
                &json!({"AliasName": alias, "TargetKeyId": key_id}),
            )
            .await;
            let named = match *form {
                "the key id" => key_id.clone(),
                "the key ARN" => key_arn.clone(),
                "an alias name" => alias.clone(),
                other => panic!("no identifier form for {other:?}"),
            };
            let topic = given_topic(
                &emulator,
                &format!("per-publish-{index}-{}", writer.to_lowercase()),
                Some(&named),
            )
            .await;

            // Every message id this case was answered with: an accepted write mints one of its
            // own, so a repeated acceptance is never one answer served twice.
            let mut minted: HashSet<String> = HashSet::new();

            for leg in LEGS {
                set_key_enabled(&emulator, &key_id, leg.enabled).await;
                for attempt in 1..=leg.writes {
                    let case = format!("{writer} {attempt} of {} {}", leg.writes, leg.case);
                    let (status, code, ids, sentence) =
                        write_message(&emulator, writer, &topic, &format!("message {attempt}"))
                            .await;
                    if leg.enabled {
                        assert_eq!(
                            (status, code.as_str()),
                            (200, ""),
                            "case {case:?} naming {form:?}: a write under a usable key must be \
                             accepted, got {status} / {code} / {sentence:?}"
                        );
                        assert_eq!(
                            ids.len(),
                            1,
                            "case {case:?} naming {form:?}: an accepted write must report one \
                             MessageId, got {ids:?}"
                        );
                        assert!(
                            minted.insert(ids[0].clone()),
                            "case {case:?} naming {form:?}: every accepted write mints its own \
                             MessageId, got the already-answered {:?}",
                            ids[0]
                        );
                    } else {
                        assert_eq!(
                            (status, code.as_str()),
                            (400, DISABLED_CODE),
                            "case {case:?} naming {form:?}: a write under a disabled key must be \
                             refused {DISABLED_CODE}, got {status} / {code} / {sentence:?}"
                        );
                        assert!(
                            ids.is_empty(),
                            "case {case:?} naming {form:?}: a refused write publishes nothing, \
                             so it must report no MessageId, got {ids:?}"
                        );
                        assert_relayed(
                            &case,
                            &sentence,
                            &format!("{key_arn} is disabled."),
                            KMS_DISABLED_CODE,
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_topic_on_an_alias_follows_whichever_key_the_alias_targets_at_the_publish() {
    let emulator = emulator();
    let (usable_key_id, usable_key_arn) = given_key(&emulator).await;
    let (disabled_key_id, disabled_key_arn) = given_key(&emulator).await;
    set_key_enabled(&emulator, &disabled_key_id, false).await;

    let alias = "alias/retargeted-topic-key";
    kms(
        &emulator,
        "CreateAlias",
        &json!({"AliasName": alias, "TargetKeyId": usable_key_id}),
    )
    .await;
    let topic = given_topic(&emulator, "topic-on-an-alias", Some(alias)).await;

    // (case, the key the alias targets when the publish arrives, the key ARN the refusal must
    //  name — `None` for a publish that must be accepted)
    let rows: &[(&str, &String, Option<&String>)] = &[
        (
            "the alias targets the enabled key",
            &usable_key_id,
            None,
        ),
        (
            "the alias is retargeted at the disabled key",
            &disabled_key_id,
            Some(&disabled_key_arn),
        ),
        (
            "the alias is retargeted at the enabled key again",
            &usable_key_id,
            None,
        ),
    ];

    for (case, target, refused_by) in rows {
        kms(
            &emulator,
            "UpdateAlias",
            &json!({"AliasName": alias, "TargetKeyId": target}),
        )
        .await;
        for writer in WRITERS {
            let case = format!("{writer} while {case}");
            let (status, code, ids, sentence) =
                write_message(&emulator, writer, &topic, "sealed under the alias").await;
            match refused_by {
                None => {
                    assert_eq!(
                        (status, code.as_str()),
                        (200, ""),
                        "case {case:?}: the alias targets a usable key, so the write must be \
                         accepted, got {status} / {code} / {sentence:?}"
                    );
                    assert_eq!(
                        ids.len(),
                        1,
                        "case {case:?}: an accepted write must report one MessageId, got {ids:?}"
                    );
                }
                Some(key_arn) => {
                    assert_eq!(
                        (status, code.as_str()),
                        (400, DISABLED_CODE),
                        "case {case:?}: the alias targets a disabled key, so the write must be \
                         refused {DISABLED_CODE}, got {status} / {code} / {sentence:?}"
                    );
                    assert!(
                        ids.is_empty(),
                        "case {case:?}: a refused write publishes nothing, so it must report no \
                         MessageId, got {ids:?}"
                    );
                    // The refusal names the key the ALIAS resolved to, which is how a client
                    // learns which key refused its publish.
                    assert_relayed(
                        &case,
                        &sentence,
                        &format!("{key_arn} is disabled."),
                        KMS_DISABLED_CODE,
                    );
                    assert!(
                        !sentence.contains(usable_key_arn.as_str()),
                        "case {case:?}: the refusal must name the key the alias targets now, \
                         got {sentence:?}"
                    );
                    assert!(
                        sentence.ends_with(RELAY_TAIL),
                        "case {case:?}: the refusal relays the KMS call that failed, so it must \
                         end with {RELAY_TAIL:?}, got {sentence:?}"
                    );
                }
            }
        }
    }
}
