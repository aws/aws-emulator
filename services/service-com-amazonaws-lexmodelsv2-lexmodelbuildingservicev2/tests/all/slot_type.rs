// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 slot type slice: `CreateSlotType` and
//! `DescribeSlotType`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them. The harness and the
//! bot/locale setup are the bot lifecycle and bot locale suites', reused rather than
//! copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a create mints a `slotTypeId` matching the modelled `Id` shape and stores the
//!   definition under the bot, bot version and locale the URI names, so a read of that
//!   id reports back what was stored;
//! * a create that names no `valueSelectionSetting` resolves values under the strategy
//!   the member documents as the default;
//! * a slot type name is unique inside its locale: a second create under a name the
//!   locale carries is refused and stores nothing, while the same name in another locale
//!   or another bot is a different slot type;
//! * a slot type belongs to one locale of one bot version: no other triple reads it, and
//!   deleting the bot takes its slot types with it;
//! * a slot type can only be added to a bot and a locale that exist, a locale holds no
//!   more slot types than its quota, and a value outside a modelled constraint or
//!   outside what the model documents is refused and stores nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, seconds, text};
use crate::bot_locale::{host_bot, locale_body, locale_uri, locales_uri};

/// The members `CreateSlotType` accepts that both it and `DescribeSlotType` report back
/// unchanged. `valueSelectionSetting` is compared through [`expected_selection`]
/// instead, because a create that names none is answered with the documented default.
const ECHOED_BY_DESCRIBE: [&str; 6] = [
    "slotTypeName",
    "description",
    "slotTypeValues",
    "parentSlotTypeSignature",
    "externalSourceSetting",
    "compositeSlotTypeSetting",
];

/// `CreateSlotType`'s URI:
/// `PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/slottypes`.
pub(crate) fn slot_types_uri(bot_id: &str, bot_version: &str, locale_id: &str) -> String {
    format!("/bots/{bot_id}/botversions/{bot_version}/botlocales/{locale_id}/slottypes")
}

/// `DescribeSlotType`'s URI, which adds the `slotTypeId` to [`slot_types_uri`].
pub(crate) fn slot_type_uri(
    bot_id: &str,
    bot_version: &str,
    locale_id: &str,
    slot_type_id: &str,
) -> String {
    format!("{}/{slot_type_id}", slot_types_uri(bot_id, bot_version, locale_id))
}

/// A `CreateSlotType` request body carrying the required `slotTypeName` plus `extra`.
pub(crate) fn slot_type_body(name: &str, extra: Value) -> Value {
    let mut body = json!({ "slotTypeName": name });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// One `slotTypeValues` entry: a sample value and, when any are given, its synonyms.
fn slot_type_value(value: &str, synonyms: &[&str]) -> Value {
    let mut entry = json!({ "sampleValue": { "value": value } });
    if !synonyms.is_empty() {
        let synonyms: Value = synonyms.iter().map(|value| json!({ "value": value })).collect();
        entry["synonyms"] = synonyms;
    }
    entry
}

/// The `valueSelectionSetting` a create must report: the one the request named, or the
/// documented default strategy when the request named none.
fn expected_selection(request: &Value) -> Value {
    match member(request, "valueSelectionSetting") {
        Value::Null => json!({ "resolutionStrategy": state::DEFAULT_RESOLUTION_STRATEGY }),
        named => named,
    }
}

/// Create a bot and one locale on its `DRAFT` version, and return the minted `botId`.
pub(crate) async fn host_locale(
    router: &MultiServiceRouter,
    bot_name: &str,
    locale_id: &str,
) -> String {
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

/// Create a slot type, assert it was accepted, and return its minted id.
pub(crate) async fn create_slot_type(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    body: &Value,
) -> String {
    let (status, created) = call(
        router,
        "PUT",
        &slot_types_uri(bot_id, "DRAFT", locale_id),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSlotType {body:?}: expected 200, got {status} {created:?}"
    );
    text(&created, "slotTypeId")
}

/// The `slotTypesCount` `DescribeBotLocale` reports for one locale.
async fn slot_types_count(router: &MultiServiceRouter, bot_id: &str, locale_id: &str) -> i64 {
    let (status, described) =
        call(router, "GET", &locale_uri(bot_id, "DRAFT", locale_id), None).await;
    assert_eq!(
        status, 200,
        "expected DescribeBotLocale to answer 200, got {status} {described:?}"
    );
    member(&described, "slotTypesCount")
        .as_i64()
        .unwrap_or(i64::MIN)
}

/// A slot type name of exactly `len` characters satisfying the modelled `Name` pattern
/// `^([0-9a-zA-Z][_-]?){1,100}$`.
pub(crate) fn name_of_length(len: usize) -> String {
    "SlotType".chars().cycle().take(len).collect()
}

/// The table every round-trip assertion runs over. `sub_slot_type_id` is a slot type
/// already stored in the locale, so the composite row composes a real one.
///
/// Rows differ in which optional members are set, in the resolution strategy (every
/// member of the modelled `SlotValueResolutionStrategy` enum —
/// `smithy_query(query="shape",shape="SlotValueResolutionStrategy")`), in how many
/// values the slot type takes and how many synonyms each carries, in the length of the
/// name, the description and a sample value (each at its published maximum), and in the
/// source the values come from.
fn slot_type_cases(sub_slot_type_id: &str) -> Vec<(&'static str, Value)> {
    let many_values: Value = (0..50)
        .map(|index| slot_type_value(&format!("value{index}"), &[]))
        .collect();

    vec![
        (
            "only the required member",
            slot_type_body("MinimalSlotType", json!({})),
        ),
        (
            "values with synonyms, resolved to the top resolution",
            slot_type_body(
                "PizzaSize",
                json!({
                    "description": "how big the pizza is",
                    "slotTypeValues": [
                        slot_type_value("small", &["little", "tiny"]),
                        slot_type_value("medium", &["regular"]),
                        slot_type_value("large", &["big", "huge", "extra large"]),
                    ],
                    "valueSelectionSetting": { "resolutionStrategy": "TopResolution" },
                }),
            ),
        ),
        (
            "values concatenated rather than resolved",
            slot_type_body(
                "FlightRoute",
                json!({
                    "slotTypeValues": [
                        slot_type_value("Seattle", &[]),
                        slot_type_value("Boston", &[]),
                    ],
                    "valueSelectionSetting": { "resolutionStrategy": "Concatenation" },
                }),
            ),
        ),
        (
            "a slot type extending the one built-in parent signature, with a regex filter",
            slot_type_body(
                "Reservation_Code",
                json!({
                    "parentSlotTypeSignature": state::SUPPORTED_PARENT_SLOT_TYPE_SIGNATURE,
                    "valueSelectionSetting": {
                        "resolutionStrategy": "OriginalValue",
                        "regexFilter": { "pattern": "[0-9]{3}-[A-Z]{2}" },
                    },
                }),
            ),
        ),
        (
            "values used as a custom vocabulary",
            slot_type_body(
                "DrinkName",
                json!({
                    "slotTypeValues": [slot_type_value("flat white", &["latte"])],
                    "valueSelectionSetting": {
                        "resolutionStrategy": "TopResolution",
                        "advancedRecognitionSetting": {
                            "audioRecognitionStrategy": "UseSlotValuesAsCustomVocabulary",
                        },
                    },
                }),
            ),
        ),
        (
            "a description at the published maximum length",
            slot_type_body(
                "DescribedSlotType",
                json!({ "description": "d".repeat(2_000) }),
            ),
        ),
        (
            "a name at the published maximum length",
            slot_type_body(&name_of_length(100), json!({})),
        ),
        (
            "a name at the published minimum length",
            slot_type_body("A", json!({})),
        ),
        (
            "a sample value at the published maximum length",
            slot_type_body(
                "LongValueSlotType",
                json!({ "slotTypeValues": [slot_type_value(&"v".repeat(140), &[])] }),
            ),
        ),
        (
            "values taken from a grammar held outside the bot",
            slot_type_body(
                "GrammarSlotType",
                json!({
                    "externalSourceSetting": {
                        "grammarSlotTypeSetting": {
                            "source": {
                                "s3BucketName": "lex-grammar-bucket",
                                "s3ObjectKey": "grammars/city.grxml",
                            },
                        },
                    },
                }),
            ),
        ),
        (
            "a composite slot type over a built-in and a custom sub slot",
            slot_type_body(
                "CompositeSlotType",
                json!({
                    "compositeSlotTypeSetting": {
                        "subSlots": [
                            { "name": "Quantity", "slotTypeId": "AMAZON.Number" },
                            { "name": "Size", "slotTypeId": sub_slot_type_id },
                        ],
                    },
                }),
            ),
        ),
        ("fifty values", {
            slot_type_body(
                "ManyValuesSlotType",
                json!({ "slotTypeValues": many_values }),
            )
        }),
    ]
}

#[tokio::test]
async fn a_created_slot_type_is_stored_and_read_back_through_its_own_id() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotTypeRoundTripBot", "en_US").await;

    // The sub slot the composite row composes has to exist before it is named.
    let sub_slot_type_id = create_slot_type(
        &router,
        &bot_id,
        "en_US",
        &slot_type_body(
            "SubSlotType",
            json!({ "slotTypeValues": [slot_type_value("one", &[])] }),
        ),
    )
    .await;

    let mut ids: BTreeSet<String> = BTreeSet::new();
    ids.insert(sub_slot_type_id.clone());
    let mut stored: Vec<(&str, String, Value)> = Vec::new();

    for (case, request) in slot_type_cases(&sub_slot_type_id) {
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

        let slot_type_id = text(&created, "slotTypeId");
        assert_eq!(
            slot_type_id.chars().count(),
            10,
            "case {case:?}: slotTypeId must be 10 characters, got {slot_type_id:?}"
        );
        assert!(
            slot_type_id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: slotTypeId must match ^[0-9a-zA-Z]+$, got {slot_type_id:?}"
        );
        assert!(
            ids.insert(slot_type_id.clone()),
            "case {case:?}: slotTypeId {slot_type_id:?} was already handed out"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&created, name),
                member(&request, name),
                "case {case:?}: CreateSlotType must report the {name} it stored, got {created:?}"
            );
        }
        assert_eq!(
            member(&created, "valueSelectionSetting"),
            expected_selection(&request),
            "case {case:?}: expected the selection setting in effect, got {created:?}"
        );
        for (name, expected) in [
            ("botId", bot_id.as_str()),
            ("botVersion", "DRAFT"),
            ("localeId", "en_US"),
        ] {
            assert_eq!(
                text(&created, name),
                expected,
                "case {case:?}: CreateSlotType must report the {name} it was called on, got {created:?}"
            );
        }
        let created_at = seconds(&created, "creationDateTime");
        assert!(
            created_at > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {created:?}"
        );

        // READ BACK — the stored record, member for member.
        let (status, described) = call(
            &router,
            "GET",
            &slot_type_uri(&bot_id, "DRAFT", "en_US", &slot_type_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSlotType to answer 200, got {status} {described:?}"
        );
        assert_eq!(
            text(&described, "slotTypeId"),
            slot_type_id,
            "case {case:?}: DescribeSlotType must report the id it was asked about, got {described:?}"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&described, name),
                member(&request, name),
                "case {case:?}: DescribeSlotType must report the stored {name}, got {described:?}"
            );
        }
        assert_eq!(
            member(&described, "valueSelectionSetting"),
            expected_selection(&request),
            "case {case:?}: DescribeSlotType must report the stored selection setting, got {described:?}"
        );
        assert!(
            (seconds(&described, "creationDateTime") - created_at).abs() < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );
        assert!(
            seconds(&described, "lastUpdatedDateTime") >= created_at,
            "case {case:?}: lastUpdatedDateTime must not precede creationDateTime, got {described:?}"
        );

        stored.push((case, slot_type_id, request));
    }

    // Each slot type is its own record: creating later ones changed none of the earlier.
    for (case, slot_type_id, request) in &stored {
        let (status, described) = call(
            &router,
            "GET",
            &slot_type_uri(&bot_id, "DRAFT", "en_US", slot_type_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: every slot type of the locale stays readable, got {status} {described:?}"
        );
        assert_eq!(
            member(&described, "slotTypeName"),
            member(request, "slotTypeName"),
            "case {case:?}: expected the name it was created with, got {described:?}"
        );
        assert_eq!(
            member(&described, "slotTypeValues"),
            member(request, "slotTypeValues"),
            "case {case:?}: expected the values it was created with, got {described:?}"
        );
    }

    // The locale reports the slot types it holds: every row plus the sub slot type.
    let expected_count = i64::try_from(stored.len() + 1).expect("the row count fits an i64");
    assert_eq!(
        slot_types_count(&router, &bot_id, "en_US").await,
        expected_count,
        "DescribeBotLocale must count the slot types stored in the locale"
    );
}

#[tokio::test]
async fn a_slot_type_name_is_unique_inside_its_own_locale() {
    let router = emulator();
    let bot_id = host_locale(&router, "ConflictingSlotTypeBot", "en_US").await;
    let other_bot_id = host_locale(&router, "SecondSlotTypeBot", "en_US").await;

    // A second locale on the SAME bot, so the name can be re-used across locales too.
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("de_DE", 0.4, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the second locale must be created: {created:?}");

    let first = slot_type_body(
        "City",
        json!({
            "description": "the stored slot type",
            "slotTypeValues": [slot_type_value("Berlin", &[])],
        }),
    );
    let first_id = create_slot_type(&router, &bot_id, "en_US", &first).await;

    // A second create under a name the locale already carries is refused, whatever else
    // the request says.
    let duplicates: Vec<(&str, Value)> = vec![
        ("the same definition again", first.clone()),
        (
            "the same name with a different definition",
            slot_type_body(
                "City",
                json!({
                    "description": "the losing slot type",
                    "valueSelectionSetting": { "resolutionStrategy": "TopResolution" },
                }),
            ),
        ),
        (
            "the same name with no optional members at all",
            slot_type_body("City", json!({})),
        ),
    ];
    for (case, request) in &duplicates {
        let (status, response) = call(
            &router,
            "PUT",
            &slot_types_uri(&bot_id, "DRAFT", "en_US"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a duplicate slot type name must be refused, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // The refusals left the stored slot type as it was, and stored nothing beside it.
    let (_, described) = call(
        &router,
        "GET",
        &slot_type_uri(&bot_id, "DRAFT", "en_US", &first_id),
        None,
    )
    .await;
    assert_eq!(
        text(&described, "description"),
        "the stored slot type",
        "a refused create must not overwrite the stored slot type, got {described:?}"
    );
    assert_eq!(
        slot_types_count(&router, &bot_id, "en_US").await,
        1,
        "a refused create must store nothing"
    );

    // The same name in ANOTHER locale, or on another bot, is a different slot type.
    let elsewhere: Vec<(&str, &str, &str)> = vec![
        ("another locale of the same bot", bot_id.as_str(), "de_DE"),
        ("another bot", other_bot_id.as_str(), "en_US"),
    ];
    let mut ids = BTreeSet::from([first_id.clone()]);
    for (case, host, locale_id) in elsewhere {
        let (status, created) = call(
            &router,
            "PUT",
            &slot_types_uri(host, "DRAFT", locale_id),
            Some(first.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same name elsewhere must be accepted, got {status} {created:?}"
        );
        assert_eq!(
            text(&created, "localeId"),
            locale_id,
            "case {case:?}: the slot type belongs to the locale it was created in, got {created:?}"
        );
        assert!(
            ids.insert(text(&created, "slotTypeId")),
            "case {case:?}: a slot type elsewhere must get its own id, got {created:?}"
        );
    }
}

#[tokio::test]
async fn a_slot_type_is_reachable_only_under_its_own_bot_version_locale_and_id() {
    let router = emulator();
    let bot_id = host_locale(&router, "ScopedSlotTypeBot", "it_IT").await;
    let other_bot_id = host_locale(&router, "OtherScopedSlotTypeBot", "it_IT").await;

    let slot_type_id = create_slot_type(
        &router,
        &bot_id,
        "it_IT",
        &slot_type_body(
            "Province",
            json!({ "slotTypeValues": [slot_type_value("Milano", &[])] }),
        ),
    )
    .await;

    // Every triple but its own misses: another bot, a numbered bot version, another
    // locale, and an id no slot type carries.
    let misses: [(&str, String); 4] = [
        (
            "another bot",
            slot_type_uri(&other_bot_id, "DRAFT", "it_IT", &slot_type_id),
        ),
        (
            "a numbered bot version",
            slot_type_uri(&bot_id, "1", "it_IT", &slot_type_id),
        ),
        (
            "another locale",
            slot_type_uri(&bot_id, "DRAFT", "en_US", &slot_type_id),
        ),
        (
            "an id no slot type carries",
            slot_type_uri(&bot_id, "DRAFT", "it_IT", "ZZZZZZZZZZ"),
        ),
    ];
    for (case, uri) in misses {
        let (status, response) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeSlotType to answer 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // A slot type is defined inside its bot, so deleting the bot takes it too.
    let (status, _) = call(&router, "DELETE", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(status, 202, "deleting the host bot must succeed");
    let (status, response) = call(
        &router,
        "GET",
        &slot_type_uri(&bot_id, "DRAFT", "it_IT", &slot_type_id),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a deleted bot's slot type must be gone, got {status} {response:?}"
    );
}

#[tokio::test]
async fn a_slot_type_is_added_only_to_a_bot_and_locale_that_exist() {
    let router = emulator();
    let bot_id = host_locale(&router, "PreconditionSlotTypeBot", "en_US").await;

    // The bot the URI names does not exist, so there is nothing to define a slot type in.
    let (status, response) = call(
        &router,
        "PUT",
        &slot_types_uri("ABCDEFGHIJ", "DRAFT", "en_US"),
        Some(slot_type_body("Orphan", json!({}))),
    )
    .await;
    assert_eq!(
        status, 412,
        "a slot type for an unknown bot must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "PreconditionFailedException",
        "expected PreconditionFailedException for an unknown bot, got {response:?}"
    );

    // The bot exists but the locale was never added to it.
    for locale_id in ["de_DE", "ja_JP"] {
        let (status, response) = call(
            &router,
            "PUT",
            &slot_types_uri(&bot_id, "DRAFT", locale_id),
            Some(slot_type_body("Orphan", json!({}))),
        )
        .await;
        assert_eq!(
            status, 412,
            "locale {locale_id}: a slot type for a locale that was never created must be refused, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "PreconditionFailedException",
            "locale {locale_id}: expected PreconditionFailedException, got {response:?}"
        );
    }

    assert_eq!(
        slot_types_count(&router, &bot_id, "en_US").await,
        0,
        "a refused create must store nothing in the bot's own locale"
    );
}

#[tokio::test]
async fn a_locale_holds_no_more_slot_types_than_its_quota() {
    let router = emulator();
    let bot_id = host_locale(&router, "QuotaSlotTypeBot", "en_US").await;

    // Fill the locale to its quota, each slot type under a name of its own.
    for index in 0..state::SLOT_TYPES_PER_LOCALE_QUOTA {
        let (status, created) = call(
            &router,
            "PUT",
            &slot_types_uri(&bot_id, "DRAFT", "en_US"),
            Some(slot_type_body(&format!("SlotType{index}"), json!({}))),
        )
        .await;
        assert_eq!(
            status, 200,
            "slot type {index}: expected 200 below the quota, got {status} {created:?}"
        );
    }

    // One past the quota is refused with the modelled quota error, and stores nothing.
    let (status, response) = call(
        &router,
        "PUT",
        &slot_types_uri(&bot_id, "DRAFT", "en_US"),
        Some(slot_type_body("OnePastTheQuota", json!({}))),
    )
    .await;
    assert_eq!(
        status, 402,
        "a create past the quota must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException past the quota, got {response:?}"
    );
    let quota = i64::try_from(state::SLOT_TYPES_PER_LOCALE_QUOTA).expect("the quota fits an i64");
    assert_eq!(
        slot_types_count(&router, &bot_id, "en_US").await,
        quota,
        "a refused create must store nothing"
    );

    // The quota is per locale, so another locale of the same bot still takes one.
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("de_DE", 0.4, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the second locale must be created: {created:?}");
    let (status, created) = call(
        &router,
        "PUT",
        &slot_types_uri(&bot_id, "DRAFT", "de_DE"),
        Some(slot_type_body("SlotType0", json!({}))),
    )
    .await;
    assert_eq!(
        status, 200,
        "another locale of the same bot must still accept a slot type, got {status} {created:?}"
    );
}

#[tokio::test]
async fn requests_outside_the_accepted_values_are_refused_and_store_nothing() {
    let router = emulator();
    let bot_id = host_locale(&router, "ConstrainedSlotTypeBot", "en_US").await;

    // Each row is refused as a 400 `ValidationException` rather than stored: a name or a
    // description past a published bound, a name the modelled pattern rejects, a value
    // outside a modelled enum, a list shorter than the modelled minimum, and a parent
    // signature the member's documentation does not support.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a name longer than the published maximum",
            slot_type_body(&name_of_length(101), json!({})),
        ),
        (
            "a name the modelled pattern rejects",
            slot_type_body("Pizza Size", json!({})),
        ),
        (
            "a description longer than the published maximum",
            slot_type_body("TooDescribed", json!({ "description": "d".repeat(2_001) })),
        ),
        (
            "a sample value longer than the published maximum",
            slot_type_body(
                "TooLongValue",
                json!({ "slotTypeValues": [slot_type_value(&"v".repeat(141), &[])] }),
            ),
        ),
        (
            "an empty value list, shorter than the modelled minimum",
            slot_type_body("NoValues", json!({ "slotTypeValues": [] })),
        ),
        (
            "a resolution strategy outside the modelled enum",
            slot_type_body(
                "BadStrategy",
                json!({ "valueSelectionSetting": { "resolutionStrategy": "FirstValue" } }),
            ),
        ),
        (
            "an audio recognition strategy outside the modelled enum",
            slot_type_body(
                "BadRecognition",
                json!({
                    "valueSelectionSetting": {
                        "resolutionStrategy": "TopResolution",
                        "advancedRecognitionSetting": {
                            "audioRecognitionStrategy": "UseSlotValuesAsGrammar",
                        },
                    },
                }),
            ),
        ),
        (
            "a built-in parent signature the member does not support",
            slot_type_body(
                "ParentedSlotType",
                json!({ "parentSlotTypeSignature": "AMAZON.Number" }),
            ),
        ),
    ];

    for (case, request) in &cases {
        let (status, response) = call(
            &router,
            "PUT",
            &slot_types_uri(&bot_id, "DRAFT", "en_US"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // None of the refusals stored a slot type.
    assert_eq!(
        slot_types_count(&router, &bot_id, "en_US").await,
        0,
        "a refused create must store nothing"
    );
}
