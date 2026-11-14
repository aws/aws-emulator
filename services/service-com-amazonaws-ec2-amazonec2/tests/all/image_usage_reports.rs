// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeImageUsageReports` — and for the report id space it shares
//! with `DeleteImageUsageReport` — driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeImageUsageReports`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Three rules are under test:
//!
//! 1. The operation REPORTS THE STORE: the set it answers with is the account's AMI usage
//!    reports narrowed by the `ReportIds`, `ImageIds` and `Filters` the request names and
//!    cut into `MaxResults`-sized pages, and every member the modelled `ImageUsageReport`
//!    carries round-trips out of the store unchanged.
//! 2. A report past its `ExpirationTime` is GONE — "the date and time when Amazon EC2 will
//!    delete the report (30 days after the report was created)" — and nothing else a
//!    describe does changes what a later caller sees.
//! 3. A member a request may get wrong is REFUSED with EC2's own code: a `ReportId` EC2
//!    could never have issued (`InvalidImageUsageReportId.Malformed`, before any lookup) as
//!    against one it could but the account holds no report for
//!    (`InvalidImageUsageReportId.NotFound`), something that is not an AMI id, a filter this
//!    operation does not document, a token the service never issued, and `DryRun`.
//!
//! Every table row is a distinct input along what those rules generalise over — which
//! documented filter, how many values, how many filters, which ids, which page size, where
//! the clock stands against an expiration — so no single recorded call is the
//! specification.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::{
    ImageUsageReport, ImageUsageResourceType, ImageUsageResourceTypeOption, Tag,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IMAGE_USAGE_REPORT_ID_PREFIX,
    MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES, MAX_FILTERS, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The three words `ImageUsageReport$State` documents
/// (`smithy_query(query="shape",shape="ImageUsageReport")`): "`available` - The report is
/// available to view", "`pending` - The report is being created and not available to view",
/// "`error` - The report could not be created". `ImageUsageReportState` is a plain string in
/// the model, so the catalog is named here from that member's documentation.
const STATE_AVAILABLE: &str = "available";
const STATE_PENDING: &str = "pending";
const STATE_ERROR: &str = "error";

/// The Region every call here is signed for. A usage report carries no Region member of its
/// own, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The id prefix the store mints a report id under: [`IMAGE_USAGE_REPORT_ID_PREFIX`]
/// (`imgusgrpt-`) plus 17 hex digits, the form
/// `ec2::state::image_usage_report_id_is_well_formed` accepts.
const REPORT_ID_PREFIX: &str = IMAGE_USAGE_REPORT_ID_PREFIX;

/// A report id shaped like one this account could hold, that nothing was ever stored under.
const UNKNOWN_REPORT_ID: &str = "imgusgrpt-0ffffffffffffffff";

/// An AMI id shaped like one the account could hold, that no seeded report names.
const UNNAMED_IMAGE_ID: &str = "ami-0ffffffffffffffff";

/// The AMIs the seeded reports were created for: `ami-` plus 17 hex digits, the two id
/// forms `image_id_is_well_formed` accepts.
const IMAGE_A: &str = "ami-0aaaaaaaaaaaaaaaa";
const IMAGE_B: &str = "ami-0bbbbbbbbbbbbbbbb";
const IMAGE_C: &str = "ami-0cccccccccccccccc";
const IMAGE_D: &str = "ami-0dddddddddddddddd";

/// An instant far enough ahead that no seeded report expires while this suite runs:
/// 2100-01-01T00:00:00Z as epoch milliseconds.
const FAR_FUTURE_MILLIS: u64 = 4_102_444_800_000;

/// One `DescribeImageUsageReports` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeImageUsageReports", params).await
}

/// The report ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "reportId")
}

/// The XML of the one report `report_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, report_id: &str) -> String {
    let (status, body) = describe(router, &indexed("ReportId", &[report_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: usize) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// One `NextToken` parameter.
fn next_token_param(token: &str) -> Vec<(String, String)> {
    vec![("NextToken".to_string(), token.to_string())]
}

/// A tag, as the seeded reports carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// An instant from epoch milliseconds, which is the precision `MillisecondDateTime` — the
/// type `ImageUsageReport$CreationTime` and `$ExpirationTime` target — carries.
fn instant(millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis)
}

/// The `<item>` texts of the one `<set>…</set>` element the body carries, or none when the
/// service wrote the set empty (a self-closing element has no inner text).
fn set_items(xml: &str, set: &str) -> Vec<String> {
    match element(xml, set) {
        Some(inner) => elements(&inner, "item"),
        None => Vec::new(),
    }
}

/// One filter-table row: the case name, the `Filters` entries the request carries as
/// `(name, values)` pairs, and the seeded positions the response must report.
type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<usize>);

/// One narrowing-table row: the case name, the seeded report positions `ReportIds` names,
/// the image ids `ImageIds` names, and the seeded positions the response must report.
type NarrowingCase<'a> = (&'a str, Vec<usize>, Vec<&'a str>, Vec<usize>);

/// One refusal-table row: the case name and the `Filters` entries the request carries.
type RefusedFilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>);

/// One dry-run-table row: the case name, the parameters the request carries beside
/// `DryRun`, and the HTTP status and error code due.
type DryRunCase<'a> = (&'a str, Vec<(String, String)>, u16, &'a str);

/// What one seeded report holds, beyond the id the store mints for it.
struct ReportFixture {
    /// The image the report was created for.
    image_id: &'static str,
    /// One of the three words `ImageUsageReport$State` documents.
    state: &'static str,
    /// "Provides additional details when the report is in an `error` state"
    /// (`ImageUsageReport$StateReason`), so only the `error` row carries one.
    state_reason: Option<&'static str>,
    /// When the report was created, as epoch milliseconds.
    created_at_millis: u64,
    /// The ISO 8601 UTC rendering of [`Self::created_at_millis`] — what the response
    /// carries and what the `creation-time` filter compares against.
    creation_time: &'static str,
    /// When EC2 deletes the report, as epoch milliseconds; `None` for a report that has
    /// been given no expiration.
    expires_at_millis: Option<u64>,
    /// "The IDs of the Amazon Web Services accounts that were specified when the report was
    /// created" (`ImageUsageReport$AccountIds`).
    account_ids: Vec<&'static str>,
    /// "The resource types that were specified when the report was created"
    /// (`ImageUsageReport$ResourceTypes`).
    resource_types: Vec<ImageUsageResourceType>,
    /// "Any tags assigned to the report" (`ImageUsageReport$Tags`).
    tags: Vec<Tag>,
}

/// One `ImageUsageResourceType`, from the two values `ImageUsageResourceType$ResourceType`
/// documents ("Valid values: `ec2:Instance` | `ec2:LaunchTemplate`") and the options
/// "valid only when `ResourceType` is `ec2:LaunchTemplate`".
///
/// An option's name and values are opaque to this service: the model gives them no catalog,
/// so the store reports whatever the creating request set.
fn resource_type(
    name: &str,
    options: Option<Vec<ImageUsageResourceTypeOption>>,
) -> ImageUsageResourceType {
    ImageUsageResourceType {
        resource_type: Some(name.to_string()),
        resource_type_options: options,
    }
}

/// One `ImageUsageResourceTypeOption`.
fn resource_type_option(name: &str, values: &[&str]) -> ImageUsageResourceTypeOption {
    ImageUsageResourceTypeOption {
        option_name: Some(name.to_string()),
        option_values: Some(values.iter().map(|value| (*value).to_string()).collect()),
    }
}

/// The reports the filter, narrowing and paging tables are written against. They differ
/// along every attribute the documented filters select on and along what the modelled
/// `ImageUsageReport` carries: which image (two reports share one, and two more share
/// another), which state, whether a state reason is set, whether an expiration is set, how
/// many accounts, how many resource types and options, how many tags, and whether the
/// creation instant carries a millisecond fraction.
fn seeded_fixtures() -> Vec<ReportFixture> {
    vec![
        ReportFixture {
            image_id: IMAGE_A,
            state: STATE_AVAILABLE,
            state_reason: None,
            // 2026-01-01T00:00:00Z — a whole second, so the wire rendering carries no
            // fraction.
            created_at_millis: 1_767_225_600_000,
            creation_time: "2026-01-01T00:00:00Z",
            expires_at_millis: Some(FAR_FUTURE_MILLIS),
            account_ids: vec!["111111111111"],
            resource_types: vec![resource_type("ec2:Instance", None)],
            tags: vec![tag("Name", "weekly"), tag("Tier", "prod")],
        },
        ReportFixture {
            image_id: IMAGE_A,
            state: STATE_PENDING,
            state_reason: None,
            // 2026-01-02T03:04:05Z
            created_at_millis: 1_767_323_045_000,
            creation_time: "2026-01-02T03:04:05Z",
            expires_at_millis: None,
            account_ids: Vec::new(),
            resource_types: Vec::new(),
            tags: Vec::new(),
        },
        ReportFixture {
            image_id: IMAGE_B,
            state: STATE_ERROR,
            state_reason: Some("The report could not be created"),
            // 2026-01-03T11:04:43.305Z — the millisecond form the `creation-time` filter's
            // documentation gives as its example.
            created_at_millis: 1_767_438_283_305,
            creation_time: "2026-01-03T11:04:43.305Z",
            expires_at_millis: Some(FAR_FUTURE_MILLIS),
            account_ids: vec!["111111111111", "222222222222"],
            resource_types: vec![resource_type(
                "ec2:LaunchTemplate",
                Some(vec![resource_type_option(
                    "ec2:LaunchTemplateVersions",
                    &["3", "4"],
                )]),
            )],
            tags: vec![tag("Name", "audit")],
        },
        ReportFixture {
            image_id: IMAGE_C,
            state: STATE_AVAILABLE,
            state_reason: None,
            // 2026-01-03T23:59:59Z — the same day as the row above, which is what the
            // `creation-time` wildcard selects on.
            created_at_millis: 1_767_484_799_000,
            creation_time: "2026-01-03T23:59:59Z",
            expires_at_millis: None,
            account_ids: vec!["333333333333"],
            resource_types: vec![
                resource_type("ec2:Instance", None),
                resource_type(
                    "ec2:LaunchTemplate",
                    Some(vec![resource_type_option(
                        "ec2:LaunchTemplateVersions",
                        &["1"],
                    )]),
                ),
            ],
            tags: vec![tag("Tier", "dev")],
        },
        ReportFixture {
            image_id: IMAGE_C,
            state: STATE_AVAILABLE,
            state_reason: None,
            // 2026-02-14T06:30:00Z
            created_at_millis: 1_771_050_600_000,
            creation_time: "2026-02-14T06:30:00Z",
            expires_at_millis: Some(FAR_FUTURE_MILLIS),
            account_ids: vec!["111111111111"],
            resource_types: vec![resource_type("ec2:Instance", None)],
            tags: vec![tag("Name", "weekly")],
        },
        ReportFixture {
            image_id: IMAGE_D,
            state: STATE_AVAILABLE,
            state_reason: None,
            // 2026-03-01T12:00:00Z
            created_at_millis: 1_772_366_400_000,
            creation_time: "2026-03-01T12:00:00Z",
            expires_at_millis: None,
            account_ids: Vec::new(),
            resource_types: Vec::new(),
            tags: Vec::new(),
        },
    ]
}

/// Write one report into the engine's store, the way `CreateImageUsageReport` would, and
/// answer the id it was given.
fn seed_one(guard: &mut Ec2State, fixture: ReportFixture) -> String {
    let report_id = guard.next_id(REPORT_ID_PREFIX);
    guard.image_usage_reports.insert(
        report_id.clone(),
        ImageUsageReport {
            image_id: Some(fixture.image_id.to_string()),
            report_id: Some(report_id.clone()),
            resource_types: Some(fixture.resource_types),
            account_ids: Some(
                fixture
                    .account_ids
                    .iter()
                    .map(|account_id| (*account_id).to_string())
                    .collect(),
            ),
            state: Some(fixture.state.to_string()),
            state_reason: fixture.state_reason.map(str::to_string),
            creation_time: Some(system_time_to_date_time(instant(
                fixture.created_at_millis,
            ))),
            expiration_time: fixture
                .expires_at_millis
                .map(|millis| system_time_to_date_time(instant(millis))),
            tags: Some(fixture.tags),
        },
    );
    report_id
}

/// Write every seeded report into the engine's store and report the ids they were given, in
/// store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    seeded_fixtures()
        .into_iter()
        .map(|fixture| seed_one(&mut guard, fixture))
        .collect()
}

/// The seeded ids at `positions`, as the expectation tables name them.
fn ids_at(seeded: &[String], positions: &[usize]) -> Vec<String> {
    positions
        .iter()
        .map(|position| seeded[*position].clone())
        .collect()
}

/// Every report id the store holds, in store order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .image_usage_reports
        .keys()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account holds no AMI usage report — `CreateImageUsageReport` is what adds one —
/// so the operation answers HTTP 200 with an empty set and no token, rather than refusing
/// the call.
#[tokio::test]
async fn a_fresh_account_reports_no_image_usage_report() {
    let (router, _engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "reportId").is_empty(),
        "fresh account: expected no report, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty set, so no token is due, got {body}"
    );
}

/// Every report the store holds is reported, in store order, and a report written after a
/// call shows up in the next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn every_stored_report_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all reports", &[]).await;
    assert_eq!(
        reported, seeded,
        "all reports: expected every stored report in store order, got {reported:?}"
    );

    let late_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        seed_one(
            &mut guard,
            ReportFixture {
                image_id: IMAGE_D,
                state: STATE_PENDING,
                state_reason: None,
                created_at_millis: 1_772_452_800_000,
                creation_time: "2026-03-02T12:00:00Z",
                expires_at_millis: None,
                account_ids: Vec::new(),
                resource_types: Vec::new(),
                tags: Vec::new(),
            },
        )
    };

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// Each member the modelled `ImageUsageReport` carries round-trips out of the store: the
/// report id, the image it was created for, the state and its reason, both instants, every
/// account id, every resource type with its options, and every tag.
#[tokio::test]
async fn every_stored_member_round_trips() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (position, fixture) in seeded_fixtures().into_iter().enumerate() {
        let report_id = &seeded[position];
        let case = format!("report {position} ({report_id})");
        let body = describe_one(&router, &case, report_id).await;

        let reported_id = element(&body, "reportId").unwrap_or_default();
        assert_eq!(
            &reported_id, report_id,
            "case {case:?}: expected reportId {report_id}, got {reported_id:?}"
        );

        let image_id = element(&body, "imageId").unwrap_or_default();
        assert_eq!(
            image_id, fixture.image_id,
            "case {case:?}: expected imageId {:?}, got {image_id:?}",
            fixture.image_id
        );

        let state = element(&body, "state").unwrap_or_default();
        assert_eq!(
            state, fixture.state,
            "case {case:?}: expected state {:?}, got {state:?}",
            fixture.state
        );

        let state_reason = element(&body, "stateReason");
        assert_eq!(
            state_reason.as_deref(),
            fixture.state_reason,
            "case {case:?}: expected stateReason {:?}, got {state_reason:?}",
            fixture.state_reason
        );

        let creation_time = element(&body, "creationTime").unwrap_or_default();
        assert_eq!(
            creation_time, fixture.creation_time,
            "case {case:?}: expected creationTime {:?}, got {creation_time:?}",
            fixture.creation_time
        );

        let expiration_time = element(&body, "expirationTime");
        match fixture.expires_at_millis {
            // Every expiring fixture is written against the same far-future instant, so
            // the rendering is checked against the instant itself rather than a literal.
            Some(_) => assert_eq!(
                expiration_time.as_deref(),
                Some("2100-01-01T00:00:00Z"),
                "case {case:?}: expected the far-future expirationTime, got \
                 {expiration_time:?}"
            ),
            None => assert!(
                expiration_time.is_none(),
                "case {case:?}: a report with no expiration carries no expirationTime, got \
                 {expiration_time:?}"
            ),
        }

        let account_ids = set_items(&body, "accountIdSet");
        assert_eq!(
            account_ids, fixture.account_ids,
            "case {case:?}: expected accountIdSet {:?}, got {account_ids:?}",
            fixture.account_ids
        );

        let reported_types = elements(&body, "resourceType");
        let expected_types: Vec<String> = fixture
            .resource_types
            .iter()
            .map(|entry| entry.resource_type.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            reported_types, expected_types,
            "case {case:?}: expected resourceTypeSet {expected_types:?}, got \
             {reported_types:?}"
        );

        let reported_options = elements(&body, "optionName");
        let expected_options: Vec<String> = fixture
            .resource_types
            .iter()
            .flat_map(|entry| entry.resource_type_options.iter().flatten())
            .map(|option| option.option_name.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            reported_options, expected_options,
            "case {case:?}: expected option names {expected_options:?}, got \
             {reported_options:?}"
        );

        let reported_option_values = set_items(&body, "optionValueSet");
        // Each fixture carries at most one option list, so the first `optionValueSet` the
        // body holds is that option's values.
        let expected_option_values: Vec<String> = fixture
            .resource_types
            .iter()
            .flat_map(|entry| entry.resource_type_options.iter().flatten())
            .flat_map(|option| option.option_values.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            reported_option_values, expected_option_values,
            "case {case:?}: expected option values {expected_option_values:?}, got \
             {reported_option_values:?}"
        );

        let tag_set = element(&body, "tagSet").unwrap_or_default();
        let keys = elements(&tag_set, "key");
        let values = elements(&tag_set, "value");
        let expected_keys: Vec<String> = fixture
            .tags
            .iter()
            .map(|entry| entry.key.clone().unwrap_or_default())
            .collect();
        let expected_values: Vec<String> = fixture
            .tags
            .iter()
            .map(|entry| entry.value.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            keys, expected_keys,
            "case {case:?}: expected tag keys {expected_keys:?}, got {keys:?}"
        );
        assert_eq!(
            values, expected_values,
            "case {case:?}: expected tag values {expected_values:?}, got {values:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing: ids and filters
// ---------------------------------------------------------------------------

/// `ReportIds` and `ImageIds` narrow the report, together and apart: the answer is the
/// reports the request named, of the images it named.
#[tokio::test]
async fn report_ids_and_image_ids_narrow_the_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, the report positions asked for, the image ids asked for, the positions due)`.
    let cases: Vec<NarrowingCase<'_>> = vec![
        ("no narrowing at all", vec![], vec![], vec![0, 1, 2, 3, 4, 5]),
        ("one report id", vec![2], vec![], vec![2]),
        ("three report ids", vec![0, 3, 5], vec![], vec![0, 3, 5]),
        (
            "every report id",
            vec![0, 1, 2, 3, 4, 5],
            vec![],
            vec![0, 1, 2, 3, 4, 5],
        ),
        ("one image, two reports", vec![], vec![IMAGE_A], vec![0, 1]),
        ("one image, one report", vec![], vec![IMAGE_B], vec![2]),
        (
            "two images",
            vec![],
            vec![IMAGE_B, IMAGE_D],
            vec![2, 5],
        ),
        (
            "an image no report names",
            vec![],
            vec![UNNAMED_IMAGE_ID],
            vec![],
        ),
        (
            "both members, agreeing",
            vec![0, 1, 2],
            vec![IMAGE_A],
            vec![0, 1],
        ),
        (
            "both members, disagreeing",
            vec![0, 1],
            vec![IMAGE_B],
            vec![],
        ),
    ];

    for (case, report_positions, image_ids, expected_positions) in cases {
        let asked = ids_at(&seeded, &report_positions);
        let mut params = indexed(
            "ReportId",
            &asked.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        params.extend(indexed("ImageId", &image_ids));
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected_positions),
            "case {case:?}: expected the reports at {expected_positions:?}, got {reported:?}"
        );
    }
}

/// Each documented filter selects on the stored report, values inside one filter are joined
/// with OR and separate filters with AND, and EC2's `*` wildcard applies — including on the
/// `creation-time` instant, which answers both the millisecond form the member documents
/// and the form the response carries.
#[tokio::test]
async fn documented_filters_select_on_the_stored_report() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCase<'_>> = vec![
        (
            "state available",
            vec![("state", vec![STATE_AVAILABLE])],
            vec![0, 3, 4, 5],
        ),
        (
            "state pending or error — two values, joined with OR",
            vec![(
                "state",
                vec![
                    STATE_PENDING,
                    STATE_ERROR,
                ],
            )],
            vec![1, 2],
        ),
        (
            "state with a wildcard",
            vec![("state", vec!["a*"])],
            vec![0, 3, 4, 5],
        ),
        (
            "state nothing is in",
            vec![("state", vec!["disabled"])],
            vec![],
        ),
        (
            "creation-time, the form the response carries",
            vec![("creation-time", vec!["2026-01-02T03:04:05Z"])],
            vec![1],
        ),
        (
            "creation-time, the millisecond form the member documents",
            vec![("creation-time", vec!["2026-01-02T03:04:05.000Z"])],
            vec![1],
        ),
        (
            "creation-time, an instant that carries milliseconds",
            vec![("creation-time", vec!["2026-01-03T11:04:43.305Z"])],
            vec![2],
        ),
        (
            "creation-time, a whole day by wildcard",
            vec![("creation-time", vec!["2026-01-03T*"])],
            vec![2, 3],
        ),
        (
            "creation-time, a whole month by wildcard",
            vec![("creation-time", vec!["2026-01-*"])],
            vec![0, 1, 2, 3],
        ),
        (
            "creation-time, two days by OR",
            vec![(
                "creation-time",
                vec!["2026-02-14T06:30:00Z", "2026-03-01T12:00:00Z"],
            )],
            vec![4, 5],
        ),
        (
            "tag:Name, one value",
            vec![("tag:Name", vec!["weekly"])],
            vec![0, 4],
        ),
        (
            "tag:Name, a value no report carries",
            vec![("tag:Name", vec!["monthly"])],
            vec![],
        ),
        (
            "tag:Tier, two values",
            vec![("tag:Tier", vec!["prod", "dev"])],
            vec![0, 3],
        ),
        (
            "tag:Owner — a key no report carries, so no value to match",
            vec![("tag:Owner", vec!["TeamA"])],
            vec![],
        ),
        ("tag-key Name", vec![("tag-key", vec!["Name"])], vec![0, 2, 4]),
        (
            "tag-key, either key",
            vec![("tag-key", vec!["Name", "Tier"])],
            vec![0, 2, 3, 4],
        ),
        (
            "two filters, joined with AND",
            vec![
                ("state", vec![STATE_AVAILABLE]),
                ("tag:Name", vec!["weekly"]),
            ],
            vec![0, 4],
        ),
        (
            "three filters, joined with AND",
            vec![
                ("state", vec![STATE_AVAILABLE]),
                ("tag-key", vec!["Tier"]),
                ("creation-time", vec!["2026-01-*"]),
            ],
            vec![0, 3],
        ),
        (
            "two filters that agree on nothing",
            vec![
                ("state", vec![STATE_PENDING]),
                ("tag-key", vec!["Name"]),
            ],
            vec![],
        ),
        (
            "a filter carrying no value at all matches nothing",
            vec![("state", vec![])],
            vec![],
        ),
    ];

    for (case, entries, expected_positions) in cases {
        let as_slices: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let reported = described_ids(&router, case, &filters(&as_slices)).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &expected_positions),
            "case {case:?}: expected the reports at {expected_positions:?}, got {reported:?}"
        );
    }
}

/// A filter this operation does not document, and one larger than the service will
/// evaluate, are refused with `InvalidParameterValue` rather than read as "no results".
#[tokio::test]
async fn an_undocumented_or_oversized_filter_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let too_many_values: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|index| format!("value-{index}"))
        .collect();
    let oversize_value = "v".repeat(MAX_FILTER_VALUE_BYTES + 1);

    // `(case, the filter entries)`: a name this operation documents no filter for, a `tag:`
    // prefix with no key after it, one filter too many, one value too many in one filter,
    // and one value too long.
    let cases: Vec<RefusedFilterCase<'_>> = vec![
        (
            "a name this operation documents no filter for",
            vec![("sim2real-not-a-real-filter", vec!["anything"])],
        ),
        (
            "a filter name that is documented for another operation",
            vec![("image-id", vec![IMAGE_A])],
        ),
        (
            "a tag filter with no key after the prefix",
            vec![("tag:", vec!["weekly"])],
        ),
        (
            "a documented name in the wrong case — filter names are case-sensitive",
            vec![("State", vec![STATE_AVAILABLE])],
        ),
        (
            "one filter more than the service evaluates",
            (0..=MAX_FILTERS).map(|_| ("state", vec!["available"])).collect(),
        ),
        (
            "one value more than one filter takes",
            vec![(
                "state",
                too_many_values.iter().map(String::as_str).collect(),
            )],
        ),
        (
            "one value longer than the service compares",
            vec![("state", vec![oversize_value.as_str()])],
        ),
    ];

    for (case, entries) in cases {
        let as_slices: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let (status, body) = describe(&router, &filters(&as_slices)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// A `ReportId` the account holds no report for is refused the way `DeleteImageUsageReport`
/// refuses one; something that is not a report id, or not an AMI id, at all is refused
/// before any lookup; and a well-formed AMI id no report names simply selects nothing.
#[tokio::test]
async fn unresolvable_ids_are_answered_with_ec2s_own_codes() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, the member, the value, the code due)`.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a report id nothing was stored under",
            "ReportId",
            UNKNOWN_REPORT_ID,
            "InvalidImageUsageReportId.NotFound",
        ),
        (
            "a report id that is not a report id at all",
            "ReportId",
            "not-a-report",
            "InvalidImageUsageReportId.Malformed",
        ),
        (
            "an image id that is not an AMI id at all",
            "ImageId",
            "not-an-ami",
            "InvalidAMIID.Malformed",
        ),
        (
            "an image id with the right prefix and the wrong digits",
            "ImageId",
            "ami-zzzz",
            "InvalidAMIID.Malformed",
        ),
    ];

    for (case, member, value, expected_code) in cases {
        let (status, body) = describe(&router, &indexed(member, &[value])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A well-formed image id no report names is NOT an error: `ImageIds` narrows the
    // reports, and a report outlives the image it was created for.
    let reported = described_ids(
        &router,
        "a well-formed image id no report names",
        &indexed("ImageId", &[UNNAMED_IMAGE_ID]),
    )
    .await;
    assert!(
        reported.is_empty(),
        "a well-formed image id no report names: expected no report, got {reported:?}"
    );

    // One unresolvable id among resolvable ones still refuses the call.
    let mut params = indexed("ReportId", &[seeded[0].as_str(), UNKNOWN_REPORT_ID]);
    params.extend(indexed("ImageId", &[IMAGE_A]));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "one unknown id among known ones: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidImageUsageReportId.NotFound",
        "one unknown id among known ones: expected InvalidImageUsageReportId.NotFound, got \
         {body}"
    );
}

// ---------------------------------------------------------------------------
// The report id space: malformed before not found
// ---------------------------------------------------------------------------

/// One `DeleteImageUsageReport` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteImageUsageReport", params).await
}

/// The one `ReportId` form field `DeleteImageUsageReport` takes.
fn report_id_param(report_id: &str) -> Vec<(String, String)> {
    vec![("ReportId".to_string(), report_id.to_string())]
}

/// One id-space row: the case name, the id the request carries, and whether it is an id EC2
/// could have ISSUED.
type ReportIdCase<'a> = (&'a str, &'a str, bool);

/// A report id EC2 could never have ISSUED is refused `InvalidImageUsageReportId.Malformed`
/// before anything is looked up, and only a well-formed id the account holds no report for
/// is `InvalidImageUsageReportId.NotFound` — on `DeleteImageUsageReport` and
/// `DescribeImageUsageReports` alike, since both read one id space.
///
/// Every row differs along what the id rule generalises over — the prefix, the separator,
/// the digit count at each accepted bound and one past it, the digit alphabet, the case of
/// the digits, and the null id — so no single id is the specification.
#[tokio::test]
async fn a_report_id_outside_the_id_space_is_refused_malformed_before_any_lookup() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, the id, whether EC2 could have issued it)`. The two accepted digit counts are
    // the short and the long EC2 resource id form (8 and 17 lowercase hexadecimal digits),
    // and the null id is excluded because ids are minted from a counter that starts at one.
    let cases: Vec<ReportIdCase<'_>> = vec![
        ("the null id, long form", "imgusgrpt-00000000000000000", false),
        ("the null id, short form", "imgusgrpt-00000000", false),
        ("the long form", "imgusgrpt-0123456789abcdef0", true),
        ("the long form, every digit used", "imgusgrpt-0fedcba9876543210", true),
        ("the short form", "imgusgrpt-0abc1234", true),
        ("the short form, one non-zero digit", "imgusgrpt-00000001", true),
        ("uppercase hexadecimal digits", "imgusgrpt-0ABCDEF0123456789", false),
        ("digits outside hexadecimal", "imgusgrpt-0zzzzzzzzzzzzzzzz", false),
        ("one digit short of the long form", "imgusgrpt-0123456789abcdef", false),
        ("one digit past the long form", "imgusgrpt-0123456789abcdef01", false),
        ("one digit short of the short form", "imgusgrpt-0abc123", false),
        ("one digit past the short form", "imgusgrpt-0abc12345", false),
        ("no digits at all", "imgusgrpt-", false),
        ("the prefix without its separator", "imgusgrpt0123456789abcdef0", false),
        ("another resource's prefix", "vol-0123456789abcdef0", false),
        ("no prefix at all", "0123456789abcdef0", false),
        ("a word that is not an id", "not-a-report", false),
        ("the long form with a trailing space", "imgusgrpt-0123456789abcdef0 ", false),
    ];

    for (case, report_id, well_formed) in cases {
        let expected_code = if well_formed {
            "InvalidImageUsageReportId.NotFound"
        } else {
            "InvalidImageUsageReportId.Malformed"
        };
        for (action, params) in [
            ("DeleteImageUsageReport", report_id_param(report_id)),
            (
                "DescribeImageUsageReports",
                indexed("ReportId", &[report_id]),
            ),
        ] {
            let (status, body) = if action == "DeleteImageUsageReport" {
                delete(&router, &params).await
            } else {
                describe(&router, &params).await
            };
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on {action}: expected {expected_code}, got {body}"
            );
            if !well_formed {
                let message = unescaped(&element(&body, "Message").unwrap_or_default());
                assert_eq!(
                    message,
                    format!("The image-usage-report ID '{report_id}' is malformed"),
                    "case {case:?} on {action}: the message names the id the request carried, \
                     got {message:?}"
                );
            }
        }
        // A refused call writes nothing: the store still holds every seeded report.
        assert_eq!(
            stored_ids(&engine),
            seeded,
            "case {case:?}: a refused call leaves the store as it found it"
        );
    }

    // The same id space resolves a report that IS held: the delete removes the one it names
    // — read back through `DescribeImageUsageReports` — and the id it removed is then
    // answered NotFound rather than Malformed, which is the half of the rule the refusals
    // above cannot show.
    let (status, body) = delete(&router, &report_id_param(&seeded[0])).await;
    assert_eq!(
        status, 200,
        "deleting a held report: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "deleting a held report reports true, got {body}"
    );
    let remaining = described_ids(&router, "after the delete", &[]).await;
    assert_eq!(
        remaining,
        seeded[1..].to_vec(),
        "after the delete: expected the other reports, got {remaining:?}"
    );
    let (status, body) = delete(&router, &report_id_param(&seeded[0])).await;
    assert_eq!(
        status, 400,
        "deleting it twice: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidImageUsageReportId.NotFound",
        "deleting it twice: expected InvalidImageUsageReportId.NotFound, got {body}"
    );
}

/// On the DELETE, `DryRun` — "checks whether you have the required permissions for the
/// action, without actually making the request" — is answered before the request's own value
/// is read at all, so the call removes nothing and reports the verdict whatever id it names:
/// the recorded contract answers a dry run `DryRunOperation` even for a `ReportId` EC2 could
/// never have issued, which is the documented idiom for a pure permission probe.
///
/// Each row differs along what the rule generalises over: whether the named report is held,
/// and whether the id is one EC2 could have issued at all.
#[tokio::test]
async fn a_dry_run_delete_removes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, the id, the status due, the code due)`.
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a report the account holds",
            seeded[0].as_str(),
            412,
            "DryRunOperation",
        ),
        (
            "another report the account holds",
            seeded[3].as_str(),
            412,
            "DryRunOperation",
        ),
        (
            "a well-formed id nothing is stored under",
            UNKNOWN_REPORT_ID,
            412,
            "DryRunOperation",
        ),
        (
            "the null id, which EC2 could never have issued",
            "imgusgrpt-00000000000000000",
            412,
            "DryRunOperation",
        ),
        (
            "another resource's prefix",
            "vol-0123456789abcdef0",
            412,
            "DryRunOperation",
        ),
    ];

    for (case, report_id, expected_status, expected_code) in cases {
        let mut params = report_id_param(report_id);
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            element(&body, "return").is_none(),
            "case {case:?}: a dry run performs no delete, so it reports no outcome, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            seeded,
            "case {case:?}: a dry run leaves every stored report where it was"
        );
    }

    // The same call without the flag is what removes the report, which is how the dry runs
    // above are known to have been answered on a report that could have been deleted.
    let (status, body) = delete(&router, &report_id_param(&seeded[0])).await;
    assert_eq!(
        status, 200,
        "the delete itself: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_ids(&engine),
        seeded[1..].to_vec(),
        "the delete itself removes the report the dry runs left in place"
    );
}

/// `DryRun` answers whether the request would have succeeded — `DryRunOperation` when it
/// would, and the reason it would not when it would not — and reports nothing either way.
#[tokio::test]
async fn dry_run_answers_whether_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, the parameters beside DryRun, the status due, the code due)`.
    let cases: Vec<DryRunCase<'_>> = vec![
        ("no other parameter", Vec::new(), 412, "DryRunOperation"),
        (
            "a request that narrows to one report",
            indexed("ReportId", &[seeded[1].as_str()]),
            412,
            "DryRunOperation",
        ),
        (
            "a request whose filter is undocumented",
            filters(&[("sim2real-not-a-real-filter", &["anything"])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a request whose report id resolves to nothing",
            indexed("ReportId", &[UNKNOWN_REPORT_ID]),
            400,
            "InvalidImageUsageReportId.NotFound",
        ),
    ];

    for (case, mut params, expected_status, expected_code) in cases {
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            elements(&body, "reportId").is_empty(),
            "case {case:?}: a dry run reports no report, got {body}"
        );
    }

    assert_eq!(
        stored_ids(&engine),
        seeded,
        "a dry run leaves every stored report where it was"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever page size a caller asks for, walking the tokens visits every stored report
/// exactly once and in the one order, and the last page carries no token.
#[tokio::test]
async fn pages_partition_the_set_whatever_the_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for page_size in 1..=seeded.len() + 1 {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = max_results_param(page_size);
            if let Some(token) = token.as_deref() {
                params.extend(next_token_param(token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "reportId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} reports in a page, got \
                 {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: the walk must end, got {pages} pages for {} reports",
                seeded.len()
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected the walk to visit every report once, got {walked:?}"
        );
    }
}

/// `ReportIds` and `ImageIds` each carry the modelled `length 0..200`
/// (`smithy_query(query="operation",operation="DescribeImageUsageReports")`), so a longer
/// list is refused before the handler runs rather than silently narrowed or truncated.
#[tokio::test]
async fn an_id_list_longer_than_the_model_admits_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // `(the member, the id form it takes)`: the modelled maximum is 200, so 201 is one too
    // many for either list.
    for (member, prefix) in [("ReportId", REPORT_ID_PREFIX), ("ImageId", "ami")] {
        let ids: Vec<String> = (1..=201)
            .map(|index| format!("{prefix}-{index:017x}"))
            .collect();
        let params = indexed(
            member,
            &ids.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {member:?}: expected HTTP 400 for {} ids, got {status} with body {body}",
            ids.len()
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {member:?}: expected an error code, got {body}"
        );
    }
}

/// A `NextToken` this service never issued is refused rather than read as a report id, and
/// a token is due only when a report follows the page.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `(case, the token)`: a report id sent as a token, a hex-encoded report id without the
    // envelope the codec writes, and something that is not a token at all.
    for (case, token) in [
        ("a report id", seeded[0].clone()),
        ("a hex-encoded report id", ::hex::encode(&seeded[0])),
        ("not a token at all", "zzzz".to_string()),
    ] {
        let (status, body) = describe(&router, &next_token_param(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // A page that holds every report is the last page, so no token is due.
    let (status, body) = describe(&router, &max_results_param(seeded.len())).await;
    assert_eq!(
        status, 200,
        "the whole set in one page: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "the whole set in one page: nothing follows it, so no token is due, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The clock: an expiration that has come due
// ---------------------------------------------------------------------------

/// "The date and time when Amazon EC2 will delete the report (30 days after the report was
/// created)" (`ImageUsageReport$ExpirationTime`): past that instant the report is gone —
/// not reported, not held, and not resolvable by id — while a report whose expiration is
/// still ahead, and one that carries none at all, stay.
#[tokio::test]
async fn a_report_past_its_expiration_is_gone() {
    let (router, engine) = super::ec2_query::emulator();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let now = u64::try_from(now).expect("the clock fits in a u64 of milliseconds");

    // `(case, when the report expires, whether it is still held)`: an hour past, a second
    // past, a second ahead, an hour ahead, and no expiration at all.
    let rows: Vec<(&str, Option<u64>, bool)> = vec![
        ("an hour past its expiration", Some(now - 3_600_000), false),
        ("a second past its expiration", Some(now - 1_000), false),
        ("a second before it", Some(now + 1_000), true),
        ("an hour before it", Some(now + 3_600_000), true),
        ("no expiration at all", None, true),
    ];

    let mut seeded: Vec<(&str, String, bool)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        for (case, expires_at_millis, survives) in &rows {
            let report_id = seed_one(
                &mut guard,
                ReportFixture {
                    image_id: IMAGE_A,
                    state: STATE_AVAILABLE,
                    state_reason: None,
                    created_at_millis: now,
                    creation_time: "",
                    expires_at_millis: *expires_at_millis,
                    account_ids: Vec::new(),
                    resource_types: Vec::new(),
                    tags: Vec::new(),
                },
            );
            seeded.push((case, report_id, *survives));
        }
    }

    let reported = described_ids(&router, "with expirations around now", &[]).await;
    let expected: Vec<String> = seeded
        .iter()
        .filter(|(_, _, survives)| *survives)
        .map(|(_, report_id, _)| report_id.clone())
        .collect();
    assert_eq!(
        reported, expected,
        "with expirations around now: expected only the reports EC2 has not deleted yet, \
         got {reported:?}"
    );

    // The deletion the clock owed is materialised, not just hidden: the store no longer
    // holds the expired reports, and a second call answers the same set.
    assert_eq!(
        stored_ids(&engine),
        expected,
        "the store holds only the reports EC2 has not deleted yet"
    );
    let again = described_ids(&router, "a second call", &[]).await;
    assert_eq!(
        again, expected,
        "a second call: expected the same set, got {again:?}"
    );

    for (case, report_id, survives) in &seeded {
        let (status, body) = describe(&router, &indexed("ReportId", &[report_id.as_str()])).await;
        if *survives {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                elements(&body, "reportId"),
                vec![report_id.clone()],
                "case {case:?}: expected the report to be reported, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidImageUsageReportId.NotFound",
                "case {case:?}: expected InvalidImageUsageReportId.NotFound, got {body}"
            );
        }
    }
}

/// A describe changes nothing a later caller sees: every report it reported is still held,
/// with the same state, after narrowing, filtering, paging and a refused call.
#[tokio::test]
async fn a_describe_leaves_the_store_as_it_found_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let before: Vec<(String, Option<String>)> = engine
        .state::<Ec2State>()
        .lock()
        .image_usage_reports
        .iter()
        .map(|(report_id, report)| (report_id.clone(), report.state.clone()))
        .collect();

    let calls: Vec<(&str, Vec<(String, String)>)> = vec![
        ("every report", Vec::new()),
        (
            "one report by id",
            indexed("ReportId", &[seeded[0].as_str()]),
        ),
        ("one image", indexed("ImageId", &[IMAGE_C])),
        (
            "one filter",
            filters(&[("state", &[STATE_AVAILABLE])]),
        ),
        ("one page", max_results_param(2)),
        (
            "a call that is refused",
            indexed("ReportId", &[UNKNOWN_REPORT_ID]),
        ),
    ];

    for (case, params) in calls {
        let (status, body) = describe(&router, &params).await;
        assert!(
            status == 200 || status == 400,
            "case {case:?}: expected the call to be answered, got {status} with body {body}"
        );
        let after: Vec<(String, Option<String>)> = engine
            .state::<Ec2State>()
            .lock()
            .image_usage_reports
            .iter()
            .map(|(report_id, report)| (report_id.clone(), report.state.clone()))
            .collect();
        assert_eq!(
            after, before,
            "case {case:?}: expected the store to be unchanged, got {after:?}"
        );
    }
}
