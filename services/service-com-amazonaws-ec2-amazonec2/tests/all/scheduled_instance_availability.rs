// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeScheduledInstanceAvailability`, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test: A SEARCH REPORTS ONE SCHEDULE PER BLOCK OF SCHEDULED INSTANCE
//! CAPACITY THE SIGNED REGION HAS SET ASIDE, each carrying the schedule the request's own
//! recurrence and slot bounds resolve to, and nothing else. Three parts to it, and the tables
//! below span each:
//!
//! 1. WHICH blocks are reported: the stored capacity of the Region the call was signed for,
//!    narrowed by the three filters the operation documents, minus any block with no instance
//!    left — paged by `MaxResults`/`NextToken` in the one order the store is keyed in. An
//!    account whose Region has no capacity set aside is answered HTTP 200 with an empty
//!    `scheduledInstanceAvailabilitySet` and no token, which is a report of an empty
//!    inventory rather than a refusal, and is what a fresh account gets.
//! 2. WHAT each reported schedule says: the zone, instance type, platform, network platform,
//!    instance count and hourly price off the stored block; the term AWS publishes for a
//!    Scheduled Instance on both term members; and the slot — the SHORTEST whole number of
//!    hours that still adds up to the documented 1,200 hours a year for the recurrence asked
//!    for, raised to the caller's own minimum — with the term's hours following from it. The
//!    operation's own examples (4 hours daily, 24 weekly, 100 monthly) are three rows of that
//!    arithmetic, not the rule.
//! 3. WHAT a search finds nothing for and what it is refused for: criteria nothing can satisfy
//!    — a window that has closed, opens beyond the three months a search can see or whose
//!    latest start precedes its earliest, and valid slot bounds that leave no room — are SERVED
//!    with an empty report, while a request no schedule could be described by (a frequency
//!    outside the three AWS offers, a day outside the published range of its period, a duration
//!    of no hours, a maximum slot at or above the published limit or not above the minimum
//!    asked for, a filter the operation does not document, a token it did not mint) is refused.
//!    `DryRun` is answered as the probe it is.
//!
//! Reporting is a READ throughout: every suite below compares the stored capacity before and
//! after, and reads twice where the answer could have moved.

use ::aws_smithy_types::DateTime;
use ::aws_smithy_types::date_time::Format;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ScheduledInstanceCapacityRecord};

use super::ec2_query::{call, emulator, error_code, error_message, filters, member};
use super::wire::{child, child_text, document, set_items};

/// The operation every row calls.
const ACTION: &str = "DescribeScheduledInstanceAvailability";

/// The envelope member the schedules are reported in.
const AVAILABILITY_SET: &str = "scheduledInstanceAvailabilitySet";

/// The Region most rows are signed for. A second Region is used wherever the rule is about
/// which Region's capacity a search sees.
const REGION: &str = "us-west-2";

/// The term a Scheduled Instance is bought for, in days: "The minimum term. The only possible
/// value is 365 days" (`ScheduledInstanceAvailability$MinTermDurationInDays`), and the same
/// sentence on `MaxTermDurationInDays`. Written out here rather than read from the service, so
/// this suite pins the value instead of restating whatever the handler carries.
const TERM_DURATION_IN_DAYS: &str = "365";

/// The hours a schedule must add up to over its term: "You must meet the minimum required
/// duration of 1,200 hours per year" (`DescribeScheduledInstanceAvailability`).
const MIN_HOURS_PER_YEAR: i64 = 1_200;

/// The duration a slot must stay below: "This value must be greater than
/// MinSlotDurationInHours and less than 1,720"
/// (`DescribeScheduledInstanceAvailabilityRequest$MaxSlotDurationInHours`).
const SLOT_DURATION_LIMIT_HOURS: i64 = 1_720;

/// The bounds of `DescribeScheduledInstanceAvailabilityMaxResults`, the modelled `@range` on
/// this operation's `MaxResults` — source:
/// `smithy_query(query="operation",operation="DescribeScheduledInstanceAvailability")`, which
/// reports `range {min: 5, max: 300}`. A page size outside it is refused by the boundary
/// validator before the handler runs, so the rows stay inside the range.
const MIN_MAX_RESULTS: i64 = 5;
const MAX_MAX_RESULTS: i64 = 300;

/// The platforms a Scheduled Instance runs — "The platform (`Linux/UNIX` or `Windows`)"
/// (`ScheduledInstanceAvailability$Platform`), which is also what the `platform` filter
/// selects on.
const LINUX: &str = "Linux/UNIX";
const WINDOWS: &str = "Windows";

/// The network platform a block of capacity in a Region that has only VPCs carries.
const NETWORK_PLATFORM: &str = "EC2-VPC";

/// The instance types the rows set capacity aside for: "You can specify one of the C3, C4, M4,
/// or R3 instance types" (`ScheduledInstanceAvailability$InstanceType`). Two of that catalog,
/// so a row can tell one block from another by type.
const C4_LARGE: &str = "c4.large";
const M4_XLARGE: &str = "m4.xlarge";

/// One form field of a request.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Now, in seconds since the Unix epoch, read the way the service reads the clock.
fn now_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

/// `secs` as the ISO 8601 UTC instant a `DateTime` member is written in.
fn iso8601(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .expect("an instant inside the representable range")
}

/// The `FirstSlotStartTimeRange` fields for a window that opens `opens_in` seconds from now
/// and runs for `span` seconds.
fn window(opens_in: i64, span: i64) -> Params {
    let opens_at = now_secs() + opens_in;
    vec![
        p(
            "FirstSlotStartTimeRange.EarliestTime",
            iso8601(opens_at),
        ),
        p("FirstSlotStartTimeRange.LatestTime", iso8601(opens_at + span)),
    ]
}

/// The window most rows search in: it opens an hour from now and stays open for an hour, which
/// is inside both published bounds — less than a day wide and under three months ahead.
fn open_window() -> Params {
    window(60 * 60, 60 * 60)
}

/// The `Recurrence` fields for a schedule of `frequency`, every `interval` units, on
/// `occurrence_days`, counted in `occurrence_unit`.
fn recurrence(
    frequency: &str,
    interval: Option<i64>,
    occurrence_days: &[i64],
    occurrence_unit: Option<&str>,
) -> Params {
    let mut fields = vec![p("Recurrence.Frequency", frequency)];
    if let Some(interval) = interval {
        fields.push(p("Recurrence.Interval", interval.to_string()));
    }
    for (index, day) in occurrence_days.iter().enumerate() {
        fields.push(p(
            &format!("Recurrence.OccurrenceDay.{}", index + 1),
            day.to_string(),
        ));
    }
    if let Some(unit) = occurrence_unit {
        fields.push(p("Recurrence.OccurrenceUnit", unit));
    }
    fields
}

/// The simplest schedule a search can ask for: every day, which needs no occurrence day and no
/// occurrence unit.
fn daily() -> Params {
    recurrence("Daily", None, &[], None)
}

/// A search with `extra` beside a well-formed window and a daily recurrence.
fn search(extra: Params) -> Params {
    let mut params = open_window();
    params.extend(daily());
    params.extend(extra);
    params
}

/// One block of capacity to set aside: its zone, platform, instance type, how many instances
/// are left in it and what an hour of one costs.
type Block<'a> = (&'a str, &'a str, &'a str, i32, &'a str);

/// Set `blocks` aside in the store, answering the keys they were stored under — the order a
/// page of them is reported in.
///
/// Nothing in the EC2 API sets Scheduled Instance capacity aside (AWS does that itself), so a
/// suite that needs an inventory writes one through the store's own writer, the way the local
/// gateway suites seed the Outpost hardware no EC2 call racks.
fn seed(engine: &engine::InMemoryEngine, blocks: &[Block<'_>]) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    blocks
        .iter()
        .map(|(zone, platform, instance_type, count, price)| {
            guard.insert_scheduled_instance_capacity(ScheduledInstanceCapacityRecord {
                availability_zone: (*zone).to_string(),
                instance_type: (*instance_type).to_string(),
                platform: (*platform).to_string(),
                network_platform: NETWORK_PLATFORM.to_string(),
                available_instance_count: *count,
                hourly_price: (*price).to_string(),
            })
        })
        .collect()
}

/// The capacity the store holds, as `(key, instances left)` pairs: compared before and after a
/// read, so a report that quietly drew capacity down or wrote a block of its own is visible.
fn stored_capacity(engine: &engine::InMemoryEngine) -> Vec<(String, i32)> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .scheduled_instance_capacity
        .iter()
        .map(|(key, record)| (key.clone(), record.available_instance_count))
        .collect()
}

/// One schedule as the response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    availability_zone: String,
    instance_type: String,
    platform: String,
    network_platform: String,
    available_instance_count: String,
    hourly_price: String,
    min_term_duration_in_days: String,
    max_term_duration_in_days: String,
    slot_duration_in_hours: String,
    total_scheduled_instance_hours: String,
    first_slot_start_time: String,
    purchase_token: String,
    frequency: String,
    interval: String,
    occurrence_days: Vec<String>,
    occurrence_relative_to_end: String,
    occurrence_unit: Option<String>,
}

/// Every schedule a successful response reports, in the order it reports them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let text = |item: roxmltree::Node<'_, '_>, name: &str| -> String {
        child_text(item, name).unwrap_or_default()
    };
    set_items(parsed.root_element(), AVAILABILITY_SET)
        .into_iter()
        .map(|item| {
            let recurrence = child(item, "recurrence");
            Reported {
                availability_zone: text(item, "availabilityZone"),
                instance_type: text(item, "instanceType"),
                platform: text(item, "platform"),
                network_platform: text(item, "networkPlatform"),
                available_instance_count: text(item, "availableInstanceCount"),
                hourly_price: text(item, "hourlyPrice"),
                min_term_duration_in_days: text(item, "minTermDurationInDays"),
                max_term_duration_in_days: text(item, "maxTermDurationInDays"),
                slot_duration_in_hours: text(item, "slotDurationInHours"),
                total_scheduled_instance_hours: text(item, "totalScheduledInstanceHours"),
                first_slot_start_time: text(item, "firstSlotStartTime"),
                purchase_token: text(item, "purchaseToken"),
                frequency: recurrence
                    .map(|node| text(node, "frequency"))
                    .unwrap_or_default(),
                interval: recurrence
                    .map(|node| text(node, "interval"))
                    .unwrap_or_default(),
                occurrence_days: recurrence
                    .map(|node| {
                        set_items(node, "occurrenceDaySet")
                            .into_iter()
                            .map(|day| day.text().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                occurrence_relative_to_end: recurrence
                    .map(|node| text(node, "occurrenceRelativeToEnd"))
                    .unwrap_or_default(),
                occurrence_unit: recurrence.and_then(|node| child_text(node, "occurrenceUnit")),
            }
        })
        .collect()
}

/// The zones of each reported schedule, with its instance type and platform: the identity of
/// the block it was computed from, which is what the ordering and filtering rows compare.
fn reported_blocks(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|schedule| {
            format!(
                "{} {} {}",
                schedule.availability_zone, schedule.platform, schedule.instance_type
            )
        })
        .collect()
}

/// Call the operation for `region` with `params`, asserting it was served, and answer the body.
async fn served(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    region: &str,
    params: &Params,
) -> String {
    let (status, body) = call(router, region, ACTION, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// The `nextToken` a response carries, or `None` when it carries none or an empty one.
fn token(body: &str) -> Option<String> {
    member(body, "nextToken").filter(|token| !token.is_empty())
}

// ---------------------------------------------------------------------------
// Rule 1: which blocks a search is answered with
// ---------------------------------------------------------------------------

/// An account whose Regions have no Scheduled Instance capacity set aside is answered a REPORT
/// of that — HTTP 200 with the availability set present and empty, and no token — for every
/// shape of well-formed search, in every Region, and the store stays empty.
///
/// The rows differ along everything the search generalises over: the frequency and its
/// interval, the occurrence days and the unit they are counted in, whether the occurrence is
/// relative to the end of the period, each documented filter, the page size at both published
/// bounds and between them, the slot bounds, and the Region the call is signed for. No row is
/// the specification; what they share is that nothing is set aside to report.
#[tokio::test]
async fn a_region_with_no_capacity_reports_an_empty_set_for_every_search() {
    let (router, engine) = emulator();

    let mut rows: Vec<(String, Params)> = vec![
        ("a daily schedule and nothing else".to_string(), search(Vec::new())),
        (
            "a daily schedule every third day".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Daily", Some(3), &[], None));
                params
            },
        ),
        (
            "a weekly schedule on one day".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Weekly", Some(1), &[3], None));
                params
            },
        ),
        (
            "a monthly schedule on two days of the month".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Monthly", Some(1), &[1, 15], Some("DayOfMonth")));
                params
            },
        ),
        (
            "a monthly schedule relative to the end of the month".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Monthly", None, &[1], Some("DayOfMonth")));
                params.push(p("Recurrence.OccurrenceRelativeToEnd", "true"));
                params
            },
        ),
        (
            "slot bounds around the minimum a daily schedule needs".to_string(),
            search(vec![
                p("MinSlotDurationInHours", "4"),
                p("MaxSlotDurationInHours", "12"),
            ]),
        ),
        (
            "DryRun asked for and declined".to_string(),
            search(vec![p("DryRun", "false")]),
        ),
        (
            "a window that opens in three weeks".to_string(),
            {
                let mut params = window(21 * 24 * 60 * 60, 2 * 60 * 60);
                params.extend(daily());
                params
            },
        ),
        (
            "a window with no width at all".to_string(),
            {
                let mut params = window(60 * 60, 0);
                params.extend(daily());
                params
            },
        ),
    ];
    for max_results in [MIN_MAX_RESULTS, 42, MAX_MAX_RESULTS] {
        rows.push((
            format!("a page size of {max_results}"),
            search(vec![p("MaxResults", max_results.to_string())]),
        ));
    }
    for (name, value) in [
        ("availability-zone", "us-west-2a"),
        ("instance-type", C4_LARGE),
        ("platform", LINUX),
    ] {
        rows.push((
            format!("the documented filter {name}"),
            search(filters(&[(name, &[value])])),
        ));
    }

    let baseline = stored_capacity(&engine);
    for region in [REGION, "us-east-1", "eu-central-1"] {
        for (label, params) in &rows {
            let case = format!("{label}, signed for {region}");
            let body = served(&router, &case, region, params).await;
            assert_eq!(
                member(&body, AVAILABILITY_SET).as_deref(),
                Some(""),
                "case {case:?}: an inventory with nothing set aside is reported as the \
                 availability set present and empty, got {body}"
            );
            assert!(
                reported(&case, &body).is_empty(),
                "case {case:?}: expected no schedule at all, got {:?}",
                reported(&case, &body)
            );
            assert_eq!(
                token(&body),
                None,
                "case {case:?}: nothing follows an empty set, so no token is due, got {body}"
            );
        }
    }
    assert_eq!(
        stored_capacity(&engine),
        baseline,
        "a reporting operation writes nothing: expected the capacity store to end where it \
         started at {baseline:?}, got {:?}",
        stored_capacity(&engine)
    );
}

/// Every block the signed Region has set aside is reported, in the store's own order, with the
/// members read off that block — and a block of ANOTHER Region is not, so the answer is
/// computed from the stored inventory rather than fixed.
#[tokio::test]
async fn every_block_of_the_signed_region_is_reported_with_its_stored_members() {
    let (router, engine) = emulator();
    let keys = seed(
        &engine,
        &[
            ("us-west-2a", LINUX, C4_LARGE, 12, "0.080"),
            ("us-west-2a", WINDOWS, M4_XLARGE, 3, "0.512"),
            ("us-west-2b", LINUX, M4_XLARGE, 7, "0.256"),
            ("us-east-1a", LINUX, C4_LARGE, 20, "0.075"),
        ],
    );
    assert_eq!(
        keys.len(),
        4,
        "the seeded blocks must each be stored under a key of their own, got {keys:?}"
    );

    let params = search(Vec::new());
    let body = served(&router, "the Region's blocks", REGION, &params).await;
    let schedules = reported("the Region's blocks", &body);
    assert_eq!(
        reported_blocks("the Region's blocks", &body),
        vec![
            "us-west-2a Linux/UNIX c4.large".to_string(),
            "us-west-2a Windows m4.xlarge".to_string(),
            "us-west-2b Linux/UNIX m4.xlarge".to_string(),
        ],
        "expected the signed Region's three blocks in the store's key order and nothing of \
         another Region, got {body}"
    );
    assert_eq!(
        token(&body),
        None,
        "three blocks fit one page, so no token is due, got {body}"
    );

    // The members a block holds come off the block, and the term members off what AWS
    // publishes for a Scheduled Instance.
    let first = schedules
        .first()
        .expect("the Region's first block is reported");
    assert_eq!(
        (
            first.availability_zone.as_str(),
            first.platform.as_str(),
            first.instance_type.as_str(),
            first.network_platform.as_str(),
            first.available_instance_count.as_str(),
            first.hourly_price.as_str(),
        ),
        (
            "us-west-2a",
            LINUX,
            C4_LARGE,
            NETWORK_PLATFORM,
            "12",
            "0.080"
        ),
        "expected the stored block's own members, got {first:?}"
    );
    for schedule in &schedules {
        assert_eq!(
            (
                schedule.min_term_duration_in_days.as_str(),
                schedule.max_term_duration_in_days.as_str()
            ),
            (TERM_DURATION_IN_DAYS, TERM_DURATION_IN_DAYS),
            "every schedule is bought for the one published term, got {schedule:?}"
        );
        assert!(
            schedule.purchase_token.starts_with("sipt-"),
            "every schedule carries a purchase token this service minted, got {schedule:?}"
        );
    }
    // A second block carries ITS own members, so no member is copied from the first.
    let second = schedules.get(1).expect("the Region's second block");
    assert_eq!(
        (
            second.platform.as_str(),
            second.instance_type.as_str(),
            second.available_instance_count.as_str(),
            second.hourly_price.as_str()
        ),
        (WINDOWS, M4_XLARGE, "3", "0.512"),
        "expected the second block's own members, got {second:?}"
    );
    assert_ne!(
        first.purchase_token, second.purchase_token,
        "two blocks are purchased with tokens of their own, got {:?} twice",
        first.purchase_token
    );

    // The same call signed for the other Region sees that Region's block and only it.
    let body = served(&router, "the other Region", "us-east-1", &params).await;
    assert_eq!(
        reported_blocks("the other Region", &body),
        vec!["us-east-1a Linux/UNIX c4.large".to_string()],
        "expected only the block set aside in the Region the call was signed for, got {body}"
    );
}

/// A block with no instance left is no availability, and neither is a window that has already
/// passed: both are an empty report rather than a refusal, and neither hides the blocks that
/// ARE available.
#[tokio::test]
async fn a_spent_block_or_a_window_that_has_passed_reports_nothing() {
    let (router, engine) = emulator();
    seed(
        &engine,
        &[
            ("us-west-2a", LINUX, C4_LARGE, 0, "0.080"),
            ("us-west-2b", LINUX, C4_LARGE, 1, "0.080"),
        ],
    );

    let body = served(&router, "a spent block", REGION, &search(Vec::new())).await;
    assert_eq!(
        reported_blocks("a spent block", &body),
        vec!["us-west-2b Linux/UNIX c4.large".to_string()],
        "a block with no instance left is not reported as a schedule, got {body}"
    );

    // A window whose latest start has passed can hold no first slot, whatever is set aside.
    let mut passed = window(-2 * 60 * 60, 60 * 60);
    passed.extend(daily());
    let body = served(&router, "a window that has passed", REGION, &passed).await;
    assert!(
        reported("a window that has passed", &body).is_empty(),
        "a window that closed before now holds no first slot, got {body}"
    );
    assert_eq!(
        member(&body, AVAILABILITY_SET).as_deref(),
        Some(""),
        "that is an empty report, not an absent member, got {body}"
    );

    // A window that OPENED before now but is still open is still searchable: the first slot
    // starts now rather than in the past.
    let mut opening = window(-60 * 60, 2 * 60 * 60);
    opening.extend(daily());
    let body = served(&router, "a window still open", REGION, &opening).await;
    let schedules = reported("a window still open", &body);
    let first_slot = schedules
        .first()
        .map(|schedule| schedule.first_slot_start_time.clone())
        .unwrap_or_default();
    let started_at = super::ec2_query::as_timestamp(&first_slot).unwrap_or_default();
    assert!(
        started_at >= (now_secs() - 60) as f64,
        "case {first_slot:?}: a first slot cannot be scheduled into the past, got \
         {started_at} against a clock at {}",
        now_secs()
    );
}

// ---------------------------------------------------------------------------
// Rule 2: what the reported schedule says
// ---------------------------------------------------------------------------

/// The slot a schedule is offered with is the SHORTEST whole number of hours that still adds
/// up to 1,200 hours over the term for the recurrence asked for, and the term's hours are that
/// slot times the number of times the schedule comes round.
///
/// The table spans the dimensions the rule generalises over: each frequency, an interval of one
/// and of several, and one named occurrence day and several. The operation's own examples —
/// four hours for a daily schedule, 24 for a weekly one, 100 for a monthly one — are three of
/// its rows, and the rest are the same arithmetic where AWS publishes no example.
#[tokio::test]
async fn the_slot_is_the_shortest_that_meets_the_minimum_hours_of_the_term() {
    let (router, engine) = emulator();
    seed(&engine, &[("us-west-2a", LINUX, C4_LARGE, 4, "0.080")]);

    // (case, recurrence fields, the slots a term holds, the slot's hours)
    let rows: Vec<(&str, Params, i64, i64)> = vec![
        ("every day", recurrence("Daily", None, &[], None), 365, 4),
        (
            "every second day",
            recurrence("Daily", Some(2), &[], None),
            182,
            7,
        ),
        (
            "every week, day left to AWS",
            recurrence("Weekly", Some(1), &[], None),
            52,
            24,
        ),
        (
            "every week on two days",
            recurrence("Weekly", Some(1), &[2, 4], None),
            104,
            12,
        ),
        (
            "every second week",
            recurrence("Weekly", Some(2), &[], None),
            26,
            47,
        ),
        (
            "every month on one day",
            recurrence("Monthly", Some(1), &[1], Some("DayOfMonth")),
            12,
            100,
        ),
        (
            "every month on two days",
            recurrence("Monthly", Some(1), &[1, 15], Some("DayOfMonth")),
            24,
            50,
        ),
        (
            "every third month",
            recurrence("Monthly", Some(3), &[1], Some("DayOfMonth")),
            4,
            300,
        ),
    ];

    for (case, fields, slots_per_term, slot_hours) in rows {
        let mut params = open_window();
        params.extend(fields);
        let body = served(&router, case, REGION, &params).await;
        let schedules = reported(case, &body);
        let schedule = schedules
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the one block must be reported: {body}"));
        assert_eq!(
            schedule.slot_duration_in_hours,
            slot_hours.to_string(),
            "case {case:?}: expected a slot of {slot_hours} hours, got {schedule:?}"
        );
        let total: i64 = schedule
            .total_scheduled_instance_hours
            .parse()
            .unwrap_or_default();
        assert_eq!(
            total,
            slot_hours * slots_per_term,
            "case {case:?}: expected the term to hold {slots_per_term} slots of {slot_hours} \
             hours, got {schedule:?}"
        );
        assert!(
            total >= MIN_HOURS_PER_YEAR,
            "case {case:?}: a schedule on offer meets the {MIN_HOURS_PER_YEAR}-hour minimum, \
             got {total}"
        );
        assert!(
            (slot_hours - 1) * slots_per_term < MIN_HOURS_PER_YEAR,
            "case {case:?}: a slot of {} hours would have met the minimum too, so {slot_hours} \
             is not the shortest",
            slot_hours - 1
        );
    }
}

/// The recurrence a schedule is reported with is the one the search asked for, with the
/// interval it resolved to — a request that names no interval asks for every period, which is
/// an interval of one — and no occurrence day invented for a schedule that named none.
#[tokio::test]
async fn the_reported_recurrence_is_the_one_the_search_asked_for() {
    let (router, engine) = emulator();
    seed(&engine, &[("us-west-2a", LINUX, C4_LARGE, 1, "0.080")]);

    // (case, recurrence fields, expected frequency, interval, days, unit, relative to end)
    type Row<'a> = (
        &'a str,
        Params,
        &'a str,
        &'a str,
        Vec<String>,
        Option<String>,
        &'a str,
    );
    let mut rows: Vec<Row<'_>> = vec![
        (
            "a daily schedule naming no interval",
            recurrence("Daily", None, &[], None),
            "Daily",
            "1",
            Vec::new(),
            None,
            "false",
        ),
        (
            "a weekly schedule on two days",
            recurrence("Weekly", Some(2), &[1, 7], None),
            "Weekly",
            "2",
            vec!["1".to_string(), "7".to_string()],
            None,
            "false",
        ),
        (
            "a monthly schedule counted in days of the month",
            recurrence("Monthly", Some(1), &[31], Some("DayOfMonth")),
            "Monthly",
            "1",
            vec!["31".to_string()],
            Some("DayOfMonth".to_string()),
            "false",
        ),
    ];
    // The same monthly schedule, relative to the end of the month: one day only, which is the
    // rule that member carries.
    let mut relative = recurrence("Monthly", None, &[2], Some("DayOfMonth"));
    relative.push(p("Recurrence.OccurrenceRelativeToEnd", "true"));
    rows.push((
        "a monthly schedule relative to the end of the month",
        relative,
        "Monthly",
        "1",
        vec!["2".to_string()],
        Some("DayOfMonth".to_string()),
        "true",
    ));

    for (case, fields, frequency, interval, days, unit, relative_to_end) in rows {
        let mut params = open_window();
        params.extend(fields);
        let body = served(&router, case, REGION, &params).await;
        let schedules = reported(case, &body);
        let schedule = schedules
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the one block must be reported: {body}"));
        assert_eq!(
            (
                schedule.frequency.as_str(),
                schedule.interval.as_str(),
                schedule.occurrence_days.clone(),
                schedule.occurrence_unit.clone(),
                schedule.occurrence_relative_to_end.as_str()
            ),
            (frequency, interval, days, unit, relative_to_end),
            "case {case:?}: expected the recurrence the search asked for, got {schedule:?}"
        );
    }
}

/// The caller's own slot bounds move the schedule: a minimum longer than the term's rule needs
/// lengthens the slot (and the term's hours with it), while a maximum shorter than that rule
/// needs leaves no schedule to report at all.
#[tokio::test]
async fn the_requested_slot_bounds_lengthen_or_rule_out_the_schedule() {
    let (router, engine) = emulator();
    seed(&engine, &[("us-west-2a", LINUX, C4_LARGE, 2, "0.080")]);

    // (case, slot bound fields, the slot expected — `None` where nothing is on offer)
    let rows: Vec<(&str, Params, Option<i64>)> = vec![
        ("no bound at all", Vec::new(), Some(4)),
        (
            "a minimum above what the term's rule needs",
            vec![p("MinSlotDurationInHours", "8")],
            Some(8),
        ),
        (
            "a minimum below it",
            vec![p("MinSlotDurationInHours", "2")],
            Some(4),
        ),
        (
            "a maximum above it",
            vec![p("MaxSlotDurationInHours", "5")],
            Some(4),
        ),
        (
            "a maximum below it",
            vec![p("MaxSlotDurationInHours", "3")],
            None,
        ),
        (
            "both bounds, far above it",
            vec![
                p("MinSlotDurationInHours", "240"),
                p("MaxSlotDurationInHours", "480"),
            ],
            Some(240),
        ),
        (
            "a minimum one hour under the published limit",
            vec![p("MinSlotDurationInHours", (SLOT_DURATION_LIMIT_HOURS - 1).to_string())],
            Some(SLOT_DURATION_LIMIT_HOURS - 1),
        ),
        (
            "a minimum at the published limit, which no slot may reach",
            vec![p("MinSlotDurationInHours", SLOT_DURATION_LIMIT_HOURS.to_string())],
            None,
        ),
    ];

    for (case, bounds, expected) in rows {
        let body = served(&router, case, REGION, &search(bounds)).await;
        let schedules = reported(case, &body);
        match expected {
            None => assert!(
                schedules.is_empty(),
                "case {case:?}: no slot fits those bounds, so nothing is on offer, got {body}"
            ),
            Some(slot) => {
                let schedule = schedules.first().unwrap_or_else(|| {
                    panic!("case {case:?}: the one block must be reported: {body}")
                });
                assert_eq!(
                    schedule.slot_duration_in_hours,
                    slot.to_string(),
                    "case {case:?}: expected a slot of {slot} hours, got {schedule:?}"
                );
                let total: i64 = schedule
                    .total_scheduled_instance_hours
                    .parse()
                    .unwrap_or_default();
                assert_eq!(
                    total,
                    slot * 365,
                    "case {case:?}: a daily schedule's term holds one slot per day, got \
                     {schedule:?}"
                );
            }
        }
    }
}

/// Each documented filter narrows the report by one field of the stored block, several values
/// of one filter are joined with OR and several filters with AND — and a value nothing holds
/// selects nothing rather than everything.
#[tokio::test]
async fn the_documented_filters_narrow_the_report() {
    let (router, engine) = emulator();
    seed(
        &engine,
        &[
            ("us-west-2a", LINUX, C4_LARGE, 1, "0.080"),
            ("us-west-2a", WINDOWS, C4_LARGE, 1, "0.130"),
            ("us-west-2b", LINUX, M4_XLARGE, 1, "0.256"),
            ("us-west-2c", WINDOWS, M4_XLARGE, 1, "0.400"),
        ],
    );

    // (case, filter entries, the blocks expected)
    let rows: Vec<(&str, Params, Vec<&str>)> = vec![
        (
            "one zone",
            filters(&[("availability-zone", &["us-west-2a"])]),
            vec![
                "us-west-2a Linux/UNIX c4.large",
                "us-west-2a Windows c4.large",
            ],
        ),
        (
            "one instance type",
            filters(&[("instance-type", &[M4_XLARGE])]),
            vec![
                "us-west-2b Linux/UNIX m4.xlarge",
                "us-west-2c Windows m4.xlarge",
            ],
        ),
        (
            "one platform",
            filters(&[("platform", &[WINDOWS])]),
            vec![
                "us-west-2a Windows c4.large",
                "us-west-2c Windows m4.xlarge",
            ],
        ),
        (
            "two values of one filter, joined with OR",
            filters(&[("availability-zone", &["us-west-2b", "us-west-2c"])]),
            vec![
                "us-west-2b Linux/UNIX m4.xlarge",
                "us-west-2c Windows m4.xlarge",
            ],
        ),
        (
            "two filters, joined with AND",
            filters(&[("platform", &[LINUX]), ("instance-type", &[C4_LARGE])]),
            vec!["us-west-2a Linux/UNIX c4.large"],
        ),
        (
            "a wildcard zone",
            filters(&[("availability-zone", &["us-west-2?"])]),
            vec![
                "us-west-2a Linux/UNIX c4.large",
                "us-west-2a Windows c4.large",
                "us-west-2b Linux/UNIX m4.xlarge",
                "us-west-2c Windows m4.xlarge",
            ],
        ),
        (
            "a zone no block of this Region sits in",
            filters(&[("availability-zone", &["us-west-2f"])]),
            Vec::new(),
        ),
        (
            "an instance type nothing is set aside for",
            filters(&[("instance-type", &["r3.8xlarge"])]),
            Vec::new(),
        ),
        (
            "a filter carrying no value at all",
            filters(&[("platform", &[])]),
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in rows {
        let body = served(&router, case, REGION, &search(entries)).await;
        assert_eq!(
            reported_blocks(case, &body),
            expected
                .iter()
                .map(|block| (*block).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly the blocks the filters leave, got {body}"
        );
    }
}

/// The report pages by `MaxResults` in the store's key order: each page carries at most that
/// many schedules, a token comes back only while another block follows, following it resumes at
/// that block, and the pages together are the whole report with nothing repeated or dropped.
#[tokio::test]
async fn the_report_pages_by_max_results_and_its_token() {
    let (router, engine) = emulator();
    let mut blocks: Vec<Block<'_>> = Vec::new();
    for zone in ["us-west-2a", "us-west-2b"] {
        for platform in [LINUX, WINDOWS] {
            for instance_type in [C4_LARGE, M4_XLARGE] {
                blocks.push((zone, platform, instance_type, 2, "0.100"));
            }
        }
    }
    seed(&engine, &blocks);

    // ONE search, built once and reused for every page: a `FirstSlotStartTimeRange`
    // derived per call moves with the clock, and the token names a position inside the
    // search that minted it — so re-deriving the window between two pages of one walk
    // would present the token back against a different search.
    let searched = search(Vec::new());
    let whole = reported_blocks(
        "the whole report",
        &served(&router, "the whole report", REGION, &searched).await,
    );
    assert_eq!(
        whole.len(),
        8,
        "every seeded block of the Region is reported in one page, got {whole:?}"
    );

    for page_size in [MIN_MAX_RESULTS, 6] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = searched.clone();
            params.push(p("MaxResults", page_size.to_string()));
            if let Some(token) = &next {
                params.push(p("NextToken", token.clone()));
            }
            let body = served(&router, &case, REGION, &params).await;
            let page = reported_blocks(&case, &body);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: a page carries at most {page_size} schedules, got {page:?}"
            );
            walked.extend(page);
            next = token(&body);
            pages += 1;
            assert!(
                pages <= 8,
                "case {case:?}: the walk must end, got {walked:?} after {pages} pages"
            );
            if next.is_none() {
                break;
            }
        }
        assert!(
            pages > 1,
            "case {case:?}: a report of {} blocks does not fit one page of {page_size}",
            whole.len()
        );
        assert_eq!(
            walked, whole,
            "case {case:?}: the pages together are the whole report, in the same order"
        );
    }

    // A token names a position inside the search it was minted for, so presenting it back
    // against a DIFFERENT search is refused rather than resumed inside a set it never
    // described.
    let body = served(
        &router,
        "a token to carry over",
        REGION,
        &search(vec![p("MaxResults", MIN_MAX_RESULTS.to_string())]),
    )
    .await;
    let carried = token(&body).expect("a report of eight blocks leaves a token after five");
    let mut other = open_window();
    other.extend(recurrence("Weekly", Some(1), &[3], None));
    other.push(p("NextToken", carried));
    let (status, body) = call(&router, REGION, ACTION, &other).await;
    assert_eq!(
        status, 400,
        "a token minted for another search is refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "expected the pagination refusal, got {body}"
    );
    assert_eq!(
        error_message(&body),
        ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        "expected EC2's own pagination-token sentence, got {body}"
    );
}

/// Reporting is a READ: the same search answers the same schedules twice, and the capacity the
/// store holds is untouched — no instance count drawn down by having been looked at.
#[tokio::test]
async fn reading_the_report_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    seed(
        &engine,
        &[
            ("us-west-2a", LINUX, C4_LARGE, 5, "0.080"),
            ("us-west-2b", WINDOWS, M4_XLARGE, 1, "0.512"),
        ],
    );
    let baseline = stored_capacity(&engine);

    let params = search(Vec::new());
    // Everything but the purchase token, which is a per-call credential carrying its own
    // expiry: the schedules themselves must repeat.
    let described = |case: &'static str, body: &str| -> Vec<Reported> {
        reported(case, body)
            .into_iter()
            .map(|mut schedule| {
                assert!(
                    !schedule.purchase_token.is_empty(),
                    "case {case:?}: every reported schedule carries a purchase token"
                );
                schedule.purchase_token = String::new();
                schedule
            })
            .collect()
    };

    let first = described(
        "first read",
        &served(&router, "first read", REGION, &params).await,
    );
    let second = described(
        "second read",
        &served(&router, "second read", REGION, &params).await,
    );
    assert_eq!(
        first, second,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert_eq!(
        stored_capacity(&engine),
        baseline,
        "the capacity a search reported is still all there, expected {baseline:?}, got {:?}",
        stored_capacity(&engine)
    );
}

// ---------------------------------------------------------------------------
// Rule 3: criteria that select nothing, and requests that are refused
// ---------------------------------------------------------------------------

/// A search whose CRITERIA nothing can satisfy is served and answered with nothing: the window
/// and the slot bounds say which schedules are on offer, so a search that finds none is an
/// empty report — never a refusal, which would say the request itself was wrong.
///
/// The rows span both ends of each criterion: a window that closed, one whose latest start
/// precedes its earliest, one that opens beyond the three months a search can see, and slot
/// bounds that cross or that leave no room for a slot meeting the term's minimum hours.
#[tokio::test]
async fn criteria_nothing_satisfies_are_served_as_an_empty_report() {
    let (router, engine) = emulator();
    seed(&engine, &[("us-west-2a", LINUX, C4_LARGE, 3, "0.080")]);
    let baseline = stored_capacity(&engine);
    let day = 24 * 60 * 60;

    let rows: Vec<(&str, Params)> = vec![
        ("a window that closed an hour ago", {
            let mut params = window(-2 * 60 * 60, 60 * 60);
            params.extend(daily());
            params
        }),
        ("a window whose latest start precedes its earliest", {
            let mut params = window(2 * 60 * 60, -(60 * 60));
            params.extend(daily());
            params
        }),
        ("a window that opens in four months", {
            let mut params = window(120 * day, 60 * 60);
            params.extend(daily());
            params
        }),
        (
            "a maximum slot under what a daily term needs",
            search(vec![p("MaxSlotDurationInHours", "3")]),
        ),
        (
            "a minimum slot at the published limit, which no slot may reach",
            search(vec![p(
                "MinSlotDurationInHours",
                SLOT_DURATION_LIMIT_HOURS.to_string(),
            )]),
        ),
        (
            "an interval longer than the term holds",
            {
                let mut params = open_window();
                params.extend(recurrence("Monthly", Some(13), &[1], Some("DayOfMonth")));
                params
            },
        ),
    ];

    for (case, params) in rows {
        let body = served(&router, case, REGION, &params).await;
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: nothing satisfies those criteria, so nothing is reported, got {body}"
        );
        assert_eq!(
            member(&body, AVAILABILITY_SET).as_deref(),
            Some(""),
            "case {case:?}: that is a served report with an empty set, got {body}"
        );
        assert_eq!(
            token(&body),
            None,
            "case {case:?}: nothing follows an empty set, so no token is due, got {body}"
        );
    }

    // A window WIDER than the shape the model describes still selects the schedules it holds:
    // a wider window only offers more instants for the first slot to start at.
    for (case, span) in [
        ("a window spanning exactly one day", day),
        ("a window spanning a week", 7 * day),
    ] {
        let mut params = window(60 * 60, span);
        params.extend(daily());
        let body = served(&router, case, REGION, &params).await;
        assert_eq!(
            reported_blocks(case, &body),
            vec!["us-west-2a Linux/UNIX c4.large".to_string()],
            "case {case:?}: a wider window still finds the Region's capacity, got {body}"
        );
    }

    // A maximum one hour under the published limit is a bound a slot can sit under, so the
    // schedule the term's rule fixes is still reported: the pair SELECTS slots, and only the
    // values the model declares invalid are refused (see the refusal rows).
    let case = "a maximum slot one hour under the published limit";
    let body = served(
        &router,
        case,
        REGION,
        &search(vec![p(
            "MaxSlotDurationInHours",
            (SLOT_DURATION_LIMIT_HOURS - 1).to_string(),
        )]),
    )
    .await;
    let schedules = reported(case, &body);
    let schedule = schedules
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the one block must be reported: {body}"));
    assert_eq!(
        schedule.slot_duration_in_hours, "4",
        "case {case:?}: the slot is the one a daily term needs, got {schedule:?}"
    );

    assert_eq!(
        stored_capacity(&engine),
        baseline,
        "none of those searches wrote anything, expected {baseline:?}, got {:?}",
        stored_capacity(&engine)
    );
}

/// A member the frequency gives no meaning to is not part of the schedule: a daily schedule
/// comes round every day, so an occurrence day or an occurrence unit carried beside one narrows
/// nothing and is left out of the reported recurrence rather than turned into a refusal.
#[tokio::test]
async fn a_member_the_frequency_gives_no_meaning_to_is_left_out_of_the_schedule() {
    let (router, engine) = emulator();
    seed(&engine, &[("us-west-2a", LINUX, C4_LARGE, 1, "0.080")]);

    // (case, recurrence fields, the days and unit expected back)
    let rows: Vec<(&str, Params, Vec<String>, Option<String>)> = vec![
        (
            "occurrence days on a daily schedule",
            recurrence("Daily", None, &[1, 2], None),
            Vec::new(),
            None,
        ),
        (
            "an occurrence unit on a daily schedule",
            recurrence("Daily", None, &[], Some("DayOfMonth")),
            Vec::new(),
            None,
        ),
        (
            "both on a daily schedule",
            recurrence("Daily", Some(1), &[3], Some("DayOfWeek")),
            Vec::new(),
            None,
        ),
        (
            "days of the week named as such on a weekly schedule",
            recurrence("Weekly", None, &[2], Some("DayOfWeek")),
            vec!["2".to_string()],
            Some("DayOfWeek".to_string()),
        ),
        (
            "a monthly schedule naming no unit for its days",
            recurrence("Monthly", None, &[1], None),
            vec!["1".to_string()],
            None,
        ),
    ];

    for (case, fields, days, unit) in rows {
        let mut params = open_window();
        params.extend(fields);
        let body = served(&router, case, REGION, &params).await;
        let schedules = reported(case, &body);
        let schedule = schedules
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the one block must be reported: {body}"));
        assert_eq!(
            (schedule.occurrence_days.clone(), schedule.occurrence_unit.clone()),
            (days, unit),
            "case {case:?}: expected only the days and unit the frequency gives meaning to, \
             got {schedule:?}"
        );
    }

    // A daily schedule that carried occurrence days is still a DAILY schedule: the days changed
    // neither how often it comes round nor the slot the term's minimum hours fixes.
    let body = served(
        &router,
        "a daily schedule that carried days",
        REGION,
        &{
            let mut params = open_window();
            params.extend(recurrence("Daily", None, &[1, 2, 3], None));
            params
        },
    )
    .await;
    let schedules = reported("a daily schedule that carried days", &body);
    let schedule = schedules.first().expect("the one block is reported");
    assert_eq!(
        (
            schedule.slot_duration_in_hours.as_str(),
            schedule.total_scheduled_instance_hours.as_str()
        ),
        ("4", "1460"),
        "a daily schedule holds one slot a day whatever days were named, got {schedule:?}"
    );
}

/// A request no schedule could be described by is REFUSED — never answered with an empty
/// report, which would say the search was served and nothing matched — and so are a filter this
/// operation does not document and a token it did not mint. `DryRun` on a search that would
/// have been served is answered as the probe it is.
///
/// The rows name the rule each one breaks; what they span is the three members a search is made
/// of and both halves of each published range.
#[tokio::test]
async fn a_request_no_schedule_could_be_described_by_is_refused() {
    let (router, engine) = emulator();
    seed(&engine, &[("us-west-2a", LINUX, C4_LARGE, 1, "0.080")]);
    let baseline = stored_capacity(&engine);

    // (case, params, status, error code, a fragment of the message)
    let mut rows: Vec<(String, Params, u16, &str, String)> = vec![
        (
            "a recurrence naming no frequency".to_string(),
            {
                let mut params = open_window();
                params.push(p("Recurrence.Interval", "1"));
                params
            },
            400,
            "MissingParameter",
            "Recurrence.Frequency".to_string(),
        ),
        (
            "a frequency AWS does not offer".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Hourly", None, &[], None));
                params
            },
            400,
            "InvalidParameterValue",
            "Daily, Weekly, Monthly".to_string(),
        ),
        (
            "an interval of no periods".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Daily", Some(0), &[], None));
                params
            },
            400,
            "InvalidParameterValue",
            "Recurrence.Interval".to_string(),
        ),
        (
            "a day of the week below the published range".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Weekly", None, &[0], None));
                params
            },
            400,
            "InvalidParameterValue",
            "days of the week (1-7".to_string(),
        ),
        (
            "a day of the week above it".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Weekly", None, &[8], None));
                params
            },
            400,
            "InvalidParameterValue",
            "days of the week (1-7".to_string(),
        ),
        (
            "a day of the month above the published range".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Monthly", None, &[32], Some("DayOfMonth")));
                params
            },
            400,
            "InvalidParameterValue",
            "days of the month (1-31)".to_string(),
        ),
        (
            "two days with an occurrence relative to the end".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Monthly", None, &[1, 2], Some("DayOfMonth")));
                params.push(p("Recurrence.OccurrenceRelativeToEnd", "true"));
                params
            },
            400,
            "InvalidParameterValue",
            "only a single day".to_string(),
        ),
        (
            "an occurrence unit AWS does not count days in".to_string(),
            {
                let mut params = open_window();
                params.extend(recurrence("Monthly", None, &[1], Some("DayOfYear")));
                params
            },
            400,
            "InvalidParameterValue",
            "DayOfWeek, DayOfMonth".to_string(),
        ),
        (
            "a minimum slot of no hours".to_string(),
            search(vec![p("MinSlotDurationInHours", "0")]),
            400,
            "InvalidParameterValue",
            "MinSlotDurationInHours".to_string(),
        ),
        (
            "a maximum slot of no hours".to_string(),
            search(vec![p("MaxSlotDurationInHours", "0")]),
            400,
            "InvalidParameterValue",
            "MaxSlotDurationInHours".to_string(),
        ),
        (
            "a maximum slot at the published limit".to_string(),
            search(vec![p(
                "MaxSlotDurationInHours",
                SLOT_DURATION_LIMIT_HOURS.to_string(),
            )]),
            400,
            "InvalidParameterValue",
            format!("less than {SLOT_DURATION_LIMIT_HOURS}"),
        ),
        (
            "a maximum slot above the published limit".to_string(),
            search(vec![p(
                "MaxSlotDurationInHours",
                (SLOT_DURATION_LIMIT_HOURS + 1).to_string(),
            )]),
            400,
            "InvalidParameterValue",
            format!("less than {SLOT_DURATION_LIMIT_HOURS}"),
        ),
        (
            "a maximum slot equal to the minimum asked for".to_string(),
            search(vec![
                p("MinSlotDurationInHours", "12"),
                p("MaxSlotDurationInHours", "12"),
            ]),
            400,
            "InvalidParameterValue",
            "greater than MinSlotDurationInHours".to_string(),
        ),
        (
            "a maximum slot shorter than the minimum asked for".to_string(),
            search(vec![
                p("MinSlotDurationInHours", "24"),
                p("MaxSlotDurationInHours", "12"),
            ]),
            400,
            "InvalidParameterValue",
            "greater than MinSlotDurationInHours".to_string(),
        ),
        (
            "a filter this operation does not document".to_string(),
            search(filters(&[("network-platform", &["EC2-VPC"])])),
            400,
            "InvalidParameterValue",
            "The filter 'network-platform' is invalid".to_string(),
        ),
        (
            "a continuation token from a page nobody was served".to_string(),
            search(vec![p("NextToken", "AAAAAAAAAAE=")]),
            400,
            "InvalidPaginationToken",
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
        (
            "the DryRun probe on a search that would have been served".to_string(),
            search(vec![p("DryRun", "true")]),
            412,
            "DryRunOperation",
            "DryRun flag is set".to_string(),
        ),
    ];
    // A page size below the modelled `@range` is refused by the BOUNDARY validator rather than
    // by this handler, with EC2's own code for a page size it cannot serve — which is what
    // tells the rows above (the rules the handler enforces) from the modelled range.
    rows.push((
        "a page size below the modelled minimum".to_string(),
        search(vec![p("MaxResults", (MIN_MAX_RESULTS - 1).to_string())]),
        400,
        "InvalidMaxResults",
        "greater than or equal to 5".to_string(),
    ));

    for (case, params, status, code, fragment) in rows {
        let (answered, body) = call(&router, REGION, ACTION, &params).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code}, got {body}"
        );
        assert!(
            error_message(&body).contains(&fragment),
            "case {case:?}: expected a message naming {fragment:?}, got {:?}",
            error_message(&body)
        );
        assert!(
            member(&body, AVAILABILITY_SET).is_none(),
            "case {case:?}: a refusal reports no availability set at all, got {body}"
        );
    }

    assert_eq!(
        stored_capacity(&engine),
        baseline,
        "a refused search writes nothing either, expected {baseline:?}, got {:?}",
        stored_capacity(&engine)
    );
}
