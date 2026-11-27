// SPDX-License-Identifier: CC0-1.0

//! `DeleteOriginAccessControl`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's origin access control
//! surface states for a delete:
//!
//! * a delete REMOVES the stored record, so it answers 204 with no body and every later
//!   read of that id answers `NoSuchOriginAccessControl` (404) while every other stored
//!   control stays readable at its own version — and the parameters the removed control
//!   held are free again, so a create repeating them is no longer the conflict it was;
//! * the delete is a conditional request on the control's current version: an `If-Match`
//!   that is missing or is not a version token at all is `InvalidIfMatchVersion` (400),
//!   one carrying a well-formed token that is not the current one is `PreconditionFailed`
//!   (412), and a refused delete changes nothing;
//! * an id no control holds is `NoSuchOriginAccessControl` (404);
//! * "You cannot delete an origin access control if it's in use. First, update all
//!   distributions to remove the origin access control from all origins, then delete the
//!   origin access control." — a control an origin of a stored distribution still names is
//!   `OriginAccessControlInUse` (409) and stays readable, and the same delete lands once
//!   no origin names it.
//!
//! The table rows differ along everything these rules generalise over: the configuration
//! deleted (every row of the lifecycle suite's table — each value of every enum the
//! configuration carries, and each description form), how many controls the account holds
//! around the one being deleted, the FORM the version token is presented in (absent, bare,
//! quoted, weak, malformed, stale), and where in a distribution's origin list the
//! reference to the control sits.
//!
//! The harness (the router, the request helpers and the control table) is the one the
//! origin access control lifecycle and distribution lifecycle suites use, reused rather
//! than copied.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, is_version_token, update_distribution, Answer,
    API_VERSION,
};
use crate::origin_access_control_lifecycle::{
    create_control, get_control, list_controls, page_ids, table, Control,
};

/// The sentence these routes answer a value written in no alphabet CloudFront mints an
/// identifier in, under the same code and status as the absent control.
const UNREADABLE_IDENTIFIER_MESSAGE: &str = "Id format is invalid";

// ---------------------------------------------------------------------------
// Calling the surface under test
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/origin-access-control/{Id}`, with `If-Match` carried exactly as
/// `if_match` gives it (and left off entirely when it is `None`).
async fn delete_control(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/origin-access-control/{id}"),
        None,
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

// ---------------------------------------------------------------------------
// A delete removes the record every read answered from
// ---------------------------------------------------------------------------

/// Deleting each stored control in turn removes exactly that control: it answers 204 with
/// no body, its id answers 404 afterwards, and every control still stored answers at the
/// version it was created with.
#[tokio::test]
async fn a_delete_removes_exactly_the_control_it_names() {
    let router = emulator();
    let rows = table();

    // The whole table is stored first, so every delete below runs against an account that
    // holds other controls too — the delete has to leave each of them alone.
    let mut stored: Vec<(Control, String, String)> = Vec::new();
    for row in &rows {
        let (id, etag) = create(&router, row).await;
        stored.push((row.clone(), id, etag));
    }

    while let Some((row, id, etag)) = stored.pop() {
        let case = row.case.clone();
        let deleted = delete_control(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 from DeleteOriginAccessControl, got {} with {}",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected no body on a successful delete, got {}",
            deleted.raw
        );

        // Gone from the read it was answered from...
        let fetched = get_control(&router, &id).await;
        assert_eq!(
            fetched.status, 404,
            "case {case}: expected HTTP 404 from GetOriginAccessControl after the delete, \
             got {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.error_code(),
            "NoSuchOriginAccessControl",
            "case {case}: expected NoSuchOriginAccessControl after the delete, got {:?} with {}",
            fetched.error_code(),
            fetched.raw
        );
        // ...and gone from the listing, which still reports every control left.
        let listed = list_controls(&router, "").await;
        let listed_ids = page_ids(&listed);
        assert!(
            !listed_ids.contains(&id),
            "case {case}: expected {id} to be gone from the listing, got {listed_ids:?}"
        );
        let remaining: Vec<String> = stored.iter().map(|(_, id, _)| id.clone()).collect();
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some(remaining.len().to_string().as_str()),
            "case {case}: expected {} controls left, got {}",
            remaining.len(),
            listed.raw
        );
        for left in &remaining {
            assert!(
                listed_ids.contains(left),
                "case {case}: expected {left} to survive the delete, got {listed_ids:?}"
            );
        }
        // Each surviving control still answers its own record, at its own version.
        for (other_row, other_id, other_etag) in &stored {
            let other = get_control(&router, other_id).await;
            assert_eq!(
                other.status, 200,
                "case {case}: expected {other_id} to stay readable, got {} with {}",
                other.status, other.raw
            );
            assert_eq!(
                other.header("etag"),
                *other_etag,
                "case {case}: expected {other_id} to stay at version {other_etag}, got {:?}",
                other.header("etag")
            );
            other_row.assert_matches(
                other
                    .root()
                    .child("OriginAccessControlConfig")
                    .unwrap_or_else(|| {
                        panic!("case {case}: no OriginAccessControlConfig for {other_id}")
                    }),
                "a surviving control after another was deleted",
                &other.raw,
            );
        }
    }

    // "If you're not using origin access controls for your Amazon Web Services account,
    // the ListOriginAccessControls operation doesn't return the Items element": the
    // account that deleted every control it had is back in that state.
    let listed = list_controls(&router, "").await;
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case all-deleted: expected an empty list after deleting every control, got {}",
        listed.raw
    );
    assert!(
        listed.root().child("Items").is_none(),
        "case all-deleted: expected no Items element for an account using no controls, got {}",
        listed.raw
    );
}

/// The parameters a control held are released with it: "An origin access control with the
/// specified parameters already exists" is about a STORED control, so once the control is
/// deleted the very same configuration creates a new one.
#[tokio::test]
async fn the_parameters_a_deleted_control_held_are_free_again() {
    let router = emulator();

    for row in table().iter().take(3) {
        let case = row.case.clone();
        let (id, etag) = create(&router, row).await;

        // While it is stored, the same parameters are the modelled conflict.
        let repeated = create_control(&router, &row.xml()).await;
        assert_eq!(
            repeated.status, 409,
            "case {case}: expected HTTP 409 while the control is stored, got {} with {}",
            repeated.status, repeated.raw
        );
        assert_eq!(
            repeated.error_code(),
            "OriginAccessControlAlreadyExists",
            "case {case}: expected OriginAccessControlAlreadyExists, got {:?} with {}",
            repeated.error_code(),
            repeated.raw
        );

        let deleted = delete_control(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 from DeleteOriginAccessControl, got {} with {}",
            deleted.status, deleted.raw
        );

        // Once it is gone, the same parameters describe a control the account does not
        // hold, so they are created — under an identity of their own.
        let (again_id, again_etag) = create(&router, row).await;
        assert_ne!(
            again_id, id,
            "case {case}: expected a new identity for the re-created control, got {again_id} again"
        );
        assert_ne!(
            again_etag, etag,
            "case {case}: expected a new version token for the re-created control, got \
             {again_etag} again"
        );
        let fetched = get_control(&router, &again_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected the re-created control to be readable, got {} with {}",
            fetched.status, fetched.raw
        );
        row.assert_matches(
            fetched
                .root()
                .child("OriginAccessControlConfig")
                .unwrap_or_else(|| panic!("case {case}: no OriginAccessControlConfig")),
            "GetOriginAccessControl after re-creating a deleted control",
            &fetched.raw,
        );

        // Leave the account empty for the next row.
        let cleaned = delete_control(&router, &again_id, Some(&again_etag)).await;
        assert_eq!(
            cleaned.status, 204,
            "case {case}: expected HTTP 204 when clearing the re-created control, got {} with {}",
            cleaned.status, cleaned.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The conditional request
// ---------------------------------------------------------------------------

/// How a row presents the version token to the delete.
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
    /// The status the delete is expected to answer with.
    status: u16,
    /// The error code the delete is expected to answer with, or `None` when it lands.
    code: Option<&'static str>,
}

/// Every form the `If-Match` value takes, and what each makes of the delete.
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
        status: 204,
        code: None,
    },
    Precondition {
        case: "if-match-current-quoted",
        version: Version::QuotedCurrent,
        status: 204,
        code: None,
    },
    Precondition {
        case: "if-match-current-weak",
        version: Version::WeakCurrent,
        status: 204,
        code: None,
    },
];

/// A delete lands only when the caller holds the control's current version: every other
/// form of `If-Match` is the refusal the operation models for it, and leaves the control
/// exactly as readable as it was.
#[tokio::test]
async fn a_delete_lands_only_on_the_current_version() {
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

        let deleted = delete_control(&router, &id, if_match).await;
        assert_eq!(
            deleted.status, precondition.status,
            "case {case}: expected HTTP {} from DeleteOriginAccessControl, got {} with {}",
            precondition.status, deleted.status, deleted.raw
        );
        match precondition.code {
            None => {
                let fetched = get_control(&router, &id).await;
                assert_eq!(
                    fetched.status, 404,
                    "case {case}: expected the control to be gone, got {} with {}",
                    fetched.status, fetched.raw
                );
            }
            Some(code) => {
                assert_eq!(
                    deleted.error_code(),
                    code,
                    "case {case}: expected {code}, got {:?} with {}",
                    deleted.error_code(),
                    deleted.raw
                );
                // A refused delete destroys nothing: the control is still readable, at the
                // same version and with the configuration it held.
                let after = get_control(&router, &id).await;
                assert_eq!(
                    after.status, 200,
                    "case {case}: expected the refused delete to leave the control readable, \
                     got {} with {}",
                    after.status, after.raw
                );
                assert_eq!(
                    after.header("etag"),
                    etag,
                    "case {case}: expected the refused delete to leave the version at {etag}, \
                     got {:?}",
                    after.header("etag")
                );
                target.assert_matches(
                    after
                        .root()
                        .child("OriginAccessControlConfig")
                        .unwrap_or_else(|| panic!("case {case}: no OriginAccessControlConfig")),
                    "GetOriginAccessControl after a refused delete",
                    &after.raw,
                );
            }
        }

        // The control the row did not target is untouched either way.
        let untouched = get_control(&router, &other_id).await;
        assert_eq!(
            untouched.status, 200,
            "case {case}: expected the other control to stay readable, got {} with {}",
            untouched.status, untouched.raw
        );
        assert_eq!(
            untouched.header("etag"),
            other_etag,
            "case {case}: expected the other control to stay at version {other_etag}, got {:?}",
            untouched.header("etag")
        );
    }
}

/// An id no control holds is `NoSuchOriginAccessControl`, whatever the request says about
/// the version and whatever the id is written in — and nothing the account holds is touched.
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
        for (version_case, if_match) in [("no-if-match", None), ("current-of-another", Some(&etag))]
        {
            let answer = delete_control(&router, missing, if_match.map(String::as_str)).await;
            assert_eq!(
                answer.status, 404,
                "case {case}/{version_case}: expected HTTP 404 for an id no control holds, \
                 got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchOriginAccessControl",
                "case {case}/{version_case}: expected NoSuchOriginAccessControl, got {:?} with {}",
                answer.error_code(),
                answer.raw
            );
            // The refusal states which of the two conditions it is — the store's absence or an
            // identifier the route cannot read — and carries nothing out of the request.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case}/{version_case}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(missing),
                "case {case}/{version_case}: expected nothing from the request in the message, \
                 got {message:?}"
            );
        }
    }

    let listed = list_controls(&router, "").await;
    assert_eq!(
        page_ids(&listed),
        vec![id.clone()],
        "case missing-ids: expected the stored control to be untouched, got {}",
        listed.raw
    );
}

// ---------------------------------------------------------------------------
// A control an origin still names
// ---------------------------------------------------------------------------

/// The `DistributionConfig` a client sends for a distribution whose origins are `origins`
/// — each `(Id, OriginAccessControlId)`, with `None` for an origin that names no control.
///
/// Built here rather than taken from the distribution harness's builder because the
/// member under test (`Origin$OriginAccessControlId`) is not one that builder carries.
/// Members follow the model's declaration order for `Origin`.
fn distribution_xml(reference: &str, origins: &[(&str, Option<&str>)]) -> String {
    let mut out = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>{}</Quantity><Items>",
        origins.len()
    );
    for (origin_id, control) in origins {
        // An Amazon API Gateway origin at its own `execute-api` endpoint: the origin an
        // `execute-api` origin access control is for, which is the type the control under
        // test carries ([`table`]'s first row).
        out.push_str(&format!(
            "<Origin><Id>{origin_id}</Id>\
             <DomainName>{origin_id}.execute-api.us-east-1.amazonaws.com</DomainName>\
             <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
             <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>"
        ));
        if let Some(control) = control {
            out.push_str(&format!(
                "<OriginAccessControlId>{control}</OriginAccessControlId>"
            ));
        }
        out.push_str("</Origin>");
    }
    out.push_str(&format!(
        "</Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>{}</TargetOriginId>\
         <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>a distribution naming an origin access control</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled></DistributionConfig>",
        origins.first().map(|(id, _)| *id).unwrap_or_default()
    ));
    out
}

/// Where in a distribution's origin list the reference to the control under test sits.
struct Attachment {
    case: &'static str,
    /// The origins the distribution is created with, as offsets into its origin ids, with
    /// `true` for the ones that name the control under test.
    attached: Vec<bool>,
}

/// "You cannot delete an origin access control if it's in use. First, update all
/// distributions to remove the origin access control from all origins, then delete the
/// origin access control."
#[tokio::test]
async fn a_control_an_origin_still_names_cannot_be_deleted() {
    let attachments = vec![
        Attachment {
            case: "the only origin",
            attached: vec![true],
        },
        Attachment {
            case: "the first of three origins",
            attached: vec![true, false, false],
        },
        Attachment {
            case: "the last of three origins",
            attached: vec![false, false, true],
        },
        Attachment {
            case: "two of three origins",
            attached: vec![true, false, true],
        },
    ];

    let rows = table();
    for attachment in &attachments {
        let case = attachment.case;
        let router = emulator();
        // Two controls: the one the distribution names, and one it does not — which stays
        // deletable throughout, so the refusal is about the reference and not about there
        // being a distribution at all.
        let (id, etag) = create(&router, &rows[0]).await;
        let (spare_id, spare_etag) = create(&router, &rows[1]).await;

        let origin_ids: Vec<String> = (0..attachment.attached.len())
            .map(|index| format!("origin-{index}"))
            .collect();
        let attached: Vec<(&str, Option<&str>)> = origin_ids
            .iter()
            .zip(&attachment.attached)
            .map(|(origin_id, holds)| {
                (
                    origin_id.as_str(),
                    if *holds { Some(id.as_str()) } else { None },
                )
            })
            .collect();
        let reference = format!("ref-{}", case.replace(' ', "-"));
        let created =
            create_distribution(&router, &distribution_xml(&reference, &attached)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
            created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        let distribution_etag = created.header("etag").to_string();

        let refused = delete_control(&router, &id, Some(&etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 for a control an origin still names, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "OriginAccessControlInUse",
            "case {case}: expected OriginAccessControlInUse, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        // The refusal states the condition and names no distribution: which distribution
        // holds the reference is no part of the sentence CloudFront serves.
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, "The CloudFront origin access control is still being used.",
            "case {case}: expected the sentence CloudFront answers this code with, got \
             {message:?}"
        );
        assert!(
            !message.contains(&distribution_id),
            "case {case}: expected the refusal to name no distribution, got {message:?}"
        );
        // And it destroys nothing: the control is still readable at the same version.
        let after = get_control(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {case}: expected the refused delete to leave the control readable, got {} \
             with {}",
            after.status, after.raw
        );
        assert_eq!(
            after.header("etag"),
            etag,
            "case {case}: expected the refused delete to leave the version at {etag}, got {:?}",
            after.header("etag")
        );

        // A control NO origin names is deletable while that same distribution stands.
        let spare = delete_control(&router, &spare_id, Some(&spare_etag)).await;
        assert_eq!(
            spare.status, 204,
            "case {case}: expected the unreferenced control to be deletable, got {} with {}",
            spare.status, spare.raw
        );

        // The documented remedy: remove the control from every origin, then delete it.
        let detached: Vec<(&str, Option<&str>)> = origin_ids
            .iter()
            .map(|origin_id| (origin_id.as_str(), None))
            .collect();
        let updated = update_distribution(
            &router,
            &distribution_id,
            &distribution_xml(&reference, &detached),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            updated.status, updated.raw
        );

        let deleted = delete_control(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected the delete to land once no origin names the control, \
             got {} with {}",
            deleted.status, deleted.raw
        );
        let gone = get_control(&router, &id).await;
        assert_eq!(
            gone.status, 404,
            "case {case}: expected the control to be gone after the delete, got {} with {}",
            gone.status, gone.raw
        );
    }
}
