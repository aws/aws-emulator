// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeScheduledInstances`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: A REPORT CARRIES ONE ENTRY PER SCHEDULED INSTANCE THE SIGNED REGION
//! HOLDS, each read off the stored purchase with the schedule the clock has reached derived
//! from its recurrence, and nothing else. Three parts to it, and the tables below span each:
//!
//! 1. WHICH purchases are reported: the stored Scheduled Instances of the Region the call was
//!    signed for, narrowed by the ids the request names, by the three filters the operation
//!    documents and by the slot-start window — paged by `MaxResults`/`NextToken` in the one
//!    order the store is keyed in. An account that has bought none is answered HTTP 200 with an
//!    empty `scheduledInstanceSet` and no token, which is a report of an empty inventory rather
//!    than a refusal, and is what a fresh account gets.
//! 2. WHAT each reported purchase says: the zone, instance type, platform, network platform,
//!    instance count, hourly price, slot and recurrence off the stored purchase; the term AWS
//!    sells, 365 days from its start; and the two timeline members — the next slot of the
//!    schedule that has not started, and the end of the latest one that has — derived from that
//!    recurrence at the instant of the read.
//! 3. WHAT selects nothing and what is refused: a filter value nothing holds, or a window no
//!    schedule starts a slot in, are SERVED with an empty report, while a request this account
//!    cannot answer for (an id of no such purchase, a value that is no Scheduled Instance id at
//!    all, a filter the operation does not document, a page size below the published minimum, a
//!    token it did not mint) is refused. `DryRun` is answered as the probe it is.
//!
//! Reporting is a READ throughout: every suite compares the store before and after, and reads
//! twice where the answer could have moved.

use ::aws_smithy_types::DateTime;
use ::aws_smithy_types::date_time::Format;
use ::chrono::Datelike;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::ops::types::ScheduledInstanceRecurrence;
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ScheduledInstanceSpec};

use super::ec2_query::{call, emulator, error_code, error_message, filters, member};
use super::wire::{child, child_text, document, set_items};

/// The operation every row calls.
const ACTION: &str = "DescribeScheduledInstances";

/// The envelope member the purchases are reported in.
const INSTANCE_SET: &str = "scheduledInstanceSet";

/// The Region most rows are signed for. A second Region is used wherever the rule is about
/// which Region's purchases a report sees.
const REGION: &str = "us-west-2";
const OTHER_REGION: &str = "us-east-1";

/// The platforms a Scheduled Instance runs — "The platform (`Linux/UNIX` or `Windows`)"
/// (`ScheduledInstance$Platform`), which is also what the `platform` filter selects on.
const LINUX: &str = "Linux/UNIX";
const WINDOWS: &str = "Windows";

/// The network platform a purchase in a Region that has only VPCs carries.
const NETWORK_PLATFORM: &str = "EC2-VPC";

/// The instance types the rows buy: "You can specify one of the C3, C4, M4, or R3 instance
/// types" (`ScheduledInstanceAvailability$InstanceType`). Two of that catalog, so a row can tell
/// one purchase from another by type.
const C4_LARGE: &str = "c4.large";
const M4_XLARGE: &str = "m4.xlarge";

/// The term a Scheduled Instance is bought for, in days: "The minimum term. The only possible
/// value is 365 days" (`ScheduledInstanceAvailability$MinTermDurationInDays`). Written out here
/// rather than read from the service, so this suite pins the value.
const TERM_DURATION_IN_DAYS: i64 = 365;

/// The published bounds of this operation's page size: "This value can be between 5 and 300.
/// The default value is 100" (`DescribeScheduledInstancesRequest$MaxResults`) — source:
/// `smithy_query(query="doc",shape="DescribeScheduledInstancesRequest",member="MaxResults")`.
const MIN_MAX_RESULTS: i64 = 5;
const MAX_MAX_RESULTS: i64 = 300;

const SECONDS_PER_HOUR: i64 = 60 * 60;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;

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

/// An ISO 8601 member as the instant it names, or `None` when it is absent or unreadable.
fn instant(value: Option<&String>) -> Option<i64> {
    let value = value.filter(|text| !text.is_empty())?;
    DateTime::from_str(value, Format::DateTime)
        .ok()
        .map(|at| at.secs())
}

/// The `SlotStartTimeRange` fields for a window that opens `opens_in` seconds from now and runs
/// for `span` seconds.
fn window(opens_in: i64, span: i64) -> Params {
    let opens_at = now_secs() + opens_in;
    vec![
        p("SlotStartTimeRange.EarliestTime", iso8601(opens_at)),
        p("SlotStartTimeRange.LatestTime", iso8601(opens_at + span)),
    ]
}

/// The recurrence a purchase repeats on, as the search that offered it reported it.
fn recurrence(
    frequency: &str,
    interval: Option<i32>,
    days: &[i32],
    relative_to_end: bool,
) -> ScheduledInstanceRecurrence {
    ScheduledInstanceRecurrence {
        frequency: Some(frequency.to_string()),
        interval,
        occurrence_day_set: (!days.is_empty()).then(|| days.to_vec()),
        occurrence_relative_to_end: Some(relative_to_end),
        occurrence_unit: (!days.is_empty()).then(|| {
            if frequency == "Weekly" {
                "DayOfWeek".to_string()
            } else {
                "DayOfMonth".to_string()
            }
        }),
    }
}

/// One purchase to store: a daily schedule whose term started two days ago, on the capacity
/// `zone`/`platform`/`instance_type` names. A row that is about the schedule itself replaces the
/// recurrence or the term's start.
fn purchase(zone: &str, platform: &str, instance_type: &str) -> ScheduledInstanceSpec {
    ScheduledInstanceSpec {
        availability_zone: zone.to_string(),
        instance_type: instance_type.to_string(),
        platform: platform.to_string(),
        network_platform: NETWORK_PLATFORM.to_string(),
        instance_count: 1,
        hourly_price: "0.095".to_string(),
        slot_duration_in_hours: 4,
        recurrence: recurrence("Daily", None, &[], false),
        term_start_secs: now_secs() - 2 * SECONDS_PER_DAY,
        purchased_at_secs: now_secs() - 3 * SECONDS_PER_DAY,
    }
}

/// Store `specs`, answering the `ScheduledInstanceId`s they were minted — the order a page of
/// them is reported in.
///
/// `PurchaseScheduledInstances` is the operation that buys one; it is still a scaffold under its
/// own issue, so a suite that needs an inventory writes one through the store's own writer, the
/// way the availability suite sets aside the capacity no EC2 call sets aside.
fn seed(engine: &engine::InMemoryEngine, specs: Vec<ScheduledInstanceSpec>) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    specs
        .into_iter()
        .map(|spec| {
            guard
                .insert_scheduled_instance(spec)
                .expect("the store holds far fewer purchases than its ceiling")
        })
        .collect()
}

/// The purchases the store holds, as `(id, the terms they were bought on)` pairs: compared before
/// and after a read, so a report that wrote a purchase of its own or moved a stored one is
/// visible.
fn stored(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .scheduled_instances
        .iter()
        .map(|(id, record)| {
            (
                id.clone(),
                format!(
                    "{zone} {platform} {instance_type} {count} {price} {slot}h \
                     {term_start}..{term_end} {hours}h {frequency:?}",
                    zone = record.spec.availability_zone,
                    platform = record.spec.platform,
                    instance_type = record.spec.instance_type,
                    count = record.spec.instance_count,
                    price = record.spec.hourly_price,
                    slot = record.spec.slot_duration_in_hours,
                    term_start = record.spec.term_start_secs,
                    term_end = record.term_end_secs,
                    hours = record.total_scheduled_instance_hours,
                    frequency = record.spec.recurrence.frequency,
                ),
            )
        })
        .collect()
}

/// One purchase as the response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    scheduled_instance_id: String,
    availability_zone: String,
    instance_type: String,
    platform: String,
    network_platform: String,
    instance_count: String,
    hourly_price: String,
    slot_duration_in_hours: String,
    total_scheduled_instance_hours: String,
    create_date: String,
    term_start_date: String,
    term_end_date: String,
    next_slot_start_time: Option<String>,
    previous_slot_end_time: Option<String>,
    frequency: String,
    interval: String,
    occurrence_days: Vec<String>,
    occurrence_relative_to_end: String,
    occurrence_unit: Option<String>,
}

/// Every purchase a successful response reports, in the order it reports them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let text = |item: roxmltree::Node<'_, '_>, name: &str| -> String {
        child_text(item, name).unwrap_or_default()
    };
    set_items(parsed.root_element(), INSTANCE_SET)
        .into_iter()
        .map(|item| {
            let recurrence = child(item, "recurrence");
            Reported {
                scheduled_instance_id: text(item, "scheduledInstanceId"),
                availability_zone: text(item, "availabilityZone"),
                instance_type: text(item, "instanceType"),
                platform: text(item, "platform"),
                network_platform: text(item, "networkPlatform"),
                instance_count: text(item, "instanceCount"),
                hourly_price: text(item, "hourlyPrice"),
                slot_duration_in_hours: text(item, "slotDurationInHours"),
                total_scheduled_instance_hours: text(item, "totalScheduledInstanceHours"),
                create_date: text(item, "createDate"),
                term_start_date: text(item, "termStartDate"),
                term_end_date: text(item, "termEndDate"),
                next_slot_start_time: child_text(item, "nextSlotStartTime"),
                previous_slot_end_time: child_text(item, "previousSlotEndTime"),
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

/// The ids a response reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|instance| instance.scheduled_instance_id)
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

/// The UTC day the instant `secs` falls on, for the rows that assert which day of a week or a
/// month a slot starts on.
fn utc_day(secs: i64) -> ::chrono::NaiveDate {
    ::chrono::DateTime::from_timestamp(secs, 0)
        .expect("an instant inside the representable range")
        .date_naive()
}

// ---------------------------------------------------------------------------
// Rule 1: which purchases a report is answered with
// ---------------------------------------------------------------------------

/// An account that has bought no Scheduled Instance is answered a REPORT of that — HTTP 200 with
/// the instance set present and empty, and no token — for every shape of well-formed request, in
/// every Region, and the store stays empty.
///
/// The rows differ along everything the request generalises over: each documented filter, a
/// filter with several values, the page size at both published bounds and between them, a window
/// behind and ahead of now, and the Region the call is signed for. No row is the specification;
/// what they share is that nothing has been bought to report.
#[tokio::test]
async fn an_account_that_bought_nothing_reports_an_empty_set_for_every_request() {
    let (router, engine) = emulator();

    let mut rows: Vec<(String, Params)> = vec![
        ("no parameter at all".to_string(), Vec::new()),
        (
            "the availability-zone filter".to_string(),
            filters(&[("availability-zone", &["us-west-2a"])]),
        ),
        (
            "the instance-type filter".to_string(),
            filters(&[("instance-type", &[C4_LARGE])]),
        ),
        (
            "the platform filter".to_string(),
            filters(&[("platform", &[LINUX])]),
        ),
        (
            "one filter with several values".to_string(),
            filters(&[("instance-type", &[C4_LARGE, M4_XLARGE])]),
        ),
        (
            "two filters at once".to_string(),
            filters(&[("platform", &[WINDOWS]), ("availability-zone", &["us-west-2b"])]),
        ),
        (
            "a window that opens in an hour".to_string(),
            window(SECONDS_PER_HOUR, SECONDS_PER_HOUR),
        ),
        (
            "a window that closed an hour ago".to_string(),
            window(-2 * SECONDS_PER_HOUR, SECONDS_PER_HOUR),
        ),
    ];
    for page_size in [MIN_MAX_RESULTS, 100, MAX_MAX_RESULTS] {
        rows.push((
            format!("a page size of {page_size}"),
            vec![p("MaxResults", page_size.to_string())],
        ));
    }

    for (case, params) in rows {
        for region in [REGION, OTHER_REGION] {
            let case = format!("{case} in {region}");
            let body = served(&router, &case, region, &params).await;
            assert_eq!(
                member(&body, INSTANCE_SET),
                Some(String::new()),
                "case {case:?}: expected the instance set present and empty, got {body}"
            );
            assert_eq!(
                token(&body),
                None,
                "case {case:?}: an empty report has no further page, got {body}"
            );
        }
    }
    assert!(
        stored(&engine).is_empty(),
        "a report writes nothing: expected the store to stay empty, got {:?}",
        stored(&engine)
    );
}

/// Every purchase the signed Region holds is reported, in the store's own order, with its members
/// read off the stored purchase — and a purchase of ANOTHER Region is not, so the answer is
/// computed from what the account bought rather than fixed.
#[tokio::test]
async fn every_purchase_of_the_signed_region_is_reported_with_its_stored_members() {
    let (router, engine) = emulator();
    let term_start = now_secs() + 6 * SECONDS_PER_HOUR;
    let purchased_at = now_secs() - SECONDS_PER_DAY;
    let specs = vec![
        {
            let mut spec = purchase("us-west-2a", LINUX, C4_LARGE);
            spec.instance_count = 3;
            spec.hourly_price = "0.080".to_string();
            spec.slot_duration_in_hours = 4;
            spec.term_start_secs = term_start;
            spec.purchased_at_secs = purchased_at;
            spec
        },
        {
            let mut spec = purchase("us-west-2b", WINDOWS, M4_XLARGE);
            spec.instance_count = 1;
            spec.hourly_price = "0.512".to_string();
            spec.recurrence = recurrence("Weekly", Some(1), &[3], false);
            spec.slot_duration_in_hours = 24;
            spec.term_start_secs = term_start;
            spec.purchased_at_secs = purchased_at;
            spec
        },
        purchase("us-east-1a", LINUX, C4_LARGE),
    ];
    let ids = seed(&engine, specs);
    let baseline = stored(&engine);

    let body = served(&router, "the signed Region", REGION, &Vec::new()).await;
    let reported = reported("the signed Region", &body);
    assert_eq!(
        reported_ids("the signed Region", &body),
        vec![ids[0].clone(), ids[1].clone()],
        "expected the two purchases of the signed Region in id order, got {body}"
    );

    let first = &reported[0];
    assert_eq!(
        (
            first.availability_zone.as_str(),
            first.instance_type.as_str(),
            first.platform.as_str(),
            first.network_platform.as_str(),
            first.instance_count.as_str(),
            first.hourly_price.as_str(),
            first.slot_duration_in_hours.as_str(),
        ),
        ("us-west-2a", C4_LARGE, LINUX, NETWORK_PLATFORM, "3", "0.080", "4"),
        "expected every member read off the stored purchase, got {first:?}"
    );
    // "The total number of hours for a single instance for the entire term": a daily schedule
    // comes round 365 times in a term, so a four-hour slot adds up to 1,460 hours.
    assert_eq!(
        first.total_scheduled_instance_hours, "1460",
        "expected the term's hours to follow from the slot and the schedule, got {first:?}"
    );
    assert_eq!(
        (
            first.create_date.as_str(),
            first.term_start_date.as_str(),
            first.term_end_date.as_str(),
        ),
        (
            iso8601(purchased_at).as_str(),
            iso8601(term_start).as_str(),
            iso8601(term_start + TERM_DURATION_IN_DAYS * SECONDS_PER_DAY).as_str(),
        ),
        "expected the purchase date and a term of {TERM_DURATION_IN_DAYS} days from its start, \
         got {first:?}"
    );
    assert_eq!(
        (
            first.frequency.as_str(),
            first.interval.as_str(),
            first.occurrence_days.clone(),
            first.occurrence_relative_to_end.as_str(),
            first.occurrence_unit.clone(),
        ),
        ("Daily", "1", Vec::new(), "false", None),
        "expected the recurrence the purchase was bought on, got {first:?}"
    );

    let second = &reported[1];
    assert_eq!(
        (
            second.frequency.as_str(),
            second.interval.as_str(),
            second.occurrence_days.clone(),
            second.occurrence_unit.clone(),
            second.slot_duration_in_hours.as_str(),
            // A weekly schedule on one day comes round 52 times in a term, so a 24-hour slot
            // adds up to 1,248 hours.
            second.total_scheduled_instance_hours.as_str(),
        ),
        (
            "Weekly",
            "1",
            vec!["3".to_string()],
            Some("DayOfWeek".to_string()),
            "24",
            "1248"
        ),
        "expected the weekly purchase reported on its own schedule, got {second:?}"
    );

    let other = served(&router, "the other Region", OTHER_REGION, &Vec::new()).await;
    assert_eq!(
        reported_ids("the other Region", &other),
        vec![ids[2].clone()],
        "expected only the purchase made in the Region the call was signed for, got {other}"
    );
    assert_eq!(
        stored(&engine),
        baseline,
        "a report writes nothing: expected the store to end where it started at {baseline:?}, \
         got {:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// Rule 2: what each reported purchase says about its schedule
// ---------------------------------------------------------------------------

/// The two timeline members are where the schedule has GOT TO: the next slot is the first one of
/// the term that has not started yet, at the time of day the term started at, and the previous
/// slot's end is the end of the latest one that has started — absent before the term's first slot
/// and, for the next slot, once the term holds no further one.
///
/// The rows span every dimension the schedule generalises over: each frequency, an interval of
/// one and of several, a named occurrence day and none, a day counted from the end of its period,
/// a term that has not started, and a term that has run out.
#[tokio::test]
async fn the_timeline_reported_is_the_slot_the_schedule_has_reached() {
    let (router, engine) = emulator();
    let now = now_secs();

    // (case, the term's start, the recurrence, the longest a caller may wait for the next slot)
    type Row<'a> = (&'a str, i64, ScheduledInstanceRecurrence, Option<i64>);
    let rows: Vec<Row<'_>> = vec![
        (
            "every day, a term running since yesterday",
            now - SECONDS_PER_DAY - SECONDS_PER_HOUR,
            recurrence("Daily", None, &[], false),
            Some(SECONDS_PER_DAY),
        ),
        (
            "every third day",
            now - SECONDS_PER_DAY - SECONDS_PER_HOUR,
            recurrence("Daily", Some(3), &[], false),
            Some(3 * SECONDS_PER_DAY),
        ),
        (
            "every week on the day the term started",
            now - 10 * SECONDS_PER_DAY,
            recurrence("Weekly", Some(1), &[], false),
            Some(7 * SECONDS_PER_DAY),
        ),
        (
            "every week on a named day",
            now - 10 * SECONDS_PER_DAY,
            recurrence("Weekly", Some(1), &[3], false),
            Some(7 * SECONDS_PER_DAY),
        ),
        (
            "every week on three named days",
            now - 10 * SECONDS_PER_DAY,
            recurrence("Weekly", Some(1), &[2, 4, 6], false),
            Some(7 * SECONDS_PER_DAY),
        ),
        (
            "every second week on a named day",
            now - 20 * SECONDS_PER_DAY,
            recurrence("Weekly", Some(2), &[5], false),
            Some(14 * SECONDS_PER_DAY),
        ),
        (
            "every month on the 15th",
            now - 70 * SECONDS_PER_DAY,
            recurrence("Monthly", Some(1), &[15], false),
            Some(31 * SECONDS_PER_DAY),
        ),
        (
            "every month on the last day",
            now - 70 * SECONDS_PER_DAY,
            recurrence("Monthly", Some(1), &[1], true),
            Some(31 * SECONDS_PER_DAY),
        ),
        (
            "every third month on the day the term started",
            now - 100 * SECONDS_PER_DAY,
            recurrence("Monthly", Some(3), &[], false),
            Some(3 * 31 * SECONDS_PER_DAY),
        ),
        (
            "a term that starts in six hours",
            now + 6 * SECONDS_PER_HOUR,
            recurrence("Daily", None, &[], false),
            Some(6 * SECONDS_PER_HOUR),
        ),
        (
            "a term that ran out a week ago",
            now - (TERM_DURATION_IN_DAYS + 7) * SECONDS_PER_DAY,
            recurrence("Daily", None, &[], false),
            None,
        ),
    ];

    // Every row's purchase is stored up front, so one walk of the store answers them all and
    // the snapshot below covers every read.
    let seeded: Vec<String> = rows
        .iter()
        .map(|(_, term_start, recurrence, _)| {
            let mut spec = purchase("us-west-2a", LINUX, C4_LARGE);
            spec.term_start_secs = *term_start;
            spec.recurrence = recurrence.clone();
            spec.slot_duration_in_hours = 6;
            seed(&engine, vec![spec])
                .pop()
                .expect("one purchase was stored")
        })
        .collect();
    let baseline = stored(&engine);

    for ((case, term_start, recurrence, within), id) in rows.into_iter().zip(seeded) {
        let body = served(
            &router,
            case,
            REGION,
            &vec![p("ScheduledInstanceId.1", id.clone())],
        )
        .await;
        let instance = reported(case, &body)
            .into_iter()
            .find(|instance| instance.scheduled_instance_id == id)
            .unwrap_or_else(|| panic!("case {case:?}: expected the purchase reported: {body}"));
        let next = instant(instance.next_slot_start_time.as_ref());
        let previous = instant(instance.previous_slot_end_time.as_ref());
        let term_end = term_start + TERM_DURATION_IN_DAYS * SECONDS_PER_DAY;

        match within {
            None => {
                assert_eq!(
                    next, None,
                    "case {case:?}: a term that has run out starts no further slot, got \
                     {instance:?}"
                );
                assert!(
                    previous.is_some_and(|previous| previous <= term_end + 6 * SECONDS_PER_HOUR),
                    "case {case:?}: expected the end of the term's last slot, got {instance:?}"
                );
            }
            Some(within) => {
                let next = next.unwrap_or_else(|| {
                    panic!("case {case:?}: expected a next slot start: {instance:?}")
                });
                assert!(
                    next > now && next <= now + within,
                    "case {case:?}: expected the next slot inside {within} seconds of {now}, got \
                     {next}"
                );
                assert!(
                    next >= term_start && next <= term_end,
                    "case {case:?}: expected the next slot inside the term \
                     {term_start}..{term_end}, got {next}"
                );
                // Every slot starts at the time of day the term started at.
                assert_eq!(
                    (next - term_start).rem_euclid(SECONDS_PER_DAY),
                    0,
                    "case {case:?}: expected a slot at the term's own time of day, got {next} \
                     against a term starting at {term_start}"
                );
                if let Some(days) = recurrence.occurrence_day_set.as_ref() {
                    let day = utc_day(next);
                    if recurrence.frequency.as_deref() == Some("Weekly") {
                        let weekday =
                            i32::try_from(day.weekday().num_days_from_sunday() + 1).unwrap_or(0);
                        assert!(
                            days.contains(&weekday),
                            "case {case:?}: expected a slot on one of the days {days:?} of the \
                             week, got day {weekday} ({day})"
                        );
                    } else if recurrence.occurrence_relative_to_end == Some(true) {
                        let tomorrow = day + ::chrono::Days::new(1);
                        assert_eq!(
                            tomorrow.month(),
                            (day.month() % 12) + 1,
                            "case {case:?}: expected a slot on the last day of its month, got \
                             {day}"
                        );
                    } else {
                        let day_of_month = i32::try_from(day.day()).unwrap_or(0);
                        assert!(
                            days.contains(&day_of_month),
                            "case {case:?}: expected a slot on one of the days {days:?} of the \
                             month, got day {day_of_month} ({day})"
                        );
                    }
                }
                if term_start > now {
                    assert_eq!(
                        next, term_start,
                        "case {case:?}: a term that has not started yet starts its first slot at \
                         the term's start, got {next}"
                    );
                    assert_eq!(
                        previous, None,
                        "case {case:?}: no schedule has ended before the term starts, got \
                         {instance:?}"
                    );
                } else {
                    let previous = previous.unwrap_or_else(|| {
                        panic!("case {case:?}: expected a previous slot end: {instance:?}")
                    });
                    assert!(
                        previous > term_start && previous < next + 6 * SECONDS_PER_HOUR,
                        "case {case:?}: expected the previous slot's end inside the term and \
                         before the next slot's own end, got {previous} against {next}"
                    );
                    assert_eq!(
                        (previous - 6 * SECONDS_PER_HOUR - term_start)
                            .rem_euclid(SECONDS_PER_DAY),
                        0,
                        "case {case:?}: the previous slot ends its six hours after a start at \
                         the term's own time of day, got {previous}"
                    );
                }
            }
        }
    }
    assert_eq!(
        stored(&engine),
        baseline,
        "none of those reads wrote anything, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// The days a stored schedule keeps are the days its period publishes: a day outside the
/// published range of the frequency is no day of that schedule, and a day named twice is one
/// day — so the report carries the published days in order, once each, and the term's hours
/// follow from that count rather than from how many days the purchase named.
///
/// The rows span both periods and both ends of each published range, with a duplicate and an
/// out-of-range day in each. The ranges are the members' own:
/// `ScheduledInstanceRecurrenceRequest$OccurrenceDays` — "for a weekly schedule, this is one or
/// more days of the week (1-7, where 1 is Sunday)" and "for a monthly schedule, this is one or
/// more days of the month (1-31)".
#[tokio::test]
async fn a_schedule_keeps_only_the_days_its_period_publishes() {
    let (router, engine) = emulator();

    // (case, the frequency, the days the purchase named, the days expected back, the slots a
    // term holds for those days)
    type Row<'a> = (&'a str, &'a str, Vec<i32>, Vec<String>, i64);
    let rows: Vec<Row<'_>> = vec![
        (
            "a weekly schedule naming both ends of its range",
            "Weekly",
            vec![7, 1],
            vec!["1".to_string(), "7".to_string()],
            104,
        ),
        (
            "a weekly schedule naming a day twice",
            "Weekly",
            vec![3, 3],
            vec!["3".to_string()],
            52,
        ),
        (
            "a weekly schedule naming days outside its range",
            "Weekly",
            vec![0, 2, 8, -4, 31],
            vec!["2".to_string()],
            52,
        ),
        (
            "a monthly schedule naming both ends of its range",
            "Monthly",
            vec![31, 1],
            vec!["1".to_string(), "31".to_string()],
            24,
        ),
        (
            "a monthly schedule naming days outside its range",
            "Monthly",
            vec![0, 15, 32, i32::MAX],
            vec!["15".to_string()],
            12,
        ),
    ];

    for (case, frequency, named, expected, slots) in rows {
        let mut spec = purchase("us-west-2a", LINUX, C4_LARGE);
        spec.recurrence = recurrence(frequency, Some(1), &named, false);
        spec.slot_duration_in_hours = 24;
        let id = seed(&engine, vec![spec])
            .pop()
            .expect("one purchase was stored");

        let body = served(&router, case, REGION, &vec![p("ScheduledInstanceId.1", id.clone())]).await;
        let instance = reported(case, &body)
            .into_iter()
            .find(|instance| instance.scheduled_instance_id == id)
            .unwrap_or_else(|| panic!("case {case:?}: expected the purchase reported: {body}"));
        assert_eq!(
            instance.occurrence_days, expected,
            "case {case:?}: expected only the days the period publishes, got {instance:?}"
        );
        assert_eq!(
            instance.total_scheduled_instance_hours,
            (24 * slots).to_string(),
            "case {case:?}: expected the term's hours to follow from {slots} slots of 24 hours, \
             got {instance:?}"
        );
    }
}

/// A report is a READ: the same request answers the same purchases twice, and the store is
/// untouched — nothing bought, nothing moved, no schedule advanced by having been looked at.
#[tokio::test]
async fn reading_the_report_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    seed(
        &engine,
        vec![
            {
                // A term whose next slot is half a day out, so no slot boundary falls between
                // the two reads.
                let mut spec = purchase("us-west-2a", LINUX, C4_LARGE);
                spec.term_start_secs = now_secs() + 12 * SECONDS_PER_HOUR;
                spec
            },
            {
                let mut spec = purchase("us-west-2b", WINDOWS, M4_XLARGE);
                spec.term_start_secs = now_secs() + 18 * SECONDS_PER_HOUR;
                spec.recurrence = recurrence("Monthly", Some(1), &[9], false);
                spec
            },
        ],
    );
    let baseline = stored(&engine);

    let first = served(&router, "the first read", REGION, &Vec::new()).await;
    let second = served(&router, "the second read", REGION, &Vec::new()).await;
    assert_eq!(
        reported("the first read", &first),
        reported("the second read", &second),
        "expected the same report twice, got {first} then {second}"
    );
    assert_eq!(
        stored(&engine),
        baseline,
        "expected the store to end where it started at {baseline:?}, got {:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// Rule 1 again: the ids, the filters and the window that narrow a report
// ---------------------------------------------------------------------------

/// The ids name which purchases to report, and an id this Region holds no purchase for is
/// REFUSED rather than left out — including one bought in another Region, which this Region holds
/// nothing for. A value that is no Scheduled Instance id at all is refused before anything is
/// looked up.
#[tokio::test]
async fn the_ids_select_the_purchases_and_an_unknown_one_is_refused() {
    let (router, engine) = emulator();
    let ids = seed(
        &engine,
        vec![
            purchase("us-west-2a", LINUX, C4_LARGE),
            purchase("us-west-2b", WINDOWS, M4_XLARGE),
            purchase("us-west-2c", LINUX, M4_XLARGE),
            purchase("us-east-1a", LINUX, C4_LARGE),
        ],
    );
    let baseline = stored(&engine);

    // (case, the ids named, the ids expected back)
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("no id at all", Vec::new(), ids[..3].to_vec()),
        ("one id", vec![ids[1].clone()], vec![ids[1].clone()]),
        (
            "two ids",
            vec![ids[2].clone(), ids[0].clone()],
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "the same id twice",
            vec![ids[0].clone(), ids[0].clone()],
            vec![ids[0].clone()],
        ),
    ];
    for (case, named, expected) in rows {
        let params: Params = named
            .iter()
            .enumerate()
            .map(|(index, id)| p(&format!("ScheduledInstanceId.{}", index + 1), id.clone()))
            .collect();
        let body = served(&router, case, REGION, &params).await;
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected exactly the purchases the ids name, got {body}"
        );
    }

    // (case, the id named, the error code expected)
    let refusals: Vec<(&str, String, &str)> = vec![
        (
            "a well-formed id nothing was bought under",
            "sci-ec20-0000-0000-0000-fffffffffffffff".to_string(),
            "InvalidScheduledInstanceId.NotFound",
        ),
        (
            "an id bought in another Region",
            ids[3].clone(),
            "InvalidScheduledInstanceId.NotFound",
        ),
        (
            "a value that is no Scheduled Instance id",
            "sci-1234".to_string(),
            "InvalidScheduledInstanceId.Malformed",
        ),
        (
            "an id of another resource kind",
            "i-0123456789abcdef0".to_string(),
            "InvalidScheduledInstanceId.Malformed",
        ),
        (
            "an id whose groups carry no hexadecimal",
            "sci-ec20-0000-0000-0000-zzzzzzzzzzzzzzz".to_string(),
            "InvalidScheduledInstanceId.Malformed",
        ),
    ];
    for (case, named, code) in refusals {
        let (status, body) = call(
            &router,
            REGION,
            ACTION,
            &[p("ScheduledInstanceId.1", named.clone())],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {case:?}: expected HTTP 400 {code}, got {status} with body {body}"
        );
        assert!(
            error_message(&body).contains(&named),
            "case {case:?}: the refusal names the id that was refused, got {body}"
        );
    }
    assert_eq!(
        stored(&engine),
        baseline,
        "none of those requests wrote anything, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// Each documented filter narrows the report by one member of the reported purchase, several
/// values of one filter are joined with OR and several filters with AND — and a value nothing
/// holds selects nothing rather than everything. A filter this operation does not document is
/// refused.
#[tokio::test]
async fn the_documented_filters_narrow_the_report() {
    let (router, engine) = emulator();
    let ids = seed(
        &engine,
        vec![
            purchase("us-west-2a", LINUX, C4_LARGE),
            purchase("us-west-2a", WINDOWS, C4_LARGE),
            purchase("us-west-2b", LINUX, M4_XLARGE),
            purchase("us-west-2c", WINDOWS, M4_XLARGE),
        ],
    );
    let baseline = stored(&engine);

    // (case, the filter entries, the purchases expected)
    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        (
            "one zone",
            filters(&[("availability-zone", &["us-west-2a"])]),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "one instance type",
            filters(&[("instance-type", &[M4_XLARGE])]),
            vec![ids[2].clone(), ids[3].clone()],
        ),
        (
            "one platform",
            filters(&[("platform", &[WINDOWS])]),
            vec![ids[1].clone(), ids[3].clone()],
        ),
        (
            "two zones, joined with OR",
            filters(&[("availability-zone", &["us-west-2b", "us-west-2c"])]),
            vec![ids[2].clone(), ids[3].clone()],
        ),
        (
            "a zone and a platform, joined with AND",
            filters(&[("availability-zone", &["us-west-2a"]), ("platform", &[LINUX])]),
            vec![ids[0].clone()],
        ),
        (
            "a wildcard over the zone",
            filters(&[("availability-zone", &["us-west-2*"])]),
            ids[..4].to_vec(),
        ),
        (
            "a value nothing holds",
            filters(&[("instance-type", &["r3.8xlarge"])]),
            Vec::new(),
        ),
        (
            "two filters nothing holds together",
            filters(&[("availability-zone", &["us-west-2c"]), ("platform", &[LINUX])]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let body = served(&router, case, REGION, &params).await;
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected exactly the purchases the filters leave, got {body}"
        );
    }

    for name in ["instance-id", "platform ", "Platform", "tag:team", ""] {
        let (status, body) = call(&router, REGION, ACTION, &filters(&[(name, &[LINUX])])).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValue"),
            "case {name:?}: a filter this operation does not document is refused, got {status} \
             with body {body}"
        );
    }
    assert_eq!(
        stored(&engine),
        baseline,
        "none of those requests wrote anything, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// The slot-start window selects the purchases whose next schedule starts inside it, each bound
/// narrowing on its own, and a purchase that starts no further slot is selected by no window at
/// all — while a request that names no window selects every purchase.
#[tokio::test]
async fn the_slot_start_window_narrows_the_report() {
    let (router, engine) = emulator();
    let now = now_secs();
    let ids = seed(
        &engine,
        vec![
            {
                let mut spec = purchase("us-west-2a", LINUX, C4_LARGE);
                spec.term_start_secs = now + SECONDS_PER_HOUR;
                spec
            },
            {
                let mut spec = purchase("us-west-2b", LINUX, C4_LARGE);
                spec.term_start_secs = now + 5 * SECONDS_PER_HOUR;
                spec
            },
            {
                let mut spec = purchase("us-west-2c", LINUX, C4_LARGE);
                spec.term_start_secs = now + 20 * SECONDS_PER_HOUR;
                spec
            },
            {
                // A term that has run out: it starts no further slot, so no window selects it.
                let mut spec = purchase("us-west-2a", WINDOWS, M4_XLARGE);
                spec.term_start_secs = now - (TERM_DURATION_IN_DAYS + 3) * SECONDS_PER_DAY;
                spec
            },
        ],
    );
    let baseline = stored(&engine);

    // (case, the window's bounds as offsets from now, the purchases expected)
    let rows: Vec<(&str, Params, Vec<String>)> = vec![
        ("no window at all", Vec::new(), ids[..4].to_vec()),
        (
            "a window over the first two starts",
            window(0, 6 * SECONDS_PER_HOUR),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "a window over the second start alone",
            window(4 * SECONDS_PER_HOUR, 2 * SECONDS_PER_HOUR),
            vec![ids[1].clone()],
        ),
        (
            "an earliest bound alone",
            vec![p(
                "SlotStartTimeRange.EarliestTime",
                iso8601(now + 6 * SECONDS_PER_HOUR),
            )],
            vec![ids[2].clone()],
        ),
        (
            "a latest bound alone",
            vec![p(
                "SlotStartTimeRange.LatestTime",
                iso8601(now + 2 * SECONDS_PER_HOUR),
            )],
            vec![ids[0].clone()],
        ),
        (
            "a window that closed an hour ago",
            window(-2 * SECONDS_PER_HOUR, SECONDS_PER_HOUR),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let body = served(&router, case, REGION, &params).await;
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected exactly the purchases whose next slot starts inside the \
             window, got {body}"
        );
    }
    assert_eq!(
        stored(&engine),
        baseline,
        "none of those requests wrote anything, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// The report pages by `MaxResults` in the store's key order: each page carries at most that many
/// purchases, a token comes back only while another purchase follows, following it resumes at
/// that purchase, and the pages together are the whole report with nothing repeated or dropped.
#[tokio::test]
async fn the_report_pages_by_max_results_and_its_token() {
    let (router, engine) = emulator();
    let mut specs = Vec::new();
    for index in 0..12 {
        let zone = ["us-west-2a", "us-west-2b", "us-west-2c"][index % 3];
        specs.push(purchase(zone, LINUX, C4_LARGE));
    }
    let ids = seed(&engine, specs);
    let baseline = stored(&engine);

    for page_size in [MIN_MAX_RESULTS, 7, MAX_MAX_RESULTS] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next {
                params.push(p("NextToken", token.clone()));
            }
            let body = served(&router, &case, REGION, &params).await;
            let page = reported_ids(&case, &body);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page carries at most {page_size} purchases, got {} in {body}",
                page.len()
            );
            walked.extend(page);
            next = token(&body);
            pages += 1;
            assert!(
                pages <= 12,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: the pages together are the whole report in id order, got {walked:?}"
        );
    }

    // Both ends of one published range: a page size outside it is refused, in the wording that
    // names the range.
    for (case, page_size) in [
        ("below the published minimum", MIN_MAX_RESULTS - 1),
        ("no page at all", 0),
        ("above the published maximum", MAX_MAX_RESULTS + 1),
        ("far above it", 10 * MAX_MAX_RESULTS),
    ] {
        let (status, body) = call(
            &router,
            REGION,
            ACTION,
            &[p("MaxResults", page_size.to_string())],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValue"),
            "case {case:?}: a page size outside the published range is refused, got {status} \
             with body {body}"
        );
        assert!(
            error_message(&body).contains(&format!(
                "It must be between {MIN_MAX_RESULTS} and {MAX_MAX_RESULTS}"
            )),
            "case {case:?}: the refusal names the published range, got {body}"
        );
    }

    let carried = "not-a-token-this-service-minted".to_string();
    let (status, body) = call(&router, REGION, ACTION, &[p("NextToken", carried)]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidPaginationToken"),
        "a token this service did not mint is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_message(&body),
        ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        "expected EC2's own pagination-token sentence, got {body}"
    );
    assert_eq!(
        stored(&engine),
        baseline,
        "none of those requests wrote anything, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}

/// `DryRun` asks whether the caller may make the request, so a request that would have been
/// served is answered `DryRunOperation` and reports nothing — and nothing is written either.
#[tokio::test]
async fn a_dry_run_is_answered_as_the_probe_it_is() {
    let (router, engine) = emulator();
    seed(&engine, vec![purchase("us-west-2a", LINUX, C4_LARGE)]);
    let baseline = stored(&engine);

    for (case, mut params) in [
        ("a dry run of the whole report", Vec::new()),
        (
            "a dry run of a filtered report",
            filters(&[("platform", &[LINUX])]),
        ),
    ] {
        params.push(p("DryRun", "true"));
        let (status, body) = call(&router, REGION, ACTION, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (412, "DryRunOperation"),
            "case {case:?}: expected the dry-run probe's own answer, got {status} with body {body}"
        );
    }
    assert_eq!(
        stored(&engine),
        baseline,
        "a dry run writes nothing, expected {baseline:?}, got {:?}",
        stored(&engine)
    );
}
