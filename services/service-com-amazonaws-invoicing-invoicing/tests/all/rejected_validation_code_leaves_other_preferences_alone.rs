// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED submission of a procurement-portal validation code leaves behind for
//! the OTHER preferences in the registry, driven end to end over this service's wire
//! protocol.
//!
//! The rule under test is the one `refused_procurement_portal_writes.rs` states and whose
//! harness this suite reuses rather than copying: what a call WRITES is what a reader
//! reports, and a refused submission of a validation code writes the attempt it reports
//! against the preference it was submitted FOR — and nothing else
//! (`smithy_query(query="shape", shape="ProcurementPortalPreference")`: `LastUpdateDate`
//! is "the date and time when the procurement portal preference was last updated"). This
//! suite takes it one resource further out: a refused submission against ONE preference
//! leaves every other preference in the registry — at its own version, with a validation
//! code of its own outstanding — exactly where its own last write left it, including when
//! the code submitted is that other preference's own.
//!
//! The rows are the forms a submitted code can be wrong in; the shared helpers (the
//! router, the wire call, the create, the reader, and the code issue the portal flow needs)
//! come from `refused_procurement_portal_writes`, so there is one harness for this plane.

use serde_json::json;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, call, create_with_delivery_endpoint, emulator_with_engine,
    get_preference, issue_code, ok, outstanding_code, portal_at, shifted,
};

/// RULE: a refused submission of a validation code writes only the preference it was
/// submitted for, so no OTHER preference in the registry moves — not its reported members
/// and not the code outstanding for it.
///
/// The rows are the forms a wrong code takes: a digit-shifted copy of the code issued for
/// the subject, a longer code, a non-digit code, a one-character code, and the code
/// issued for the bystander preference (wrong for the subject, and no licence to touch
/// the bystander either).
#[tokio::test]
async fn a_rejected_validation_code_leaves_the_other_preferences_alone() {
    let (router, engine) = emulator_with_engine();

    let subject = arn_of(
        &ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_with_delivery_endpoint(portal_at(0), "BUYER-SUBJECT", "SUPPLIER-SUBJECT"),
        )
        .await,
    );
    let bystander = arn_of(
        &ok(
            &router,
            "CreateProcurementPortalPreference",
            &create_with_delivery_endpoint(
                portal_at(1),
                "BUYER-BYSTANDER",
                "SUPPLIER-BYSTANDER",
            ),
        )
        .await,
    );
    // One successful write on the bystander, so the version it has to keep is not the one
    // a create happens to leave.
    ok(
        &router,
        "PutProcurementPortalPreference",
        &accepted_update_round(&bystander, 1),
    )
    .await;

    let issued = issue_code(&router, &engine, &subject).await;
    let bystanders_code = issue_code(&router, &engine, &bystander).await;
    let before = get_preference(&router, &bystander).await;

    let wrong_codes = [
        shifted(&issued),
        "12345678".to_string(),
        "not-a-code".to_string(),
        "7".to_string(),
        bystanders_code.clone(),
    ];
    for wrong in &wrong_codes {
        assert_ne!(
            wrong, &issued,
            "case {wrong:?}: the row's code has to be the wrong one for the subject"
        );
        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": subject, "Code": wrong}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {wrong:?}: expected HTTP 400, got {status} {body}"
        );
        let after = get_preference(&router, &bystander).await;
        assert_eq!(
            after, before,
            "case {wrong:?}: another preference's refused submission must not touch this \
             one, got {after}"
        );
        assert_eq!(
            outstanding_code(&engine, &bystander).as_deref(),
            Some(bystanders_code.as_str()),
            "case {wrong:?}: another preference's refused submission must not retire this \
             one's code"
        );
    }
}
