// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the slot-ordering validation a Lex Models V2 locale build
//! performs: which terminal `botLocaleStatus` a `BuildBotLocale` settles on when an intent
//! of the locale holds slots its `slotPriorities` does not order, and what
//! `DescribeBotLocale` then reports in `failureReasons`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot/locale
//! setup and the intent and slot helpers are the other suites', reused rather than copied.
//!
//! The rule under test, asserted over a TABLE of intent contents rather than one case: a
//! build settles `Failed` — carrying one reason per intent, naming that intent and the
//! slots in question — when any intent of the locale holds a slot no entry of its
//! `slotPriorities` orders, and `Built` only when every slot of every intent is ordered.
//! The rows differ in the number of slots an intent holds, which of them are ordered, the
//! `slotConstraint` each elicits under (the modelled `SlotConstraint` admits exactly
//! `Required` and `Optional` —
//! `smithy_query(query="shape",shape="SlotConstraint")`), the intent that holds them and
//! the locale the build is asked for, so what is asserted is the rule and not one recorded
//! locale.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, member, text};
use crate::bot_locale::locale_uri;
use crate::intents_and_builds::{build_uri, create_intent, intent_body, intent_uri};
use crate::slot_type::host_locale;
use crate::slots::{slot_body, slots_uri};

/// The sample utterance every intent under test carries, so the locale is past the OTHER
/// thing a build validates (it holds a custom intent with a valid utterance) and the
/// outcome turns on the slot ordering alone.
const UTTERANCE: &str = "I want to order a pizza";

/// A `CreateIntent` / `UpdateIntent` body for a custom intent carrying [`UTTERANCE`] plus
/// `extra`.
fn intent_with_utterance(name: &str, extra: Value) -> Value {
    let mut body = intent_body(name, json!({ "sampleUtterances": [{ "utterance": UTTERANCE }] }));
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// Create a slot on one intent under the named `slotConstraint`, assert it was accepted,
/// and return its minted id.
async fn create_slot(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    intent_id: &str,
    slot_name: &str,
    constraint: &str,
) -> String {
    let body = slot_body(
        slot_name,
        json!({
            "valueElicitationSetting": {
                "slotConstraint": constraint,
                "promptSpecification": {
                    "messageGroups": [{
                        "message": { "plainTextMessage": { "value": "Which one?" } },
                    }],
                    "maxRetries": 2,
                },
            },
        }),
    );
    let (status, created) = call(
        router,
        "PUT",
        &slots_uri(bot_id, "DRAFT", locale_id, intent_id),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSlot {body:?}: expected 200, got {status} {created:?}"
    );
    text(&created, "slotId")
}

/// Order `slot_ids` on the intent through `UpdateIntent`, keeping its utterance, and
/// assert the stored `slotPriorities` are the ones asked for.
async fn order_slots(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    intent_id: &str,
    intent_name: &str,
    slot_ids: &[String],
) {
    let priorities: Value = slot_ids
        .iter()
        .enumerate()
        .map(|(index, slot_id)| json!({ "priority": index + 1, "slotId": slot_id }))
        .collect();
    let (status, updated) = call(
        router,
        "PUT",
        &intent_uri(bot_id, "DRAFT", locale_id, intent_id),
        Some(intent_with_utterance(
            intent_name,
            json!({ "slotPriorities": priorities.clone() }),
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UpdateIntent to answer 200, got {status} {updated:?}"
    );
    // The priorities the request supplied are the priorities the intent holds, the empty
    // list this helper sends when it is asked to order nothing included.
    assert_eq!(
        member(&updated, "slotPriorities"),
        priorities,
        "UpdateIntent must report the priorities it stored, expected {priorities:?}, got \
         {updated:?}"
    );
}

/// Submit a build, assert it is accepted as in flight, and return what
/// `DescribeBotLocale` reports once the terminal status the clock owes has fallen due.
async fn build_and_settle(router: &MultiServiceRouter, bot_id: &str, locale_id: &str) -> Value {
    let (status, submitted) = call(
        router,
        "POST",
        &build_uri(bot_id, "DRAFT", locale_id),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "locale {locale_id}: expected BuildBotLocale to answer 202, got {status} {submitted:?}"
    );
    assert_eq!(
        text(&submitted, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILDING,
        "locale {locale_id}: a submitted build reports the transient status, got {submitted:?}"
    );

    tokio::time::sleep(state::BUILD_SETTLES_AFTER + Duration::from_millis(150)).await;
    let (status, described) =
        call(router, "GET", &locale_uri(bot_id, "DRAFT", locale_id), None).await;
    assert_eq!(
        status, 200,
        "locale {locale_id}: expected DescribeBotLocale to answer 200, got {status} \
         {described:?}"
    );
    described
}

/// One row of the table: the intent a locale is given, the slots it is given in that
/// order, and which of those slots are then ordered by its `slotPriorities`.
struct Row {
    case: &'static str,
    locale_id: &'static str,
    intent_name: &'static str,
    /// The slots to create on the intent, in the order they are created: the slot name
    /// and the `slotConstraint` it elicits under.
    slots: Vec<(&'static str, &'static str)>,
    /// Which of `slots`, by index, an `UpdateIntent` then orders.
    ordered: Vec<usize>,
}

/// The intent contents the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one Required slot, nothing ordered",
            locale_id: "en_US",
            intent_name: "OrderPizza",
            slots: vec![("PizzaSize", "Required")],
            ordered: vec![],
        },
        Row {
            case: "one Optional slot, nothing ordered",
            locale_id: "en_US",
            intent_name: "OrderPizza",
            slots: vec![("Crust", "Optional")],
            ordered: vec![],
        },
        Row {
            case: "a Required and an Optional slot, nothing ordered",
            locale_id: "en_US",
            intent_name: "ReserveTable",
            slots: vec![("PartySize", "Required"), ("SeatingType", "Optional")],
            ordered: vec![],
        },
        Row {
            case: "three slots, only the middle one ordered",
            locale_id: "en_US",
            intent_name: "OrderPizza",
            slots: vec![
                ("PizzaSize", "Required"),
                ("Crust", "Optional"),
                ("Toppings", "Required"),
            ],
            ordered: vec![1],
        },
        Row {
            case: "three slots, every one of them ordered",
            locale_id: "en_US",
            intent_name: "OrderPizza",
            slots: vec![
                ("PizzaSize", "Required"),
                ("Crust", "Optional"),
                ("Toppings", "Required"),
            ],
            ordered: vec![0, 1, 2],
        },
        Row {
            case: "the one slot of the intent ordered",
            locale_id: "en_US",
            intent_name: "OrderPizza",
            slots: vec![("PizzaSize", "Required")],
            ordered: vec![0],
        },
        Row {
            case: "an intent holding no slot at all",
            locale_id: "en_US",
            intent_name: "OrderPizza",
            slots: vec![],
            ordered: vec![],
        },
        Row {
            case: "slots whose creation order is the reverse of their names",
            locale_id: "en_US",
            intent_name: "PickOption",
            slots: vec![("Zeta", "Required"), ("Alpha", "Optional")],
            ordered: vec![],
        },
        Row {
            case: "another locale of the service, nothing ordered",
            locale_id: "de_DE",
            intent_name: "BestellePizza",
            slots: vec![("Teiggroesse", "Required")],
            ordered: vec![],
        },
        Row {
            case: "another locale of the service, every slot ordered",
            locale_id: "de_DE",
            intent_name: "BestellePizza",
            slots: vec![("Teiggroesse", "Required")],
            ordered: vec![0],
        },
    ]
}

#[tokio::test]
async fn a_build_fails_on_every_intent_whose_slots_its_priorities_do_not_order() {
    for row in rows() {
        let router = emulator();
        let bot_id = host_locale(&router, "SlotPriorityBot", row.locale_id).await;
        let intent_id = create_intent(
            &router,
            &bot_id,
            row.locale_id,
            &intent_with_utterance(row.intent_name, json!({})),
        )
        .await;

        let mut slot_ids = Vec::new();
        for (slot_name, constraint) in &row.slots {
            slot_ids.push(
                create_slot(
                    &router,
                    &bot_id,
                    row.locale_id,
                    &intent_id,
                    slot_name,
                    constraint,
                )
                .await,
            );
        }
        if !row.ordered.is_empty() {
            let ordered: Vec<String> = row
                .ordered
                .iter()
                .map(|index| slot_ids[*index].clone())
                .collect();
            order_slots(
                &router,
                &bot_id,
                row.locale_id,
                &intent_id,
                row.intent_name,
                &ordered,
            )
            .await;
        }

        // The slots left out of the priorities, in the order the intent acquired them.
        let unordered: Vec<String> = row
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _)| !row.ordered.contains(index))
            .map(|(_, (slot_name, _))| (*slot_name).to_string())
            .collect();

        let described = build_and_settle(&router, &bot_id, row.locale_id).await;
        let expected_status = if unordered.is_empty() {
            state::BOT_LOCALE_STATUS_BUILT
        } else {
            state::BOT_LOCALE_STATUS_FAILED
        };
        let actual_status = text(&described, "botLocaleStatus");
        assert_eq!(
            actual_status, expected_status,
            "case {:?}: expected the build to settle at {expected_status}, got \
             {actual_status:?} in {described:?}",
            row.case
        );

        let expected_reasons = if unordered.is_empty() {
            Value::Null
        } else {
            json!([state::unprioritised_slots_failure_reason(
                &unordered,
                row.intent_name
            )])
        };
        let actual_reasons = member(&described, "failureReasons");
        assert_eq!(
            actual_reasons, expected_reasons,
            "case {:?}: expected failureReasons {expected_reasons:?}, got {actual_reasons:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_reason_is_worded_the_way_the_service_words_it() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotPriorityWordingBot", "en_US").await;
    let intent_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_with_utterance("ReserveTable", json!({})),
    )
    .await;
    create_slot(&router, &bot_id, "en_US", &intent_id, "PartySize", "Required").await;

    // One unordered slot: the brackets hold just it.
    let described = build_and_settle(&router, &bot_id, "en_US").await;
    assert_eq!(
        member(&described, "failureReasons"),
        json!([
            "Slot ids [PartySize] in intent ReserveTable don't define a slot priority. Update \
             the intent to add a priority to these slots."
        ]),
        "one unordered slot is reported with the service's own sentence, got {described:?}"
    );

    // A second unordered slot joins the first inside the brackets, comma separated, in the
    // order the intent acquired them.
    create_slot(
        &router,
        &bot_id,
        "en_US",
        &intent_id,
        "SeatingType",
        "Optional",
    )
    .await;
    let described = build_and_settle(&router, &bot_id, "en_US").await;
    assert_eq!(
        member(&described, "failureReasons"),
        json!([
            "Slot ids [PartySize, SeatingType] in intent ReserveTable don't define a slot \
             priority. Update the intent to add a priority to these slots."
        ]),
        "both unordered slots are reported in one sentence, got {described:?}"
    );
}

#[tokio::test]
async fn one_reason_per_intent_beside_the_other_thing_a_build_validates() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotPriorityPerIntentBot", "en_US").await;

    // Two intents, each holding a slot nothing orders: the build reports one reason for
    // each, in the order the locale acquired the intents.
    let first = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_with_utterance("OrderPizza", json!({})),
    )
    .await;
    create_slot(&router, &bot_id, "en_US", &first, "PizzaSize", "Required").await;
    let second = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_with_utterance("ReserveTable", json!({})),
    )
    .await;
    create_slot(&router, &bot_id, "en_US", &second, "PartySize", "Optional").await;

    let described = build_and_settle(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&described, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_FAILED,
        "two intents with unordered slots must fail the build, got {described:?}"
    );
    assert_eq!(
        member(&described, "failureReasons"),
        json!([
            state::unprioritised_slots_failure_reason(&["PizzaSize".to_string()], "OrderPizza"),
            state::unprioritised_slots_failure_reason(&["PartySize".to_string()], "ReserveTable"),
        ]),
        "each intent contributes its own reason, got {described:?}"
    );

    // A locale that also has nothing to recognise reports that reason too: the two things
    // a build validates are reported side by side, not one instead of the other.
    let other_bot = host_locale(&router, "SlotPriorityAndUtteranceBot", "en_US").await;
    let bare = create_intent(
        &router,
        &other_bot,
        "en_US",
        &intent_body("OrderPizza", json!({})),
    )
    .await;
    create_slot(&router, &other_bot, "en_US", &bare, "PizzaSize", "Required").await;
    let described = build_and_settle(&router, &other_bot, "en_US").await;
    assert_eq!(
        member(&described, "failureReasons"),
        json!([
            state::no_utterances_failure_reason("en_US"),
            state::unprioritised_slots_failure_reason(&["PizzaSize".to_string()], "OrderPizza"),
        ]),
        "both reasons are reported, got {described:?}"
    );
}

#[tokio::test]
async fn ordering_the_slot_makes_the_same_locale_build_and_unordering_it_fails_again() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotPriorityRebuildBot", "en_US").await;
    let intent_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_with_utterance("OrderPizza", json!({})),
    )
    .await;
    let slot_id = create_slot(&router, &bot_id, "en_US", &intent_id, "PizzaSize", "Required").await;

    let failed = build_and_settle(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&failed, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_FAILED,
        "the unordered slot must fail the build, got {failed:?}"
    );

    // Ordering the slot and nothing else makes the same locale build, and the reasons of
    // the failed build go with it.
    order_slots(
        &router,
        &bot_id,
        "en_US",
        &intent_id,
        "OrderPizza",
        std::slice::from_ref(&slot_id),
    )
    .await;
    // The priority is readable back off the intent, so what the build validates is stored
    // state and not one request's content.
    let (status, described_intent) = call(
        &router,
        "GET",
        &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeIntent to answer 200, got {status} {described_intent:?}"
    );
    assert_eq!(
        member(&described_intent, "slotPriorities"),
        json!([{ "priority": 1, "slotId": slot_id }]),
        "DescribeIntent must report the stored priority, got {described_intent:?}"
    );

    let built = build_and_settle(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&built, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILT,
        "the ordered locale must build, got {built:?}"
    );
    assert_eq!(
        member(&built, "failureReasons"),
        Value::Null,
        "a locale that built carries no failureReasons, got {built:?}"
    );

    // And the other way: taking the priority away again fails the next build, which is the
    // same rule read from the other side.
    order_slots(&router, &bot_id, "en_US", &intent_id, "OrderPizza", &[]).await;
    let failed_again = build_and_settle(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&failed_again, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_FAILED,
        "the locale must fail the build again, got {failed_again:?}"
    );
    assert_eq!(
        member(&failed_again, "failureReasons"),
        json!([state::unprioritised_slots_failure_reason(
            &["PizzaSize".to_string()],
            "OrderPizza"
        )]),
        "the failed rebuild must report the reason again, got {failed_again:?}"
    );
}
