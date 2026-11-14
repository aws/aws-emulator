// SPDX-License-Identifier: CC0-1.0

//! What `CreateDocumentationPart` PERSISTS, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], [`Answer`], `create_api`,
//! `string_member`, `page_items` and `assert_failure` are this crate's one wire-test
//! harness, reused from `rest_api_lifecycle`; the few request builders below are this
//! suite's own, so it touches no sibling suite.
//!
//! The sibling `documentation_parts` suite covers the location rules (each entity type's
//! valid fields, their documented defaults and the refusal of a field the type makes
//! invalid), the filters and pagination of the readers, and an import. This suite pins
//! the rules of the CREATE itself that it does not:
//!
//! * a documentation part belongs to the API the request names, so the record's identity
//!   is that API plus its completed location — one location documented under two APIs is
//!   two parts, each readable only through its own API;
//! * `restApiId` naming no stored API is the modelled 404, stating the identifier, and
//!   persists nothing;
//! * the content map is stored as the TEXT the request sent, character for character,
//!   over a table of JSON objects differing along escaping, non-ASCII content, nesting,
//!   key order and insignificant whitespace;
//! * the content map's stored length is bounded in CHARACTERS, asserted at the bound and
//!   one past it with both single-byte and multi-byte fillers;
//! * a location's `statusCode` spans its modelled pattern's range, asserted on each
//!   published bound, inside it, on the wildcard and one past each bound.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::create_documentation_part::MAX_PROPERTIES_CHARS;

use crate::rest_api_lifecycle::{
    assert_failure, create_api, page_items, string_member, Answer, Gateway,
};

/// The modelled error codes these cases assert.
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// What `DocumentationPartLocation$statusCode` documents for a location that names no
/// status code: "The default value is `*` for any status code" — the literal the model
/// states, not the implementation's constant for it.
const DOCUMENTED_STATUS_CODE_DEFAULT: &str = "*";

/// Identifiers no stored API has. The rows differ along what the rule generalises over:
/// the length (one character, the ten characters API Gateway mints, longer), the alphabet
/// (letters, digits, a mix, a hyphen) and the case — the message quotes what the request
/// sent, and `restApiId` is a plain `String` with no `@pattern`
/// (`smithy_query(query="operation", operation="CreateDocumentationPart")`), so every row
/// reaches the lookup rather than the boundary validator.
const UNKNOWN_IDS: &[&str] = &[
    "q",
    "zzzzzzzzzz",
    "0123456789",
    "Ab3Xy9Zq1W",
    "no-such-api-00000001",
];

/// The sentence this service states for an API identifier that resolves to no API; the
/// `rest_api_identifier_refusals` suite states the same rule across the service's other
/// operations. The account segment is the emulator's one account
/// (`engine::emulator_account_id`), never a literal.
fn no_such_api(rest_api_id: &str) -> String {
    format!(
        "Invalid API identifier specified {}:{rest_api_id}",
        engine::emulator_account_id()
    )
}

/// A content map, as the JSON string `DocumentationPart$properties` documents ("The map
/// must be encoded as a JSON string").
fn content_map(description: &str) -> String {
    json!({ "description": description }).to_string()
}

/// `CreateDocumentationPart` with `location` and `properties` sent as given.
async fn create_part(gateway: &Gateway, api: &str, location: Value, properties: &str) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/restapis/{api}/documentation/parts"),
            Some(json!({ "location": location, "properties": properties })),
        )
        .await
}

/// `CreateDocumentationPart`, asserting the modelled 201 and returning the minted id.
async fn created_part_id(
    gateway: &Gateway,
    api: &str,
    location: Value,
    properties: &str,
    case: &str,
) -> String {
    let answer = create_part(gateway, api, location, properties).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", case)
}

/// `GetDocumentationPart` for one part of one API.
async fn get_part(gateway: &Gateway, api: &str, id: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/parts/{id}"),
            None,
        )
        .await
}

/// The ids `GetDocumentationParts` reports for one API, in creation order, asserting the
/// modelled 200. The page size is the documented maximum, so one page holds every part a
/// case created.
async fn listed_ids(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/parts?limit=500"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .iter()
        .map(|item| string_member(item, "id", case))
        .collect()
}

/// Locations spanning the entity types a part can target, each already setting every
/// member its type makes valid, so a row's stored location is the row's own. Source:
/// `DocumentationPartType`'s value set and each `DocumentationPartLocation` member's
/// documentation (`smithy_query(query="shape", shape="DocumentationPartLocation")`).
fn locations() -> Vec<(&'static str, Value)> {
    vec![
        ("API", json!({ "type": "API" })),
        ("MODEL", json!({ "type": "MODEL", "name": "Pet" })),
        ("RESOURCE", json!({ "type": "RESOURCE", "path": "/pets" })),
        (
            "METHOD",
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
        ),
        (
            "RESPONSE_HEADER",
            json!({
                "type": "RESPONSE_HEADER",
                "path": "/pets",
                "method": "GET",
                "statusCode": "200",
                "name": "x-count",
            }),
        ),
    ]
}

#[tokio::test]
async fn a_documentation_part_belongs_to_the_api_the_request_names() {
    let gateway = Gateway::new();
    let (first, _root) = create_api(&gateway, "documentation-owner-first").await;
    let (second, _root) = create_api(&gateway, "documentation-owner-second").await;

    let mut first_ids: Vec<String> = Vec::new();
    let mut second_ids: Vec<String> = Vec::new();
    for (entity_type, location) in locations() {
        // The SAME location under both APIs: each is the documentation of a different
        // API's entity, so neither create is a conflict with the other.
        for (api, ids) in [(&first, &mut first_ids), (&second, &mut second_ids)] {
            let case = format!("type={entity_type}, api={api}");
            let properties = content_map(entity_type);
            let id = created_part_id(&gateway, api, location.clone(), &properties, &case).await;
            let read = get_part(&gateway, api, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: GetDocumentationPart expected 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["location"], location,
                "case {case}: expected location {location:?}, got {:?}",
                read.body["location"]
            );
            assert_eq!(
                read.body["properties"].as_str(),
                Some(properties.as_str()),
                "case {case}: expected properties {properties:?}, got {:?}",
                read.body["properties"]
            );
            ids.push(id);
        }
    }

    // Each API reports exactly its own parts. The collection's own order is the ADDRESS
    // of the entity each part documents (`collection_page_envelope` pins that order), not
    // the order the parts were created in, so what is compared here is which parts each
    // API holds.
    let mut expected = first_ids.clone();
    expected.sort();
    let mut listed = listed_ids(&gateway, &first, "the first api's collection").await;
    listed.sort();
    assert_eq!(
        listed, expected,
        "case the first api's collection: expected {expected:?}, got {listed:?}"
    );
    let mut expected = second_ids.clone();
    expected.sort();
    let mut listed = listed_ids(&gateway, &second, "the second api's collection").await;
    listed.sort();
    assert_eq!(
        listed, expected,
        "case the second api's collection: expected {expected:?}, got {listed:?}"
    );

    // A part is addressed through its OWN api: the other api does not hold it, however
    // identical the two locations are.
    for (owner, other, ids) in [
        (&first, &second, &first_ids),
        (&second, &first, &second_ids),
    ] {
        for id in ids {
            let case = format!("part {id} of api {owner} read through api {other}");
            let answer = get_part(&gateway, other, id).await;
            assert_failure(&answer, 404, NOT_FOUND, &case);
        }
    }

    // Every minted identifier is distinct, across both APIs.
    let mut minted: Vec<String> = first_ids.iter().chain(second_ids.iter()).cloned().collect();
    let total = minted.len();
    minted.sort_unstable();
    minted.dedup();
    assert_eq!(
        minted.len(),
        total,
        "case minted identifiers: expected {total} distinct ids, got {minted:?}"
    );
}

#[tokio::test]
async fn creating_a_documentation_part_under_an_unknown_api_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let (stored, _root) = create_api(&gateway, "documentation-unknown-api").await;

    // Identifiers no stored API has, spanning the lengths, alphabets and cases the rule
    // generalises over (see [`UNKNOWN_IDS`]).
    for api in UNKNOWN_IDS {
        let case = format!("CreateDocumentationPart under api {api:?}");
        let answer = create_part(
            &gateway,
            api,
            json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
            &content_map("list pets"),
        )
        .await;
        assert_failure(&answer, 404, NOT_FOUND, &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(no_such_api(api).as_str()),
            "case {case}: expected the message {:?}, got {:?}",
            no_such_api(api),
            answer.body["message"]
        );
    }

    // The same request against the STORED identifier is served, so the refusals above are
    // about the identifier and not about the request.
    let case = "CreateDocumentationPart under the stored api";
    created_part_id(
        &gateway,
        &stored,
        json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
        &content_map("list pets"),
        case,
    )
    .await;
    let listed = listed_ids(&gateway, &stored, case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case}: expected only the accepted part, got {listed:?}"
    );
}

#[tokio::test]
async fn a_content_map_is_stored_as_the_text_the_request_sent() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-content-text").await;

    // Each row is a content map — "a content map of API-specific key-value pairs ...
    // encoded as a JSON string" (`DocumentationPart$properties`) — differing along what
    // re-rendering the JSON would change: the escapes it carries, non-ASCII content, how
    // deeply it nests, the order of its keys and the insignificant whitespace between
    // them.
    let rows: &[(&str, &str)] = &[
        ("an empty content map", "{}"),
        ("a plain description", r#"{"description":"list pets"}"#),
        (
            "escaped control characters and quotes",
            r#"{"description":"a \"quoted\" line\nand a\ttab \\ backslash"}"#,
        ),
        (
            "non-ASCII content of several scripts and widths",
            r#"{"description":"Gestión de mascotas — 寵物 🐶"}"#,
        ),
        (
            "non-ASCII content written as JSON \\u escapes",
            r#"{"description":"Gesti\u00f3n \ud83d\udc36"}"#,
        ),
        (
            "a nested content map",
            r#"{"schema":{"type":"object","properties":{"id":{"type":"string"}}},"required":["id"]}"#,
        ),
        (
            "keys in an order a re-render would sort",
            r#"{"zeta":1,"alpha":2,"mu":3}"#,
        ),
        (
            "insignificant whitespace between the pairs",
            "{ \"description\" : \"spaced out\" ,\n  \"summary\" : \"pets\" }",
        ),
        (
            "values of every JSON type",
            r#"{"s":"t","n":1.5,"b":true,"z":null,"a":[1,2],"o":{}}"#,
        ),
    ];
    for (index, (case, properties)) in rows.iter().enumerate() {
        // One location per row, so each row stores its own part rather than clashing with
        // the row before it.
        let location = json!({
            "type": "QUERY_PARAMETER",
            "path": "/pets",
            "method": "GET",
            "name": format!("q{index}"),
        });
        let answer = create_part(&gateway, &api, location, properties).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["properties"].as_str(),
            Some(*properties),
            "case {case}: expected the content map answered verbatim, got {:?}",
            answer.body["properties"]
        );
        let id = string_member(&answer.body, "id", case);
        let read = get_part(&gateway, &api, &id).await;
        assert_eq!(
            read.body["properties"].as_str(),
            Some(*properties),
            "case {case}: expected the content map stored verbatim, got {:?}",
            read.body["properties"]
        );
    }
}

#[tokio::test]
async fn a_content_map_is_bounded_in_characters_and_one_past_the_bound_stores_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-content-bound").await;

    // The content map each row sends is `{"description":"<filler>"}`, so what it costs
    // beyond the filler is the length of that same map with an empty description.
    let overhead = content_map("").chars().count();

    // Each row is a filler character and a length: at the bound this emulator stores
    // (`MAX_PROPERTIES_CHARS`, the crate's own constant rather than a literal) and one
    // past it, for a single-byte character and for characters of two, three and four
    // UTF-8 bytes — the bound is in CHARACTERS, so a multi-byte filler reaches exactly
    // the same bound.
    let fillers: &[(&str, char)] = &[
        ("a single-byte filler", 'a'),
        ("a two-byte filler", 'é'),
        ("a three-byte filler", '寵'),
        ("a four-byte filler", '🐶'),
    ];
    let mut stored = 0usize;
    for (index, (filler_case, filler)) in fillers.iter().enumerate() {
        for (bound_case, length, accepted) in [
            ("at the bound", MAX_PROPERTIES_CHARS - overhead, true),
            (
                "one past the bound",
                MAX_PROPERTIES_CHARS - overhead + 1,
                false,
            ),
        ] {
            let case = format!("{filler_case} {bound_case}");
            let properties = format!(
                "{{\"description\":\"{}\"}}",
                std::iter::repeat_n(*filler, length).collect::<String>()
            );
            assert_eq!(
                properties.chars().count(),
                length + overhead,
                "case {case}: the test built a content map of the wrong length"
            );
            let location = json!({
                "type": "QUERY_PARAMETER",
                "path": "/pets",
                "method": "GET",
                "name": format!("q{index}-{}", usize::from(accepted)),
            });
            let answer = create_part(&gateway, &api, location, &properties).await;
            if accepted {
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected 201, got {} with {:?}",
                    answer.status, answer.body
                );
                let id = string_member(&answer.body, "id", &case);
                let read = get_part(&gateway, &api, &id).await;
                assert_eq!(
                    read.body["properties"].as_str(),
                    Some(properties.as_str()),
                    "case {case}: expected the content map stored whole, got {} characters",
                    read.body["properties"].as_str().unwrap_or_default().len()
                );
                stored += 1;
            } else {
                assert_failure(&answer, 400, BAD_REQUEST, &case);
            }
        }
    }

    // Only the accepted rows reached the store.
    let listed = listed_ids(&gateway, &api, "after the bound cases").await;
    assert_eq!(
        listed.len(),
        stored,
        "case after the bound cases: expected {stored} stored parts, got {listed:?}"
    );
}

#[tokio::test]
async fn a_locations_status_code_spans_the_modelled_patterns_range() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-status-codes").await;

    // `DocumentationPartLocation$statusCode` is patterned `^([1-5]\d\d|\*|\s*)$`
    // (`smithy_query(query="shape", shape="DocumentationPartLocation")`), so the range is
    // 100 through 599 beside the `*` wildcard and the blank value the pattern admits.
    // Each row is a `statusCode` the pattern ADMITS and the value the part is stored
    // with: a code is kept as given, and a value that names no code is "not explicitly
    // specified", which the member documents as defaulting to `*` ("The default value is
    // * for any status code").
    let admitted: &[(&str, Option<&str>, &str)] = &[
        ("the lower bound", Some("100"), "100"),
        ("inside the range", Some("301"), "301"),
        ("the upper bound", Some("599"), "599"),
        (
            "the wildcard",
            Some("*"),
            DOCUMENTED_STATUS_CODE_DEFAULT,
        ),
        ("blank", Some(""), DOCUMENTED_STATUS_CODE_DEFAULT),
        ("whitespace", Some("   "), DOCUMENTED_STATUS_CODE_DEFAULT),
        ("absent", None, DOCUMENTED_STATUS_CODE_DEFAULT),
    ];
    for (index, (case, sent, expected)) in admitted.iter().enumerate() {
        // One path per row, so two rows that resolve to the same status code still name
        // different entities.
        let mut location = json!({ "type": "RESPONSE", "path": format!("/r{index}"), "method": "GET" });
        if let Some(sent) = sent {
            location["statusCode"] = json!(sent);
        }
        let id = created_part_id(&gateway, &api, location, &content_map(case), case).await;
        let read = get_part(&gateway, &api, &id).await;
        assert_eq!(
            read.body["location"]["statusCode"].as_str(),
            Some(*expected),
            "case {case}: expected statusCode {expected:?}, got {:?}",
            read.body["location"]["statusCode"]
        );
    }

    // Each row is a `statusCode` the pattern REFUSES: one past each bound, too few and
    // too many digits, and a value that is not a number at all. The pattern is enforced
    // by the CONSTRAINT layer in front of the handler, whose own error class is
    // `ValidationException` (HTTP 400) and not the operation's modelled
    // `BadRequestException`. See the `validation_message_rules` suite for that rule.
    let refused: &[&str] = &["099", "600", "99", "1000", "2xx", "0", "-1", "1 0"];
    for (index, sent) in refused.iter().enumerate() {
        let case = format!("statusCode={sent:?}");
        let answer = create_part(
            &gateway,
            &api,
            json!({
                "type": "RESPONSE",
                "path": format!("/refused{index}"),
                "method": "GET",
                "statusCode": sent,
            }),
            &content_map(&case),
        )
        .await;
        assert_failure(&answer, 400, "ValidationException", &case);
    }

    // Only the admitted rows reached the store.
    let listed = listed_ids(&gateway, &api, "after the status-code cases").await;
    assert_eq!(
        listed.len(),
        admitted.len(),
        "case after the status-code cases: expected {} stored parts, got {listed:?}",
        admitted.len()
    );
}
