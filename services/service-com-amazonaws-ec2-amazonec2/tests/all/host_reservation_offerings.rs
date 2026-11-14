// SPDX-License-Identifier: CC0-1.0

//! `DescribeHostReservationOfferings` end-to-end: which Dedicated Host reservation
//! offerings the service publishes, how they are priced, and how a caller narrows and
//! pages them.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules under test:
//!
//! 1. The published catalog is every instance family of the modelled `InstanceType`
//!    enum, on every value of the modelled `PaymentOption` enum, for each term the
//!    duration members document (one year and three years), each entry carrying an
//!    `hro-<17 hex>` id that is unique within the catalog.
//! 2. A payment option names WHEN the term is paid for, so for one term the three
//!    options split the same total: `AllUpfront` quotes no hourly price, `NoUpfront` no
//!    upfront price, `PartialUpfront` half each way — and a three-year term's total is
//!    three times a one-year term's.
//! 3. `Filter` (`instance-family`, `payment-option`) are ANDed with one another and
//!    ORed over each filter's values, with EC2's `*` and `?` wildcards; a filter the
//!    operation does not document is refused with `InvalidParameterValue`, and so is a
//!    request past this service's filter-count, values-per-filter or value-size ceiling.
//! 4. `MinDuration` / `MaxDuration` bound the term inclusively, and `OfferingId` selects
//!    the one offering carrying that id — an id the catalog does not hold names none.
//! 5. `MaxResults` and `NextToken` page the matching offerings: each page holds at most
//!    `MaxResults` items, the walk reports every matching offering exactly once, and the
//!    last page carries no token. A token this service did not mint is refused with
//!    `InvalidPaginationToken`, and `MaxResults` outside the modelled 5..500 range is
//!    refused at the boundary.
//! 6. The describe is a read: repeating it answers the same offerings under the same
//!    ids, which is what lets a caller purchase an offering an earlier call reported.
//!
//! Offering ORDER within a response is deliberately not asserted — EC2 says the order of
//! elements in a response might vary — so every comparison here is over a sorted set.
//! What the pagination test does rely on is that the order is STABLE across calls, which
//! is what makes a cursor token see each offering exactly once.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES, MAX_FILTERS};

use super::ec2_query::{element, elements, error_code, error_message, filters, unescaped};

/// Any Region: an offering is a published price-list entry, and the results "describe all
/// of the Dedicated Host reservation offerings, including offerings that might not match
/// the instance family and Region of your Dedicated Hosts", so the Region the request was
/// signed for does not narrow them.
const REGION: &str = "us-east-1";

/// The seconds in a one-year term, as the duration members document them: "the number of
/// seconds in a year (365x24x60x60) times one of the supported durations (1 or 3)"
/// (`DescribeHostReservationOfferingsRequest$MinDuration`).
const ONE_YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// The seconds in a three-year term, from the same sentence.
const THREE_YEAR_SECONDS: i64 = 3 * ONE_YEAR_SECONDS;

/// One `DescribeHostReservationOfferings` call carrying `params`, as its HTTP status and
/// XML body.
async fn describe(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeHostReservationOfferings", params).await
}

/// One offering of a `DescribeHostReservationOfferings` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Offering {
    offering_id: String,
    instance_family: String,
    payment_option: String,
    currency_code: String,
    duration: i64,
    /// The hourly price, in thousandths of the currency — see [`milli`].
    hourly_price_milli: i64,
    /// The price due at purchase, in thousandths of the currency.
    upfront_price_milli: i64,
}

impl Offering {
    /// What the whole term costs, in thousandths: what is due at purchase plus the hourly
    /// price for every hour of the term.
    fn total_milli(&self) -> i64 {
        self.upfront_price_milli + self.hourly_price_milli * (self.duration / 3_600)
    }
}

/// A price as the model carries it — a decimal `String` with three places (`"1.499"`) —
/// read as whole thousandths of the currency, so the test's arithmetic is exact.
fn milli(case: &str, price: &str) -> i64 {
    let (whole, fraction) = price
        .split_once('.')
        .unwrap_or_else(|| panic!("case {case:?}: a price is quoted with decimals, got {price:?}"));
    assert_eq!(
        fraction.len(),
        3,
        "case {case:?}: a price is quoted with three decimal places, got {price:?}"
    );
    let whole: i64 = whole
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: a numeric price, got {price:?}"));
    let fraction: i64 = fraction
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: a numeric price, got {price:?}"));
    whole * 1_000 + fraction
}

/// Every offering in an ec2Query `DescribeHostReservationOfferings` body, in the order the
/// response carried them. Each offering is one `<item>` of the `<offeringSet>` list — and
/// the list's first element carries an `xmlns` attribute, so the opening tag is read up to
/// its `>`.
fn offerings(case: &str, body: &str) -> Vec<Offering> {
    let list = element(body, "offeringSet").unwrap_or_default();
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
                offering_id: member("offeringId"),
                instance_family: member("instanceFamily"),
                payment_option: member("paymentOption"),
                currency_code: member("currencyCode"),
                duration: member("duration")
                    .parse()
                    .unwrap_or_else(|_| panic!("case {case:?}: a numeric duration in {item:?}")),
                hourly_price_milli: milli(case, &member("hourlyPrice")),
                upfront_price_milli: milli(case, &member("upfrontPrice")),
            }
        })
        .collect()
}

/// Every offering a request reports over a whole `NextToken` walk, with the number of
/// pages it took. Each page is asserted to hold at most `page_size` offerings when the
/// request names one.
async fn walk(
    router: &MultiServiceRouter,
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
        let (status, body) = describe(router, &call_params).await;
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

/// The instance families the catalog is expected to cover: the family of every instance
/// type the modelled `InstanceType` enum declares
/// (`crate::ops::types::INSTANCE_TYPE_VALUES`, from
/// `smithy_query(query="shape",shape="InstanceType")`), split by the service's own
/// `instance_family_of`, which is the same catalog `DescribeInstanceTypeOfferings`
/// reports offerings for.
fn expected_families() -> BTreeSet<String> {
    ec2::ops::types::INSTANCE_TYPE_VALUES
        .iter()
        .map(|instance_type| ec2::state::instance_family_of(instance_type).to_string())
        .collect()
}

/// The payment options the catalog is expected to cover: every value of the modelled
/// `PaymentOption` enum (`smithy_query(query="shape",shape="PaymentOption")`).
fn expected_payment_options() -> BTreeSet<String> {
    ec2::ops::types::PAYMENT_OPTION_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}

/// `Filter`/`MinDuration`/`MaxDuration`/`OfferingId` form fields, as one named field.
fn field(name: &str, value: &str) -> Vec<(String, String)> {
    vec![(name.to_string(), value.to_string())]
}

// ---------------------------------------------------------------------------
// Rule 1: the published catalog
// ---------------------------------------------------------------------------

/// The catalog is exactly every modelled instance family × every modelled payment option
/// × the one-year and three-year terms, each entry under a unique `hro-<17 hex>` id.
///
/// The families and options are read from the value lists the model generated, so the
/// assertion is over the whole class rather than over a sample: a family the model
/// declares but the emulator withheld, and an offering it published for something the
/// model does not declare, both fail here. The catalog spans more than one page, so it is
/// collected over a `NextToken` walk.
#[tokio::test]
async fn the_catalog_is_every_family_option_and_term() {
    let (router, _engine) = super::ec2_query::emulator();
    let families = expected_families();
    let options = expected_payment_options();
    assert!(
        families.len() > 50,
        "case catalog: the modelled InstanceType enum should carry the whole fleet, got {}",
        families.len()
    );

    let (walked, pages) = walk(&router, "catalog", &[], None).await;
    assert!(
        pages > 1,
        "case catalog: the catalog should span several pages, got {pages}"
    );

    let expected: BTreeSet<(String, String, i64)> = families
        .iter()
        .flat_map(|family| {
            options.iter().flat_map(move |option| {
                [ONE_YEAR_SECONDS, THREE_YEAR_SECONDS]
                    .into_iter()
                    .map(move |duration| (family.clone(), option.clone(), duration))
            })
        })
        .collect();
    let reported: BTreeSet<(String, String, i64)> = walked
        .iter()
        .map(|offering| {
            (
                offering.instance_family.clone(),
                offering.payment_option.clone(),
                offering.duration,
            )
        })
        .collect();
    assert_eq!(
        reported.len(),
        walked.len(),
        "case catalog: each family/option/term is offered once, got {} offerings",
        walked.len()
    );
    assert_eq!(
        reported,
        expected,
        "case catalog: the offerings must be exactly the modelled product; \
         missing {:?}, unexpected {:?}",
        expected.difference(&reported).take(8).collect::<Vec<_>>(),
        reported.difference(&expected).take(8).collect::<Vec<_>>()
    );

    // Entries the issue this suite closes did not cite, pinned by name.
    for pinned in ["mac1", "r5b", "u-6tb1"] {
        assert!(
            families.contains(pinned),
            "case catalog: expected {pinned:?} among the modelled families, got {}",
            families.len()
        );
    }

    let ids: BTreeSet<&String> = walked.iter().map(|offering| &offering.offering_id).collect();
    assert_eq!(
        ids.len(),
        walked.len(),
        "case catalog: every offering carries its own id, got {} ids for {} offerings",
        ids.len(),
        walked.len()
    );
    for offering in &walked {
        let suffix = offering
            .offering_id
            .strip_prefix("hro-")
            .unwrap_or_else(|| panic!("case catalog: an offering id reads hro-<hex>, got {offering:?}"));
        assert!(
            suffix.len() == 17 && suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "case catalog: an offering id reads hro-<17 lowercase hex>, got {offering:?}"
        );
        assert_eq!(
            offering.currency_code, "USD",
            "case catalog: the only modelled CurrencyCodeValues value is USD, got {offering:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: what a payment option says about the price
// ---------------------------------------------------------------------------

/// For one term, the three payment options split the SAME total, each the way its own
/// name says; and a three-year term costs three times a one-year term.
///
/// The rows span every value of the modelled `PaymentOption` enum against both terms, and
/// each row is checked on two families, so neither the split nor the total can be a
/// property of one family. The expectation is stated as the relation between the two
/// price members rather than as a price list: AWS publishes its prices outside the API, so
/// what the emulator owes a caller is a self-consistent pair.
#[tokio::test]
async fn each_payment_option_splits_the_same_term_total() {
    let (router, _engine) = super::ec2_query::emulator();

    for family in ["m4", "c6g"] {
        let params = filters(&[("instance-family", &[family])]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {family}: expected 200, got {status} {body}"
        );
        let reported = offerings(family, &body);
        assert_eq!(
            reported.len(),
            expected_payment_options().len() * 2,
            "case {family}: expected one offering per option and term, got {reported:?}"
        );

        for duration in [ONE_YEAR_SECONDS, THREE_YEAR_SECONDS] {
            let term: Vec<&Offering> = reported
                .iter()
                .filter(|offering| offering.duration == duration)
                .collect();
            let hours = duration / 3_600;
            let total = term
                .first()
                .unwrap_or_else(|| panic!("case {family}/{duration}: no offering for the term"))
                .total_milli();
            assert!(
                total > 0,
                "case {family}/{duration}: a term costs something, got {term:?}"
            );
            for offering in &term {
                let case = format!("{family}/{duration}/{}", offering.payment_option);
                assert_eq!(
                    offering.total_milli(),
                    total,
                    "case {case:?}: every option splits the same total {total}, got {offering:?}"
                );
                match offering.payment_option.as_str() {
                    "AllUpfront" => {
                        assert_eq!(
                            (offering.hourly_price_milli, offering.upfront_price_milli),
                            (0, total),
                            "case {case:?}: All Upfront pays the whole term at purchase, got {offering:?}"
                        );
                    }
                    // "Does not apply to No Upfront offerings"
                    // (`HostOffering$UpfrontPrice`), so it is quoted as zero.
                    "NoUpfront" => {
                        assert_eq!(
                            (offering.hourly_price_milli, offering.upfront_price_milli),
                            (total / hours, 0),
                            "case {case:?}: No Upfront pays the whole term hourly, got {offering:?}"
                        );
                    }
                    "PartialUpfront" => {
                        assert_eq!(
                            (offering.hourly_price_milli, offering.upfront_price_milli),
                            (total / hours / 2, total / 2),
                            "case {case:?}: Partial Upfront pays half each way, got {offering:?}"
                        );
                    }
                    other => panic!("case {case:?}: {other:?} is not a modelled PaymentOption"),
                }
            }
        }

        let total_of = |duration: i64| {
            reported
                .iter()
                .find(|offering| {
                    offering.duration == duration && offering.payment_option == "AllUpfront"
                })
                .map(Offering::total_milli)
                .unwrap_or_else(|| panic!("case {family}: no All Upfront offering for {duration}"))
        };
        assert_eq!(
            total_of(THREE_YEAR_SECONDS),
            3 * total_of(ONE_YEAR_SECONDS),
            "case {family}: a three-year term costs three times a one-year term"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the documented filters
// ---------------------------------------------------------------------------

/// Each documented filter narrows the field it names, values inside one filter are ORed,
/// and two filters are ANDed.
///
/// The rows differ in which field is filtered, in how many values the filter carries, and
/// in the form of the value: an exact family, two families, a `*` wildcard, a `?`
/// wildcard, one payment option, two payment options, and both filters at once. Each row
/// states the families and options it expects as a predicate, so a row asserts the whole
/// matching set rather than a count.
#[tokio::test]
async fn the_documented_filters_narrow_the_catalog() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, Filter fields, is a family expected, is an option expected)
    type Row = (
        &'static str,
        Vec<(String, String)>,
        fn(&str) -> bool,
        fn(&str) -> bool,
    );
    let rows: [Row; 7] = [
        (
            "one family",
            filters(&[("instance-family", &["m4"])]),
            |family| family == "m4",
            |_| true,
        ),
        (
            "two families (OR)",
            filters(&[("instance-family", &["m4", "c5"])]),
            |family| family == "m4" || family == "c5",
            |_| true,
        ),
        (
            "a * wildcard",
            filters(&[("instance-family", &["m5*"])]),
            |family| family.starts_with("m5"),
            |_| true,
        ),
        (
            "a ? wildcard",
            filters(&[("instance-family", &["?4"])]),
            |family| family.len() == 2 && family.ends_with('4'),
            |_| true,
        ),
        (
            "one payment option",
            filters(&[("payment-option", &["AllUpfront"])]),
            |_| true,
            |option| option == "AllUpfront",
        ),
        (
            "two payment options (OR)",
            filters(&[("payment-option", &["AllUpfront", "NoUpfront"])]),
            |_| true,
            |option| option == "AllUpfront" || option == "NoUpfront",
        ),
        (
            "both filters (AND)",
            filters(&[
                ("instance-family", &["m4"]),
                ("payment-option", &["NoUpfront"]),
            ]),
            |family| family == "m4",
            |option| option == "NoUpfront",
        ),
    ];

    for (case, params, family_expected, option_expected) in rows {
        let (walked, _pages) = walk(&router, case, &params, None).await;
        assert!(
            !walked.is_empty(),
            "case {case:?}: expected the filter to match something"
        );
        for offering in &walked {
            assert!(
                family_expected(&offering.instance_family),
                "case {case:?}: {:?} is outside the filtered families",
                offering.instance_family
            );
            assert!(
                option_expected(&offering.payment_option),
                "case {case:?}: {:?} is outside the filtered options",
                offering.payment_option
            );
        }
        // Everything the filter admits is reported, not merely a subset of it: each
        // admitted family carries every admitted option on both terms.
        let families: BTreeSet<&String> = walked
            .iter()
            .map(|offering| &offering.instance_family)
            .collect();
        let options: BTreeSet<&String> = walked
            .iter()
            .map(|offering| &offering.payment_option)
            .collect();
        assert_eq!(
            walked.len(),
            families.len() * options.len() * 2,
            "case {case:?}: expected {} families x {} options x 2 terms, got {}",
            families.len(),
            options.len(),
            walked.len()
        );
        for family in expected_families()
            .iter()
            .filter(|family| family_expected(family))
        {
            assert!(
                families.contains(family),
                "case {case:?}: expected {family:?} among the reported families"
            );
        }
        for option in expected_payment_options()
            .iter()
            .filter(|option| option_expected(option))
        {
            assert!(
                options.contains(option),
                "case {case:?}: expected {option:?} among the reported options"
            );
        }
    }
}

/// A filter this operation does not document is refused with HTTP 400
/// `InvalidParameterValue` naming it, rather than ignored.
///
/// The rows differ in how the name is wrong: another operation's filter, the member's own
/// casing, an underscore for the hyphen, and the empty name.
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, &str); 4] = [
        ("another operation's filter", "instance-type"),
        ("member-cased", "InstanceFamily"),
        ("underscored", "payment_option"),
        ("empty", ""),
    ];

    for (case, name) in cases {
        let params = filters(&[(name, &["m4"])]);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for filter {name:?}, got {status} {body}"
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
        assert!(
            elements(&body, "offeringId").is_empty(),
            "case {case:?}: a refused request reports no offerings, got {body}"
        );
    }
}

/// A request at each filtering ceiling is served, and one step past any of them is
/// refused with `InvalidParameterValue` reporting no offering.
///
/// The model declares no `@length` on `FilterList`, so the filter count, the values in
/// one filter and each value's size are bounded by this service's own caps; the rows are
/// one per factor, each checked at the ceiling and one step past it, because a request
/// asking for more comparisons than one call will do is told so rather than served a
/// narrowing that stopped half way.
#[tokio::test]
async fn a_request_past_a_filtering_ceiling_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    let at_filter_ceiling: Vec<(&str, &[&str])> = (0..MAX_FILTERS)
        .map(|_| ("instance-family", &["m4"][..]))
        .collect();
    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("instance-family", &["m4"][..]))
        .collect();
    let many_values: Vec<String> = (0..MAX_FILTER_VALUES).map(|n| format!("m{n}")).collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let past_value_ceiling: Vec<String> = (0..=MAX_FILTER_VALUES).map(|n| format!("m{n}")).collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);
    let past_byte_ceiling = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);

    /// One row of the ceiling table: the case name, the `Filter` fields the request
    /// carries, and whether the request is inside every ceiling.
    type CeilingRow = (&'static str, Vec<(String, String)>, bool);
    let cases: Vec<CeilingRow> = vec![
        ("MAX_FILTERS filters", filters(&at_filter_ceiling), true),
        (
            "one filter past MAX_FILTERS",
            filters(&past_filter_ceiling),
            false,
        ),
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("instance-family", &many_values)]),
            true,
        ),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("instance-family", &past_value_ceiling)]),
            false,
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("instance-family", &[long_value.as_str()])]),
            true,
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("instance-family", &[past_byte_ceiling.as_str()])]),
            false,
        ),
    ];

    for (case, params, inside) in cases {
        let (status, body) = describe(&router, &params).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: a request at the ceiling is served, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                elements(&body, "offeringId").is_empty(),
                "case {case:?}: a refused request reports no offering, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the duration bounds and OfferingId
// ---------------------------------------------------------------------------
/// `MinDuration` and `MaxDuration` bound the term inclusively, and a value that is not a
/// term reservations are offered for is refused.
///
/// "The number of seconds specified must be the number of seconds in a year (365x24x60x60)
/// times one of the supported durations (1 or 3)" (`$MinDuration`, and `$MaxDuration` word
/// for word) — so the rows are each published term as each bound, the pair that spans both,
/// and, for the refusal, a value one second off a term at each end.
#[tokio::test]
async fn the_duration_bounds_select_the_terms() {
    let (router, _engine) = super::ec2_query::emulator();
    let one_year = ONE_YEAR_SECONDS.to_string();
    let three_years = THREE_YEAR_SECONDS.to_string();
    /// One row of the duration-bound table: the case name, the `MinDuration` and the
    /// `MaxDuration` the request carries, and the terms expected back.
    type Row<'a> = (&'static str, Option<&'a str>, Option<&'a str>, Vec<i64>);
    let rows: [Row<'_>; 6] = [
        (
            "no bound",
            None,
            None,
            vec![ONE_YEAR_SECONDS, THREE_YEAR_SECONDS],
        ),
        (
            "min at the one-year bound",
            Some(one_year.as_str()),
            None,
            vec![ONE_YEAR_SECONDS, THREE_YEAR_SECONDS],
        ),
        (
            "min at the three-year bound",
            Some(three_years.as_str()),
            None,
            vec![THREE_YEAR_SECONDS],
        ),
        (
            "max at the one-year bound",
            None,
            Some(one_year.as_str()),
            vec![ONE_YEAR_SECONDS],
        ),
        (
            "max at the three-year bound",
            None,
            Some(three_years.as_str()),
            vec![ONE_YEAR_SECONDS, THREE_YEAR_SECONDS],
        ),
        (
            "both bounds span the terms",
            Some(one_year.as_str()),
            Some(three_years.as_str()),
            vec![ONE_YEAR_SECONDS, THREE_YEAR_SECONDS],
        ),
    ];

    for (case, minimum, maximum, expected) in rows {
        let mut params = filters(&[("instance-family", &["m4"])]);
        if let Some(minimum) = minimum {
            params.extend(field("MinDuration", minimum));
        }
        if let Some(maximum) = maximum {
            params.extend(field("MaxDuration", maximum));
        }
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let reported: BTreeSet<i64> = offerings(case, &body)
            .iter()
            .map(|offering| offering.duration)
            .collect();
        let expected: BTreeSet<i64> = expected.into_iter().collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the terms {expected:?}, got {reported:?}"
        );
    }

    // A bound that is not a term reservations are offered for names no term at all, so it is
    // refused rather than used as a bound that quietly matches everything or nothing. The
    // rows are one second off a published term at each end, for each of the two members.
    for (case, member, value) in [
        ("min one past the one-year term", "MinDuration", "31536001"),
        ("min one past the three-year term", "MinDuration", "94608001"),
        ("max one below the one-year term", "MaxDuration", "31535999"),
        ("max one below the three-year term", "MaxDuration", "94607999"),
    ] {
        let mut params = filters(&[("instance-family", &["m4"])]);
        params.extend(field(member, value));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a bound that is no term, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            elements(&body, "offeringId").is_empty(),
            "case {case:?}: a refused request reports no offering, got {body}"
        );
    }
}

/// `OfferingId` reports the one offering carrying that id, and an id the catalog does not
/// publish is refused rather than reported as no offering.
///
/// The ids that are expected to resolve are read back out of an unfiltered describe, over
/// rows spanning the payment options and both terms, so the test names no id of its own.
/// The rows that resolve to nothing are ids the service CAN read and the catalog does not
/// publish — an id past the catalog's end, and one at the top of the form's range — and an
/// offering id names ONE entry of the price list, so an id this catalog did not issue is
/// refused under the id's own class exactly as a value that is no offering id at all is
/// (`an_offering_id_that_is_not_one_is_refused`).
#[tokio::test]
async fn an_offering_id_selects_one_offering() {
    let (router, _engine) = super::ec2_query::emulator();
    let params = filters(&[("instance-family", &["r5"])]);
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "case seed: expected 200, got {status} {body}");
    let catalog = offerings("seed", &body);
    assert_eq!(
        catalog.len(),
        expected_payment_options().len() * 2,
        "case seed: expected one offering per option and term, got {catalog:?}"
    );

    for expected in &catalog {
        let case = format!("{} {}", expected.payment_option, expected.duration);
        let (status, body) = describe(&router, &field("OfferingId", &expected.offering_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let reported = offerings(&case, &body);
        assert_eq!(
            reported,
            vec![expected.clone()],
            "case {case:?}: expected exactly the offering the id names"
        );
        assert!(
            element(&body, "nextToken").is_none(),
            "case {case:?}: one offering fits in a page, got {body}"
        );
    }

    // Ids of the catalog's own form that it does not publish: one just past its end, and
    // the largest value the form holds. An offering id names one entry of the price list, so
    // an id this catalog did not issue names no offering at all.
    let misses: [(&str, &str); 2] = [
        ("an unheld id of the right form", "hro-0ffffffffffffffff"),
        ("the largest id the form holds", "hro-0fffffffffffffffe"),
    ];
    for (case, offering_id) in misses {
        let (status, body) = describe(&router, &field("OfferingId", offering_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for OfferingId {offering_id:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidHostReservationOfferingId.Malformed",
            "case {case:?}: expected the offering id's own refusal, got {body}"
        );
        assert!(
            elements(&body, "offeringId").is_empty(),
            "case {case:?}: a refused request reports no offering, got {body}"
        );
    }
}

/// An `OfferingId` that is not a host reservation offering id AT ALL is refused with HTTP
/// 400 `InvalidHostReservationOfferingId.Malformed`, naming the value the request sent —
/// never with the empty offering set a readable id the catalog does not publish gets.
///
/// The rows span the ways a value falls outside the id form AWS issues
/// (`hro-03f707bf363b6b324`, the form the `OfferingId` member's examples name, read by
/// `ec2::ops::describe_host_reservation_offerings::host_offering_id_is_well_formed`): the
/// all-zero placeholder, a leading digit above zero (the long form carries 68 bits for a
/// 64-bit identifier, so an issued id always reads `hro-0…`), the retired short id form, a
/// suffix one digit too short and one too long, a non-hexadecimal digit, an upper-case
/// digit, another resource's prefix, a bare name, and the empty value.
#[tokio::test]
async fn an_offering_id_that_is_not_one_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    let rows: [(&str, &str); 11] = [
        ("the all-zero placeholder", "hro-00000000000000000"),
        ("a leading digit above zero", "hro-fffffffffffffffff"),
        ("a leading digit above zero, decimal", "hro-10000000000000000"),
        ("the retired short form", "hro-0abc1234"),
        ("one digit too short", "hro-0123456789abcdef"),
        ("one digit too long", "hro-0123456789abcdef01"),
        ("a non-hexadecimal digit", "hro-0g23456789abcdef0"),
        ("an upper-case digit", "hro-0123456789ABCDEF0"),
        ("another resource's prefix", "i-0123456789abcdef0"),
        ("a bare name", "not-an-offering"),
        ("the empty value", ""),
    ];

    for (case, offering_id) in rows {
        let (status, body) = describe(&router, &field("OfferingId", offering_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for OfferingId {offering_id:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidHostReservationOfferingId.Malformed",
            "case {case:?}: expected the malformed-offering-id code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The specified host reservation Offering ID '{offering_id}' is not valid."),
            "case {case:?}: expected the sentence naming the value the request sent, got {body}"
        );
        assert!(
            elements(&body, "offeringId").is_empty(),
            "case {case:?}: a refused request reports no offering, got {body}"
        );
    }

    // And every id the published catalog carries IS readable, so no offering is made
    // unreachable by the rule above.
    let (status, body) = describe(&router, &filters(&[("instance-family", &["m5"])])).await;
    assert_eq!(
        status, 200,
        "case catalog: expected 200, got {status} {body}"
    );
    let published = offerings("catalog", &body);
    assert!(
        !published.is_empty(),
        "case catalog: the catalog must publish offerings to check, got {body}"
    );
    for offering in &published {
        let (status, body) = describe(&router, &field("OfferingId", &offering.offering_id)).await;
        assert_eq!(
            status, 200,
            "case {id:?}: a published offering id must be readable, got {status} {body}",
            id = offering.offering_id
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: MaxResults / NextToken
// ---------------------------------------------------------------------------
/// A `NextToken` walk reports every matching offering exactly once, in pages of at most
/// `MaxResults`, and the last page carries no token.
///
/// The rows are page sizes spanning the modelled `DescribeHostReservationsMaxResults`
/// range 5..500 (`smithy_query(query="shape",shape="DescribeHostReservationsMaxResults")`):
/// its lower bound, a size that does not divide the result count, a size larger than the
/// result count, and its upper bound. The expectation is the un-paged answer to the same
/// request, so paging is asserted to change only the packaging.
#[tokio::test]
async fn a_next_token_walk_reports_every_offering_once() {
    let (router, _engine) = super::ec2_query::emulator();
    // `c5*` keeps the walk to a few dozen offerings while still spanning several
    // families, so a small page size means many pages.
    let narrowing = || filters(&[("instance-family", &["c5*"])]);

    let (status, body) = describe(&router, &narrowing()).await;
    assert_eq!(status, 200, "case unpaged: expected 200, got {status} {body}");
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

    for page_size in [5usize, 7, 500, unpaged.len() + 1] {
        let case = format!("MaxResults={page_size}");
        let (seen, pages) = walk(&router, &case, &narrowing(), Some(page_size)).await;
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
        let expected_pages = unpaged.len().div_ceil(page_size.min(500));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} offerings, got {pages}",
            unpaged.len()
        );
    }
}

/// `MaxResults` outside the modelled 5..500 range is refused, each published bound is
/// accepted, and the value one past each bound is not.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, MaxResults, is it inside the modelled range)
    let cases: [(&str, &str, bool); 6] = [
        ("one below the minimum", "4", false),
        ("the minimum", "5", true),
        ("inside", "250", true),
        ("the maximum", "500", true),
        ("one above the maximum", "501", false),
        ("zero", "0", false),
    ];

    for (case, value, inside) in cases {
        let params = field("MaxResults", value);
        let (status, body) = describe(&router, &params).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={value} is inside 5..500, got {status} {body}"
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
                "case {case:?}: MaxResults={value} is outside 5..500, got {status} {body}"
            );
            assert!(
                elements(&body, "offeringId").is_empty(),
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
        ("an unencoded cursor", "m4 0031536000 NoUpfront"),
        ("empty", ""),
    ];

    for (case, token) in cases {
        let params = field("NextToken", token);
        let (status, body) = describe(&router, &params).await;
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
            elements(&body, "offeringId").is_empty(),
            "case {case:?}: a refused request reports no offerings, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: the describe is a read
// ---------------------------------------------------------------------------

/// Repeating the describe answers the same offerings under the same ids, so nothing a
/// read does changes what a later caller sees — and an id one call reported can still be
/// named afterwards.
///
/// The rows differ in what the request selects: the whole catalog's first page, one
/// family, one payment option, and one offering by id.
#[tokio::test]
async fn repeating_the_describe_answers_the_same_offerings() {
    let (router, _engine) = super::ec2_query::emulator();
    let seed = filters(&[("instance-family", &["t3"])]);
    let (status, body) = describe(&router, &seed).await;
    assert_eq!(status, 200, "case seed: expected 200, got {status} {body}");
    let seeded = offerings("seed", &body);
    let seeded_id = seeded
        .first()
        .map(|offering| offering.offering_id.clone())
        .expect("case seed: expected an offering to name");

    let cases: [(&str, Vec<(String, String)>); 4] = [
        ("the first page of the catalog", field("MaxResults", "5")),
        ("one family", filters(&[("instance-family", &["t3"])])),
        (
            "one payment option",
            filters(&[("payment-option", &["PartialUpfront"])]),
        ),
        ("one offering by id", field("OfferingId", &seeded_id)),
    ];

    for (case, params) in cases {
        let (first_status, first_body) = describe(&router, &params).await;
        let (second_status, second_body) = describe(&router, &params).await;
        assert_eq!(
            first_status, 200,
            "case {case:?}: expected 200, got {first_status} {first_body}"
        );
        assert_eq!(
            second_status, 200,
            "case {case:?}: expected 200 on the repeat, got {second_status} {second_body}"
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
