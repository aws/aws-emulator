// SPDX-License-Identifier: CC0-1.0

//! How the entity-set family refuses an identifier it will not act on, across both
//! families (threat and trusted) and all three operations that carry one (the read,
//! the update and the delete).
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch` (the
//! harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. An entity-set identifier outside the alphabet the member is constrained to is a
//!    CONSTRAINT violation: HTTP 400 under this service's `InvalidInputException`
//!    class, carrying the validation-error template — how many violations were
//!    detected, the member path the request spelled, and the pattern the value had to
//!    satisfy — and nothing of the caller's value.
//! 2. An identifier of the right FORM that names no set is an ABSENCE: HTTP 400
//!    under this service's `InvalidInputException` class in the
//!    `BadRequestException` envelope — the only client error either family's
//!    operations declare (`smithy_query(operation="GetThreatEntitySet")`) — with the
//!    one fixed sentence the family
//!    words it with, naming neither the member, nor the value, nor which family was
//!    addressed. An id no create minted, a stored id in another spelling, one minted
//!    for the other family and one another detector minted are all that same absence.
//! 3. The constraint is decided from the REQUEST, before anything is resolved, so a
//!    malformed identifier is refused the same way under a detector that does not
//!    exist; an identifier of the right form under such a detector is still the
//!    DETECTOR family's own refusal.
//! 4. Each refusal is a LOOKUP and not a constant: the same operations over the
//!    identifier the create minted succeed, and the stored set survives every refusal.
//!
//! The rows differ along what the rules generalise over: the FAMILY, the OPERATION
//! CLASS, and the FORM of the identifier — which character outside the alphabet it
//! carries, its LENGTH (one character up to well past a minted id), its CASE, and
//! whether it was minted at all.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, error_shape, new_router, Router, ABSENT_DETECTOR_ID,
};

/// The modelled error SHAPE restJson1 resolves a 400 of this service to (the
/// `x-amzn-errortype` header).
const BAD_REQUEST: &str = "BadRequestException";

/// The error CLASS a 400 carries in the body's `__type` and `type`.
const INVALID_INPUT: &str = "InvalidInputException";

/// What the entity-set family answers for an identifier of the right form that names
/// no set.
const NOT_FOUND_MESSAGE: &str = "The specified entity set does not exist.";

/// The validation-error template, with the member path and the pattern filled in.
fn constraint_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: [A-Za-z0-9]+"
    )
}

/// The DETECTOR family's own refusal, which an entity-set absence must not borrow.
const DETECTOR_NOT_OWNED: &str =
    "The request is rejected because the input detectorId is not owned by the current account.";

/// The claim rule 2 forbids an entity-set refusal from making.
const OWNERSHIP_CLAIM: &str = "is not owned by the current account";

/// Identifiers outside `[A-Za-z0-9]+`, each carrying a different character the
/// alphabet excludes, at a different place and length: a hyphenated word, an
/// underscore, a trailing dot on an otherwise minted-looking id, a plus, a bang, a
/// colon, and a minted-looking id with a suffix.
const MALFORMED_IDS: [&str; 7] = [
    "not-a-set",
    "set_id",
    "0123456789abcdef0123456789abcde.",
    "a+b",
    "ABCDEF!",
    "set:id",
    "ffffffffffffffffffffffffffffffff-1",
];

/// A well-formed identifier (32 lowercase hex characters, the form of every id this
/// service mints) that no create here ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// One well-formed identifier that names nothing, at a length and in a case no mint
/// produces: the shortest the pattern admits, a digits-only one, and one well past 32
/// characters.
const UNMINTED_FORMS: [&str; 3] = [
    "a",
    "0123456789",
    "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0U1v2W3x4",
];

/// A location of the `s3://` form a create accepts
/// (`smithy_query(query="doc",shape="CreateThreatEntitySetRequest",member="Location")`).
const LOCATION: &str = "s3://guardduty-lists/entities.txt";

/// One entity-set family: the URI segment its sub-resource collection hangs off, and
/// the wire name of the id member its requests carry
/// (`smithy_query(operation="GetThreatEntitySet,GetTrustedEntitySet")`).
struct Family {
    case: &'static str,
    segment: &'static str,
    id_member: &'static str,
    /// The member a create answers the minted id in.
    id_key: &'static str,
    /// A format `<Family>Format` admits (`smithy_query(shape="ThreatEntitySetFormat,
    /// TrustedEntitySetFormat")`).
    format: &'static str,
}

const FAMILIES: [Family; 2] = [
    Family {
        case: "threat entity set",
        segment: "threatentityset",
        id_member: "threatEntitySetId",
        id_key: "threatEntitySetId",
        format: "TXT",
    },
    Family {
        case: "trusted entity set",
        segment: "trustedentityset",
        id_member: "trustedEntitySetId",
        id_key: "trustedEntitySetId",
        format: "STIX",
    },
];

/// Create one set of `family` under `detector` and return the id it minted.
async fn create(router: &Router, detector: &str, family: &Family) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector}/{}", family.segment),
        Some(json!({
            "name": "refusal-probe",
            "format": family.format,
            "location": LOCATION,
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the create to answer 200, got {status} {body:?}",
        family.case
    );
    body[family.id_key]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a {}, got {body:?}",
                family.case, family.id_key
            )
        })
        .to_string()
}

/// The three operations that carry an entity-set id: the read, the update and the
/// delete. The update's body names only the set's `Name`, so a refusal cannot be
/// mistaken for a rejection of what it would have written.
fn requests(detector: &str, segment: &str, id: &str) -> [(&'static str, &'static str, String, Option<Value>); 3] {
    let path = format!("/detector/{detector}/{segment}/{id}");
    [
        ("read", "GET", path.clone(), None),
        (
            "update",
            "POST",
            path.clone(),
            Some(json!({"name": "must-not-land"})),
        ),
        ("delete", "DELETE", path, None),
    ]
}

/// The answer a refusal must carry: the status, the error class the body's `__type`
/// names, the error shape the `x-amzn-errortype` header names, and the sentence.
struct Expected {
    status: u16,
    class: &'static str,
    shape: &'static str,
    message: String,
}

/// The constraint violation an identifier outside the alphabet earns, on `member`.
fn constraint_violation(member: &str) -> Expected {
    Expected {
        status: 400,
        class: INVALID_INPUT,
        shape: BAD_REQUEST,
        message: constraint_message(member),
    }
}

/// The absence an identifier of the right form that names no set earns: the same
/// status, class and envelope every client rejection of this service carries, and
/// the family's own sentence.
fn absent_entity_set() -> Expected {
    Expected {
        status: 400,
        class: INVALID_INPUT,
        shape: BAD_REQUEST,
        message: NOT_FOUND_MESSAGE.to_string(),
    }
}

/// Assert one refusal: its status, the error class in the body, the error shape in the
/// header, the sentence word for word, and that the identifier the request sent is not
/// quoted back.
fn assert_refusal(case: &str, status: u16, body: &Value, expected: &Expected, sent: &str) {
    assert_eq!(
        status, expected.status,
        "case {case:?}: expected HTTP {}, got {status} {body:?}",
        expected.status
    );
    let class = error_code(body);
    assert_eq!(
        class, expected.class,
        "case {case:?}: expected the {} class, got {class:?} {body:?}",
        expected.class
    );
    let shape = error_shape(body);
    assert_eq!(
        shape, expected.shape,
        "case {case:?}: expected the {} envelope shape, got {shape:?} {body:?}",
        expected.shape
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected.message,
        "case {case:?}: expected message {:?}, got {message:?}",
        expected.message
    );
    assert!(
        !message.contains(sent),
        "case {case:?}: the refusal must not quote {sent:?} back, got {message:?}"
    );
}

/// Assert `status`/`body` is the CONSTRAINT violation both entity-set families
/// answer an identifier outside the alphabet `member` is constrained to with: HTTP
/// 400 under the `InvalidInputException` class in the `BadRequestException`
/// envelope, carrying the validation-error template that names the member and the
/// pattern and nothing of the caller's value.
///
/// Exported because the suites that drive a malformed set id for their own rules
/// assert the same answer, and the template must not drift between them.
pub(crate) fn assert_entity_set_id_constraint_violation(
    case: &str,
    status: u16,
    body: &Value,
    member: &str,
    id: &str,
) {
    assert_refusal(case, status, body, &constraint_violation(member), id);
}

/// The `Status` a read reports for a stored set, failing the test if the read did not
/// succeed.
async fn status_of(router: &Router, detector: &str, segment: &str, id: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector}/{segment}/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the stored set to read back 200, got {status} {body:?}"
    );
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a status, got {body:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: an identifier outside the alphabet is a constraint violation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_outside_the_alphabet_is_refused_with_the_validation_template() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for family in &FAMILIES {
        let stored = create(&router, &detector, family).await;
        let before = status_of(&router, &detector, family.segment, &stored, family.case).await;

        for malformed in MALFORMED_IDS {
            for (op, method, path, body) in requests(&detector, family.segment, malformed) {
                let case = format!("{} {op}: malformed id {malformed:?}", family.case);
                let (status, got) = call(&router, method, &path, body).await;
                assert_refusal(
                    &case,
                    status,
                    &got,
                    &constraint_violation(family.id_member),
                    malformed,
                );
                assert!(
                    got.get("status").is_none(),
                    "case {case:?}: a refusal must report no entity set, got {got:?}"
                );
            }

            // Rule 3: the constraint is decided from the request, so the same
            // identifier is refused the same way under a detector nothing minted.
            let case = format!(
                "{} read under an absent detector: malformed id {malformed:?}",
                family.case
            );
            let (status, got) = call(
                &router,
                "GET",
                &format!(
                    "/detector/{ABSENT_DETECTOR_ID}/{}/{malformed}",
                    family.segment
                ),
                None,
            )
            .await;
            assert_refusal(
                &case,
                status,
                &got,
                &constraint_violation(family.id_member),
                malformed,
            );
        }

        // Rule 4: none of those refusals touched the stored set.
        let after = status_of(&router, &detector, family.segment, &stored, family.case).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused call must leave the stored set alone, got {after:?} after {before:?}",
            family.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an identifier of the right form that names nothing is the absence.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_well_formed_identifier_naming_no_set_is_the_familys_absence() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // One set of each family, so a row can address the OTHER family's id — a
    // well-formed id this family's collection does not hold.
    let threat = create(&router, &detector, &FAMILIES[0]).await;
    let trusted = create(&router, &detector, &FAMILIES[1]).await;
    let stored = [threat.clone(), trusted.clone()];

    for (index, family) in FAMILIES.iter().enumerate() {
        let mine = stored[index].clone();
        let theirs = stored[1 - index].clone();

        // Each row is a well-formed identifier this family's collection does not
        // hold, and why.
        let mut rows: Vec<(String, String)> = vec![
            (ABSENT_SET_ID.to_string(), "an id no create minted".to_string()),
            (mine.to_uppercase(), "the stored id upper-cased".to_string()),
            (theirs.clone(), "the other family's stored id".to_string()),
        ];
        for form in UNMINTED_FORMS {
            rows.push((
                form.to_string(),
                format!("a well-formed id of {} characters", form.len()),
            ));
        }

        for (id, why) in &rows {
            for (op, method, path, body) in requests(&detector, family.segment, id) {
                let case = format!("{} {op}: {why}", family.case);
                let (status, got) = call(&router, method, &path, body).await;
                assert_refusal(&case, status, &got, &absent_entity_set(), id);
                let message = got["message"].as_str().unwrap_or_default();
                assert!(
                    !message.contains(family.id_member) && !message.contains(OWNERSHIP_CLAIM),
                    "case {case:?}: the absence names neither the member nor an owner, got {message:?}"
                );
            }
        }

        // Rule 3: an identifier of the right form under a detector nothing minted is
        // the DETECTOR family's own refusal, not this family's absence.
        let case = format!("{}: absent detector", family.case);
        let (status, got) = call(
            &router,
            "GET",
            &format!(
                "/detector/{ABSENT_DETECTOR_ID}/{}/{ABSENT_SET_ID}",
                family.segment
            ),
            None,
        )
        .await;
        assert_refusal(
            &case,
            status,
            &got,
            &Expected {
                status: 400,
                class: INVALID_INPUT,
                shape: BAD_REQUEST,
                message: DETECTOR_NOT_OWNED.to_string(),
            },
            ABSENT_DETECTOR_ID,
        );

        // Rule 4: the id the create minted still resolves, so every refusal above
        // was a lookup over the store rather than a constant.
        let (status, got) = call(
            &router,
            "GET",
            &format!("/detector/{detector}/{}/{mine}", family.segment),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the minted id to still resolve, got {status} {got:?}",
            family.case
        );
        assert_eq!(
            got["name"].as_str(),
            Some("refusal-probe"),
            "case {:?}: expected the stored name, got {got:?}",
            family.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2, continued: an id minted elsewhere is the same absence, not an
// ownership claim.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_minted_under_another_detector_is_the_same_absence() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // A second endpoint, with a detector and a set of its own: the id it minted is
    // well formed and real, and simply not one the first endpoint's detector holds.
    let elsewhere = new_router();
    let other_detector = enabled_detector(&elsewhere).await;

    for family in &FAMILIES {
        let mine = create(&router, &detector, family).await;
        let theirs = create(&elsewhere, &other_detector, family).await;

        for (op, method, path, body) in requests(&detector, family.segment, &theirs) {
            let case = format!("{} {op}: an id another detector minted", family.case);
            let (status, got) = call(&router, method, &path, body).await;
            assert_refusal(&case, status, &got, &absent_entity_set(), &theirs);
        }

        // The set that endpoint really holds is untouched by those refusals, and so
        // is this one's: each still reports the name its own create stored.
        for (router, detector, id, case) in [
            (&router, &detector, &mine, "the first endpoint's set"),
            (&elsewhere, &other_detector, &theirs, "the other endpoint's set"),
        ] {
            let (status, got) = call(
                router,
                "GET",
                &format!("/detector/{detector}/{}/{id}", family.segment),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {got:?}"
            );
            assert_eq!(
                got["name"].as_str(),
                Some("refusal-probe"),
                "case {case:?}: expected the stored name, got {got:?}"
            );
        }
    }
}
