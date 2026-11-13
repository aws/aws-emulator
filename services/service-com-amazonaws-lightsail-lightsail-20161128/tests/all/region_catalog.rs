// SPDX-License-Identifier: CC0-1.0

//! `GetRegions` — the Region catalog, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRegions` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson envelope are exercised rather than bypassed.
//!
//! The rules under test, each asserted over a TABLE rather than over one call:
//!
//! * the catalog is the Regions Lightsail SERVES, each reported once, in one
//!   stable order that opens with a US Region — a Region name the modelled
//!   `RegionName` enum admits but the service does not operate in (`us-west-1`)
//!   is not reported at all;
//! * every reported Region describes itself: display name, the documented
//!   description sentence — opened by the opt-in clause for a Region that is
//!   disabled by default — and the continent code its name's geography implies;
//! * both Availability Zone list members are ALWAYS present — empty, never
//!   absent — whatever the two include flags say; and
//! * a zone list carries zones only for the Region the request was served in,
//!   and then exactly that Region's published zones; every other Region answers
//!   an empty list.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// Every Region name the model declares: `smithy_query(query="shape",
/// shape="RegionName")`, generated into the service crate as
/// `REGION_NAME_VALUES`. It is the set of names a `ResourceLocation` may carry,
/// which is WIDER than the catalog `GetRegions` reports.
const MODELLED_REGION_NAMES: &[&str] =
    service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// One served Region: `(name, displayName, the letters of its Availability
/// Zones)`, in the order `GetRegions` reports them.
///
/// SOURCE: the Regions the Lightsail user guide's "Regions and Availability
/// Zones in Amazon Lightsail" lists, with the zones it lists for each — six in
/// `us-east-1`, four in `us-west-2`, two in `ca-central-1`, and `a`, `c`, `d` in
/// `ap-northeast-1`, which publishes no `b`. `us-west-1` is absent: Lightsail
/// does not operate there, even though the modelled enum admits the name.
/// Written out here independently of the service's own `REGION_SPECS`, so these
/// rows pin the published catalog rather than restating whatever the code holds.
const SERVED_REGIONS: &[(&str, &str, &str)] = &[
    ("us-east-1", "Virginia", "abcdef"),
    ("us-east-2", "Ohio", "abc"),
    ("us-west-2", "Oregon", "abcd"),
    ("ap-south-1", "Mumbai", "abc"),
    ("ap-northeast-2", "Seoul", "abc"),
    ("ap-southeast-1", "Singapore", "abc"),
    ("ap-southeast-2", "Sydney", "abc"),
    ("ap-northeast-1", "Tokyo", "acd"),
    ("ca-central-1", "Montreal", "ab"),
    ("eu-central-1", "Frankfurt", "abc"),
    ("eu-west-1", "Ireland", "abc"),
    ("eu-west-2", "London", "abc"),
    ("eu-west-3", "Paris", "abc"),
    ("eu-north-1", "Stockholm", "abc"),
    ("sa-east-1", "Sao Paulo", "abc"),
    ("ap-east-1", "Hong Kong", "abc"),
    ("eu-south-2", "Spain", "abc"),
    ("ap-southeast-3", "Jakarta", "abc"),
    ("ap-southeast-5", "Malaysia", "abc"),
];

/// The sentence stem `Region$description` documents ("This region is recommended
/// to serve users in the eastern United States and eastern Canada").
const DESCRIPTION_STEM: &str = "This region is recommended to serve users in ";

/// The stem the description of an OPT-IN Region carries instead — a Region AWS
/// leaves disabled until an account enables it, which this response says in its
/// description and nowhere else.
const OPT_IN_DESCRIPTION_STEM: &str =
    "This opt-in Region (Region that is disabled by default) is recommended to serve users in ";

/// The Regions of [`SERVED_REGIONS`] that are opt-in Regions: the ones
/// introduced after the opt-in mechanism was published, which an account has to
/// enable before it can use them.
const OPT_IN_REGIONS: &[&str] = &["ap-east-1", "ap-southeast-3", "ap-southeast-5", "eu-south-2"];

/// The stem the description of `name` opens with.
fn description_stem(name: &str) -> &'static str {
    if OPT_IN_REGIONS.contains(&name) {
        OPT_IN_DESCRIPTION_STEM
    } else {
        DESCRIPTION_STEM
    }
}

/// The clause the description of `name` closes with: an opt-in Region's sentence
/// is punctuated with a period, a default-enabled Region's is not.
fn description_close(name: &str) -> &'static str {
    if OPT_IN_REGIONS.contains(&name) {
        "."
    } else {
        ""
    }
}

/// The two zone list members of the modelled `Region`, with the request member
/// that asks for each.
const ZONE_LISTS: &[(&str, &str)] = &[
    ("availabilityZones", "includeAvailabilityZones"),
    (
        "relationalDatabaseAvailabilityZones",
        "includeRelationalDatabaseAvailabilityZones",
    ),
];

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `GetRegions` with `body`, in `region` when one is given, answering
    /// its HTTP status and parsed body.
    async fn call(&self, region: Option<&str>, body: Value) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.GetRegions"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `GetRegions` with `body`, required to succeed, answering its `regions`.
    async fn regions(&self, case: &str, region: Option<&str>, body: Value) -> Vec<Value> {
        let (status, parsed) = self.call(region, body).await;
        assert_eq!(
            status, 200,
            "case {case}: GetRegions must succeed, got {status} {parsed:?}"
        );
        parsed["regions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a regions array, got {parsed:?}"))
            .clone()
    }

    /// The reported Regions keyed by `name`, with the report order beside them.
    async fn keyed(
        &self,
        case: &str,
        region: Option<&str>,
        body: Value,
    ) -> (BTreeMap<String, Value>, Vec<String>) {
        let regions = self.regions(case, region, body).await;
        let order = reported_names(case, &regions);
        let mut keyed = BTreeMap::new();
        for (name, entry) in order.iter().zip(regions) {
            assert!(
                keyed.insert(name.clone(), entry).is_none(),
                "case {case}: region {name:?} was reported twice"
            );
        }
        (keyed, order)
    }
}

/// The `name` of each reported Region, in report order.
fn reported_names(case: &str, regions: &[Value]) -> Vec<String> {
    regions
        .iter()
        .map(|region| {
            region["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a region name, got {region:?}"))
                .to_string()
        })
        .collect()
}

/// The request body asking for the zone lists `instance_zones` /
/// `database_zones` select, in the `form` the flags take (absent, or explicit).
fn request_body(form: &str, instance_zones: bool, database_zones: bool) -> Value {
    let mut body = json!({});
    for (wanted, member) in [
        (instance_zones, "includeAvailabilityZones"),
        (
            database_zones,
            "includeRelationalDatabaseAvailabilityZones",
        ),
    ] {
        match form {
            // Every flag spelled out, true or false.
            "explicit" => body[member] = json!(wanted),
            // Only the flags that ask for something are sent at all.
            _ if wanted => body[member] = json!(true),
            _ => {}
        }
    }
    body
}

/// The zone names `region` reports in `member`.
fn zone_names(case: &str, region: &Value, member: &str) -> Vec<String> {
    let zones = region[member].as_array().unwrap_or_else(|| {
        panic!("case {case}: {member} must always be present as a list, got {region:?}")
    });
    for zone in zones {
        assert_eq!(
            zone["state"].as_str(),
            Some("available"),
            "case {case}: expected a serving zone state in {member}, got {zone:?}"
        );
    }
    zones
        .iter()
        .map(|zone| {
            zone["zoneName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a zoneName, got {zone:?}"))
                .to_string()
        })
        .collect()
}

/// The zones Region `name` publishes, as names, from [`SERVED_REGIONS`].
fn published_zones(name: &str) -> Vec<String> {
    SERVED_REGIONS
        .iter()
        .find(|(region, _, _)| *region == name)
        .map(|(region, _, letters)| {
            letters
                .chars()
                .map(|letter| format!("{region}{letter}"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every Region the service serves is reported exactly once, in one stable
/// order that opens with a US Region, and a Region name the model admits but the
/// service does not operate in is not reported at all.
#[tokio::test]
async fn the_catalog_is_the_regions_the_service_serves() {
    let emulator = Emulator::new();
    let (_, order) = emulator
        .keyed("the whole catalog", Some(DEFAULT_REGION), json!({}))
        .await;

    let expected: Vec<&str> = SERVED_REGIONS.iter().map(|(name, _, _)| *name).collect();
    assert_eq!(
        order, expected,
        "case the whole catalog: expected every served region once, in report order, got {order:?}"
    );

    // The order AWS reports is US first, so a client that reads `regions[0]` as
    // its home region gets a US one.
    assert_eq!(
        order.first().map(String::as_str),
        Some("us-east-1"),
        "case the whole catalog: expected the first entry to be us-east-1, got {order:?}"
    );

    // Every reported name is a modelled `RegionName`, and every modelled name
    // the service does not serve is absent — `us-west-1` is such a name.
    let reported: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    let modelled: BTreeSet<&str> = MODELLED_REGION_NAMES.iter().copied().collect();
    assert!(
        reported.is_subset(&modelled),
        "case the whole catalog: every reported region must be a modelled RegionName, got {reported:?}"
    );
    // `us-west-1` is such a name: the model admits it, the service does not
    // operate there, so nothing may report it.
    assert!(
        modelled.contains("us-west-1"),
        "case us-west-1: the model is expected to admit this name, got {modelled:?}"
    );
    assert!(
        !reported.contains("us-west-1"),
        "case us-west-1: the service does not operate there, so it must not be reported, got {reported:?}"
    );
}

/// Every reported Region describes itself: the display name of the place it is
/// named after, the documented description sentence, and the continent code its
/// name's geography implies.
#[tokio::test]
async fn every_reported_region_describes_itself() {
    let emulator = Emulator::new();
    let (regions, _) = emulator
        .keyed("region members", Some(DEFAULT_REGION), json!({}))
        .await;

    for (name, display_name, _) in SERVED_REGIONS {
        let region = regions
            .get(*name)
            .unwrap_or_else(|| panic!("case {name}: expected this served region to be reported"));

        assert_eq!(
            region["displayName"].as_str(),
            Some(*display_name),
            "case {name}: expected displayName {display_name:?}, got {:?}",
            region["displayName"]
        );

        let description = region["description"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: expected a description, got {region:?}"));
        // The form the modelled `Region$description` documentation shows, opened
        // by the opt-in clause where the Region is disabled by default.
        let stem = description_stem(name);
        assert!(
            description.starts_with(stem) && description.len() > stem.len(),
            "case {name}: expected the documented description form {stem:?}, got {description:?}"
        );
        // And only an opt-in Region says so: the clause is not boilerplate.
        assert_eq!(
            description.contains("opt-in Region"),
            OPT_IN_REGIONS.contains(name),
            "case {name}: the opt-in clause belongs to exactly the Regions disabled by \
             default, got {description:?}"
        );

        // `Region$continentCode` is "the continent code (NA, meaning North
        // America)", and an AWS region name begins with its geography.
        let expected_continent = match name.split('-').next() {
            Some("us" | "ca") => "NA",
            Some("sa") => "SA",
            Some("eu") => "EU",
            Some("ap") => "AP",
            other => panic!("case {name}: unexpected geography prefix {other:?}"),
        };
        assert_eq!(
            region["continentCode"].as_str(),
            Some(expected_continent),
            "case {name}: expected continent code {expected_continent}, got {:?}",
            region["continentCode"]
        );
    }
}

/// The description AWS's own answer gives for each served Region: the clause is
/// per Region, so every row is pinned by value as well as by form.
#[tokio::test]
async fn the_regions_with_a_known_description_report_it() {
    let emulator = Emulator::new();
    let (regions, _) = emulator
        .keyed("known descriptions", Some(DEFAULT_REGION), json!({}))
        .await;

    // (region, the clause AWS reports after the sentence stem)
    //
    // SOURCE: AWS's own answers for these Regions. The clause names the whole
    // geography the Region is recommended for — the Region's own territory
    // together with the wider area, in AWS's wording and AWS's spelling of
    // `Austalia` — never the territory alone. The two eastern US Regions share
    // one clause, and so do the two Regions AWS recommends for Ireland, the
    // United Kingdom and Iceland. The rows are every served Region, so they span
    // a clause naming a country, one naming a city and an area, one naming
    // several continents, two Regions sharing one clause, and the opt-in Regions,
    // whose sentence opens with the opt-in clause and closes with a period.
    let known: &[(&str, &str)] = &[
        ("us-east-1", "the eastern United States"),
        ("us-east-2", "the eastern United States"),
        (
            "us-west-2",
            "the northwestern United States, Alaska, and western Canada",
        ),
        ("ap-south-1", "India and southern Asia"),
        ("ap-northeast-1", "Japan"),
        ("ap-northeast-2", "South Korea"),
        ("ap-southeast-1", "India and Southeast Asia"),
        (
            "ap-southeast-2",
            "Austalia, New Zealand, and the South Pacific",
        ),
        ("ca-central-1", "Canada and the northeastern United States"),
        ("eu-central-1", "Europe, the Middle East, and Africa"),
        ("eu-west-1", "Ireland, the United Kingdom, and Iceland"),
        ("eu-west-2", "Ireland, the United Kingdom, and Iceland"),
        ("eu-west-3", "France and central Europe"),
        ("eu-north-1", "Stockholm and north Europe"),
        ("sa-east-1", "Brazil and South America"),
        ("ap-east-1", "Hong Kong and Asia"),
        ("eu-south-2", "Spain and South Europe"),
        ("ap-southeast-3", "Indonesia and Southeast Asia"),
        ("ap-southeast-5", "Malaysia and Southeast Asia"),
    ];

    for (name, serves) in known {
        let region = regions
            .get(*name)
            .unwrap_or_else(|| panic!("case {name}: expected this served region to be reported"));
        let expected = format!(
            "{}{serves}{}",
            description_stem(name),
            description_close(name)
        );
        assert_eq!(
            region["description"].as_str(),
            Some(expected.as_str()),
            "case {name}: expected {expected:?}, got {:?}",
            region["description"]
        );
        // And the clause is the wider geography rather than the place the Region
        // is named after: a description that only restated `displayName` would
        // tell a caller nothing about which users the Region is for.
        let display_name = region["displayName"].as_str().unwrap_or_default();
        assert_ne!(
            *serves, display_name,
            "case {name}: the clause must name the geography the Region serves, not its \
             display name, got {serves:?}"
        );
    }

    // Every served Region is pinned above: a Region added to the catalog without
    // AWS's own clause for it would otherwise go unchecked.
    for name in regions.keys() {
        assert!(
            known.iter().any(|(pinned, _)| pinned == name),
            "case {name}: expected every served Region's description to be pinned here"
        );
    }
}

/// Both zone list members are ALWAYS present, whatever the include flags say and
/// whichever Region the request is served in: a caller that did not ask gets an
/// empty list, never an absent member.
#[tokio::test]
async fn both_zone_lists_are_always_present() {
    let emulator = Emulator::new();

    // Rows span the forms the two optional booleans can take — absent, false and
    // true, in every combination — and the request being served in a region that
    // publishes zones, one the service does not serve, and none at all.
    for region in [Some(DEFAULT_REGION), Some("us-west-2"), Some("us-west-1"), None] {
        for form in ["absent", "explicit"] {
            for (instance_zones, database_zones) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let case = format!(
                    "{form} flags ({instance_zones}, {database_zones}) in {region:?}"
                );
                let body = request_body(form, instance_zones, database_zones);
                let regions = emulator.regions(&case, region, body).await;
                assert_eq!(
                    regions.len(),
                    SERVED_REGIONS.len(),
                    "case {case}: expected the whole catalog, got {} entries",
                    regions.len()
                );
                for entry in &regions {
                    for (member, _) in ZONE_LISTS {
                        assert!(
                            entry[*member].is_array(),
                            "case {case}: {member} must be present as a list, got {:?}",
                            entry[*member]
                        );
                    }
                }
            }
        }
    }
}

/// A zone list carries zones only for the Region the request was served in, and
/// then exactly that Region's published zones; every other Region answers an
/// empty list. Each list is independent: it is filled only when its own request
/// member asked for it.
#[tokio::test]
async fn zones_are_reported_only_for_the_region_the_request_is_served_in() {
    let emulator = Emulator::new();

    // Rows differ along the Region the request is served in: one with four
    // zones, one with six, one that skips a zone letter, one with two, a
    // modelled Region the service does not serve (so no entry can match it), and
    // an unsigned call, which is served in the default Region.
    let served_in: &[(&str, Option<&str>)] = &[
        ("us-west-2", Some("us-west-2")),
        ("us-east-1", Some("us-east-1")),
        ("ap-northeast-1", Some("ap-northeast-1")),
        ("ca-central-1", Some("ca-central-1")),
        ("us-west-1", Some("us-west-1")),
        ("an unsigned call", None),
    ];

    for (label, request_region) in served_in {
        // The region an entry's zones may be reported for: an unsigned call is
        // served in the default region, and a region the service does not serve
        // matches no entry.
        let home = request_region.unwrap_or(DEFAULT_REGION);
        for (instance_zones, database_zones) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let case = format!("{label}: zones ({instance_zones}, {database_zones})");
            let body = request_body("explicit", instance_zones, database_zones);
            let (regions, _) = emulator.keyed(&case, *request_region, body).await;

            for (name, _, _) in SERVED_REGIONS {
                let entry = regions
                    .get(*name)
                    .unwrap_or_else(|| panic!("case {case}: expected region {name} to be reported"));
                let is_home = *name == home;
                for (member, asked) in [
                    ("availabilityZones", instance_zones),
                    ("relationalDatabaseAvailabilityZones", database_zones),
                ] {
                    let reported = zone_names(&case, entry, member);
                    let expected: Vec<String> = if asked && is_home {
                        published_zones(name)
                    } else {
                        Vec::new()
                    };
                    assert_eq!(
                        reported, expected,
                        "case {case} / {name}: expected {member} {expected:?}, got {reported:?}"
                    );
                }
            }
        }
    }
}

/// Reading the catalog changes nothing a later caller sees: two identical calls,
/// and calls with the zone lists and from another Region in between, answer the
/// same catalog.
#[tokio::test]
async fn reading_the_catalog_does_not_change_it() {
    let emulator = Emulator::new();
    let first = emulator
        .regions("first read", Some(DEFAULT_REGION), json!({}))
        .await;
    let with_zones = emulator
        .regions(
            "read with zones",
            Some("us-west-2"),
            json!({"includeAvailabilityZones": true}),
        )
        .await;
    let second = emulator
        .regions("second read", Some(DEFAULT_REGION), json!({}))
        .await;

    assert_eq!(
        first, second,
        "case repeated read: the catalog must not change between reads"
    );
    assert_eq!(
        reported_names("read with zones", &with_zones),
        reported_names("first read", &first),
        "case read with zones: the same regions in the same order, zones or not"
    );
}

/// Every zone a Region advertises is a location this service accepts: the zone
/// names `GetRegions` publishes resolve back to their own Region, which is what
/// `CreateInstances` validates an `availabilityZone` against.
#[tokio::test]
async fn published_zones_belong_to_their_region() {
    let emulator = Emulator::new();

    for (name, _, _) in SERVED_REGIONS {
        let case = format!("zone to region in {name}");
        let (regions, _) = emulator
            .keyed(
                &case,
                Some(name),
                json!({"includeAvailabilityZones": true, "includeRelationalDatabaseAvailabilityZones": true}),
            )
            .await;
        let entry = regions
            .get(*name)
            .unwrap_or_else(|| panic!("case {case}: expected region {name} to be reported"));

        for member in ["availabilityZones", "relationalDatabaseAvailabilityZones"] {
            let zones = zone_names(&case, entry, member);
            assert!(
                !zones.is_empty(),
                "case {case}: the request's own region must report its {member}"
            );
            for zone_name in zones {
                let resolved =
                    service_com_amazonaws_lightsail_lightsail_20161128::state::region_spec_of_availability_zone(
                        &zone_name,
                    )
                    .map(|spec| spec.name);
                assert_eq!(
                    resolved,
                    Some(*name),
                    "case {case}: zone {zone_name} must resolve back to its own region, got {resolved:?}"
                );
            }
        }
    }
}
