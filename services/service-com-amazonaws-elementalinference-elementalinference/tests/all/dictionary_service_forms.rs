// SPDX-License-Identifier: CC0-1.0

//! The FORMS this service issues, end to end over the wire: the value set a refusal
//! states, the document an export carries, and the ids a create mints.
//!
//! Each suite asserts a rule over a TABLE of inputs rather than one case, because each
//! rule is about a form the SERVICE owns and not about anything a particular request
//! carried:
//!
//! * a value outside an enum's value set is refused with the constraint sentence, and
//!   the value set in it is stated in the order THIS SERVICE states that enum's values
//!   in — the same order for every member targeting the enum and every value rejected,
//!   and for an enum whose order the service has shown nothing of, the order the model
//!   declares;
//! * the entries payload a dictionary reports is the service's CANONICAL document — the
//!   entry list rendered compactly — so clients that state one entry list read one
//!   document back however each of them formatted its request;
//! * every id this service mints is drawn from the one form it issues ids in, for both
//!   kinds of resource and whatever the create carried.

use crate::harness::{
    call, create_body, create_path, pair, router, tagged_create_body, DICTIONARY_LANGUAGE,
    TAGGABLE_KINDS,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::ops::types::DICTIONARY_LANGUAGE_VALUES;
use service_com_amazonaws_elementalinference_elementalinference::state::DICTIONARY_LANGUAGE_VALUE_SET;
use smithy_server_runtime::MultiServiceRouter;

/// The value set this service states when it refuses a `language` outside
/// `DictionaryLanguage`, written out as the refusal states it — the order the service
/// issues for that enum, which is not the order the model declares its members in.
const STATED_LANGUAGE_SET: &str = "spa, por, deu, fra, ita, eng, ara";

/// The constraint sentence a value outside an enum's value set earns: the framework
/// envelope, the member at fault, the value it carried and the value set.
fn expected_enum_refusal(member: &str, value: &str, stated_set: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{stated_set}]"
    )
}

/// The `message` of a refusal, after asserting its status and error code.
fn refusal_message(label: &str, answered: (u16, Value)) -> String {
    let (status, body) = answered;
    assert_eq!(
        status, 400,
        "case {label:?}: a value outside an enum is a 400, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ValidationException"),
        "case {label:?}: that 400 is a ValidationException, got {body}"
    );
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the refusal carries a message, got {body}"))
        .to_string()
}

/// Create one dictionary and return its id.
async fn create_dictionary(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/v1/dictionary", Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "CreateDictionary({body}) answers 202, got {status} with {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created dictionary carries an id, got {created}"))
        .to_string()
}

/// `GET /v1/dictionary/{id}/entries/export`: the documented way to read a dictionary's
/// entries back.
async fn exported_entries(router: &MultiServiceRouter, label: &str, id: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/v1/dictionary/{id}/entries/export"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: ExportDictionaryEntries answers 200, got {status} with {body}"
    );
    body["entries"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the export carries an entries payload, got {body}"))
        .to_string()
}

/// The value set the refusal states holds exactly the values the MODEL declares, so the
/// stated order cannot drift into a different set of values than the one the service
/// accepts.
#[test]
fn the_stated_language_value_set_is_the_modelled_one_reordered() {
    let mut stated: Vec<&str> = DICTIONARY_LANGUAGE_VALUE_SET.to_vec();
    let listed = stated.join(", ");
    assert_eq!(
        listed, STATED_LANGUAGE_SET,
        "the service states the value set as {STATED_LANGUAGE_SET:?}, got {listed:?}"
    );
    stated.sort_unstable();
    let mut modelled: Vec<&str> = DICTIONARY_LANGUAGE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        stated, modelled,
        "the stated value set holds exactly the modelled values, got {stated:?} against \
         {modelled:?}"
    );
}

/// A `language` outside `DictionaryLanguage` is refused with the value set in the order
/// this service states it — the same order for every operation, every member and every
/// rejected value.
#[tokio::test]
async fn an_out_of_enum_language_states_the_value_set_in_the_services_order() {
    let router = router();
    let live = create_dictionary(
        &router,
        json!({"name": "statedset", "language": DICTIONARY_LANGUAGE}),
    )
    .await;

    // Values outside the enum that differ along everything a rejected value can differ
    // along: a word, a shorter code, the right value in the wrong case, a regional tag,
    // one with trailing space, the empty value, and a long value.
    let rejected = [
        "klingon",
        "en",
        "ENG",
        "eng-us",
        "spa ",
        "",
        "a-very-long-language-value-that-no-enum-lists",
    ];
    let mut stated_sets: Vec<String> = Vec::new();

    for value in rejected {
        // Both members of this service that carry a dictionary language: the create's
        // and the update's.
        let rows: Vec<(String, &'static str, String, Value)> = vec![
            (
                format!("CreateDictionary.language={value:?}"),
                "POST",
                "/v1/dictionary".to_string(),
                json!({"name": "statedsetnew", "language": value}),
            ),
            (
                format!("UpdateDictionary.language={value:?}"),
                "PATCH",
                format!("/v1/dictionary/{live}"),
                json!({"language": value}),
            ),
        ];
        for (label, method, uri, body) in rows {
            let message = refusal_message(&label, call(&router, method, &uri, Some(body)).await);
            let expected = expected_enum_refusal("language", value, STATED_LANGUAGE_SET);
            assert_eq!(
                message, expected,
                "case {label:?}: expected {expected:?}, got {message:?}"
            );
            let set = message
                .rsplit_once('[')
                .map(|(_, set)| set.trim_end_matches(']').to_string())
                .unwrap_or_else(|| panic!("case {label:?}: the refusal states a set, got {message:?}"));
            stated_sets.push(set);
        }
    }

    stated_sets.dedup();
    assert_eq!(
        stated_sets.len(),
        1,
        "the stated value set does not depend on the request, got {stated_sets:?}"
    );
}

/// An enum this service has published no order for keeps the order the MODEL declares:
/// the stated order is replaced for the value sets the service states differently, and
/// for nothing else.
#[tokio::test]
async fn an_enum_with_no_stated_order_keeps_the_models_declaration_order() {
    let router = router();

    // The declaration order of each enum, from
    // `smithy_query(query="shape",shape="DataSourceSport,TranscriptionLanguage,ProfanityFilterMode")`
    // — none of them alphabetical, so an order that had been sorted would show here.
    let rows: Vec<(&str, &str, String, Value, &str, &str)> = vec![
        (
            "SearchFixtures.sport",
            "POST",
            "/v1/fixtures".to_string(),
            json!({"sport": "cricket", "startDate": "2026-03-14", "endDate": "2026-03-15"}),
            "cricket",
            "basketball, american-football",
        ),
        (
            "CreateFeed.outputs.subtitling.language",
            "POST",
            "/v1/feed".to_string(),
            json!({
                "name": "statedsetfeed",
                "outputs": [{
                    "name": "sub1",
                    "status": "ENABLED",
                    "outputConfig": {"subtitling": {"language": "klingon"}}
                }]
            }),
            "klingon",
            "eng, eng-au, eng-gb, eng-us, fra, ita, deu, spa, por, ara",
        ),
        (
            "CreateFeed.outputs.subtitling.profanityFilter",
            "POST",
            "/v1/feed".to_string(),
            json!({
                "name": "statedsetfeed2",
                "outputs": [{
                    "name": "sub1",
                    "status": "ENABLED",
                    "outputConfig": {"subtitling": {"language": "eng", "profanityFilter": "LOUD"}}
                }]
            }),
            "LOUD",
            "DISABLED, CENSOR, DROP",
        ),
    ];

    for (label, method, uri, body, value, declared_set) in rows {
        let message = refusal_message(label, call(&router, method, &uri, Some(body)).await);
        let clause = format!("Member must satisfy enum value set: [{declared_set}]");
        assert!(
            message.contains(&clause),
            "case {label:?}: expected the declared set {clause:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!("Value '{value}' at ")),
            "case {label:?}: the refusal names the rejected value {value:?}, got {message:?}"
        );
    }
}

/// One row of the entries table: a payload as a client wrote it, and the document the
/// service reports for it.
struct EntriesCase {
    label: &'static str,
    /// Whether this row states the same entry list as the other rows marked so — the
    /// rows that must all export the SAME document.
    one_list: bool,
    submitted: String,
    exported: &'static str,
}

/// Payloads differing in formatting (compact, spaced, indented, tab-separated), in the
/// number of entries, in whether an entry carries alternates and how many, in the
/// characters an entry holds — and two that state no entry list at all.
fn entries_cases() -> Vec<EntriesCase> {
    // The same three-entry list, written four ways: compact, with a space after every
    // ':' and ',' (what a default JSON serializer emits), pretty-printed, and with tabs
    // and newlines between tokens.
    const THREE: &str = "[{\"content\":\"COLOUR\",\"sounds_like\":[\"color\"]},\
                         {\"content\":\"FAVOURITE\",\"sounds_like\":[\"favorite\"]},\
                         {\"content\":\"REALISE\",\"sounds_like\":[\"realize\"]}]";
    let three_spaced = THREE.replace(':', ": ").replace(',', ", ");
    let three_indented = "[\n  {\n    \"content\": \"COLOUR\",\n    \"sounds_like\": [\"color\"]\n  },\n  \
                          {\n    \"content\": \"FAVOURITE\",\n    \"sounds_like\": [\"favorite\"]\n  },\n  \
                          {\n    \"content\": \"REALISE\",\n    \"sounds_like\": [\"realize\"]\n  }\n]"
        .to_string();
    let three_tabbed = THREE.replace('{', "{\t").replace('}', "\n}");
    vec![
        EntriesCase {
            label: "the list written compactly",
            one_list: true,
            submitted: THREE.to_string(),
            exported: THREE,
        },
        EntriesCase {
            label: "the list with a space after every ':' and ','",
            one_list: true,
            submitted: three_spaced,
            exported: THREE,
        },
        EntriesCase {
            label: "the list pretty-printed over several lines",
            one_list: true,
            submitted: three_indented,
            exported: THREE,
        },
        EntriesCase {
            label: "the list with tabs and newlines between tokens",
            one_list: true,
            submitted: three_tabbed,
            exported: THREE,
        },
        EntriesCase {
            label: "one entry, spaced",
            one_list: false,
            submitted: "[{\"content\": \"COLOUR\", \"sounds_like\": [\"color\"]}]".to_string(),
            exported: "[{\"content\":\"COLOUR\",\"sounds_like\":[\"color\"]}]",
        },
        EntriesCase {
            label: "an entry with no alternates at all",
            one_list: false,
            submitted: "[{\"content\": \"birdie\"}]".to_string(),
            exported: "[{\"content\":\"birdie\"}]",
        },
        EntriesCase {
            label: "an entry with an empty list of alternates",
            one_list: false,
            submitted: "[{\"content\": \"birdie\", \"sounds_like\": []}]".to_string(),
            exported: "[{\"content\":\"birdie\",\"sounds_like\":[]}]",
        },
        EntriesCase {
            label: "an entry with three alternates",
            one_list: false,
            submitted: "[ { \"content\" : \"REALISE\" , \"sounds_like\" : [ \"realize\" , \
                        \"real eyes\" , \"realyse\" ] } ]"
                .to_string(),
            exported: "[{\"content\":\"REALISE\",\"sounds_like\":[\"realize\",\"real eyes\",\
                       \"realyse\"]}]",
        },
        EntriesCase {
            label: "an empty entry list",
            one_list: false,
            submitted: "[\n]".to_string(),
            exported: "[]",
        },
        EntriesCase {
            label: "an entry holding non-ASCII text",
            one_list: false,
            submitted: "[{\"content\": \"Fußball\", \"sounds_like\": [\"fussball\"]}]".to_string(),
            exported: "[{\"content\":\"Fußball\",\"sounds_like\":[\"fussball\"]}]",
        },
        EntriesCase {
            label: "an entry holding characters a JSON string escapes",
            one_list: false,
            submitted: "[{\"content\": \"say \\\"hi\\\"\\tnow\", \"sounds_like\": [\"say hi\"]}]"
                .to_string(),
            exported: "[{\"content\":\"say \\\"hi\\\"\\tnow\",\"sounds_like\":[\"say hi\"]}]",
        },
        // A payload that states no entry list has no list to render, so it is kept as
        // submitted — whitespace included.
        EntriesCase {
            label: "a newline-separated word list, which states no entry list",
            one_list: false,
            submitted: "Hawks\nFalcons\n".to_string(),
            exported: "Hawks\nFalcons\n",
        },
        EntriesCase {
            label: "a JSON array of bare strings, which states no entry list",
            one_list: false,
            submitted: "[\"COLOUR\", \"FAVOURITE\"]".to_string(),
            exported: "[\"COLOUR\", \"FAVOURITE\"]",
        },
    ]
}

/// The exported entries document is the service's canonical form of the entry list the
/// dictionary holds, whichever handler stored it and however the client formatted the
/// payload.
#[tokio::test]
async fn the_exported_entries_document_is_the_services_canonical_form() {
    let router = router();
    let mut one_list_exports: Vec<String> = Vec::new();

    for (index, case) in entries_cases().into_iter().enumerate() {
        let label = case.label;

        // Stored by CreateDictionary.
        let created = create_dictionary(
            &router,
            json!({
                "name": format!("canoncreate{index}"),
                "language": DICTIONARY_LANGUAGE,
                "entries": case.submitted,
            }),
        )
        .await;
        let exported = exported_entries(&router, label, &created).await;
        assert_eq!(
            exported, case.exported,
            "case {label:?}: the create stores the canonical document, expected {:?}, got \
             {exported:?}",
            case.exported
        );

        // And by UpdateDictionary, onto a dictionary created with another payload.
        let updated = create_dictionary(
            &router,
            json!({
                "name": format!("canonupdate{index}"),
                "language": DICTIONARY_LANGUAGE,
                "entries": "[{\"content\":\"BASE\"}]",
            }),
        )
        .await;
        let (status, patched) = call(
            &router,
            "PATCH",
            &format!("/v1/dictionary/{updated}"),
            Some(json!({"entries": case.submitted})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateDictionary answers 200, got {status} with {patched}"
        );
        let exported_after_update = exported_entries(&router, label, &updated).await;
        assert_eq!(
            exported_after_update, case.exported,
            "case {label:?}: the update stores the canonical document too, expected {:?}, got \
             {exported_after_update:?}",
            case.exported
        );

        if case.one_list {
            one_list_exports.push(exported);
        }
    }

    one_list_exports.dedup();
    assert_eq!(
        one_list_exports.len(),
        1,
        "every client that states one entry list reads one document back, got \
         {one_list_exports:?}"
    );
}

/// Whether `id` is drawn from the form this service issues ids in: 19 lower-case
/// alphanumeric characters.
fn is_minted_id(id: &str) -> bool {
    id.len() == 19
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// Create bodies of `kind` that differ along what the create carries, so the id's form
/// cannot depend on the request.
fn create_bodies(kind: &str, prefix: &str) -> Vec<(String, Value)> {
    let mut bodies: Vec<(String, Value)> = Vec::new();
    // The smallest body of the kind, and the same body carrying a tag set — both built
    // through the harness, which is where this crate's create bodies are defined.
    bodies.push((
        format!("{kind}: the smallest body"),
        create_body(kind, &format!("{prefix}{kind}0")),
    ));
    bodies.push((
        format!("{kind}: with tags"),
        tagged_create_body(
            kind,
            &format!("{prefix}{kind}1"),
            &[pair("team", "subtitles")],
        ),
    ));
    // A dictionary also varies along its language and its entries payload.
    for (index, language) in DICTIONARY_LANGUAGE_VALUES.iter().enumerate() {
        if kind != "dictionary" {
            break;
        }
        bodies.push((
            format!("dictionary: language={language}"),
            json!({
                "name": format!("{prefix}lang{index}"),
                "language": language,
                "entries": format!("[{{\"content\":\"{language}-term\"}}]"),
            }),
        ));
    }
    bodies
}

/// Every id this service mints is drawn from the one form it issues ids in — for both
/// kinds of resource it mints, and whatever the create carried.
#[tokio::test]
async fn every_minted_id_is_drawn_from_the_form_the_service_issues() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for kind in TAGGABLE_KINDS {
        for (label, body) in create_bodies(kind, "mintedid") {
            let (status, created) =
                call(&router, "POST", &create_path(kind), Some(body.clone())).await;
            assert_eq!(
                status, 202,
                "case {label:?}: the create answers 202, got {status} with {created}"
            );
            let id = created["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: the create returns an id, got {created}"))
                .to_string();
            assert!(
                is_minted_id(&id),
                "case {label:?}: the minted id is 19 lower-case alphanumeric characters, got \
                 {id:?}"
            );
            let arn = created["arn"].as_str().unwrap_or_default();
            assert!(
                arn.ends_with(&format!("{kind}/{id}")),
                "case {label:?}: the ARN names the minted id, got {arn:?} for {id:?}"
            );
            assert!(
                !minted.contains(&id),
                "case {label:?}: every minted id is distinct, got {id:?} twice"
            );
            minted.push(id);
        }
    }
}
