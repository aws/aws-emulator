// SPDX-License-Identifier: CC0-1.0

//! The `glue` codes that publish an account-wide COUNT or CONCURRENCY plane, including the
//! two this catalog had bound to a per-API request rate and to a neighbouring plane.
//!
//! THE RULE: for a `ServiceCode`, the row published under a `QuotaCode` carries the
//! `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`, `Period`, `UsageMetric` and
//! `Description` of the quota the service publishes UNDER THAT CODE — the service's own
//! wording, never a neighbouring quota's and never a sentence generated from an operation
//! name — and a quota whose own code no source states is published under no code at all. The
//! code is the only key `GetAWSDefaultServiceQuota`, `GetServiceQuota` and
//! `RequestServiceQuotaIncrease` take, so a row bound to the wrong quota hands a client a
//! limit it never asked about: a per-second call rate read where a resource count belongs is
//! wrong by orders of magnitude in the direction that refuses legal work.
//!
//! The table spans what the rule generalises over rather than repeating one row: a COUNT
//! plane of the whole account beside the CONCURRENCY plane of the same account; the
//! per-parent sibling of each counted plane (per table, per job) so the account ceiling and
//! the per-resource one are asserted to be different rows under different codes; a plane
//! this catalog had bound to a generated request-rate row beside one it had bound to another
//! plane's row; and default values from 2,000 to 20,000,000.
//!
//! The converse half — that the quotas those two bindings displaced are published under NO
//! code, in particular that no row of the catalog carries the generated
//! `Rate of <Operation> requests` template for an operation whose rate `glue` publishes no
//! code for — is asserted below over the WHOLE catalog rather than over this table, because
//! that is where a re-bound row could reappear.
//!
//! The harness (`client`, `emulator`, `default_quotas_by_code`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas_by_code, emulator, walk_pages};

/// One published row, member for member.
struct Row {
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    description: &'static str,
}

/// The rows this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of `glue` states the quota
/// published under each of these codes, with the name, limit and sentence it carries for it.
const ROWS: &[Row] = &[
    // The account's table versions, under the code the recorded listing binds that plane
    // to: the code this catalog had served the account's concurrent job runs under.
    Row {
        quota_code: "L-337244C9",
        quota_name: "Max table versions per account",
        value: 1_000_000.0,
        description: "The maximum number of table versions in your account.",
    },
    // The account's CONCURRENCY plane, under the code the recorded listing binds it to: the
    // code this catalog had served the generated `UpdateTable` request-rate row under, which
    // is why this row's limit was understated by more than two orders of magnitude.
    Row {
        quota_code: "L-5E4153CA",
        quota_name: "Max concurrent job runs per account",
        value: 2000.0,
        description: "The maximum number of concurrent job runs in your account.",
    },
    // The per-JOB sibling of that count plane — the jobs an account may hold — so the
    // concurrency ceiling above is asserted to be its own row rather than this one's.
    Row {
        quota_code: "L-611FDDE4",
        quota_name: "Max jobs per account",
        value: 2000.0,
        description: "The maximum number of jobs in your account.",
    },
    // The account's PARTITION ceiling, the largest counted plane of this catalog, beside
    // the per-table one below: two codes, two rows, one plane each.
    Row {
        quota_code: "L-FEBBFA7A",
        quota_name: "Max partitions per account",
        value: 20_000_000.0,
        description: "The maximum number of partitions in your account.",
    },
    Row {
        quota_code: "L-2C3F5401",
        quota_name: "Max partitions per table",
        value: 10_000_000.0,
        description: "The maximum number of partitions per table.",
    },
];

/// Operations whose `glue` call rate this catalog had published under a code the recorded
/// listing binds to a counted plane: the rate names no code of the service, so no row of the
/// catalog may carry its generated name or sentence.
///
/// SOURCE: the recorded listing states a counted plane under each of those codes, and states
/// no code for these call rates.
const RATES_PUBLISHED_UNDER_NO_CODE: &[&str] = &["UpdateTable", "GetJob", "GetTableVersions"];

/// Every pinned row carries the quota published under its own code, member for member, in
/// the default listing and in the code-keyed default read alike.
#[tokio::test]
async fn a_glue_account_plane_carries_the_quota_published_under_its_code() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");

    let catalog = default_quotas_by_code(&caller, "glue").await;
    for row in ROWS {
        let case = format!("glue/{}", row.quota_code);
        let listed = catalog.get(row.quota_code).unwrap_or_else(|| {
            panic!(
                "case {case}: expected the code to be published, got the codes {:?}",
                catalog.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            (
                listed["QuotaName"].clone(),
                listed["Value"].as_f64(),
                listed["Description"].clone(),
                listed["Unit"].clone(),
                listed["Adjustable"].clone(),
                listed["GlobalQuota"].clone(),
            ),
            (
                json!(row.quota_name),
                Some(row.value),
                json!(row.description),
                json!("None"),
                json!(true),
                json!(false),
            ),
            "case {case}: expected the quota published under this code, got {listed}"
        );
        assert!(
            listed.get("Period").is_none_or(Value::is_null)
                && listed.get("UsageMetric").is_none_or(Value::is_null),
            "case {case}: a counted plane is published over no window and meters no API call \
             count, got {listed}"
        );

        let read = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": "glue", "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            &read, listed,
            "case {case}: the listing carries the record the single read returns, got {read} \
             and {listed}"
        );
    }
}

/// Each pinned plane is published under exactly ONE code of the catalog, and the account
/// ceiling and its per-parent sibling are never the same row.
#[tokio::test]
async fn each_pinned_glue_plane_is_published_under_one_code() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    let catalog = default_quotas_by_code(&caller, "glue").await;
    for row in ROWS {
        let case = format!("glue/{}", row.quota_code);
        let carrying: Vec<&String> = catalog
            .iter()
            .filter(|(_, quota)| quota["QuotaName"] == json!(row.quota_name))
            .map(|(code, _)| code)
            .collect();
        assert_eq!(
            carrying,
            vec![&row.quota_code.to_string()],
            "case {case}: this plane is published under one code only, got it under \
             {carrying:?}"
        );
        let described: Vec<&String> = catalog
            .iter()
            .filter(|(_, quota)| quota["Description"] == json!(row.description))
            .map(|(code, _)| code)
            .collect();
        assert_eq!(
            described,
            vec![&row.quota_code.to_string()],
            "case {case}: this sentence belongs to one row only, got it under {described:?}"
        );
    }
}

/// No row of the whole `glue` catalog carries the generated request-rate template for an
/// operation whose rate the service publishes no code for — neither as a `QuotaName` nor as
/// a `Description` — and no counted plane is published over a per-second window.
#[tokio::test]
async fn no_glue_row_carries_a_rate_template_for_an_unpublished_rate() {
    let engine = emulator();
    let caller = client(&engine, "ap-southeast-2");

    let catalog = default_quotas_by_code(&caller, "glue").await;
    for api in RATES_PUBLISHED_UNDER_NO_CODE {
        let case = format!("glue/{api}");
        let name = format!("Rate of {api} requests");
        let carrying: Vec<&String> = catalog
            .iter()
            .filter(|(_, quota)| {
                quota["QuotaName"] == json!(name.as_str())
                    || quota["Description"]
                        .as_str()
                        .is_some_and(|sentence| sentence.contains(&format!("{api} requests")))
            })
            .map(|(code, _)| code)
            .collect();
        assert!(
            carrying.is_empty(),
            "case {case}: a call rate the service publishes no code for must be published \
             under no row, got it under {carrying:?}"
        );
    }

    // The same over the applied listing, which a client may walk instead of the default one.
    let applied = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "glue"}),
        "Quotas",
        None,
    )
    .await;
    for api in RATES_PUBLISHED_UNDER_NO_CODE {
        assert!(
            applied
                .iter()
                .all(|quota| quota["QuotaName"] != json!(format!("Rate of {api} requests"))),
            "case glue/{api}: the applied listing publishes the same rows as the default one"
        );
    }
}

/// An increase requested by one of these codes is minted against the quota the service
/// publishes under it, reads back under that code and leaves the published default alone.
#[tokio::test]
async fn an_increase_by_a_rebound_glue_code_targets_that_code_s_quota() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    for row in ROWS {
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
