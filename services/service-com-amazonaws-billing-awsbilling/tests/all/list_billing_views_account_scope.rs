// SPDX-License-Identifier: CC0-1.0

//! The account scope of `ListBillingViews`, end to end over the awsJson1_0 wire.
//!
//! One rule: this read reports the caller's OWN billing views, so a record the
//! store holds under an ARN of another account is part of no page — the same
//! account boundary every ARN-addressed read of a view decides before it looks
//! the ARN up. A listing is therefore not a second channel for a record that is
//! refused with `AccessDeniedException` (403) when a request names it.
//!
//! The rows span what the rule generalises over: the foreign account id, the
//! partition, the resource id (including `primary`, the id of a view that DOES
//! exist in the caller's own account) and its depth, the `BillingViewType` of the
//! stored record, which filter member the request carries (each of the six, and
//! none at all), whether the filter names a foreign account, and the page size a
//! caller walks with. The control in the other direction is pinned in every
//! case: the caller's own views — planted and created alike — ARE reported, so
//! the rule is a boundary rather than a blanket emptiness.
//!
//! The ARN table and the harness (`call`, `emulator_with_engine`, `plant_view`, …)
//! are the ones `arn_account_boundary.rs` and `billing_views.rs` own.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;

use crate::arn_account_boundary::FOREIGN_BILLING_VIEW_ARNS;
use crate::billing_views::{
    call, emulator_with_engine, error_code, now_secs, plant_view, ACCOUNT_ID, PRIMARY_ARN,
};

/// A marker carried by every field of a planted foreign record an answer could
/// possibly report (its name, description, tag and source view), so any part of
/// such a record reaching a body is recognisable.
const FOREIGN_MARKER: &str = "foreign-account-record";

/// The accounts the foreign records of [`FOREIGN_BILLING_VIEW_ARNS`] are owned
/// by, as the `ownerAccountId` filter would name them.
const FOREIGN_OWNER_ACCOUNTS: [&str; 3] = ["111122223333", "123456789012", "999999999999"];

/// The account the source views of every planted foreign record belong to, as the
/// `sourceAccountId` filter would name it.
const FOREIGN_SOURCE_ACCOUNT: &str = "444455556666";

/// Every value the modeled `BillingViewType` declares
/// (`smithy_query(query="shape", shape="BillingViewType")`), so the planted
/// foreign records span the types a `billingViewTypes` filter can name — and the
/// widest such filter is the whole enum.
const EVERY_VIEW_TYPE: [&str; 5] = [
    "PRIMARY",
    "BILLING_GROUP",
    "CUSTOM",
    "BILLING_TRANSFER",
    "BILLING_TRANSFER_SHOWBACK",
];

/// The modeled `SearchOption`, which declares exactly one value
/// (`smithy_query(query="shape", shape="SearchOption")`).
const STARTS_WITH: &str = "STARTS_WITH";

/// The names the `CUSTOM` views this suite creates in the caller's own account
/// carry.
const OWN_NAMES: [&str; 2] = ["scope-own-alpha", "scope-own-beta"];

/// Plant one billing-view record under `arn` — an ARN of another account — with
/// [`FOREIGN_MARKER`] in every field an answer could report.
///
/// `CreateBillingView` mints ARNs in the CALLER's account only, so a record of
/// another account is written the way the service holds one, through the
/// harness's planting helper. That state is exactly what this rule is about.
fn plant_foreign_view(engine: &Arc<engine::InMemoryEngine>, arn: &str, view_type: &str) {
    plant_view(engine, arn, |view| {
        view.name = format!("{FOREIGN_MARKER}-name");
        view.description = Some(format!("{FOREIGN_MARKER}-description"));
        view.billing_view_type = view_type.to_string();
        // The owner of a view is the account its ARN embeds (an ARN's fifth
        // colon-separated segment).
        view.owner_account_id = arn.split(':').nth(4).unwrap_or_default().to_string();
        view.source_views = vec![format!(
            "arn:aws:billing::{FOREIGN_SOURCE_ACCOUNT}:billingview/{FOREIGN_MARKER}-source"
        )];
        view.tags = BTreeMap::from([(
            format!("{FOREIGN_MARKER}-key"),
            Some(format!("{FOREIGN_MARKER}-value")),
        )]);
    });
}

/// Create one `CUSTOM` billing view of the caller's own account and answer the
/// ARN the service minted for it.
async fn create_own_view(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": [PRIMARY_ARN], "clientToken": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateBillingView to answer 200, got {status} {created}"
    );
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: a create must answer an arn, got {created}"))
        .to_string()
}

/// The world every test here reads over: the caller's own views (the account's
/// `PRIMARY` view and two created `CUSTOM` ones) beside a record under each ARN
/// of [`FOREIGN_BILLING_VIEW_ARNS`], one per modeled `BillingViewType`.
///
/// Two of those foreign ARNs sort BELOW every ARN of the caller's account and
/// three above it (`-` precedes `:`, and the foreign account ids are larger), so
/// a walk that held them would both report them and move its page boundaries.
async fn world() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = emulator_with_engine();
    let mut own = vec![PRIMARY_ARN.to_string()];
    for name in OWN_NAMES {
        own.push(create_own_view(&router, name).await);
    }
    own.sort();
    for (arn, view_type) in FOREIGN_BILLING_VIEW_ARNS.iter().zip(EVERY_VIEW_TYPE) {
        plant_foreign_view(&engine, arn, view_type);
    }
    (router, engine, own)
}

/// The ARNs an answer reports, in the order it reports them.
fn reported_arns(body: &Value) -> Vec<String> {
    body["billingViews"]
        .as_array()
        .map(|listed| {
            listed
                .iter()
                .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that no field of a planted foreign record reached `body`.
fn assert_no_foreign_content(case: &str, body: &Value) {
    let rendered = serde_json::to_string(body).expect("the response body serializes");
    assert!(
        !rendered.contains(FOREIGN_MARKER),
        "case {case:?}: no part of a record stored under another account's ARN may reach the \
         answer, got {rendered:?}"
    );
}

/// The `names` request member searching for `prefix`.
fn names_filter(prefix: &str) -> Value {
    json!([{ "searchOption": STARTS_WITH, "searchValue": prefix }])
}

#[tokio::test]
async fn every_filter_spelling_lists_the_callers_own_views_only() {
    let (router, _engine, own) = world().await;
    let now = now_secs();

    // (case, request, the ARNs the request must select)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!({}), own.clone()),
        ("an empty arns list", json!({ "arns": [] }), own.clone()),
        (
            "every modeled billing view type",
            json!({ "billingViewTypes": EVERY_VIEW_TYPE }),
            own.clone(),
        ),
        (
            "the caller's own owner account",
            json!({ "ownerAccountId": ACCOUNT_ID }),
            own.clone(),
        ),
        (
            "a period covering every record",
            json!({ "activeTimeRange": { "activeAfterInclusive": now, "activeBeforeInclusive": now + 3_600 } }),
            own.clone(),
        ),
        (
            // Only an always-active view is listed for a period before it came
            // into existence, which of the caller's views is the PRIMARY one —
            // and the foreign record of the PRIMARY type is always active too,
            // so this row is the sharp one.
            "a period long past",
            json!({ "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 3_600 } }),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "a name prefix only the foreign records carry",
            json!({ "names": names_filter(FOREIGN_MARKER) }),
            vec![],
        ),
        (
            "the source account the foreign records draw on",
            json!({ "sourceAccountId": FOREIGN_SOURCE_ACCOUNT }),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "ListBillingViews", &request).await;
        assert_eq!(
            (status, reported_arns(&body).as_slice()),
            (200, expected.as_slice()),
            "case {case:?}: expected the caller's own views for {request}, got {status} {body}"
        );
        assert_no_foreign_content(case, &body);
    }

    // Each foreign owner, named by the filter that selects by owner: the record
    // is in the store, owned by that very account, and selected by nothing.
    for owner in FOREIGN_OWNER_ACCOUNTS {
        let case = format!("the owner account {owner} of a foreign record");
        let request = json!({ "ownerAccountId": owner });
        let (status, body) = call(&router, "ListBillingViews", &request).await;
        assert_eq!(
            (status, reported_arns(&body).as_slice()),
            (200, [].as_slice()),
            "case {case:?}: expected an empty page for {request}, got {status} {body}"
        );
        assert_no_foreign_content(&case, &body);
    }
}

#[tokio::test]
async fn a_paged_walk_reports_the_callers_own_views_exactly_once() {
    let (router, _engine, own) = world().await;

    // A page size for every way the caller's set can be cut: one view at a time,
    // a size that does not divide it, one that holds it whole, and the default
    // the request omits ("Default is 100", the `maxResults` member doc).
    for page_size in [None, Some(1), Some(2), Some(3), Some(100)] {
        let case = format!("a page size of {page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // A generous bound on the number of pages: the walk must end, and one
        // page per view plus a few is already more than any size needs.
        for _ in 0..own.len() + 5 {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListBillingViews", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for {request}, got {status} {body}"
            );
            assert_no_foreign_content(&case, &body);
            let page = reported_arns(&body);
            assert!(
                page.len() <= page_size.unwrap_or(100),
                "case {case:?}: a page carries no more than the page size, got {page:?}"
            );
            walked.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end, got a token still outstanding after {} views",
            walked.len()
        );
        assert_eq!(
            (walked.as_slice(), walked.len()),
            (own.as_slice(), own.len()),
            "case {case:?}: expected every view of the caller's account exactly once, got \
             {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_foreign_record_is_neither_named_nor_listed_and_stays_in_the_store() {
    let (router, engine, own) = world().await;

    for arn in FOREIGN_BILLING_VIEW_ARNS {
        // Named, the record is refused on the account boundary…
        let (status, body) = call(&router, "ListBillingViews", &json!({ "arns": [arn] })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {arn:?}: naming a foreign-account ARN is refused, got {status} {body}"
        );
        assert_no_foreign_content(arn, &body);

        // …and the sibling read of the same ARN agrees.
        let (status, body) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {arn:?}: GetBillingView refuses the same ARN, got {status} {body}"
        );
        assert_no_foreign_content(arn, &body);
    }

    // Unnamed, it is reported by no page either.
    let (status, body) = call(&router, "ListBillingViews", &json!({})).await;
    let listed = reported_arns(&body);
    assert_eq!(
        (status, listed.as_slice()),
        (200, own.as_slice()),
        "case \"the unfiltered listing\": expected the caller's own views, got {status} {body}"
    );
    for arn in FOREIGN_BILLING_VIEW_ARNS {
        assert!(
            !listed.contains(&arn.to_string()),
            "case {arn:?}: a foreign-account ARN is listed by no request, got {listed:?}"
        );
    }

    // The listing answered from the store without changing it: the records it
    // did not report are still there, exactly as planted.
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    assert_eq!(
        state.billing_views.len(),
        own.len() + FOREIGN_BILLING_VIEW_ARNS.len(),
        "case \"after the listings\": the store still holds every record, got {:?}",
        state.billing_views.keys().collect::<Vec<_>>()
    );
    for arn in FOREIGN_BILLING_VIEW_ARNS {
        let stored = state
            .billing_views
            .get(arn)
            .unwrap_or_else(|| panic!("case {arn:?}: the planted record is gone"));
        assert_eq!(
            stored.name.as_str(),
            format!("{FOREIGN_MARKER}-name").as_str(),
            "case {arn:?}: a listing changes nothing it did not report, got {stored:?}"
        );
    }
}
