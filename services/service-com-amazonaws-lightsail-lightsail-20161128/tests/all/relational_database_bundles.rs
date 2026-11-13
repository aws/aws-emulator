// SPDX-License-Identifier: CC0-1.0

//! `GetRelationalDatabaseBundles` — the managed-database bundle catalog, driven
//! over the wire.
//!
//! Every wire case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRelationalDatabaseBundles` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over the WHOLE catalog, over every form the
//! `includeInactive` flag takes and over a SET of page tokens rather than one
//! recorded call:
//!
//! * the operation reports the service's published database plan catalog — every
//!   plan whose `isActive` is true, plus the retired ones when `includeInactive`
//!   is set — each entry carrying the vCPU count, RAM, disk size, included
//!   monthly data transfer, encryption flag and monthly price that catalog
//!   records for it;
//! * one page is that slice from the entry `pageToken` names onward, bounded by
//!   the published page size, with `nextPageToken` naming the first entry of the
//!   next page and present only while entries remain — so the walk reports the
//!   slice in order, exactly once each;
//! * a `pageToken` that names no bundle of the slice is a modelled 400
//!   `InvalidInputException`; and
//! * reporting the catalog is a read: identical calls answer identically and
//!   nothing else a caller can see changes.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    relational_database_bundle_page, relational_database_bundle_spec, RelationalDatabaseBundleSpec,
    RELATIONAL_DATABASE_BUNDLE_PAGE_SIZE, RELATIONAL_DATABASE_BUNDLE_SPECS,
};

use crate::instance_lifecycle::Emulator;

/// The operation under test.
const OPERATION: &str = "GetRelationalDatabaseBundles";

/// Every member the modelled `RelationalDatabaseBundle` structure declares
/// (`smithy_query(query="shape",shape="RelationalDatabaseBundle")`), in wire
/// spelling. A reported bundle carries exactly these and nothing else.
const BUNDLE_MEMBERS: &[&str] = &[
    "bundleId",
    "cpuCount",
    "diskSizeInGb",
    "isActive",
    "isEncrypted",
    "name",
    "price",
    "ramSizeInGb",
    "transferPerMonthInGb",
];

/// One published Lightsail managed-database plan.
///
/// SOURCE: the managed-database plan table of the Lightsail pricing page — a
/// standard plan at $15 a month with 1 GB RAM, 2 vCPUs and 40 GB of SSD storage,
/// $30 with 2 GB / 2 vCPUs / 80 GB, $60 with 4 GB / 2 vCPUs / 120 GB, $115 with
/// 8 GB / 2 vCPUs / 240 GB, $245 with 16 GB / 4 vCPUs / 480 GB and $490 with
/// 32 GB / 8 vCPUs / 960 GB, 100 GB of included data transfer on the three
/// smallest plans, 200 GB on the 8 GB plan, 300 GB on the 16 GB one and 400 GB
/// on the largest, each price the table's own figure for that plan rather than a
/// step computed from the plan below it — beside
/// the high-availability column of the same table, which sells the same hardware
/// with a standby in a second Availability Zone for twice the standard price. The plans are the `2_0` generation the service publishes;
/// the superseded `1_0` generation is retired, so it is not reported unless a
/// call asks for inactive plans. Written out here independently of the service's
/// own `RELATIONAL_DATABASE_BUNDLE_SPECS`, so these rows pin the published
/// figures rather than restating whatever the code happens to hold.
struct Plan {
    /// The standard (single-database) plan's bundle id.
    bundle_id: &'static str,
    /// The same plan with a standby: the id carries an `ha` token after the size.
    ha_bundle_id: &'static str,
    name: &'static str,
    ha_name: &'static str,
    cpu_count: i64,
    ram_size_in_gb: f64,
    disk_size_in_gb: i64,
    transfer_per_month_in_gb: i64,
    price: f64,
}

const PUBLISHED_PLANS: &[Plan] = &[
    Plan {
        bundle_id: "micro_2_0",
        ha_bundle_id: "micro_ha_2_0",
        name: "Micro",
        ha_name: "Micro with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price: 15.0,
    },
    Plan {
        bundle_id: "small_2_0",
        ha_bundle_id: "small_ha_2_0",
        name: "Small",
        ha_name: "Small with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 100,
        price: 30.0,
    },
    Plan {
        bundle_id: "medium_2_0",
        ha_bundle_id: "medium_ha_2_0",
        name: "Medium",
        ha_name: "Medium with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 120,
        transfer_per_month_in_gb: 100,
        price: 60.0,
    },
    Plan {
        bundle_id: "large_2_0",
        ha_bundle_id: "large_ha_2_0",
        name: "Large",
        ha_name: "Large with High Availability",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 240,
        transfer_per_month_in_gb: 200,
        price: 115.0,
    },
    Plan {
        bundle_id: "xlarge_2_0",
        ha_bundle_id: "xlarge_ha_2_0",
        name: "Xlarge",
        ha_name: "Xlarge with High Availability",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 480,
        transfer_per_month_in_gb: 300,
        price: 245.0,
    },
    Plan {
        bundle_id: "2xlarge_2_0",
        ha_bundle_id: "2xlarge_ha_2_0",
        name: "2Xlarge",
        ha_name: "2Xlarge with High Availability",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 960,
        transfer_per_month_in_gb: 400,
        price: 490.0,
    },
];

/// Whether `actual` is `expected` to the cent (a `float` member on the wire).
fn is_close(actual: Option<f64>, expected: f64) -> bool {
    actual.is_some_and(|actual| (actual - expected).abs() < 0.005)
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// One page of the catalog: the page's bundles and its `nextPageToken`.
async fn page(emulator: &Emulator, request: Value) -> (Vec<Value>, Option<String>) {
    let case = request.clone();
    let body = emulator.ok(OPERATION, request).await;
    let bundles = body["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a bundles array, got {body:?}"))
        .clone();
    let next = body["nextPageToken"].as_str().map(str::to_string);
    (bundles, next)
}

/// Every bundle the walk from `request` reports, in page order, together with the
/// size of each page it took.
async fn walk(emulator: &Emulator, request: Value) -> (Vec<Value>, Vec<usize>) {
    let mut bundles = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let (this_page, next) = page(emulator, body).await;
        page_sizes.push(this_page.len());
        bundles.extend(this_page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case {request:?}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (bundles, page_sizes)
}

/// The bundles the walk from `request` reports, keyed by `bundleId`.
async fn reported(emulator: &Emulator, request: Value) -> BTreeMap<String, Value> {
    let case = request.clone();
    let (bundles, _) = walk(emulator, request).await;
    let mut keyed = BTreeMap::new();
    for bundle in bundles {
        let id = bundle["bundleId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: every bundle carries a bundleId, got {bundle:?}")
            })
            .to_string();
        assert!(
            keyed.insert(id.clone(), bundle).is_none(),
            "case {case:?}: bundleId {id:?} was reported twice"
        );
    }
    keyed
}

/// Every published plan — standard and high-availability — comes back with the
/// hardware, included transfer and price the catalog publishes for it, and
/// nothing else does.
#[tokio::test]
async fn every_published_plan_is_reported_with_its_own_specs_and_price() {
    let emulator = Emulator::new();
    let bundles = reported(&emulator, json!({})).await;

    // Each plan contributes two rows: the standard bundle, and the
    // high-availability bundle with the same hardware at twice the price.
    for plan in PUBLISHED_PLANS {
        for (bundle_id, name, price) in [
            (plan.bundle_id, plan.name, plan.price),
            (plan.ha_bundle_id, plan.ha_name, plan.price * 2.0),
        ] {
            let bundle = bundles.get(bundle_id).unwrap_or_else(|| {
                panic!(
                    "case {bundle_id}: expected this published plan to be reported, got {:?}",
                    bundles.keys().collect::<Vec<_>>()
                )
            });

            assert_eq!(
                bundle["name"].as_str(),
                Some(name),
                "case {bundle_id}: expected the plan's published label as its name, got {:?}",
                bundle["name"]
            );
            assert_eq!(
                bundle["cpuCount"].as_i64(),
                Some(plan.cpu_count),
                "case {bundle_id}: expected cpuCount {}, got {:?}",
                plan.cpu_count,
                bundle["cpuCount"]
            );
            assert!(
                is_close(bundle["ramSizeInGb"].as_f64(), plan.ram_size_in_gb),
                "case {bundle_id}: expected ramSizeInGb {}, got {:?}",
                plan.ram_size_in_gb,
                bundle["ramSizeInGb"]
            );
            assert_eq!(
                bundle["diskSizeInGb"].as_i64(),
                Some(plan.disk_size_in_gb),
                "case {bundle_id}: expected diskSizeInGb {}, got {:?}",
                plan.disk_size_in_gb,
                bundle["diskSizeInGb"]
            );
            assert_eq!(
                bundle["transferPerMonthInGb"].as_i64(),
                Some(plan.transfer_per_month_in_gb),
                "case {bundle_id}: expected transferPerMonthInGb {}, got {:?}",
                plan.transfer_per_month_in_gb,
                bundle["transferPerMonthInGb"]
            );
            assert!(
                is_close(bundle["price"].as_f64(), price),
                "case {bundle_id}: expected price {price}, got {:?}",
                bundle["price"]
            );
            // Lightsail encrypts managed-database storage at rest on every plan.
            assert_eq!(
                bundle["isEncrypted"].as_bool(),
                Some(true),
                "case {bundle_id}: a managed database is encrypted at rest, got {:?}",
                bundle["isEncrypted"]
            );
            assert_eq!(
                bundle["isActive"].as_bool(),
                Some(true),
                "case {bundle_id}: a plan that is still offered is active, got {:?}",
                bundle["isActive"]
            );

            // Exactly the modelled members, so nothing is reported that the shape
            // does not declare and nothing it declares is left out.
            let members: BTreeSet<&str> = bundle
                .as_object()
                .unwrap_or_else(|| panic!("case {bundle_id}: a bundle is an object, got {bundle:?}"))
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                members,
                BUNDLE_MEMBERS.iter().copied().collect::<BTreeSet<_>>(),
                "case {bundle_id}: expected exactly the modelled RelationalDatabaseBundle members, got {members:?}"
            );

            // A reported id resolves in the catalog, and resolves to the figures
            // that were reported.
            let spec = relational_database_bundle_spec(bundle_id).unwrap_or_else(|| {
                panic!("case {bundle_id}: a reported id must resolve in the catalog")
            });
            assert_eq!(
                i64::from(spec.disk_size_in_gb),
                plan.disk_size_in_gb,
                "case {bundle_id}: the catalog lookup must answer the published disk size, got {spec:?}"
            );
        }
    }

    let reported_ids: BTreeSet<&str> = bundles.keys().map(String::as_str).collect();
    let published_ids: BTreeSet<&str> = PUBLISHED_PLANS
        .iter()
        .flat_map(|plan| [plan.bundle_id, plan.ha_bundle_id])
        .collect();
    assert_eq!(
        reported_ids, published_ids,
        "case default request: expected exactly the published plans, got {reported_ids:?}"
    );

    // An id the catalog does not publish resolves to nothing — the lookup answers
    // from the catalog rather than from the shape of what it is asked.
    for absent in ["micro_9_9", "tiny_1_0", "micro", "MICRO_1_0", "micro_ha", ""] {
        assert!(
            relational_database_bundle_spec(absent).is_none(),
            "case {absent:?}: an id the catalog does not publish must not resolve"
        );
    }
}

/// The `includeInactive` flag selects a slice of the one catalog: absent and
/// `false` report the active entries, `true` reports every entry, and the first is
/// always contained in the second.
///
/// The expectations come from the catalog the service serves, so the rule holds
/// whatever that catalog grows to hold — including a plan Lightsail later retires.
#[tokio::test]
async fn the_include_inactive_flag_selects_a_slice_of_the_one_catalog() {
    let emulator = Emulator::new();

    let active_ids: BTreeSet<&str> = RELATIONAL_DATABASE_BUNDLE_SPECS
        .iter()
        .filter(|spec| spec.is_active)
        .map(|spec| spec.bundle_id)
        .collect();
    let all_ids: BTreeSet<&str> = RELATIONAL_DATABASE_BUNDLE_SPECS
        .iter()
        .map(|spec| spec.bundle_id)
        .collect();

    // The rows differ along the only dimension the flag has: absent, off, on.
    let rows: &[(&str, Value, &BTreeSet<&str>)] = &[
        ("absent", json!({}), &active_ids),
        ("false", json!({"includeInactive": false}), &active_ids),
        ("true", json!({"includeInactive": true}), &all_ids),
    ];

    let mut by_case: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (case, request, expected) in rows {
        let bundles = reported(&emulator, request.clone()).await;
        let ids: BTreeSet<String> = bundles.keys().cloned().collect();
        let expected_ids: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            ids, expected_ids,
            "case includeInactive={case}: expected the catalog slice {expected_ids:?}, got {ids:?}"
        );

        if *case != "true" {
            for (id, bundle) in &bundles {
                assert_eq!(
                    bundle["isActive"].as_bool(),
                    Some(true),
                    "case includeInactive={case}: {id} must not be reported while inactive, got {bundle:?}"
                );
            }
        }
        by_case.insert(case, ids);
    }

    let active = &by_case["absent"];
    assert_eq!(
        active, &by_case["false"],
        "case includeInactive absent vs false: an absent flag means false, got {active:?} and {:?}",
        by_case["false"]
    );
    assert!(
        by_case["true"].is_superset(active),
        "case includeInactive=true: the full catalog must contain every active bundle, got {:?} against {active:?}",
        by_case["true"]
    );
}

/// A plan in a catalog. Only the id and `is_active` matter to the slice and
/// paging rules, so the figures are the same on every row.
fn spec(bundle_id: &'static str, is_active: bool) -> RelationalDatabaseBundleSpec {
    RelationalDatabaseBundleSpec {
        bundle_id,
        name: "Micro",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 100,
        price_usd_per_month: 15.0,
        is_encrypted: true,
        is_active,
    }
}

/// The slice and paging rules themselves, over catalogs and page sizes the
/// published catalog does not have.
///
/// The published catalog holds no retired plan (Lightsail has withdrawn none) and
/// is smaller than one page, so the wire cases above cannot tell "filtered on
/// `isActive`" from "reported everything", nor exercise a second page. These rows
/// put the rules' own dimensions under test: how many entries the catalog holds,
/// which of them are inactive and where, how big a page is, and which entry the
/// token names.
#[test]
fn a_page_is_the_requested_slice_from_the_token_onward_bounded_by_the_page_size() {
    /// One row: a catalog, a request, and the page it must answer — the ids in
    /// order and the next page token — or `None` when the token names no entry of
    /// the slice, which the handler turns into `InvalidInputException`.
    struct Row {
        case: &'static str,
        catalog: Vec<RelationalDatabaseBundleSpec>,
        include_inactive: bool,
        page_size: usize,
        page_token: Option<&'static str>,
        expected: Option<(&'static [&'static str], Option<&'static str>)>,
    }

    let five = || {
        vec![
            spec("micro_1_0", true),
            spec("small_1_0", true),
            spec("medium_1_0", true),
            spec("large_1_0", true),
            spec("micro_ha_1_0", true),
        ]
    };

    let rows = vec![
        Row {
            case: "an empty catalog",
            catalog: vec![],
            include_inactive: false,
            page_size: 2,
            page_token: None,
            expected: Some((&[], None)),
        },
        Row {
            case: "a page bigger than the catalog",
            catalog: five(),
            include_inactive: false,
            page_size: 20,
            page_token: None,
            expected: Some((
                &[
                    "micro_1_0",
                    "small_1_0",
                    "medium_1_0",
                    "large_1_0",
                    "micro_ha_1_0",
                ],
                None,
            )),
        },
        Row {
            case: "the first of three pages",
            catalog: five(),
            include_inactive: false,
            page_size: 2,
            page_token: None,
            expected: Some((&["micro_1_0", "small_1_0"], Some("medium_1_0"))),
        },
        Row {
            case: "an empty token asks for the first page",
            catalog: five(),
            include_inactive: false,
            page_size: 2,
            page_token: Some(""),
            expected: Some((&["micro_1_0", "small_1_0"], Some("medium_1_0"))),
        },
        Row {
            case: "a token resumes at the entry it names",
            catalog: five(),
            include_inactive: false,
            page_size: 2,
            page_token: Some("medium_1_0"),
            expected: Some((&["medium_1_0", "large_1_0"], Some("micro_ha_1_0"))),
        },
        Row {
            case: "the last page is short and ends the walk",
            catalog: five(),
            include_inactive: false,
            page_size: 2,
            page_token: Some("micro_ha_1_0"),
            expected: Some((&["micro_ha_1_0"], None)),
        },
        Row {
            case: "a one-entry page",
            catalog: five(),
            include_inactive: false,
            page_size: 1,
            page_token: Some("large_1_0"),
            expected: Some((&["large_1_0"], Some("micro_ha_1_0"))),
        },
        Row {
            case: "a page size of zero still carries one entry, so a walk advances",
            catalog: five(),
            include_inactive: false,
            page_size: 0,
            page_token: None,
            expected: Some((&["micro_1_0"], Some("small_1_0"))),
        },
        Row {
            case: "a retired plan is skipped by default",
            catalog: vec![
                spec("micro_1_0", false),
                spec("small_1_0", true),
                spec("medium_1_0", false),
                spec("large_1_0", true),
            ],
            include_inactive: false,
            page_size: 20,
            page_token: None,
            expected: Some((&["small_1_0", "large_1_0"], None)),
        },
        Row {
            case: "a retired plan is reported when asked for",
            catalog: vec![
                spec("micro_1_0", false),
                spec("small_1_0", true),
                spec("medium_1_0", false),
                spec("large_1_0", true),
            ],
            include_inactive: true,
            page_size: 20,
            page_token: None,
            expected: Some((
                &["micro_1_0", "small_1_0", "medium_1_0", "large_1_0"],
                None,
            )),
        },
        Row {
            case: "paging the active slice skips the retired entries between pages",
            catalog: vec![
                spec("micro_1_0", true),
                spec("small_1_0", false),
                spec("medium_1_0", true),
                spec("large_1_0", false),
                spec("micro_ha_1_0", true),
            ],
            include_inactive: false,
            page_size: 1,
            page_token: Some("micro_1_0"),
            expected: Some((&["micro_1_0"], Some("medium_1_0"))),
        },
        Row {
            case: "every plan retired, reported only when asked for",
            catalog: vec![spec("micro_1_0", false), spec("small_1_0", false)],
            include_inactive: false,
            page_size: 20,
            page_token: None,
            expected: Some((&[], None)),
        },
        Row {
            case: "a token naming a retired plan is outside the default slice",
            catalog: vec![spec("micro_1_0", false), spec("small_1_0", true)],
            include_inactive: false,
            page_size: 20,
            page_token: Some("micro_1_0"),
            expected: None,
        },
        Row {
            case: "the same token names an entry of the full slice",
            catalog: vec![spec("micro_1_0", false), spec("small_1_0", true)],
            include_inactive: true,
            page_size: 20,
            page_token: Some("micro_1_0"),
            expected: Some((&["micro_1_0", "small_1_0"], None)),
        },
        Row {
            case: "a token naming no plan at all",
            catalog: five(),
            include_inactive: true,
            page_size: 20,
            page_token: Some("gigantic_9_9"),
            expected: None,
        },
        Row {
            case: "a token on an empty catalog",
            catalog: vec![],
            include_inactive: true,
            page_size: 20,
            page_token: Some("micro_1_0"),
            expected: None,
        },
    ];

    for row in &rows {
        let case = row.case;
        let answered = relational_database_bundle_page(
            &row.catalog,
            row.include_inactive,
            row.page_token,
            row.page_size,
        );
        match (&row.expected, answered) {
            (None, answered) => assert!(
                answered.is_none(),
                "case {case:?}: a token outside the slice has no page, got {:?}",
                answered.map(|page| page.bundles.len())
            ),
            (Some((expected_ids, expected_next)), answered) => {
                let answered = answered.unwrap_or_else(|| {
                    panic!("case {case:?}: expected the page {expected_ids:?}, got no page at all")
                });
                let ids: Vec<String> = answered
                    .bundles
                    .iter()
                    .map(|bundle| bundle.bundle_id.clone().unwrap_or_default())
                    .collect();
                assert_eq!(
                    ids,
                    expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>(),
                    "case {case:?}: expected the slice in catalog order, got {ids:?}"
                );
                assert!(
                    ids.len() <= row.page_size.max(1),
                    "case {case:?}: a page carries at most {} entries, got {}",
                    row.page_size.max(1),
                    ids.len()
                );
                assert_eq!(
                    answered.next_page_token.as_deref(),
                    *expected_next,
                    "case {case:?}: wrong nextPageToken, got {:?}",
                    answered.next_page_token
                );
            }
        }
    }
}

/// The walk over a catalog that outgrows a page reports every entry of the
/// requested slice exactly once, in catalog order, and terminates.
#[test]
fn a_paged_walk_reports_the_slice_once_each_whatever_the_page_size() {
    let catalog: Vec<RelationalDatabaseBundleSpec> = vec![
        spec("micro_1_0", true),
        spec("small_1_0", false),
        spec("medium_1_0", true),
        spec("large_1_0", true),
        spec("micro_ha_1_0", false),
        spec("small_ha_1_0", true),
        spec("medium_ha_1_0", true),
    ];

    // Rows differ along the page size — including one far above the catalog — and
    // along which slice is walked.
    for page_size in [1usize, 2, 3, 6, 7, 50] {
        for include_inactive in [false, true] {
            let case = format!("page size {page_size}, includeInactive={include_inactive}");
            let expected: Vec<&str> = catalog
                .iter()
                .filter(|spec| include_inactive || spec.is_active)
                .map(|spec| spec.bundle_id)
                .collect();

            let mut reported: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let page = relational_database_bundle_page(
                    &catalog,
                    include_inactive,
                    token.as_deref(),
                    page_size,
                )
                .unwrap_or_else(|| {
                    panic!("case {case}: the token {token:?} the walk was handed must open a page")
                });
                pages += 1;
                assert!(
                    page.bundles.len() <= page_size,
                    "case {case}: a page carries at most {page_size} entries, got {}",
                    page.bundles.len()
                );
                let is_last = page.next_page_token.is_none();
                if !is_last {
                    assert_eq!(
                        page.bundles.len(),
                        page_size,
                        "case {case}: a page followed by another must be full, got {}",
                        page.bundles.len()
                    );
                }
                reported.extend(
                    page.bundles
                        .iter()
                        .map(|bundle| bundle.bundle_id.clone().unwrap_or_default()),
                );
                match page.next_page_token {
                    Some(next) => token = Some(next),
                    None => break,
                }
                assert!(
                    pages < 100,
                    "case {case}: the walk did not terminate after {pages} pages"
                );
            }

            assert_eq!(
                reported,
                expected
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect::<Vec<_>>(),
                "case {case}: the walk must report the slice in order, once each, got {reported:?}"
            );
        }
    }
}

/// Over the wire: every published bundle id is a page token that opens the
/// catalog suffix from there, an absent and an empty token both ask for the first
/// page, and every page is bounded by the published page size.
#[tokio::test]
async fn a_page_token_resumes_the_catalog_at_the_bundle_it_names() {
    let emulator = Emulator::new();
    let (whole, page_sizes) = walk(&emulator, json!({"includeInactive": true})).await;
    let ids: Vec<String> = whole
        .iter()
        .map(|bundle| bundle["bundleId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !ids.is_empty(),
        "case whole catalog: expected at least one database bundle, got none"
    );

    for (index, size) in page_sizes.iter().enumerate() {
        assert!(
            *size <= RELATIONAL_DATABASE_BUNDLE_PAGE_SIZE,
            "case page {index}: a page carries at most {RELATIONAL_DATABASE_BUNDLE_PAGE_SIZE} bundles, got {size}"
        );
        if index + 1 != page_sizes.len() {
            assert_eq!(
                *size, RELATIONAL_DATABASE_BUNDLE_PAGE_SIZE,
                "case page {index}: a page followed by another must be full, got {size}"
            );
        }
    }

    for (index, token) in ids.iter().enumerate() {
        let (suffix, _) = walk(
            &emulator,
            json!({"includeInactive": true, "pageToken": token}),
        )
        .await;
        let suffix_ids: Vec<String> = suffix
            .iter()
            .map(|bundle| bundle["bundleId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            suffix_ids,
            ids[index..],
            "case pageToken {token:?}: expected the catalog from there on, got {suffix_ids:?}"
        );
    }

    for request in [
        json!({"includeInactive": true}),
        json!({"includeInactive": true, "pageToken": ""}),
    ] {
        let (first_page, _) = page(&emulator, request.clone()).await;
        let page_ids: Vec<String> = first_page
            .iter()
            .map(|bundle| bundle["bundleId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            page_ids,
            ids[..page_sizes[0]],
            "case {request:?}: expected the first page, got {page_ids:?}"
        );
    }
}

/// A `pageToken` that names no bundle of the requested slice is the modelled 400
/// `InvalidInputException`.
#[tokio::test]
async fn a_page_token_that_names_no_bundle_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a token can fail to name a catalog entry: a size
    // the service does not sell, a generation it does not, a half id, an id in the
    // wrong case, an opaque cursor from elsewhere, and whitespace.
    for token in [
        "tiny_1_0",
        "micro_9_9",
        "micro",
        "micro_ha",
        "MICRO_2_0",
        "AAAAAAAA",
        " ",
    ] {
        for request in [
            json!({"pageToken": token}),
            json!({"pageToken": token, "includeInactive": true}),
        ] {
            let (status, body) = emulator.call(OPERATION, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {request:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {request:?}: expected the modelled InvalidInputException, got {body:?}"
            );
        }
    }
}

/// The catalog's plans grow together: a dearer plan never buys less, and a
/// high-availability plan is its standard plan's hardware at a higher price.
#[test]
fn the_published_plans_grow_with_their_price() {
    for plan in PUBLISHED_PLANS {
        let standard = relational_database_bundle_spec(plan.bundle_id)
            .unwrap_or_else(|| panic!("case {}: the standard plan must exist", plan.bundle_id));
        let high_availability = relational_database_bundle_spec(plan.ha_bundle_id).unwrap_or_else(
            || panic!("case {}: the standby plan must exist", plan.ha_bundle_id),
        );
        assert!(
            standard.cpu_count == high_availability.cpu_count
                && (standard.ram_size_in_gb - high_availability.ram_size_in_gb).abs() < 0.005
                && standard.disk_size_in_gb == high_availability.disk_size_in_gb
                && standard.transfer_per_month_in_gb
                    == high_availability.transfer_per_month_in_gb,
            "case {}: a standby plan sells the standard plan's hardware, got {high_availability:?} against {standard:?}",
            plan.ha_bundle_id
        );
        assert!(
            high_availability.price_usd_per_month > standard.price_usd_per_month,
            "case {}: a standby costs more than the standard plan, got {} against {}",
            plan.ha_bundle_id,
            high_availability.price_usd_per_month,
            standard.price_usd_per_month
        );
    }

    // Within each family the plans are ordered, and a dearer one never buys less.
    let families: [Vec<&str>; 2] = [
        PUBLISHED_PLANS.iter().map(|plan| plan.bundle_id).collect(),
        PUBLISHED_PLANS
            .iter()
            .map(|plan| plan.ha_bundle_id)
            .collect(),
    ];
    for family in &families {
        let specs: Vec<RelationalDatabaseBundleSpec> = family
            .iter()
            .map(|id| {
                relational_database_bundle_spec(id)
                    .unwrap_or_else(|| panic!("case {id}: the catalog must publish it"))
            })
            .collect();
        for (cheaper, dearer) in specs.iter().zip(specs.iter().skip(1)) {
            assert!(
                dearer.price_usd_per_month > cheaper.price_usd_per_month
                    && dearer.ram_size_in_gb >= cheaper.ram_size_in_gb
                    && dearer.disk_size_in_gb >= cheaper.disk_size_in_gb
                    && dearer.cpu_count >= cheaper.cpu_count
                    && dearer.transfer_per_month_in_gb >= cheaper.transfer_per_month_in_gb,
                "case {} after {}: a dearer plan must not buy less, got {dearer:?} after {cheaper:?}",
                dearer.bundle_id,
                cheaper.bundle_id
            );
        }
    }
}

/// Reporting the catalog is a read: it answers the same thing every time,
/// whatever else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let requests = [
        json!({}),
        json!({"includeInactive": false}),
        json!({"includeInactive": true}),
    ];

    let mut first = Vec::new();
    for request in &requests {
        first.push(emulator.ok(OPERATION, request.clone()).await);
    }

    // A paged walk, then a mutation of unrelated state, then the same reads
    // again: the published catalog is not per-caller and not affected by what the
    // store holds.
    let _ = walk(&emulator, json!({"includeInactive": true})).await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["db-bundle-reader"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_2_0"
            }),
        )
        .await;

    for (index, request) in requests.iter().enumerate() {
        let again = emulator.ok(OPERATION, request.clone()).await;
        assert_eq!(
            again, first[index],
            "case {request}: two reads of the catalog must agree, got {again:?} after {:?}",
            first[index]
        );
    }

    // The instance the mutation created is still there: reading the catalog
    // consumed nothing.
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "db-bundle-reader"}))
        .await;
    assert_eq!(
        instance["instance"]["name"].as_str(),
        Some("db-bundle-reader"),
        "case store unchanged: expected the created instance to survive the reads, got {instance:?}"
    );

    // A second emulator, with its own store, publishes the same catalog.
    let other = Emulator::new();
    for (index, request) in requests.iter().enumerate() {
        let elsewhere = other.ok(OPERATION, request.clone()).await;
        assert_eq!(
            elsewhere, first[index],
            "case {request}: the catalog must not depend on which emulator serves it, got {elsewhere:?}"
        );
    }
}
