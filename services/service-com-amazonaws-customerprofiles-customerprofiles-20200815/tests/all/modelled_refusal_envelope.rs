// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the envelope a request refused BEFORE any handler ran is answered
//! in.
//!
//! The rule under test: a refusal of the input is answered as the error shape this model
//! declares for it — `BadRequestException` (HTTP 400), whose prose member the model spells
//! `Message` and which `x-amzn-errortype` names — whichever layer refused the request, and
//! the prose is AWS's:
//!
//! * a value the model's own constraints refuse is stated in AWS's validation envelope,
//!   `<n> validation error(s) detected: Value '<value>' at '<member path>' failed to
//!   satisfy constraint: <requirement>`, with the value withheld for a member the model
//!   marks `@sensitive`, an enumerated type's value set listed in the order the layer in
//!   front of the service walks its own table of the admissible values, and a `@pattern`
//!   quoted as the model declares it for a value bound to the request URI and without its
//!   whole-value anchors for a BODY member;
//! * a value whose JSON type the member it is bound to cannot be read from never became an
//!   input at all, so it is stated as the class it was sent as and the modelled type it
//!   could not become: `<CLASS>_VALUE can not be converted to a <Type>`.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees — body AND the error-type header an SDK keys its error classes off. The HTTP
//! plumbing and the create helpers are this crate's one test harness, in
//! `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::service::modelled_refusal::REFUSED_AS_MODELLED;

use crate::domain_profile_lifecycle::{error_code, error_message, Emulator};

/// The domain the refusals are asked for, inside the modelled `name` charset so the
/// refusal under test is the only one.
const DOMAIN: &str = "refusaldom";

/// The error shape every refusal of an input is answered as.
const BAD_REQUEST: &str = "BadRequestException";

/// One row of the tables below: what the request is, and the sentence the refusal carries.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    message: String,
}

/// Drive every row and assert the status, the error shape (in the body's discriminator AND
/// in the header a restJson1 client resolves the shape from) and the sentence.
async fn refused(emulator: &Emulator, rows: Vec<Row>) {
    for row in rows {
        let (status, error_type, body) = emulator
            .send_parts(row.method, &row.path, row.body.clone())
            .await;
        assert_eq!(
            (status, error_code(&body), error_type.as_str()),
            (400, BAD_REQUEST, BAD_REQUEST),
            "case {:?}: expected 400 {BAD_REQUEST} in the body and the error-type header, \
             got {status} {error_type:?} {body:?}",
            row.case
        );
        let stated = error_message(&body);
        assert_eq!(
            stated, row.message,
            "case {:?}: expected the Message {:?}, got {stated:?} (whole body {body:?})",
            row.case, row.message
        );
    }
}

/// AWS's validation envelope over one clause.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause for a value the refusal quotes back.
fn clause(value: &str, member: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The clause for a member the model marks `@sensitive`, whose value is NOT echoed.
fn withheld_clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// RULE over the `@range` class, on both halves of a request: the clause names the
/// submitted number, the member path and the modelled bound it broke. The rows differ along
/// which bound was broken (floor, ceiling), where the value travels (an `@httpQuery`
/// parameter, a body member), and which operation carries it — `ListDomains$MaxResults`
/// (`maxSize100`, 1..100) and `CreateDomain`/`UpdateDomain$DefaultExpirationDays`
/// (`expirationDaysInteger`, 1..1098), per `smithy_query(query="operation",
/// operation="ListDomains,CreateDomain,UpdateDomain")`; each bound is tested AT the value
/// one past it.
#[tokio::test]
async fn a_number_outside_its_modelled_range_is_refused_with_the_bound_it_broke() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    let mut rows = Vec::new();
    // (case, the submitted number, the bound sentence) for the query-bound page size.
    for (case, submitted, requirement) in [
        ("a page size below the floor", "0", "Member must have value greater than or equal to 1"),
        (
            "a page size above the ceiling",
            "101",
            "Member must have value less than or equal to 100",
        ),
        (
            "a negative page size",
            "-7",
            "Member must have value greater than or equal to 1",
        ),
    ] {
        rows.push(Row {
            case,
            method: "GET",
            path: format!("/domains?max-results={submitted}"),
            body: None,
            message: one_error(&clause(submitted, "maxResults", requirement)),
        });
    }
    // The same class on a BODY member, on the create and the update that share the bound.
    for (case, submitted, requirement) in [
        (
            "a retention below the floor",
            0,
            "Member must have value greater than or equal to 1",
        ),
        (
            "a retention above the ceiling",
            1099,
            "Member must have value less than or equal to 1098",
        ),
    ] {
        let message = one_error(&clause(
            &submitted.to_string(),
            "defaultExpirationDays",
            requirement,
        ));
        rows.push(Row {
            case,
            method: "POST",
            path: "/domains/anotherdom".to_string(),
            body: Some(json!({ "DefaultExpirationDays": submitted })),
            message: message.clone(),
        });
        rows.push(Row {
            case,
            method: "PUT",
            path: format!("/domains/{DOMAIN}"),
            body: Some(json!({ "DefaultExpirationDays": submitted })),
            message,
        });
    }

    refused(&emulator, rows).await;
}

/// RULE over the `@pattern` class: the clause names the submitted value, the member path
/// and the expression the value had to satisfy — quoted as the model declares it for a
/// value bound to the request URI, and without the whole-value `^`/`$` for a BODY member.
///
/// The rows differ along the binding (an `@httpLabel` path segment, a body member), the
/// expression (`name`'s `^[a-zA-Z0-9_-]+$`, `uuid`'s `^[a-f0-9]{32}$`, per
/// `smithy_query(query="shape", shape="name,uuid")`) and the shape of the rejected value.
#[tokio::test]
async fn a_value_outside_its_modelled_pattern_is_refused_with_the_expression() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    let mut rows = Vec::new();
    // A value bound to the request URI: the expression is quoted as the model writes it.
    for (case, submitted) in [
        ("a domain name carrying dots", "probe.ccp.ad.5a90d3e6"),
        ("a domain name carrying a tilde", "probe~ccp"),
        ("a domain name carrying a colon", "arn:aws:profile"),
    ] {
        rows.push(Row {
            case,
            method: "POST",
            path: format!("/domains/{submitted}/profiles/search"),
            body: Some(json!({ "KeyName": "_account", "Values": ["1"] })),
            message: one_error(&clause(
                submitted,
                "domainName",
                "Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]+$",
            )),
        });
    }
    // A BODY member: the expression is quoted without the pair the model wraps it in.
    for (case, submitted) in [
        ("a profile id that is not hex at all", "not-a-profile-id".to_string()),
        ("a profile id in upper-case hex", "A".repeat(32)),
        ("a profile id one character short", "a".repeat(31)),
    ] {
        rows.push(Row {
            case,
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles/delete"),
            body: Some(json!({ "ProfileId": submitted })),
            message: one_error(&clause(
                &submitted,
                "profileId",
                "Member must satisfy regular expression pattern: [a-f0-9]{32}",
            )),
        });
    }

    refused(&emulator, rows).await;
}

/// RULE over the enum class: the clause lists every value the enumerated type admits, in
/// the order the layer in front of the service walks its own table of them — which is a
/// property of the TYPE and not of the member that carried the value — and the value itself
/// is withheld when the model marks that member `@sensitive`.
///
/// The value sets are the model's own (`smithy_query(query="shape",
/// shape="PartyType,Gender,logicalOperator")`: `PartyType` is `INDIVIDUAL, BUSINESS,
/// OTHER`, `Gender` is `MALE, FEMALE, UNSPECIFIED`, `logicalOperator` is `AND, OR`), each
/// listed here in the order a refusal states it; the first two are `@sensitive` and the
/// third is not, which is what decides whether the clause quotes the rejected value.
#[tokio::test]
async fn a_value_outside_an_enumerated_type_is_refused_with_the_whole_value_set() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    let rows = vec![
        Row {
            case: "a party type outside the set",
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles"),
            body: Some(json!({ "PartyType": "ORGANISATION" })),
            message: one_error(&withheld_clause(
                "partyType",
                "Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]",
            )),
        },
        Row {
            case: "a party type differing only in case",
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles"),
            body: Some(json!({ "PartyType": "individual" })),
            message: one_error(&withheld_clause(
                "partyType",
                "Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]",
            )),
        },
        Row {
            case: "a gender outside the set",
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles"),
            body: Some(json!({ "Gender": "OTHER" })),
            message: one_error(&withheld_clause(
                "gender",
                "Member must satisfy enum value set: [FEMALE, MALE, UNSPECIFIED]",
            )),
        },
        Row {
            case: "a logical operator outside the set, which the model does not mark sensitive",
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles/search"),
            body: Some(json!({
                "KeyName": "_account",
                "Values": ["1"],
                "LogicalOperator": "XOR",
            })),
            message: one_error(&clause(
                "XOR",
                "logicalOperator",
                "Member must satisfy enum value set: [OR, AND]",
            )),
        },
    ];

    refused(&emulator, rows).await;
}

/// RULE: a `@required` member the request omits is stated in the same envelope, as the one
/// requirement it broke, with no value to echo.
#[tokio::test]
async fn a_required_member_the_request_omits_is_refused_in_the_same_envelope() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    let rows = vec![
        Row {
            case: "a create carrying no retention",
            method: "POST",
            path: "/domains/emptybodydom".to_string(),
            body: Some(json!({})),
            message: one_error(
                "Value null at 'defaultExpirationDays' failed to satisfy constraint: \
                 Member must not be null",
            ),
        },
        Row {
            case: "a delete carrying no profile id",
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles/delete"),
            body: Some(json!({})),
            message: one_error(
                "Value null at 'profileId' failed to satisfy constraint: \
                 Member must not be null",
            ),
        },
    ];

    refused(&emulator, rows).await;
}

/// RULE: a value whose JSON type the member it is bound to cannot be read from never
/// becomes an input, so the refusal states the CLASS of value that was sent and the
/// modelled type it could not become — never the value itself, however long it was.
///
/// The rows differ along the class sent (number, flag, string, array, object) and the
/// modelled target it was bound to: a `sensitiveString1To255` string member, the
/// `expirationDaysInteger` integer, and the `TagMap` map, per
/// `smithy_query(query="operation", operation="CreateProfile,CreateDomain")`.
#[tokio::test]
async fn a_value_its_member_cannot_be_read_from_is_refused_as_the_class_it_was_sent_as() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    let mut rows = Vec::new();
    // A string member handed something that is not a string.
    for (case, submitted, class) in [
        ("a number where a string is modelled", json!(1_234_567), "NUMBER_VALUE"),
        ("a flag where a string is modelled", json!(true), "BOOLEAN_VALUE"),
        ("an array where a string is modelled", json!(["1234567"]), "ARRAY_VALUE"),
        (
            "an object where a string is modelled",
            json!({ "number": "1234567" }),
            "OBJECT_VALUE",
        ),
    ] {
        rows.push(Row {
            case,
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles"),
            body: Some(json!({ "AccountNumber": submitted })),
            message: format!("{class} can not be converted to a String"),
        });
    }
    // An integer member handed something that is not a whole number its width holds, and a
    // map member handed something that is not an object.
    for (case, body, message) in [
        (
            "a string where an integer is modelled",
            json!({ "DefaultExpirationDays": "ninety" }),
            "STRING_VALUE can not be converted to an Integer",
        ),
        (
            "a fraction where an integer is modelled",
            json!({ "DefaultExpirationDays": 90.5 }),
            "NUMBER_VALUE can not be converted to an Integer",
        ),
        (
            "an array where a map is modelled",
            json!({ "DefaultExpirationDays": 90, "Tags": ["owner"] }),
            "ARRAY_VALUE can not be converted to a Map",
        ),
    ] {
        rows.push(Row {
            case,
            method: "POST",
            path: "/domains/unreadabledom".to_string(),
            body: Some(body),
            message: message.to_string(),
        });
    }

    refused(&emulator, rows).await;
}

/// Every operation the model binds is served behind the wrapper that answers these
/// refusals, so no operation is left answering the framework's own envelope.
///
/// The surface is read out of the service's own Smithy model — the same file the schemas
/// are generated from — rather than listed twice.
#[test]
fn every_modelled_operation_is_served_behind_the_modelled_refusal() {
    let model: Value = serde_json::from_str(include_str!(
        "../../smithy-model/com-amazonaws-customerprofiles-customerprofiles-20200815.json"
    ))
    .expect("the model is JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model declares shapes");
    let mut modelled: Vec<&str> = shapes
        .iter()
        .filter(|(_, shape)| shape.get("type").and_then(Value::as_str) == Some("operation"))
        .map(|(id, _)| id.rsplit('#').next().unwrap_or(id))
        .collect();
    modelled.sort_unstable();

    let mut served: Vec<&str> = REFUSED_AS_MODELLED.to_vec();
    served.sort_unstable();

    let unserved: Vec<&&str> = modelled.iter().filter(|op| !served.contains(op)).collect();
    let unmodelled: Vec<&&str> = served.iter().filter(|op| !modelled.contains(op)).collect();
    assert!(
        unserved.is_empty() && unmodelled.is_empty(),
        "expected every modelled operation to be served behind the modelled refusal: \
         {} modelled, {} served, not served {unserved:?}, not in the model {unmodelled:?}",
        modelled.len(),
        served.len()
    );
}

/// The wrapper answers only what was refused BEFORE a handler ran: a success is untouched,
/// and a refusal a HANDLER built keeps its own sentence rather than being restated.
#[tokio::test]
async fn an_answer_a_handler_produced_is_left_as_it_was() {
    let emulator = Emulator::new();
    let created = emulator.create_domain(DOMAIN, 90).await;
    assert_eq!(
        created.get("DomainName").and_then(Value::as_str),
        Some(DOMAIN),
        "setup: expected the create to answer from the stored domain, got {created:?}"
    );

    // A second create of the same name is the handler's own refusal, in the same shape.
    let (status, error_type, body) = emulator
        .send_parts(
            "POST",
            &format!("/domains/{DOMAIN}"),
            Some(json!({ "DefaultExpirationDays": 90 })),
        )
        .await;
    assert_eq!(
        (status, error_code(&body), error_type.as_str()),
        (400, BAD_REQUEST, BAD_REQUEST),
        "case \"a duplicate create\": expected 400 {BAD_REQUEST}, got {status} {body:?}"
    );
    let stated = error_message(&body);
    assert!(
        stated.contains(DOMAIN) && !stated.contains("validation error detected"),
        "case \"a duplicate create\": expected the handler's own sentence naming {DOMAIN:?}, \
         got {stated:?}"
    );

    // A read of a domain that is not stored keeps the shape its handler raised.
    let (status, error_type, body) = emulator.send_parts("GET", "/domains/nosuchdom", None).await;
    assert_eq!(
        (status, error_code(&body), error_type.as_str()),
        (404, "ResourceNotFoundException", "ResourceNotFoundException"),
        "case \"a read of an absent domain\": expected 404 ResourceNotFoundException, \
         got {status} {error_type:?} {body:?}"
    );
    assert!(
        !error_message(&body).is_empty(),
        "case \"a read of an absent domain\": expected a Message, got {body:?}"
    );
}

/// RULE: a search takes exactly one value per key name, and a key handed any other number
/// of values is refused as `BadRequestException` naming that key and the count the request
/// carried for it — the refusal a HANDLER raises, in the same modelled shape.
///
/// The rows differ along the count submitted (none, two, three), which key carried it (the
/// required `KeyName`/`Values` pair, an `AdditionalSearchKeys` entry) and the key name, so
/// the sentence's two variable parts are both exercised.
#[tokio::test]
async fn a_search_key_handed_other_than_one_value_is_refused_with_the_count() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;

    // (case, the body, the key the refusal names, the count it states)
    let rows: Vec<(&str, Value, &str, usize)> = vec![
        (
            "two values for the required key",
            json!({ "KeyName": "_account", "Values": ["a1", "a2"] }),
            "_account",
            2,
        ),
        (
            "three values for the required key",
            json!({ "KeyName": "_email", "Values": ["a@x", "b@x", "c@x"] }),
            "_email",
            3,
        ),
        (
            "no values at all",
            json!({ "KeyName": "_phone", "Values": [] }),
            "_phone",
            0,
        ),
        (
            "two values for an additional key",
            json!({
                "KeyName": "_account",
                "Values": ["a1"],
                "AdditionalSearchKeys": [{ "KeyName": "_email", "Values": ["a@x", "b@x"] }],
            }),
            "_email",
            2,
        ),
    ];

    let rows = rows
        .into_iter()
        .map(|(case, body, key_name, count)| Row {
            case,
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles/search"),
            body: Some(body),
            message: format!("Expected 1 values for key name {key_name} but got {count}"),
        })
        .collect();

    refused(&emulator, rows).await;
}

/// RULE: a search resumes only at a page it ISSUED, so a `NextToken` the caller built is
/// refused — whatever its shape — with the sentence the service words for a pagination
/// argument it cannot use, while the token a page really handed back resumes that page.
#[tokio::test]
async fn a_next_token_this_search_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;
    for index in 0..3 {
        emulator
            .create_profile(
                DOMAIN,
                json!({
                    "EmailAddress": "shared@example.com",
                    "AccountNumber": format!("ACCT-{index}"),
                }),
            )
            .await;
    }
    let search = json!({ "KeyName": "_email", "Values": ["shared@example.com"] });

    // A page this search really issued, and the token it handed back with it.
    let (status, first) = emulator
        .search(DOMAIN, search.clone(), "?max-results=1")
        .await;
    let issued = first
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("setup: expected a NextToken, got {status} {first:?}"));

    // The token it issued resumes the next page.
    let (status, resumed) = emulator
        .search(
            DOMAIN,
            search.clone(),
            &format!("?max-results=1&next-token={issued}"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the token the search issued\": expected the page to resume, got {status} {resumed:?}"
    );

    // (case, a token this search did not issue)
    let forged: Vec<(&str, String)> = vec![
        ("a token that is not a token at all", "not-a-token".to_string()),
        ("a profile id, which is the cursor inside a token", "a".repeat(32)),
        ("a token with its first character changed", flipped(&issued)),
        ("a token with a character dropped", issued[..issued.len() - 2].to_string()),
        ("a token with text appended", format!("{issued}00")),
    ];

    let rows = forged
        .into_iter()
        .map(|(case, token)| Row {
            case,
            method: "POST",
            path: format!("/domains/{DOMAIN}/profiles/search?next-token={token}"),
            body: Some(search.clone()),
            message: "SearchProfiles failed due to the nextToken or the maxResults is invalid"
                .to_string(),
        })
        .collect();

    refused(&emulator, rows).await;
}

/// `token` with its first character changed to another the same rendering admits, so the
/// value has a real token's shape and length and is still not one this store minted.
fn flipped(token: &str) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    characters[0] = if characters[0] == '0' { '1' } else { '0' };
    characters.into_iter().collect()
}

/// RULE: a domain's `Tags` are always reported by the operations that project a domain —
/// the map it carries, and present and empty for a domain that carries none — while the
/// domain's other optional members stay absent until they are set.
#[tokio::test]
async fn a_domains_tags_are_always_reported() {
    let emulator = Emulator::new();

    // (case, the Tags the create carried, the map every projection reports)
    for (case, submitted, expected) in [
        ("a domain created with no tags", json!({}), json!({})),
        (
            "a domain created with one tag",
            json!({ "sim2real": "probe" }),
            json!({ "sim2real": "probe" }),
        ),
        (
            "a domain created with several tags",
            json!({ "owner": "ada", "stage": "probe" }),
            json!({ "owner": "ada", "stage": "probe" }),
        ),
    ] {
        let domain = format!("tagsdom{}", submitted.as_object().map_or(0, |tags| tags.len()));
        let mut body = json!({ "DefaultExpirationDays": 90 });
        if !submitted.as_object().is_some_and(|tags| tags.is_empty()) {
            body["Tags"] = submitted.clone();
        }
        let (status, created) = emulator.send("POST", &format!("/domains/{domain}"), Some(body)).await;
        assert_eq!(
            (status, created.get("Tags")),
            (200, Some(&expected)),
            "case {case:?}: expected CreateDomain to report {expected}, got {status} {created:?}"
        );

        let (status, read) = emulator.send("GET", &format!("/domains/{domain}"), None).await;
        assert_eq!(
            (status, read.get("Tags")),
            (200, Some(&expected)),
            "case {case:?}: expected GetDomain to report {expected}, got {status} {read:?}"
        );
        // The members that were never set stay absent, which is what makes the always-
        // present `Tags` member a distinction a caller can read.
        for member in ["DefaultEncryptionKey", "DeadLetterQueueUrl", "Matching"] {
            assert!(
                read.get(member).is_none(),
                "case {case:?}: expected {member} to stay absent, got {read:?}"
            );
        }
    }
}
