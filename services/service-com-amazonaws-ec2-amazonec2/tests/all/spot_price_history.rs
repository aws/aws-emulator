// SPDX-License-Identifier: CC0-1.0

//! `DescribeSpotPriceHistory` end-to-end, over the wire the service actually speaks: an
//! `application/x-www-form-urlencoded` ec2Query POST carrying
//! `Action=DescribeSpotPriceHistory`, dispatched through the same `MultiServiceRouter` the
//! emulator serves with and signed for a Region, which is where the service reads the Region
//! it is being asked about.
//!
//! The rules under test:
//!
//! 1. The history is COMPUTED FROM THE CATALOGS: one entry per price change of every
//!    Availability Zone of the signed-for Region, instance type of the modelled catalog and
//!    product description of the modelled `RIProductDescription` set that the request narrows
//!    to. Each entry carries the zone's name and its id, the type, the description, the price
//!    in the six fractional digits a Spot price is published with, and the instant the change
//!    took effect. The describe writes nothing.
//! 2. A price is a FUNCTION of the type's published vCPUs and memory, the platform, the zone
//!    and the change point: repeat calls report it unchanged, a bigger size of a family costs
//!    more than a smaller one of the same family, `Windows` costs more than `Linux/UNIX` on
//!    the same type, and the Amazon VPC form of a platform costs exactly what the bare form
//!    does.
//! 3. `StartTime`/`EndTime` bound the change points reported: every change point inside the
//!    window, plus the last change at or before `StartTime` — "the effective price as of the
//!    start time" — with nothing after `EndTime`, nothing after now, and nothing older than
//!    the published 90 days.
//! 4. `AvailabilityZone`, `AvailabilityZoneId`, `InstanceTypes`, `ProductDescriptions` and
//!    each documented filter narrow that set and nothing else; `MaxResults`/`NextToken` page
//!    one window of it; `DryRun` performs nothing; and a parameter that names no zone, type
//!    or filter is refused.
//!
//! Every table row is a distinct input along what those rules generalise over — the Region,
//! the instance types, the product descriptions, the window, the filter, the page size — so
//! no single recorded call is the specification.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use ec2::ops::describe_spot_price_history::{
    MAX_HISTORY_DAYS, SECONDS_PER_DAY, SECONDS_PER_PRICE_PERIOD,
};
use ec2::ops::types::R_I_PRODUCT_DESCRIPTION_VALUES;
use ec2::state::{availability_zones_in_region, instance_type_record};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, filters, indexed};

/// One `DescribeSpotPriceHistory` call signed for `region` and carrying `params`, as its HTTP
/// status and XML body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeSpotPriceHistory", params).await
}

/// One form field, for a value that is not a list.
fn field(name: &str, value: impl ToString) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// An absolute instant as the ISO 8601 UTC form `StartTime` and `EndTime` are documented in.
fn iso(at: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("a representable instant")
}

/// Now, in seconds since the Unix epoch — the clock the handler reads too.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock after the Unix epoch")
            .as_secs(),
    )
    .expect("an instant this century")
}

/// The change point in force at `at`: the grid point at or before it.
fn change_point_at(at: i64) -> i64 {
    at.div_euclid(SECONDS_PER_PRICE_PERIOD) * SECONDS_PER_PRICE_PERIOD
}

/// One reported entry of a Spot price history. Every member is read as text, so one the entry
/// does not carry reads as `None` rather than as a default.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    instance_type: Option<String>,
    product_description: Option<String>,
    spot_price: Option<String>,
    timestamp: Option<String>,
}

impl Entry {
    /// The combination this entry prices: its zone, instance type and product description.
    fn combination(&self) -> (String, String, String) {
        (
            self.availability_zone.clone().unwrap_or_default(),
            self.instance_type.clone().unwrap_or_default(),
            self.product_description.clone().unwrap_or_default(),
        )
    }

    /// Its price as a number, for the ordering assertions. The six-digit form is pinned
    /// separately, so a price that is not a decimal amount fails there rather than here.
    fn price(&self, case: &str) -> f64 {
        let price = self.spot_price.clone().unwrap_or_default();
        price
            .parse()
            .unwrap_or_else(|error| panic!("case {case:?}: {price:?} must be a price ({error})"))
    }

    /// The instant its change took effect, in seconds since the Unix epoch.
    fn at(&self, case: &str) -> i64 {
        let timestamp = self.timestamp.clone().unwrap_or_default();
        ::aws_smithy_types::DateTime::from_str(
            &timestamp,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {timestamp:?} must be an ISO 8601 instant ({error})")
        })
        .secs()
    }
}

/// Every entry of an ec2Query `DescribeSpotPriceHistory` body, in the order the response
/// carried them. Each entry is one `<item>` of the `<spotPriceHistorySet>` list, whose first
/// element carries an `xmlns` attribute — so the opening tag is read up to its `>`, exactly
/// as the `DescribeCapacityBlockOfferings` suite reads its own list.
fn entries(body: &str) -> Vec<Entry> {
    let list = element(body, "spotPriceHistorySet").unwrap_or_default();
    list.split("<item")
        .skip(1)
        .map(|rest| {
            let after_tag = rest.split_once('>').map(|(_, rest)| rest).unwrap_or(rest);
            let item = after_tag.split("</item>").next().unwrap_or_default();
            let text = |name: &str| element(item, name);
            Entry {
                availability_zone: text("availabilityZone"),
                availability_zone_id: text("availabilityZoneId"),
                instance_type: text("instanceType"),
                product_description: text("productDescription"),
                spot_price: text("spotPrice"),
                timestamp: text("timestamp"),
            }
        })
        .collect()
}

/// A successful call's history.
async fn history(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    params: &[(String, String)],
) -> Vec<Entry> {
    let (status, body) = describe(router, region, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    entries(&body)
}

/// The Availability Zone names of `region`, and their ids, as the Region's zone facts give
/// them (`ec2::state::availability_zones_in_region`).
fn zones_of(region: &str) -> Vec<(String, String)> {
    availability_zones_in_region(region)
        .into_iter()
        .map(|zone| (zone.zone_name, zone.zone_id))
        .collect()
}

/// The entries of `reported` grouped by the combination they price, keeping the order the
/// response carried them in within each group.
fn grouped(reported: &[Entry]) -> BTreeMap<(String, String, String), Vec<Entry>> {
    let mut groups: BTreeMap<(String, String, String), Vec<Entry>> = BTreeMap::new();
    for entry in reported {
        groups
            .entry(entry.combination())
            .or_default()
            .push(entry.clone());
    }
    groups
}

/// Whether `price` is written the way `SpotPrice$SpotPrice` carries one: a decimal amount
/// with six fractional digits.
fn is_published_price(price: &str) -> bool {
    match price.split_once('.') {
        Some((whole, fraction)) => {
            !whole.is_empty()
                && whole.bytes().all(|byte| byte.is_ascii_digit())
                && fraction.len() == 6
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the history is computed from the catalogs
// ---------------------------------------------------------------------------

/// One row of the history table: the case, the Region the call is signed for, the instance
/// types and product descriptions it narrows to, and how far back `StartTime` is set (in
/// seconds before the call, or `None` for a call that names no window).
struct HistoryCase {
    case: &'static str,
    region: &'static str,
    instance_types: &'static [&'static str],
    product_descriptions: &'static [&'static str],
    start_before: Option<i64>,
}

/// The calls every shape assertion below is made over.
///
/// The rows differ along everything rule 1 generalises over: the Region (and so the zone set
/// and the zone ids), the number and size of the instance types — entries of the modelled
/// `InstanceType` catalog (`smithy_query(query="shape",shape="InstanceType")`), none of them
/// named by the issue this suite closes — the product descriptions, including each form of
/// both modelled platforms, and the window, including none at all and one older than the
/// published 90 days.
fn history_cases() -> Vec<HistoryCase> {
    vec![
        HistoryCase {
            case: "one type and one platform, no window",
            region: "us-east-1",
            instance_types: &["t3.micro"],
            product_descriptions: &["Linux/UNIX"],
            start_before: None,
        },
        HistoryCase {
            case: "two types and both platforms over two days",
            region: "eu-west-2",
            instance_types: &["m5.large", "c6g.medium"],
            product_descriptions: &["Linux/UNIX", "Windows"],
            start_before: Some(2 * SECONDS_PER_DAY),
        },
        HistoryCase {
            case: "every modelled product description over a week",
            region: "us-west-2",
            instance_types: &["p4d.24xlarge"],
            product_descriptions: R_I_PRODUCT_DESCRIPTION_VALUES,
            start_before: Some(7 * SECONDS_PER_DAY),
        },
        HistoryCase {
            case: "three types on the Amazon VPC forms",
            region: "ap-southeast-1",
            instance_types: &["a1.medium", "r5.xlarge", "x2gd.metal"],
            product_descriptions: &["Linux/UNIX (Amazon VPC)", "Windows (Amazon VPC)"],
            start_before: Some(SECONDS_PER_DAY),
        },
        HistoryCase {
            case: "a start older than the published ninety days",
            region: "eu-central-1",
            instance_types: &["t2.nano"],
            product_descriptions: &["Linux/UNIX"],
            start_before: Some(200 * SECONDS_PER_DAY),
        },
    ]
}

/// Every combination the request narrows to is priced at every change point of the window,
/// with the zone facts, the type, the description and a published price — and the describe
/// writes nothing.
#[tokio::test]
async fn every_narrowed_combination_is_priced_at_each_change_point() {
    let (router, engine) = super::ec2_query::emulator();

    for row in history_cases() {
        let case = row.case;
        let before = now_seconds();
        let mut params = indexed("InstanceType", row.instance_types);
        params.extend(indexed("ProductDescription", row.product_descriptions));
        if let Some(start_before) = row.start_before {
            params.push(field("StartTime", iso(before - start_before)));
        }

        let stored_before = engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .stored_bytes();
        let reported = history(&router, row.region, case, &params).await;
        let stored_after = engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .stored_bytes();
        let after = now_seconds();

        // Rule 1: the describe is a read. It stores no price, so a later caller sees exactly
        // what it saw.
        assert_eq!(
            stored_before, stored_after,
            "case {case:?}: reporting a price history must not write to the store"
        );

        // Exactly the combinations the request narrows to, each priced at least once.
        let expected: BTreeSet<(String, String, String)> = zones_of(row.region)
            .iter()
            .flat_map(|(zone_name, _)| {
                row.instance_types.iter().flat_map(move |instance_type| {
                    row.product_descriptions.iter().map(move |product| {
                        (
                            zone_name.clone(),
                            (*instance_type).to_string(),
                            (*product).to_string(),
                        )
                    })
                })
            })
            .collect();
        let groups = grouped(&reported);
        let combinations: BTreeSet<(String, String, String)> = groups.keys().cloned().collect();
        assert_eq!(
            combinations, expected,
            "case {case:?}: expected one priced series per narrowed combination, got \
             {combinations:?}"
        );

        let zone_ids: BTreeMap<String, String> = zones_of(row.region).into_iter().collect();
        // The window, as the request named it and the service bounds it: no change point
        // after the clock, and none older than the published history. Both bounds are read
        // against the clock before AND after the call, so a change point falling due
        // between the two readings is not read as a wrong answer.
        let newest_before = change_point_at(before);
        let newest_after = change_point_at(after);
        let oldest_allowed: Vec<i64> = [before, after]
            .into_iter()
            .map(|clock| {
                let start = row.start_before.map_or(clock, |start_before| {
                    (before - start_before).max(clock - MAX_HISTORY_DAYS * SECONDS_PER_DAY)
                });
                change_point_at(start.min(clock))
            })
            .collect();

        for (combination, series) in &groups {
            let case = format!("{case} for {combination:?}");
            for entry in series {
                assert_eq!(
                    entry.availability_zone_id.as_deref(),
                    zone_ids.get(&combination.0).map(String::as_str),
                    "case {case:?}: expected the zone's own id, got {entry:?}"
                );
                let price = entry.spot_price.clone().unwrap_or_default();
                assert!(
                    is_published_price(&price),
                    "case {case:?}: expected a price with six fractional digits, got {price:?}"
                );
                assert!(
                    entry.price(&case) > 0.0,
                    "case {case:?}: expected a price above zero, got {price:?}"
                );
                let at = entry.at(&case);
                assert_eq!(
                    at % SECONDS_PER_PRICE_PERIOD,
                    0,
                    "case {case:?}: expected a change point on the price grid, got {at}"
                );
                assert!(
                    at <= newest_after,
                    "case {case:?}: expected no change point after the clock \
                     ({newest_after}), got {at}"
                );
                assert!(
                    at >= oldest_allowed[0].min(oldest_allowed[1]),
                    "case {case:?}: expected no change point older than \
                     {oldest_allowed:?}, got {at}"
                );
            }

            // The series is the consecutive change points of the window, most recent first:
            // every one of them, exactly once, with no gap.
            let points: Vec<i64> = series.iter().map(|entry| entry.at(&case)).collect();
            let newest = *points.first().expect("a non-empty series");
            let oldest = *points.last().expect("a non-empty series");
            let expected_points: Vec<i64> = (0..points.len())
                .map(|step| {
                    newest
                        - i64::try_from(step).expect("a bounded index")
                            * SECONDS_PER_PRICE_PERIOD
                })
                .collect();
            assert_eq!(
                points, expected_points,
                "case {case:?}: expected the window's consecutive change points, most \
                 recent first, got {points:?}"
            );
            assert!(
                [newest_before, newest_after].contains(&newest),
                "case {case:?}: expected the newest change point to be the one in force \
                 now ({newest_before} or {newest_after}), got {newest}"
            );
            assert!(
                oldest_allowed.contains(&oldest),
                "case {case:?}: expected the oldest change point to be the price effective \
                 as of the start of the window ({oldest_allowed:?}), got {oldest}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the price is a function of the type, the platform, the zone and the change point
// ---------------------------------------------------------------------------

/// One row of the repeat table: the case, the Region the call is signed for and the whole
/// parameter list it carries.
type RepeatCase = (&'static str, &'static str, Vec<(String, String)>);

/// The same call answers the same prices: a price is derived from the type, the platform, the
/// zone and the change point, so nothing about it is drawn per call.
#[tokio::test]
async fn repeat_calls_report_the_same_price() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows differing along what the rule generalises over: the Region, the narrowing, the
    // window and the page size.
    let cases: [RepeatCase; 4] = [
        (
            "one type, no window",
            "us-east-1",
            indexed("InstanceType", &["t3.micro"]),
        ),
        (
            "two types and both platforms",
            "eu-west-1",
            [
                indexed("InstanceType", &["m5.large", "m6g.medium"]),
                indexed("ProductDescription", &["Linux/UNIX", "Windows"]),
            ]
            .concat(),
        ),
        (
            "a whole Region, one page of it",
            "ap-northeast-1",
            vec![field("MaxResults", 5)],
        ),
        (
            "a three-day window",
            "us-west-2",
            [
                indexed("InstanceType", &["c5.xlarge"]),
                vec![field(
                    "StartTime",
                    iso(now_seconds() - 3 * SECONDS_PER_DAY),
                )],
            ]
            .concat(),
        ),
    ];

    for (case, region, params) in cases {
        let first = history(&router, region, case, &params).await;
        let second = history(&router, region, case, &params).await;
        assert!(
            !first.is_empty(),
            "case {case:?}: expected a non-empty history to compare"
        );
        assert_eq!(
            first, second,
            "case {case:?}: two identical calls must report the same prices"
        );
    }
}

/// One row of the size table: the case and two instance types of one family, the second of
/// which the catalog publishes strictly more vCPUs and memory for.
type SizeCase = (&'static str, &'static str, &'static str);

/// A price is a share of the On-Demand rate of the type's published vCPUs and memory, so the
/// bigger size of a family costs more; a `Windows` price carries the platform's licence
/// premium on top, so it costs more than `Linux/UNIX` for the same type; and the Amazon VPC
/// form of a platform is the same platform, so it costs exactly the same.
#[tokio::test]
async fn a_price_follows_the_types_published_capacity_and_the_platform() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "us-east-1";
    // Pairs from four different families, each spanning a different size step.
    let sizes: [SizeCase; 4] = [
        ("burstable, micro to large", "t3.micro", "t3.large"),
        ("general purpose, large to 4xlarge", "m5.large", "m5.4xlarge"),
        ("Graviton compute, medium to 8xlarge", "c6g.medium", "c6g.8xlarge"),
        ("memory optimised, xlarge to 24xlarge", "r5.xlarge", "r5.24xlarge"),
    ];

    for (case, smaller, bigger) in sizes {
        // The ordering under test is the ordering of the catalog's own facts, read from the
        // same place the handler prices from, so a row cannot pass by naming two types whose
        // published capacity does not differ.
        let facts = |instance_type: &str| {
            let record = instance_type_record(instance_type)
                .unwrap_or_else(|| panic!("case {case:?}: {instance_type} must be modelled"));
            (record.default_vcpus, record.memory_mib)
        };
        let (small_vcpus, small_memory) = facts(smaller);
        let (big_vcpus, big_memory) = facts(bigger);
        assert!(
            big_vcpus >= small_vcpus
                && big_memory >= small_memory
                && (big_vcpus > small_vcpus || big_memory > small_memory),
            "case {case:?}: the catalog must publish more capacity for {bigger} \
             ({big_vcpus} vCPUs, {big_memory} MiB) than for {smaller} ({small_vcpus} \
             vCPUs, {small_memory} MiB)"
        );

        let mut params = indexed("InstanceType", &[smaller, bigger]);
        params.extend(indexed("ProductDescription", R_I_PRODUCT_DESCRIPTION_VALUES));
        let reported = history(&router, region, case, &params).await;
        // One call, so every entry prices the same zone set at the same change point: the
        // comparisons below hold the zone, the change point and (where they vary the type)
        // the platform fixed.
        let priced: BTreeMap<(String, String, String), String> = reported
            .iter()
            .map(|entry| (entry.combination(), entry.spot_price.clone().unwrap_or_default()))
            .collect();
        let price_of = |zone: &str, instance_type: &str, product: &str| -> f64 {
            let key = (
                zone.to_string(),
                instance_type.to_string(),
                product.to_string(),
            );
            let price = priced
                .get(&key)
                .unwrap_or_else(|| panic!("case {case:?}: no price reported for {key:?}"));
            price
                .parse()
                .unwrap_or_else(|error| panic!("case {case:?}: {price:?} is no price ({error})"))
        };

        for (zone_name, _) in zones_of(region) {
            for platform in ["Linux/UNIX", "Windows"] {
                let small = price_of(&zone_name, smaller, platform);
                let big = price_of(&zone_name, bigger, platform);
                assert!(
                    big > small,
                    "case {case:?}: in {zone_name} on {platform}, expected {bigger} \
                     ({big}) to cost more than {smaller} ({small})"
                );
                // The Amazon VPC form of a platform is the same platform at the same price.
                let vpc = format!("{platform} (Amazon VPC)");
                let (small_vpc, big_vpc) = (
                    price_of(&zone_name, smaller, &vpc),
                    price_of(&zone_name, bigger, &vpc),
                );
                assert_eq!(
                    (small_vpc, big_vpc),
                    (small, big),
                    "case {case:?}: in {zone_name}, expected {vpc} to cost what {platform} \
                     costs, got {small_vpc} and {big_vpc}"
                );
            }
            // The licence premium: Windows costs more than Linux/UNIX for the same type in
            // the same zone at the same change point.
            for instance_type in [smaller, bigger] {
                let linux = price_of(&zone_name, instance_type, "Linux/UNIX");
                let windows = price_of(&zone_name, instance_type, "Windows");
                assert!(
                    windows > linux,
                    "case {case:?}: in {zone_name}, expected Windows ({windows}) to cost \
                     more than Linux/UNIX ({linux}) for {instance_type}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the window bounds the change points reported
// ---------------------------------------------------------------------------

/// One row of the window table: the case, how far back `StartTime` is set, how far back
/// `EndTime` is set (negative for an end in the future, `None` for a call that names none),
/// and how many change points each combination is expected to be priced at.
type WindowCase = (&'static str, Option<i64>, Option<i64>, usize);

/// `StartTime` and `EndTime` bound the change points: every one inside the window, plus the
/// last change at or before `StartTime`, and never one after `EndTime`, after now, or older
/// than the published 90 days.
#[tokio::test]
async fn the_requested_window_bounds_the_change_points() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "us-east-1";
    let day = SECONDS_PER_DAY;
    let cases: [WindowCase; 7] = [
        ("no window at all", None, None, 1),
        ("three days back, no end", Some(3 * day), None, 4),
        ("a closed window of three days", Some(5 * day), Some(2 * day), 4),
        ("an end in the future", Some(day), Some(-10 * day), 2),
        (
            "a start older than the published history",
            Some(200 * day),
            None,
            usize::try_from(MAX_HISTORY_DAYS).expect("a positive bound") + 1,
        ),
        ("an end before the start", Some(0), Some(3 * day), 0),
        (
            "a window wholly older than the published history",
            Some(200 * day),
            Some(150 * day),
            0,
        ),
    ];

    for (case, start_before, end_before, expected_points) in cases {
        let at = now_seconds();
        let mut params = indexed("InstanceType", &["m5.large"]);
        params.extend(indexed("ProductDescription", &["Linux/UNIX"]));
        if let Some(start_before) = start_before {
            params.push(field("StartTime", iso(at - start_before)));
        }
        if let Some(end_before) = end_before {
            params.push(field("EndTime", iso(at - end_before)));
        }

        let reported = history(&router, region, case, &params).await;
        if expected_points == 0 {
            assert!(
                reported.is_empty(),
                "case {case:?}: a window that holds no instant prices nothing, got \
                 {reported:?}"
            );
            continue;
        }

        let groups = grouped(&reported);
        assert_eq!(
            groups.len(),
            zones_of(region).len(),
            "case {case:?}: expected one series per zone, got {:?}",
            groups.keys()
        );
        for (combination, series) in &groups {
            let case = format!("{case} for {combination:?}");
            assert_eq!(
                series.len(),
                expected_points,
                "case {case:?}: expected {expected_points} change points, got {:?}",
                series
                    .iter()
                    .map(|entry| entry.timestamp.clone())
                    .collect::<Vec<_>>()
            );
            let points: Vec<i64> = series.iter().map(|entry| entry.at(&case)).collect();
            let newest = *points.first().expect("a non-empty series");
            let oldest = *points.last().expect("a non-empty series");

            // Nothing after the end of the window, which is never later than now.
            let end = end_before.map_or(at, |end_before| (at - end_before).min(at));
            assert!(
                newest <= end,
                "case {case:?}: expected no change point after {end}, got {newest}"
            );
            // The oldest entry is the change in force AT the start of the window — "the last
            // price change before the start time, which is the effective price as of the
            // start time" — clamped to the published history.
            let start = start_before
                .map_or(end, |start_before| at - start_before)
                .max(at - MAX_HISTORY_DAYS * SECONDS_PER_DAY);
            assert!(
                oldest <= start && oldest + SECONDS_PER_PRICE_PERIOD > start,
                "case {case:?}: expected the change in force at {start}, got {oldest}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: narrowing, paging, DryRun and refusals
// ---------------------------------------------------------------------------

/// One row of the narrowing table: the case, the whole parameter list the call carries, and
/// the zones, instance types and product descriptions the answer must hold exactly.
type NarrowingCase = (
    &'static str,
    Vec<(String, String)>,
    Vec<String>,
    Vec<&'static str>,
    Vec<&'static str>,
);

/// Each narrowing member and each documented filter selects exactly the entries that match
/// it, and nothing else.
#[tokio::test]
async fn each_narrowing_selects_exactly_the_matching_prices() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "us-east-1";
    let zones = zones_of(region);
    let (first_zone, first_zone_id) = zones[0].clone();
    let (second_zone, second_zone_id) = zones[1].clone();

    // A baseline call to read a reported price and timestamp back out of, so the rows below
    // filter on values the service itself published rather than on invented ones.
    let baseline = history(
        &router,
        region,
        "baseline",
        &indexed("InstanceType", &["t3.micro"]),
    )
    .await;
    let sample = baseline.first().cloned().expect("a priced entry");
    let sample_price = sample.spot_price.clone().unwrap_or_default();
    let sample_timestamp = sample.timestamp.clone().unwrap_or_default();
    // A wildcard on the id of the second zone, as a value the filter table below holds.
    let zone_id_pattern = format!("*{second_zone_id}");

    // (case, the parameters narrowing the call, the zones expected, the types expected, the
    // product descriptions expected)
    let cases: Vec<NarrowingCase> = vec![
        (
            "the AvailabilityZone member",
            [
                indexed("InstanceType", &["t3.micro"]),
                vec![field("AvailabilityZone", &second_zone)],
            ]
            .concat(),
            vec![second_zone.clone()],
            vec!["t3.micro"],
            R_I_PRODUCT_DESCRIPTION_VALUES.to_vec(),
        ),
        (
            "the AvailabilityZoneId member",
            [
                indexed("InstanceType", &["t3.micro"]),
                vec![field("AvailabilityZoneId", &first_zone_id)],
            ]
            .concat(),
            vec![first_zone.clone()],
            vec!["t3.micro"],
            R_I_PRODUCT_DESCRIPTION_VALUES.to_vec(),
        ),
        (
            "two values of the availability-zone filter",
            [
                indexed("InstanceType", &["t3.micro"]),
                filters(&[(
                    "availability-zone",
                    &[first_zone.as_str(), second_zone.as_str()],
                )]),
            ]
            .concat(),
            vec![first_zone.clone(), second_zone.clone()],
            vec!["t3.micro"],
            R_I_PRODUCT_DESCRIPTION_VALUES.to_vec(),
        ),
        (
            "a wildcard on the availability-zone-id filter",
            [
                indexed("InstanceType", &["t3.micro"]),
                filters(&[("availability-zone-id", &[zone_id_pattern.as_str()])]),
            ]
            .concat(),
            vec![second_zone.clone()],
            vec!["t3.micro"],
            R_I_PRODUCT_DESCRIPTION_VALUES.to_vec(),
        ),
        (
            "the instance-type filter narrowing InstanceTypes",
            [
                indexed("InstanceType", &["m5.large", "c5.large"]),
                filters(&[("instance-type", &["m5.*"])]),
            ]
            .concat(),
            zones.iter().map(|(name, _)| name.clone()).collect(),
            vec!["m5.large"],
            R_I_PRODUCT_DESCRIPTION_VALUES.to_vec(),
        ),
        (
            "the product-description filter",
            [
                indexed("InstanceType", &["t3.micro"]),
                filters(&[("product-description", &["Windows*"])]),
            ]
            .concat(),
            zones.iter().map(|(name, _)| name.clone()).collect(),
            vec!["t3.micro"],
            vec!["Windows", "Windows (Amazon VPC)"],
        ),
        (
            "ProductDescriptions naming one platform",
            [
                indexed("InstanceType", &["t3.micro"]),
                indexed("ProductDescription", &["Linux/UNIX (Amazon VPC)"]),
            ]
            .concat(),
            zones.iter().map(|(name, _)| name.clone()).collect(),
            vec!["t3.micro"],
            vec!["Linux/UNIX (Amazon VPC)"],
        ),
    ];

    for (case, params, expected_zones, expected_types, expected_products) in cases {
        let reported = history(&router, region, case, &params).await;
        let zones_reported: BTreeSet<String> = reported
            .iter()
            .map(|entry| entry.availability_zone.clone().unwrap_or_default())
            .collect();
        let types_reported: BTreeSet<String> = reported
            .iter()
            .map(|entry| entry.instance_type.clone().unwrap_or_default())
            .collect();
        let products_reported: BTreeSet<String> = reported
            .iter()
            .map(|entry| entry.product_description.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            zones_reported,
            expected_zones.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: expected exactly the narrowed zones, got {zones_reported:?}"
        );
        assert_eq!(
            types_reported,
            expected_types
                .iter()
                .map(|value| (*value).to_string())
                .collect::<BTreeSet<String>>(),
            "case {case:?}: expected exactly the narrowed instance types, got \
             {types_reported:?}"
        );
        assert_eq!(
            products_reported,
            expected_products
                .iter()
                .map(|value| (*value).to_string())
                .collect::<BTreeSet<String>>(),
            "case {case:?}: expected exactly the narrowed product descriptions, got \
             {products_reported:?}"
        );
    }

    // The two filters that compare against a value only a priced entry holds: the price
    // itself, and the change point — the latter in both renderings the member's doc and the
    // response write a timestamp in.
    let price_filtered = history(
        &router,
        region,
        "the spot-price filter",
        &[
            indexed("InstanceType", &["t3.micro"]),
            filters(&[("spot-price", &[sample_price.as_str()])]),
        ]
        .concat(),
    )
    .await;
    assert!(
        !price_filtered.is_empty()
            && price_filtered
                .iter()
                .all(|entry| entry.spot_price.as_deref() == Some(sample_price.as_str())),
        "case \"the spot-price filter\": expected only entries priced {sample_price:?}, got \
         {price_filtered:?}"
    );

    for (case, pattern) in [
        ("the timestamp filter, ISO 8601", sample_timestamp.clone()),
        (
            "the timestamp filter, the documented UTC form",
            "* UTC *".to_string(),
        ),
    ] {
        let reported = history(
            &router,
            region,
            case,
            &[
                indexed("InstanceType", &["t3.micro"]),
                vec![field("StartTime", iso(now_seconds() - 3 * SECONDS_PER_DAY))],
                filters(&[("timestamp", &[pattern.as_str()])]),
            ]
            .concat(),
        )
        .await;
        let points: BTreeSet<String> = reported
            .iter()
            .map(|entry| entry.timestamp.clone().unwrap_or_default())
            .collect();
        let expected: BTreeSet<String> = if case.ends_with("ISO 8601") {
            // One change point: the one the value names exactly.
            BTreeSet::from([sample_timestamp.clone()])
        } else {
            // Every change point of the window: the pattern matches the UTC form the filter
            // is documented in, which every entry's timestamp also holds.
            (0..4_i64)
                .map(|step| {
                    iso(change_point_at(now_seconds()) - step * SECONDS_PER_PRICE_PERIOD)
                })
                .collect()
        };
        assert_eq!(
            points, expected,
            "case {case:?}: expected the change points {expected:?}, got {points:?}"
        );
    }

    // A product description outside the modelled `RIProductDescription` set
    // (`smithy_query(query="shape",shape="SpotPrice")`) names no priced product, so it
    // selects nothing rather than everything.
    let unknown_product = history(
        &router,
        region,
        "a product description the model does not declare",
        &[
            indexed("InstanceType", &["t3.micro"]),
            indexed("ProductDescription", &["Red Hat Enterprise Linux"]),
        ]
        .concat(),
    )
    .await;
    assert!(
        unknown_product.is_empty(),
        "case \"a product description the model does not declare\": expected no prices, got \
         {unknown_product:?}"
    );
}

/// `MaxResults` bounds one page of the window and `NextToken` walks the rest of it: the walk
/// reports exactly the unpaged sequence, in the same order, each entry once, and the last
/// page carries no token.
#[tokio::test]
async fn max_results_and_next_token_page_one_window() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "eu-west-1";
    let narrowing = [
        indexed("InstanceType", &["m5.large", "c5.large"]),
        indexed("ProductDescription", &["Linux/UNIX"]),
        vec![field("StartTime", iso(now_seconds() - SECONDS_PER_DAY))],
    ]
    .concat();

    let whole = history(&router, region, "the whole window", &narrowing).await;
    assert_eq!(
        whole.len(),
        zones_of(region).len() * 2 * 2,
        "expected one entry per zone, type and change point, got {whole:?}"
    );

    for page_size in [1_usize, 2, 5, whole.len(), whole.len() + 1] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<Entry> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = narrowing.clone();
            params.push(field("MaxResults", page_size));
            if let Some(token) = &token {
                params.push(field("NextToken", token));
            }
            let (status, body) = describe(&router, region, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = entries(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: a paged walk must report the window exactly once, in order"
        );
    }

    // A token minted for one search names a position inside THAT window, so it is refused
    // when it is presented back against another.
    let mut first_page = narrowing.clone();
    first_page.push(field("MaxResults", 1));
    let (_status, body) = describe(&router, region, &first_page).await;
    let token = element(&body, "nextToken").expect("a token for the second page");
    let mut other_search = indexed("InstanceType", &["t3.micro"]);
    other_search.push(field("NextToken", &token));
    let (status, body) = describe(&router, region, &other_search).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidPaginationToken"),
        "a token presented against another search must be refused, got {body}"
    );
}

/// A cursor carries the window its walk is being read over, and no cursor can widen that
/// window past the published history: the emulator authenticates nobody and a page token is
/// opaque rather than unforgeable, so the window it names is held to the change points the
/// clock permits before anything is priced.
#[tokio::test]
async fn a_cursor_cannot_widen_the_window_past_the_published_history() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "us-east-1";
    let mut params = indexed("InstanceType", &["m5.large"]);
    params.extend(indexed("ProductDescription", &["Linux/UNIX"]));
    params.push(field("MaxResults", 1));

    let (status, body) = describe(&router, region, &params).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 for the first page, got {status} with body {body}"
    );
    let token = element(&body, "nextToken").expect("a token for the second page");
    let cursor = ec2::state::decode_page_token(&token).expect("a token this service minted");
    let fields: Vec<&str> = cursor.split('|').collect();
    assert_eq!(
        fields.len(),
        5,
        "the cursor must name the walk's search, window and position, got {cursor:?}"
    );

    // The same search and position, with a window no clock could permit: four thousand
    // million change points either side of the epoch.
    let tampered = ec2::state::encode_page_token(&format!(
        "{}|{}|-4000000000|4000000000|0",
        fields[0], fields[1]
    ));
    let mut resumed = params.clone();
    resumed.push(field("NextToken", &tampered));
    let (status, body) = describe(&router, region, &resumed).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 for the resumed page, got {status} with body {body}"
    );
    let reported = entries(&body);
    assert_eq!(
        reported.len(),
        1,
        "expected the one entry the page size allows, got {reported:?}"
    );
    let at = reported
        .first()
        .expect("one entry")
        .at("a tampered cursor window");
    let now = now_seconds();
    assert!(
        at <= change_point_at(now)
            && at >= change_point_at(now - MAX_HISTORY_DAYS * SECONDS_PER_DAY),
        "expected a change point inside the published history, got {at}"
    );
}

/// `DryRun` performs nothing: a call that would have succeeded answers 412
/// `DryRunOperation`, and one that would have been refused is refused with its own reason.
#[tokio::test]
async fn dry_run_performs_nothing() {
    let (router, _engine) = super::ec2_query::emulator();

    let mut would_succeed = indexed("InstanceType", &["t3.micro"]);
    would_succeed.push(field("DryRun", "true"));
    let (status, body) = describe(&router, "us-east-1", &would_succeed).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "a dry run that would have succeeded answers DryRunOperation, got {body}"
    );
    assert!(
        element(&body, "spotPriceHistorySet").is_none(),
        "a dry run reports no price, got {body}"
    );

    // A request outside what the model declares is refused by the boundary validator before
    // the handler runs, so the dry run of one keeps that refusal rather than answering
    // "would have succeeded".
    let mut would_be_refused = vec![field("AvailabilityZone", "us-east-1z")];
    would_be_refused.push(field("DryRun", "true"));
    let (status, body) = describe(&router, "us-east-1", &would_be_refused).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "a dry run of a request that would have been refused keeps that refusal, got {body}"
    );
}

/// One row of the refusal table: the case, the parameters, the error code the refusal carries
/// and text its message must name.
type RefusalCase = (&'static str, Vec<(String, String)>, &'static str, String);

/// A parameter that names no zone, no instance type, no filter or no page this service serves
/// is refused, and the refusal names the value at fault.
#[tokio::test]
async fn a_parameter_outside_what_the_service_serves_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "us-east-1";
    let zones = zones_of(region);
    let (zone_name, zone_id) = zones[0].clone();

    let cases: Vec<RefusalCase> = vec![
        (
            "both ways of naming a zone",
            vec![
                field("AvailabilityZone", &zone_name),
                field("AvailabilityZoneId", &zone_id),
            ],
            "InvalidParameterCombination",
            "AvailabilityZoneId".to_string(),
        ),
        (
            "a zone the Region does not have",
            vec![field("AvailabilityZone", "us-east-1z")],
            "InvalidParameterValue",
            "us-east-1z".to_string(),
        ),
        (
            "a zone id the Region does not have",
            vec![field("AvailabilityZoneId", "use1-az9")],
            "InvalidParameterValue",
            "use1-az9".to_string(),
        ),
        (
            "a zone of another Region",
            vec![field("AvailabilityZone", "eu-west-1a")],
            "InvalidParameterValue",
            "eu-west-1a".to_string(),
        ),
        (
            "an instance type the model does not declare",
            indexed("InstanceType", &["m5.enormous"]),
            "InvalidParameterValue",
            "m5.enormous".to_string(),
        ),
        (
            "an instance family rather than a type",
            indexed("InstanceType", &["m5"]),
            "InvalidParameterValue",
            "m5".to_string(),
        ),
        (
            "a filter this operation does not document",
            filters(&[("spot-prices", &["0.1"])]),
            "InvalidParameterValue",
            "spot-prices".to_string(),
        ),
        (
            "a filter name of another describe",
            filters(&[("instance-id", &["i-1234567890abcdef0"])]),
            "InvalidParameterValue",
            "instance-id".to_string(),
        ),
        (
            "a page of no items",
            vec![field("MaxResults", 0)],
            "InvalidParameterValue",
            "0".to_string(),
        ),
        (
            "a token this service did not mint",
            vec![field("NextToken", "not-a-token")],
            "InvalidPaginationToken",
            // This refusal's sentence is fixed: it names no token.
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
    ];

    for (case, params, expected_code, named) in cases {
        let (status, body) = describe(&router, region, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {message:?}"
        );
        assert!(
            element(&body, "spotPriceHistorySet").is_none(),
            "case {case:?}: a refused request reports no price, got {body}"
        );
    }
}
