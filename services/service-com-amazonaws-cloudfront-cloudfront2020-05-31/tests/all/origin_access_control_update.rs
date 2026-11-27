// SPDX-License-Identifier: CC0-1.0

//! `UpdateOriginAccessControl`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's origin access control
//! surface states for an update:
//!
//! * the update REPLACES the stored configuration with the one its payload carries, down
//!   to the members the payload leaves out, and MINTS a fresh version token — "the new
//!   version of the origin access control after it has been updated" — under the id the
//!   control was created with, so a later `GetOriginAccessControl` answers from that record
//!   and the account still holds exactly the controls it held;
//! * the update is a conditional request on the control's current version: an `If-Match`
//!   that is missing or is not a version token at all is `InvalidIfMatchVersion` (400), one
//!   carrying a well-formed token that is not the current one is `PreconditionFailed`
//!   (412), and a refused update changes nothing — so the token an update hands back is
//!   the only one the NEXT update of that control lands on;
//! * an id no control holds is `NoSuchOriginAccessControl` (404);
//! * `OriginAccessControlConfig$Name`: "You can specify up to 64 characters", so an update
//!   submitting a longer name is `InvalidArgument` (400) and the stored configuration is
//!   left as it was — an update cannot store a configuration the create would have
//!   refused;
//! * `OriginAccessControlAlreadyExists` — the parameters ANOTHER stored control holds
//!   cannot be moved onto this one (409, carrying the service's fixed sentence for that
//!   code, which names neither control), while an update re-submitting the control's OWN
//!   parameters is not a conflict with itself and still mints a fresh version.
//!
//! The table rows differ along everything these rules generalise over: the configuration
//! updated and the configuration updated TO (every row of the lifecycle suite's table, so
//! every value of every enum the configuration carries and every description form appears
//! on both sides of an update), whether the update changes one member or all of them,
//! whether the optional `Description` is added, changed or dropped, the FORM the version
//! token is presented in (absent, bare, quoted, weak, malformed, stale) and the name
//! lengths at and past the published bound.
//!
//! The harness (the router, the request helpers and the control table) is the one the
//! origin access control lifecycle and distribution lifecycle suites use, reused rather
//! than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ORIGIN_ACCESS_CONTROL_NAME, NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::origin_access_control_lifecycle::{
    create_control, get_control, list_controls, page_ids, table, Control,
};

/// The sentence `OriginAccessControlAlreadyExists` carries: the service's fixed text for
/// that code, written out here rather than read from the crate's own constant so a wrong
/// constant cannot make the assertion below pass.
const ALREADY_EXISTS_MESSAGE: &str = "An origin access control with the same name already exists.";

/// The sentence these routes answer a value written in no alphabet CloudFront mints an
/// identifier in, under the same code and status as the absent control.
const UNREADABLE_IDENTIFIER_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/origin-access-control/{Id}/config` with the
/// `OriginAccessControlConfig` document `body` — the member the operation binds to the
/// HTTP payload — and `If-Match` carried exactly as `if_match` gives it (left off entirely
/// when it is `None`).
async fn update_control(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/origin-access-control/{id}/config"),
        Some(body),
        if_match,
    )
    .await
}

/// Create one control through the API and report `(Id, ETag)`.
async fn create(router: &MultiServiceRouter, row: &Control) -> (String, String) {
    let created = create_control(router, &row.xml()).await;
    assert_eq!(
        created.status,
        201,
        "case {}: expected HTTP 201 from CreateOriginAccessControl, got {} with {}",
        row.case,
        created.status,
        created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    assert!(
        is_version_token(&etag),
        "case {}: expected a minted version token in ETag, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// Assert that the control `id` names reads back as `expected` at version `etag`.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    id: &str,
    etag: &str,
    expected: &Control,
    context: &str,
) {
    let fetched = get_control(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {} ({context}): expected the control to be readable, got {} with {}",
        expected.case, fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {} ({context}): expected the read to report version {etag}, got {:?}",
        expected.case,
        fetched.header("etag")
    );
    assert_eq!(
        fetched.root().text_at("Id"),
        Some(id),
        "case {} ({context}): expected the read to report Id {id}, got {:?} with {}",
        expected.case,
        fetched.root().text_at("Id"),
        fetched.raw
    );
    expected.assert_matches(
        fetched
            .root()
            .child("OriginAccessControlConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {} ({context}): no OriginAccessControlConfig in {}",
                    expected.case, fetched.raw
                )
            }),
        context,
        &fetched.raw,
    );
}

/// The ids the account holds, as a listing reports them.
async fn stored_ids(router: &MultiServiceRouter) -> Vec<String> {
    let listed = list_controls(router, "").await;
    assert_eq!(
        listed.status, 200,
        "case the whole account: expected HTTP 200 from ListOriginAccessControls, got {} with {}",
        listed.status, listed.raw
    );
    page_ids(&listed)
}

// ---------------------------------------------------------------------------
// An update replaces the configuration every later read answers from
// ---------------------------------------------------------------------------

/// The configurations an update is run FROM and TO: every row of the table updated to the
/// next row's configuration, so each value of every enum and each description form appears
/// on both sides of an update, plus the single-member variations (a description added,
/// changed, emptied and dropped, and a rename) of the first row.
fn transitions() -> Vec<(Control, Control)> {
    let rows = table();
    let mut cases: Vec<(Control, Control)> = Vec::new();
    for (index, from) in rows.iter().enumerate() {
        let to = &rows[(index + 1) % rows.len()];
        cases.push((
            from.clone(),
            to.with_name(
                &format!("{} -> {}", from.case, to.case),
                // A name of its own, so the target configuration is one no stored control
                // holds: what this row is about is the REPLACEMENT, not the conflict.
                &format!("oac-updated-to-{index}"),
            ),
        ));
    }

    let base = &rows[0];
    for (case, description) in [
        ("a description added", Some("a description the create left out")),
        ("a description changed", Some("a description that replaces another")),
        ("a description emptied", Some("")),
        ("a description dropped", None),
    ] {
        cases.push((
            base.with_description(&format!("{case} (before)"), Some("the original description")),
            base.with_description(case, description),
        ));
    }
    cases.push((
        base.clone(),
        base.with_name("a rename at the published bound", &"r".repeat(MAX_ORIGIN_ACCESS_CONTROL_NAME)),
    ));
    // `Name` is bounded in CHARACTERS ("You can specify up to 64 characters"), so a name of
    // that many multi-byte characters is inside the bound.
    cases.push((
        base.clone(),
        base.with_name(
            "a rename to multi-byte characters at the published bound",
            &"é".repeat(MAX_ORIGIN_ACCESS_CONTROL_NAME),
        ),
    ));
    cases
}

/// An update replaces the whole stored configuration under the control's own id, mints a
/// fresh version token, and leaves the account holding exactly the controls it held.
#[tokio::test]
async fn an_update_replaces_the_whole_configuration() {
    for (from, to) in transitions() {
        let case = to.case.clone();
        let router = emulator();
        let (id, created_etag) = create(&router, &from).await;
        let before = stored_ids(&router).await;

        let updated = update_control(&router, &id, &to.xml(), Some(&created_etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateOriginAccessControl, got {} with {}",
            updated.status, updated.raw
        );

        // The answer reports the record just written: the same identity, the submitted
        // configuration and a NEW version token.
        assert_eq!(
            updated.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the update to report Id {id}, got {:?} with {}",
            updated.root().text_at("Id"),
            updated.raw
        );
        let etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a minted version token in ETag, got {etag:?}"
        );
        assert_ne!(
            etag, created_etag,
            "case {case}: expected a fresh version token, got {created_etag} again"
        );
        to.assert_matches(
            updated
                .root()
                .child("OriginAccessControlConfig")
                .unwrap_or_else(|| {
                    panic!("case {case}: no OriginAccessControlConfig in {}", updated.raw)
                }),
            "UpdateOriginAccessControl",
            &updated.raw,
        );

        // A later read answers from the stored record, at the version the update minted.
        assert_reads_back(
            &router,
            &id,
            &etag,
            &to,
            "GetOriginAccessControl after an update",
        )
        .await;

        // The account holds the same controls: an update is not a create.
        let after = stored_ids(&router).await;
        assert_eq!(
            after, before,
            "case {case}: expected the account to hold {before:?} after the update, got {after:?}"
        );

        // The version the update replaced no longer lands: the token it handed back is the
        // only one the next update of this control matches on.
        let stale = update_control(&router, &id, &from.xml(), Some(&created_etag)).await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected HTTP 412 for the version the update replaced, got {} with {}",
            stale.status, stale.raw
        );
        let again = update_control(&router, &id, &from.xml(), Some(&etag)).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected the token the update minted to land, got {} with {}",
            again.status, again.raw
        );
        assert_reads_back(
            &router,
            &id,
            again.header("etag"),
            &from,
            "GetOriginAccessControl after updating back",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// The conditional request
// ---------------------------------------------------------------------------

/// How a row presents the version token to the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    /// No `If-Match` header at all.
    Absent,
    /// The control's current version, exactly as the `ETag` header reported it.
    Current,
    /// The current version in HTTP entity-tag quotes.
    QuotedCurrent,
    /// The current version as a weak entity tag.
    WeakCurrent,
    /// A literal that is not a version token at all.
    Malformed(&'static str),
    /// A well-formed version token that is another control's — so not this one's.
    OtherControl,
}

/// One row of the precondition table.
#[derive(Debug, Clone, Copy)]
struct Precondition {
    case: &'static str,
    version: Version,
    /// The status the update is expected to answer with.
    status: u16,
    /// The error code the update is expected to answer with, or `None` when it lands.
    code: Option<&'static str>,
}

/// Every form the `If-Match` value takes, and what each makes of the update.
const PRECONDITIONS: &[Precondition] = &[
    Precondition {
        case: "no-if-match",
        version: Version::Absent,
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "empty-if-match",
        version: Version::Malformed(""),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-that-is-not-a-token",
        version: Version::Malformed("not-a-version"),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-token-of-the-wrong-length",
        version: Version::Malformed("E123"),
        status: 400,
        code: Some("InvalidIfMatchVersion"),
    },
    Precondition {
        case: "if-match-naming-another-control",
        version: Version::OtherControl,
        status: 412,
        code: Some("PreconditionFailed"),
    },
    Precondition {
        case: "if-match-current",
        version: Version::Current,
        status: 200,
        code: None,
    },
    Precondition {
        case: "if-match-current-quoted",
        version: Version::QuotedCurrent,
        status: 200,
        code: None,
    },
    Precondition {
        case: "if-match-current-weak",
        version: Version::WeakCurrent,
        status: 200,
        code: None,
    },
];

/// An update lands only when the caller holds the control's current version: every other
/// form of `If-Match` is the refusal the operation models for it, and leaves the stored
/// configuration and its version exactly as they were.
#[tokio::test]
async fn an_update_lands_only_on_the_current_version() {
    let rows = table();
    for precondition in PRECONDITIONS {
        let case = precondition.case;
        let router = emulator();
        // Two controls, so a well-formed token that is not the target's is available
        // without inventing one.
        let target = &rows[0];
        let other = &rows[1];
        let (id, etag) = create(&router, target).await;
        let (other_id, other_etag) = create(&router, other).await;
        let submitted = target.with_name(case, &format!("oac-{case}"));

        let quoted = format!("\"{etag}\"");
        let weak = format!("W/\"{etag}\"");
        let if_match: Option<&str> = match precondition.version {
            Version::Absent => None,
            Version::Current => Some(&etag),
            Version::QuotedCurrent => Some(&quoted),
            Version::WeakCurrent => Some(&weak),
            Version::Malformed(literal) => Some(literal),
            Version::OtherControl => Some(&other_etag),
        };

        let updated = update_control(&router, &id, &submitted.xml(), if_match).await;
        assert_eq!(
            updated.status, precondition.status,
            "case {case}: expected HTTP {} from UpdateOriginAccessControl, got {} with {}",
            precondition.status, updated.status, updated.raw
        );
        match precondition.code {
            None => {
                let minted = updated.header("etag").to_string();
                assert_ne!(
                    minted, etag,
                    "case {case}: expected a fresh version token, got {etag} again"
                );
                assert_reads_back(
                    &router,
                    &id,
                    &minted,
                    &submitted,
                    "GetOriginAccessControl after an accepted update",
                )
                .await;
            }
            Some(code) => {
                assert_eq!(
                    updated.error_code(),
                    code,
                    "case {case}: expected {code}, got {:?} with {}",
                    updated.error_code(),
                    updated.raw
                );
                // A refused update writes nothing: the control is still readable, at the
                // same version and with the configuration it held.
                assert_reads_back(
                    &router,
                    &id,
                    &etag,
                    target,
                    "GetOriginAccessControl after a refused update",
                )
                .await;
            }
        }

        // The control the row did not target is untouched either way.
        assert_reads_back(
            &router,
            &other_id,
            &other_etag,
            other,
            "GetOriginAccessControl of the control the update did not name",
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// An id no control holds
// ---------------------------------------------------------------------------

/// An id no control holds is `NoSuchOriginAccessControl`, whatever the request says about
/// the version — and nothing the account holds is touched.
#[tokio::test]
async fn an_id_no_control_holds_is_refused() {
    let router = emulator();
    let rows = table();
    let (id, etag) = create(&router, &rows[0]).await;

    // Ids no control holds: one of the shape this service mints, one of another entity
    // family's shape, and one that is no entity id at all. The ALPHABET the value is written in
    // is what decides the sentence — `Id` carries no `@pattern` and no `@length`, so the value
    // of the alphabet an identifier is minted in is resolved against the store and reported as
    // the control that is absent, while the two written outside it could never have been an
    // identifier of this mint and are the identifier the route reports it cannot read.
    for (case, missing, expected) in [
        (
            "minted-shape",
            "E1A2B3C4D5E6F7",
            NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE,
        ),
        (
            "another-family",
            "cg_1A2B3C4D5E6F7",
            UNREADABLE_IDENTIFIER_MESSAGE,
        ),
        ("not-an-id", "no-such-control", UNREADABLE_IDENTIFIER_MESSAGE),
    ] {
        for (version_case, if_match) in [
            ("no-if-match", None),
            ("current-of-another", Some(etag.as_str())),
        ] {
            let answer = update_control(&router, missing, &rows[1].xml(), if_match).await;
            assert_eq!(
                answer.status, 404,
                "case {case}/{version_case}: expected HTTP 404 from \
                 UpdateOriginAccessControl, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchOriginAccessControl",
                "case {case}/{version_case}: expected NoSuchOriginAccessControl, got {:?} \
                 with {}",
                answer.error_code(),
                answer.raw
            );
            assert!(
                answer.raw.contains(expected),
                "case {case}/{version_case}: expected the refusal to state {expected:?}, got {}",
                answer.raw
            );
        }
    }

    // The control the account does hold is exactly as it was.
    assert_reads_back(
        &router,
        &id,
        &etag,
        &rows[0],
        "GetOriginAccessControl after updates of ids no control holds",
    )
    .await;
}

// ---------------------------------------------------------------------------
// The configuration the update submits
// ---------------------------------------------------------------------------

/// `OriginAccessControlConfig$Name`: "You can specify up to 64 characters" — the bound an
/// update is held to, in characters, exactly as the create is.
#[tokio::test]
async fn a_name_past_the_published_bound_is_refused() {
    let rows = table();
    // The published bound, one inside it, and one past it — in single-byte and in
    // multi-byte characters, because the bound counts characters.
    let lengths = [
        ("one character", 1usize, true),
        ("one inside the bound", MAX_ORIGIN_ACCESS_CONTROL_NAME - 1, true),
        ("the published bound", MAX_ORIGIN_ACCESS_CONTROL_NAME, true),
        ("one past the bound", MAX_ORIGIN_ACCESS_CONTROL_NAME + 1, false),
        ("far past the bound", MAX_ORIGIN_ACCESS_CONTROL_NAME * 4, false),
    ];

    for (character, letter) in [("ascii", "n"), ("multi-byte", "é")] {
        for (label, length, accepted) in lengths {
            let case = format!("{character} name of {length} characters ({label})");
            let router = emulator();
            let (id, etag) = create(&router, &rows[0]).await;
            let submitted = rows[0].with_name(&case, &letter.repeat(length));

            let updated = update_control(&router, &id, &submitted.xml(), Some(&etag)).await;
            if accepted {
                assert_eq!(
                    updated.status, 200,
                    "case {case}: expected HTTP 200 from UpdateOriginAccessControl, got {} \
                     with {}",
                    updated.status, updated.raw
                );
                assert_reads_back(
                    &router,
                    &id,
                    updated.header("etag"),
                    &submitted,
                    "GetOriginAccessControl after a name inside the bound",
                )
                .await;
            } else {
                assert_eq!(
                    updated.status, 400,
                    "case {case}: expected HTTP 400 from UpdateOriginAccessControl, got {} \
                     with {}",
                    updated.status, updated.raw
                );
                assert_eq!(
                    updated.error_code(),
                    "InvalidArgument",
                    "case {case}: expected InvalidArgument, got {:?} with {}",
                    updated.error_code(),
                    updated.raw
                );
                // A refused update writes nothing.
                assert_reads_back(
                    &router,
                    &id,
                    &etag,
                    &rows[0],
                    "GetOriginAccessControl after a name past the bound",
                )
                .await;
            }
        }
    }
}

/// `OriginAccessControlAlreadyExists`: the parameters ANOTHER stored control holds cannot
/// be moved onto this one — and the refusal carries the service's fixed sentence for that
/// code — while an update re-submitting the control's OWN parameters is not a conflict with
/// itself.
#[tokio::test]
async fn parameters_another_control_holds_cannot_be_moved_onto_this_one() {
    let rows = table();
    for pair in rows.chunks(2).take(4) {
        if pair.len() < 2 {
            continue;
        }
        let (target, other) = (&pair[0], &pair[1]);
        let case = format!("{} taking over {}", target.case, other.case);
        let router = emulator();
        let (id, etag) = create(&router, target).await;
        let (other_id, other_etag) = create(&router, other).await;

        let conflict = update_control(&router, &id, &other.xml(), Some(&etag)).await;
        assert_eq!(
            conflict.status, 409,
            "case {case}: expected HTTP 409 from UpdateOriginAccessControl, got {} with {}",
            conflict.status, conflict.raw
        );
        assert_eq!(
            conflict.error_code(),
            "OriginAccessControlAlreadyExists",
            "case {case}: expected OriginAccessControlAlreadyExists, got {:?} with {}",
            conflict.error_code(),
            conflict.raw
        );
        // The refusal carries the service's fixed sentence for this code, so it names
        // neither the parameters submitted nor the control that holds them. The sentence is
        // a LITERAL here rather than the crate's own constant, so a wrong constant cannot
        // make this assertion pass.
        let message = conflict.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, ALREADY_EXISTS_MESSAGE,
            "case {case}: expected the fixed conflict sentence, got {}",
            conflict.raw
        );
        assert!(
            !conflict.raw.contains(other_id.as_str()),
            "case {case}: expected the refusal not to name the control that holds the \
             parameters ({other_id}), got {}",
            conflict.raw
        );
        // Both controls are exactly as they were.
        assert_reads_back(
            &router,
            &id,
            &etag,
            target,
            "GetOriginAccessControl after a refused take-over",
        )
        .await;
        assert_reads_back(
            &router,
            &other_id,
            &other_etag,
            other,
            "GetOriginAccessControl of the control that holds the parameters",
        )
        .await;

        // The control's own parameters are not a conflict with itself: the update lands and
        // mints a fresh version, which is the one the next update matches on.
        let itself = update_control(&router, &id, &target.xml(), Some(&etag)).await;
        assert_eq!(
            itself.status, 200,
            "case {case}: expected an update re-submitting the control's own parameters to \
             land, got {} with {}",
            itself.status, itself.raw
        );
        let minted = itself.header("etag").to_string();
        assert_ne!(
            minted, etag,
            "case {case}: expected a fresh version token, got {etag} again"
        );
        assert_reads_back(
            &router,
            &id,
            &minted,
            target,
            "GetOriginAccessControl after an update to its own parameters",
        )
        .await;
    }
}
