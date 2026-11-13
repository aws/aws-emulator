// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the OUTCOME of a Lex Models V2 locale build: which terminal
//! `botLocaleStatus` a `BuildBotLocale` settles on, and what `DescribeBotLocale` then
//! reports about it.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot/locale
//! setup and the intent helpers are the other suites', reused rather than copied.
//!
//! The rule under test, asserted over a TABLE of locale contents rather than one case:
//! a submitted build settles on the terminal status the locale's own content determines —
//! `Built` for a locale holding at least one custom intent with a valid sample utterance,
//! and `Failed`, carrying the one failure reason that names the locale, for a locale
//! holding none — and the reasons become readable only with the status that carries them.
//! The rows differ in how the locale falls on either side of that rule: no intent but the
//! one Amazon Lex adds, a custom intent with no utterance, one whose only utterance is
//! blank, a built-in-based intent that does carry utterances, one custom intent with an
//! utterance, several, and a second locale id, so what is asserted is the rule and not one
//! recorded locale.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, member, seconds, text};
use crate::bot_locale::locale_uri;
use crate::intents_and_builds::{build_uri, create_intent, intent_body, intent_uri};
use crate::slot_type::host_locale;

/// One row of the table: the intents a locale is given, and whether the service can build
/// it.
struct Row {
    case: &'static str,
    locale_id: &'static str,
    /// `CreateIntent` bodies, applied in order to the locale under test.
    intents: Vec<Value>,
    buildable: bool,
}

/// The locale contents the rule is asserted over.
fn rows() -> Vec<Row> {
    let many: Value = (0..5)
        .map(|index| json!({ "utterance": format!("I want option {index}") }))
        .collect();

    vec![
        Row {
            case: "nothing but the fallback intent Amazon Lex adds to a new locale",
            locale_id: "en_US",
            intents: vec![],
            buildable: false,
        },
        Row {
            case: "a custom intent with no sample utterance at all",
            locale_id: "en_US",
            intents: vec![intent_body("OrderPizza", json!({}))],
            buildable: false,
        },
        Row {
            case: "a custom intent whose only utterance carries no text",
            locale_id: "en_US",
            intents: vec![intent_body(
                "OrderPizza",
                json!({ "sampleUtterances": [{ "utterance": "   " }] }),
            )],
            buildable: false,
        },
        Row {
            case: "an intent built on a built-in signature, utterances and all, and no \
                   custom intent",
            locale_id: "en_US",
            intents: vec![intent_body(
                "Help_Intent",
                json!({
                    "parentIntentSignature": "AMAZON.HelpIntent",
                    "sampleUtterances": [{ "utterance": "I need help" }],
                }),
            )],
            buildable: false,
        },
        Row {
            case: "one custom intent with one utterance",
            locale_id: "en_US",
            intents: vec![intent_body(
                "OrderPizza",
                json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
            )],
            buildable: true,
        },
        Row {
            case: "one custom intent with several utterances, beside one with none",
            locale_id: "en_US",
            intents: vec![
                intent_body("CancelOrder", json!({})),
                intent_body("PickOption", json!({ "sampleUtterances": many })),
            ],
            buildable: true,
        },
        Row {
            case: "another locale of the service, unbuildable",
            locale_id: "de_DE",
            intents: vec![intent_body("BestellePizza", json!({}))],
            buildable: false,
        },
        Row {
            case: "another locale of the service, buildable",
            locale_id: "de_DE",
            intents: vec![intent_body(
                "BestellePizza",
                json!({ "sampleUtterances": [{ "utterance": "Ich moechte eine Pizza" }] }),
            )],
            buildable: true,
        },
    ]
}

/// Submit a build and return the status it reports.
async fn submit_build(router: &MultiServiceRouter, bot_id: &str, locale_id: &str) -> String {
    let (status, built) = call(
        router,
        "POST",
        &build_uri(bot_id, "DRAFT", locale_id),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 202,
        "locale {locale_id}: expected BuildBotLocale to answer 202, got {status} {built:?}"
    );
    text(&built, "botLocaleStatus")
}

/// `DescribeBotLocale`, past the span the clock owes the build, so the terminal status is
/// the one reported.
async fn settled_locale(router: &MultiServiceRouter, bot_id: &str, locale_id: &str) -> Value {
    tokio::time::sleep(state::BUILD_SETTLES_AFTER + Duration::from_millis(150)).await;
    let (status, described) = call(router, "GET", &locale_uri(bot_id, "DRAFT", locale_id), None).await;
    assert_eq!(
        status, 200,
        "locale {locale_id}: expected DescribeBotLocale to answer 200, got {status} {described:?}"
    );
    described
}

#[tokio::test]
async fn a_build_settles_on_the_outcome_the_locale_content_determines() {
    for row in rows() {
        let router = emulator();
        let bot_id = host_locale(&router, "BuildOutcomeBot", row.locale_id).await;
        for intent in &row.intents {
            create_intent(&router, &bot_id, row.locale_id, intent).await;
        }

        let submitted = submit_build(&router, &bot_id, row.locale_id).await;
        assert_eq!(
            submitted,
            state::BOT_LOCALE_STATUS_BUILDING,
            "case {:?}: a submitted build reports the transient status, got {submitted:?}",
            row.case
        );

        // While the build is in flight the outcome is not a caller's to read yet.
        let (_, building) = call(
            &router,
            "GET",
            &locale_uri(&bot_id, "DRAFT", row.locale_id),
            None,
        )
        .await;
        assert_eq!(
            member(&building, "failureReasons"),
            Value::Null,
            "case {:?}: a build still in flight reports no failureReasons, got {building:?}",
            row.case
        );

        let described = settled_locale(&router, &bot_id, row.locale_id).await;
        let expected_status = if row.buildable {
            state::BOT_LOCALE_STATUS_BUILT
        } else {
            state::BOT_LOCALE_STATUS_FAILED
        };
        assert_eq!(
            text(&described, "botLocaleStatus"),
            expected_status,
            "case {:?}: expected the build to settle at {expected_status}, got {described:?}",
            row.case
        );

        let expected_reasons = if row.buildable {
            Value::Null
        } else {
            json!([state::no_utterances_failure_reason(row.locale_id)])
        };
        assert_eq!(
            member(&described, "failureReasons"),
            expected_reasons,
            "case {:?}: expected failureReasons {expected_reasons:?}, got {described:?}",
            row.case
        );

        // The build RAN either way, so the locale carries the instant it was submitted.
        assert!(
            seconds(&described, "lastBuildSubmittedDateTime") > 0.0,
            "case {:?}: a settled build must report when it was submitted, got {described:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_failure_reason_names_the_locale_the_build_was_asked_for() {
    let router = emulator();
    // Two locales of one bot, each holding only a custom intent with no utterance: the
    // reason each reports names its own locale and nothing else of the request.
    let bot_id = host_locale(&router, "FailureReasonBot", "en_US").await;
    let (status, created) = call(
        &router,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales"),
        Some(json!({ "localeId": "ja_JP", "nluIntentConfidenceThreshold": 0.4 })),
    )
    .await;
    assert_eq!(status, 202, "the second locale must be created, got {created:?}");

    for locale_id in ["en_US", "ja_JP"] {
        create_intent(
            &router,
            &bot_id,
            locale_id,
            &intent_body("OrderPizza", json!({})),
        )
        .await;
        submit_build(&router, &bot_id, locale_id).await;
    }

    for locale_id in ["en_US", "ja_JP"] {
        let described = settled_locale(&router, &bot_id, locale_id).await;
        let reasons = member(&described, "failureReasons");
        assert_eq!(
            reasons,
            json!([state::no_utterances_failure_reason(locale_id)]),
            "locale {locale_id}: the reason names the locale of the build, got {reasons:?}"
        );
        let rendered = reasons.to_string();
        assert!(
            !rendered.contains(&bot_id) && !rendered.contains("OrderPizza"),
            "locale {locale_id}: the reason carries no other identifier, got {rendered:?}"
        );
    }
}

#[tokio::test]
async fn a_rebuild_reports_the_outcome_of_the_content_it_was_given() {
    let router = emulator();
    let bot_id = host_locale(&router, "RebuildOutcomeBot", "en_US").await;
    let intent_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_body("OrderPizza", json!({})),
    )
    .await;

    // A locale with nothing to recognise fails the build.
    submit_build(&router, &bot_id, "en_US").await;
    let failed = settled_locale(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&failed, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_FAILED,
        "a locale with no utterance must fail its build, got {failed:?}"
    );

    // Supplying the missing utterance and rebuilding settles the same locale at Built,
    // and the reasons of the failed build are gone with it.
    let with_utterance = intent_body(
        "OrderPizza",
        json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
    );
    let (status, updated) = call(
        &router,
        "PUT",
        &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(with_utterance),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UpdateIntent to answer 200, got {status} {updated:?}"
    );
    assert_eq!(
        member(&updated, "sampleUtterances"),
        json!([{ "utterance": "I want to order a pizza" }]),
        "UpdateIntent must report the utterances it stored, got {updated:?}"
    );

    submit_build(&router, &bot_id, "en_US").await;
    let built = settled_locale(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&built, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_BUILT,
        "the rebuilt locale must be Built, got {built:?}"
    );
    assert_eq!(
        member(&built, "failureReasons"),
        Value::Null,
        "a locale that built carries no failureReasons, got {built:?}"
    );

    // And the other way: taking the utterance away again fails the next build, which is
    // the same rule read from the other side.
    let (status, cleared) = call(
        &router,
        "PUT",
        &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(intent_body("OrderPizza", json!({}))),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UpdateIntent to answer 200, got {status} {cleared:?}"
    );
    assert_eq!(
        member(&cleared, "sampleUtterances"),
        Value::Null,
        "an update that names no utterance leaves the intent none at all, got {cleared:?}"
    );

    submit_build(&router, &bot_id, "en_US").await;
    let failed_again = settled_locale(&router, &bot_id, "en_US").await;
    assert_eq!(
        text(&failed_again, "botLocaleStatus"),
        state::BOT_LOCALE_STATUS_FAILED,
        "the locale must fail the build again, got {failed_again:?}"
    );
    assert_eq!(
        member(&failed_again, "failureReasons"),
        json!([state::no_utterances_failure_reason("en_US")]),
        "the failed rebuild must report the reason again, got {failed_again:?}"
    );
}
