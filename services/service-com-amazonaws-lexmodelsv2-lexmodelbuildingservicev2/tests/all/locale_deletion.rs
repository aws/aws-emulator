// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 locale deletion slice: `DeleteBotLocale`
//! and what it takes with the locale.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them. The harness and the
//! bot/locale/intent/slot setup are the earlier suites', reused rather than copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a delete removes the locale the triple names together with every resource defined
//!   in it — its intents, its slot types and the slots of those intents — so nothing of
//!   it is readable afterwards, and the call reports the triple it removed under the
//!   `Deleting` status;
//! * a delete touches only the locale it names: the same locale id of another bot, and
//!   another locale of the same bot, keep everything defined in them, and the id is free
//!   to be created again as a locale holding nothing but the fallback intent;
//! * a triple the store holds no locale under is refused with the error this operation's
//!   model declares, and removes nothing.

use serde_json::Value;
use serde_json::json;
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, text};
use crate::bot_locale::{host_bot, locale_body, locale_uri, locales_uri};
use crate::intents_and_builds::{create_intent, intent_body, intent_uri};
use crate::slot_type::{create_slot_type, slot_type_body, slot_type_uri};
use crate::slots::{slot_body, slot_uri, slots_uri};

/// Everything one locale was populated with, each under the id it was stored as: a
/// resource of the locale has to be unreadable once the locale is gone.
#[derive(Debug, Default)]
struct Populated {
    intent_ids: Vec<String>,
    slot_type_ids: Vec<String>,
    /// Each slot as `(the intent that holds it, its slot id)` — the id alone does not
    /// address a slot.
    slots: Vec<(String, String)>,
}

/// One row of the deletion table: how much the locale holds when it is deleted.
#[derive(Debug)]
struct Case {
    what: &'static str,
    locale_id: &'static str,
    intents: usize,
    slot_types: usize,
    slots_per_intent: usize,
}

/// The table every deletion assertion runs over: rows differ in the locale id, in how
/// many intents the locale holds, in whether it holds slot types at all and in how many
/// slots each of its intents holds — including the locale that holds nothing but the
/// fallback intent Amazon Lex adds as it creates one.
fn deletion_cases() -> Vec<Case> {
    vec![
        Case {
            what: "a locale holding only the fallback intent",
            locale_id: "en_US",
            intents: 0,
            slot_types: 0,
            slots_per_intent: 0,
        },
        Case {
            what: "a locale holding intents and nothing else",
            locale_id: "de_DE",
            intents: 3,
            slot_types: 0,
            slots_per_intent: 0,
        },
        Case {
            what: "a locale holding slot types and nothing else",
            locale_id: "ja_JP",
            intents: 0,
            slot_types: 2,
            slots_per_intent: 0,
        },
        Case {
            what: "a locale holding an intent, a slot type and a slot",
            locale_id: "fr_CA",
            intents: 1,
            slot_types: 1,
            slots_per_intent: 1,
        },
        Case {
            what: "a locale holding several intents with several slots each",
            locale_id: "es_419",
            intents: 2,
            slot_types: 1,
            slots_per_intent: 3,
        },
    ]
}

/// Create one bot with one locale on its `DRAFT` version, populate the locale as the row
/// asks, and answer the bot id and the ids everything was stored under.
async fn host_populated_locale(
    router: &MultiServiceRouter,
    bot_name: &str,
    case: &Case,
) -> (String, Populated) {
    let bot_id = host_bot(router, bot_name).await;
    let (status, created) = call(
        router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body(case.locale_id, 0.4, json!({}))),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {:?}: expected CreateBotLocale to answer 202, got {status} {created:?}",
        case.what
    );

    let mut populated = Populated::default();
    for index in 0..case.slot_types {
        populated.slot_type_ids.push(
            create_slot_type(
                router,
                &bot_id,
                case.locale_id,
                &slot_type_body(&format!("DeletedSlotType{index}"), json!({})),
            )
            .await,
        );
    }
    for index in 0..case.intents {
        let intent_id = create_intent(
            router,
            &bot_id,
            case.locale_id,
            &intent_body(&format!("DeletedIntent{index}"), json!({})),
        )
        .await;
        for slot_index in 0..case.slots_per_intent {
            // A slot elicits values for a slot type of its own locale when the row
            // created one, so the delete has to take a slot that referenced one too.
            let extra = match populated.slot_type_ids.first() {
                Some(slot_type_id) => json!({ "slotTypeId": slot_type_id }),
                None => json!({}),
            };
            let body = slot_body(&format!("DeletedSlot{index}x{slot_index}"), extra);
            let (status, created) = call(
                router,
                "PUT",
                &slots_uri(&bot_id, "DRAFT", case.locale_id, &intent_id),
                Some(body.clone()),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateSlot to answer 200, got {status} {created:?}",
                case.what
            );
            populated
                .slots
                .push((intent_id.clone(), text(&created, "slotId")));
        }
        populated.intent_ids.push(intent_id);
    }
    (bot_id, populated)
}

/// Every read that must answer `found` for the locale and everything stored in it.
async fn assert_reads(
    router: &MultiServiceRouter,
    case: &str,
    bot_id: &str,
    locale_id: &str,
    populated: &Populated,
    found: bool,
) {
    let expected = if found { 200 } else { 404 };
    let mut reads: Vec<(String, String)> = vec![(
        "the locale".to_string(),
        locale_uri(bot_id, "DRAFT", locale_id),
    )];
    for intent_id in &populated.intent_ids {
        reads.push((
            format!("intent {intent_id}"),
            intent_uri(bot_id, "DRAFT", locale_id, intent_id),
        ));
    }
    for slot_type_id in &populated.slot_type_ids {
        reads.push((
            format!("slot type {slot_type_id}"),
            slot_type_uri(bot_id, "DRAFT", locale_id, slot_type_id),
        ));
    }
    for (intent_id, slot_id) in &populated.slots {
        reads.push((
            format!("slot {slot_id}"),
            slot_uri(bot_id, "DRAFT", locale_id, intent_id, slot_id),
        ));
    }

    for (what, uri) in reads {
        let (status, response) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected a read of {what} to answer {expected}, got {status} \
             {response:?}"
        );
        if !found {
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for {what}, got {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_removes_the_locale_and_every_resource_defined_in_it() {
    let router = emulator();

    for (index, case) in deletion_cases().iter().enumerate() {
        let (bot_id, populated) =
            host_populated_locale(&router, &format!("DeleteLocaleBot{index}"), case).await;

        // Everything the row asked for is there to be deleted.
        assert_reads(
            &router,
            case.what,
            &bot_id,
            case.locale_id,
            &populated,
            true,
        )
        .await;

        let (status, deleted) = call(
            &router,
            "DELETE",
            &locale_uri(&bot_id, "DRAFT", case.locale_id),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: expected DeleteBotLocale to answer 202, got {status} {deleted:?}",
            case.what
        );
        assert_eq!(
            text(&deleted, "botId"),
            bot_id,
            "case {:?}: the delete must report the bot it was called on, got {deleted:?}",
            case.what
        );
        assert_eq!(
            text(&deleted, "botVersion"),
            "DRAFT",
            "case {:?}: the delete must report the bot version, got {deleted:?}",
            case.what
        );
        assert_eq!(
            text(&deleted, "localeId"),
            case.locale_id,
            "case {:?}: the delete must report the locale it removed, got {deleted:?}",
            case.what
        );
        assert_eq!(
            text(&deleted, "botLocaleStatus"),
            state::BOT_LOCALE_STATUS_DELETING,
            "case {:?}: a deleted locale first enters the Deleting status, got {deleted:?}",
            case.what
        );

        // And nothing of the locale is readable any more.
        assert_reads(
            &router,
            case.what,
            &bot_id,
            case.locale_id,
            &populated,
            false,
        )
        .await;

        // A second delete of the same triple finds nothing to remove.
        let (status, response) = call(
            &router,
            "DELETE",
            &locale_uri(&bot_id, "DRAFT", case.locale_id),
            None,
        )
        .await;
        assert_eq!(
            status, 412,
            "case {:?}: deleting the locale twice must be refused, got {status} {response:?}",
            case.what
        );
        assert_eq!(
            error_code(&response),
            "PreconditionFailedException",
            "case {:?}: expected PreconditionFailedException, got {response:?}",
            case.what
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_locale_it_names() {
    let router = emulator();
    let case = Case {
        what: "the deleted locale",
        locale_id: "en_US",
        intents: 1,
        slot_types: 1,
        slots_per_intent: 1,
    };
    let (bot_id, deleted) = host_populated_locale(&router, "ScopedDeleteLocaleBot", &case).await;

    // A second locale of the SAME bot, populated the same way.
    let kept = Case {
        what: "another locale of the same bot",
        locale_id: "de_DE",
        ..case
    };
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body(kept.locale_id, 0.4, json!({}))),
    )
    .await;
    assert_eq!(
        status, 202,
        "the second locale of the bot must be created, got {status} {created:?}"
    );
    let kept_slot_type = create_slot_type(
        &router,
        &bot_id,
        kept.locale_id,
        &slot_type_body("KeptSlotType", json!({})),
    )
    .await;
    let kept_intent = create_intent(
        &router,
        &bot_id,
        kept.locale_id,
        &intent_body("KeptIntent", json!({})),
    )
    .await;
    let kept_here = Populated {
        intent_ids: vec![kept_intent],
        slot_type_ids: vec![kept_slot_type],
        slots: Vec::new(),
    };

    // And the SAME locale id on another bot, populated the same way.
    let (other_bot_id, kept_there) =
        host_populated_locale(&router, "OtherScopedDeleteLocaleBot", &case).await;

    let (status, response) = call(
        &router,
        "DELETE",
        &locale_uri(&bot_id, "DRAFT", case.locale_id),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "the delete must succeed, got {status} {response:?}"
    );

    assert_reads(
        &router,
        kept.what,
        &bot_id,
        kept.locale_id,
        &kept_here,
        true,
    )
    .await;
    assert_reads(
        &router,
        "the same locale id of another bot",
        &other_bot_id,
        case.locale_id,
        &kept_there,
        true,
    )
    .await;
    assert_reads(&router, case.what, &bot_id, case.locale_id, &deleted, false).await;

    // The id is free again, and what is created under it holds nothing of what went:
    // "a locale is created with the fallback intent Amazon Lex adds to it", and that is
    // the one intent the new locale counts.
    let (status, recreated) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body(
            case.locale_id,
            0.9,
            json!({ "description": "the locale created after the delete" }),
        )),
    )
    .await;
    assert_eq!(
        status, 202,
        "the locale id must be free to create again, got {status} {recreated:?}"
    );
    let (status, described) = call(
        &router,
        "GET",
        &locale_uri(&bot_id, "DRAFT", case.locale_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the recreated locale must be readable, got {status} {described:?}"
    );
    assert_eq!(
        text(&described, "description"),
        "the locale created after the delete",
        "the recreated locale is the one that was just created, got {described:?}"
    );
    assert_eq!(
        member(&described, "intentsCount"),
        Value::from(1),
        "a locale created after the delete holds only its fallback intent, got {described:?}"
    );
    assert_eq!(
        member(&described, "slotTypesCount"),
        Value::from(0),
        "a locale created after the delete holds no slot type, got {described:?}"
    );
}

#[tokio::test]
async fn a_delete_of_a_locale_the_store_does_not_hold_is_refused_and_removes_nothing() {
    let router = emulator();
    let case = Case {
        what: "the stored locale",
        locale_id: "en_US",
        intents: 1,
        slot_types: 1,
        slots_per_intent: 1,
    };
    let (bot_id, populated) = host_populated_locale(&router, "RefusedDeleteLocaleBot", &case).await;

    // One row per triple the store holds no locale under, each with the sentence this
    // service states for the first identifier in the request path that failed to
    // resolve.
    let misses: Vec<(&str, String, String)> = vec![
        (
            "a bot the store does not hold",
            locale_uri("ABCDEFGHIJ", "DRAFT", case.locale_id),
            state::request_path_resource_missing(
                state::EXPORTED_BOT_LABEL,
                &[("id", "ABCDEFGHIJ"), ("version", "DRAFT")],
            ),
        ),
        (
            "a locale never created on a bot that exists",
            locale_uri(&bot_id, "DRAFT", "nl_NL"),
            state::request_path_resource_missing(state::EXPORTED_BOT_LOCALE_LABEL, &[("id", "nl_NL")]),
        ),
        (
            "a locale id no supported locale carries",
            locale_uri(&bot_id, "DRAFT", "xx_XX"),
            state::request_path_resource_missing(state::EXPORTED_BOT_LOCALE_LABEL, &[("id", "xx_XX")]),
        ),
    ];

    for (what, uri, expected) in &misses {
        let (status, response) = call(&router, "DELETE", uri, None).await;
        assert_eq!(
            status, 412,
            "case {what:?}: expected HTTP 412, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "PreconditionFailedException",
            "case {what:?}: expected PreconditionFailedException, got {response:?}"
        );
        let message = text(&response, "message");
        assert_eq!(
            &message, expected,
            "case {what:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // None of the refusals removed the locale the bot does hold, or anything in it.
    assert_reads(
        &router,
        case.what,
        &bot_id,
        case.locale_id,
        &populated,
        true,
    )
    .await;
}
