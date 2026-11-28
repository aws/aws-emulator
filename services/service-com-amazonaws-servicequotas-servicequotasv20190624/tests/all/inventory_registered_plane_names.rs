// SPDX-License-Identifier: CC0-1.0

//! That every `ServiceCode` the integrated-service inventory registers is LISTED and
//! RESOLVES — including the codes whose registered display name no source available here
//! states.
//!
//! THE RULE, stated over the inventory rather than over one entry: `ListServices` reports
//! one `ServiceInfo` per `ServiceCode` Service Quotas registers, ordered by `ServiceCode`,
//! and an entry's `ServiceName` is the display name registered for THAT code — never the
//! `ServiceCode` echoed back where a display name can be cited at all
//! (`smithy_query(query="shape", shape="ServiceInfo")`: `ServiceInfo{ServiceCode @length
//! 1..63 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$, ServiceName}`;
//! `smithy_query(query="operation", operation="ListServices")`: output `{NextToken,
//! Services: ServiceInfoListDefinition}`). The model enumerates no catalog, so which
//! codes exist is AWS's registered fact, carried by `src/state.rs::SEED_SERVICE_CATALOG`.
//!
//! What this suite owns, and what it does NOT: `inventory_name_as_registered.rs` owns the
//! registered NAME of an entry (and the `inventory` walk both suites read the listing
//! through), `integrated_service_display_names.rs` the shapes a name takes, and
//! `inventory_registered_display_names.rs` discovery through paging. This suite owns
//! MEMBERSHIP: that a registered code is enumerated once and resolves its quota listings,
//! asserted over the codes a client cannot reach at all when the catalog drops them —
//! including those whose registered name is stated by nothing available here, where the
//! name cannot be pinned as a value but the code's presence can.
//!
//! The harness (`client`, `emulator`, `service_inventory`) is the one `quota_workflow.rs`
//! owns; the inventory walk is the one `inventory_name_as_registered.rs` owns.

use serde_json::json;

use crate::inventory_name_as_registered::inventory;
use crate::quota_workflow::{client, emulator, service_inventory};

/// The registered codes this suite pins the presence of.
///
/// SOURCE: AWS's own integrated-service inventory lists each of these codes (the
/// `ServiceInfo.ServiceCode` values `aws service-quotas list-services` reports). They are
/// the codes the report behind this suite names as missing from the catalog, spanning what
/// a code looks like: one word, a hyphenated plane, a hyphenated plane of a hyphenated
/// product. For the last four the registered display name is stated by no source available
/// here, so only the code-side half of the rule is asserted for every row: a name that is
/// not the code echoed back, rather than one pinned string.
const REGISTERED_CODES: &[&str] = &[
    "migrationhubstrategy",
    "networkinsights",
    "observabilityadmin",
    "opensearch",
    "qbusiness",
    "rolesanywhere",
    "pca-connector-ad",
    "pca-connector-scep",
    "route53-recovery-control-config",
    "route53-recovery-readiness",
];

/// Every row is enumerated once, at its place in the one ascending listing, under a
/// display name of its own — and the code a client discovers there resolves both quota
/// listings, so a code the catalog would have dropped is reachable rather than merely
/// present.
#[tokio::test]
async fn a_registered_code_is_enumerated_once_and_resolves() {
    let engine = emulator();
    let client = client(&engine, "ap-northeast-1");

    let listed = service_inventory(&client).await;
    let codes: Vec<&str> = listed
        .iter()
        .filter_map(|service| service["ServiceCode"].as_str())
        .collect();
    for pair in codes.windows(2) {
        assert!(
            pair[0] < pair[1],
            "case ListServices: the inventory is ordered by ServiceCode, got {:?} before {:?}",
            pair[0],
            pair[1]
        );
    }

    let names = inventory(&client).await;
    for code in REGISTERED_CODES {
        let held: Vec<&str> = codes.iter().copied().filter(|held| held == code).collect();
        assert_eq!(
            held.len(),
            1,
            "case {code}: expected the inventory to enumerate this registered code once, got \
             {held:?} in {} entries",
            codes.len()
        );

        let name = names
            .get(*code)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("case {code}: the inventory must list this code"));
        assert!(
            !name.is_empty() && name != *code,
            "case {code}: expected a display name of its own, got {name:?}"
        );

        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let response = client.ok(operation, json!({"ServiceCode": code})).await;
            assert!(
                response["Quotas"].is_array(),
                "case {code} {operation}: a listed code must resolve its listing, got {response}"
            );
        }
    }
}
