// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 intent and build slice: `CreateIntent`,
//! `DescribeIntent` and `BuildBotLocale`, plus the two counts `DescribeBotLocale` reports
//! off them.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them. The harness and the
//! bot/locale setup are the bot lifecycle and bot locale suites', reused rather than
//! copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a create mints an `intentId` matching the modelled `Id` shape and stores the intent
//!   under the bot, bot version and locale the URI names, so a read of that id reports
//!   back what was stored;
//! * a locale is created with the fallback intent Amazon Lex adds to it, and
//!   `DescribeBotLocale` reports the number of intents and slot types the locale holds —
//!   never an absent count;
//! * an intent name is unique inside its locale, and an intent is reachable only under
//!   its own triple, so deleting the bot takes its intents with it;
//! * a submitted build is recorded on the locale: it is `Building` from the submission,
//!   which is what `lastBuildSubmittedDateTime` reports, and the `Built` the clock owes
//!   is materialised once the span has elapsed.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, seconds, text};
use crate::bot_locale::{host_bot, locale_body, locale_uri, locales_uri};

/// The members `CreateIntent` accepts that both it and `DescribeIntent` report back
/// unchanged, the optional collections of the intent included: a member the request named
/// is reported as it was given, and one it did not name is reported not at all.
const ECHOED_BY_DESCRIBE: [&str; 8] = [
    "intentName",
    "intentDisplayName",
    "description",
    "parentIntentSignature",
    "dialogCodeHook",
    "fulfillmentCodeHook",
    "intentConfirmationSetting",
    "intentClosingSetting",
];

/// The list members of an intent, which both a create and a describe echo back exactly as
/// the request supplied them.
const HELD_LISTS: [&str; 3] = ["sampleUtterances", "inputContexts", "outputContexts"];

/// `CreateIntent`'s URI:
/// `PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents`.
pub(crate) fn intents_uri(bot_id: &str, bot_version: &str, locale_id: &str) -> String {
    format!("/bots/{bot_id}/botversions/{bot_version}/botlocales/{locale_id}/intents")
}

/// `DescribeIntent`'s URI, which adds the `intentId` to [`intents_uri`]. `UpdateIntent`
/// is the same URI under `PUT`.
pub(crate) fn intent_uri(
    bot_id: &str,
    bot_version: &str,
    locale_id: &str,
    intent_id: &str,
) -> String {
    format!("{}/{intent_id}", intents_uri(bot_id, bot_version, locale_id))
}

/// `BuildBotLocale`'s URI, which is `DescribeBotLocale`'s under `POST`.
pub(crate) fn build_uri(bot_id: &str, bot_version: &str, locale_id: &str) -> String {
    locale_uri(bot_id, bot_version, locale_id)
}

/// A `CreateIntent` request body carrying the required `intentName` plus `extra`.
pub(crate) fn intent_body(name: &str, extra: Value) -> Value {
    let mut body = json!({ "intentName": name });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// Create a bot and one locale on its `DRAFT` version, and return the minted `botId`.
async fn host_locale(router: &MultiServiceRouter, bot_name: &str, locale_id: &str) -> String {
    let bot_id = host_bot(router, bot_name).await;
    let (status, created) = call(
        router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body(locale_id, 0.4, json!({}))),
    )
    .await;
    assert_eq!(
        status, 202,
        "locale {locale_id}: expected CreateBotLocale to answer 202, got {status} {created:?}"
    );
    bot_id
}

/// Create an intent, assert it was accepted, and return its minted id.
pub(crate) async fn create_intent(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    body: &Value,
) -> String {
    let (status, created) = call(
        router,
        "PUT",
        &intents_uri(bot_id, "DRAFT", locale_id),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateIntent {body:?}: expected 200, got {status} {created:?}"
    );
    text(&created, "intentId")
}

/// One count `DescribeBotLocale` reports for a locale, or `i64::MIN` when it reports
/// none — which is itself a divergence, since the member is a NUMBER of records.
async fn locale_count(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    name: &str,
) -> i64 {
    let (status, described) =
        call(router, "GET", &locale_uri(bot_id, "DRAFT", locale_id), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBotLocale to answer 200, got {status} {described:?}"
    );
    member(&described, name).as_i64().unwrap_or(i64::MIN)
}

/// An intent name of exactly `len` characters satisfying the modelled `Name` pattern
/// `^([0-9a-zA-Z][_-]?){1,100}$` (`smithy_query(query="shape",shape="Name")`).
fn name_of_length(len: usize) -> String {
    "OrderPizza".chars().cycle().take(len).collect()
}

/// The table every round-trip assertion runs over: rows differ in which optional members
/// are set, in how many sample utterances and contexts the intent carries, in the length
/// of its name and description (each at its published maximum) and in whether it is based
/// on a built-in intent.
fn intent_cases() -> Vec<(&'static str, Value)> {
    let many_utterances: Value = (0..25)
        .map(|index| json!({ "utterance": format!("I want option {index}") }))
        .collect();

    vec![
        (
            "only the required member",
            intent_body("MinimalIntent", json!({})),
        ),
        (
            "sample utterances and a description",
            intent_body(
                "OrderPizza",
                json!({
                    "description": "order a pizza",
                    "sampleUtterances": [
                        { "utterance": "I want to order a pizza" },
                        { "utterance": "Can I order a {PizzaSize} pizza" },
                    ],
                }),
            ),
        ),
        (
            "a display name and many sample utterances",
            intent_body(
                "PickOption",
                json!({
                    "intentDisplayName": "Pick an option",
                    "sampleUtterances": many_utterances,
                }),
            ),
        ),
        (
            "code hooks on both sides of fulfillment",
            intent_body(
                "BookTrip",
                json!({
                    "dialogCodeHook": { "enabled": true },
                    "fulfillmentCodeHook": { "enabled": true },
                }),
            ),
        ),
        (
            "input and output contexts at their published maxima",
            intent_body(
                "ResumeOrder",
                json!({
                    "inputContexts": (0..5)
                        .map(|index| json!({ "name": format!("InputContext{index}") }))
                        .collect::<Value>(),
                    "outputContexts": (0..10)
                        .map(|index| json!({
                            "name": format!("OutputContext{index}"),
                            "timeToLiveInSeconds": 60,
                            "turnsToLive": 2,
                        }))
                        .collect::<Value>(),
                }),
            ),
        ),
        (
            "an intent based on a built-in intent",
            intent_body(
                "Help_Intent",
                json!({ "parentIntentSignature": "AMAZON.HelpIntent" }),
            ),
        ),
        (
            "a closing response the bot sends when the intent is done",
            intent_body(
                "CancelOrder",
                json!({
                    "intentClosingSetting": {
                        "closingResponse": {
                            "messageGroups": [{
                                "message": { "plainTextMessage": { "value": "Your order is cancelled." } },
                            }],
                            "allowInterrupt": true,
                        },
                        "active": true,
                    },
                }),
            ),
        ),
        (
            "a name and a description at their published maximum lengths",
            intent_body(
                &name_of_length(100),
                json!({ "description": "d".repeat(2_000) }),
            ),
        ),
    ]
}

#[tokio::test]
async fn a_created_intent_is_stored_and_read_back_through_its_own_triple() {
    let router = emulator();
    let bot_id = host_locale(&router, "IntentRoundTripBot", "en_US").await;

    let mut created: Vec<(String, String)> = Vec::new();

    for (case, request) in intent_cases() {
        let (status, answered) = call(
            &router,
            "PUT",
            &intents_uri(&bot_id, "DRAFT", "en_US"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateIntent to answer 200, got {status} {answered:?}"
        );
        let intent_id = text(&answered, "intentId");
        // The modelled `Id` shape: exactly 10 characters from `[0-9a-zA-Z]`.
        assert!(
            intent_id.len() == 10 && intent_id.chars().all(char::is_alphanumeric),
            "case {case:?}: expected a 10-character alphanumeric intentId, got {intent_id:?}"
        );
        assert_eq!(
            text(&answered, "botId"),
            bot_id,
            "case {case:?}: CreateIntent must report the bot it was called on, got {answered:?}"
        );
        assert_eq!(
            text(&answered, "localeId"),
            "en_US",
            "case {case:?}: CreateIntent must report the locale it was called on, got {answered:?}"
        );
        let created_at = seconds(&answered, "creationDateTime");
        assert!(
            created_at > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {answered:?}"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&answered, name),
                member(&request, name),
                "case {case:?}: CreateIntent must report the stored {name}, got {answered:?}"
            );
        }
        for name in HELD_LISTS {
            assert_eq!(
                member(&answered, name),
                member(&request, name),
                "case {case:?}: CreateIntent must report the {name} it was given, got \
                 {answered:?}"
            );
        }

        // READ BACK — the stored record, member for member.
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
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&described, name),
                member(&request, name),
                "case {case:?}: DescribeIntent must report the stored {name}, got {described:?}"
            );
        }
        for name in HELD_LISTS {
            assert_eq!(
                member(&described, name),
                member(&request, name),
                "case {case:?}: DescribeIntent must report the stored {name}, got \
                 {described:?}"
            );
        }
        assert_eq!(
            text(&described, "intentId"),
            intent_id,
            "case {case:?}: DescribeIntent must report the id it was asked about"
        );
        assert!(
            (seconds(&described, "creationDateTime") - created_at).abs() < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );
        assert!(
            seconds(&described, "lastUpdatedDateTime") >= created_at,
            "case {case:?}: lastUpdatedDateTime must not precede creationDateTime, got \
             {described:?}"
        );

        created.push((intent_id, text(&request, "intentName")));
    }

    // Every id minted is distinct, and each intent is still its own record: creating
    // later ones changed none of the earlier.
    let ids: BTreeSet<&str> = created.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "every CreateIntent must mint its own intentId, got {created:?}"
    );
    for (intent_id, intent_name) in &created {
        let (status, described) = call(
            &router,
            "GET",
            &intent_uri(&bot_id, "DRAFT", "en_US", intent_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "intent {intent_id}: every intent of the locale stays readable, got {status} \
             {described:?}"
        );
        assert_eq!(
            &text(&described, "intentName"),
            intent_name,
            "intent {intent_id}: expected the name it was created with, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_locale_is_created_with_its_fallback_intent_and_counts_what_it_holds() {
    let router = emulator();
    let bot_id = host_locale(&router, "IntentCountingBot", "en_US").await;

    // A locale nothing was added to already holds the fallback intent Amazon Lex adds
    // as it creates the locale, and no slot type.
    assert_eq!(
        locale_count(&router, &bot_id, "en_US", "intentsCount").await,
        1,
        "a new locale must report the fallback intent it was created with"
    );
    assert_eq!(
        locale_count(&router, &bot_id, "en_US", "slotTypesCount").await,
        0,
        "a new locale holds no slot type, which is a count of zero rather than no count"
    );

    // Each create adds exactly one to the count of the locale it was called on.
    let names = ["First_Intent", "Second_Intent", "Third_Intent"];
    for (added, name) in names.iter().enumerate() {
        create_intent(&router, &bot_id, "en_US", &intent_body(name, json!({}))).await;
        let expected = i64::try_from(added).expect("the row index is small") + 2;
        assert_eq!(
            locale_count(&router, &bot_id, "en_US", "intentsCount").await,
            expected,
            "after creating {name} the locale must report {expected} intents"
        );
    }

    // Another locale of the same bot counts only its own.
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("de_DE", 0.4, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the second locale must be created, got {created:?}");
    assert_eq!(
        locale_count(&router, &bot_id, "de_DE", "intentsCount").await,
        1,
        "a second locale holds its own fallback intent and none of the first locale's"
    );

    // The name Amazon Lex gave that fallback intent is taken, like any other.
    let (status, refused) = call(
        &router,
        "PUT",
        &intents_uri(&bot_id, "DRAFT", "en_US"),
        Some(intent_body(state::FALLBACK_INTENT_NAME, json!({}))),
    )
    .await;
    assert_eq!(
        status, 400,
        "the fallback intent's name is already in the locale, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException for a name the locale carries, got {refused:?}"
    );
}

#[tokio::test]
async fn an_intent_is_created_once_per_locale_and_only_on_a_locale_that_exists() {
    let router = emulator();
    let bot_id = host_locale(&router, "ScopedIntentBot", "en_US").await;
    let other_bot_id = host_locale(&router, "OtherScopedIntentBot", "en_US").await;

    let first = intent_body("OrderPizza", json!({ "description": "the stored intent" }));
    let intent_id = create_intent(&router, &bot_id, "en_US", &first).await;

    // A second create under the same name in the same locale is refused, and the stored
    // intent is left as it was.
    let (status, refused) = call(
        &router,
        "PUT",
        &intents_uri(&bot_id, "DRAFT", "en_US"),
        Some(intent_body("OrderPizza", json!({ "description": "the losing intent" }))),
    )
    .await;
    assert_eq!(
        status, 400,
        "a duplicate intentName must be refused, got {status} {refused:?}"
    );
    let (_, described) = call(
        &router,
        "GET",
        &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        None,
    )
    .await;
    assert_eq!(
        text(&described, "description"),
        "the stored intent",
        "the refused create must not have overwritten the stored intent, got {described:?}"
    );

    // The same name in ANOTHER locale, or on another bot, is a different intent.
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("ja_JP", 0.4, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the second locale must be created, got {created:?}");
    for (case, bot, locale) in [
        ("another locale of the same bot", bot_id.as_str(), "ja_JP"),
        ("another bot", other_bot_id.as_str(), "en_US"),
    ] {
        let (status, created) = call(
            &router,
            "PUT",
            &intents_uri(bot, "DRAFT", locale),
            Some(first.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same intentName elsewhere must be accepted, got {status} \
             {created:?}"
        );
    }

    // Every triple but its own misses.
    let misses: [(&str, String); 3] = [
        (
            "another bot",
            intent_uri(&other_bot_id, "DRAFT", "en_US", &intent_id),
        ),
        (
            "another locale of the same bot",
            intent_uri(&bot_id, "DRAFT", "ja_JP", &intent_id),
        ),
        (
            "an intent id never minted",
            intent_uri(&bot_id, "DRAFT", "en_US", "ABCDEFGHIJ"),
        ),
    ];
    for (case, uri) in misses {
        let (status, response) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeIntent to answer 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // An intent can only be added to a locale that exists.
    let (status, refused) = call(
        &router,
        "PUT",
        &intents_uri(&bot_id, "DRAFT", "nl_NL"),
        Some(first.clone()),
    )
    .await;
    assert_eq!(
        status, 412,
        "an intent for a locale that does not exist must be refused, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "PreconditionFailedException",
        "expected PreconditionFailedException for an unknown locale, got {refused:?}"
    );

    // An intent is defined inside its bot, so deleting the bot takes it too.
    let (status, _) = call(&router, "DELETE", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(status, 202, "deleting the host bot must succeed");
    let (status, response) = call(
        &router,
        "GET",
        &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a deleted bot's intent must be gone, got {status} {response:?}"
    );
}

#[tokio::test]
async fn a_submitted_build_is_recorded_and_settles_from_building_to_built() {
    let router = emulator();
    let bot_id = host_locale(&router, "BuildingBot", "en_US").await;
    // This suite's subject is the transition a submitted build makes, so the locale is
    // given the one thing a build needs to succeed: a custom intent with an utterance.
    // The buildability rule itself is asserted in the `locale_build_outcome` suite.
    create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_body(
            "OrderPizza",
            json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
        ),
    )
    .await;

    // Nothing has been submitted for a freshly created locale.
    let (_, described) = call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "en_US"), None).await;
    assert_eq!(
        member(&described, "lastBuildSubmittedDateTime"),
        Value::Null,
        "a locale never submitted for building has no build timestamp, got {described:?}"
    );

    let (status, built) = call(
        &router,
        "POST",
        &build_uri(&bot_id, "DRAFT", "en_US"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "expected BuildBotLocale to answer 202, got {status} {built:?}"
    );
    assert_eq!(
        text(&built, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILDING,
        "a submitted build reports the transient status, got {built:?}"
    );
    let submitted_at = seconds(&built, "lastBuildSubmittedDateTime");
    assert!(
        submitted_at > 0.0,
        "the submission instant must be reported, got {built:?}"
    );
    for name in ["botId", "botVersion", "localeId"] {
        assert_eq!(
            text(&built, name),
            match name {
                "botId" => bot_id.clone(),
                "botVersion" => "DRAFT".to_string(),
                _ => "en_US".to_string(),
            },
            "BuildBotLocale must report the {name} it was called on, got {built:?}"
        );
    }

    // Straight after the submission the caller still observes the transient status, and
    // the build timestamp is the one the submission reported.
    let (_, described) = call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "en_US"), None).await;
    assert_eq!(
        text(&described, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILDING,
        "a locale read back immediately must still be Building, got {described:?}"
    );
    assert!(
        (seconds(&described, "lastBuildSubmittedDateTime") - submitted_at).abs() < 1e-3,
        "DescribeBotLocale must report the stored build timestamp, got {described:?}"
    );

    // Past the due instant the transition the clock owed is materialised.
    tokio::time::sleep(state::BUILD_SETTLES_AFTER + Duration::from_millis(150)).await;
    let (_, described) = call(&router, "GET", &locale_uri(&bot_id, "DRAFT", "en_US"), None).await;
    assert_eq!(
        text(&described, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILT,
        "a built locale past its due instant must be Built, got {described:?}"
    );

    // A second build is submitted from the settled locale, and moves it back to Building
    // under a timestamp no earlier than the first.
    let (status, rebuilt) = call(
        &router,
        "POST",
        &build_uri(&bot_id, "DRAFT", "en_US"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "a locale that was built can be built again, got {status} {rebuilt:?}"
    );
    assert_eq!(
        text(&rebuilt, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILDING,
        "a resubmitted build reports the transient status, got {rebuilt:?}"
    );
    assert!(
        seconds(&rebuilt, "lastBuildSubmittedDateTime") >= submitted_at,
        "the second submission cannot precede the first, got {rebuilt:?}"
    );

    // A locale the store holds nothing under is not built.
    for (case, uri) in [
        ("a locale never created", build_uri(&bot_id, "DRAFT", "nl_NL")),
        ("a bot that does not exist", build_uri("ABCDEFGHIJ", "DRAFT", "en_US")),
    ] {
        let (status, refused) = call(&router, "POST", &uri, Some(json!({}))).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected BuildBotLocale to refuse with 412, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {refused:?}"
        );
    }
}
