// SPDX-License-Identifier: CC0-1.0

//! The `QuotaCode`s of the `glue` catalog that a sourced binding VACATED: each keeps
//! answering a row.
//!
//! THE RULE: every `QuotaCode` a service's default-quota catalog publishes keeps a row, and
//! a code whose own published quota no source states carries the row it has carried — when
//! a sourced binding took that plane to its own code, the plane that binding displaced. The
//! `QuotaCode` is the only key `GetAWSDefaultServiceQuota`, `GetServiceQuota`,
//! `RequestServiceQuotaIncrease` and `ListRequestedServiceQuotaChangeHistoryByQuota` take,
//! so a code that resolves to nothing makes a quota the service does have look
//! nonexistent; and a plane left with no code of its own is published under no row rather
//! than under a code minted for it.
//!
//! The converse half — a quota whose own code no source states is published under NO code —
//! is asserted by `glue_default_rows_under_their_published_codes.rs` and
//! `glue_account_planes_under_their_published_codes.rs`, which own it; this suite owns the
//! half above, so each invariant lives in one place.
//!
//! The table spans what the rule generalises over rather than repeating one row: a code
//! vacated by an account plane, one vacated by a Data Catalog plane and one vacated by a
//! per-table plane; a row carrying a sentence beside one whose sentence no source states
//! and which therefore carries none; a row published over a per-second window beside rows
//! published over none; and values from 10 to 100,000. Every row is read through BOTH
//! listings and BOTH code-keyed single reads, and driven through the increase path, because
//! the code is what a client acts on.
//!
//! The harness (`client`, `emulator`, `default_quotas_by_code`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use serde_json::json;

use crate::quota_workflow::{client, default_quotas_by_code, emulator, walk_pages};

/// One vacated code and the row it keeps.
struct Vacated {
    quota_code: &'static str,
    /// The quota the row carries: the plane the sourced binding displaced.
    quota_name: &'static str,
    value: f64,
    /// The sentence that plane has carried, or `None` where no source states one — in
    /// which case the row publishes none rather than one worded by this emulator.
    description: Option<&'static str>,
    /// The code whose sourced binding took the plane this code used to carry.
    took_by: &'static str,
    /// The quota that sourced binding publishes under `took_by`.
    took_by_quota_name: &'static str,
}

/// The codes this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of `glue` states the quota
/// published under `L-F953935E` (the account's databases), `L-11FA2C1A` (its crawlers) and
/// `L-096DBB95` (its spare compute capacity); the rows below are the planes those three
/// bindings displaced, each under the code its plane left, at the limit and with the
/// sentence this catalog has carried it under. No source states which quota `glue`
/// publishes under these three codes, so none of these rows is a claim that AWS publishes
/// that plane under that code — what is asserted here is that the code keeps resolving, and
/// that the plane it carries is published under exactly one code.
const VACATED: &[Vacated] = &[
    // The `UpdateTable` CALL RATE, under the code the account's database plane left when
    // the sourced binding took that plane to `L-F953935E`: a row published over a window
    // rather than a counted plane.
    Vacated {
        quota_code: "L-5E4153CA",
        quota_name: "Rate of UpdateTable requests",
        value: 10.0,
        description: Some(
            "The maximum number of UpdateTable requests you can make per second in this \
             account in the current Region.",
        ),
        took_by: "L-F953935E",
        took_by_quota_name: "Max databases per account",
    },
    // The account's CONCURRENT JOB RUNS, under the code the account-wide crawler plane left
    // when the sourced binding took that plane to `L-11FA2C1A`. Its published sentence is
    // one no source available here states, so the row carries none.
    Vacated {
        quota_code: "L-337244C9",
        quota_name: "Concurrent job runs per account",
        value: 200.0,
        description: None,
        took_by: "L-11FA2C1A",
        took_by_quota_name: "Number of crawlers per account",
    },
    // A PER-TABLE plane rather than an account one: the table versions of one table, under
    // a code whose own quota no source states, carrying the plane the sourced `L-096DBB95`
    // binding displaced.
    Vacated {
        quota_code: "L-2E9FB93F",
        quota_name: "Table versions per table",
        value: 100_000.0,
        description: Some("The maximum number of table versions per table."),
        took_by: "L-096DBB95",
        took_by_quota_name: "Max spare compute capacity consumed in data processing units \
                             (DPUs) per account.",
    },
];

/// A vacated code keeps a row: it is published in both listings and resolves through both
/// code-keyed single reads, carrying the plane the sourced binding displaced — while that
/// binding's own code still carries the quota a source states for it.
#[tokio::test]
async fn a_vacated_glue_code_keeps_the_row_it_has_carried() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let catalog = default_quotas_by_code(&caller, "glue").await;
    // One walk of the applied listing for the whole table: every published code must appear
    // in it, and the walk does not depend on which row is being checked.
    let applied_listing = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "glue"}),
        "Quotas",
        None,
    )
    .await;
    for row in VACATED {
        let case = format!("glue/{}", row.quota_code);
        let pair = json!({"ServiceCode": "glue", "QuotaCode": row.quota_code});

        let listed = catalog.get(row.quota_code).unwrap_or_else(|| {
            panic!(
                "case {case}: every code this catalog publishes keeps a row, got the codes \
                 {:?}",
                catalog.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            listed["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the plane this code has carried, got {listed}"
        );
        assert_eq!(
            listed["Value"].as_f64(),
            Some(row.value),
            "case {case}: expected the limit this code has carried, got {listed}"
        );
        match row.description {
            Some(sentence) => assert_eq!(
                listed["Description"],
                json!(sentence),
                "case {case}: expected the sentence this row has carried, got {listed}"
            ),
            None => assert!(
                listed["Description"].as_str().is_none_or(str::is_empty),
                "case {case}: a row whose sentence no source states publishes none, got \
                 {listed}"
            ),
        }

        // Both code-keyed single reads resolve it, and the default read returns exactly the
        // record the listing carries.
        let read = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
        assert_eq!(
            &read, listed,
            "case {case}: the listing carries the record the single read returns, got {read} \
             and {listed}"
        );
        let applied = caller.ok("GetServiceQuota", pair.clone()).await["Quota"].clone();
        assert_eq!(
            (applied["QuotaName"].clone(), applied["Value"].as_f64()),
            (json!(row.quota_name), Some(row.value)),
            "case {case}: the applied plane resolves the same quota at its default value, \
             got {applied}"
        );

        // The APPLIED listing publishes the code too, so a client that walks either listing
        // finds it.
        assert!(
            applied_listing
                .iter()
                .any(|quota| quota["QuotaCode"] == json!(row.quota_code)),
            "case {case}: the applied listing publishes every code the default listing does"
        );

        // The sourced binding that took this code's former plane still publishes its own
        // quota: carrying a displaced plane here never overwrites a row a source states.
        let sourced = catalog.get(row.took_by).unwrap_or_else(|| {
            panic!("case {case}: expected {} to be published", row.took_by)
        });
        assert_eq!(
            sourced["QuotaName"],
            json!(row.took_by_quota_name),
            "case {case}: {} must keep the quota a source states for it, got {sourced}",
            row.took_by
        );
    }
}

/// The plane a vacated code carries is published under exactly ONE code of the service, and
/// no code of this catalog resolves to nothing.
#[tokio::test]
async fn a_displaced_plane_a_vacated_code_carries_is_published_once() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    let catalog = default_quotas_by_code(&caller, "glue").await;
    for row in VACATED {
        let case = format!("glue/{}", row.quota_code);
        let carrying: Vec<&String> = catalog
            .iter()
            .filter(|(_, quota)| quota["QuotaName"] == json!(row.quota_name))
            .map(|(code, _)| code)
            .collect();
        assert_eq!(
            carrying,
            vec![&row.quota_code.to_string()],
            "case {case}: the plane this code carries is published under one code only, got \
             it under {carrying:?}"
        );
    }

    // The invariant over the WHOLE catalog, not only the table above: every code the
    // listing publishes resolves through the single read, and resolves the same quota.
    for (code, listed) in &catalog {
        let read = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": "glue", "QuotaCode": code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            &read, listed,
            "case glue/{code}: every published code resolves the row the listing carries, \
             got {read}"
        );
    }
}

/// An increase requested by a vacated code is minted against the quota that code's row
/// carries, reads back in that code's history, and leaves the published default where it
/// was.
#[tokio::test]
async fn an_increase_requested_by_a_vacated_glue_code_targets_its_row_s_quota() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    for row in VACATED {
        let case = format!("glue/{}", row.quota_code);
        let pair = json!({"ServiceCode": "glue", "QuotaCode": row.quota_code});
        let desired = row.value * 2.0;

        let mut body = pair.clone();
        body["DesiredValue"] = json!(desired);
        let requested =
            caller.ok("RequestServiceQuotaIncrease", body).await["RequestedQuota"].clone();
        assert_eq!(
            (
                requested["QuotaCode"].clone(),
                requested["QuotaName"].clone(),
                requested["DesiredValue"].as_f64(),
            ),
            (
                json!(row.quota_code),
                json!(row.quota_name),
                Some(desired)
            ),
            "case {case}: an increase must be minted against the quota published under the \
             code it names, got {requested}"
        );

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
            "case {case}: the history of this quota carries the request made against it, got \
             {history:?}"
        );

        let after = caller.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (after["QuotaName"].clone(), after["Value"].as_f64()),
            (json!(row.quota_name), Some(row.value)),
            "case {case}: the published default is unchanged by a request, got {after}"
        );
    }
}
