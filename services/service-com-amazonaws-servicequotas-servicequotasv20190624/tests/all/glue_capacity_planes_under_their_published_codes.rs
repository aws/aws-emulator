// SPDX-License-Identifier: CC0-1.0

//! The INCREASE PATH of the `glue` quotas whose `QuotaCode` this catalog had bound to a
//! per-API request rate or to a neighbouring plane: the development-endpoint, integration
//! and data-quality-ruleset planes of an account, and the Data Catalog's global tag plane.
//!
//! THE RULE: for a `ServiceCode`, the row published under a `QuotaCode` carries the
//! `QuotaName`, `Value`, `Unit`, `Adjustable`, `Period`, `UsageMetric` and `Description` of
//! the quota the service publishes UNDER THAT CODE, never a neighbouring quota's. The code
//! is the only key `GetAWSDefaultServiceQuota`, `GetServiceQuota` and
//! `RequestServiceQuotaIncrease` take, so the quota a client reads by a code is the quota
//! its increase request targets: a row bound to the wrong quota aims the request at a limit
//! the client never asked about.
//!
//! Which quota each of these codes publishes is pinned member for member — beside the
//! quotas displaced by those bindings and the code left carrying no row — by
//! `glue_default_rows_under_their_published_codes.rs`, which owns that table. This suite
//! drives the SAME codes through the increase path, so the binding is asserted where a
//! client ACTS on it and not only where it reads it: one copy of each invariant, each in
//! the suite whose subject it is.
//!
//! The table spans what the rule generalises over rather than repeating one row: an account
//! plane named in the service's `Max … per account` form and one in its `Number of …` form,
//! a Data Catalog plane published with the terse `Catalog: …` sentence, and default values
//! from 25 to 1,000,000. The harness (`client`, `emulator`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, emulator, walk_pages};

/// One rebound row, by the quota the service publishes under its code.
struct Plane {
    quota_code: &'static str,
    quota_name: &'static str,
    /// The default `Value` the service publishes under the code.
    default_value: f64,
}

/// The codes this suite drives through the increase path.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of `glue` states which quota
/// is published under each of these codes, with the name and value it carries for it.
const PLANES: &[Plane] = &[
    Plane {
        quota_code: "L-DBA56E2F",
        quota_name: "Max development endpoint per account",
        default_value: 25.0,
    },
    Plane {
        quota_code: "L-F1AAB534",
        quota_name: "Number of integrations",
        default_value: 40.0,
    },
    Plane {
        quota_code: "L-87F5991C",
        quota_name: "Max data quality rulesets per account",
        default_value: 1_000_000.0,
    },
    Plane {
        quota_code: "L-785C1F39",
        quota_name: "Global max tags",
        default_value: 10_000.0,
    },
];

/// An increase requested by one of these codes is minted against the quota the service
/// publishes under it, reads back under that code in the history, and leaves the published
/// default where it was.
#[tokio::test]
async fn an_increase_requested_by_a_rebound_glue_code_targets_that_code_s_quota() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for plane in PLANES {
        let case = format!("glue/{}", plane.quota_code);
        let pair = json!({"ServiceCode": "glue", "QuotaCode": plane.quota_code});
        let desired = plane.default_value * 2.0;

        // What the client reads by the code before it acts on it.
        let default = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
        assert_eq!(
            (default["QuotaName"].clone(), default["Value"].as_f64()),
            (json!(plane.quota_name), Some(plane.default_value)),
            "case {case}: expected the quota published under this code, got {default}"
        );

        let mut body = pair.clone();
        body["DesiredValue"] = json!(desired);
        let requested = caller.ok("RequestServiceQuotaIncrease", body).await["RequestedQuota"]
            .clone();

        assert_eq!(
            (
                requested["ServiceCode"].clone(),
                requested["QuotaCode"].clone(),
                requested["QuotaName"].clone(),
                requested["DesiredValue"].as_f64(),
            ),
            (
                json!("glue"),
                json!(plane.quota_code),
                json!(plane.quota_name),
                Some(desired),
            ),
            "case {case}: an increase must be minted against the quota published under the \
             code it names, got {requested}"
        );
        assert_eq!(
            requested["Status"],
            json!("PENDING"),
            "case {case}: a new request awaits its decision, got {requested}"
        );

        // The history is keyed on the same pair, so the request a client made by this code
        // is the one it reads back under it.
        let history = walk_pages(
            &caller,
            "ListRequestedServiceQuotaChangeHistoryByQuota",
            &pair,
            "RequestedQuotas",
            None,
        )
        .await;
        assert!(
            history.contains(&requested),
            "case {case}: the history of this quota must carry the request made against it, \
             got {history:?}"
        );
        assert!(
            history.iter().all(|entry| entry["QuotaName"] == json!(plane.quota_name)),
            "case {case}: every entry of this quota's history names that quota, got \
             {history:?}"
        );

        // A request never moves what the service publishes as the default.
        let after = caller.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (after["QuotaName"].clone(), after["Value"].as_f64()),
            (json!(plane.quota_name), Some(plane.default_value)),
            "case {case}: the published default must be unchanged by a request, got {after}"
        );
    }
}

/// A code this catalog publishes no quota of the service under carries no row, so an
/// increase cannot be requested against it either: the increase path refuses it with the
/// error the model declares for a quota the service does not publish.
#[tokio::test]
async fn an_increase_requested_by_a_glue_code_with_no_published_quota_is_refused() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-3");

    // A well-formed code in AWS's minted namespace that this catalog publishes no `glue`
    // quota under. The codes whose own published quota no source states are not of this
    // kind: each keeps a row (see `glue_vacated_codes_keep_their_rows.rs`), so each
    // resolves.
    let (status, error) = caller
        .refused(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "glue",
                "QuotaCode": "L-00000000",
                "DesiredValue": 2_000.0,
            }),
        )
        .await;
    assert_eq!(
        (status, error.as_str()),
        (400, "NoSuchResourceException"),
        "case glue/L-00000000: an increase on a code this service publishes nothing under \
         must be refused"
    );

    // And nothing was created for it: the history of that pair is refused the same way.
    let (status, error) = caller
        .refused(
            "ListRequestedServiceQuotaChangeHistoryByQuota",
            json!({"ServiceCode": "glue", "QuotaCode": "L-00000000"}),
        )
        .await;
    assert_eq!(
        (status, error.as_str()),
        (400, "NoSuchResourceException"),
        "case glue/L-00000000: the history of a code this service publishes nothing under \
         must be refused"
    );
}

/// Every `Value` this suite pins is one `GetServiceQuota` resolves at the same code, so the
/// applied plane and the default plane agree on which quota a code names.
#[tokio::test]
async fn the_applied_plane_resolves_the_same_glue_quota_under_each_rebound_code() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    for plane in PLANES {
        let case = format!("glue/{}", plane.quota_code);
        let applied = caller
            .ok(
                "GetServiceQuota",
                json!({"ServiceCode": "glue", "QuotaCode": plane.quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            (
                applied["QuotaName"].clone(),
                applied["Value"].as_f64(),
                applied["Unit"].clone(),
            ),
            (json!(plane.quota_name), Some(plane.default_value), json!("None")),
            "case {case}: the applied read resolves the quota published under the code at its \
             default value, got {applied}"
        );
        assert!(
            applied.get("Period").is_none_or(Value::is_null)
                && applied.get("UsageMetric").is_none_or(Value::is_null),
            "case {case}: a counted plane is published over no window and meters no API call \
             count, got {applied}"
        );
    }
}
