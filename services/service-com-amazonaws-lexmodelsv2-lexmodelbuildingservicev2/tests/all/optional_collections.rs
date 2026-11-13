// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ONE rule over this service's restJson1 documents: a
//! modelled optional collection member is serialized only when the stored record has a
//! value for it.
//!
//! A write stores what the request supplied — a collection it did not name has NO value,
//! and one it named is stored as given, the empty collection included — and every
//! response that echoes that member reports exactly that: the key is in the document when
//! the record holds a value, and is not in the document at all when it does not. That is
//! the difference a client reads with `'name' in response`, which an empty array answers
//! the wrong way round.
//!
//! The rule is asserted over a TABLE whose rows differ in which collections the request
//! names, in whether a named one is empty or carries entries, and in how many entries it
//! carries (up to the published maxima of the modelled list shapes). It is checked on
//! every operation that echoes one of these members: `CreateIntent`, `UpdateIntent`,
//! `DescribeIntent`, `CreateBot`, `DescribeBot`, `CreateSlotType` and `DescribeSlotType`.

use serde_json::{json, Value};

use crate::bot_lifecycle::{call, create_body, emulator, member, text};
use crate::intents_and_builds::{create_intent, intent_body, intent_uri, intents_uri};
use crate::slot_type::{host_locale, slot_type_body, slot_types_uri};

/// The optional collection members an intent carries, as the modelled intent shapes
/// declare them (`smithy_query(query="operation",operation="CreateIntent")` and
/// `...operation="UpdateIntent"`): none is `@required` and none carries a `@default`.
/// `slotPriorities` is on the update and the read but not on the create, so a create can
/// never report it.
const INTENT_COLLECTIONS: [&str; 4] = [
    "sampleUtterances",
    "inputContexts",
    "outputContexts",
    "slotPriorities",
];

/// Is `name` a key of this JSON document at all? A member with no value is not
/// serialized, so this — and not whether the value is empty — is what a presence test
/// against the response reads.
fn has_key(body: &Value, name: &str) -> bool {
    body.as_object()
        .is_some_and(|object| object.contains_key(name))
}

/// Assert `response` echoes `name` exactly as `request` supplied it: present and equal
/// when the request named it, and absent from the document when it did not.
fn assert_echoed(case: &str, operation: &str, request: &Value, response: &Value, name: &str) {
    match request.get(name) {
        Some(supplied) => {
            assert!(
                has_key(response, name),
                "case {case:?}: {operation} must carry the {name} the request supplied \
                 ({supplied:?}), got {response:?}"
            );
            assert_eq!(
                &member(response, name),
                supplied,
                "case {case:?}: {operation} must report the {name} the request supplied, got \
                 {response:?}"
            );
        }
        None => assert!(
            !has_key(response, name),
            "case {case:?}: {operation} must not carry a {name} key when the request supplied \
             none, got {response:?}"
        ),
    }
}

/// One `outputContexts` entry: the context a fulfilled intent activates, with the
/// lifetime the modelled `OutputContext` carries.
fn output_context(name: &str) -> Value {
    json!({ "name": name, "timeToLiveInSeconds": 60, "turnsToLive": 2 })
}

/// The table of intent writes. Rows differ in which collections the request names,
/// whether a named one is empty, and how many entries it carries — the maxima are the
/// modelled `InputContextsList` (@length 0..5) and `OutputContextsList` (@length 0..10)
/// (`smithy_query(query="shape",shape="InputContextsList,OutputContextsList")`);
/// `SampleUtterancesList` and `SlotPrioritiesList` carry no length constraint.
fn intent_collection_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("no collection member at all", json!({})),
        (
            "an empty input context list and nothing else",
            json!({ "inputContexts": [] }),
        ),
        (
            "empty lists of both kinds of context",
            json!({ "inputContexts": [], "outputContexts": [] }),
        ),
        (
            "an empty sample utterance list",
            json!({ "sampleUtterances": [] }),
        ),
        (
            "one input context and no output context",
            json!({ "inputContexts": [{ "name": "OrderInProgress" }] }),
        ),
        (
            "one output context and no input context",
            json!({ "outputContexts": [output_context("OrderPlaced")] }),
        ),
        (
            "input contexts at the published maximum of five",
            json!({
                "inputContexts": (0..5)
                    .map(|index| json!({ "name": format!("InputContext{index}") }))
                    .collect::<Value>(),
            }),
        ),
        (
            "output contexts at the published maximum of ten",
            json!({
                "outputContexts": (0..10)
                    .map(|index| output_context(&format!("OutputContext{index}")))
                    .collect::<Value>(),
            }),
        ),
        (
            "sample utterances but neither kind of context",
            json!({
                "sampleUtterances": [
                    { "utterance": "I want to order a pizza" },
                    { "utterance": "Can I order a {PizzaSize} pizza" },
                ],
            }),
        ),
        (
            "an utterance, an input context and an empty output context list",
            json!({
                "sampleUtterances": [{ "utterance": "Resume my order" }],
                "inputContexts": [{ "name": "OrderInProgress" }],
                "outputContexts": [],
            }),
        ),
    ]
}

#[tokio::test]
async fn a_created_intent_reports_the_collections_the_request_supplied_and_no_other() {
    let router = emulator();
    let bot_id = host_locale(&router, "IntentCollectionBot", "en_US").await;

    for (index, (case, collections)) in intent_collection_cases().into_iter().enumerate() {
        let request = intent_body(&format!("Intent_{index}"), collections);
        let (status, created) = call(
            &router,
            "PUT",
            &intents_uri(&bot_id, "DRAFT", "en_US"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateIntent to answer 200, got {status} {created:?}"
        );
        for name in INTENT_COLLECTIONS {
            assert_echoed(case, "CreateIntent", &request, &created, name);
        }

        // The same document a later read reports, off the record the create stored.
        let intent_id = text(&created, "intentId");
        let (status, described) = call(
            &router,
            "GET",
            &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeIntent to answer 200, got {status} {described:?}"
        );
        for name in INTENT_COLLECTIONS {
            assert_echoed(case, "DescribeIntent", &request, &described, name);
        }
    }
}

#[tokio::test]
async fn an_updated_intent_reports_the_collections_that_update_supplied_and_no_other() {
    let router = emulator();
    let bot_id = host_locale(&router, "IntentCollectionUpdateBot", "en_US").await;

    // The intent starts out carrying collections of every kind, so each row below is a
    // replacement and not a first write: what the update leaves the record with is what
    // the update named, whatever it carried before.
    let created = intent_body(
        "OrderPizza",
        json!({
            "sampleUtterances": [{ "utterance": "I want to order a pizza" }],
            "inputContexts": [{ "name": "OrderInProgress" }],
            "outputContexts": [output_context("OrderPlaced")],
        }),
    );
    let intent_id = create_intent(&router, &bot_id, "en_US", &created).await;

    // Rows: an update that names nothing, one that names an empty collection of every
    // kind, and one that names entries again — including the `slotPriorities` only an
    // update carries.
    let updates: Vec<(&str, Value)> = vec![
        ("an update naming no collection at all", json!({})),
        (
            "an update naming an empty list of every kind",
            json!({
                "sampleUtterances": [],
                "inputContexts": [],
                "outputContexts": [],
                "slotPriorities": [],
            }),
        ),
        (
            "an update naming entries of every kind again",
            json!({
                "sampleUtterances": [{ "utterance": "Order a pizza" }],
                "inputContexts": [{ "name": "Resumed" }],
                "outputContexts": [output_context("Ordered")],
            }),
        ),
        (
            "an update naming only the contexts",
            json!({
                "inputContexts": [{ "name": "OrderInProgress" }, { "name": "Resumed" }],
                "outputContexts": [],
            }),
        ),
    ];

    for (case, collections) in updates {
        let request = intent_body("OrderPizza", collections);
        let (status, updated) = call(
            &router,
            "PUT",
            &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateIntent to answer 200, got {status} {updated:?}"
        );
        for name in INTENT_COLLECTIONS {
            assert_echoed(case, "UpdateIntent", &request, &updated, name);
        }

        let (status, described) = call(
            &router,
            "GET",
            &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeIntent to answer 200, got {status} {described:?}"
        );
        for name in INTENT_COLLECTIONS {
            assert_echoed(case, "DescribeIntent", &request, &described, name);
        }
    }
}

#[tokio::test]
async fn a_created_bot_reports_the_collections_the_request_supplied_and_no_other() {
    let router = emulator();

    // `botMembers` targets the modelled `BotMembers` list (@length 0..10) and both tag
    // members target `TagMap` (@length 0..200)
    // (`smithy_query(query="shape",shape="BotMembers,TagMap")`), so an empty collection
    // is a value the request may supply for any of the three.
    let member_entry = |id: &str, name: &str| {
        json!({
            "botMemberId": id,
            "botMemberName": name,
            "botMemberAliasId": "TSTALIASID",
            "botMemberAliasName": "AmazonLexTestAlias",
            "botMemberVersion": "DRAFT",
        })
    };
    let cases: Vec<(&str, Value)> = vec![
        ("no collection member at all", json!({})),
        (
            "an empty bot member list",
            json!({ "botType": "BotNetwork", "botMembers": [] }),
        ),
        ("empty tag maps", json!({ "botTags": {}, "testBotAliasTags": {} })),
        (
            "two bot members and no tag",
            json!({
                "botType": "BotNetwork",
                "botMembers": [
                    member_entry("ABCDEFGHIJ", "MemberOne"),
                    member_entry("KLMNOPQRST", "MemberTwo"),
                ],
            }),
        ),
        (
            "tags on the bot and on its test alias, and no member",
            json!({
                "botTags": { "team": "voice", "env": "test" },
                "testBotAliasTags": { "env": "test" },
            }),
        ),
    ];

    for (index, (case, collections)) in cases.into_iter().enumerate() {
        let request = create_body(&format!("CollectionBot{index}"), 300, collections);
        let (status, created) = call(&router, "PUT", "/bots", Some(request.clone())).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected CreateBot to answer 202, got {status} {created:?}"
        );
        for name in ["botMembers", "botTags", "testBotAliasTags"] {
            assert_echoed(case, "CreateBot", &request, &created, name);
        }

        // `DescribeBotResponse` carries `botMembers` and neither tag member
        // (`smithy_query(query="operation",operation="DescribeBot")`), so that is the one
        // collection a read of the bot echoes.
        let bot_id = text(&created, "botId");
        let (status, described) = call(&router, "GET", &format!("/bots/{bot_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBot to answer 200, got {status} {described:?}"
        );
        assert_echoed(case, "DescribeBot", &request, &described, "botMembers");
    }
}

#[tokio::test]
async fn a_created_slot_type_reports_the_values_the_request_supplied_and_no_other() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotTypeCollectionBot", "en_US").await;

    // The modelled `SlotTypeValues` list has a minimum length of 1
    // (`smithy_query(query="shape",shape="SlotTypeValues")`), so a slot type either
    // carries values or carries the member not at all: an explicitly empty list is
    // refused, which the slot type suite asserts.
    let value = |text: &str| json!({ "sampleValue": { "value": text } });
    let cases: Vec<(&str, Value)> = vec![
        ("no values at all", json!({})),
        ("one value", json!({ "slotTypeValues": [value("large")] })),
        (
            "three values",
            json!({ "slotTypeValues": [value("small"), value("medium"), value("large")] }),
        ),
    ];

    for (index, (case, values)) in cases.into_iter().enumerate() {
        let request = slot_type_body(&format!("SlotType{index}"), values);
        let (status, created) = call(
            &router,
            "PUT",
            &slot_types_uri(&bot_id, "DRAFT", "en_US"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateSlotType to answer 200, got {status} {created:?}"
        );
        assert_echoed(case, "CreateSlotType", &request, &created, "slotTypeValues");

        let slot_type_id = text(&created, "slotTypeId");
        let (status, described) = call(
            &router,
            "GET",
            &format!(
                "{}/{slot_type_id}",
                slot_types_uri(&bot_id, "DRAFT", "en_US")
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSlotType to answer 200, got {status} {described:?}"
        );
        assert_echoed(
            case,
            "DescribeSlotType",
            &request,
            &described,
            "slotTypeValues",
        );
    }
}
