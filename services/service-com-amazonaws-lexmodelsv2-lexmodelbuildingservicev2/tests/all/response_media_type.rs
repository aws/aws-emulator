// SPDX-License-Identifier: CC0-1.0

//! The wire shape of this service's responses: the media type they announce, and the
//! resolution the `Timestamp` members they report are serialized at.
//!
//! # The media type
//!
//! Every reply of the Lex Models V2 model-building front end — whatever the status, and
//! whichever layer of the request walk produced it — is labelled with the awsJson `1.1`
//! media type, not the `application/json` the `restJson1` protocol trait its model
//! declares implies. A client that keys its parsing or its content negotiation off the
//! media type reads that label, so it is part of the wire contract.
//!
//! The rule is asserted over a TABLE spanning two dimensions, because the media type is
//! the SERVICE's and so belongs to neither one layer nor one operation:
//!
//! * the layers that can answer a request — a handler's success, a handler's modelled
//!   error, the boundary refusal the framework renders inside the walk, an operation
//!   nobody has written yet, and a request that names no modelled operation at all;
//! * the operations that answer them, across the bot, bot locale and slot type resources
//!   of this front end, on each status those operations reach here (200, 202, 400, 404,
//!   409, 412, 501).
//!
//! The pair itself is named once in `src/service.rs` (`RESPONSE_MEDIA_TYPE`) and read from
//! there, so this suite cannot drift from the service.
//!
//! # The resolution of a reported timestamp
//!
//! A modelled `Timestamp` member carries no `@timestampFormat` override anywhere in this
//! model, so under `restJson1` every one of them is serialized as an epoch-seconds
//! number — and the number this service reports carries at most millisecond resolution,
//! whatever resolution the clock the instant was read from has. A client that holds a
//! timestamp in a millisecond-resolution representation can therefore reproduce exactly
//! the value it was served, and compare a read's timestamp with the one the write
//! reported.
//!
//! That rule is the SERVICE's too — one conversion serves every member that reports a
//! stored instant — so it is asserted over a TABLE spanning the operations that report
//! one, across the bot, bot locale, slot type, intent and slot resources, and over each
//! kind of answer that carries one: a create, an update, a read and a listing. A second
//! table drives the conversion itself over the sub-millisecond parts a clock reading can
//! have.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::service::RESPONSE_MEDIA_TYPE;
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;

use crate::bot_lifecycle::{call, call_announcing, call_raw, create_body, emulator, text};
use crate::bot_locale::{locale_body, locale_uri, locales_uri};
use crate::intents_and_builds::{create_intent, intent_body, intent_uri, intents_uri};
use crate::slot_type::{
    create_slot_type, host_locale, slot_type_body, slot_type_uri, slot_types_uri,
};
use crate::slots::{slot_body, slot_uri, slots_uri};

/// One row of the table: how the request is made, and the status the layer under test
/// answers with.
struct Answered {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
}

#[tokio::test]
async fn every_response_announces_the_media_type_this_service_answers_under() {
    let router = emulator();

    // A bot and one of its locales, so the rows below can hit a handler that succeeds
    // and one that refuses from stored state.
    let (status, _, created) = call_announcing(
        &router,
        "PUT",
        "/bots",
        Some(create_body("MediaTypeBot", 300, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the host bot must be created, got {created:?}");
    let bot_id = created
        .get("botId")
        .and_then(Value::as_str)
        .expect("CreateBot reports the id it minted")
        .to_string();

    // A second bot with a locale of its own, carrying one slot type already, so the
    // slot-type rows below can read a stored record back, collide with a name the locale
    // holds, and ask for one no record carries.
    let slot_bot = host_locale(&router, "MediaTypeSlotTypeBot", "en_US").await;
    let stored_slot_type =
        create_slot_type(&router, &slot_bot, "en_US", &slot_type_body("City", json!({}))).await;
    let slot_types = slot_types_uri(&slot_bot, "DRAFT", "en_US");

    let rows = vec![
        Answered {
            case: "a create a handler served",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("en_US", 0.4, json!({}))),
            status: 202,
        },
        Answered {
            case: "a read a handler served",
            method: "GET",
            uri: locale_uri(&bot_id, "DRAFT", "en_US"),
            body: None,
            status: 200,
        },
        Answered {
            case: "a modelled error a handler raised",
            method: "GET",
            uri: "/bots/ABCDEFGHIJ".to_string(),
            body: None,
            status: 404,
        },
        Answered {
            case: "a locale a handler refused as a conflict",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("en_US", 0.4, json!({}))),
            status: 409,
        },
        Answered {
            case: "a precondition a handler refused",
            method: "POST",
            uri: locale_uri(&bot_id, "DRAFT", "nl_NL"),
            body: Some(json!({})),
            status: 412,
        },
        Answered {
            case: "a constraint the framework refused inside the walk",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("en_US", 2.5, json!({}))),
            status: 400,
        },
        Answered {
            case: "an absent required member the framework refused",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(json!({ "localeId": "de_DE" })),
            status: 400,
        },
        Answered {
            case: "an operation nobody has written yet",
            method: "POST",
            uri: format!("{}/intents", locale_uri(&bot_id, "DRAFT", "en_US")),
            body: Some(json!({})),
            status: 501,
        },
        Answered {
            case: "a request that names no modelled operation",
            method: "GET",
            uri: "/policy".to_string(),
            body: None,
            status: 501,
        },
        Answered {
            case: "a slot type a handler created",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body("Size", json!({ "description": "a size" }))),
            status: 200,
        },
        Answered {
            case: "a slot type a handler read back",
            method: "GET",
            uri: format!("{slot_types}/{stored_slot_type}"),
            body: None,
            status: 200,
        },
        Answered {
            case: "a slot type listing a handler served",
            method: "POST",
            uri: slot_types.clone(),
            body: Some(json!({})),
            status: 200,
        },
        Answered {
            case: "a slot type name a handler refused as already taken",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body("City", json!({}))),
            status: 400,
        },
        Answered {
            case: "a slot type for a locale a handler refused",
            method: "PUT",
            uri: slot_types_uri(&slot_bot, "DRAFT", "nl_NL"),
            body: Some(slot_type_body("Orphan", json!({}))),
            status: 412,
        },
        Answered {
            case: "a slot type id no record carries",
            method: "GET",
            uri: format!("{slot_types}/ZZZZZZZZZZ"),
            body: None,
            status: 404,
        },
        Answered {
            case: "a slot type name outside its modelled pattern, refused inside the walk",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body("not a slot type name!", json!({}))),
            status: 400,
        },
        Answered {
            case: "a slot type whose parent signature the model does not support",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body(
                "Parented",
                json!({ "parentSlotTypeSignature": "AMAZON.Number" }),
            )),
            status: 400,
        },
        Answered {
            case: "a slot type update nobody has written yet",
            method: "PUT",
            uri: format!("{slot_types}/{stored_slot_type}"),
            body: Some(slot_type_body("City", json!({}))),
            status: 501,
        },
    ];

    for row in rows {
        let (status, announced, body) =
            call_announcing(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body:?}",
            row.case, row.status
        );
        assert_eq!(
            announced.as_deref(),
            Some(RESPONSE_MEDIA_TYPE.announced()),
            "case {:?} (HTTP {status}): expected content-type {}, got {announced:?}",
            row.case,
            RESPONSE_MEDIA_TYPE.announced()
        );
        assert_ne!(
            announced.as_deref(),
            Some(RESPONSE_MEDIA_TYPE.implied()),
            "case {:?}: the protocol-implied media type must not reach a client",
            row.case
        );
    }
}

/// The most fractional digits an epoch-seconds timestamp of this service may carry:
/// millisecond resolution.
const MAX_FRACTIONAL_DIGITS: usize = 3;

/// Nanoseconds in one millisecond.
const NANOS_PER_MILLISECOND: u32 = 1_000_000;

/// The `Timestamp` members the operations under test report, by the wire name each
/// carries (`smithy_query(query="operation",operation="CreateBot,DescribeBot,UpdateBot,
/// ListBots,CreateBotLocale,DescribeBotLocale,CreateSlotType,DescribeSlotType,
/// ListSlotTypes,CreateIntent,UpdateIntent,DescribeIntent,ListIntents,CreateSlot,
/// DescribeSlot,ListSlots")`): none of them declares a `@timestampFormat`, so each is
/// serialized as an epoch-seconds number.
const TIMESTAMP_MEMBERS: [&str; 3] = [
    "creationDateTime",
    "lastUpdatedDateTime",
    "lastBuildSubmittedDateTime",
];

/// One row of the timestamp table: a request, the status it is answered with, and the
/// `Timestamp` members its answer must report.
struct Reports {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    members: &'static [&'static str],
}

/// The text of every number `document` carries under `member`, exactly as the response
/// serialized it, wherever in the document it appears — a member of the answer itself, or
/// one per entry of a summary list.
///
/// A scan over the document TEXT rather than a parse, because parsing a number into an
/// `f64` throws away the one thing under test here: how many digits were written.
fn wire_numbers(document: &str, member: &str) -> Vec<String> {
    let needle = format!("\"{member}\":");
    document
        .match_indices(needle.as_str())
        .map(|(at, _)| {
            document[at + needle.len()..]
                .trim_start()
                .chars()
                .take_while(|character| {
                    character.is_ascii_digit() || matches!(character, '.' | '-' | '+' | 'e' | 'E')
                })
                .collect::<String>()
        })
        .collect()
}

/// How many fractional digits a serialized number carries, or `None` when it is not
/// written as a plain decimal number at all — an exponent form is one AWS never writes
/// for an epoch-seconds timestamp.
fn fractional_digits(token: &str) -> Option<usize> {
    if token.contains('e') || token.contains('E') {
        return None;
    }
    Some(token.split_once('.').map_or(0, |(_, fraction)| fraction.len()))
}

/// Assert every timestamp member `document` reports is an epoch-seconds number of at
/// most millisecond resolution, and that the members `expected` names are among them.
fn assert_millisecond_resolution(case: &str, document: &str, expected: &[&str]) {
    for member in TIMESTAMP_MEMBERS {
        let reported = wire_numbers(document, member);
        assert!(
            !expected.contains(&member) || !reported.is_empty(),
            "case {case:?}: the answer must report {member}, got {document}"
        );
        for token in &reported {
            let digits = fractional_digits(token);
            assert!(
                digits.is_some_and(|digits| digits <= MAX_FRACTIONAL_DIGITS),
                "case {case:?}: {member} must be an epoch-seconds number of at most \
                 {MAX_FRACTIONAL_DIGITS} fractional digits, got {token}"
            );
        }
    }
}

#[tokio::test]
async fn every_reported_timestamp_is_epoch_seconds_at_millisecond_resolution() {
    let router = emulator();
    let bot_id = host_locale(&router, "TimestampBot", "en_US").await;
    let slot_type_id =
        create_slot_type(&router, &bot_id, "en_US", &slot_type_body("City", json!({}))).await;
    let intent_id =
        create_intent(&router, &bot_id, "en_US", &intent_body("OrderDrink", json!({}))).await;
    let (status, created_slot) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(slot_body("Size", json!({ "slotTypeId": slot_type_id }))),
    )
    .await;
    assert_eq!(
        status, 200,
        "the slot under test must be created, got {status} {created_slot:?}"
    );
    let slot_id = text(&created_slot, "slotId");

    let rows = vec![
        Reports {
            case: "a bot a create minted",
            method: "PUT",
            uri: "/bots".to_string(),
            body: Some(create_body("TimestampSecondBot", 300, json!({}))),
            status: 202,
            members: &["creationDateTime"],
        },
        Reports {
            case: "a bot a read reports",
            method: "GET",
            uri: format!("/bots/{bot_id}"),
            body: None,
            status: 200,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "a bot an update stored",
            method: "PUT",
            uri: format!("/bots/{bot_id}"),
            body: Some(create_body("TimestampBotRenamed", 400, json!({}))),
            status: 202,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "the bots a listing reports",
            method: "POST",
            uri: "/bots".to_string(),
            body: Some(json!({})),
            status: 200,
            members: &["lastUpdatedDateTime"],
        },
        Reports {
            case: "a bot locale a create minted",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("de_DE", 0.4, json!({}))),
            status: 202,
            members: &["creationDateTime"],
        },
        Reports {
            case: "a bot locale a read reports",
            method: "GET",
            uri: locale_uri(&bot_id, "DRAFT", "en_US"),
            body: None,
            status: 200,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "a slot type a create minted",
            method: "PUT",
            uri: slot_types_uri(&bot_id, "DRAFT", "en_US"),
            body: Some(slot_type_body("Colour", json!({}))),
            status: 200,
            members: &["creationDateTime"],
        },
        Reports {
            case: "a slot type a read reports",
            method: "GET",
            uri: slot_type_uri(&bot_id, "DRAFT", "en_US", &slot_type_id),
            body: None,
            status: 200,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "the slot types a listing reports",
            method: "POST",
            uri: slot_types_uri(&bot_id, "DRAFT", "en_US"),
            body: Some(json!({})),
            status: 200,
            members: &["lastUpdatedDateTime"],
        },
        Reports {
            case: "an intent a create minted",
            method: "PUT",
            uri: intents_uri(&bot_id, "DRAFT", "en_US"),
            body: Some(intent_body("OrderFood", json!({}))),
            status: 200,
            members: &["creationDateTime"],
        },
        Reports {
            case: "an intent an update stored",
            method: "PUT",
            uri: intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            body: Some(intent_body(
                "OrderDrink",
                json!({ "description": "a drink order" }),
            )),
            status: 200,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "an intent a read reports",
            method: "GET",
            uri: intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            body: None,
            status: 200,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "a slot a create minted",
            method: "PUT",
            uri: slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            body: Some(slot_body(
                "Quantity",
                json!({ "slotTypeId": "AMAZON.Number" }),
            )),
            status: 200,
            members: &["creationDateTime"],
        },
        Reports {
            case: "a slot a read reports",
            method: "GET",
            uri: slot_uri(&bot_id, "DRAFT", "en_US", &intent_id, &slot_id),
            body: None,
            status: 200,
            members: &["creationDateTime", "lastUpdatedDateTime"],
        },
        Reports {
            case: "the slots a listing reports",
            method: "POST",
            uri: slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            body: Some(json!({})),
            status: 200,
            members: &["lastUpdatedDateTime"],
        },
    ];

    for row in rows {
        let (status, _, document) = call_raw(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {document}",
            row.case, row.status
        );
        assert_millisecond_resolution(row.case, &document, row.members);
    }
}

#[tokio::test]
async fn a_stored_instant_goes_on_the_wire_truncated_to_whole_milliseconds() {
    // (case, the sub-second part of the stored reading, the sub-second part reported)
    let cases: [(&str, u32, u32); 8] = [
        ("an instant on the second", 0, 0),
        ("one nanosecond past the second", 1, 0),
        ("one nanosecond short of a millisecond", 999_999, 0),
        ("exactly one millisecond", 1_000_000, 1_000_000),
        ("one nanosecond short of two milliseconds", 1_999_999, 1_000_000),
        ("a reading already at millisecond resolution", 377_000_000, 377_000_000),
        ("a reading at sub-microsecond resolution", 477_175_700, 477_000_000),
        ("the last nanosecond of the second", 999_999_999, 999_000_000),
    ];
    // Any second of the epoch does: the truncation is of the sub-second part.
    let seconds = 1_700_000_000;

    for (case, stored, reported) in cases {
        let at = SystemTime::UNIX_EPOCH + Duration::new(seconds, stored);
        let wire = state::to_date_time(at);
        assert_eq!(
            wire.secs(),
            i64::try_from(seconds).expect("the second fits an i64"),
            "case {case:?}: truncating to milliseconds must not move the whole second, got {}",
            wire.secs()
        );
        assert_eq!(
            wire.subsec_nanos(),
            reported,
            "case {case:?}: expected the reported sub-second part to be {reported} ns, got {}",
            wire.subsec_nanos()
        );
        assert_eq!(
            wire.subsec_nanos() % NANOS_PER_MILLISECOND,
            0,
            "case {case:?}: a reported instant carries whole milliseconds only, got {} ns",
            wire.subsec_nanos()
        );
        assert!(
            wire.subsec_nanos() <= stored && stored - wire.subsec_nanos() < NANOS_PER_MILLISECOND,
            "case {case:?}: the reported instant must be the stored one truncated towards the \
             epoch by less than a millisecond, got {} ns for {stored} ns",
            wire.subsec_nanos()
        );
    }
}
