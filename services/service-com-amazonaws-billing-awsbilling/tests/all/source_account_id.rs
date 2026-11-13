// SPDX-License-Identifier: CC0-1.0

//! The `sourceAccountId` surface of the billing-view reads, end to end over the
//! awsJson1_0 wire.
//!
//! One rule, asserted over the class it generalises over: a view's source account
//! is read off the billing views it DRAWS FROM, so `sourceAccountId` is the
//! account owning the first ARN in its stored source views (absent for a view
//! that draws from none), and the `ListBillingViews` `sourceAccountId` filter
//! selects every view that uses a billing view of that account as a source.
//!
//! Sources (the model, not a recording):
//! `smithy_query(query="doc", shape="ListBillingViewsRequest", member="sourceAccountId")`
//! — "Filters the results to include only billing views that use the specified
//! account as a source"; `shape="BillingViewListElement"`/`"BillingViewElement"`,
//! `member="sourceAccountId"` — "The Amazon Web Services account ID that owns the
//! source billing view, if this is a derived billing view."
//!
//! The rows differ along everything the rule generalises over: how many sources a
//! view holds (one, several, the modeled maximum of ten), which accounts own them
//! (the caller's, another account's, a mix in either order), and whether the view
//! draws from anything at all. The harness (`call`, `emulator`, …) is the one
//! `billing_views.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{call, emulator, error_code, ACCOUNT_ID, PRIMARY_ARN};

/// Account ids other than the emulated caller's, for the ARNs of source views
/// owned elsewhere. Any 12-digit string is one — the modeled `AccountId` shape is
/// `^[0-9]{12}$`.
const OTHER_ACCOUNT: &str = "111122223333";
const THIRD_ACCOUNT: &str = "999999999999";

/// A 12-digit account that owns none of the source views any row names, so no
/// view may ever be selected for it.
const UNRELATED_ACCOUNT: &str = "123456789012";

/// A billing-view ARN of `account`, in the modeled `BillingViewArn` form.
fn view_arn(account: &str, view_id: &str) -> String {
    format!("arn:aws:billing::{account}:billingview/{view_id}")
}

/// Create one `CUSTOM` billing view drawing from `source_views` and return its
/// minted ARN.
///
/// `CreateBillingView` stores the `sourceViews` it is given without resolving
/// them, so the rows below can name a view of another account; were that
/// ownership check ever added, these creates — not the assertions they set up —
/// are what would start refusing, and the rows would move to an association
/// that is allowed.
async fn create_view(
    router: &MultiServiceRouter,
    case: &str,
    token: &str,
    source_views: &[String],
) -> String {
    let (status, body) = call(
        router,
        "CreateBillingView",
        &json!({ "name": token, "sourceViews": source_views, "clientToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the harness create must answer 200, got {status} {body:?}"
    );
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateBillingView answered no arn: {body:?}"))
        .to_string()
}

/// The `sourceAccountId` `GetBillingView` reports for `arn`, as an `Option` so an
/// absent member and a present one are distinguishable.
async fn get_source_account(router: &MultiServiceRouter, case: &str, arn: &str) -> Option<String> {
    let (status, body) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBillingView to answer 200, got {status} {body:?}"
    );
    body["billingView"]["sourceAccountId"]
        .as_str()
        .map(str::to_string)
}

/// Every `(arn, sourceAccountId)` pair `ListBillingViews` reports for `request`.
async fn list_views(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> Vec<(String, Option<String>)> {
    let (status, body) = call(router, "ListBillingViews", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListBillingViews to answer 200, got {status} {body:?}"
    );
    body["billingViews"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: billingViews is a list, got {body:?}"))
        .iter()
        .map(|view| {
            (
                view["arn"].as_str().unwrap_or_default().to_string(),
                view["sourceAccountId"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// One row of the table: a view created over `source_views`, the account its
/// reads must report, and every account whose views it uses as a source.
struct Row {
    case: &'static str,
    source_views: Vec<String>,
    reported: &'static str,
    selecting_accounts: Vec<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one source view in the caller's own account",
            source_views: vec![PRIMARY_ARN.to_string()],
            reported: ACCOUNT_ID,
            selecting_accounts: vec![ACCOUNT_ID],
        },
        Row {
            case: "three source views, all in the caller's own account",
            source_views: vec![
                PRIMARY_ARN.to_string(),
                view_arn(ACCOUNT_ID, "own-a"),
                view_arn(ACCOUNT_ID, "own-b"),
            ],
            reported: ACCOUNT_ID,
            selecting_accounts: vec![ACCOUNT_ID],
        },
        Row {
            case: "one source view owned by another account",
            source_views: vec![view_arn(OTHER_ACCOUNT, "primary")],
            reported: OTHER_ACCOUNT,
            selecting_accounts: vec![OTHER_ACCOUNT],
        },
        Row {
            case: "source views owned by two other accounts",
            source_views: vec![
                view_arn(OTHER_ACCOUNT, "primary"),
                view_arn(THIRD_ACCOUNT, "primary"),
            ],
            reported: OTHER_ACCOUNT,
            selecting_accounts: vec![OTHER_ACCOUNT, THIRD_ACCOUNT],
        },
        Row {
            case: "an own-account source view first, a foreign one second",
            source_views: vec![PRIMARY_ARN.to_string(), view_arn(THIRD_ACCOUNT, "primary")],
            reported: ACCOUNT_ID,
            selecting_accounts: vec![ACCOUNT_ID, THIRD_ACCOUNT],
        },
        Row {
            case: "a foreign source view first, an own-account one second",
            source_views: vec![view_arn(THIRD_ACCOUNT, "primary"), PRIMARY_ARN.to_string()],
            reported: THIRD_ACCOUNT,
            selecting_accounts: vec![ACCOUNT_ID, THIRD_ACCOUNT],
        },
        Row {
            case: "ten source views, the modeled maximum, spanning three accounts",
            source_views: (0..10)
                .map(|index| {
                    let account = match index % 3 {
                        0 => OTHER_ACCOUNT,
                        1 => ACCOUNT_ID,
                        _ => THIRD_ACCOUNT,
                    };
                    view_arn(account, &format!("wide-{index}"))
                })
                .collect(),
            reported: OTHER_ACCOUNT,
            selecting_accounts: vec![ACCOUNT_ID, OTHER_ACCOUNT, THIRD_ACCOUNT],
        },
    ]
}

#[tokio::test]
async fn every_read_reports_the_account_owning_the_views_a_view_draws_from() {
    let router = emulator();
    let rows = rows();
    let mut arns = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        arns.push(
            create_view(
                &router,
                row.case,
                &format!("src-acct-{index}"),
                &row.source_views,
            )
            .await,
        );
    }

    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        assert_eq!(
            get_source_account(&router, case, arn).await.as_deref(),
            Some(row.reported),
            "case {case:?}: GetBillingView must report the account owning the source view, \
             sources {:?}",
            row.source_views
        );
    }

    // The unfiltered list reports the same projection as the per-view read, so no
    // caller can see the two surfaces disagree about one record.
    let listed = list_views(&router, "no filter", &json!({})).await;
    for (row, arn) in rows.iter().zip(&arns) {
        let case = row.case;
        let entry = listed
            .iter()
            .find(|(listed_arn, _)| listed_arn == arn)
            .unwrap_or_else(|| panic!("case {case:?}: {arn} must be listed, got {listed:?}"));
        assert_eq!(
            entry.1.as_deref(),
            Some(row.reported),
            "case {case:?}: ListBillingViews must report the same source account as the read, \
             got {entry:?}"
        );
    }

    // The account's default view draws on no other view, so it is not derived
    // from one and reports no source account at all.
    assert_eq!(
        get_source_account(&router, "the default PRIMARY view", PRIMARY_ARN).await,
        None,
        "case \"the default PRIMARY view\": a view that draws from nothing has no source account"
    );
    let primary_entry = listed
        .iter()
        .find(|(arn, _)| arn == PRIMARY_ARN)
        .expect("the default PRIMARY view is listed");
    assert_eq!(
        primary_entry.1, None,
        "case \"the default PRIMARY view\": expected no sourceAccountId in the list, got \
         {primary_entry:?}"
    );
}

#[tokio::test]
async fn the_filter_selects_every_view_that_uses_the_account_as_a_source() {
    let router = emulator();
    let rows = rows();
    let mut arns = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        arns.push(
            create_view(
                &router,
                row.case,
                &format!("filter-{index}"),
                &row.source_views,
            )
            .await,
        );
    }

    for account in [ACCOUNT_ID, OTHER_ACCOUNT, THIRD_ACCOUNT, UNRELATED_ACCOUNT] {
        let case = format!("sourceAccountId={account}");
        let selected: Vec<String> = list_views(&router, &case, &json!({ "sourceAccountId": account }))
            .await
            .into_iter()
            .map(|(arn, _)| arn)
            .collect();
        let mut expected: Vec<String> = rows
            .iter()
            .zip(&arns)
            .filter(|(row, _)| row.selecting_accounts.contains(&account))
            .map(|(_, arn)| arn.clone())
            .collect();
        expected.sort();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the views using that account as a source, got {selected:?}"
        );
    }

    // The filter composes with the others rather than replacing them: a view must
    // satisfy both to be selected.
    let case = "sourceAccountId with billingViewTypes=PRIMARY";
    let selected = list_views(
        &router,
        case,
        &json!({ "sourceAccountId": ACCOUNT_ID, "billingViewTypes": ["PRIMARY"] }),
    )
    .await;
    assert!(
        selected.is_empty(),
        "case {case:?}: the default PRIMARY view uses no account as a source, got {selected:?}"
    );

    // A value the modeled `AccountId` pattern (`^[0-9]{12}$`) refuses is a 400
    // whatever the store holds, so the filter never silently matches nothing.
    for (case, value) in [
        ("too short", "12345"),
        ("not digits", "abcdefghijkl"),
        ("too long", "0000000000000"),
    ] {
        let (status, body) = call(
            &router,
            "ListBillingViews",
            &json!({ "sourceAccountId": value }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException for {value:?}, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn the_source_account_follows_the_sources_a_later_call_changes() {
    let router = emulator();
    let case = "association round trip";
    let arn = create_view(&router, case, "round-trip", &[PRIMARY_ARN.to_string()]).await;
    let other = create_view(&router, case, "other-source", &[PRIMARY_ARN.to_string()]).await;

    assert_eq!(
        get_source_account(&router, case, &arn).await.as_deref(),
        Some(ACCOUNT_ID),
        "case {case:?}: the created view draws from the caller's own PRIMARY view"
    );

    // Dropping every source leaves a view that draws from nothing: it is no
    // longer derived, so no account owns its source view and no filter value
    // selects it.
    let (status, body) = call(
        &router,
        "DisassociateSourceViews",
        &json!({ "arn": arn, "sourceViews": [PRIMARY_ARN] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DisassociateSourceViews to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        get_source_account(&router, case, &arn).await,
        None,
        "case {case:?}: a view whose sources were all removed reports no source account"
    );
    for account in [ACCOUNT_ID, OTHER_ACCOUNT] {
        let selected: Vec<String> = list_views(
            &router,
            case,
            &json!({ "sourceAccountId": account }),
        )
        .await
        .into_iter()
        .map(|(listed, _)| listed)
        .collect();
        assert!(
            !selected.contains(&arn),
            "case {case:?}: a view with no source must not be selected for {account}, got \
             {selected:?}"
        );
    }

    // Associating a source puts the account that owns it back on every read.
    let (status, body) = call(
        &router,
        "AssociateSourceViews",
        &json!({ "arn": arn, "sourceViews": [other] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AssociateSourceViews to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        get_source_account(&router, case, &arn).await.as_deref(),
        Some(ACCOUNT_ID),
        "case {case:?}: the re-associated source is owned by the caller's account"
    );
    let selected: Vec<String> = list_views(
        &router,
        case,
        &json!({ "sourceAccountId": ACCOUNT_ID }),
    )
    .await
    .into_iter()
    .map(|(listed, _)| listed)
    .collect();
    assert!(
        selected.contains(&arn),
        "case {case:?}: the view is selected again once it draws from that account, got \
         {selected:?}"
    );

    // Reading the projection changes nothing a later caller sees: the same reads
    // answer the same way.
    assert_eq!(
        get_source_account(&router, case, &arn).await.as_deref(),
        Some(ACCOUNT_ID),
        "case {case:?}: a repeated read reports the same source account"
    );
}
