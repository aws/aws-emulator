// SPDX-License-Identifier: CC0-1.0

//! Where the type-level roll-up gets the compliance it counts: every compliance item the
//! Region holds, whoever reported it.
//!
//! RULE under test: **`ListComplianceSummaries` "returns a summary count of compliant and
//! non-compliant resources for a compliance type", and the items it counts are the ones the
//! Region its request was signed for holds — the ones Patch Manager GATHERED as much as the
//! ones a caller wrote — so its counts agree with what `ListComplianceItems` reports of those
//! same items, and the compliance of another Region is counted in that Region's roll-up and in
//! no other** (`smithy_query(query="operation",
//! operation="ListComplianceSummaries,ListComplianceItems")`).
//!
//! The suite in `all/compliance_summaries.rs` spans what a type's counts are computed FROM
//! (how many resources and items, which statuses, which severities, the page and the filters);
//! these rows span what the rule generalises over that it does not: WHO reported the items the
//! roll-up is over (this service's own patching run, a caller's `PutComplianceItems`, and both
//! at once) and WHICH Region holds them.
//!
//! Every case goes through the front door a client uses: the shared harness in
//! `all/parameter_store.rs` dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` and
//! an `awsJson1_1` body. Nothing is computed in the test — the patch compliance is gathered by
//! this service's own run of the published patch-baseline document, the written compliance goes
//! in through `PutComplianceItems`, and every expected count is read back out of
//! `ListComplianceItems`. The ONE thing seeded through the engine is the registered managed
//! node, because no call of this service registers one.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::compliance_items::{put, register, MANAGED_INSTANCE};
use crate::parameter_store::{call_in_region, router_with_engine, REGION};

/// A second Region, to pin that a roll-up counts the compliance of the Region its read was
/// signed for. The first is the harness's own [`REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The nodes the cases report compliance about: node ids of the forms the modelled
/// `InstanceId` pattern admits (`^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`).
const FIRST_NODE: &str = "i-0123456789abcdef0";
const SECOND_NODE: &str = "i-0abc1234";

/// The Command document Patch Manager publishes, which every account and Region holds without
/// anybody creating it.
const PATCH_DOCUMENT: &str = "AWS-RunPatchBaseline";

/// The compliance type a patching run reports its patches under
/// (`ComplianceSummaryItem$ComplianceType`: "the compliance type can be Association, Patch, or
/// Custom:string").
const PATCH_TYPE: &str = "Patch";

/// A compliance type of the caller's own namespace, written through the put.
const SCAN_TYPE: &str = "Custom:Scan";

/// The `OperatingSystem` a Linux managed node is patched against here.
const LINUX_OPERATING_SYSTEM: &str = "AMAZON_LINUX_2023";

/// The compliance level the baseline assigns the patches it approves, which is the severity the
/// items it gathers carry.
const COMPLIANCE_LEVEL: &str = "HIGH";

/// The two statuses an item carries (`ComplianceItem$Status`).
const COMPLIANT: &str = "COMPLIANT";
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The page `ListComplianceItems` is read in: the maximum its own range admits
/// (`ListComplianceItemsRequest$MaxResults`, range 1..50).
const ITEM_PAGE: i64 = 50;

/// How many pages of items one case may walk before the walk is called broken, rather than
/// looping for ever on a read that keeps handing out a token.
const MAX_PAGES: usize = 200;

/// The member of a `SeveritySummary` an item of `severity` is counted in: the modelled
/// `ComplianceSeverity` vocabulary (`smithy_query(query="shape", shape="ComplianceSeverity")` —
/// CRITICAL, HIGH, MEDIUM, LOW, INFORMATIONAL, UNSPECIFIED), beside the count member named for
/// each (`smithy_query(query="shape", shape="SeveritySummary")`). A value outside the
/// vocabulary is counted as unspecified, the one level the summary has a member for it in.
fn severity_member(severity: &str) -> &'static str {
    match severity.to_ascii_uppercase().as_str() {
        "CRITICAL" => "CriticalCount",
        "HIGH" => "HighCount",
        "MEDIUM" => "MediumCount",
        "LOW" => "LowCount",
        "INFORMATIONAL" => "InformationalCount",
        _ => "UnspecifiedCount",
    }
}

/// Every `ComplianceSummaryItem` `ListComplianceSummaries` answers `body` with at `region`,
/// which must be served.
async fn summaries(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: &Value,
) -> Vec<Value> {
    let (status, reported) =
        call_in_region(router, region, "ListComplianceSummaries", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the roll-up must be served, got {status} with {reported}"
    );
    reported["ComplianceSummaryItems"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a read reports ComplianceSummaryItems, got {reported}")
        })
        .clone()
}

/// The summary `region` reports for `compliance_type`, or `None` when it reports none.
async fn summary_of(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    compliance_type: &str,
) -> Option<Value> {
    summaries(router, case, region, &json!({}))
        .await
        .into_iter()
        .find(|summary| summary["ComplianceType"].as_str() == Some(compliance_type))
}

/// Every compliance item `region` holds about `node`, walked page by page through
/// `ListComplianceItems` — the read the roll-up must agree with.
async fn every_item_of(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    node: &str,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut body = json!({
            "ResourceIds": [node],
            "ResourceTypes": [MANAGED_INSTANCE],
            "MaxResults": ITEM_PAGE,
        });
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (status, reported) = call_in_region(router, region, "ListComplianceItems", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the item read must be served, got {status} with {reported}"
        );
        items.extend(
            reported["ComplianceItems"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = reported["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            return items;
        }
    }
    panic!("case {case:?}: the item read kept handing out a token after {MAX_PAGES} pages");
}

/// What the ITEMS say one compliance type's summary must be: how many of the resources holding
/// items of it are compliant — a resource is, only while every item it holds of the type is —
/// how many are not, and the `SeveritySummary` member counts of the items under each status.
#[derive(Debug, Default, PartialEq, Eq)]
struct Expected {
    compliant_resources: i64,
    non_compliant_resources: i64,
    compliant_severities: BTreeMap<&'static str, i64>,
    non_compliant_severities: BTreeMap<&'static str, i64>,
}

/// Fold `items` — as `ListComplianceItems` reported them — into what each type's summary must
/// say.
fn expected_of(case: &str, items: &[Value]) -> BTreeMap<String, Expected> {
    let mut folded: BTreeMap<String, Expected> = BTreeMap::new();
    let mut resources: BTreeMap<(String, String), bool> = BTreeMap::new();
    for item in items {
        let compliance_type = item["ComplianceType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an item carries a ComplianceType, got {item}"))
            .to_string();
        let resource_id = item["ResourceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an item carries a ResourceId, got {item}"))
            .to_string();
        let status = item["Status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an item carries a Status, got {item}"));
        let severity = item["Severity"].as_str().unwrap_or("UNSPECIFIED");
        let compliant = status == COMPLIANT;
        let entry = folded.entry(compliance_type.clone()).or_default();
        let counts = if compliant {
            &mut entry.compliant_severities
        } else {
            &mut entry.non_compliant_severities
        };
        *counts.entry(severity_member(severity)).or_default() += 1;
        let held = resources
            .entry((compliance_type, resource_id))
            .or_insert(true);
        *held = *held && compliant;
    }
    for ((compliance_type, _), compliant) in resources {
        let entry = folded.entry(compliance_type).or_default();
        if compliant {
            entry.compliant_resources += 1;
        } else {
            entry.non_compliant_resources += 1;
        }
    }
    folded
}

/// What one reported summary says, in the same shape [`expected_of`] computes.
fn reported_as(case: &str, summary: &Value) -> Expected {
    let count = |half: &str, member: &str| {
        summary[half][member]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected {half}.{member}, got {summary}"))
    };
    let severities = |half: &str| {
        [
            "CriticalCount",
            "HighCount",
            "MediumCount",
            "LowCount",
            "InformationalCount",
            "UnspecifiedCount",
        ]
        .into_iter()
        .filter_map(|member| {
            let counted = summary[half]["SeveritySummary"][member]
                .as_i64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected {half}.{member}, got {summary}")
                });
            (counted != 0).then_some((member, counted))
        })
        .collect::<BTreeMap<&'static str, i64>>()
    };
    Expected {
        compliant_resources: count("CompliantSummary", "CompliantCount"),
        non_compliant_resources: count("NonCompliantSummary", "NonCompliantCount"),
        compliant_severities: severities("CompliantSummary"),
        non_compliant_severities: severities("NonCompliantSummary"),
    }
}

/// Create a baseline of `LINUX_OPERATING_SYSTEM` that approves every patch published for it on
/// the day it was released, and make it the default of that operating system — which is what
/// the nodes of it are patched against — answering the baseline id its create minted.
async fn default_baseline(router: &MultiServiceRouter, case: &str, region: &str) -> String {
    let (status, created) = call_in_region(
        router,
        region,
        "CreatePatchBaseline",
        &json!({
            "Name": "Compliance.Linux",
            "OperatingSystem": LINUX_OPERATING_SYSTEM,
            "ApprovalRules": {
                "PatchRules": [{
                    "PatchFilterGroup": { "PatchFilters": [{ "Key": "PRODUCT", "Values": ["*"] }] },
                    "ApproveAfterDays": 0,
                    "ComplianceLevel": COMPLIANCE_LEVEL,
                    "EnableNonSecurity": true,
                }],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the baseline create must be served, got {status} with {created}"
    );
    let baseline_id = created["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create reports a BaselineId, got {created}"))
        .to_string();
    let (status, registered) = call_in_region(
        router,
        region,
        "RegisterDefaultPatchBaseline",
        &json!({ "BaselineId": baseline_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the default register must be served, got {status} with {registered}"
    );
    baseline_id
}

/// Send the published patch-baseline document to `node` as a scan, which is what makes this
/// service gather the node's patch compliance.
async fn scan(router: &MultiServiceRouter, case: &str, region: &str, node: &str) {
    let (status, sent) = call_in_region(
        router,
        region,
        "SendCommand",
        &json!({
            "DocumentName": PATCH_DOCUMENT,
            "InstanceIds": [node],
            "Parameters": { "Operation": ["Scan"] },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the scan must be accepted, got {status} with {sent}"
    );
}

/// One Region's row: what the row covers, the Region its read is signed for, and the
/// `(CompliantCount, NonCompliantCount)` each compliance type that Region holds items of must
/// be counted with, in the order the roll-up answers them.
type RegionRow<'a> = (&'a str, &'a str, &'a [(&'a str, (i64, i64))]);

/// One item as a put carries it.
fn written(id: &str, severity: &str, status: &str) -> Value {
    json!({ "Id": id, "Severity": severity, "Status": status })
}

/// RULE: the roll-up counts every compliance item the Region holds, whoever reported it — so
/// the `Patch` summary agrees with the patch items the service's own run gathered, a written
/// type is summarised beside it, and neither moves the other's counts.
#[tokio::test]
async fn the_roll_up_counts_the_compliance_the_service_gathered_as_well_as_what_was_written() {
    let (router, engine) = router_with_engine();
    register(&engine, REGION, FIRST_NODE);

    // (what the row covers, the type it is about)
    let case = "the patch compliance one scan gathered";
    default_baseline(&router, case, REGION).await;
    scan(&router, case, REGION, FIRST_NODE).await;

    let gathered = every_item_of(&router, case, REGION, FIRST_NODE).await;
    assert!(
        gathered
            .iter()
            .any(|item| item["ComplianceType"].as_str() == Some(PATCH_TYPE)),
        "case {case:?}: expected the scan's patches to be held as compliance items, got \
         {gathered:?}"
    );
    let expected = expected_of(case, &gathered);
    let patch = summary_of(&router, case, REGION, PATCH_TYPE)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {PATCH_TYPE}"));
    assert_eq!(
        reported_as(case, &patch),
        expected[PATCH_TYPE],
        "case {case:?}: expected the counts the gathered items support {:?}, got {patch}",
        expected[PATCH_TYPE]
    );

    // A type a CALLER wrote about the same node is rolled up beside the gathered one, and
    // leaves its counts where they were: the two sources are items of one store.
    let case = "a written type beside the gathered one";
    let (status, answered) = put(
        &router,
        REGION,
        MANAGED_INSTANCE,
        FIRST_NODE,
        SCAN_TYPE,
        json!([
            written("disk", "LOW", COMPLIANT),
            written("kernel", "CRITICAL", NON_COMPLIANT),
        ]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the put must be served, got {status} with {answered}"
    );
    let held = every_item_of(&router, case, REGION, FIRST_NODE).await;
    let expected = expected_of(case, &held);
    let reported = summaries(&router, case, REGION, &json!({})).await;
    let types: Vec<&str> = reported
        .iter()
        .filter_map(|summary| summary["ComplianceType"].as_str())
        .collect();
    assert_eq!(
        types,
        vec![SCAN_TYPE, PATCH_TYPE],
        "case {case:?}: expected both types rolled up, in ascending order, got {types:?}"
    );
    for summary in &reported {
        let compliance_type = summary["ComplianceType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a summary names its type, got {summary}"));
        assert_eq!(
            reported_as(case, summary),
            expected[compliance_type],
            "case {case:?}: expected {compliance_type} to count the items it holds {:?}, got \
             {summary}",
            expected[compliance_type]
        );
    }

    // And a filter narrows the roll-up over the GATHERED items the same way: the type the
    // filter names is the only one summarised.
    let case = "a filter on the gathered type";
    let narrowed = summaries(
        &router,
        case,
        REGION,
        &json!({"Filters": [{ "Key": "ComplianceType", "Values": [PATCH_TYPE] }]}),
    )
    .await;
    let types: Vec<&str> = narrowed
        .iter()
        .filter_map(|summary| summary["ComplianceType"].as_str())
        .collect();
    assert_eq!(
        types,
        vec![PATCH_TYPE],
        "case {case:?}: expected only the filtered type, got {types:?}"
    );
    assert_eq!(
        reported_as(case, &narrowed[0]),
        expected[PATCH_TYPE],
        "case {case:?}: expected the filtered type's own counts {:?}, got {}",
        expected[PATCH_TYPE],
        narrowed[0]
    );
}

/// RULE: a roll-up counts the compliance of the Region its read was signed for, so the same
/// items are counted in that Region's summaries and in no other.
#[tokio::test]
async fn a_roll_up_counts_only_the_compliance_of_the_region_it_was_signed_for() {
    let (router, engine) = router_with_engine();
    // The same node id registered in both Regions, and a second node in one of them: a node is
    // held per Region, so what each Region holds about a node is its own.
    register(&engine, REGION, FIRST_NODE);
    register(&engine, OTHER_REGION, FIRST_NODE);
    register(&engine, OTHER_REGION, SECOND_NODE);

    // (the Region, the node, the type, the items written about it there)
    let rows: &[(&str, &str, &str, &str)] = &[
        (REGION, FIRST_NODE, SCAN_TYPE, COMPLIANT),
        (OTHER_REGION, FIRST_NODE, SCAN_TYPE, NON_COMPLIANT),
        (OTHER_REGION, SECOND_NODE, "Custom:Audit", NON_COMPLIANT),
    ];
    for (region, node, compliance_type, status) in rows {
        let (code, answered) = put(
            &router,
            region,
            MANAGED_INSTANCE,
            node,
            compliance_type,
            json!([written("probe", "MEDIUM", status)]),
        )
        .await;
        assert_eq!(
            code, 200,
            "the setup put of {compliance_type} about {node} at {region} must be served, got \
             {code} with {answered}"
        );
    }

    // (what the row covers, the Region, what its roll-up must report per type)
    let cases: &[RegionRow<'_>] = &[
        (
            "the Region holding one compliant resource of one type",
            REGION,
            &[(SCAN_TYPE, (1, 0))],
        ),
        (
            "the Region holding items about two resources, of two types",
            OTHER_REGION,
            &[("Custom:Audit", (0, 1)), (SCAN_TYPE, (0, 1))],
        ),
    ];
    for (case, region, expected) in cases {
        let reported = summaries(&router, case, region, &json!({})).await;
        let counted: Vec<(&str, (i64, i64))> = reported
            .iter()
            .map(|summary| {
                let as_reported = reported_as(case, summary);
                (
                    summary["ComplianceType"].as_str().unwrap_or_default(),
                    (
                        as_reported.compliant_resources,
                        as_reported.non_compliant_resources,
                    ),
                )
            })
            .collect();
        assert_eq!(
            counted,
            expected.to_vec(),
            "case {case:?}: expected {region} to count only its own items {expected:?}, got \
             {counted:?}"
        );
    }

    // A type only the other Region holds items of is reported by no roll-up of this one, not
    // even as an empty summary: the Region holds no item of it to count.
    let case = "a type only the other Region holds";
    assert!(
        summary_of(&router, case, REGION, "Custom:Audit")
            .await
            .is_none(),
        "case {case:?}: expected {REGION} to report no summary for a type it holds no item of"
    );
}
