// SPDX-License-Identifier: CC0-1.0

//! The account boundary of every ARN-addressed operation of this service, end to
//! end over the awsJson1_0 wire.
//!
//! One rule, asserted over the whole surface it covers: an operation that
//! identifies a resource of this service by ARN enforces the caller-account
//! boundary BEFORE it consults the store, so a well-formed ARN whose embedded
//! account id is not the caller's is `AccessDeniedException` (403) whatever that
//! ARN names — and never a `ResourceNotFoundException` or a `ValidationException`,
//! which would leak whether a resource exists in the other account.
//!
//! The tables span the class the rule generalises over: the foreign account id,
//! the partition the ARN names, the resource id (including one that DOES exist in
//! the caller's own account, so the answer cannot be coming from a lookup), the
//! resource type segment, and every implemented operation that resolves an ARN —
//! the lifecycle writers, the source-view plane and the tagging/policy plane
//! alike.
//!
//! The harness (`call`, `emulator_with_engine`, …) is the one `billing_views.rs`
//! owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{
    call, emulator_with_engine, error_code, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN,
};
use crate::rejection_fidelity::message;

/// How one row of the operation table builds its request body from the ARN under
/// test: the member carrying the ARN differs between operations (`arn` for the
/// billing-view surface, `resourceArn` for the tagging and policy surface).
pub(crate) type BodyOf = fn(&str) -> Value;

/// Every implemented operation that identifies a billing view by its own `arn`
/// member, with the rest of its required input. `ListBillingViews` reaches the
/// ARN through its `arns` filter.
pub(crate) const BILLING_VIEW_ARN_OPERATIONS: [(&str, BodyOf); 7] = [
    ("GetBillingView", |arn| json!({ "arn": arn })),
    ("UpdateBillingView", |arn| {
        json!({ "arn": arn, "name": "renamed" })
    }),
    ("DeleteBillingView", |arn| {
        json!({ "arn": arn, "force": true })
    }),
    ("ListBillingViews", |arn| json!({ "arns": [arn] })),
    ("AssociateSourceViews", |arn| {
        json!({ "arn": arn, "sourceViews": [PRIMARY_ARN] })
    }),
    ("DisassociateSourceViews", |arn| {
        json!({ "arn": arn, "sourceViews": [PRIMARY_ARN] })
    }),
    ("ListSourceViewsForBillingView", |arn| json!({ "arn": arn })),
];

/// Every implemented operation that identifies a resource by `resourceArn` — the
/// tagging and policy plane, whose modeled `ResourceArn` admits any resource type
/// segment, not just `billingview/`.
pub(crate) const RESOURCE_ARN_OPERATIONS: [(&str, BodyOf); 4] = [
    ("TagResource", |arn| {
        json!({ "resourceArn": arn, "resourceTags": [{ "key": "team", "value": "finance" }] })
    }),
    ("UntagResource", |arn| {
        json!({ "resourceArn": arn, "resourceTagKeys": ["team"] })
    }),
    ("ListTagsForResource", |arn| json!({ "resourceArn": arn })),
    ("GetResourcePolicy", |arn| json!({ "resourceArn": arn })),
];

/// ARNs outside the caller's account in the `billingview/` form every
/// `BillingViewArn` member takes. They vary the foreign account id, the partition
/// and the resource id — including `primary`, which names a view that DOES exist
/// in the caller's own account, and a nested resource id — so no answer to one of
/// them can be coming from a store lookup. Shared with the suite that asserts the
/// same ARNs against a store that HOLDS a record under each of them
/// (`cross_account_existence_signal.rs`), so the two cannot drift apart.
pub(crate) const FOREIGN_BILLING_VIEW_ARNS: [&str; 5] = [
    "arn:aws:billing::111122223333:billingview/primary",
    "arn:aws:billing::123456789012:billingview/does-not-exist",
    "arn:aws-us-gov:billing::111122223333:billingview/primary",
    "arn:aws-cn:billing::999999999999:billingview/custom-1",
    "arn:aws:billing::111122223333:billingview/a/nested/id",
];

/// ARNs outside the caller's account whose resource type segment is NOT
/// `billingview/`: the boundary is about the account the ARN embeds, not about
/// what the segment names, and only the tagging and policy plane's `resourceArn`
/// admits these.
///
/// Source: the `resourceArn` member of `TagResource`,
/// `smithy_query(query="operation", operation="TagResource")` — pattern
/// `^arn:aws[a-z-]*:(billing)::[0-9]{12}:[a-zA-Z0-9/:_\+=\.\@-]{0,70}[a-zA-Z0-9]$`.
pub(crate) const FOREIGN_RESOURCE_TYPE_ARNS: [&str; 2] = [
    "arn:aws:billing::111122223333:budget/none",
    "arn:aws:billing::111122223333:x",
];

/// Create one `CUSTOM` billing view drawing from the account's `PRIMARY` view and
/// return its ARN.
async fn create_view(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": [PRIMARY_ARN], "clientToken": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the harness create must answer 200, got {status} {body:?}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateBillingView answered no arn: {body:?}"))
        .to_string()
}

/// Assert that `operation` refuses `arn` with the cross-account refusal, and that
/// the refusal is built from the requested ARN and the invoked action rather than
/// from anything the store holds.
async fn assert_cross_account_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body_of: BodyOf,
    arn: &str,
) {
    let case = format!("{operation} on {arn}");
    let (status, body) = call(router, operation, &body_of(arn)).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: expected 403 AccessDeniedException, got {status} {body:?}"
    );
    let stated = message(&body);
    assert!(
        stated.contains(arn),
        "case {case:?}: the refusal names the requested ARN, got {stated:?}"
    );
    assert!(
        stated.contains(&format!("billing:{operation}")),
        "case {case:?}: the refusal names the denied action, got {stated:?}"
    );
}

#[tokio::test]
async fn every_arn_addressed_operation_refuses_a_foreign_account_arn_before_any_lookup() {
    let (router, engine) = emulator_with_engine();
    let own = create_view(&router, "boundary-view").await;
    let own_id = own
        .rsplit_once(':')
        .expect("a created ARN carries a resource segment")
        .1
        .to_string();

    // The shared table above, plus the resource id of the view created here: that
    // id resolves in the caller's own account, so neither answer can be coming
    // from a store lookup.
    let foreign_arns: Vec<String> = FOREIGN_BILLING_VIEW_ARNS
        .iter()
        .map(|arn| (*arn).to_string())
        .chain([format!("arn:aws:billing::111122223333:{own_id}")])
        .collect();

    for arn in &foreign_arns {
        for (operation, body_of) in BILLING_VIEW_ARN_OPERATIONS {
            assert_cross_account_refusal(&router, operation, body_of, arn).await;
        }
        for (operation, body_of) in RESOURCE_ARN_OPERATIONS {
            assert_cross_account_refusal(&router, operation, body_of, arn).await;
        }
    }

    // A resource type segment that is not `billingview/` is refused the same way
    // by the plane that admits it.
    for arn in FOREIGN_RESOURCE_TYPE_ARNS {
        for (operation, body_of) in RESOURCE_ARN_OPERATIONS {
            assert_cross_account_refusal(&router, operation, body_of, arn).await;
        }
    }

    // Nothing the refusals above touched: the caller's own store is exactly as
    // the create left it, tags and source views included.
    let state = service_com_amazonaws_billing_awsbilling::state::service_state(&*engine)
        .expect("the billing state is reachable from the shared engine");
    let state = state.lock();
    let stored = state
        .billing_views
        .get(&own)
        .expect("the view the harness created is still stored");
    assert!(
        stored.tags.is_empty(),
        "case \"after the refusals\": a refused cross-account call attaches no tag, got {:?}",
        stored.tags
    );
    assert_eq!(
        stored.source_views,
        vec![PRIMARY_ARN.to_string()],
        "case \"after the refusals\": a refused cross-account call changes no association, \
         got {:?}",
        stored.source_views
    );
    assert!(
        state.billing_views.contains_key(PRIMARY_ARN),
        "case \"after the refusals\": the account's PRIMARY view is still stored"
    );
}

#[tokio::test]
async fn an_arn_in_the_callers_own_account_is_answered_from_the_store() {
    let (router, _engine) = emulator_with_engine();
    let own = create_view(&router, "own-view").await;

    // The gate is keyed on the ACCOUNT the ARN embeds, so an ARN of the caller's
    // own account is decided by the store — whatever that decision is (200, a
    // 404 for an ARN naming nothing, a 400 for a view the service manages) it is
    // never the cross-account refusal.
    let own_arns = [
        ("an existing CUSTOM view", own.clone()),
        ("the managed PRIMARY view", PRIMARY_ARN.to_string()),
        ("an absent billing view", ABSENT_ARN.to_string()),
        (
            "an absent non-billingview resource",
            format!("arn:aws:billing::{ACCOUNT_ID}:budget/none"),
        ),
    ];
    for (row, arn) in &own_arns {
        for (operation, body_of) in RESOURCE_ARN_OPERATIONS {
            let case = format!("{row}: {operation} on {arn}");
            let (status, body) = call(&router, operation, &body_of(arn)).await;
            assert_ne!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: an ARN of the caller's own account is not refused across \
                 accounts, got {status} {body:?}"
            );
        }
    }
    for (row, arn) in &own_arns {
        for (operation, body_of) in [
            BILLING_VIEW_ARN_OPERATIONS[0],
            BILLING_VIEW_ARN_OPERATIONS[4],
            BILLING_VIEW_ARN_OPERATIONS[6],
        ] {
            let case = format!("{row}: {operation} on {arn}");
            let (status, body) = call(&router, operation, &body_of(arn)).await;
            assert_ne!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: an ARN of the caller's own account is not refused across \
                 accounts, got {status} {body:?}"
            );
        }
    }

    // The round trip the gate must not have broken: a tag written through the
    // caller's own ARN is the tag a later read reports, and removing it leaves
    // the set empty.
    let round_trip = create_view(&router, "round-trip-view").await;
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": round_trip, "resourceTags": [{ "key": "team", "value": "finance" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"tagging the caller's own view\": expected 200, got {status} {body:?}"
    );
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({ "resourceArn": round_trip }),
    )
    .await;
    assert_eq!(
        (status, listed["resourceTags"].clone()),
        (200, json!([{ "key": "team", "value": "finance" }])),
        "case \"reading back the caller's own tags\": expected the stored tag, got {status} \
         {listed:?}"
    );
    let (status, body) = call(
        &router,
        "UntagResource",
        &json!({ "resourceArn": round_trip, "resourceTagKeys": ["team"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"untagging the caller's own view\": expected 200, got {status} {body:?}"
    );
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({ "resourceArn": round_trip }),
    )
    .await;
    assert_eq!(
        (status, listed["resourceTags"].clone()),
        (200, json!([])),
        "case \"reading back after the untag\": expected no tags, got {status} {listed:?}"
    );
}
