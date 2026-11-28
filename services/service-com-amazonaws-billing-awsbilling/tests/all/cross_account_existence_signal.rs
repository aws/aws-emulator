// SPDX-License-Identifier: CC0-1.0

//! The account boundary of this service holds WHATEVER THE STORE HOLDS, end to
//! end over the awsJson1_0 wire.
//!
//! One rule: an operation that identifies a resource of this service by ARN
//! decides the caller-account boundary BEFORE it consults the store, so a
//! well-formed ARN whose embedded account id is not the caller's is
//! `AccessDeniedException` (403) and the answer carries nothing a record stored
//! under that ARN would have contributed — the existence of a resource in another
//! account is not observable through this API.
//!
//! This is the half of the rule a refusal table alone cannot reach: when the
//! store holds nothing under a foreign ARN, "the boundary was decided first" and
//! "the lookup missed and the miss was reported as a denial" are
//! indistinguishable. Each test here therefore drives TWO emulators over the same
//! request — one whose store HOLDS a record under the foreign ARN, one whose
//! store does not — and pins that every ARN-addressed operation answers them
//! identically, status, error code and message alike.
//!
//! The control in the other direction is pinned too: within the caller's OWN
//! account the answer DOES come from the store (a stored view is reported, an ARN
//! no view is stored under is `ResourceNotFoundException`), which is what makes
//! the comparison above meaningful rather than two blanket refusals.
//!
//! The operation tables are the ones `arn_account_boundary.rs` owns; the harness
//! (`call`, `emulator_with_engine`, …) the one `billing_views.rs` owns.

use std::collections::BTreeMap;
use std::sync::Arc;

use service_com_amazonaws_billing_awsbilling::state::BILLING_VIEW_TYPE_CUSTOM;
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use service_com_amazonaws_ram_amazonresourcesharing::state as resource_sharing;

use crate::arn_account_boundary::{
    BodyOf, BILLING_VIEW_ARN_OPERATIONS, FOREIGN_BILLING_VIEW_ARNS, FOREIGN_RESOURCE_TYPE_ARNS,
    RESOURCE_ARN_OPERATIONS,
};
use crate::billing_views::{
    call, emulator_with_engine, error_code, plant_view, ACCOUNT_ID, PRIMARY_ARN,
};
use crate::rejection_fidelity::message;

/// A marker string planted in every field of a planted record that an answer
/// could possibly carry (its name, description, tag and source view), so an
/// answer built from the store is recognisable as one.
const PLANTED_MARKER: &str = "planted-record-content";

/// The principal, in another account, the planted shares share a view with: an
/// identity outside the owner's organization, which is the one a resource-based
/// policy has to grant.
const FOREIGN_PRINCIPAL: &str = "arn:aws:iam::444455556666:root";

/// One row of the foreign-ARN table: the case, the ARN, and whether the ARN is in
/// the `billingview/` form the `BillingViewArn` operations take (the tagging and
/// policy plane's `ResourceArn` admits any resource type segment, so those rows
/// reach only that plane).
type ForeignRow = (&'static str, String, bool);

/// Plant one billing-view record in the store under `arn`, drawing from
/// `source_views` and carrying [`PLANTED_MARKER`] in every field an answer could
/// report.
///
/// The store is written to through the harness's planting helper because the API
/// mints an ARN in the caller's own account only, so a record under a FOREIGN ARN
/// — the one thing that tells an account-boundary refusal from a reported lookup
/// miss — is not reachable through `CreateBillingView`.
fn plant_record(engine: &Arc<engine::InMemoryEngine>, arn: &str, source_views: Vec<String>) {
    plant_view(engine, arn, |view| {
        view.name = format!("{PLANTED_MARKER}-name");
        view.description = Some(format!("{PLANTED_MARKER}-description"));
        view.billing_view_type = BILLING_VIEW_TYPE_CUSTOM.to_string();
        // The owner of a view is the account its ARN embeds (the fifth
        // colon-separated segment of an ARN).
        view.owner_account_id = arn.split(':').nth(4).unwrap_or_default().to_string();
        view.source_views = source_views;
        view.tags = BTreeMap::from([(
            format!("{PLANTED_MARKER}-key"),
            Some(format!("{PLANTED_MARKER}-value")),
        )]);
    });
}

/// Share the billing view `arn` the way the account shares one — a live resource
/// share of this account, holding that view and an identity outside the owner's
/// organization — so a resource-based policy IS attached to the ARN.
///
/// That document is the only thing `GetResourcePolicy` could report for the ARN,
/// so a store that holds one is what makes "the boundary was decided first"
/// distinguishable from "the policy lookup missed".
fn plant_share(engine: &Arc<engine::InMemoryEngine>, arn: &str) {
    let shares = resource_sharing::service_state(&**engine)
        .expect("the resource sharing state is reachable from the shared engine");
    let mut shares = shares.lock();
    let now = shares.tick();
    let share = resource_sharing::ShareRecord {
        arn: shares.fresh_share_arn("us-east-1"),
        name: format!("{PLANTED_MARKER}-share"),
        owning_account_id: ACCOUNT_ID.to_string(),
        allow_external_principals: true,
        status: resource_sharing::SHARE_STATUS_ACTIVE.to_string(),
        creation_time: now,
        last_updated_time: now,
        ..Default::default()
    };
    shares.insert_share(share.clone());
    shares.associate(&share, resource_sharing::ASSOCIATION_TYPE_RESOURCE, arn, now);
    shares.associate(
        &share,
        resource_sharing::ASSOCIATION_TYPE_PRINCIPAL,
        FOREIGN_PRINCIPAL,
        now,
    );
}

/// The ARNs outside the caller's account the rule is asserted over: the two
/// tables `arn_account_boundary.rs` owns, which vary the foreign account id, the
/// partition, the resource id — including `primary`, which names a view that DOES
/// exist in the caller's own account — its depth, and the resource type segment.
/// Shared rather than restated, so the suite that asserts these ARNs against an
/// EMPTY store and this one, which holds a record under each, cannot drift apart.
fn foreign_rows() -> Vec<ForeignRow> {
    FOREIGN_BILLING_VIEW_ARNS
        .iter()
        .map(|arn| ("a billing-view ARN of another account", (*arn).to_string(), true))
        .chain(
            FOREIGN_RESOURCE_TYPE_ARNS
                .iter()
                .map(|arn| ("another resource type segment", (*arn).to_string(), false)),
        )
        .collect()
}

/// Every operation that resolves `arn`, for one row of the table above.
fn operations_for(row: &ForeignRow) -> Vec<(&'static str, BodyOf)> {
    if row.2 {
        BILLING_VIEW_ARN_OPERATIONS
            .into_iter()
            .chain(RESOURCE_ARN_OPERATIONS)
            .collect()
    } else {
        RESOURCE_ARN_OPERATIONS.to_vec()
    }
}

#[tokio::test]
async fn a_foreign_account_arn_answers_the_same_whether_or_not_the_store_holds_it() {
    let (without_record, _bare_engine) = emulator_with_engine();
    let (with_record, seeded_engine) = emulator_with_engine();

    let rows = foreign_rows();
    for (_, arn, _) in &rows {
        plant_record(
            &seeded_engine,
            arn,
            vec![format!(
                "arn:aws:billing::111122223333:billingview/{PLANTED_MARKER}-source"
            )],
        );
        plant_share(&seeded_engine, arn);
    }

    for row in &rows {
        let (case, arn, _) = row;
        for (operation, body_of) in operations_for(row) {
            let label = format!("{case}: {operation} on {arn}");
            let (bare_status, bare_body) = call(&without_record, operation, &body_of(arn)).await;
            let (seeded_status, seeded_body) = call(&with_record, operation, &body_of(arn)).await;

            assert_eq!(
                (seeded_status, error_code(&seeded_body).as_str()),
                (403, "AccessDeniedException"),
                "case {label:?}: a foreign-account ARN the store HOLDS is still refused with \
                 403 AccessDeniedException, got {seeded_status} {seeded_body:?}"
            );
            assert_eq!(
                (bare_status, error_code(&bare_body), message(&bare_body)),
                (
                    seeded_status,
                    error_code(&seeded_body),
                    message(&seeded_body)
                ),
                "case {label:?}: the answer must not depend on what is stored under the ARN, \
                 got {bare_status} {bare_body:?} with nothing stored and {seeded_status} \
                 {seeded_body:?} with a record stored"
            );
            let rendered =
                serde_json::to_string(&seeded_body).expect("the response body serializes");
            assert!(
                !rendered.contains(PLANTED_MARKER),
                "case {label:?}: no part of the stored record may reach the answer, got \
                 {rendered:?}"
            );
        }
    }

    // A refused cross-account call is not a partial one: every planted record is
    // exactly as planted, and nothing was added or removed.
    let state = seeded_engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    assert_eq!(
        state.billing_views.len(),
        rows.len() + 1,
        "case \"after the refusals\": the planted records and the account's own PRIMARY view \
         are what the store holds, got {:?}",
        state.billing_views.keys().collect::<Vec<_>>()
    );
    for (case, arn, _) in &rows {
        let stored = state
            .billing_views
            .get(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the planted record under {arn} is gone"));
        assert_eq!(
            (
                stored.name.as_str(),
                stored.source_views.len(),
                stored.tags.len(),
            ),
            (format!("{PLANTED_MARKER}-name").as_str(), 1, 1),
            "case {case:?}: a refused cross-account call changes nothing it refused, got \
             {stored:?}"
        );
    }
    drop(state);

    // The policy attached to each of those ARNs is still attached: a refused
    // cross-account call neither reported it nor took it away.
    let shares = resource_sharing::service_state(&*seeded_engine)
        .expect("the resource sharing state is reachable from the shared engine");
    let shares = shares.lock();
    for (case, arn, _) in &rows {
        assert!(
            shares.resource_policies(arn, None).is_some_and(|documents| {
                documents.iter().any(|document| document.contains(arn))
            }),
            "case {case:?}: the planted share of {arn} must still attach a policy to it, got {:?}",
            shares.resource_policies(arn, None)
        );
    }
}

#[tokio::test]
async fn within_the_callers_own_account_the_answer_comes_from_the_store() {
    let (without_record, _bare_engine) = emulator_with_engine();
    let (with_record, seeded_engine) = emulator_with_engine();

    // The rows differ in the one thing the answer is built from: how many source
    // views the stored record draws from — none (what the account's own PRIMARY
    // view holds), one, and several.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "a view with no source views",
            format!("arn:aws:billing::{ACCOUNT_ID}:billingview/own-no-sources"),
            vec![],
        ),
        (
            "a view with one source view",
            format!("arn:aws:billing::{ACCOUNT_ID}:billingview/own-one-source"),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "a view with three source views",
            format!("arn:aws:billing::{ACCOUNT_ID}:billingview/own-three-sources"),
            (0..3)
                .map(|index| format!("arn:aws:billing::{ACCOUNT_ID}:billingview/source-{index}"))
                .collect(),
        ),
    ];
    for (_, arn, sources) in &rows {
        plant_record(&seeded_engine, arn, sources.clone());
    }

    for (case, arn, sources) in &rows {
        // The store holds the record: the read reports the list it holds.
        let (status, body) = call(
            &with_record,
            "ListSourceViewsForBillingView",
            &serde_json::json!({ "arn": arn }),
        )
        .await;
        let listed: Vec<String> = body["sourceViews"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            (status, listed.as_slice()),
            (200, sources.as_slice()),
            "case {case:?}: expected the stored source views, got {status} {body:?}"
        );

        // The same ARN with nothing stored under it: the miss is reported as the
        // modeled 404, never as the account-boundary refusal.
        let (status, body) = call(
            &without_record,
            "ListSourceViewsForBillingView",
            &serde_json::json!({ "arn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an ARN of the caller's own account that names no view is a 404, \
             got {status} {body:?}"
        );
        // This service's own ARN-validation form, shared with the `arns` filter
        // of `ListBillingViews`.
        let expected = format!("[{arn}] is an invalid billing view arn");
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );

        // The sibling read of the same ARN agrees about which account the ARN
        // belongs to: it resolves the stored record and reports the miss as its
        // own modeled 404.
        let (status, body) = call(
            &with_record,
            "GetBillingView",
            &serde_json::json!({ "arn": arn }),
        )
        .await;
        assert_eq!(
            (status, body["billingView"]["arn"].as_str()),
            (200, Some(arn.as_str())),
            "case {case:?}: GetBillingView resolves the stored record, got {status} {body:?}"
        );
        let (status, body) = call(
            &without_record,
            "GetBillingView",
            &serde_json::json!({ "arn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: GetBillingView reports the miss as a 404, got {status} {body:?}"
        );
    }
}
