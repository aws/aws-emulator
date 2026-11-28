// SPDX-License-Identifier: CC0-1.0

//! That the rows of the `cognito-identity`, `comprehend`, `directconnect`, `dms`, `docdb`
//! and `quicksight` catalogs carry AWS's own wording and nothing worded here, over the
//! wire.
//!
//! The rule: a row published under a `QuotaCode` reports the text AWS publishes UNDER THAT
//! CODE — verbatim where this checkout can cite it and NO `Description` member at all
//! where it cannot — so a sentence authored in this emulator is never published, and the
//! row answers the same facts at every endpoint the catalog is served from.
//!
//! Only the two things `recorded_default_catalog_rows.rs` does not already assert live
//! here: the published facts of those six services read at ANOTHER Region's endpoint
//! (through that suite's own `assert_row`, over its own `ROWS` table, so the row data has
//! one home), and the negative rule over each of those services' WHOLE catalog — no row of
//! any of them, named by the tables or not, may carry one of the wordings this catalog used
//! to serve in place of AWS's sentence.

use crate::quota_workflow::{client, default_quotas_by_code, emulator};
use crate::recorded_default_catalog_rows::{assert_row, rows_of};

/// The `ServiceCode`s whose published wording this suite covers.
const COVERED_SERVICE_CODES: &[&str] = &[
    "cognito-identity",
    "comprehend",
    "directconnect",
    "dms",
    "docdb",
    "quicksight",
];

/// The sentences this catalog MUST NOT publish: wordings authored in the emulator that
/// stood where AWS's own text for that quota belongs.
///
/// SOURCE: the recorded default-quota listing of each service code, which reports each of
/// these as a value mismatch against the sentence AWS publishes for the quota — one
/// re-wording per service that dropped a word of AWS's fixed text, one that shortened the
/// sentence to a phrase, and two that paraphrased it.
const RE_WORDED_SENTENCES: &[(&str, &str)] = &[
    (
        "directconnect",
        "The maximum number of virtual private gateways per Direct Connect gateway.",
    ),
    ("docdb", "The maximum number of DB clusters"),
    (
        "quicksight",
        "The maximum total size of the reference documents attached to one chat agent.",
    ),
    (
        "quicksight",
        "The number of days the history of a scheduled task is retained for.",
    ),
];

/// Every published row of these services reports the same facts at a second Region's
/// endpoint, with the `QuotaArn` naming that Region.
#[tokio::test]
async fn a_published_row_reports_the_same_facts_at_another_endpoint() {
    let engine = emulator();
    let region = "ap-northeast-1";
    let caller = client(&engine, region);

    let mut checked = 0_usize;
    for row in rows_of(COVERED_SERVICE_CODES) {
        assert_row(&caller, region, row).await;
        checked += 1;
    }

    assert!(
        checked >= 30,
        "case catalog: the rule must be asserted over the rows these services publish, got \
         {checked}"
    );
}

/// No row of these services publishes a sentence worded in this emulator where AWS
/// publishes its own.
#[tokio::test]
async fn no_published_row_carries_a_sentence_worded_here() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for service_code in COVERED_SERVICE_CODES {
        let listed = default_quotas_by_code(&caller, service_code).await;
        for (code, quota) in &listed {
            let served = quota["Description"].as_str().unwrap_or_default();
            for (re_worded_service, re_worded) in RE_WORDED_SENTENCES {
                assert!(
                    served != *re_worded,
                    "case {service_code}/{code}: expected AWS's own text for the quota, got the \
                     wording {re_worded:?} this catalog used to serve for {re_worded_service}"
                );
            }
        }
    }
}
