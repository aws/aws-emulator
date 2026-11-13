// SPDX-License-Identifier: CC0-1.0

//! The refusal this control plane answers when a request's `configId` names no enforced
//! guardrail configuration of the account, driven end to end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! Amazon Bedrock reports an account-enforced guardrail configuration a `configId`
//! addresses but the account does not hold with ONE fixed sentence, and the sentence
//! carries no part of the request: not the submitted id, not the account, not the Region.
//! Both operations addressed by that id answer alike, with the modelled
//! `ResourceNotFoundException` at `@httpError(404)`
//! (`smithy_query(query="operation",operation="DeleteEnforcedGuardrailConfiguration,PutEnforcedGuardrailConfiguration")`):
//! the resource and the error class are the same whether the `configId` travels in the
//! path (`DELETE /enforcedGuardrailsConfiguration/{configId}`) or in the body (the put's
//! optional `configId` member), so it is the refusal of the shared configuration LOOKUP
//! rather than of one operation.
//!
//! # What the table below generalises over
//!
//! The rows differ along what the rule spans: the FORM of the submitted id over the set
//! `AccountEnforcedGuardrailConfigurationId` admits (`^[a-z0-9]+$`, no length bound) — one
//! character, digits only, letters only, a long one, the installed id with a digit
//! appended, and the id of a configuration that was installed and then DELETED — and the
//! STATE of the account when the id is submitted: holding no configuration at all, and
//! holding one under a different id. Each row is driven through both operations, and each
//! leaves whatever the account holds exactly as it was.

use serde_json::{json, Value};

use crate::support::{emulator, Emulator, Reply, ACCOUNT, REGION};

/// The URI `PutEnforcedGuardrailConfiguration` and `ListEnforcedGuardrailsConfiguration`
/// are bound to; the delete appends `/{configId}` to it.
const CONFIGURATION_URI: &str = "/enforcedGuardrailsConfiguration";

/// The sentence Amazon Bedrock answers for an enforced guardrail configuration a `configId`
/// does not address, written out here rather than read off the crate so the recorded text is
/// pinned by this suite independently of the constant that produces it.
const NOT_FOUND: &str = "No default guardrail configuration found for the specified config identifier.";

/// `PUT /enforcedGuardrailsConfiguration`, naming the guardrail revision `identifier`/`1`
/// and — when the row carries one — the `configId` the put addresses.
async fn put(emu: &Emulator, identifier: &str, config_id: Option<&str>) -> Reply {
    let mut body = json!({
        "guardrailInferenceConfig": {
            "guardrailIdentifier": identifier,
            "guardrailVersion": "1",
        },
    });
    if let Some(config_id) = config_id {
        body["configId"] = json!(config_id);
    }
    emu.call("PUT", CONFIGURATION_URI, Some(body)).await
}

/// `DELETE /enforcedGuardrailsConfiguration/{configId}`.
async fn delete(emu: &Emulator, config_id: &str) -> Reply {
    emu.call("DELETE", &format!("{CONFIGURATION_URI}/{config_id}"), None)
        .await
}

/// The rows `ListEnforcedGuardrailsConfiguration` reports.
async fn rows(emu: &Emulator) -> Vec<Value> {
    let reply = emu.call("GET", CONFIGURATION_URI, None).await;
    assert_eq!(
        reply.status, 200,
        "the listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .get("guardrailsConfig")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every refusal this suite checks: 404, the modelled code, the fixed sentence, and nothing
/// of the request inside it.
fn assert_not_found(case: &str, submitted: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?}",
        reply.status,
        reply.body
    );
    let message = reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        message, NOT_FOUND,
        "case {case:?}: expected the message {NOT_FOUND:?}, got {message:?}"
    );
    // Nothing of the request is reflected back into the sentence. A one- or two-character
    // id is a substring of ordinary English, so the echo check is taken over the ids long
    // enough for a match to mean the id was interpolated; the equality above pins every
    // row's wording regardless.
    let echoed: Vec<(&str, &str)> = [("the account", ACCOUNT), ("the Region", REGION)]
        .into_iter()
        .chain((submitted.len() >= 4).then_some(("the submitted configId", submitted)))
        .collect();
    for (what, part) in echoed {
        assert!(
            !message.contains(part),
            "case {case:?}: the sentence must carry no part of the request — {what} \
             {part:?} — got {message:?}"
        );
    }
}

/// A guardrail with one numbered version, which is the revision every put here enforces.
async fn guardrail_with_a_version(emu: &Emulator, name: &str) -> String {
    let id = emu.create_named(name).await;
    let cut = emu.create_version(&id, json!({})).await;
    assert_eq!(
        cut.status, 202,
        "case {name:?}: CreateGuardrailVersion must answer 202, got {} {:?}",
        cut.status, cut.body
    );
    id
}

/// The ids a row submits, over the set `AccountEnforcedGuardrailConfigurationId`
/// (`^[a-z0-9]+$`) admits: no length is published for it, so the forms span one character
/// to a long one, and digits-only to letters-only.
fn unheld_ids(installed: &str) -> Vec<(String, String)> {
    vec![
        (
            "one character, the shortest the pattern admits".to_string(),
            "a".to_string(),
        ),
        ("digits only".to_string(), "123456789012".to_string()),
        ("letters only".to_string(), "zzzzzzzzzzzz".to_string()),
        ("a 64-character id".to_string(), "c0".repeat(32)),
        (
            "the installed id with a digit appended".to_string(),
            format!("{installed}0"),
        ),
    ]
}

#[tokio::test]
async fn an_unheld_config_id_is_refused_in_one_sentence_by_both_operations() {
    let emu = emulator();
    let guardrail = guardrail_with_a_version(&emu, "unheld-config-id").await;

    // The account holds NOTHING yet, so every id addresses no configuration — whichever
    // operation submits it.
    for (form, config_id) in unheld_ids("0000000000") {
        for (operation, reply) in [
            ("DeleteEnforcedGuardrailConfiguration", delete(&emu, &config_id).await),
            (
                "PutEnforcedGuardrailConfiguration",
                put(&emu, &guardrail, Some(&config_id)).await,
            ),
        ] {
            assert_not_found(
                &format!("{operation} / nothing installed / {form}"),
                &config_id,
                &reply,
            );
        }
        assert_eq!(
            rows(&emu).await,
            Vec::<Value>::new(),
            "case {form:?}: a refused call must install nothing"
        );
    }

    // With a configuration INSTALLED, an id that is not its own still addresses nothing,
    // and the installed row is left exactly as it was.
    let installed = put(&emu, &guardrail, None).await;
    assert_eq!(
        installed.status, 200,
        "case setup: the put that installs the configuration must answer 200, got {} {:?}",
        installed.status, installed.body
    );
    let config_id = installed.text("configId");
    let baseline = rows(&emu).await;
    assert_eq!(
        baseline.len(),
        1,
        "case setup: the account holds exactly one configuration, got {baseline:?}"
    );

    for (form, unheld) in unheld_ids(&config_id) {
        for (operation, reply) in [
            ("DeleteEnforcedGuardrailConfiguration", delete(&emu, &unheld).await),
            (
                "PutEnforcedGuardrailConfiguration",
                put(&emu, &guardrail, Some(&unheld)).await,
            ),
        ] {
            assert_not_found(
                &format!("{operation} / one installed / {form}"),
                &unheld,
                &reply,
            );
        }
        assert_eq!(
            rows(&emu).await,
            baseline,
            "case {form:?}: a refused call must leave the installed configuration as it was"
        );
    }

    // The id of a configuration that WAS installed and then deleted is the same miss: the
    // repeated `@idempotent` delete, and a put that addresses it, answer the one sentence.
    let removed = delete(&emu, &config_id).await;
    assert_eq!(
        removed.status, 200,
        "case setup: the delete of the installed id must answer 200, got {} {:?}",
        removed.status, removed.body
    );
    for (operation, reply) in [
        ("DeleteEnforcedGuardrailConfiguration", delete(&emu, &config_id).await),
        (
            "PutEnforcedGuardrailConfiguration",
            put(&emu, &guardrail, Some(&config_id)).await,
        ),
    ] {
        assert_not_found(
            &format!("{operation} / a deleted configuration's id"),
            &config_id,
            &reply,
        );
    }
    assert_eq!(
        rows(&emu).await,
        Vec::<Value>::new(),
        "case \"a deleted configuration's id\": nothing is installed once it is deleted"
    );
}
