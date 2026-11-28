// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstanceTypeOfferings` end-to-end: which instance types a Region reports
//! as offered, in which locations, and how a caller narrows and pages them.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with, carrying
//! the Region in the SigV4 `Credential=` scope, which is where the service reads the
//! Region it is being asked about.
//!
//! The rules under test:
//!
//! 1. A Region reports the instance types it offers in each location of the requested
//!    `LocationType`: the Region itself for `region` (the default when a request names
//!    none, where the whole modelled `InstanceType` catalog is offered), its Availability
//!    Zones for `availability-zone`, their AZ IDs for `availability-zone-id`, and nothing
//!    for `outpost`, because no operation of this emulator creates an Outpost. Not every
//!    zone offers every type — the Region's last zone offers only the previous generations
//!    — which is why the describe reports per location at all. A `LocationType` outside the
//!    modelled enum is refused.
//! 2. `Filters` (`instance-type`, `location`) are ANDed with one another and ORed over
//!    each filter's values, with EC2's `*` and `?` wildcards; a filter the operation
//!    does not document is refused with `InvalidParameterValue`.
//! 3. `MaxResults` and `NextToken` page the matching offerings: each page holds at most
//!    `MaxResults` items, the walk reports every matching offering exactly once, and
//!    the last page carries no token. A token this service did not mint is refused with
//!    `InvalidPaginationToken`, and `MaxResults` outside the modelled 5..1000 range is
//!    refused at the boundary.
//! 4. `DryRun` performs nothing and answers 412 `DryRunOperation`.
//! 5. The describe is a read: repeating it answers the same offerings.
//!
//! Offering ORDER within a response is deliberately not asserted — EC2 says the order
//! of elements in a response might vary — so every comparison here is over a sorted
//! set. What the pagination test does rely on is that the order is STABLE across
//! calls, which is what makes a cursor token see each offering exactly once.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters, unescaped};

/// One `DescribeInstanceTypeOfferings` call signed for `region` and carrying `params`,
/// as its HTTP status and XML body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeInstanceTypeOfferings", params).await
}

/// One offering of a `DescribeInstanceTypeOfferings` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Offering {
    instance_type: String,
    location_type: String,
    location: String,
}

/// Every offering in an ec2Query `DescribeInstanceTypeOfferings` body, in the order the
/// response carried them. Each offering is one `<item>` of the
/// `<instanceTypeOfferingSet>` list — and the list's first element carries an `xmlns`
/// attribute, so the opening tag is read up to its `>`.
fn offerings(case: &str, body: &str) -> Vec<Offering> {
    let list = element(body, "instanceTypeOfferingSet").unwrap_or_default();
    list.split("<item")
        .skip(1)
        .map(|rest| {
            let after_tag = rest.split_once('>').map(|(_, rest)| rest).unwrap_or(rest);
            let item = after_tag.split("</item>").next().unwrap_or_default();
            let member = |name: &str| {
                element(item, name)
                    .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in offering {item:?}"))
            };
            Offering {
                instance_type: member("instanceType"),
                location_type: member("locationType"),
                location: member("location"),
            }
        })
        .collect()
}

/// Every offering a request reports over a whole `NextToken` walk, with the number of
/// pages it took. Each page is asserted to hold at most `page_size` offerings when the
/// request names one.
async fn walk(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    params: &[(String, String)],
    page_size: Option<usize>,
) -> (Vec<Offering>, usize) {
    let mut seen: Vec<Offering> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut call_params = params.to_vec();
        if let Some(page_size) = page_size {
            call_params.push(("MaxResults".to_string(), page_size.to_string()));
        }
        if let Some(token) = &token {
            call_params.push(("NextToken".to_string(), token.clone()));
        }
        let (status, body) = describe(router, region, &call_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 on page {}, got {status} {body}",
            pages + 1
        );
        let page = offerings(case, &body);
        if let Some(page_size) = page_size {
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} offerings, got {}",
                page.len()
            );
        }
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= 1_000,
            "case {case:?}: the walk must terminate, got {pages} pages"
        );
        token = element(&body, "nextToken");
        if token.is_none() {
            return (seen, pages);
        }
    }
}

/// The set of locations a response reports offerings in.
fn locations(case: &str, body: &str) -> BTreeSet<String> {
    offerings(case, body)
        .into_iter()
        .map(|offering| offering.location)
        .collect()
}

/// The set of instance types a response reports.
fn instance_types(case: &str, body: &str) -> BTreeSet<String> {
    offerings(case, body)
        .into_iter()
        .map(|offering| offering.instance_type)
        .collect()
}

/// `BTreeSet` of owned strings from a slice of string literals.
fn owned(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// `LocationType=<value>` as a form field.
fn location_type(value: &str) -> Vec<(String, String)> {
    vec![("LocationType".to_string(), value.to_string())]
}

/// Instance types from the modelled `InstanceType` catalog
/// (`smithy_query(query="shape",shape="InstanceType")`) used as the narrowing filter in
/// the location tests. None of them is cited by the issue this suite closes, and they
/// span four generations and three architectures — and every one of them is CURRENT
/// generation, so the zones that offer them are the ones
/// [`zones_offering_current_generation`] names.
const CATALOG_SAMPLE: [&str; 4] = ["t2.nano", "m5.large", "c6g.medium", "r7iz.32xlarge"];

/// The zones of `region` that offer the current-generation instance types: every zone that
/// Region PUBLISHES except its last.
///
/// AWS does not offer every instance type in every Availability Zone, which is why a
/// `LocationType` of `availability-zone` reports per zone at all; this emulator places that
/// boundary at the LAST zone of a Region, which offers only the previous generations
/// (`crate::state::location_offers_instance_type`). So a Region publishing three zones
/// offers the current generation in two of them, and one publishing six offers it in five.
fn zones_offering_current_generation(region: &str) -> BTreeSet<String> {
    let letters = service_common::aws_availability_zones::zone_letters(region);
    letters
        .iter()
        .take(letters.len().saturating_sub(1))
        .map(|letter| format!("{region}{letter}"))
        .collect()
}

/// The AZ-IDs matching [`zones_offering_current_generation`] — `<prefix>-az1` is the
/// Region's first zone, so they are the indices from 1 up to that Region's zone count.
fn zone_ids_offering_current_generation(region: &str, prefix: &str) -> BTreeSet<String> {
    let count = service_common::aws_availability_zones::zone_letters(region).len();
    (1..count)
        .map(|index| format!("{prefix}-az{index}"))
        .collect()
}

/// One row of the location-type table: the case name, the `LocationType` form fields the
/// request carries, and the locations the Region is expected to report.
type LocationTypeCase = (
    &'static str,
    Vec<(String, String)>,
    fn(&str) -> BTreeSet<String>,
);

/// One row of the filter table: the case name, the `Filters` form fields the request
/// carries, and the instance types and locations expected back.
type FilterCase = (
    &'static str,
    Vec<(String, String)>,
    BTreeSet<String>,
    BTreeSet<String>,
);

// ---------------------------------------------------------------------------
// Rule 1: a Region's locations, per LocationType
// ---------------------------------------------------------------------------

/// Each `LocationType` reports the offerings of its own locations of the Region the
/// request was signed for, and a request naming no `LocationType` reports the Region's.
///
/// The rows span every value of the modelled `LocationType` enum
/// (`smithy_query(query="shape",shape="LocationType")`) plus the absent-member case, and
/// each is checked in two Regions whose names have a different shape, so the locations
/// cannot be a constant. The requests carry an `instance-type` filter only to keep the
/// body small; rule 1's claim about the full catalog is asserted by
/// `every_catalogued_instance_type_is_offered_in_the_region` below.
#[tokio::test]
async fn each_location_type_reports_the_locations_of_its_own_kind() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [LocationTypeCase; 5] = [
        ("absent", Vec::new(), |region| owned(&[region])),
        ("region", location_type("region"), |region| {
            owned(&[region])
        }),
        (
            "availability-zone",
            location_type("availability-zone"),
            zones_offering_current_generation,
        ),
        (
            "availability-zone-id",
            location_type("availability-zone-id"),
            |region| {
                // The Region's zone-id prefix, as `DescribeAvailabilityZones` reports
                // it: `us-east-1` -> `use1`, `ap-northeast-2` -> `apne2`.
                let prefix = match region {
                    "us-east-1" => "use1",
                    "ap-northeast-2" => "apne2",
                    other => panic!("no zone-id prefix pinned for {other:?}"),
                };
                zone_ids_offering_current_generation(region, prefix)
            },
        ),
        // An Outpost is hardware in a customer's own facility and no operation of this
        // emulator creates one, so a Region offers no instance type in an Outpost.
        ("outpost", location_type("outpost"), |_| BTreeSet::new()),
    ];

    for (case, location_fields, expected) in cases {
        for region in ["us-east-1", "ap-northeast-2"] {
            let mut params = location_fields.clone();
            params.extend(filters(&[("instance-type", &CATALOG_SAMPLE)]));
            let (status, body) = describe(&router, region, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{region}: expected 200, got {status} {body}"
            );
            let expected_locations = expected(region);
            assert_eq!(
                locations(case, &body),
                expected_locations,
                "case {case:?}/{region}: expected offerings in {expected_locations:?}, got {body}"
            );
            // Every location of the kind offers each filtered instance type.
            let reported = offerings(case, &body);
            assert_eq!(
                reported.len(),
                expected_locations.len() * CATALOG_SAMPLE.len(),
                "case {case:?}/{region}: expected each location to offer every filtered type, got {reported:?}"
            );
            for offering in &reported {
                let expected_type = if case == "absent" { "region" } else { case };
                assert_eq!(
                    offering.location_type, expected_type,
                    "case {case:?}/{region}: expected locationType {expected_type:?}, got {offering:?}"
                );
            }
        }
    }
}

/// Every instance type the modelled `InstanceType` catalog declares is offered in the
/// current Region EXCEPT the ones AWS publishes no Region for, and nothing outside the
/// catalog is offered at all.
///
/// The catalog is read from the value list the model generated, so the assertion is over
/// the whole class rather than over a sample: a type the model declares and the Region is
/// offered but the emulator withheld, or a type it offered that the model does not declare,
/// both fail here. The whole-host sizes of the newest families, and the bare metal size of a
/// previous generation storage optimized one, are taken out by NAME, those being the types no
/// Region's published offering set names — the rule
/// `region_restricted_instance_type_offerings` covers.
/// `u-6tb1.metal` and `x2gd.medium` are checked by name as entries the issue did not
/// cite. The catalog is larger than one page, so the offerings are collected over a
/// `NextToken` walk.
#[tokio::test]
async fn every_catalogued_instance_type_is_offered_in_the_region() {
    let (router, _engine) = super::ec2_query::emulator();
    const REGION: &str = "eu-west-1";
    /// The types AWS publishes no Region for, so no Region offers them in any location.
    const UNOFFERED_ANYWHERE: [&str; 31] = [
        "c8gb.metal-24xl",
        "c8gb.metal-48xl",
        "c8gn.metal-24xl",
        "c8gn.metal-48xl",
        "c8ib.metal-48xl",
        "c8ib.metal-96xl",
        "i3.metal",
        "m8gb.metal-24xl",
        "m8gb.metal-48xl",
        "m8gn.metal-24xl",
        "m8gn.metal-48xl",
        "m8ib.metal-48xl",
        "m8ib.metal-96xl",
        "m8idb.metal-48xl",
        "m8idb.metal-96xl",
        "m8in.metal-48xl",
        "m8in.metal-96xl",
        "m9g.metal-24xl",
        "m9gd.metal-24xl",
        "r8gb.metal-24xl",
        "r8gb.metal-48xl",
        "r8gn.metal-24xl",
        "r8gn.metal-48xl",
        "r8ib.metal-48xl",
        "r8ib.metal-96xl",
        "r8idb.metal-48xl",
        "r8idb.metal-96xl",
        "r8idn.metal-48xl",
        "r8idn.metal-96xl",
        "r8in.metal-48xl",
        "r8in.metal-96xl",
    ];
    let catalog: BTreeSet<String> = ec2::ops::types::INSTANCE_TYPE_VALUES
        .iter()
        .filter(|value| !UNOFFERED_ANYWHERE.contains(value))
        .map(|value| (*value).to_string())
        .collect();
    assert!(
        catalog.len() > 100,
        "case catalog: the modelled InstanceType enum should carry the whole fleet, got {}",
        catalog.len()
    );

    let (walked, _pages) = walk(&router, REGION, "region", &[], None).await;
    let reported: BTreeSet<String> = walked
        .iter()
        .map(|offering| offering.instance_type.clone())
        .collect();
    assert_eq!(
        reported, catalog,
        "case region: the offerings must be exactly the modelled catalog; \
         missing {:?}, unexpected {:?}",
        catalog.difference(&reported).collect::<Vec<_>>(),
        reported.difference(&catalog).collect::<Vec<_>>()
    );
    assert_eq!(
        walked.len(),
        catalog.len(),
        "case region: the Region offers each type once, got {} offerings",
        walked.len()
    );
    for pinned in ["u-6tb1.metal", "x2gd.medium"] {
        assert!(
            reported.contains(pinned),
            "case region: expected {pinned:?} among the offerings, got {} types",
            reported.len()
        );
    }
}

/// A `LocationType` outside the modelled enum is refused with HTTP 400 naming the member
/// and the value, rather than answered as the Region or with an empty list.
///
/// The modelled `LocationType` enum (`smithy_query(query="shape",shape="LocationType")`)
/// is a constraint the request walk enforces before the handler runs, so the refusal here
/// is the boundary validator's; the handler keeps its own `InvalidParameterValue` path for
/// the same case so no unknown value can ever read as the default `region`.
///
/// The rows differ in how the value is wrong: a near-miss plural, a member-cased value, a
/// value from another API's vocabulary, and the empty string.
#[tokio::test]
async fn an_unknown_location_type_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, &str); 4] = [
        ("plural near-miss", "availability-zones"),
        ("member-cased", "AvailabilityZone"),
        ("another vocabulary", "local-zone"),
        ("empty", ""),
    ];

    for (case, value) in cases {
        let (status, body) = describe(&router, "us-east-1", &location_type(value)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for LocationType {value:?}, got {status} {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.to_lowercase().contains("locationtype"),
            "case {case:?}: the refusal must name the member, got {message:?}"
        );
        assert!(
            message.contains(value),
            "case {case:?}: the refusal must name the value {value:?}, got {message:?}"
        );
        assert!(
            element(&body, "instanceTypeOfferingSet").is_none(),
            "case {case:?}: a refused request reports no offerings, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: Filters
// ---------------------------------------------------------------------------

/// `Filters` are ANDed with one another and ORed over each filter's values, with EC2's
/// `*` and `?` wildcards.
///
/// The rows differ in which documented filter is used, in how many values it carries, in
/// which wildcard narrows it, and in whether the filters select nothing. Every
/// expectation is computed from the modelled catalog, never written out by hand.
///
/// The LOCATIONS a row reports follow the per-zone offerings: a row narrowed to
/// current-generation types only reports the zones
/// [`zones_offering_current_generation`] names, while one whose wildcard also matches a
/// previous-generation family reports every zone of the Region, the previous generations
/// being offered everywhere.
#[tokio::test]
async fn filters_and_over_names_and_or_over_values() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "eu-west-1";
    let current_generation_zones = zones_offering_current_generation(region);
    let every_zone = owned(&["eu-west-1a", "eu-west-1b", "eu-west-1c"]);
    let catalog = ec2::ops::types::INSTANCE_TYPE_VALUES;

    let cases: [FilterCase; 6] = [
        (
            "one instance-type value",
            filters(&[("instance-type", &["m5.large"])]),
            owned(&["m5.large"]),
            current_generation_zones.clone(),
        ),
        (
            "three instance-type values ORed",
            filters(&[("instance-type", &["m5.large", "t3.nano", "c6g.medium"])]),
            owned(&["m5.large", "t3.nano", "c6g.medium"]),
            current_generation_zones.clone(),
        ),
        (
            "instance-type wildcard *",
            filters(&[("instance-type", &["m5d.*"])]),
            catalog
                .iter()
                .filter(|value| value.starts_with("m5d."))
                .map(|value| (*value).to_string())
                .collect(),
            current_generation_zones.clone(),
        ),
        (
            // `c?.large` matches previous-generation families too (`c3.large`), and those
            // are offered in every zone of the Region.
            "instance-type wildcard ?",
            filters(&[("instance-type", &["c?.large"])]),
            catalog
                .iter()
                .filter(|value| {
                    value.len() == "c5.large".len()
                        && value.starts_with('c')
                        && value.ends_with(".large")
                })
                .map(|value| (*value).to_string())
                .collect(),
            every_zone,
        ),
        (
            "location ANDed with instance-type",
            filters(&[
                ("instance-type", &["m5.large", "t3.nano"]),
                ("location", &["eu-west-1b"]),
            ]),
            owned(&["m5.large", "t3.nano"]),
            owned(&["eu-west-1b"]),
        ),
        (
            "location that is no zone of the Region",
            filters(&[("location", &["us-east-1a"])]),
            BTreeSet::new(),
            BTreeSet::new(),
        ),
    ];

    for (case, filter_fields, expected_types, expected_locations) in cases {
        let mut params = location_type("availability-zone");
        params.extend(filter_fields);
        let (status, body) = describe(&router, region, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            instance_types(case, &body),
            expected_types,
            "case {case:?}: expected {expected_types:?}, got {body}"
        );
        assert_eq!(
            locations(case, &body),
            expected_locations,
            "case {case:?}: expected locations {expected_locations:?}, got {body}"
        );
    }
}

/// A filter name this operation does not document is refused with `InvalidParameterValue`
/// naming it, rather than silently ignored.
///
/// The rows differ in how the name is wrong: a near-miss of a documented filter, a
/// member-cased name, a name from another describe, and no name at all. The two names the
/// operation DOES document
/// (`smithy_query(query="doc",shape="DescribeInstanceTypeOfferingsRequest",member="Filters")`)
/// are accepted in the same test.
#[tokio::test]
async fn an_unknown_filter_name_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    for name in ["instance-type", "location"] {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &filters(&[(name, &["m5.large", "us-east-1"])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a documented filter must be accepted, got {status} {body}"
        );
    }

    let cases: [(&str, &str); 4] = [
        ("plural near-miss", "instance-types"),
        ("member-cased", "InstanceType"),
        ("another describe's filter", "zone-name"),
        ("no name", ""),
    ];
    for (case, name) in cases {
        let (status, body) =
            describe(&router, "us-east-1", &filters(&[(name, &["m5.large"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(&format!("'{name}'")),
            "case {case:?}: the message must name the filter {name:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: MaxResults / NextToken
// ---------------------------------------------------------------------------

/// A `NextToken` walk reports every matching offering exactly once, in pages of at most
/// `MaxResults`, and the last page carries no token.
///
/// The rows are page sizes spanning the modelled `DITOMaxResults` range 5..1000
/// (`smithy_query(query="shape",shape="DITOMaxResults")`): its lower bound, a size that
/// does not divide the result count, a size larger than the result count, and its upper
/// bound. The expectation is the un-paged answer to the same request, so paging is
/// asserted to change only the packaging.
#[tokio::test]
async fn a_next_token_walk_reports_every_offering_once() {
    let (router, _engine) = super::ec2_query::emulator();
    // `c5*` keeps the walk to a few dozen offerings per zone while still spanning three
    // locations, so a small page size means many pages.
    let narrowing = || {
        let mut params = location_type("availability-zone");
        params.extend(filters(&[("instance-type", &["c5*"])]));
        params
    };

    let (status, body) = describe(&router, "us-west-2", &narrowing()).await;
    assert_eq!(
        status, 200,
        "case unpaged: expected 200, got {status} {body}"
    );
    let unpaged = offerings("unpaged", &body);
    assert!(
        unpaged.len() > 20,
        "case unpaged: the walk needs several pages' worth of offerings, got {}",
        unpaged.len()
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case unpaged: fewer offerings than one page holds ends the walk, got {body}"
    );
    let expected: BTreeSet<Offering> = unpaged.iter().cloned().collect();
    assert_eq!(
        expected.len(),
        unpaged.len(),
        "case unpaged: an offering is reported once, got {unpaged:?}"
    );

    for page_size in [5usize, 7, 1000, unpaged.len() + 1] {
        let case = format!("MaxResults={page_size}");
        let (seen, pages) = walk(
            &router,
            "us-west-2",
            &case,
            &narrowing(),
            Some(page_size),
        )
        .await;
        assert_eq!(
            seen.len(),
            unpaged.len(),
            "case {case:?}: expected {} offerings over the walk, got {}",
            unpaged.len(),
            seen.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<Offering>>(),
            expected,
            "case {case:?}: the walk must report exactly the un-paged offerings"
        );
        let expected_pages = unpaged.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} offerings, got {pages}",
            unpaged.len()
        );
    }
}

/// `MaxResults` outside the modelled 5..1000 range is refused, each published bound is
/// accepted, and the value one past each bound is not.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, MaxResults, is it inside the modelled range)
    let cases: [(&str, &str, bool); 6] = [
        ("one below the minimum", "4", false),
        ("the minimum", "5", true),
        ("inside", "250", true),
        ("the maximum", "1000", true),
        ("one above the maximum", "1001", false),
        ("zero", "0", false),
    ];

    for (case, value, inside) in cases {
        let params = vec![("MaxResults".to_string(), value.to_string())];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={value} is inside 5..1000, got {status} {body}"
            );
            let page = offerings(case, &body);
            let limit: usize = value.parse().expect("a numeric MaxResults");
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} offerings, got {}",
                page.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={value} is outside 5..1000, got {status} {body}"
            );
            assert!(
                element(&body, "instanceTypeOfferingSet").is_none(),
                "case {case:?}: a refused request reports no offerings, got {body}"
            );
        }
    }
}

/// A `NextToken` this service did not mint is refused with `InvalidPaginationToken`,
/// rather than restarting the walk at the first offering.
///
/// The rows differ in how the token is wrong: not a token at all, hex that decodes to
/// something else, a cursor presented unencoded, and an empty one.
#[tokio::test]
async fn an_unknown_next_token_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, &str); 4] = [
        ("not a token", "not-a-token"),
        ("hex of something else", "99999999"),
        ("an unencoded cursor", "us-east-1 m5.large"),
        ("empty", ""),
    ];

    for (case, token) in cases {
        let params = vec![("NextToken".to_string(), token.to_string())];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for NextToken {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
        assert!(
            element(&body, "instanceTypeOfferingSet").is_none(),
            "case {case:?}: a refused request reports no offerings, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: DryRun
// ---------------------------------------------------------------------------

/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, while the
/// same call without it answers the Region's offerings.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let (router, _engine) = super::ec2_query::emulator();
    for case in ["DryRun alone", "DryRun beside a filter"] {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        if case.contains("filter") {
            params.extend(filters(&[("instance-type", &["m5.large"])]));
        }
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected 412, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "instanceTypeOfferingSet").is_none(),
            "case {case:?}: a dry run reports no offerings, got {body}"
        );
    }

    // `DryRun=false` is not a dry run: the request is performed.
    let params = vec![("DryRun".to_string(), "false".to_string())];
    let (status, body) = describe(&router, "us-east-1", &params).await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected 200, got {status} {body}"
    );
    assert!(
        !offerings("DryRun=false", &body).is_empty(),
        "case \"DryRun=false\": expected the Region's offerings, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the describe is a read
// ---------------------------------------------------------------------------

/// Repeating the describe answers the same offerings: the call reports facts of a Region
/// and persists nothing, so no caller can see a later call differ.
///
/// The rows differ in what the repeated request asks for: the whole Region, one zone's
/// offerings, and one page of a walk — the last also pins that a token keeps its meaning
/// across calls.
#[tokio::test]
async fn repeating_the_describe_answers_the_same_offerings() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, Vec<(String, String)>); 3] = [
        ("the whole Region", Vec::new()),
        ("one zone", {
            let mut params = location_type("availability-zone");
            params.extend(filters(&[("location", &["us-east-1c"])]));
            params
        }),
        ("one page", {
            let mut params = vec![("MaxResults".to_string(), "5".to_string())];
            params.extend(filters(&[("instance-type", &["m5*"])]));
            params
        }),
    ];

    for (case, params) in cases {
        let (first_status, first_body) = describe(&router, "us-east-1", &params).await;
        let (second_status, second_body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            first_status, 200,
            "case {case:?}: expected 200, got {first_status} {first_body}"
        );
        assert_eq!(
            second_status, first_status,
            "case {case:?}: a repeat answers the same status, got {second_status}"
        );
        let first = offerings(case, &first_body);
        let second = offerings(case, &second_body);
        assert!(
            !first.is_empty(),
            "case {case:?}: expected offerings to compare, got {first_body}"
        );
        assert_eq!(
            first, second,
            "case {case:?}: a repeat answers the same offerings in the same order"
        );
        assert_eq!(
            element(&first_body, "nextToken"),
            element(&second_body, "nextToken"),
            "case {case:?}: a repeat answers the same token"
        );
    }
}
