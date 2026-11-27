// SPDX-License-Identifier: CC0-1.0

//! Where the catalog resolution sits relative to the partner-entitlement denial, and where
//! the bounds this service invents for itself sit relative to both — over the wire.
//!
//! The rule under test: an operation that WRITES resolves the `Catalog` its request names
//! before the account-level entitlement gate, so a catalog this service does not hold is
//! answered with that lookup's own 404 `ResourceNotFoundException` (``Catalog `<name>` not
//! found``) whatever the calling account holds, and the gate refuses only a request whose
//! catalog resolved; a REPORTING operation keeps the gate ahead of its lookups, so an
//! un-entitled caller reading the service learns nothing about which catalogs exist.
//!
//! The second rule, from the same family: a bound this service puts on a member the model
//! constrains NOT AT ALL is the emulator's own and no refusal the modelled service has, so
//! it is answered only after every refusal the modelled service owns for that request —
//! the modelled constraints (the centralized walk) first, then the catalog lookup and the
//! entitlement gate above. A request that breaks such a bound AND names a catalog the
//! service does not hold, or comes from an un-entitled account, is therefore answered with
//! the 404 or the denial, and gets this service's own `ValidationException` only once both
//! of those passed.
//!
//! The tables span the class the rule covers rather than one call: every operation that
//! binds a `Catalog` member, split into the writing and the reporting side (and the split
//! is asserted to cover every one of them, so a new catalog-bound operation cannot escape
//! the table), crossed with several catalog names the modelled `CatalogName` pattern
//! admits (`^[A-Za-z0-9_-]+$`, `smithy_query(query="shape",shape="CatalogName")`) and no
//! catalog answers to, with both entitlements, and — on the other side — every catalog the
//! service holds. The one bad-catalog create the issue reported is one row among them.
//!
//! The second rule's rows span the same way: every write operation that owns a bound on an
//! unconstrained member (the create and the update, which carry such members directly, and
//! the amendment, which writes them through a `FieldPath`), crossed with each member the
//! bound applies to and each of the two state gates, and crossed back against the SAME
//! bodies once both gates pass — where the bound is what answers them.
//!
//! Every assertion goes over the wire through the shared
//! [`crate::benefit_application_lifecycle::Harness`] (`MultiServiceRouter::dispatch`,
//! `awsJson1_0`, `POST /`, `X-Amz-Target: PartnerCentralBenefitsService.<Operation>`).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    MAX_AMENDMENTS_PER_REQUEST, MAX_DETAILS_MEMBERS, MAX_DETAILS_VALUE_LEN, SEEDED_CATALOGS,
    VALIDATION_REASON_FIELD,
};

use crate::amend_benefit_application::{amend_body, amendment};
use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::catalog_resolution_one_accessor::store_snapshot;
use crate::details_document_guard::flat_object;
use crate::service_error_rules::{
    assert_validation_invariants, clause, message_of, one, pattern_clause,
    APPLICATION_IDENTIFIER_PATTERN, BENEFIT_IDENTIFIER_PATTERN, UNKNOWN_APPLICATION,
    UNKNOWN_BENEFIT,
};
use crate::unknown_catalog_lookup::{
    assert_catalog_not_found, catalog_bound_requests, unknown_catalogs, with_catalog,
};

/// The status this service serves its `AccessDeniedException` with, written out here
/// rather than read from the service's own constant so the value itself is under test. The
/// shape's modelled `@httpError` is 403 and the override is recorded behaviour, pinned on
/// its own in `tests/all/entitlement_denial.rs`.
const DENIED_STATUS: u16 = 400;

/// The sentence the entitlement gate refuses with, written out here rather than read from
/// the service's own constant so the text itself is under test. It carries no variable
/// part.
const ENTITLEMENT_MESSAGE: &str = "This action requires an active AWS partner benefit. \
Register as an AWS partner if you haven't already, or contact AWS partner central \
support.";

/// The catalog-bound operations that WRITE: each one mints, updates or retires a record
/// inside the catalog its request names (`smithy_query(query="digest")` for the surface,
/// and each operation's own documented effect for which side it is on).
const WRITING_OPERATIONS: &[&str] = &[
    "AmendBenefitApplication",
    "AssociateBenefitApplicationResource",
    "CancelBenefitApplication",
    "CreateBenefitApplication",
    "DisassociateBenefitApplicationResource",
    "RecallBenefitApplication",
    "SubmitBenefitApplication",
    "UpdateBenefitApplication",
];

/// The catalog-bound operations that only REPORT: a get or a listing of records the
/// catalog holds, which change nothing.
const REPORTING_OPERATIONS: &[&str] = &[
    "GetBenefit",
    "GetBenefitAllocation",
    "GetBenefitApplication",
    "ListBenefitAllocations",
    "ListBenefitApplications",
    "ListBenefits",
];

/// One well-formed request per catalog-bound operation, as the table drives it.
type Requests = Vec<(&'static str, Value)>;

/// The well-formed request per catalog-bound operation, split along the rule: the writing
/// side first, the reporting side second.
///
/// The split is checked against the whole set the sibling suite keeps, so an operation
/// that binds a `Catalog` and is named on neither list fails here instead of going
/// untested.
fn requests_by_side() -> (Requests, Requests) {
    let requests = catalog_bound_requests();
    let mut writing: Requests = Vec::new();
    let mut reporting: Requests = Vec::new();
    for (operation, body) in requests {
        if WRITING_OPERATIONS.contains(&operation) {
            writing.push((operation, body));
        } else if REPORTING_OPERATIONS.contains(&operation) {
            reporting.push((operation, body));
        } else {
            panic!(
                "case {operation:?}: a catalog-bound operation must be named on the writing or \
                 the reporting side of this suite"
            );
        }
    }
    let mut covered: Vec<&str> = writing
        .iter()
        .chain(reporting.iter())
        .map(|(operation, _)| *operation)
        .collect();
    covered.sort_unstable();
    let mut expected: Vec<&str> = WRITING_OPERATIONS
        .iter()
        .chain(REPORTING_OPERATIONS.iter())
        .copied()
        .collect();
    expected.sort_unstable();
    assert_eq!(
        covered, expected,
        "the two sides together must span every catalog-bound operation, got {covered:?}"
    );
    (writing, reporting)
}

/// Require the entitlement gate's own answer: [`DENIED_STATUS`], `AccessDeniedException`
/// and the fixed entitlement sentence.
async fn assert_denied(harness: &Harness, case: &str, operation: &str, body: &Value) {
    let (status, response) = harness.call(operation, body).await;
    assert_eq!(
        status, DENIED_STATUS,
        "case {case:?} on {operation}: expected {DENIED_STATUS}, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "AccessDeniedException",
        "case {case:?} on {operation}: expected AccessDeniedException, got {response:?}"
    );
    let message = message_of(&response).unwrap_or_default();
    assert_eq!(
        message, ENTITLEMENT_MESSAGE,
        "case {case:?} on {operation}: expected the entitlement sentence, got {message:?}"
    );
}

/// The rule's own table: on every WRITING operation, a catalog this service does not hold
/// is answered with the catalog lookup's 404 — the same answer, down to the message, for a
/// caller the entitlement gate would deny and for one it would admit, because the catalog
/// is resolved before the gate is consulted.
#[tokio::test]
async fn a_writing_operation_answers_an_unheld_catalog_ahead_of_the_entitlement_gate() {
    let unentitled = Harness::unentitled();
    let entitled = Harness::new();
    let (writing, _) = requests_by_side();
    for (name, catalog) in unknown_catalogs() {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog.as_str()),
            "case {name:?}: {catalog:?} must name no catalog this service holds"
        );
        for (operation, body) in &writing {
            let body = with_catalog(body, &catalog);
            // Both entitlements, pinned against the same expected answer, so the refusal
            // is the resource lookup's and not the account's.
            let case = format!("{name}, un-entitled");
            assert_catalog_not_found(&unentitled, &case, operation, &body).await;
            let case = format!("{name}, entitled");
            assert_catalog_not_found(&entitled, &case, operation, &body).await;
        }
    }
}

/// The other side of the gate's order: once the catalog resolves, a writing request from
/// an account with no active AWS partner benefit is still refused by the gate — on every
/// catalog this service holds, so the fix moved the catalog refusal ahead of the denial
/// without weakening it.
#[tokio::test]
async fn the_gate_still_refuses_a_writing_request_naming_a_catalog_this_service_holds() {
    let harness = Harness::unentitled();
    let (writing, _) = requests_by_side();
    for catalog in SEEDED_CATALOGS {
        for (operation, body) in &writing {
            let body = with_catalog(body, catalog);
            assert_denied(&harness, catalog, operation, &body).await;
        }
    }
}

/// The reporting side keeps the order it had: the denial answers an un-entitled caller
/// ahead of the catalog lookup, so the answer says nothing about which catalogs exist —
/// while the SAME request from an entitled caller reaches the lookup and gets its 404, so
/// the denial was a decision about the ACCOUNT and not a constant.
#[tokio::test]
async fn a_reporting_operation_answers_the_entitlement_denial_ahead_of_its_catalog_lookup() {
    let unentitled = Harness::unentitled();
    let entitled = Harness::new();
    let (_, reporting) = requests_by_side();
    for (name, catalog) in unknown_catalogs() {
        for (operation, body) in &reporting {
            let body = with_catalog(body, &catalog);
            assert_denied(&unentitled, name, operation, &body).await;
            let case = format!("{name}, entitled");
            assert_catalog_not_found(&entitled, &case, operation, &body).await;
        }
    }
}

/// Both refusals are reads of state: a request refused for an unheld catalog, and one
/// refused by the gate, leave every record this store holds exactly as it was — so the
/// reordering above moved no write ahead of either refusal.
#[tokio::test]
async fn a_refused_request_changes_nothing_whichever_refusal_answered_it() {
    let harness = Harness::unentitled();
    let before = store_snapshot(&harness);
    let (writing, reporting) = requests_by_side();
    for (name, catalog) in unknown_catalogs() {
        for (operation, body) in writing.iter().chain(reporting.iter()) {
            let body = with_catalog(body, &catalog);
            let (status, response) = harness.call(operation, &body).await;
            assert!(
                [404, DENIED_STATUS].contains(&status),
                "case {name:?} on {operation}: expected one of the two refusals, got {status} \
                 with {response:?}"
            );
        }
    }
    for catalog in SEEDED_CATALOGS {
        for (operation, body) in writing.iter().chain(reporting.iter()) {
            let body = with_catalog(body, catalog);
            assert_denied(&harness, catalog, operation, &body).await;
        }
    }
    let after = store_snapshot(&harness);
    assert_eq!(
        after, before,
        "a refused request must change no stored record"
    );
}

// ---------------------------------------------------------------------------
// The second rule: a bound this service owns answers after the state gates
// ---------------------------------------------------------------------------

/// The write operations that enforce a bound of this service's OWN on a member the model
/// constrains not at all: the create and the update, which carry `PartnerContacts`,
/// `FileDetails` and the unconstrained `BenefitApplicationDetails` document directly, and
/// the amendment, which writes those same members through a `FieldPath` and bounds how
/// many amendments one request carries (`smithy_query(query="operation",
/// operation="CreateBenefitApplication,UpdateBenefitApplication,AmendBenefitApplication")`
/// shows the model puts no constraint on `Contact$Phone`, `FileInput$BusinessUseCase`,
/// `Document` or `AmendmentList`). Every other writing operation carries no such member.
const OPERATIONS_WITH_A_BOUND_OF_THEIR_OWN: &[&str] = &[
    "AmendBenefitApplication",
    "CreateBenefitApplication",
    "UpdateBenefitApplication",
];

/// The requirement a value past a length this service owns fails, written out here rather
/// than read from the service's own helper so the sentence itself is under test — the one
/// length requirement this service states (`tests/all/handler_enforced_length_wording.rs`).
fn max_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a details document holding more members than this service stores fails
/// (`tests/all/details_document_guard.rs`).
fn too_many_members() -> String {
    format!("Member must hold no more than {MAX_DETAILS_MEMBERS} members")
}

/// One request that breaks a bound this service OWNS and nothing the model constrains.
struct OwnBound {
    case: String,
    operation: &'static str,
    /// The body, naming [`CATALOG`]; the gate tables below replace that member.
    body: Value,
    /// The member path the refusal names once both state gates have passed.
    member: &'static str,
    /// The requirement that refusal states.
    requirement: String,
}

/// One request per (operation, unconstrained member) pair, each breaking that member's
/// bound by exactly one unit and satisfying every modelled constraint — the class the
/// second rule covers.
///
/// `identifier` and `revision` name the record the update and the amendment aim at, and
/// `benefit_identifier` the benefit the create applies for: a table driving a refusal the
/// state gates answer passes identifiers nothing resolves, while the table that drives the
/// bound itself passes a stored draft.
fn requests_breaking_a_bound_this_service_owns(
    benefit_identifier: &str,
    identifier: &str,
    revision: &str,
) -> Vec<OwnBound> {
    let over_long = "v".repeat(MAX_DETAILS_VALUE_LEN + 1);
    let contacts = json!([{ "Email": "ada@example.com", "Phone": over_long }]);
    let files = json!([{
        "FileURI": "s3://partner-bucket/plan.pdf",
        "BusinessUseCase": over_long,
    }]);
    let details = flat_object(MAX_DETAILS_MEMBERS + 1);
    let amendments = Value::Array(
        (0..=MAX_AMENDMENTS_PER_REQUEST)
            .map(|index| amendment(&format!("BenefitApplicationDetails.f{index}"), "x"))
            .collect(),
    );
    let phone = "partnerContacts.1.member.phone";
    let use_case = "fileDetails.1.member.businessUseCase";
    let value_bound = max_length(MAX_DETAILS_VALUE_LEN);

    let create = |token: &str, member: &str, value: &Value| {
        let mut body = json!({
            "Catalog": CATALOG,
            "ClientToken": token,
            "BenefitIdentifier": benefit_identifier,
            "Name": "own-bound-order",
        });
        body[member] = value.clone();
        body
    };
    let update = |token: &str, member: &str, value: &Value| {
        let mut body = json!({
            "Catalog": CATALOG,
            "ClientToken": token,
            "Identifier": identifier,
            "Revision": revision,
        });
        body[member] = value.clone();
        body
    };

    let rows = vec![
        OwnBound {
            case: "a create whose contact Phone is one character past the bound".to_string(),
            operation: "CreateBenefitApplication",
            body: create("own-bound-create-phone", "PartnerContacts", &contacts),
            member: phone,
            requirement: value_bound.clone(),
        },
        OwnBound {
            case: "a create whose attachment BusinessUseCase is one character past the bound"
                .to_string(),
            operation: "CreateBenefitApplication",
            body: create("own-bound-create-use-case", "FileDetails", &files),
            member: use_case,
            requirement: value_bound.clone(),
        },
        OwnBound {
            case: "a create whose details document holds one member too many".to_string(),
            operation: "CreateBenefitApplication",
            body: create(
                "own-bound-create-details",
                "BenefitApplicationDetails",
                &details,
            ),
            member: "benefitApplicationDetails",
            requirement: too_many_members(),
        },
        OwnBound {
            case: "an update whose contact Phone is one character past the bound".to_string(),
            operation: "UpdateBenefitApplication",
            body: update("own-bound-update-phone", "PartnerContacts", &contacts),
            member: phone,
            requirement: value_bound.clone(),
        },
        OwnBound {
            case: "an update whose attachment BusinessUseCase is one character past the bound"
                .to_string(),
            operation: "UpdateBenefitApplication",
            body: update("own-bound-update-use-case", "FileDetails", &files),
            member: use_case,
            requirement: value_bound.clone(),
        },
        OwnBound {
            case: "an update whose details document holds one member too many".to_string(),
            operation: "UpdateBenefitApplication",
            body: update(
                "own-bound-update-details",
                "BenefitApplicationDetails",
                &details,
            ),
            member: "benefitApplicationDetails",
            requirement: too_many_members(),
        },
        OwnBound {
            case: "an amendment carrying one amendment too many".to_string(),
            operation: "AmendBenefitApplication",
            body: amend_body(identifier, revision, "own-bound-amend-count", amendments),
            member: "amendments",
            requirement: max_length(MAX_AMENDMENTS_PER_REQUEST),
        },
        OwnBound {
            case: "an amendment writing a Phone one character past the bound".to_string(),
            operation: "AmendBenefitApplication",
            body: amend_body(
                identifier,
                revision,
                "own-bound-amend-phone",
                json!([amendment("PartnerContacts[0].Phone", &over_long)]),
            ),
            member: "amendments.1.member.newValue",
            requirement: value_bound,
        },
    ];

    // The table must span every write that owns such a bound, so one that grew a member of
    // this kind cannot escape the rule, and each of them is on the writing side above.
    let mut covered: Vec<&str> = rows.iter().map(|row| row.operation).collect();
    covered.sort_unstable();
    covered.dedup();
    assert_eq!(
        covered, OPERATIONS_WITH_A_BOUND_OF_THEIR_OWN,
        "the table must span every write that bounds an unconstrained member, got {covered:?}"
    );
    for operation in covered {
        assert!(
            WRITING_OPERATIONS.contains(&operation),
            "case {operation:?}: a request that writes must be on the writing side"
        );
    }
    rows
}

/// One request that breaks a MODELLED constraint as well as a bound this service owns: the
/// walk's refusal is what such a request is answered with, wherever the gates would have
/// refused it.
struct ModelledFirst {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The message the centralized walk's refusal carries, naming the modelled member.
    expected: String,
}

/// One such request per write that owns a bound: the identifier member carries a value
/// outside its modelled `@pattern`, and the same body also breaks this service's own bound.
fn requests_breaking_a_modelled_constraint_too() -> Vec<ModelledFirst> {
    let over_long = "v".repeat(MAX_DETAILS_VALUE_LEN + 1);
    let contacts = json!([{ "Email": "ada@example.com", "Phone": over_long }]);
    let amendments = Value::Array(
        (0..=MAX_AMENDMENTS_PER_REQUEST)
            .map(|index| amendment(&format!("BenefitApplicationDetails.f{index}"), "x"))
            .collect(),
    );
    vec![
        ModelledFirst {
            case: "a create naming a benefit outside the modelled pattern",
            operation: "CreateBenefitApplication",
            body: json!({
                "Catalog": CATALOG,
                "ClientToken": "modelled-first-create",
                "BenefitIdentifier": "not-a-benefit-identifier",
                "Name": "own-bound-order",
                "PartnerContacts": contacts,
            }),
            expected: one(&pattern_clause(
                "benefitIdentifier",
                BENEFIT_IDENTIFIER_PATTERN,
            )),
        },
        ModelledFirst {
            case: "an update naming an application outside the modelled pattern",
            operation: "UpdateBenefitApplication",
            body: json!({
                "Catalog": CATALOG,
                "ClientToken": "modelled-first-update",
                "Identifier": "not-an-application-identifier",
                "Revision": "1",
                "PartnerContacts": contacts,
            }),
            expected: one(&pattern_clause(
                "identifier",
                APPLICATION_IDENTIFIER_PATTERN,
            )),
        },
        ModelledFirst {
            case: "an amendment naming an application outside the modelled pattern",
            operation: "AmendBenefitApplication",
            body: amend_body(
                "not-an-application-identifier",
                "1",
                "modelled-first-amend",
                amendments,
            ),
            expected: one(&pattern_clause(
                "identifier",
                APPLICATION_IDENTIFIER_PATTERN,
            )),
        },
    ]
}

/// Require this service's own refusal: 400 `ValidationException` whose message is the
/// envelope over the clause naming `member` and `requirement`, with the field-validation
/// reason.
fn assert_own_bound_refused(case: &str, status: u16, out: &Value, member: &str, requirement: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from this service's own bound, got {status} with {out:?}"
    );
    assert_eq!(
        error_code(out),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {out:?}"
    );
    let message = message_of(out)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {out:?}"));
    assert_validation_invariants(case, &message);
    let expected = one(&clause(member, requirement));
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    assert_eq!(
        out["Reason"],
        json!(VALIDATION_REASON_FIELD),
        "case {case:?}: expected the field-validation reason, got {out:?}"
    );
}

/// A catalog this service does not hold outranks every bound this service owns: the answer
/// is the catalog lookup's 404, identical for a caller the gate would deny and one it would
/// admit, on every write that owns such a bound and for every member it bounds.
#[tokio::test]
async fn an_unheld_catalog_outranks_a_bound_this_service_owns() {
    let unentitled = Harness::unentitled();
    let entitled = Harness::new();
    // No record answers to either identifier, which is what makes the rows below a
    // question about the ORDER and not about what the store holds.
    let rows =
        || requests_breaking_a_bound_this_service_owns(UNKNOWN_BENEFIT, UNKNOWN_APPLICATION, "1");
    for (name, catalog) in unknown_catalogs() {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog.as_str()),
            "case {name:?}: {catalog:?} must name no catalog this service holds"
        );
        for row in rows() {
            let body = with_catalog(&row.body, &catalog);
            let case = format!("{name}, {}, un-entitled", row.case);
            assert_catalog_not_found(&unentitled, &case, row.operation, &body).await;
            let case = format!("{name}, {}, entitled", row.case);
            assert_catalog_not_found(&entitled, &case, row.operation, &body).await;
        }
    }
}

/// The entitlement gate outranks them too: with the catalog resolved, a request from an
/// account holding no active AWS partner benefit is denied rather than told about a bound
/// this service invented — on every catalog the service holds.
#[tokio::test]
async fn the_entitlement_gate_outranks_a_bound_this_service_owns() {
    let harness = Harness::unentitled();
    let rows =
        || requests_breaking_a_bound_this_service_owns(UNKNOWN_BENEFIT, UNKNOWN_APPLICATION, "1");
    for catalog in SEEDED_CATALOGS {
        for row in rows() {
            let body = with_catalog(&row.body, catalog);
            let case = format!("{catalog}, {}", row.case);
            assert_denied(&harness, &case, row.operation, &body).await;
        }
    }
}

/// The other side of that order, so the two tables above pin a decision and not a constant:
/// the SAME bodies, with a catalog the service holds and an entitled caller, are answered
/// by the bound itself — 400 `ValidationException` naming the member and the requirement —
/// and change no stored record.
#[tokio::test]
async fn a_bound_this_service_owns_answers_once_both_state_gates_have_passed() {
    let harness = Harness::new();
    let benefit_identifier = text("seed", &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "own-bound-order-seed",
                "BenefitIdentifier": benefit_identifier,
                "Name": "own-bound-order",
                // Bounded values in both list members, so the amendment row below has an
                // element to write and moves nothing but the value under test.
                "PartnerContacts": [{ "Email": "ada@example.com", "Phone": "+1-555-0100" }],
                "FileDetails": [{
                    "FileURI": "s3://partner-bucket/plan.pdf",
                    "BusinessUseCase": "funding request",
                }],
            }),
        )
        .await;
    let identifier = text("seed", &created, "Id");
    let revision = text("seed", &created, "Revision");

    let before = harness.get("seed", &identifier).await;
    let snapshot = store_snapshot(&harness);
    for row in
        requests_breaking_a_bound_this_service_owns(&benefit_identifier, &identifier, &revision)
    {
        let (status, out) = harness.call(row.operation, &row.body).await;
        assert_own_bound_refused(&row.case, status, &out, row.member, &row.requirement);
    }
    let after = harness.get("after", &identifier).await;
    assert_eq!(
        after, before,
        "a request refused by a bound this service owns must change nothing, got {after:?}"
    );
    assert_eq!(
        store_snapshot(&harness),
        snapshot,
        "a request refused by a bound this service owns must change no stored record"
    );
}

/// Above both gates sits the walk over the MODELLED constraints: a request that breaks one
/// of those as well as a bound this service owns is answered with the walk's refusal,
/// naming the modelled member — whether the catalog it names is one the service holds and
/// whatever the calling account holds.
#[tokio::test]
async fn a_modelled_constraint_outranks_the_state_gates_and_this_services_own_bounds() {
    let unentitled = Harness::unentitled();
    let entitled = Harness::new();
    for row in requests_breaking_a_modelled_constraint_too() {
        // (case, the harness, the catalog the request names)
        let probes: [(String, &Harness, String); 3] = [
            (
                format!("{}, a catalog the service holds, entitled", row.case),
                &entitled,
                CATALOG.to_string(),
            ),
            (
                format!("{}, a catalog the service holds, un-entitled", row.case),
                &unentitled,
                CATALOG.to_string(),
            ),
            (
                format!("{}, a catalog the service does not hold, un-entitled", row.case),
                &unentitled,
                "not-a-real-catalog-34556872".to_string(),
            ),
        ];
        for (case, harness, catalog) in probes {
            let body = with_catalog(&row.body, &catalog);
            let (status, out) = harness.call(row.operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the walk's 400, got {status} with {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
            let message = message_of(&out).unwrap_or_else(|| {
                panic!("case {case:?}: the refusal carries no Message: {out:?}")
            });
            assert_validation_invariants(&case, &message);
            assert_eq!(
                message, row.expected,
                "case {case:?}: expected {:?}, got {message:?}",
                row.expected
            );
        }
    }
}
