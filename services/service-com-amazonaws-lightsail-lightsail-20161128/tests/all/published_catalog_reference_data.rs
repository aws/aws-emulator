// SPDX-License-Identifier: CC0-1.0

//! The published reference catalogs serve the service's OWN data, driven over the
//! wire through the same front door a client uses.
//!
//! The rule under test, across every catalog this service publishes: each row is
//! the entry the service publishes and each column the value the service
//! publishes for it — the whole plan table including the sizes offered on one
//! platform only, the images a caller can select including the application images
//! named after the product they install, the catalog generation's own version
//! date, the whole geography sentence of a Region, and the price list's own
//! figures — rather than a value the emulator derived from a neighbouring row, a
//! name or a doubling rule.
//!
//! Every case is a TABLE spanning what that rule generalises over: the sizes
//! published on both platforms and the larger ones published on Linux/Unix alone,
//! an operating-system image and an application image, a Region that is enabled
//! by default and one that is opt-in, a standard database plan and its
//! high-availability twin, and both database engines. Catalog figures are
//! additionally followed THROUGH the control plane: a plan is created on and read
//! back, and the estimate charges the price the listing published.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::instance_lifecycle::Emulator;

/// The two values of the modelled `InstancePlatform` enum
/// (`smithy_query(query="shape",shape="InstancePlatform")`).
const LINUX_UNIX: &str = "LINUX_UNIX";
const WINDOWS: &str = "WINDOWS";

/// The generation token the published instance plan ids carry.
const GENERATION: &str = "3_0";

/// Every entry of a reported list, over however many pages it takes.
async fn listing(emulator: &Emulator, operation: &str, member: &str, request: Value) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..50usize {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let answer = emulator.ok(operation, body).await;
        entries.extend(
            answer[member]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {operation}: expected a {member} array, got {answer:?}")
                })
                .clone(),
        );
        match answer["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return entries,
        }
    }
    panic!("case {operation}: the paged walk did not terminate")
}

/// A string member of a reported entry, or a panic naming the case.
fn text(entry: &Value, member: &str) -> String {
    entry[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {entry:?}: expected a string {member}"))
        .to_string()
}

/// Whether a reported dollar figure is `expected`, to the cent.
fn is_price(reported: Option<f64>, expected: f64) -> bool {
    reported.is_some_and(|price| (price - expected).abs() < 0.005)
}

// ---------------------------------------------------------------------------
// GetBundles — the instance plan table
// ---------------------------------------------------------------------------

/// One row of the published instance plan table.
///
/// SOURCE: the "Instances" plans of the Lightsail pricing page, which
/// `GetBundles` answers from: the hardware, the included monthly transfer and the
/// plan power of each size, the Linux/Unix monthly price, and the Windows price of
/// the same size where the table publishes a Windows plan of it — which it does up
/// to the 64 GB plan, above which the sizes are offered on Linux/Unix only.
///
/// The rows are every published size, so they span a size published on both
/// platforms and the larger sizes published on one, and the figures are pinned on
/// sizes no report quoted as well as on the ones it did.
struct PlanRow {
    size: &'static str,
    cpu_count: i64,
    ram_size_in_gb: f64,
    disk_size_in_gb: i64,
    transfer_per_month_in_gb: i64,
    power: i64,
    linux_price: f64,
    windows_price: Option<f64>,
}

const PLAN_TABLE: &[PlanRow] = &[
    PlanRow {
        size: "nano",
        cpu_count: 2,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_per_month_in_gb: 1_024,
        power: 298,
        linux_price: 5.0,
        windows_price: Some(9.5),
    },
    PlanRow {
        size: "micro",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_per_month_in_gb: 2_048,
        power: 500,
        linux_price: 7.0,
        windows_price: Some(16.0),
    },
    PlanRow {
        size: "small",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 60,
        transfer_per_month_in_gb: 3_072,
        power: 1_000,
        linux_price: 12.0,
        windows_price: Some(26.0),
    },
    PlanRow {
        size: "medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 80,
        transfer_per_month_in_gb: 4_096,
        power: 2_000,
        linux_price: 24.0,
        windows_price: Some(46.0),
    },
    PlanRow {
        size: "large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 160,
        transfer_per_month_in_gb: 5_120,
        power: 3_000,
        linux_price: 44.0,
        windows_price: Some(86.0),
    },
    PlanRow {
        size: "xlarge",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 320,
        transfer_per_month_in_gb: 6_144,
        power: 4_000,
        linux_price: 84.0,
        windows_price: Some(166.0),
    },
    PlanRow {
        size: "2xlarge",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 640,
        transfer_per_month_in_gb: 7_168,
        power: 5_000,
        linux_price: 164.0,
        windows_price: Some(326.0),
    },
    PlanRow {
        size: "4xlarge",
        cpu_count: 16,
        ram_size_in_gb: 64.0,
        disk_size_in_gb: 1_280,
        transfer_per_month_in_gb: 8_192,
        power: 6_000,
        linux_price: 384.0,
        windows_price: Some(646.0),
    },
    PlanRow {
        size: "8xlarge",
        cpu_count: 32,
        ram_size_in_gb: 128.0,
        disk_size_in_gb: 2_560,
        transfer_per_month_in_gb: 9_216,
        power: 7_000,
        linux_price: 884.0,
        windows_price: None,
    },
    PlanRow {
        size: "12xlarge",
        cpu_count: 48,
        ram_size_in_gb: 192.0,
        disk_size_in_gb: 3_840,
        transfer_per_month_in_gb: 10_240,
        power: 7_500,
        linux_price: 1_324.0,
        windows_price: None,
    },
    PlanRow {
        size: "16xlarge",
        cpu_count: 64,
        ram_size_in_gb: 256.0,
        disk_size_in_gb: 5_120,
        transfer_per_month_in_gb: 11_264,
        power: 8_000,
        linux_price: 1_764.0,
        windows_price: None,
    },
];

/// The plan this listing reports under `bundle_id`, or `None`.
fn plan_of(bundles: &[Value], bundle_id: &str) -> Option<Value> {
    bundles
        .iter()
        .find(|bundle| text(bundle, "bundleId") == bundle_id)
        .cloned()
}

#[tokio::test]
async fn the_plan_catalog_reports_every_size_of_the_plan_table_with_its_own_figures() {
    let emulator = Emulator::new();
    let bundles = listing(&emulator, "GetBundles", "bundles", json!({})).await;

    for row in PLAN_TABLE {
        let linux_id = format!("{}_{GENERATION}", row.size);
        let case = format!("{linux_id} on {LINUX_UNIX}");
        let reported = plan_of(&bundles, &linux_id).unwrap_or_else(|| {
            panic!("case {case}: expected the published plan, got {bundles:?}")
        });

        for (member, want, got) in [
            ("cpuCount", row.cpu_count, reported["cpuCount"].as_i64()),
            (
                "diskSizeInGb",
                row.disk_size_in_gb,
                reported["diskSizeInGb"].as_i64(),
            ),
            (
                "transferPerMonthInGb",
                row.transfer_per_month_in_gb,
                reported["transferPerMonthInGb"].as_i64(),
            ),
            ("power", row.power, reported["power"].as_i64()),
        ] {
            assert_eq!(
                got,
                Some(want),
                "case {case}: expected {member} {want}, got {got:?}"
            );
        }
        assert!(
            reported["ramSizeInGb"]
                .as_f64()
                .is_some_and(|ram| (ram - row.ram_size_in_gb).abs() < 0.001),
            "case {case}: expected {} GB of RAM, got {:?}",
            row.ram_size_in_gb,
            reported["ramSizeInGb"]
        );
        assert!(
            is_price(reported["price"].as_f64(), row.linux_price),
            "case {case}: expected the plan table's price {}, got {:?}",
            row.linux_price,
            reported["price"]
        );
        assert_eq!(
            reported["instanceType"].as_str(),
            Some(row.size),
            "case {case}: expected the size token, got {reported:?}"
        );

        // The Windows plan of the size exists exactly where the table publishes a
        // Windows price for it, carries the SAME hardware, and costs the table's
        // Windows figure.
        let windows_id = format!("{}_win_{GENERATION}", row.size);
        let windows_case = format!("{windows_id} on {WINDOWS}");
        let windows = plan_of(&bundles, &windows_id);
        match row.windows_price {
            None => assert!(
                windows.is_none(),
                "case {windows_case}: the plan table publishes no Windows plan of this size, so \
                 the catalog must report none, got {windows:?}"
            ),
            Some(price) => {
                let windows = windows.unwrap_or_else(|| {
                    panic!("case {windows_case}: expected the published Windows plan")
                });
                assert!(
                    is_price(windows["price"].as_f64(), price),
                    "case {windows_case}: expected the plan table's Windows price {price}, got {:?}",
                    windows["price"]
                );
                for member in ["cpuCount", "ramSizeInGb", "diskSizeInGb", "power"] {
                    assert_eq!(
                        windows[member], reported[member],
                        "case {windows_case}: the two platforms of a size share their {member}, \
                         got {windows:?}"
                    );
                }
            }
        }
    }

    // And the listing holds nothing BEYOND the table: every reported plan is a
    // published size on a platform the table prices it on.
    let published: Vec<String> = PLAN_TABLE
        .iter()
        .flat_map(|row| {
            let mut ids = vec![format!("{}_{GENERATION}", row.size)];
            if row.windows_price.is_some() {
                ids.push(format!("{}_win_{GENERATION}", row.size));
            }
            ids
        })
        .collect();
    let reported: Vec<String> = bundles
        .iter()
        .map(|bundle| text(bundle, "bundleId"))
        .collect();
    for bundle_id in &reported {
        assert!(
            published.contains(bundle_id),
            "case {bundle_id}: expected a plan of the published table, got {reported:?}"
        );
    }
    assert_eq!(
        reported.len(),
        published.len(),
        "case the whole catalog: expected every published plan once, got {reported:?}"
    );
}

/// The hours AWS bills one Lightsail monthly plan over: a monthly plan is charged
/// by the hour at `price / 730`, so a window of 730 hours is one plan's worth.
const BILLING_MONTH_HOURS: f64 = 730.0;

/// How far ahead of now a cost-estimate window opens, so the whole window lies in
/// the estimate's own future and no part of it is clipped.
const WINDOW_OPENS_IN_SECONDS: f64 = 86_400.0;

/// Now, as the epoch seconds `GetCostEstimate` takes.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A plan is not only listed: an instance runs on it, reports the plan's hardware
/// and is billed at the plan's published price.
///
/// The rows are the four largest plans — the ones a sizing or cost calculation
/// over the big end of the table reaches — and they span a size published on both
/// platforms and the three published on Linux/Unix alone.
#[tokio::test]
async fn an_instance_runs_on_the_largest_published_plans_at_their_published_price() {
    let emulator = Emulator::new();
    let bundles = listing(&emulator, "GetBundles", "bundles", json!({})).await;

    for row in PLAN_TABLE
        .iter()
        .filter(|row| ["4xlarge", "8xlarge", "12xlarge", "16xlarge"].contains(&row.size))
    {
        let bundle_id = format!("{}_{GENERATION}", row.size);
        let name = format!("plan-{}", row.size);
        let case = bundle_id.clone();
        emulator
            .ok(
                "CreateInstances",
                json!({
                    "instanceNames": [name],
                    "availabilityZone": "us-east-1a",
                    "blueprintId": "amazon_linux_2023",
                    "bundleId": bundle_id,
                }),
            )
            .await;

        let instance = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();
        assert_eq!(
            instance["bundleId"].as_str(),
            Some(bundle_id.as_str()),
            "case {case}: expected the instance to run on the plan it was created on, \
             got {instance:?}"
        );
        let hardware = &instance["hardware"];
        assert_eq!(
            hardware["cpuCount"].as_i64(),
            Some(row.cpu_count),
            "case {case}: expected the plan's vCPU count, got {hardware:?}"
        );
        assert!(
            hardware["ramSizeInGb"]
                .as_f64()
                .is_some_and(|ram| (ram - row.ram_size_in_gb).abs() < 0.001),
            "case {case}: expected the plan's RAM, got {hardware:?}"
        );
        assert_eq!(
            hardware["disks"][0]["sizeInGb"].as_i64(),
            Some(row.disk_size_in_gb),
            "case {case}: expected the plan's SSD size, got {hardware:?}"
        );

        // The listing's price is the price the plan is billed at: one billing
        // month of the plan costs exactly what the catalog published.
        let start = now_epoch_seconds() + WINDOW_OPENS_IN_SECONDS;
        let end = start + BILLING_MONTH_HOURS * 3_600.0;
        let body = emulator
            .ok(
                "GetCostEstimate",
                json!({"resourceName": name, "startTime": start, "endTime": end}),
            )
            .await;
        let charge = body["resourcesBudgetEstimate"][0]["costEstimates"][0]["resultsByTime"][0]
            .clone();
        assert!(
            is_price(charge["usageCost"].as_f64(), row.linux_price),
            "case {case}: expected a billing month to cost the published {}, got {charge:?}",
            row.linux_price
        );
        assert!(
            is_price(
                plan_of(&bundles, &bundle_id)
                    .and_then(|plan| plan["price"].as_f64()),
                row.linux_price
            ),
            "case {case}: the listed price and the billed price must be one figure"
        );
    }
}

// ---------------------------------------------------------------------------
// GetBlueprints — the instance image catalog
// ---------------------------------------------------------------------------

/// The Windows SQL Server application image the catalog publishes.
///
/// SOURCE: the recorded `GetBlueprints` answer — blueprintId
/// `windows_server_2022_sql_2022_express`, named after the product it installs,
/// grouped `windows_2022_sql_exp_2022`, of type `app` on the `WINDOWS` platform,
/// and needing the 4 GB plan's power because it installs a database engine beside
/// the operating system.
const SQL_IMAGE_ID: &str = "windows_server_2022_sql_2022_express";
const SQL_IMAGE_NAME: &str = "SQL Server 2022 Express";
const SQL_IMAGE_GROUP: &str = "windows_2022_sql_exp_2022";

#[tokio::test]
async fn the_windows_sql_server_image_is_published_and_can_be_created_from() {
    let emulator = Emulator::new();
    let blueprints = listing(&emulator, "GetBlueprints", "blueprints", json!({})).await;
    let bundles = listing(&emulator, "GetBundles", "bundles", json!({})).await;

    let position = blueprints
        .iter()
        .position(|blueprint| text(blueprint, "blueprintId") == SQL_IMAGE_ID)
        .unwrap_or_else(|| {
            panic!(
                "case {SQL_IMAGE_ID}: expected the catalog to publish it, got {:?}",
                blueprints
                    .iter()
                    .map(|blueprint| text(blueprint, "blueprintId"))
                    .collect::<Vec<_>>()
            )
        });
    let reported = &blueprints[position];

    // It is published inside the Windows block: every entry up to and including it
    // is a Windows image, so it precedes the first Linux/Unix one.
    for earlier in &blueprints[..=position] {
        assert_eq!(
            text(earlier, "platform"),
            WINDOWS,
            "case {SQL_IMAGE_ID}: expected the Windows images to be published first, \
             got {earlier:?}"
        );
    }

    assert_eq!(
        reported["name"].as_str(),
        Some(SQL_IMAGE_NAME),
        "case {SQL_IMAGE_ID}: expected the catalog's name for the product, got {reported:?}"
    );
    assert_eq!(
        reported["group"].as_str(),
        Some(SQL_IMAGE_GROUP),
        "case {SQL_IMAGE_ID}: expected the published family name, got {reported:?}"
    );
    assert_eq!(
        reported["type"].as_str(),
        Some("app"),
        "case {SQL_IMAGE_ID}: an image that preinstalls a product is an app, got {reported:?}"
    );

    // Its minimum is the power of the plan with 4 GB of RAM — read from the plan
    // listing, so the floor names a plan a caller can actually select.
    let four_gb_power = bundles
        .iter()
        .find(|bundle| {
            bundle["ramSizeInGb"].as_f64().is_some_and(|ram| (ram - 4.0).abs() < 0.001)
        })
        .and_then(|bundle| bundle["power"].as_i64())
        .unwrap_or_else(|| panic!("case {SQL_IMAGE_ID}: expected a 4 GB plan in the listing"));
    assert_eq!(
        reported["minPower"].as_i64(),
        Some(four_gb_power),
        "case {SQL_IMAGE_ID}: expected the 4 GB plan's power as the floor, got {reported:?}"
    );

    // And it can be selected: an instance created from it reports the image and
    // the catalog's own name for it.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["sql-express"],
                "availabilityZone": "us-east-1a",
                "blueprintId": SQL_IMAGE_ID,
                "bundleId": "medium_win_3_0",
            }),
        )
        .await;
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "sql-express"}))
        .await["instance"]
        .clone();
    assert_eq!(
        instance["blueprintId"].as_str(),
        Some(SQL_IMAGE_ID),
        "case {SQL_IMAGE_ID}: expected the instance to name its image, got {instance:?}"
    );
    assert_eq!(
        instance["blueprintName"].as_str(),
        Some(SQL_IMAGE_NAME),
        "case {SQL_IMAGE_ID}: expected the catalog's own name on the instance, got {instance:?}"
    );
}

/// Whether `version` is a catalog version: the `YYYY.MM.DD` publication date of a
/// catalog generation.
fn is_catalog_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// `Blueprint$version` is the CATALOG generation's date, so it is one value across
/// the whole listing — an image's own software release is not what it carries.
///
/// The rows are the two listings the request can ask for: the active images and
/// every published image, which must agree on the generation.
#[tokio::test]
async fn every_published_image_carries_the_catalog_generations_version() {
    let emulator = Emulator::new();

    let mut generations: Vec<String> = Vec::new();
    for request in [json!({}), json!({"includeInactive": true})] {
        let case = request.to_string();
        let blueprints = listing(&emulator, "GetBlueprints", "blueprints", request).await;
        assert!(
            !blueprints.is_empty(),
            "case {case}: expected the catalog to report images"
        );
        for blueprint in &blueprints {
            let id = text(blueprint, "blueprintId");
            let version = text(blueprint, "version");
            assert!(
                is_catalog_version(&version),
                "case {case}: expected {id} to carry the generation's YYYY.MM.DD date, \
                 got {version:?}"
            );
            assert_eq!(
                version,
                text(&blueprints[0], "version"),
                "case {case}: expected one version across the generation, got {id} at {version:?}"
            );
        }
        generations.push(text(&blueprints[0], "version"));
    }
    assert_eq!(
        generations[0], generations[1],
        "case both listings: the generation does not depend on which images are asked for, \
         got {generations:?}"
    );
}

// ---------------------------------------------------------------------------
// GetRegions — the Region catalog
// ---------------------------------------------------------------------------

/// The sentence stem `Region$description` documents, and the stem an OPT-IN
/// Region's description carries instead.
const DESCRIPTION_STEM: &str = "This region is recommended to serve users in ";
const OPT_IN_STEM: &str =
    "This opt-in Region (Region that is disabled by default) is recommended to serve users in ";

/// The Regions AWS leaves disabled until an account enables them, whose
/// description says so and closes with a period.
const OPT_IN_REGIONS: &[&str] = &["ap-east-1", "ap-southeast-3", "ap-southeast-5", "eu-south-2"];

/// The Regions whose description names a geography WIDER than the place the
/// Region is named after — AWS words the clause as the territory plus the area it
/// serves, joined by "and".
///
/// SOURCE: the recorded `GetRegions` answer for each of these Regions. The rows
/// are every Region AWS gives such a clause, so the rule is pinned on Regions no
/// report quoted as well as on the ones it did.
const WIDER_GEOGRAPHY_REGIONS: &[&str] = &[
    "ap-east-1",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-5",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "sa-east-1",
    "us-west-2",
];

#[tokio::test]
async fn a_region_description_is_the_whole_sentence_aws_publishes_for_it() {
    let emulator = Emulator::new();
    let regions = emulator.ok("GetRegions", json!({})).await["regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case the whole catalog: expected a regions array"))
        .clone();

    for region in &regions {
        let case = text(region, "name");
        let description = text(region, "description");
        let display_name = text(region, "displayName");
        let is_opt_in = OPT_IN_REGIONS.contains(&case.as_str());
        let stem = if is_opt_in { OPT_IN_STEM } else { DESCRIPTION_STEM };

        let clause = description.strip_prefix(stem).unwrap_or_else(|| {
            panic!("case {case}: expected the sentence to open with {stem:?}, got {description:?}")
        });
        // An opt-in Region's sentence is punctuated; a default-enabled one's is
        // not, and the clause is never empty either way.
        assert_eq!(
            clause.ends_with('.'),
            is_opt_in,
            "case {case}: only an opt-in Region's sentence closes with a period, \
             got {description:?}"
        );
        let clause = clause.trim_end_matches('.');
        assert!(
            !clause.is_empty(),
            "case {case}: expected the geography the Region serves, got {description:?}"
        );
        assert_ne!(
            clause, display_name,
            "case {case}: the clause must name the geography the Region serves rather than \
             restate its display name, got {description:?}"
        );
        if WIDER_GEOGRAPHY_REGIONS.contains(&case.as_str()) {
            assert!(
                clause.contains(" and "),
                "case {case}: expected AWS's wider geography clause beside the territory, \
                 got {description:?}"
            );
        }
    }

    // Every Region AWS gives a wider-geography clause is served by this catalog,
    // so none of those rows goes unchecked.
    let served: Vec<String> = regions.iter().map(|region| text(region, "name")).collect();
    for name in WIDER_GEOGRAPHY_REGIONS {
        assert!(
            served.contains(&(*name).to_string()),
            "case {name}: expected the catalog to serve this Region, got {served:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetRelationalDatabaseBundles — the managed-database plan table
// ---------------------------------------------------------------------------

/// One row of the published managed-database plan table: the standard plan's
/// monthly price, and the hardware the high-availability twin of the same size
/// shares with it.
///
/// SOURCE: the managed-database plan table of the Lightsail pricing page. Each
/// price is the table's own figure for that plan — the table charges $115, $245
/// and $490 for the three largest, which no doubling rule produces — and the
/// high-availability column sells the same hardware with a standby for twice the
/// standard plan's price. The rows are every published size.
struct DatabasePlanRow {
    bundle_id: &'static str,
    ha_bundle_id: &'static str,
    price: f64,
}

const DATABASE_PLAN_TABLE: &[DatabasePlanRow] = &[
    DatabasePlanRow {
        bundle_id: "micro_2_0",
        ha_bundle_id: "micro_ha_2_0",
        price: 15.0,
    },
    DatabasePlanRow {
        bundle_id: "small_2_0",
        ha_bundle_id: "small_ha_2_0",
        price: 30.0,
    },
    DatabasePlanRow {
        bundle_id: "medium_2_0",
        ha_bundle_id: "medium_ha_2_0",
        price: 60.0,
    },
    DatabasePlanRow {
        bundle_id: "large_2_0",
        ha_bundle_id: "large_ha_2_0",
        price: 115.0,
    },
    DatabasePlanRow {
        bundle_id: "xlarge_2_0",
        ha_bundle_id: "xlarge_ha_2_0",
        price: 245.0,
    },
    DatabasePlanRow {
        bundle_id: "2xlarge_2_0",
        ha_bundle_id: "2xlarge_ha_2_0",
        price: 490.0,
    },
];

#[tokio::test]
async fn the_database_plan_catalog_charges_the_price_list_and_twice_it_for_a_standby() {
    let emulator = Emulator::new();
    let bundles = listing(
        &emulator,
        "GetRelationalDatabaseBundles",
        "bundles",
        json!({}),
    )
    .await;

    let of = |bundle_id: &str| -> Value {
        bundles
            .iter()
            .find(|bundle| text(bundle, "bundleId") == bundle_id)
            .unwrap_or_else(|| panic!("case {bundle_id}: expected the published plan"))
            .clone()
    };

    for row in DATABASE_PLAN_TABLE {
        let standard = of(row.bundle_id);
        let high_availability = of(row.ha_bundle_id);
        assert!(
            is_price(standard["price"].as_f64(), row.price),
            "case {}: expected the price list's {}, got {:?}",
            row.bundle_id,
            row.price,
            standard["price"]
        );
        assert!(
            is_price(high_availability["price"].as_f64(), row.price * 2.0),
            "case {}: a standby doubles the standard plan's price, expected {}, got {:?}",
            row.ha_bundle_id,
            row.price * 2.0,
            high_availability["price"]
        );
        for member in [
            "cpuCount",
            "ramSizeInGb",
            "diskSizeInGb",
            "transferPerMonthInGb",
        ] {
            assert_eq!(
                high_availability[member], standard[member],
                "case {}: the standby plan sells the same {member}, got {high_availability:?}",
                row.ha_bundle_id
            );
        }
    }

    // Whatever the catalog grows to hold: the plans grow with their price, so no
    // larger plan is cheaper than a smaller one.
    let prices: Vec<f64> = DATABASE_PLAN_TABLE.iter().map(|row| row.price).collect();
    for pair in prices.windows(2) {
        assert!(
            pair[1] > pair[0],
            "case the price list: expected the plans to grow with their price, got {prices:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetRelationalDatabaseBlueprints — the managed-database engine catalog
// ---------------------------------------------------------------------------

/// Each published database blueprint serves ONE patch release of the major
/// version its id names, and says so in its version description.
///
/// The rows are every published blueprint, so both engines and both description
/// forms (with and without the managed-release revision label) are covered.
#[tokio::test]
async fn each_database_blueprint_serves_a_patch_release_of_the_major_version_it_names() {
    let emulator = Emulator::new();
    let blueprints = listing(
        &emulator,
        "GetRelationalDatabaseBlueprints",
        "blueprints",
        json!({}),
    )
    .await;

    assert!(
        !blueprints.is_empty(),
        "case the whole catalog: expected the catalog to publish blueprints"
    );

    for blueprint in &blueprints {
        let case = text(blueprint, "blueprintId");
        let engine = text(blueprint, "engine");
        let engine_description = text(blueprint, "engineDescription");
        let engine_version = text(blueprint, "engineVersion");
        let version_description = text(blueprint, "engineVersionDescription");

        // The id is the engine and the major version it describes, so the version
        // the entry serves is a release OF that major version.
        let major = case
            .strip_prefix(&format!("{engine}_"))
            .unwrap_or_else(|| panic!("case {case}: expected the id to name its engine {engine}"))
            .replace('_', ".");
        assert!(
            engine_version.starts_with(&format!("{major}.")),
            "case {case}: expected a release of {major}, got {engine_version:?}"
        );

        // The version description names the engine and then the very version this
        // entry serves — a managed release adds the revision label it is published
        // at — so the description and the version never disagree.
        let named_version = version_description
            .strip_suffix("-R1")
            .unwrap_or(&version_description);
        assert!(
            named_version.ends_with(&engine_version)
                && named_version.len() > engine_version.len(),
            "case {case}: expected the description to name the engine beside {engine_version:?}, \
             got {version_description:?}"
        );
        assert!(
            !engine_description.is_empty(),
            "case {case}: expected the published engine description, got {blueprint:?}"
        );
    }

    // Exactly one blueprint per engine is that engine's default: the version a
    // database is created at when the caller names none.
    for engine in ["mysql", "postgres"] {
        let defaults: Vec<String> = blueprints
            .iter()
            .filter(|blueprint| {
                text(blueprint, "engine") == engine
                    && blueprint["isEngineDefault"].as_bool() == Some(true)
            })
            .map(|blueprint| text(blueprint, "blueprintId"))
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "case {engine}: expected exactly one default version, got {defaults:?}"
        );
    }
}
