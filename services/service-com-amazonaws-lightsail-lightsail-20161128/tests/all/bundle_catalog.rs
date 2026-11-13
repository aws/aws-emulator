// SPDX-License-Identifier: CC0-1.0

//! `GetBundles` — the instance-plan (bundle) catalog, driven over the wire.
//!
//! Every wire case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetBundles` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than bypassed.
//!
//! The rules under test, asserted over the WHOLE catalog and over sets of
//! requests rather than one recorded call:
//!
//! * every reported bundle's members are its catalog entry's: the id is
//!   `<size>_<generation>` for a Linux/Unix plan and `<size>_win_<generation>` for
//!   the Windows plan of the same size, `instanceType` is that size token, `name`
//!   is the size token title-cased (`micro` -> `Micro`, as `Bundle$name`'s own
//!   example shows), `supportedPlatforms` is the single platform the id names, and
//!   the hardware, transfer, power and price are the plan's;
//! * `includeInactive` is what widens the answer from the active plans to every
//!   published one, and an absent flag reports the active ones only;
//! * `appCategory` selects the catalog: a request naming a category reports the
//!   Lightsail for Research plans (this emulator publishes none, so that list is
//!   empty), one naming none reports the instance plans, and a category outside
//!   the modelled `AppCategory` enum is refused by the constraint walk with a 400
//!   `ValidationException`;
//! * a page is the reported list from the bundle `pageToken` names onward, bounded
//!   by the page size, with `nextPageToken` present only while plans remain — so
//!   the walk reports each plan in order, exactly once — and a token that names no
//!   reported bundle is a modelled 400 `InvalidInputException`;
//! * a reported bundle id can be created from, and the instance then reports that
//!   plan's own hardware and monthly transfer; and
//! * the operation reports, never mutates: identical calls answer identically, and
//!   the catalog does not depend on what the store holds.
//!
//! The paging and slicing rules are additionally exercised directly against
//! `state::bundles_from` / `state::bundle_page` over SYNTHETIC catalogs, at page
//! sizes the published catalog (answered whole in one page) cannot reach.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The front door every suite in this test target drives the service through, and
// the awsJson error-code reader, both already implemented beside this suite.
use super::instance_lifecycle::{error_code, Emulator};

/// The operation under test.
const OPERATION: &str = "GetBundles";

/// The page size the service publishes for this operation, read from the crate
/// rather than restated here, so a page that grows stays bounded by it.
const PAGE_SIZE: usize = state::BUNDLE_PAGE_SIZE;

/// The two values of the modelled `InstancePlatform` enum
/// (`smithy_query(query="shape",shape="InstancePlatform")`).
const LINUX_UNIX: &str = "LINUX_UNIX";
const WINDOWS: &str = "WINDOWS";

/// One expected published plan, written out here independently of the service's
/// own `BUNDLE_SPECS` so these rows pin the published figures rather than
/// restating whatever the code happens to hold.
///
/// SOURCE: the Lightsail instance-plan table (the "Instances" plans of the
/// Lightsail pricing page, which `GetBundles` answers from) — each step doubles
/// RAM while the SSD size and monthly transfer follow the plan table, the vCPU
/// count is two up to the 8 GB plan and doubles with the plan above it, and the
/// Windows plan of a size costs more because it carries an operating-system
/// licence. The rows span what the rule generalises over: the smallest plan, a
/// mid-sized one, the two largest published on both platforms (whose size token
/// carries a multiplier), the largest published on Linux/Unix alone, and Windows
/// plans of two different sizes.
struct Expected {
    bundle_id: &'static str,
    instance_type: &'static str,
    name: &'static str,
    platform: &'static str,
    cpu_count: i64,
    ram_size_in_gb: f64,
    disk_size_in_gb: i64,
    transfer_per_month_in_gb: i64,
    power: i64,
    price: f64,
}

const EXPECTED: &[Expected] = &[
    Expected {
        bundle_id: "nano_3_0",
        instance_type: "nano",
        name: "Nano",
        platform: LINUX_UNIX,
        cpu_count: 2,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_per_month_in_gb: 1_024,
        power: 298,
        price: 5.0,
    },
    Expected {
        bundle_id: "medium_3_0",
        instance_type: "medium",
        name: "Medium",
        platform: LINUX_UNIX,
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 4_096,
        power: 2_000,
        price: 24.0,
    },
    Expected {
        bundle_id: "2xlarge_3_0",
        instance_type: "2xlarge",
        name: "2Xlarge",
        platform: LINUX_UNIX,
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 640,
        transfer_per_month_in_gb: 7_168,
        power: 5_000,
        price: 164.0,
    },
    Expected {
        bundle_id: "4xlarge_3_0",
        instance_type: "4xlarge",
        name: "4Xlarge",
        platform: LINUX_UNIX,
        cpu_count: 16,
        ram_size_in_gb: 64.0,
        disk_size_in_gb: 1_280,
        transfer_per_month_in_gb: 8_192,
        power: 6_000,
        price: 384.0,
    },
    Expected {
        bundle_id: "16xlarge_3_0",
        instance_type: "16xlarge",
        name: "16Xlarge",
        platform: LINUX_UNIX,
        cpu_count: 64,
        ram_size_in_gb: 256.0,
        disk_size_in_gb: 5_120,
        transfer_per_month_in_gb: 11_264,
        power: 8_000,
        price: 1_764.0,
    },
    Expected {
        bundle_id: "micro_win_3_0",
        instance_type: "micro",
        name: "Micro",
        platform: WINDOWS,
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 2_048,
        power: 500,
        price: 16.0,
    },
    Expected {
        bundle_id: "xlarge_win_3_0",
        instance_type: "xlarge",
        name: "Xlarge",
        platform: WINDOWS,
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 320,
        transfer_per_month_in_gb: 6_144,
        power: 4_000,
        price: 166.0,
    },
];

// ---------------------------------------------------------------------------
// The rule's own derivations, restated here rather than read off the crate
// ---------------------------------------------------------------------------

/// The platform a bundle id names: Windows for exactly the ids carrying the `win`
/// token between size and generation (`micro_win_2_0`).
fn want_platform(bundle_id: &str) -> &'static str {
    if bundle_id.split('_').any(|token| token == "win") {
        WINDOWS
    } else {
        LINUX_UNIX
    }
}

/// The size token a bundle id names: its leading token.
fn want_instance_type(bundle_id: &str) -> &str {
    bundle_id.split('_').next().unwrap_or_default()
}

/// Whether `price` is `expected` dollars, to the cent.
fn is_price(price: Option<f64>, expected: f64) -> bool {
    price.is_some_and(|price| (price - expected).abs() < 0.005)
}

/// Whether `size` is `expected` GB, to a hundredth of a GB (RAM is a float: the
/// smallest plan carries half a gigabyte).
fn is_gb(size: Option<f64>, expected: f64) -> bool {
    size.is_some_and(|size| (size - expected).abs() < 0.01)
}

// ---------------------------------------------------------------------------
// This suite's reads, over the shared harness
// ---------------------------------------------------------------------------

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
        let (reported, next) = page(emulator, body).await;
        page_sizes.push(reported.len());
        bundles.extend(reported);
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

/// A member of a bundle, as a string, or a panic naming the case.
fn member(bundle: &Value, name: &str) -> String {
    bundle[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {bundle:?}: expected a string {name}"))
        .to_string()
}

/// The reported ids of a list of bundles.
fn ids(bundles: &[Value]) -> Vec<String> {
    bundles
        .iter()
        .map(|bundle| member(bundle, "bundleId"))
        .collect()
}

/// The ids the published catalog reports, in its own order, filtered the way
/// `includeInactive` filters: each plan size on each platform.
fn published_ids(include_inactive: bool) -> Vec<String> {
    state::bundles_from(state::BUNDLE_SPECS, include_inactive)
        .into_iter()
        .map(|bundle| bundle.bundle_id.unwrap_or_default())
        .collect()
}

// ---------------------------------------------------------------------------
// The reported members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_reported_bundle_reports_its_own_plan() {
    let emulator = Emulator::new();
    let (bundles, _) = walk(&emulator, json!({"includeInactive": true})).await;

    assert!(
        !bundles.is_empty(),
        "case whole catalog: expected at least one bundle, got none"
    );

    for bundle in &bundles {
        let case = member(bundle, "bundleId");

        // `<size>[_win]_<major>_<minor>`: the id form the modelled
        // `Bundle$bundleId` documentation's `micro_x_x` shows, with the platform
        // token a Windows plan carries.
        let tokens: Vec<&str> = case.split('_').collect();
        let platform = want_platform(&case);
        let expected_tokens = if platform == WINDOWS { 4 } else { 3 };
        assert_eq!(
            tokens.len(),
            expected_tokens,
            "case {case:?}: expected a <size>[_win]_<major>_<minor> bundle id, got {tokens:?}"
        );
        for version in &tokens[tokens.len() - 2..] {
            assert!(
                !version.is_empty() && version.chars().all(|c| c.is_ascii_digit()),
                "case {case:?}: expected numeric version tokens, got {version:?}"
            );
        }

        // `instanceType` is "The instance type (`micro`)" — the plan's size token,
        // which is the id's leading token.
        assert_eq!(
            member(bundle, "instanceType"),
            want_instance_type(&case),
            "case {case:?}: expected the id's size token as instanceType, got {bundle:?}"
        );

        // `supportedPlatforms` is the platform the id names, and only it: "You can
        // only launch a `WINDOWS` bundle on a blueprint that supports the
        // `WINDOWS` platform."
        assert_eq!(
            bundle["supportedPlatforms"],
            json!([platform]),
            "case {case:?}: expected exactly the platform the id names, got {bundle:?}"
        );

        // A plan charges for hardware it provides, and its power is the value a
        // blueprint's `minPower` is compared against.
        assert!(
            bundle["cpuCount"].as_i64().is_some_and(|cpus| cpus >= 1),
            "case {case:?}: a plan provides at least one vCPU, got {bundle:?}"
        );
        assert!(
            bundle["ramSizeInGb"].as_f64().is_some_and(|ram| ram > 0.0),
            "case {case:?}: a plan provides RAM, got {bundle:?}"
        );
        assert!(
            bundle["diskSizeInGb"].as_i64().is_some_and(|disk| disk > 0),
            "case {case:?}: a plan provides an SSD, got {bundle:?}"
        );
        assert!(
            bundle["transferPerMonthInGb"]
                .as_i64()
                .is_some_and(|transfer| transfer > 0),
            "case {case:?}: a plan provides monthly transfer, got {bundle:?}"
        );
        assert!(
            bundle["price"].as_f64().is_some_and(|price| price > 0.0),
            "case {case:?}: a plan has a monthly price, got {bundle:?}"
        );
        assert!(
            bundle["power"].as_i64().is_some_and(|power| power > 0),
            "case {case:?}: a plan has a power value, got {bundle:?}"
        );

        // "the value is either 0 or 1" (`Bundle$publicIpv4AddressCount`), and
        // every plan this catalog publishes is a standard plan that includes one
        // public IPv4 address (0 is the IPv6-only case, which it publishes none
        // of).
        let ipv4 = bundle["publicIpv4AddressCount"].as_i64();
        assert!(
            ipv4 == Some(0) || ipv4 == Some(1),
            "case {case:?}: publicIpv4AddressCount is either 0 or 1, got {ipv4:?}"
        );
        assert_eq!(
            ipv4,
            Some(1),
            "case {case:?}: a published standard plan includes one public IPv4 address, got \
             {ipv4:?}"
        );

        assert_eq!(
            bundle["isActive"].as_bool(),
            Some(true),
            "case {case:?}: every published plan is still offered, got {bundle:?}"
        );

        // `supportedAppCategories` "only applies to Lightsail for Research
        // resources", and the member is optional: a standard plan must leave it
        // ABSENT rather than carry an invented category.
        assert!(
            bundle["supportedAppCategories"].is_null(),
            "case {case:?}: a standard plan carries no supportedAppCategories, got {bundle:?}"
        );
    }

    // Every reported id resolves in the catalog, and reports that entry's
    // hardware: the answer is the catalog's, not the request's.
    for bundle in &bundles {
        let case = member(bundle, "bundleId");
        let spec = state::bundle_spec(&case)
            .unwrap_or_else(|| panic!("case {case:?}: a reported id must resolve in the catalog"));
        assert_eq!(
            bundle["cpuCount"].as_i64(),
            Some(i64::from(spec.cpu_count)),
            "case {case:?}: the reported vCPU count must be the catalog's, got {bundle:?}"
        );
        assert_eq!(
            bundle["diskSizeInGb"].as_i64(),
            Some(i64::from(spec.disk_size_in_gb)),
            "case {case:?}: the reported SSD size must be the catalog's, got {bundle:?}"
        );
    }

    // No id is reported twice, and the catalog is exactly the published one.
    let all_ids = ids(&bundles);
    let unique: BTreeSet<&String> = all_ids.iter().collect();
    assert_eq!(
        unique.len(),
        all_ids.len(),
        "case whole catalog: a bundle was reported twice: {all_ids:?}"
    );
    assert_eq!(
        all_ids,
        published_ids(true),
        "case whole catalog: expected the published catalog in its own order"
    );
}

#[tokio::test]
async fn the_published_sample_plans_report_their_published_figures() {
    let emulator = Emulator::new();
    let (bundles, _) = walk(&emulator, json!({"includeInactive": true})).await;
    let reported = ids(&bundles);

    for want in EXPECTED {
        let case = want.bundle_id;
        let found = bundles
            .iter()
            .find(|bundle| member(bundle, "bundleId") == case)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the catalog to carry it, got {reported:?}")
            });

        assert_eq!(
            member(found, "instanceType"),
            want.instance_type,
            "case {case:?}: wrong instanceType, got {found:?}"
        );
        assert_eq!(
            member(found, "name"),
            want.name,
            "case {case:?}: wrong friendly name, got {found:?}"
        );
        assert_eq!(
            found["supportedPlatforms"],
            json!([want.platform]),
            "case {case:?}: wrong supportedPlatforms, got {found:?}"
        );
        assert_eq!(
            found["cpuCount"].as_i64(),
            Some(want.cpu_count),
            "case {case:?}: expected cpuCount {}, got {found:?}",
            want.cpu_count
        );
        assert!(
            is_gb(found["ramSizeInGb"].as_f64(), want.ram_size_in_gb),
            "case {case:?}: expected ramSizeInGb {}, got {found:?}",
            want.ram_size_in_gb
        );
        assert_eq!(
            found["diskSizeInGb"].as_i64(),
            Some(want.disk_size_in_gb),
            "case {case:?}: expected diskSizeInGb {}, got {found:?}",
            want.disk_size_in_gb
        );
        assert_eq!(
            found["transferPerMonthInGb"].as_i64(),
            Some(want.transfer_per_month_in_gb),
            "case {case:?}: expected transferPerMonthInGb {}, got {found:?}",
            want.transfer_per_month_in_gb
        );
        assert_eq!(
            found["power"].as_i64(),
            Some(want.power),
            "case {case:?}: expected power {}, got {found:?}",
            want.power
        );
        assert!(
            is_price(found["price"].as_f64(), want.price),
            "case {case:?}: expected price {}, got {found:?}",
            want.price
        );
    }
}

/// The Windows plan of a size provides the same hardware as its Linux/Unix plan
/// and costs more: the licence is what the extra buys. A size the plan table
/// publishes no Windows price for has no Windows plan at all.
#[tokio::test]
async fn the_windows_plan_of_a_size_costs_more_than_its_linux_plan() {
    let emulator = Emulator::new();
    let (bundles, _) = walk(&emulator, json!({"includeInactive": true})).await;

    for spec in state::BUNDLE_SPECS {
        let case = spec.size;
        let of = |platform: &str| -> Option<Value> {
            let id = state::bundle_id(spec.size, platform, state::BUNDLE_GENERATION);
            bundles
                .iter()
                .find(|bundle| member(bundle, "bundleId") == id)
                .cloned()
        };
        let linux = of(LINUX_UNIX)
            .unwrap_or_else(|| panic!("case {case}: every published size has a Linux/Unix plan"));
        let Some(windows) = of(WINDOWS) else {
            assert!(
                !spec.is_published_on(WINDOWS),
                "case {case}: the plan table prices this size on Windows, so it must be reported"
            );
            continue;
        };
        assert!(
            spec.is_published_on(WINDOWS),
            "case {case}: a Windows plan was reported for a size the table does not price there, \
             got {windows:?}"
        );

        for hardware in [
            "cpuCount",
            "diskSizeInGb",
            "ramSizeInGb",
            "transferPerMonthInGb",
            "power",
            "instanceType",
            "name",
        ] {
            assert_eq!(
                linux[hardware], windows[hardware],
                "case {case}: the two platforms of a size share their {hardware}, got {linux:?} \
                 and {windows:?}"
            );
        }
        let linux_price = linux["price"].as_f64().unwrap_or_default();
        let windows_price = windows["price"].as_f64().unwrap_or_default();
        assert!(
            windows_price > linux_price,
            "case {case}: the Windows plan carries a licence, so it costs more than \
             {linux_price}, got {windows_price}"
        );
    }
}

/// The friendly name is DERIVED from the size token, never carried per entry:
/// rows span the published sizes and sizes the published catalog does not carry.
///
/// SOURCE of the derivation: `Bundle$name`, "A friendly name for the bundle
/// (`Micro`)" — the size token with its first letter upper-cased, so a token that
/// opens with a multiplier capitalises the letter after it (`2xlarge` ->
/// `2Xlarge`), the form the recorded plan names carry.
#[test]
fn the_friendly_name_is_the_size_token_title_cased() {
    let rows: &[(&str, &str)] = &[
        ("nano", "Nano"),
        ("micro", "Micro"),
        ("small", "Small"),
        ("medium", "Medium"),
        ("large", "Large"),
        ("xlarge", "Xlarge"),
        ("2xlarge", "2Xlarge"),
        ("4xlarge", "4Xlarge"),
        // Sizes the published catalog does not carry: the derivation is the rule,
        // not a lookup table of the sizes served today.
        ("8xlarge", "8Xlarge"),
        ("16xlarge", "16Xlarge"),
        ("tiny", "Tiny"),
        ("", ""),
    ];

    for (size, expected) in rows {
        let got = state::bundle_size_name(size);
        assert_eq!(
            got, *expected,
            "case size {size:?}: expected the friendly name {expected:?}, got {got:?}"
        );
    }
}

/// A bundle id is the size token, the Windows platform token where the plan is a
/// Windows one, then the generation — over sizes and generations the published
/// catalog does not carry.
#[test]
fn a_bundle_id_names_its_size_platform_and_generation() {
    let rows: &[(&str, &str, &str, &str)] = &[
        ("nano", LINUX_UNIX, "3_0", "nano_3_0"),
        ("nano", WINDOWS, "3_0", "nano_win_3_0"),
        ("2xlarge", LINUX_UNIX, "2_0", "2xlarge_2_0"),
        ("2xlarge", WINDOWS, "2_0", "2xlarge_win_2_0"),
        ("micro", LINUX_UNIX, "9_7", "micro_9_7"),
        ("micro", WINDOWS, "9_7", "micro_win_9_7"),
    ];

    for (size, platform, generation, expected) in rows {
        let got = state::bundle_id(size, platform, generation);
        assert_eq!(
            got, *expected,
            "case {size} on {platform} at {generation}: expected the id {expected:?}, got {got:?}"
        );
        // And the id resolves back to the plan of that size, whatever its
        // generation: that is how an instance on an older generation reports its
        // hardware.
        assert_eq!(
            state::bundle_spec(&got).map(|spec| spec.size),
            Some(*size),
            "case {got:?}: expected the id to resolve to the {size} plan"
        );
    }
}

// ---------------------------------------------------------------------------
// includeInactive
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inactive_bundles_are_reported_only_when_the_request_asks_for_them() {
    let emulator = Emulator::new();

    // Rows: the flag absent, sent false, and sent true.
    for (request, include_inactive) in [
        (json!({}), false),
        (json!({"includeInactive": false}), false),
        (json!({"includeInactive": true}), true),
    ] {
        let case = request.clone();
        let (bundles, _) = walk(&emulator, request).await;
        let reported = ids(&bundles);
        assert_eq!(
            reported,
            published_ids(include_inactive),
            "case {case:?}: expected the catalog slice for includeInactive={include_inactive}"
        );
        if !include_inactive {
            for bundle in &bundles {
                assert_eq!(
                    bundle["isActive"].as_bool(),
                    Some(true),
                    "case {case:?}: an inactive plan must be withheld, got {bundle:?}"
                );
            }
        }
    }

    // The default slice is contained in the full one, whatever the catalog holds.
    let active: BTreeSet<String> = published_ids(false).into_iter().collect();
    let everything: BTreeSet<String> = published_ids(true).into_iter().collect();
    assert!(
        everything.is_superset(&active),
        "case includeInactive=true: the full catalog must contain every active plan, got \
         {everything:?} against {active:?}"
    );
}

/// The slice rule itself, over catalogs that DO hold a retired plan.
///
/// The published catalog has no retired size yet, so the wire case above cannot
/// tell "filtered on `isActive`" from "reported everything". These rows put the
/// rule's own dimension under test: how many of the catalog's entries are
/// inactive, and where they sit in it.
#[test]
fn the_slice_rule_reports_an_inactive_plan_only_when_asked_for_it() {
    /// One row: the case name, the catalog, the sizes reported by default, and the
    /// sizes reported with `includeInactive`.
    struct Row {
        case: &'static str,
        catalog: Vec<state::BundleSpec>,
        active_only: &'static [&'static str],
        everything: &'static [&'static str],
    }

    let rows = [
        Row {
            case: "an empty catalog",
            catalog: vec![],
            active_only: &[],
            everything: &[],
        },
        Row {
            case: "every plan active",
            catalog: vec![synthetic("nano", true), synthetic("large", true)],
            active_only: &["nano", "large"],
            everything: &["nano", "large"],
        },
        Row {
            case: "a retired plan between two active ones",
            catalog: vec![
                synthetic("nano", true),
                synthetic("small", false),
                synthetic("large", true),
            ],
            active_only: &["nano", "large"],
            everything: &["nano", "small", "large"],
        },
        Row {
            case: "a retired plan last",
            catalog: vec![synthetic("nano", true), synthetic("large", false)],
            active_only: &["nano"],
            everything: &["nano", "large"],
        },
        Row {
            case: "every plan retired",
            catalog: vec![synthetic("nano", false), synthetic("large", false)],
            active_only: &[],
            everything: &["nano", "large"],
        },
    ];

    for row in &rows {
        let case = row.case;
        for (include_inactive, expected) in [(false, row.active_only), (true, row.everything)] {
            let reported = state::bundles_from(&row.catalog, include_inactive);
            // One entry per platform and size: the plans of a platform in the
            // catalog's own order, the platforms in the published order.
            let want: Vec<String> = state::BUNDLE_PLATFORMS
                .iter()
                .flat_map(|platform| {
                    expected.iter().map(move |size| {
                        state::bundle_id(size, platform, state::BUNDLE_GENERATION)
                    })
                })
                .collect();
            let got: Vec<String> = reported
                .iter()
                .map(|bundle| bundle.bundle_id.clone().unwrap_or_default())
                .collect();
            assert_eq!(
                got, want,
                "case {case:?} with includeInactive={include_inactive}: expected {want:?} in the \
                 catalog's own order, got {got:?}"
            );
            for bundle in &reported {
                assert_eq!(
                    bundle.supported_app_categories, None,
                    "case {case:?}: a standard plan carries no app category, got {bundle:?}"
                );
            }
        }
    }

    // A plan's supported app categories are the PLAN's own data, like its
    // platform: a standard plan publishes none and reports the member absent,
    // while a plan that does publish categories reports exactly those — never a
    // category taken from the request.
    let research = state::BundleSpec {
        app_categories: &["LfR"],
        ..synthetic("nano", true)
    };
    for bundle in state::bundles_from(&[research], true) {
        assert_eq!(
            bundle.supported_app_categories.as_deref(),
            Some(["LfR".to_string()].as_slice()),
            "case a plan publishing a category: expected the plan's own categories, got \
             {bundle:?}"
        );
    }
}

/// A plan in a synthetic catalog, active or retired — the figures are irrelevant
/// to the slice and paging rules, only the size and `is_active` are.
fn synthetic(size: &'static str, is_active: bool) -> state::BundleSpec {
    state::BundleSpec {
        size,
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
        power: 500,
        price_usd_per_month: 5.0,
        windows_price_usd_per_month: Some(12.0),
        is_active,
        app_categories: &[],
    }
}

// ---------------------------------------------------------------------------
// appCategory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_naming_an_app_category_reports_the_research_catalog() {
    let emulator = Emulator::new();

    let (bundles, next) = page(&emulator, json!({"appCategory": "LfR"})).await;
    // This emulator publishes no Lightsail for Research plan, so the Research
    // catalog is empty — and an empty list is still a list, with no next page.
    assert!(
        bundles.is_empty(),
        "case appCategory LfR: this emulator publishes no Research plan, so the Research \
         catalog is empty, got {bundles:?}"
    );
    assert_eq!(
        bundles.len(),
        state::LFR_BUNDLE_SPECS.len(),
        "case appCategory LfR: expected the published Research catalog, got {bundles:?}"
    );
    assert_eq!(
        next, None,
        "case appCategory LfR: a last page carries no nextPageToken, got {next:?}"
    );

    // The category SELECTS the catalog rather than filtering the standard one, so
    // the two catalogs the selection answers from are disjoint.
    let standard: Vec<&str> = state::bundle_catalog(None)
        .iter()
        .map(|spec| spec.size)
        .collect();
    let research: Vec<&str> = state::bundle_catalog(Some("LfR"))
        .iter()
        .map(|spec| spec.size)
        .collect();
    for size in &research {
        assert!(
            !standard.contains(size),
            "case appCategory LfR: the Research catalog must not hold an instance plan, got \
             {size:?}"
        );
    }

    // And the instance catalog is unaffected by a Research read.
    let (instance_plans, _) = walk(&emulator, json!({})).await;
    assert_eq!(
        ids(&instance_plans),
        published_ids(false),
        "case after a Research read: the instance catalog must be unchanged"
    );
}

#[tokio::test]
async fn an_app_category_outside_the_modelled_enum_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a category can fail to name an `AppCategory`
    // member: wrong case, a longer form, an unrelated word, an empty string and a
    // padded value. The member carries the modelled enum's value set, so the
    // refusal is the constraint walk's `ValidationException`, before the handler.
    for category in ["lfr", "LFR", "LightsailForResearch", "", "LfR "] {
        let (status, body) = emulator
            .call(OPERATION, json!({"appCategory": category}))
            .await;
        assert_eq!(
            status, 400,
            "case appCategory {category:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case appCategory {category:?}: expected the modelled enum's value set to be \
             enforced, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_page_token_resumes_the_catalog_at_the_bundle_it_names() {
    let emulator = Emulator::new();

    for request in [json!({}), json!({"includeInactive": true})] {
        let case = request.clone();
        let (whole, page_sizes) = walk(&emulator, request.clone()).await;
        let all_ids = ids(&whole);

        for (index, size) in page_sizes.iter().enumerate() {
            assert!(
                *size <= PAGE_SIZE,
                "case {case:?} page {index}: a page carries at most {PAGE_SIZE} bundles, got \
                 {size}"
            );
        }
        assert_eq!(
            all_ids.len(),
            page_sizes.iter().sum::<usize>(),
            "case {case:?}: the pages must account for every bundle, got {page_sizes:?}"
        );

        // Every reported id is a token, including the first and the last: the page
        // it opens is the reported list from there on.
        for (index, token) in all_ids.iter().enumerate() {
            let mut body = request.clone();
            body["pageToken"] = json!(token);
            let (suffix, _) = walk(&emulator, body).await;
            assert_eq!(
                ids(&suffix),
                all_ids[index..],
                "case {case:?} pageToken {token:?}: expected the reported list from there on"
            );
        }

        // An absent token and an empty one both ask for the first page.
        for token in [None, Some("")] {
            let mut body = request.clone();
            if let Some(token) = token {
                body["pageToken"] = json!(token);
            }
            let (first_page, _) = page(&emulator, body).await;
            assert_eq!(
                ids(&first_page),
                all_ids[..page_sizes[0]],
                "case {case:?} pageToken {token:?}: expected the first page"
            );
        }
    }
}

#[tokio::test]
async fn a_page_token_that_names_no_reported_bundle_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a token can fail to name an entry of the reported
    // list: a size that is not served, a size without its generation, a generation
    // that is not served, the friendly name, whitespace and an opaque cursor from
    // elsewhere.
    for token in [
        "not_a_bundle",
        "nano",
        "nano_9_9",
        "Nano",
        " ",
        "AAAAAAAA",
        "nano_win",
    ] {
        let (status, body) = emulator.call(OPERATION, json!({"pageToken": token})).await;
        assert_eq!(
            status, 400,
            "case pageToken {token:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case pageToken {token:?}: expected the modelled InvalidInputException, got {body:?}"
        );
    }

    // A token that IS a reported id opens a page at it.
    let (resumed, _) = page(&emulator, json!({"pageToken": "micro_win_3_0"})).await;
    assert_eq!(
        ids(&resumed).first().map(String::as_str),
        Some("micro_win_3_0"),
        "case pageToken micro_win_3_0: expected the page to open at it, got {resumed:?}"
    );
}

#[test]
fn a_paged_walk_reports_every_bundle_once_at_any_page_size() {
    let catalog = vec![
        synthetic("nano", true),
        synthetic("micro", true),
        synthetic("small", false),
        synthetic("medium", true),
        synthetic("large", true),
    ];
    let reported = state::bundles_from(&catalog, true);
    let all_ids: Vec<String> = reported
        .iter()
        .map(|bundle| bundle.bundle_id.clone().unwrap_or_default())
        .collect();

    // Rows: page sizes below, at and above the catalog's size, plus 0 — which
    // cannot mean "an empty page forever", so it carries one entry.
    for page_size in [0usize, 1, 2, 3, 7, 10, 25] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (page, next) = state::bundle_page(&reported, page_size, token.as_deref())
                .unwrap_or_else(|| panic!("case page size {page_size}: expected a page"));
            pages += 1;
            assert!(
                page.len() <= page_size.max(1),
                "case page size {page_size}: a page carries at most {page_size} bundles, got {}",
                page.len()
            );
            assert!(
                pages < 100,
                "case page size {page_size}: the walk did not terminate"
            );
            walked.extend(
                page.iter()
                    .map(|bundle| bundle.bundle_id.clone().unwrap_or_default()),
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, all_ids,
            "case page size {page_size}: expected every bundle once, in order"
        );
    }

    // A token that names no entry of the list has no page.
    for token in ["nano", "nope", "", "xlarge_3_0"] {
        assert!(
            state::bundle_page(&reported, 2, Some(token)).is_none(),
            "case pageToken {token:?}: expected no page"
        );
    }

    // Pathological inputs are TOTAL, not panics: a very long token names no
    // entry, and the largest page size answers the whole list with no next page.
    let long_token = "a".repeat(10_000);
    assert!(
        state::bundle_page(&reported, usize::MAX, Some(&long_token)).is_none(),
        "case a 10000-character pageToken: expected no page"
    );
    let (whole, next) = state::bundle_page(&reported, usize::MAX, None)
        .unwrap_or_else(|| panic!("case page size usize::MAX: expected the whole list"));
    assert_eq!(
        whole.len(),
        reported.len(),
        "case page size usize::MAX: expected every bundle in one page, got {}",
        whole.len()
    );
    assert_eq!(
        next, None,
        "case page size usize::MAX: a page holding everything has no next page, got {next:?}"
    );

    // A token naming a plan the request WITHHELD has no page either: a token is
    // resolved against the list the request asks for, and the retired plan is not
    // in the default slice.
    let active_only = state::bundles_from(&catalog, false);
    assert!(
        state::bundle_page(&active_only, 2, Some("small_3_0")).is_none(),
        "case pageToken small_3_0 without includeInactive: a withheld plan opens no page"
    );
    assert!(
        state::bundle_page(&reported, 2, Some("small_3_0")).is_some(),
        "case pageToken small_3_0 with includeInactive: the plan is reported, so it opens a page"
    );
}

// ---------------------------------------------------------------------------
// The catalog against the rest of the service
// ---------------------------------------------------------------------------

/// An instance created on a reported bundle reports that plan's own hardware and
/// monthly transfer: the catalog and the instance answer from one table.
#[tokio::test]
async fn an_instance_created_on_a_reported_bundle_reports_that_plans_hardware() {
    let emulator = Emulator::new();
    let (bundles, _) = walk(&emulator, json!({})).await;

    // Rows: the smallest plan, a mid-sized one, the largest and a Windows plan —
    // each created from a blueprint of the platform its id names.
    let rows: &[(&str, &str)] = &[
        ("nano_3_0", "ubuntu_22_04"),
        ("medium_3_0", "wordpress"),
        ("4xlarge_3_0", "amazon_linux_2023"),
        ("large_win_3_0", "windows_server_2022"),
    ];

    for (index, (bundle_id, blueprint_id)) in rows.iter().enumerate() {
        let case = *bundle_id;
        let bundle = bundles
            .iter()
            .find(|bundle| member(bundle, "bundleId") == case)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected the catalog to report it, got {:?}",
                    ids(&bundles)
                )
            });

        let name = format!("plan-reader-{index}");
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": [name],
                    "availabilityZone": "us-east-1a",
                    "blueprintId": blueprint_id,
                    "bundleId": bundle_id,
                }),
            )
            .await;
        let instance = emulator
            .ok("GetInstance", json!({"instanceName": name}))
            .await["instance"]
            .clone();

        assert_eq!(
            instance["bundleId"].as_str(),
            Some(case),
            "case {case}: the instance must report the plan it was created on, got {instance:?}"
        );
        assert_eq!(
            instance["hardware"]["cpuCount"], bundle["cpuCount"],
            "case {case}: expected the plan's vCPU count, got {instance:?}"
        );
        assert!(
            is_gb(
                instance["hardware"]["ramSizeInGb"].as_f64(),
                bundle["ramSizeInGb"].as_f64().unwrap_or_default()
            ),
            "case {case}: expected the plan's RAM, got {instance:?}"
        );
        assert_eq!(
            instance["hardware"]["disks"][0]["sizeInGb"], bundle["diskSizeInGb"],
            "case {case}: expected the plan's SSD as the system disk, got {instance:?}"
        );
        assert_eq!(
            instance["networking"]["monthlyTransfer"]["gbPerMonthAllocated"],
            bundle["transferPerMonthInGb"],
            "case {case}: expected the plan's monthly transfer, got {instance:?}"
        );
    }
}

/// Reporting the catalog is a read: it answers the same thing every time, whatever
/// else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let requests = [
        json!({}),
        json!({"includeInactive": false}),
        json!({"includeInactive": true}),
        json!({"appCategory": "LfR"}),
    ];

    let mut first = Vec::new();
    for request in &requests {
        first.push(walk(&emulator, request.clone()).await.0);
    }

    // A mutation of unrelated state, then the same reads again: the published
    // catalog is not per-caller and not affected by what the store holds.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["catalog-reader"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0"
            }),
        )
        .await;

    for (index, request) in requests.iter().enumerate() {
        let again = walk(&emulator, request.clone()).await.0;
        assert_eq!(
            again, first[index],
            "case {request:?}: two reads of the catalog must agree"
        );
    }

    // The instance the mutation created is still there: reading the catalog
    // consumed nothing.
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "catalog-reader"}))
        .await;
    assert_eq!(
        instance["instance"]["name"].as_str(),
        Some("catalog-reader"),
        "case store unchanged: expected the created instance to survive the reads, got \
         {instance:?}"
    );

    // A second emulator, with its own store, publishes the same catalog.
    let other = Emulator::new();
    for (index, request) in requests.iter().enumerate() {
        let elsewhere = walk(&other, request.clone()).await.0;
        assert_eq!(
            elsewhere, first[index],
            "case {request:?}: the catalog must not depend on which emulator serves it"
        );
    }
}
