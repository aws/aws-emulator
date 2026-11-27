// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the profile object type TEMPLATE surface: the two routes this
//! service's Smithy model binds for it — `GET /templates`
//! (`ListProfileObjectTypeTemplates`) and `GET /templates/{TemplateId}`
//! (`GetProfileObjectTypeTemplate`), per `smithy_query(query="operation",
//! operation="GetProfileObjectTypeTemplate,ListProfileObjectTypeTemplates")`.
//!
//! The rule under test: the service claims every route its model binds, and an input
//! outside the modelled constraints of the operation behind that route is refused as
//! `BadRequestException` (400) in AWS's validation envelope BEFORE any handler runs; an
//! input inside every one of those constraints reaches the operation the route names, and
//! an operation with no implementation yet answers the emulator's not-implemented envelope
//! (HTTP 501, `NotImplemented`, naming that operation) — never a routing miss, and never a
//! success a client could not tell from a real answer.
//!
//! The constraints the rows below are built from are the model's own:
//! `GetProfileObjectTypeTemplateRequest$TemplateId` is `name` (len 1..64, pattern
//! `^[a-zA-Z0-9_-]+$`), and `ListProfileObjectTypeTemplatesRequest` carries `MaxResults`
//! (`maxSize100`, 1..100) and `NextToken` (`token`, len 1..1024). Each bound is driven AT
//! its published value and one past it.
//!
//! A template is PREDEFINED by AWS — "a predefined ProfileObjectType, such as
//! 'Salesforce-Account' or 'Salesforce-Contact'" — so the catalogue of templates, and the
//! mappings and limits each one carries, are published reference data this service's model
//! does not declare. Neither template operation is implemented here for that reason, and
//! these rows pin the 501 the stub census records for them rather than a catalogue this
//! emulator cannot know. Implementing either one turns its rows into assertions on the
//! answer it then computes.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees — the body AND the `x-amzn-errortype` header an SDK keys its error classes
//! off. The HTTP plumbing is this crate's one test harness, in
//! `tests/all/domain_profile_lifecycle.rs`.

use crate::domain_profile_lifecycle::{error_code, Emulator};
use crate::profile_object_types::error_message;

/// The shape every refusal of an input is answered as.
const BAD_REQUEST: &str = "BadRequestException";

/// The shape the emulator answers for an operation its model binds and nothing implements
/// yet: 501, never an empty success.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// AWS's validation envelope over one clause.
fn one_error(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// A `GET` on `path` is refused as `BadRequestException` (400) — in the body's
/// discriminator and in the error-type header — carrying exactly `message`.
async fn refused(emulator: &Emulator, case: &str, path: &str, message: &str) {
    let (status, error_type, body) = emulator.send_parts("GET", path, None).await;
    assert_eq!(
        (status, error_code(&body), error_type.as_str()),
        (400, BAD_REQUEST, BAD_REQUEST),
        "case {case:?}: expected 400 {BAD_REQUEST} in the body and the error-type header, \
         got {status} {error_type:?} {body:?}"
    );
    let stated = error_message(&body);
    assert_eq!(
        stated, message,
        "case {case:?}: expected the Message {message:?}, got {stated:?} \
         (whole body {body:?})"
    );
}

/// A `GET` on `path` is carried through validation to `operation`, which has no
/// implementation yet and so answers the not-implemented envelope. The sentence names the
/// operation, so a path that resolved to the WRONG one fails here instead of passing on the
/// status alone.
async fn reaches(emulator: &Emulator, case: &str, path: &str, operation: &str) {
    let (status, error_type, body) = emulator.send_parts("GET", path, None).await;
    assert_eq!(
        (status, error_code(&body), error_type.as_str()),
        (501, NOT_IMPLEMENTED, NOT_IMPLEMENTED),
        "case {case:?}: expected the input to pass validation and reach {operation}, \
         got {status} {error_type:?} {body:?}"
    );
    let stated = error_message(&body);
    assert!(
        stated.contains(operation),
        "case {case:?}: expected the sentence to name {operation}, got {stated:?}"
    );
}

/// RULE over `TemplateId`'s two modelled constraints: a value breaking either is refused as
/// `BadRequestException` naming the value, the member path and the requirement it broke.
///
/// The rows differ along which constraint broke (the `@length` ceiling, the `@pattern`) and
/// along the character that breaks the pattern, with the length driven one past 64 and far
/// past it.
#[tokio::test]
async fn a_template_id_outside_its_modelled_constraints_is_refused_before_any_handler() {
    let emulator = Emulator::new();

    // The pattern class: one row per character outside `[a-zA-Z0-9_-]`, in a value that is
    // otherwise a well-formed template id.
    for (case, submitted) in [
        ("a template id carrying a dot", "Salesforce.Account"),
        ("a template id carrying a colon", "arn:aws:template"),
        ("a template id carrying a tilde", "Zendesk~User"),
        ("a template id carrying a plus", "Marketo+Lead"),
    ] {
        refused(
            &emulator,
            case,
            &format!("/templates/{submitted}"),
            &one_error(
                submitted,
                "templateId",
                "Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]+$",
            ),
        )
        .await;
    }

    // The length class: the ceiling is 64, so one past it and far past it are both refused
    // for the length rather than for the charset, which these values satisfy.
    for (case, submitted) in [
        (
            "a template id one character past the ceiling",
            "t".repeat(65),
        ),
        ("a template id far past the ceiling", "t".repeat(255)),
    ] {
        refused(
            &emulator,
            case,
            &format!("/templates/{submitted}"),
            &one_error(
                &submitted,
                "templateId",
                "Member must have length less than or equal to 64",
            ),
        )
        .await;
    }
}

/// RULE: a `TemplateId` inside every modelled constraint is an input, so it is carried to
/// `GetProfileObjectTypeTemplate` — which this emulator does not implement, and therefore
/// answers 501 rather than a template it cannot know.
///
/// The rows span the admissible class: both published length bounds, every character group
/// the pattern admits, and the two-part form AWS's own template ids take.
#[tokio::test]
async fn a_template_id_inside_every_modelled_constraint_reaches_its_operation() {
    let emulator = Emulator::new();

    for (case, submitted) in [
        ("a template id at the length floor", "t".to_string()),
        ("a template id at the length ceiling", "t".repeat(64)),
        ("digits alone", "0123456789".to_string()),
        (
            "every character group the pattern admits",
            "aZ9_-".to_string(),
        ),
        ("underscores alone", "_".repeat(3)),
        (
            "the two-part form AWS's template ids take",
            "Salesforce-Account".to_string(),
        ),
        ("another of that form", "CTR-NoInferred".to_string()),
    ] {
        reaches(
            &emulator,
            case,
            &format!("/templates/{submitted}"),
            "GetProfileObjectTypeTemplate",
        )
        .await;
    }
}

/// RULE over the listing's two pagination arguments: a page size outside `1..100` or a
/// token longer than 1024 is refused as `BadRequestException` naming the bound it broke,
/// while a value inside the bounds reaches `ListProfileObjectTypeTemplates`.
///
/// Both bounds are driven AT their published value and one past it, and the route is asked
/// for with and without its trailing separator so the listing — not the single-template
/// read — is what both forms resolve to.
#[tokio::test]
async fn the_template_listing_refuses_a_page_size_or_token_outside_its_modelled_bounds() {
    let emulator = Emulator::new();
    const LISTING: &str = "ListProfileObjectTypeTemplates";

    // (case, the submitted page size, the bound it broke)
    for (case, submitted, requirement) in [
        (
            "a page size below the floor",
            "0",
            "Member must have value greater than or equal to 1",
        ),
        (
            "a negative page size",
            "-1",
            "Member must have value greater than or equal to 1",
        ),
        (
            "a page size one past the ceiling",
            "101",
            "Member must have value less than or equal to 100",
        ),
    ] {
        refused(
            &emulator,
            case,
            &format!("/templates?max-results={submitted}"),
            &one_error(submitted, "maxResults", requirement),
        )
        .await;
    }

    // A token one past the modelled ceiling is refused for its length.
    let past_ceiling = "t".repeat(1025);
    refused(
        &emulator,
        "a next token one character past the ceiling",
        &format!("/templates?next-token={past_ceiling}"),
        &one_error(
            &past_ceiling,
            "nextToken",
            "Member must have length less than or equal to 1024",
        ),
    )
    .await;

    // Inside the bounds — including AT each one — the listing is what the route reaches,
    // asked for with its trailing separator and without.
    for (case, path) in [
        ("no pagination arguments at all", "/templates".to_string()),
        ("the route's trailing separator", "/templates/".to_string()),
        (
            "a page size at the floor",
            "/templates?max-results=1".to_string(),
        ),
        (
            "a page size at the ceiling",
            "/templates?max-results=100".to_string(),
        ),
        (
            "a next token at the length ceiling",
            format!("/templates?next-token={}", "t".repeat(1024)),
        ),
        (
            "both arguments at once",
            format!("/templates?max-results=50&next-token={}", "t".repeat(16)),
        ),
    ] {
        reaches(&emulator, case, &path, LISTING).await;
    }
}
