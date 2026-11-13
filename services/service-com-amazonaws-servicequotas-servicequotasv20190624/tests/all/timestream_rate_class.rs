// SPDX-License-Identifier: CC0-1.0

//! What the default-quota catalog publishes for Amazon Timestream's per-API REQUEST-RATE
//! quotas, and under which `QuotaCode` it publishes each of them.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same router the emulator serves
//! with, over ONE shared engine, so what is exercised is the wire behaviour.
//!
//! Three rules are pinned, each over the CLASS of rows rather than over one quota:
//!
//! * Timestream publishes a quota that throttles ONE of its APIs under the name
//!   `<Api> request rate`, described by the sentence of that class with the API named in
//!   it, in `Unit` `None`, NOT adjustable, counted per Region, over a one-second window,
//!   and metered by the `AWS/Usage` `CallCount` of that API under the `Timestream`
//!   service dimension. Every member is DERIVED here from the row's own `QuotaName`, so
//!   however many rows of the class the catalog publishes, each one is a case and a row
//!   that borrows a neighbouring quota's text, window or metric fails;
//! * each such row is published under the `QuotaCode` AWS publishes THAT API's rate
//!   under, and the same code resolves the same quota through `GetAWSDefaultServiceQuota`
//!   and in the applied plane, so a client that reads a code out of the listing resolves
//!   it everywhere;
//! * a code this catalog cannot attribute to a quota of this service resolves NOWHERE,
//!   which is what keeps a row from being served under another quota's code.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns; `api_rate_quota_rows.rs` pins the same shape of rule for the service whose rate
//! rows are published under the `Rate of <Api> API requests` wording, which is a
//! different class from this one.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas, emulator};

/// The `ServiceCode` whose rate class this suite is about.
const SERVICE_CODE: &str = "timestream";

/// The `AWS/Usage` display name Timestream's call counts are published under, which is
/// the `Service` dimension of every metered row of the class.
const USAGE_SERVICE: &str = "Timestream";

/// How many rows of the class the catalog must publish for the derived rule to have been
/// asserted over a class rather than over one row.
const RATE_ROWS_EXPECTED: usize = 10;

/// One published rate row: the API it throttles and the requests per second it is
/// published at, under the code AWS publishes that API's rate under.
struct RateRow {
    quota_code: &'static str,
    api: &'static str,
    per_second: f64,
}

/// The rate rows this suite names, spanning both rates the catalog publishes (one and ten
/// per second beside the five of a batch-load listing), a write API, a read API and the
/// scheduled-query and batch-load planes.
///
/// SOURCE: the recorded default-quota listing of `timestream` for the three codes it
/// names (`L-F07A9AB4` = `ListBatchLoadTasks` at 5, `L-798CB19D` = `CreateScheduledQuery`
/// at 1, `L-C49D398A` = `CreateBatchLoadTask`), and AWS's published Service Quotas data
/// for the `timestream` service code for the rest
/// (`src/state/quota_rows/timestream.rs`). Two of the five rows name quotas no reported
/// instance cited.
const RATE_ROWS: &[RateRow] = &[
    RateRow {
        quota_code: "L-F07A9AB4",
        api: "ListBatchLoadTasks",
        per_second: 5.0,
    },
    RateRow {
        quota_code: "L-798CB19D",
        api: "CreateScheduledQuery",
        per_second: 1.0,
    },
    RateRow {
        quota_code: "L-C49D398A",
        api: "CreateBatchLoadTask",
        per_second: 1.0,
    },
    RateRow {
        quota_code: "L-05CF2F29",
        api: "CancelQuery",
        per_second: 10.0,
    },
    RateRow {
        quota_code: "L-FD5A0C1A",
        api: "UpdateTable",
        per_second: 1.0,
    },
];

/// Codes in AWS's minted namespace that this catalog attributes to no Timestream quota:
/// the two it used to key the scheduled-query and batch-load creation rates on, which the
/// recorded listing publishes those rates under other codes.
const UNATTRIBUTED_CODES: &[&str] = &["L-0C6E4BCB", "L-16B72159"];

/// The API a `QuotaName` of this service's rate class throttles, or `None` for a name
/// outside the class.
///
/// The class is `<Api> request rate`, where `<Api>` is ONE API operation's own name — a
/// single word in the casing an operation is named in — so a name whose first word is
/// prose does not read as a rate row.
fn rated_api(quota_name: &str) -> Option<&str> {
    let api = quota_name.strip_suffix(" request rate")?;
    let names_one_operation = !api.is_empty()
        && !api.contains(char::is_whitespace)
        && api.starts_with(char::is_uppercase);
    names_one_operation.then_some(api)
}

/// The `UsageMetric` the class publishes for the rate of `api`.
fn call_count_of(api: &str) -> Value {
    json!({
        "MetricNamespace": "AWS/Usage",
        "MetricName": "CallCount",
        "MetricStatisticRecommendation": "Sum",
        "MetricDimensions": {
            "Service": USAGE_SERVICE,
            "Class": "None",
            "Resource": api,
            "Type": "API",
        },
    })
}

/// RULE: every row of this service's rate class publishes the name, sentence, unit,
/// adjustability, window and `AWS/Usage` call count of the API IT throttles — each
/// derived from the row's own name, so every published row of the class is a case.
#[tokio::test]
async fn every_timestream_rate_row_publishes_its_own_apis_rate() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    let listed = default_quotas(&caller, SERVICE_CODE).await;
    let mut rate_rows = 0_usize;

    for quota in &listed {
        let name = quota["QuotaName"].as_str().unwrap_or_default();
        let code = quota["QuotaCode"].as_str().unwrap_or_default();
        let case = format!("{SERVICE_CODE}/{code} ({name:?})");

        let Some(api) = rated_api(name) else {
            // Every other row: it must not borrow the sentence of an API it does not
            // throttle, which is the shape of the defect.
            let described = quota["Description"].as_str().unwrap_or_default();
            for other in &listed {
                let Some(other_api) = other["QuotaName"].as_str().and_then(rated_api) else {
                    continue;
                };
                let borrowed = format!("Maximum number of {other_api} requests per second.");
                assert_ne!(
                    described, borrowed,
                    "case {case}: a row must not publish another quota's sentence, got \
                     {described:?}"
                );
            }
            continue;
        };

        rate_rows += 1;
        let expected = format!("Maximum number of {api} requests per second.");
        assert_eq!(
            quota["Description"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected the sentence of the class, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!("None"),
            "case {case}: a rate is published as a bare count of requests, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(false),
            "case {case}: this service publishes its API rates as not adjustable, got {quota}"
        );
        assert_eq!(
            quota["GlobalQuota"],
            json!(false),
            "case {case}: an API rate is served per Region, got {quota}"
        );
        assert_eq!(
            quota["Period"],
            json!({"PeriodValue": 1, "PeriodUnit": "SECOND"}),
            "case {case}: a rate is published over the one-second window, got {quota}"
        );
        assert_eq!(
            quota["UsageMetric"],
            call_count_of(api),
            "case {case}: expected the AWS/Usage call count of this API, got {quota}"
        );
        let value = quota["Value"].as_f64().unwrap_or_default();
        assert!(
            value > 0.0,
            "case {case}: a published rate is a positive number of requests per second, got \
             {value}"
        );
    }

    assert!(
        rate_rows >= RATE_ROWS_EXPECTED,
        "case {SERVICE_CODE}: the rule must be asserted over the class — expected at least \
         {RATE_ROWS_EXPECTED} rate rows, got {rate_rows}"
    );
}

/// RULE: a named rate row is published under the code AWS publishes THAT API's rate
/// under, at the rate AWS publishes, and the same code resolves the same quota through
/// the single default read and in the applied plane.
#[tokio::test]
async fn a_named_rate_row_resolves_by_its_own_code_everywhere() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);

    let listed = default_quotas(&caller, SERVICE_CODE).await;

    for row in RATE_ROWS {
        let case = format!("{SERVICE_CODE}/{}", row.quota_code);
        let pair = json!({"ServiceCode": SERVICE_CODE, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            (
                quota["QuotaName"].clone(),
                quota["Value"].clone(),
                quota["Description"].clone()
            ),
            (
                json!(format!("{} request rate", row.api)),
                json!(row.per_second),
                json!(format!("Maximum number of {} requests per second.", row.api))
            ),
            "case {case}: expected the rate AWS publishes under this code, got {quota}"
        );
        assert_eq!(
            quota["UsageMetric"],
            call_count_of(row.api),
            "case {case}: expected the AWS/Usage call count of this API, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{SERVICE_CODE}/{}",
                row.quota_code
            )),
            "case {case}: a default quota's ARN names the Region and no account, got {quota}"
        );
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {} rows",
            listed.len()
        );

        // The applied plane is keyed on the same code, so a client that read the code out
        // of the default listing resolves its value in force too.
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (
                applied["QuotaCode"].clone(),
                applied["QuotaName"].clone(),
                applied["Value"].clone()
            ),
            (
                json!(row.quota_code),
                json!(format!("{} request rate", row.api)),
                json!(row.per_second)
            ),
            "case {case}: the applied read resolves the same quota at its default value, got \
             {applied}"
        );
    }
}

/// RULE: a code this catalog cannot attribute to a quota of this service is published
/// nowhere and resolves nowhere, while the service itself still resolves its listing.
#[tokio::test]
async fn an_unattributed_code_resolves_nowhere() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let listed = default_quotas(&caller, SERVICE_CODE).await;
    let codes: Vec<&str> = listed
        .iter()
        .filter_map(|quota| quota["QuotaCode"].as_str())
        .collect();

    for unattributed in UNATTRIBUTED_CODES {
        let case = format!("{SERVICE_CODE}/{unattributed}");
        assert!(
            !codes.contains(unattributed),
            "case {case}: a code this catalog attributes to no quota of the service must not \
             be published, got {codes:?}"
        );

        for operation in [
            "GetAWSDefaultServiceQuota",
            "GetServiceQuota",
            "RequestServiceQuotaIncrease",
        ] {
            let mut body = json!({"ServiceCode": SERVICE_CODE, "QuotaCode": unattributed});
            if operation == "RequestServiceQuotaIncrease" {
                body["DesiredValue"] = json!(1_000.0);
            }
            let (status, error) = caller.refused(operation, body).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case}: {operation} must refuse a code this service publishes nothing \
                 under"
            );
        }
    }

    assert!(
        !listed.is_empty(),
        "case {SERVICE_CODE}: the service code must still resolve its published listing"
    );
}
