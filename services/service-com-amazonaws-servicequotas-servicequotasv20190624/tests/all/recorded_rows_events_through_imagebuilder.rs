// SPDX-License-Identifier: CC0-1.0

//! The published TEXT of a default-quota row of these ten catalogs: it belongs to that
//! row's own quota, and a row whose published text no source states carries none.
//!
//! THE RULE: a row published under a `QuotaCode` carries the `QuotaName`, `Value`,
//! `Unit`, `Adjustable`, `Period`, `UsageMetric` and `Description` of the quota the
//! service publishes UNDER THAT CODE — the description as the service words it, never a
//! paraphrase and never a neighbouring quota's. Two consequences of the `Description`
//! half are asserted here, and neither is reachable from a per-row table (the per-row
//! facts live in `aws_quota_metadata_per_code.rs`, the per-catalog code and name
//! invariants in `aws_minted_catalog_coverage.rs`):
//!
//! * inside one `ServiceCode` no two rows carry the SAME `Description`. A description is
//!   the sentence the service publishes for one quota, so a sentence appearing twice means
//!   one of the two rows is describing the other's quota — the borrowed-text half of this
//!   catalog's defect, which a name or code check cannot see;
//! * the rows whose published sentence no source available here states carry NO
//!   `Description` at all, rather than one the catalog words itself. The pairs are named
//!   below, so a sentence invented for one of them later fails this test, and a row
//!   outside the list that loses its description fails it too.
//!
//! The walk spans the ten `ServiceCode`s this change re-keyed and reads them over the
//! wire through `ListAWSDefaultServiceQuotas` (paged by the shared harness), so both
//! invariants are asserted on the served response rather than on the seed table.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::quota_workflow::{client, default_quotas, emulator, Client};

/// The `ServiceCode`s these invariants are walked over.
const SERVICE_CODES: &[&str] = &[
    "events",
    "firehose",
    "fms",
    "fsx",
    "gamelift",
    "glacier",
    "glue",
    "groundstation",
    "guardduty",
    "imagebuilder",
];

/// The `(ServiceCode, QuotaCode)` pairs published WITHOUT a `Description`.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each service states which
/// quota some of its codes publish without stating every member of those rows. For the
/// account's version 2 EVENT SOURCES (`events`), the VPCs one Network Firewall policy may
/// remediate automatically (`fms`) and the locations one fleet may have in a Region
/// (`gamelift`) it states the quota's name, limit and adjustability while stating no
/// sentence, so each of those rows carries none rather than a sentence this catalog would be
/// wording itself, which is what the rule forbids.
///
/// One `glue` code is the same case reached from the other side: no source states the quota
/// it publishes, so it keeps the row it has carried — the account's security configurations
/// under `L-4071B0E3`, whose sentence belongs to the quota `L-83192DBF` publishes and is
/// therefore published there rather than twice.
///
/// `pub(crate)` so the suites that assert "every row carries its published sentence"
/// read the one list rather than each keeping a copy of it.
pub(crate) const PUBLISHED_WITHOUT_A_DESCRIPTION: &[(&str, &str)] = &[
    ("events", "L-D082A4F6"),
    ("fms", "L-BCA8AD34"),
    ("gamelift", "L-55650DB7"),
    ("glue", "L-4071B0E3"),
];

/// Whether the row `service_code` publishes under `quota_code` is one whose own published
/// sentence no source available here states — the question the per-service suites ask of
/// [`PUBLISHED_WITHOUT_A_DESCRIPTION`], answered here so they share the one list AND the one
/// lookup over it.
pub(crate) fn sentence_is_unsourced(service_code: &str, quota_code: &str) -> bool {
    PUBLISHED_WITHOUT_A_DESCRIPTION.contains(&(service_code, quota_code))
}

/// Every row of these catalogs, as `ServiceCode -> the listed rows`.
async fn catalogs(caller: &Client) -> BTreeMap<&'static str, Vec<Value>> {
    let mut catalogs = BTreeMap::new();
    for service_code in SERVICE_CODES {
        let listed = default_quotas(caller, service_code).await;
        assert!(
            !listed.is_empty(),
            "case {service_code}: the service publishes a default-quota catalog, got none"
        );
        catalogs.insert(*service_code, listed);
    }
    catalogs
}

/// A published `Description` belongs to ONE quota of a service: no two rows of one
/// catalog carry the same sentence.
#[tokio::test]
async fn a_published_description_belongs_to_one_row_of_its_catalog() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    let mut described = 0_usize;
    for (service_code, listed) in catalogs(&caller).await {
        let mut by_description: BTreeMap<String, String> = BTreeMap::new();
        for quota in &listed {
            let code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a row without a code: {quota}"))
                .to_string();
            let Some(description) = quota["Description"].as_str() else {
                continue;
            };
            if let Some(first) = by_description.insert(description.to_string(), code.clone()) {
                panic!(
                    "case {service_code}/{code}: a published description belongs to one quota, \
                     got {description:?} under {code} and under {first}"
                );
            }
            described += 1;
        }
    }

    assert!(
        described > 150,
        "case catalog: the rule must be asserted over the described rows of these catalogs \
         (which hold well over 150 of them), got {described} rows"
    );
}

/// A row whose published sentence no source states carries NO `Description`, and every
/// other row carries one.
#[tokio::test]
async fn a_row_with_no_stated_sentence_is_published_without_a_description() {
    let engine = emulator();
    let caller = client(&engine, "ap-southeast-2");

    let mut undescribed: Vec<(&str, String)> = Vec::new();
    for (service_code, listed) in catalogs(&caller).await {
        for quota in &listed {
            let code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a row without a code: {quota}"))
                .to_string();
            let case = format!("{service_code}/{code}");
            let stated = !PUBLISHED_WITHOUT_A_DESCRIPTION
                .iter()
                .any(|(service, quota_code)| *service == service_code && *quota_code == code);
            let described = quota["Description"]
                .as_str()
                .is_some_and(|description| !description.is_empty());
            assert_eq!(
                described, stated,
                "case {case}: a row carries the sentence its service publishes for it, and \
                 none where no source states one, got {quota}"
            );
            if !described {
                undescribed.push((service_code, code));
            }
        }
    }

    assert_eq!(
        undescribed.len(),
        PUBLISHED_WITHOUT_A_DESCRIPTION.len(),
        "case catalog: exactly the pairs no source states a sentence for are published \
         without one, got {undescribed:?}"
    );
}
