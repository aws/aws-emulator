// SPDX-License-Identifier: CC0-1.0

//! The ISSUING half of the procurement-portal validation flow:
//! `SendProcurementPortalValidation` "initiates the validation process by issuing a
//! validation code that confirms ownership and connectivity of the configured procurement
//! portal endpoint" (`smithy_query(query="doc",
//! shape="SendProcurementPortalValidation")`), so issuing that code is the work the call
//! performs, and `VerifyProcurementPortalValidation` completes the flow on the code the
//! send issued.
//!
//! Every call goes through the same `MultiServiceRouter` the production router uses, with
//! the shared harness of `refused_procurement_portal_writes` rather than a copy of it. The
//! one thing read out of state is the issued CODE: the modelled response carries only the
//! ARN, because the real service delivers the code through the portal the preference
//! configures.
//!
//! The rules under test, each spanned by a table rather than one request:
//!
//! * a send against a preference that exists issues a code, in the six-decimal-digit form
//!   the flow's `Code` is quoted in, which the verify then accepts — over the portals the
//!   enum admits, at several configuration-version counts, and however many sends preceded
//!   it (a later send retires the code an earlier one issued);
//! * every send issues a code of its own whatever `ClientToken` it carries: a refused call
//!   records no answer for a later one to replay, and this emulator's send reports the
//!   delivery of the test e-invoice it could not make;
//! * a completed validation can be started again: the next send issues a code of its own
//!   and the consumed one completes nothing;
//! * issuing a code writes no CONFIGURATION, so no configuration member a reader reports
//!   moves — what the write does advance is the `Version` and `LastUpdateDate` every
//!   applied write of the resource advances — and neither does completing the validation,
//!   whose only reported effect besides those two is the verification status of the planes
//!   it validated (the rule `completed_validation_moves_the_plane_status` spans).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::state::ComAmazonawsInvoicingInvoicingState;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, call, create_with_delivery_endpoint, emulator_with_engine,
    error_code, error_text, get_preference, minimal_create, ok, outstanding_code, portal_at,
    undeliverable_sentence, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// How many decimal digits an issued validation code has: the form
/// `ComAmazonawsInvoicingInvoicingState::issue_portal_validation_code` mints and the
/// portal-validation flow's `Code` is quoted in.
const CODE_DIGITS: usize = 6;

/// The status a plane reports once its portal connection has been verified: one of the
/// values `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const VALIDATED: &str = "VALIDATED";

/// Whether the validation of `arn` has been completed, per state — no modelled member
/// reports it.
fn is_verified(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> bool {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .get(arn)
        .and_then(|stored| stored.validation.as_ref())
        .is_some_and(|validation| validation.verified_at.is_some())
}

/// How many preferences state holds, so a refused send is shown to create none.
fn stored_preferences(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsInvoicingInvoicingState>()
        .lock()
        .procurement_portal_preferences
        .len()
}

/// What a reader reports for a preference APART from the members a status write moves:
/// each plane's verification status and status reason, and the `Version` and
/// `LastUpdateDate` every applied write of the resource advances. What is left is the
/// preference's configuration, which a call of the validation flow never writes.
fn configuration(preference: &Value) -> Value {
    let mut configuration = preference.clone();
    if let Some(members) = configuration.as_object_mut() {
        for moved_by_a_status_write in [
            "EinvoiceDeliveryPreferenceStatus",
            "EinvoiceDeliveryPreferenceStatusReason",
            "PurchaseOrderRetrievalPreferenceStatus",
            "PurchaseOrderRetrievalPreferenceStatusReason",
            "Version",
            "LastUpdateDate",
        ] {
            members.remove(moved_by_a_status_write);
        }
    }
    configuration
}

/// The `Version` a reader reports for a preference.
fn version(preference: &Value) -> i64 {
    preference
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a Version in {preference}"))
}

/// One send, asserting the modelled answer: the refusal that reports the delivery of the
/// test e-invoice this emulator could not make — HTTP 400 `ValidationException`, naming the
/// delivery target when the preference configures the endpoint the document goes to. The
/// code the call ISSUED is outstanding either way, which is the work under test here.
async fn send(router: &MultiServiceRouter, case: &str, request: &Value) {
    let (status, body) = call(router, "SendProcurementPortalValidation", request).await;
    assert_eq!(
        status, 400,
        "case {case}: the send reports the delivery it could not make, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        error_text(&body),
        undeliverable_sentence(PORTAL_INSTANCE_ENDPOINT),
        "case {case}: the refusal names the delivery target it could not reach, got {body}"
    );
}

/// One row: a preference reached through a chosen number of successful configuration
/// writes, on a chosen portal, with a chosen number of sends before the code is submitted.
struct Row {
    label: &'static str,
    portal: &'static str,
    /// Successful configuration writes after the create, so the version the sends must
    /// leave alone is not the one a create happens to leave.
    writes_after_create: i64,
    /// How many times the validation is started before the outstanding code is submitted:
    /// a later send issues again and retires what the previous one issued.
    sends: usize,
}

/// The rows: every portal of the enum, at several version counts, with one send and with
/// several.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one send, straight after the create",
            portal: portal_at(0),
            writes_after_create: 0,
            sends: 1,
        },
        Row {
            label: "one send, after one configuration update",
            portal: portal_at(1),
            writes_after_create: 1,
            sends: 1,
        },
        Row {
            label: "two sends, after one configuration update",
            portal: portal_at(2),
            writes_after_create: 1,
            sends: 2,
        },
        Row {
            label: "three sends, after two configuration updates",
            portal: portal_at(3),
            writes_after_create: 2,
            sends: 3,
        },
        Row {
            label: "two sends, straight after the create",
            portal: portal_at(0),
            writes_after_create: 0,
            sends: 2,
        },
    ]
}

/// RULE: the call that starts a validation issues the validation code the flow runs on and
/// records it against the named preference as the outstanding one; the submission of that
/// code is what completes the validation, and a code an earlier start issued is retired by
/// the code the next one issues. Recording a code writes no CONFIGURATION, so nothing a
/// reader reports of it moves — only the `Version` and `LastUpdateDate` every applied write
/// of the resource advances.
#[tokio::test]
async fn every_send_issues_a_code_the_verify_then_accepts() {
    let (router, engine) = emulator_with_engine();
    // Every code this test sees, so the rows are shown to get codes of their own rather
    // than one constant.
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (index, row) in rows().iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    row.portal,
                    &format!("BUYER-ISSUE-{index}"),
                    &format!("SUPPLIER-ISSUE-{index}"),
                ),
            )
            .await,
        );
        for round in 0..row.writes_after_create {
            ok(
                &router,
                "PutProcurementPortalPreference",
                &accepted_update_round(&arn, round),
            )
            .await;
        }
        let before = get_preference(&router, &arn).await;
        let expected_version = 1 + row.writes_after_create;
        assert_eq!(
            version(&before),
            expected_version,
            "case {}: the create and {} configuration writes leave Version \
             {expected_version}, got {before}",
            row.label,
            row.writes_after_create
        );
        // Every write that follows, counted as it is made.
        let mut writes = expected_version;

        // Nothing is outstanding before the first send, so no code could be submitted.
        assert_eq!(
            outstanding_code(&engine, &arn),
            None,
            "case {}: no code is outstanding before a send",
            row.label
        );

        let mut retired: Vec<String> = Vec::new();
        let mut outstanding = String::new();
        for attempt in 1..=row.sends {
            let case = format!("{} / send {attempt}", row.label);
            send(
                &router,
                &case,
                &json!({"ProcurementPortalPreferenceArn": arn}),
            )
            .await;
            writes += 1;
            let issued = outstanding_code(&engine, &arn)
                .unwrap_or_else(|| panic!("case {case}: the send must issue a code"));
            assert_eq!(
                issued.len(),
                CODE_DIGITS,
                "case {case}: an issued code is {CODE_DIGITS} decimal digits, got {issued:?}"
            );
            assert!(
                issued.chars().all(|digit| digit.is_ascii_digit()),
                "case {case}: an issued code is {CODE_DIGITS} decimal digits, got {issued:?}"
            );
            assert!(
                seen.insert(issued.clone()),
                "case {case}: each send issues a code of its own, got the repeated {issued:?}"
            );
            if !outstanding.is_empty() {
                retired.push(std::mem::take(&mut outstanding));
            }
            outstanding = issued;
        }

        // Issuing codes wrote no configuration: what the sends advanced is the version
        // every applied write of this resource advances, and nothing else a reader reports.
        let after_sends = get_preference(&router, &arn).await;
        assert_eq!(
            configuration(&after_sends),
            configuration(&before),
            "case {}: issuing a code must leave every configuration member as it was, got \
             {after_sends}",
            row.label
        );
        assert_eq!(
            version(&after_sends),
            writes,
            "case {}: each issued code is one version of the resource, so Version is \
             {writes}, got {after_sends}",
            row.label
        );

        // A code an earlier send issued is no longer the outstanding one, so it completes
        // nothing.
        for stale in &retired {
            let (status, body) = call(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": stale}),
            )
            .await;
            writes += 1;
            assert_eq!(
                status, 400,
                "case {} / a retired code: expected HTTP 400, got {status} {body}",
                row.label
            );
            assert!(
                !is_verified(&engine, &arn),
                "case {} / a retired code: a refused submission must not complete the \
                 validation",
                row.label
            );
        }

        // The code the latest send issued is what completes the validation.
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": outstanding}),
        )
        .await;
        writes += 1;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {}: the verify answers with the ARN it validated, got {completed}",
            row.label
        );
        assert!(
            is_verified(&engine, &arn),
            "case {}: submitting the issued code completes the validation",
            row.label
        );
        let after_verify = get_preference(&router, &arn).await;
        assert_eq!(
            configuration(&after_verify),
            configuration(&before),
            "case {}: completing a validation writes no configuration either, got \
             {after_verify}",
            row.label
        );
        // What it DOES write is the verification state: each plane that was waiting for
        // the portal connection to be verified now reports it validated, and that status
        // write is one version of the resource.
        for (member, was) in [
            (
                "EinvoiceDeliveryPreferenceStatus",
                before.get("EinvoiceDeliveryPreferenceStatus"),
            ),
            (
                "PurchaseOrderRetrievalPreferenceStatus",
                before.get("PurchaseOrderRetrievalPreferenceStatus"),
            ),
        ] {
            let actual = after_verify.get(member).and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(VALIDATED),
                "case {}: the {member} that was {was:?} reports {VALIDATED} once the \
                 validation completed, got {actual:?}",
                row.label
            );
        }
        assert_eq!(
            version(&after_verify),
            writes,
            "case {}: the completed validation is one more version of the resource, so \
             Version is {writes}, got {after_verify}",
            row.label
        );
    }
}

/// RULE: a `ClientToken` records an ANSWER for a later call carrying it to replay, and a
/// call that was refused has none — so every send starts the validation again: it issues a
/// code of its own, which retires the code the previous send issued, whatever token it
/// carries and whether that token was used before.
///
/// The rows differ along the token's shape, inside `BasicStringWithoutSpace`'s
/// `@length(0..1024)` and `^\S+$` (`smithy_query(query="operation",
/// operation="SendProcurementPortalValidation")`), and along whether a send states one at
/// all.
#[tokio::test]
async fn every_send_issues_a_fresh_code_whatever_token_it_carries() {
    let (router, engine) = emulator_with_engine();

    let longest = "t".repeat(1024);
    for (index, token) in ["t", "a-client-token", "9f2b-4c1d", longest.as_str()]
        .into_iter()
        .enumerate()
    {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    PORTAL_NAMES[index % PORTAL_NAMES.len()],
                    &format!("BUYER-REPLAY-{index}"),
                    &format!("SUPPLIER-REPLAY-{index}"),
                ),
            )
            .await,
        );
        // The sends of this row: the token repeated, then one stating no token and one
        // stating another, so neither a repeat nor a fresh token is what decides.
        let requests = [
            ("the token stated", json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": token})),
            ("the token repeated", json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": token})),
            ("the token repeated again", json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": token})),
            ("no token at all", json!({"ProcurementPortalPreferenceArn": arn})),
            (
                "another token",
                json!({"ProcurementPortalPreferenceArn": arn, "ClientToken": format!("another-token-{index}")}),
            ),
        ];

        let mut previous: Option<String> = None;
        let mut writes = version(&get_preference(&router, &arn).await);
        for (shape, request) in requests {
            let case = format!("token {token:?}, {shape}");
            send(&router, &case, &request).await;
            writes += 1;
            let issued = outstanding_code(&engine, &arn)
                .unwrap_or_else(|| panic!("case {case}: the send must issue a code"));
            if let Some(previous) = previous.as_deref() {
                assert_ne!(
                    issued, previous,
                    "case {case}: a send issues a code of its own, got the previous \
                     {previous:?}"
                );

                // The code the previous send issued is retired by this one.
                let (status, body) = call(
                    &router,
                    "VerifyProcurementPortalValidation",
                    &json!({"ProcurementPortalPreferenceArn": arn, "Code": previous}),
                )
                .await;
                writes += 1;
                assert_eq!(
                    status, 400,
                    "case {case}: the code a later send replaced completes nothing, got \
                     {status} {body}"
                );
            }
            // Each issued code is one write of the resource, and so is each refused
            // submission of a retired one.
            let reported = get_preference(&router, &arn).await;
            assert_eq!(
                version(&reported),
                writes,
                "case {case}: expected Version {writes} after {writes} applied writes, got \
                 {reported}"
            );
            previous = Some(issued);
        }
    }
}

/// RULE: a validation that has been completed can be started again — the start issues a
/// code of its own, and the code a completed validation consumed completes nothing a
/// second time.
///
/// The rows are the portals the enum admits, so the re-start is not keyed to one portal.
#[tokio::test]
async fn a_completed_validation_can_be_started_again() {
    let (router, engine) = emulator_with_engine();

    for (index, portal) in PORTAL_NAMES.iter().enumerate() {
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    portal,
                    &format!("BUYER-REDO-{index}"),
                    &format!("SUPPLIER-REDO-{index}"),
                ),
            )
            .await,
        );

        send(
            &router,
            &format!("{portal} first send"),
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        let first = outstanding_code(&engine, &arn)
            .unwrap_or_else(|| panic!("case {portal}: the send must issue a code"));
        ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": first}),
        )
        .await;
        assert_eq!(
            outstanding_code(&engine, &arn),
            None,
            "case {portal}: the completed code is no longer outstanding"
        );

        send(
            &router,
            &format!("{portal} second send"),
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        let second = outstanding_code(&engine, &arn)
            .unwrap_or_else(|| panic!("case {portal}: a send after a completed validation issues"));
        assert_ne!(
            second, first,
            "case {portal}: the second send issues a code of its own, got the consumed {first:?}"
        );

        let (status, body) = call(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": first}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {portal}: a consumed code completes nothing a second time, got {status} {body}"
        );
        ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": second}),
        )
        .await;
        assert!(
            is_verified(&engine, &arn),
            "case {portal}: the code the second send issued completes the validation"
        );
    }
}

/// RULE: an operation that names a resource by ARN resolves it against state, so an ARN
/// naming no preference is the modelled `ResourceNotFoundException` (HTTP 404) and no
/// state — no validation, no preference — is created for it.
///
/// The rows are well-formed preference ARNs differing in the id's characters and length,
/// alongside one preference that does exist, so a 404 is on the row's own account.
#[tokio::test]
async fn a_send_for_an_arn_that_names_no_preference_issues_nothing() {
    let (router, engine) = emulator_with_engine();
    let held = arn_of(
        &ok(
            &router,
            "CreateProcurementPortalPreference",
            &minimal_create(portal_at(0), "BUYER-HELD", "SUPPLIER-HELD"),
        )
        .await,
    );
    let held_before = stored_preferences(&engine);

    for id in [
        "a",
        "does-not-exist",
        "0123456789ab",
        "A-Very-Long-Preference-Id-0123456789",
    ] {
        let arn = engine::emulator_arn(
            "invoicing",
            "",
            &format!("procurement-portal-preference/{id}"),
        );
        let (status, body) = call(
            &router,
            "SendProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {arn}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "ResourceNotFoundException",
            "case {arn}: expected the modelled not-found error, got {body}"
        );
        assert_eq!(
            stored_preferences(&engine),
            held_before,
            "case {arn}: a refused send must create no preference"
        );
        assert_eq!(
            outstanding_code(&engine, &held),
            None,
            "case {arn}: a refused send must issue no code for another preference"
        );
    }
}
