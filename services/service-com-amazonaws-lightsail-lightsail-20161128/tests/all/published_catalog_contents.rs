// SPDX-License-Identifier: CC0-1.0

//! The published REFERENCE catalogs, driven over the wire through the same front
//! door a client uses.
//!
//! The rule under test, across every reporting operation over a catalog this
//! service publishes: the operation answers the catalog the service publishes —
//! its entries, in the order the service publishes them, each carrying the value
//! the catalog holds for every member it publishes — rather than a value derived
//! from the request or an entry the service does not publish.
//!
//! Every case is a TABLE spanning what that rule generalises over: for each
//! catalog, entries of every class it holds (both platforms of an instance plan,
//! both engines of a database blueprint, the published generation and the retired
//! one, a Region that is enabled by default and one that is opt-in), and the
//! catalog-wide ORDER and MEMBERSHIP invariants asserted over whatever the
//! catalog holds rather than over one entry written out by hand.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::instance_lifecycle::Emulator;

/// The two values of the modelled `InstancePlatform` enum
/// (`smithy_query(query="shape",shape="InstancePlatform")`).
const LINUX_UNIX: &str = "LINUX_UNIX";
const WINDOWS: &str = "WINDOWS";

/// Every entry of a reported list, over however many pages it takes.
async fn listing(emulator: &Emulator, operation: &str, member: &str, request: Value) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..50usize {
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
        assert!(
            page < 49,
            "case {operation}: the paged walk did not terminate, got {} entries",
            entries.len()
        );
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

// ---------------------------------------------------------------------------
// GetBlueprints
// ---------------------------------------------------------------------------

/// One published Windows image.
///
/// SOURCE: the Windows images the Lightsail instance-image list publishes, as the
/// recorded `GetBlueprints` answer reports them: the catalog opens with them,
/// the operating-system images newest release first and then the application
/// images that preinstall the SQL Server database engine on one, each grouped
/// under the release's own family name and carrying the plan power it needs plus
/// its vendor's product and licence pages. The rows are the whole Windows block,
/// so the family name is pinned on entries the report did not quote as well as on
/// the one it did, and they span an image the catalog names after its id's words
/// and one it names after the product it installs.
struct WindowsImage {
    blueprint_id: &'static str,
    name: &'static str,
    group: &'static str,
    blueprint_type: &'static str,
    min_power: i64,
}

const WINDOWS_IMAGES: &[WindowsImage] = &[
    WindowsImage {
        blueprint_id: "windows_server_2025",
        name: "Windows Server 2025",
        group: "windows_2025",
        blueprint_type: "os",
        min_power: WINDOWS_MIN_POWER,
    },
    WindowsImage {
        blueprint_id: "windows_server_2022",
        name: "Windows Server 2022",
        group: "windows_2022",
        blueprint_type: "os",
        min_power: WINDOWS_MIN_POWER,
    },
    WindowsImage {
        blueprint_id: "windows_server_2019",
        name: "Windows Server 2019",
        group: "windows_2019",
        blueprint_type: "os",
        min_power: WINDOWS_MIN_POWER,
    },
    WindowsImage {
        blueprint_id: "windows_server_2016",
        name: "Windows Server 2016",
        group: "windows_2016",
        blueprint_type: "os",
        min_power: WINDOWS_MIN_POWER,
    },
    WindowsImage {
        blueprint_id: "windows_server_2022_sql_2022_express",
        name: "SQL Server 2022 Express",
        group: "windows_2022_sql_exp_2022",
        blueprint_type: "app",
        // An image that installs the database engine beside the operating system
        // needs the 4 GB plan, whose power this is.
        min_power: 2_000,
    },
];

/// The minimum bundle power a Windows operating-system image requires: it does
/// not run on the smallest instance plan, so its floor is the `micro` plan's
/// power.
const WINDOWS_MIN_POWER: i64 = 500;

/// `Blueprint$version` of every entry of the published catalog: the publication
/// date of the catalog generation the images belong to, in `YYYY.MM.DD` form.
///
/// SOURCE: the recorded `GetBlueprints` answer, which reports this one version
/// across entries of different upstream releases — a Windows Server 2025
/// operating-system image and a SQL Server 2022 application image carry the same
/// one — so the member pins the catalog generation rather than the image's own
/// software release.
const CATALOG_VERSION: &str = "2026.09.17";

#[tokio::test]
async fn the_blueprint_catalog_opens_with_the_windows_images_it_publishes() {
    let emulator = Emulator::new();
    let blueprints = listing(
        &emulator,
        "GetBlueprints",
        "blueprints",
        json!({"includeInactive": true}),
    )
    .await;

    let platforms: Vec<String> = blueprints
        .iter()
        .map(|blueprint| text(blueprint, "platform"))
        .collect();
    let ids: Vec<String> = blueprints
        .iter()
        .map(|blueprint| text(blueprint, "blueprintId"))
        .collect();

    // The Windows images are a PREFIX of the listing: every entry before the
    // first Linux/Unix one is a Windows one, and none after it is.
    let windows_count = platforms.iter().take_while(|p| *p == WINDOWS).count();
    assert_eq!(
        windows_count,
        platforms.iter().filter(|p| *p == WINDOWS).count(),
        "case the whole catalog: the Windows images must be reported first, got {ids:?}"
    );
    assert_eq!(
        windows_count,
        WINDOWS_IMAGES.len(),
        "case the whole catalog: expected every published Windows image, got {ids:?}"
    );
    assert_eq!(
        ids[..windows_count],
        WINDOWS_IMAGES
            .iter()
            .map(|image| image.blueprint_id.to_string())
            .collect::<Vec<_>>()[..],
        "case the whole catalog: expected the Windows images newest release first, got {ids:?}"
    );

    for want in WINDOWS_IMAGES {
        let case = want.blueprint_id;
        let reported = blueprints
            .iter()
            .find(|blueprint| text(blueprint, "blueprintId") == case)
            .unwrap_or_else(|| panic!("case {case}: expected the catalog to publish it, got {ids:?}"));

        assert_eq!(
            reported["group"].as_str(),
            Some(want.group),
            "case {case}: expected the published family name {:?}, got {reported:?}",
            want.group
        );
        assert_eq!(
            reported["name"].as_str(),
            Some(want.name),
            "case {case}: expected the catalog's own name {:?}, got {reported:?}",
            want.name
        );
        assert_eq!(
            reported["type"].as_str(),
            Some(want.blueprint_type),
            "case {case}: expected type {:?}, got {reported:?}",
            want.blueprint_type
        );
        assert_eq!(
            reported["minPower"].as_i64(),
            Some(want.min_power),
            "case {case}: expected the minimum plan power {}, got {reported:?}",
            want.min_power
        );
        // The three members a Windows entry publishes beside its platform data:
        // the image revision, and the vendor's product and licence pages.
        for member in ["versionCode", "productUrl", "licenseUrl"] {
            assert!(
                reported[member].as_str().is_some_and(|value| !value.is_empty()),
                "case {case}: expected the published {member}, got {reported:?}"
            );
        }
    }

    // `version` is the catalog GENERATION's publication date, so every entry of
    // the listing carries it — whatever release its own software is at.
    for blueprint in &blueprints {
        let case = text(blueprint, "blueprintId");
        assert_eq!(
            blueprint["version"].as_str(),
            Some(CATALOG_VERSION),
            "case {case}: expected the catalog generation's version, got {blueprint:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetBundles
// ---------------------------------------------------------------------------

/// One published instance plan.
///
/// SOURCE: the "Instances" plans of the Lightsail pricing page, which
/// `GetBundles` answers from, at the generation the catalog publishes: the
/// Linux/Unix monthly price of each size beside the Windows price of the same
/// size where the table publishes one, the vCPU count (two up to the 8 GB plan,
/// doubling with the plan above it) and the plan power a blueprint's `minPower`
/// is compared against. The rows are every published size, so the figures are
/// pinned on the sizes the report did not quote as well as on the three it did,
/// and they span the sizes published on both platforms and the larger ones
/// published on Linux/Unix alone.
struct Plan {
    size: &'static str,
    name: &'static str,
    cpu_count: i64,
    power: i64,
    linux_price: f64,
    /// The Windows price of the size, and `None` where the plan table publishes
    /// no Windows plan of it — the sizes above the 64 GB plan, which Lightsail
    /// offers on Linux/Unix only.
    windows_price: Option<f64>,
}

const PLANS: &[Plan] = &[
    Plan {
        size: "nano",
        name: "Nano",
        cpu_count: 2,
        power: 298,
        linux_price: 5.0,
        windows_price: Some(9.5),
    },
    Plan {
        size: "micro",
        name: "Micro",
        cpu_count: 2,
        power: 500,
        linux_price: 7.0,
        windows_price: Some(16.0),
    },
    Plan {
        size: "small",
        name: "Small",
        cpu_count: 2,
        power: 1_000,
        linux_price: 12.0,
        windows_price: Some(26.0),
    },
    Plan {
        size: "medium",
        name: "Medium",
        cpu_count: 2,
        power: 2_000,
        linux_price: 24.0,
        windows_price: Some(46.0),
    },
    Plan {
        size: "large",
        name: "Large",
        cpu_count: 2,
        power: 3_000,
        linux_price: 44.0,
        windows_price: Some(86.0),
    },
    Plan {
        size: "xlarge",
        name: "Xlarge",
        cpu_count: 4,
        power: 4_000,
        linux_price: 84.0,
        windows_price: Some(166.0),
    },
    Plan {
        size: "2xlarge",
        name: "2Xlarge",
        cpu_count: 8,
        power: 5_000,
        linux_price: 164.0,
        windows_price: Some(326.0),
    },
    Plan {
        size: "4xlarge",
        name: "4Xlarge",
        cpu_count: 16,
        power: 6_000,
        linux_price: 384.0,
        windows_price: Some(646.0),
    },
    Plan {
        size: "8xlarge",
        name: "8Xlarge",
        cpu_count: 32,
        power: 7_000,
        linux_price: 884.0,
        windows_price: None,
    },
    Plan {
        size: "12xlarge",
        name: "12Xlarge",
        cpu_count: 48,
        power: 7_500,
        linux_price: 1_324.0,
        windows_price: None,
    },
    Plan {
        size: "16xlarge",
        name: "16Xlarge",
        cpu_count: 64,
        power: 8_000,
        linux_price: 1_764.0,
        windows_price: None,
    },
];

/// Whether a reported dollar figure is `expected`, to the cent.
fn is_price(reported: Option<f64>, expected: f64) -> bool {
    reported.is_some_and(|price| (price - expected).abs() < 0.005)
}

#[tokio::test]
async fn the_instance_plan_catalog_reports_the_linux_plans_before_the_windows_ones() {
    let emulator = Emulator::new();
    let bundles = listing(&emulator, "GetBundles", "bundles", json!({})).await;

    let platforms: Vec<String> = bundles
        .iter()
        .map(|bundle| bundle["supportedPlatforms"][0].as_str().unwrap_or_default().to_string())
        .collect();
    let ids: Vec<String> = bundles
        .iter()
        .map(|bundle| text(bundle, "bundleId"))
        .collect();

    // Every Linux/Unix plan precedes every Windows plan: the platform blocks do
    // not interleave.
    let linux_count = platforms.iter().take_while(|p| *p == LINUX_UNIX).count();
    assert_eq!(
        linux_count,
        platforms.iter().filter(|p| *p == LINUX_UNIX).count(),
        "case the whole catalog: the Linux/Unix plans must precede the Windows ones, got {ids:?}"
    );
    assert_eq!(
        linux_count,
        PLANS.len(),
        "case the whole catalog: expected one Linux/Unix plan per published size, got {ids:?}"
    );

    // And within each platform the sizes are the catalog's own order: the
    // Linux/Unix block carries every published size, the Windows block the sizes
    // the plan table publishes a Windows price for.
    let sizes_of = |block: &[Value]| -> Vec<String> {
        block
            .iter()
            .map(|bundle| text(bundle, "instanceType"))
            .collect()
    };
    let published_sizes: Vec<String> = PLANS.iter().map(|plan| plan.size.to_string()).collect();
    let windows_sizes: Vec<String> = PLANS
        .iter()
        .filter(|plan| plan.windows_price.is_some())
        .map(|plan| plan.size.to_string())
        .collect();
    assert_eq!(
        sizes_of(&bundles[..linux_count]),
        published_sizes,
        "case the Linux/Unix block: expected the published sizes in order, got {ids:?}"
    );
    assert_eq!(
        sizes_of(&bundles[linux_count..]),
        windows_sizes,
        "case the Windows block: expected the sizes published on Windows in order, got {ids:?}"
    );

    for plan in PLANS {
        let mut cases = vec![(
            LINUX_UNIX,
            format!("{}_3_0", plan.size),
            Some(plan.linux_price),
        )];
        cases.push((
            WINDOWS,
            format!("{}_win_3_0", plan.size),
            plan.windows_price,
        ));
        for (platform, bundle_id, price) in cases {
            let case = format!("{bundle_id} on {platform}");
            let reported = bundles
                .iter()
                .find(|bundle| text(bundle, "bundleId") == bundle_id);
            let Some(price) = price else {
                // A size the plan table does not publish on this platform has no
                // bundle at all: a plan priced like its Linux/Unix twin would
                // announce one the service does not sell.
                assert!(
                    reported.is_none(),
                    "case {case}: the catalog publishes no such plan, got {reported:?}"
                );
                continue;
            };
            let reported = reported.unwrap_or_else(|| {
                panic!("case {case}: expected the catalog to publish it, got {ids:?}")
            });
            assert_eq!(
                reported["name"].as_str(),
                Some(plan.name),
                "case {case}: expected the plan label {:?}, got {reported:?}",
                plan.name
            );
            assert_eq!(
                reported["cpuCount"].as_i64(),
                Some(plan.cpu_count),
                "case {case}: expected cpuCount {}, got {reported:?}",
                plan.cpu_count
            );
            assert_eq!(
                reported["power"].as_i64(),
                Some(plan.power),
                "case {case}: expected power {}, got {reported:?}",
                plan.power
            );
            assert!(
                is_price(reported["price"].as_f64(), price),
                "case {case}: expected price {price}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetContainerAPIMetadata
// ---------------------------------------------------------------------------

/// The lightsailctl plugin release the container API publishes — the version a
/// client compares its installed plugin against before pushing an image.
const LIGHTSAILCTL_VERSION: &str = "1.0.7";

#[tokio::test]
async fn the_container_api_publishes_the_current_lightsailctl_version() {
    let emulator = Emulator::new();
    let body = emulator.ok("GetContainerAPIMetadata", json!({})).await;
    let entries = body["metadata"]
        .as_array()
        .unwrap_or_else(|| panic!("case the metadata catalog: expected an array, got {body:?}"))
        .clone();

    let version = entries
        .iter()
        .find(|entry| entry["name"].as_str() == Some("lightsailctlVersion"))
        .unwrap_or_else(|| {
            panic!("case lightsailctlVersion: expected the published entry, got {entries:?}")
        })["value"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    assert_eq!(
        version, LIGHTSAILCTL_VERSION,
        "case lightsailctlVersion: expected the published plugin release, got {version:?}"
    );
    // A plugin release is a `major.minor.patch` version, each component a number.
    let components: Vec<&str> = version.split('.').collect();
    assert_eq!(
        components.len(),
        3,
        "case lightsailctlVersion: expected a major.minor.patch version, got {version:?}"
    );
    for component in &components {
        assert!(
            !component.is_empty() && component.chars().all(|c| c.is_ascii_digit()),
            "case lightsailctlVersion: expected numeric components, got {version:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetLoadBalancerTlsPolicies
// ---------------------------------------------------------------------------

/// One published TLS security policy: its name, the protocol tokens it enables
/// and the description that states which TLS versions those are.
///
/// SOURCE: the policies the recorded `GetLoadBalancerTlsPolicies` answer reports
/// — the default policy and the restricted forward-secrecy one — with the
/// `Protocol-` prefixed token of each version, in the recorded capitalisation.
/// The rows span a policy that enables three versions and one that enables a
/// single version.
struct Policy {
    name: &'static str,
    protocols: &'static [&'static str],
    description: &'static str,
}

const POLICIES: &[Policy] = &[
    Policy {
        name: "TLS-2016-08",
        protocols: &["Protocol-TLSv1", "Protocol-TLSV1.1", "Protocol-TLSv1.2"],
        description: "Enables TLS versions 1, 1.1, and 1.2",
    },
    Policy {
        name: "TLS-FS-1-2-Res-2019-08",
        protocols: &["Protocol-TLSv1.2"],
        description: "Enables only TLS version 1.2",
    },
];

#[tokio::test]
async fn the_tls_policy_catalog_publishes_prefixed_protocol_tokens() {
    let emulator = Emulator::new();
    let policies = listing(
        &emulator,
        "GetLoadBalancerTlsPolicies",
        "tlsPolicies",
        json!({}),
    )
    .await;

    let reported_names: BTreeSet<String> = policies
        .iter()
        .map(|policy| text(policy, "name"))
        .collect();
    let published_names: BTreeSet<String> = POLICIES
        .iter()
        .map(|policy| policy.name.to_string())
        .collect();
    assert_eq!(
        reported_names, published_names,
        "case the whole catalog: expected exactly the published policies, got {reported_names:?}"
    );

    for want in POLICIES {
        let case = want.name;
        let reported = policies
            .iter()
            .find(|policy| text(policy, "name") == case)
            .unwrap_or_else(|| panic!("case {case}: expected the catalog to publish it"));
        assert_eq!(
            reported["protocols"],
            json!(want.protocols),
            "case {case}: expected the policy's own protocol tokens {:?}, got {:?}",
            want.protocols,
            reported["protocols"]
        );
        assert_eq!(
            reported["description"].as_str(),
            Some(want.description),
            "case {case}: expected the description {:?}, got {:?}",
            want.description,
            reported["description"]
        );
    }

    // Whatever the catalog grows to hold, a protocol token is the prefixed form:
    // the bare version name is not interchangeable with it on the wire.
    for policy in &policies {
        let case = text(policy, "name");
        for protocol in policy["protocols"].as_array().unwrap_or(&Vec::new()) {
            let token = protocol.as_str().unwrap_or_default();
            assert!(
                token.starts_with("Protocol-TLS"),
                "case {case}: expected a Protocol- prefixed token, got {token:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetRegions
// ---------------------------------------------------------------------------

/// The Regions AWS leaves disabled until an account enables them, whose
/// description says so, against ones that are enabled by default.
const OPT_IN_REGIONS: &[&str] = &["ap-east-1", "ap-southeast-3", "ap-southeast-5", "eu-south-2"];

/// The clause an opt-in Region's description opens with instead of "This
/// region".
const OPT_IN_OPENING: &str = "This opt-in Region (Region that is disabled by default)";

#[tokio::test]
async fn the_region_catalog_marks_an_opt_in_region_in_its_description() {
    let emulator = Emulator::new();
    let regions = emulator.ok("GetRegions", json!({})).await["regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case the whole catalog: expected a regions array"))
        .clone();

    // Rows: Regions that are enabled by default, each with the sentence AWS
    // reports for it, and the opt-in Regions, whose sentence opens differently.
    let known: &[(&str, &str)] = &[
        (
            "eu-west-1",
            "This region is recommended to serve users in Ireland, the United Kingdom, and Iceland",
        ),
        (
            "eu-west-2",
            "This region is recommended to serve users in Ireland, the United Kingdom, and Iceland",
        ),
        (
            "eu-central-1",
            "This region is recommended to serve users in Europe, the Middle East, and Africa",
        ),
        (
            "eu-north-1",
            "This region is recommended to serve users in Stockholm and north Europe",
        ),
        (
            "eu-south-2",
            "This opt-in Region (Region that is disabled by default) is recommended to serve users \
             in Spain and South Europe.",
        ),
    ];

    for (name, expected) in known {
        let region = regions
            .iter()
            .find(|region| region["name"].as_str() == Some(*name))
            .unwrap_or_else(|| panic!("case {name}: expected the catalog to report it"));
        assert_eq!(
            region["description"].as_str(),
            Some(*expected),
            "case {name}: expected {expected:?}, got {:?}",
            region["description"]
        );
    }

    // And the opt-in clause belongs to exactly the Regions that are disabled by
    // default — it is not boilerplate on every entry.
    for region in &regions {
        let name = text(region, "name");
        let description = text(region, "description");
        assert_eq!(
            description.starts_with(OPT_IN_OPENING),
            OPT_IN_REGIONS.contains(&name.as_str()),
            "case {name}: the opt-in clause belongs to exactly the Regions disabled by default, \
             got {description:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetRelationalDatabaseBlueprints
// ---------------------------------------------------------------------------

/// One published database blueprint: the version it serves its major version at,
/// and the version description that names the engine beside it.
///
/// SOURCE: the recorded `GetRelationalDatabaseBlueprints` answer. A managed
/// PostgreSQL release is published as a revision of its community version, so its
/// version description ends with that revision label; a MySQL release carries
/// none. The rows span both engines and both forms, and every PostgreSQL series
/// the catalog publishes.
struct DatabaseBlueprint {
    blueprint_id: &'static str,
    engine_version: &'static str,
    engine_version_description: &'static str,
}

const DATABASE_BLUEPRINTS: &[DatabaseBlueprint] = &[
    DatabaseBlueprint {
        blueprint_id: "mysql_8_4",
        engine_version: "8.4.11",
        engine_version_description: "MySQL 8.4.11",
    },
    DatabaseBlueprint {
        blueprint_id: "postgres_13",
        engine_version: "13.23-rds.20260514",
        engine_version_description: "PostgreSQL 13.23-rds.20260514-R1",
    },
    DatabaseBlueprint {
        blueprint_id: "postgres_14",
        engine_version: "14.24",
        engine_version_description: "PostgreSQL 14.24-R1",
    },
    DatabaseBlueprint {
        blueprint_id: "postgres_15",
        engine_version: "15.19",
        engine_version_description: "PostgreSQL 15.19-R1",
    },
    DatabaseBlueprint {
        blueprint_id: "postgres_16",
        engine_version: "16.15",
        engine_version_description: "PostgreSQL 16.15-R1",
    },
    DatabaseBlueprint {
        blueprint_id: "postgres_17",
        engine_version: "17.11",
        engine_version_description: "PostgreSQL 17.11-R1",
    },
    DatabaseBlueprint {
        blueprint_id: "postgres_18",
        engine_version: "18.6",
        engine_version_description: "PostgreSQL 18.6-R1",
    },
];

#[tokio::test]
async fn the_database_blueprint_catalog_describes_each_version_as_its_engine_publishes_it() {
    let emulator = Emulator::new();
    let blueprints = listing(
        &emulator,
        "GetRelationalDatabaseBlueprints",
        "blueprints",
        json!({}),
    )
    .await;

    for want in DATABASE_BLUEPRINTS {
        let case = want.blueprint_id;
        let reported = blueprints
            .iter()
            .find(|blueprint| text(blueprint, "blueprintId") == case)
            .unwrap_or_else(|| panic!("case {case}: expected the catalog to publish it"));
        assert_eq!(
            reported["engineVersion"].as_str(),
            Some(want.engine_version),
            "case {case}: expected engineVersion {:?}, got {reported:?}",
            want.engine_version
        );
        assert_eq!(
            reported["engineVersionDescription"].as_str(),
            Some(want.engine_version_description),
            "case {case}: expected engineVersionDescription {:?}, got {reported:?}",
            want.engine_version_description
        );
    }

    // Whatever the catalog grows to hold: a version description names the engine
    // and then the very version that entry serves, so the two never disagree.
    for blueprint in &blueprints {
        let case = text(blueprint, "blueprintId");
        let version = text(blueprint, "engineVersion");
        let description = text(blueprint, "engineVersionDescription");
        assert!(
            description.contains(&version) && description.len() > version.len(),
            "case {case}: expected the description to name the engine beside {version:?}, got \
             {description:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetRelationalDatabaseBundles
// ---------------------------------------------------------------------------

/// The database plans the catalog publishes, in the generation it publishes them
/// at: every tier, standard and with a standby.
///
/// SOURCE: the managed-database plan table of the Lightsail pricing page, as the
/// recorded `GetRelationalDatabaseBundles` answer reports it.
const PUBLISHED_DATABASE_BUNDLES: &[(&str, &str, i64)] = &[
    ("micro_2_0", "Micro", 2),
    ("small_2_0", "Small", 2),
    ("medium_2_0", "Medium", 2),
    ("large_2_0", "Large", 2),
    ("xlarge_2_0", "Xlarge", 4),
    ("2xlarge_2_0", "2Xlarge", 8),
    ("micro_ha_2_0", "Micro with High Availability", 2),
    ("small_ha_2_0", "Small with High Availability", 2),
    ("medium_ha_2_0", "Medium with High Availability", 2),
    ("large_ha_2_0", "Large with High Availability", 2),
    ("xlarge_ha_2_0", "Xlarge with High Availability", 4),
    ("2xlarge_ha_2_0", "2Xlarge with High Availability", 8),
];
/// The generation the catalog has superseded: reported only when a call asks for
/// inactive plans, and still resolvable for a database created on it.
const RETIRED_DATABASE_BUNDLE: &str = "micro_1_0";

#[tokio::test]
async fn the_database_plan_catalog_publishes_the_current_generation() {
    let emulator = Emulator::new();

    let active = listing(
        &emulator,
        "GetRelationalDatabaseBundles",
        "bundles",
        json!({}),
    )
    .await;
    let active_ids: Vec<String> = active
        .iter()
        .map(|bundle| text(bundle, "bundleId"))
        .collect();
    assert_eq!(
        active_ids,
        PUBLISHED_DATABASE_BUNDLES
            .iter()
            .map(|(id, _, _)| (*id).to_string())
            .collect::<Vec<_>>(),
        "case the default request: expected the published generation in order, got {active_ids:?}"
    );

    for (bundle_id, name, cpu_count) in PUBLISHED_DATABASE_BUNDLES {
        let reported = active
            .iter()
            .find(|bundle| text(bundle, "bundleId") == *bundle_id)
            .unwrap_or_else(|| panic!("case {bundle_id}: expected the catalog to publish it"));
        assert_eq!(
            reported["name"].as_str(),
            Some(*name),
            "case {bundle_id}: expected the plan label {name:?}, got {reported:?}"
        );
        assert_eq!(
            reported["cpuCount"].as_i64(),
            Some(*cpu_count),
            "case {bundle_id}: expected cpuCount {cpu_count}, got {reported:?}"
        );
        assert_eq!(
            reported["isActive"].as_bool(),
            Some(true),
            "case {bundle_id}: a published plan is active, got {reported:?}"
        );
    }

    // The superseded generation is withheld from the default answer and reported
    // as inactive when asked for.
    let everything = listing(
        &emulator,
        "GetRelationalDatabaseBundles",
        "bundles",
        json!({"includeInactive": true}),
    )
    .await;
    let retired: Vec<&Value> = everything
        .iter()
        .filter(|bundle| bundle["isActive"].as_bool() == Some(false))
        .collect();
    assert!(
        !retired.is_empty(),
        "case includeInactive=true: expected the superseded generation, got {:?}",
        everything
            .iter()
            .map(|bundle| text(bundle, "bundleId"))
            .collect::<Vec<_>>()
    );
    for bundle in &retired {
        let case = text(bundle, "bundleId");
        assert!(
            !active_ids.contains(&case),
            "case {case}: a retired plan must be withheld from the default answer"
        );
    }

    // And a database created on the superseded plan still resolves its hardware:
    // that is what keeps the retired generation in the catalog.
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "retired-plan-db",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": RETIRED_DATABASE_BUNDLE,
                "masterDatabaseName": "appdb",
                "masterUsername": "dbadmin",
            }),
        )
        .await;
    let database = emulator
        .ok(
            "GetRelationalDatabase",
            json!({"relationalDatabaseName": "retired-plan-db"}),
        )
        .await["relationalDatabase"]
        .clone();
    assert_eq!(
        database["relationalDatabaseBundleId"].as_str(),
        Some(RETIRED_DATABASE_BUNDLE),
        "case {RETIRED_DATABASE_BUNDLE}: expected the database to report the plan it was created \
         on, got {database:?}"
    );
    assert_eq!(
        database["hardware"]["cpuCount"].as_i64(),
        Some(1),
        "case {RETIRED_DATABASE_BUNDLE}: expected the retired plan's own hardware, got {database:?}"
    );
}
