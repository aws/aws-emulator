// SPDX-License-Identifier: CC0-1.0

//! The account relationships `ListBillingViewSegments` reports, and the
//! segments they cut, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about one RULE: a billing view's reported segment
//! timeline is its RECORDED timeline refined by the account relationships of the
//! account that owns it, each read from the service that OWNS that fact — the
//! organization (and the instant the account joined it) from AWS Organizations,
//! the billing-group membership (and the group's primary account, and so the
//! `PRO_FORMA` shaping of the data) from Billing Conductor — so a relationship
//! that changed inside one recorded period answers with several segments, each
//! with its own time range, and a relationship a record states itself is the one
//! that record reports.
//!
//! That is what makes the answer FOLLOW the calls a client makes: the worlds
//! below are established by writing the owning services' own records, the way
//! their own operations write them (`Organizations:CreateOrganization` records an
//! organization and the management account's joined timestamp;
//! `BillingConductor:CreateBillingGroup`/`AssociateAccounts` record a group and a
//! membership of it from a billing period). The whole wire path — a client
//! calling those operations and then reading the segments back — is pinned in
//! `core/router/tests/all/billing_view_segments_follow_account_relationships.rs`.
//!
//! The rows differ along everything the rule generalises over: which
//! relationship changed, whether it changed before, inside or after the
//! requested period, whether a membership is still in force or has ended,
//! whether its group designates a primary account, a membership covering no
//! billing period at all, two memberships in a row, both relationships changing
//! at different instants, which bounds the request carries, and the page size.
//!
//! The harness (`call`, `standalone_emulator_with_engine`,
//! `set_organization_management_account`, `ACCOUNT_ID`, `PRIMARY_ARN`) is the one
//! `billing_views.rs` owns, the segment reading (`list`, `over`, `reported`,
//! `timeline`, `Reported`) is `billing_view_segments.rs`'s, and the
//! civil-calendar `at` is `credit_allocation_history.rs`'s.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    BillingViewSegment, BILLING_DOMAIN_BILLABLE, BILLING_DOMAIN_PRO_FORMA,
};
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use service_com_amazonaws_billingconductor_awsbillingconductor::state as billing_conductor;
use service_com_amazonaws_organizations_awsorganizationsv20161128::ComAmazonawsOrganizationsAwsorganizationsv20161128State;
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_view_segments::{list, over, reported, timeline, Reported};
use crate::billing_views::{
    call, set_organization_management_account, standalone_emulator_with_engine, ACCOUNT_ID,
    PRIMARY_ARN,
};
use crate::credit_allocation_history::at;

/// The four UTC month boundaries the worlds below change relationships at, far
/// enough in the past that the clock cannot move them.
const JANUARY: i64 = 1_735_689_600; // 2025-01-01T00:00:00Z
const FEBRUARY: i64 = 1_738_368_000; // 2025-02-01T00:00:00Z
const MARCH: i64 = 1_740_787_200; // 2025-03-01T00:00:00Z
const APRIL: i64 = 1_743_465_600; // 2025-04-01T00:00:00Z

/// The instant the account joined an organization in most worlds below: inside
/// February, so the change falls in the MIDDLE of a requested period and of a
/// recorded segment.
const JOINED: i64 = 1_739_188_800; // 2025-02-10T12:00:00Z

/// The billing month a membership begins or ends in, as Billing Conductor
/// records one (`AccountAssociationRecord`, `^\d{4}-(0[1-9]|1[0-2])$`).
const JANUARY_MONTH: &str = "2025-01";
const FEBRUARY_MONTH: &str = "2025-02";
const MARCH_MONTH: &str = "2025-03";

/// The account that manages the organization the worlds below put the caller's
/// account in — a 12-digit account id that is not the caller's own.
const MANAGEMENT_ACCOUNT_ID: &str = "123456789012";

/// The primary account of the billing group a membership names, and of the
/// second group in the world that has two.
const GROUP_PRIMARY_ACCOUNT_ID: &str = "210987654321";
const SECOND_GROUP_PRIMARY_ACCOUNT_ID: &str = "321098765432";

/// The accounts a RECORDED segment names itself, which no owning service holds:
/// what the precedence test shows a record's own statement reporting.
const RECORDED_MANAGEMENT_ACCOUNT_ID: &str = "555555555555";
const RECORDED_GROUP_PRIMARY_ACCOUNT_ID: &str = "666666666666";

/// One expected segment: the domain reported over `[begin, end)`, and the
/// account relationships reported with it. No world here records a billing
/// transfer, so no row expects a `billingTransferAccountId` — see the module
/// doc of `src/state/account_relationships.rs` for why nothing derives one.
fn segment(
    domain: &str,
    begin: i64,
    end: i64,
    management_account_id: Option<&str>,
    billing_group_primary_account_id: Option<&str>,
) -> Reported {
    Reported {
        domain: Some(domain.to_string()),
        begin: Some(begin as f64),
        end: Some(end as f64),
        management_account_id: management_account_id.map(str::to_string),
        billing_group_primary_account_id: billing_group_primary_account_id.map(str::to_string),
        billing_transfer_account_id: None,
    }
}

/// Record, in the AWS Organizations store this engine holds, that the caller's
/// account is a member of an organization managed by `management_account` which
/// it joined at `joined`.
///
/// The organization is recorded exactly as `Organizations:CreateOrganization`
/// records one (the shared harness), and the joined timestamp — that store's own
/// record of when the account became a member, which its `JoinedTimestamp`
/// reports — is then placed at the instant the world wants, the way a client
/// that created its organization at that instant would have left it.
fn join_organization(
    engine: &Arc<engine::InMemoryEngine>,
    management_account: &str,
    joined: i64,
) {
    set_organization_management_account(engine, Some(management_account));
    let organizations = engine.state::<ComAmazonawsOrganizationsAwsorganizationsv20161128State>();
    let mut organizations = organizations.lock();
    let account = organizations
        .accounts
        .get_mut(ACCOUNT_ID)
        .expect("creating an organization records the account that joined it");
    account.joined_timestamp = aws_smithy_types::DateTime::from_secs(joined);
}

/// Record, in the Billing Conductor store this engine holds, a billing group
/// `group_id` whose primary account is `primary_account`, and a membership of it
/// by the caller's account from the billing period `from` up to (but not
/// including) `until`.
///
/// Both records are the ones that service's own operations write:
/// `CreateBillingGroup` stores the group under its ARN, and a membership is held
/// per BILLING PERIOD, from the period the association was made in.
fn join_billing_group(
    engine: &Arc<engine::InMemoryEngine>,
    group_id: &str,
    primary_account: Option<&str>,
    from: &str,
    until: Option<&str>,
) {
    let state =
        engine.state::<billing_conductor::ComAmazonawsBillingconductorAwsbillingconductorState>();
    let mut state = state.lock();
    let arn = billing_conductor::billing_group_arn(group_id);
    state.billing_groups.insert(
        arn.clone(),
        billing_conductor::BillingGroupRecord {
            arn: arn.clone(),
            name: format!("group-{group_id}"),
            primary_account_id: primary_account.map(str::to_string),
            ..billing_conductor::BillingGroupRecord::default()
        },
    );
    state
        .account_associations
        .entry(ACCOUNT_ID.to_string())
        .or_default()
        .push(billing_conductor::AccountAssociationRecord {
            billing_group_arn: arn,
            inclusive_start_billing_period: from.to_string(),
            exclusive_end_billing_period: until.map(str::to_string),
        });
}

/// One row of the relationship table: the world it puts the owning services'
/// stores in, and the segments the caller's PRIMARY view then reports over the
/// whole of `[JANUARY, APRIL)`.
struct Row {
    case: &'static str,
    world: fn(&Arc<engine::InMemoryEngine>),
    want: Vec<Reported>,
}

/// What each world reports, over one requested period. Every row starts from the
/// SAME recorded timeline — the one open-ended `BILLABLE` record a fresh
/// account's PRIMARY view carries, which states no relationship — so what
/// differs between rows is only what the owning services hold.
fn relationship_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a standalone account, in no organization and no billing group",
            world: |_| {},
            want: vec![segment(BILLING_DOMAIN_BILLABLE, JANUARY, APRIL, None, None)],
        },
        Row {
            case: "an organization joined in the middle of the period",
            world: |engine| join_organization(engine, MANAGEMENT_ACCOUNT_ID, JOINED),
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, JOINED, None, None),
                segment(
                    BILLING_DOMAIN_BILLABLE,
                    JOINED,
                    APRIL,
                    Some(MANAGEMENT_ACCOUNT_ID),
                    None,
                ),
            ],
        },
        Row {
            case: "an organization joined before the period",
            world: |engine| join_organization(engine, MANAGEMENT_ACCOUNT_ID, at(2024, 6, 1, 0)),
            want: vec![segment(
                BILLING_DOMAIN_BILLABLE,
                JANUARY,
                APRIL,
                Some(MANAGEMENT_ACCOUNT_ID),
                None,
            )],
        },
        Row {
            case: "an organization joined after the period",
            world: |engine| join_organization(engine, MANAGEMENT_ACCOUNT_ID, at(2025, 6, 1, 0)),
            want: vec![segment(BILLING_DOMAIN_BILLABLE, JANUARY, APRIL, None, None)],
        },
        Row {
            case: "an organization the caller's own account manages",
            world: |engine| join_organization(engine, ACCOUNT_ID, JOINED),
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, JOINED, None, None),
                segment(
                    BILLING_DOMAIN_BILLABLE,
                    JOINED,
                    APRIL,
                    Some(ACCOUNT_ID),
                    None,
                ),
            ],
        },
        Row {
            case: "a billing group membership still in force",
            world: |engine| {
                join_billing_group(
                    engine,
                    "100000000001",
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                    MARCH_MONTH,
                    None,
                )
            },
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, MARCH, None, None),
                segment(
                    BILLING_DOMAIN_PRO_FORMA,
                    MARCH,
                    APRIL,
                    None,
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                ),
            ],
        },
        Row {
            case: "a billing group membership that ended inside the period",
            world: |engine| {
                join_billing_group(
                    engine,
                    "100000000002",
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                    FEBRUARY_MONTH,
                    Some(MARCH_MONTH),
                )
            },
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, FEBRUARY, None, None),
                segment(
                    BILLING_DOMAIN_PRO_FORMA,
                    FEBRUARY,
                    MARCH,
                    None,
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                ),
                segment(BILLING_DOMAIN_BILLABLE, MARCH, APRIL, None, None),
            ],
        },
        Row {
            case: "a billing group that designates no primary account",
            world: |engine| join_billing_group(engine, "100000000003", None, FEBRUARY_MONTH, None),
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, FEBRUARY, None, None),
                segment(BILLING_DOMAIN_PRO_FORMA, FEBRUARY, APRIL, None, None),
            ],
        },
        Row {
            case: "a membership that covered no billing period at all",
            world: |engine| {
                join_billing_group(
                    engine,
                    "100000000004",
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                    FEBRUARY_MONTH,
                    Some(FEBRUARY_MONTH),
                )
            },
            want: vec![segment(BILLING_DOMAIN_BILLABLE, JANUARY, APRIL, None, None)],
        },
        Row {
            case: "a membership that ended before the period",
            world: |engine| {
                join_billing_group(
                    engine,
                    "100000000005",
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                    "2024-01",
                    Some("2024-06"),
                )
            },
            want: vec![segment(BILLING_DOMAIN_BILLABLE, JANUARY, APRIL, None, None)],
        },
        Row {
            case: "two memberships in a row, of groups with different primary accounts",
            world: |engine| {
                join_billing_group(
                    engine,
                    "100000000006",
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                    FEBRUARY_MONTH,
                    Some(MARCH_MONTH),
                );
                join_billing_group(
                    engine,
                    "100000000007",
                    Some(SECOND_GROUP_PRIMARY_ACCOUNT_ID),
                    MARCH_MONTH,
                    None,
                );
            },
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, FEBRUARY, None, None),
                segment(
                    BILLING_DOMAIN_PRO_FORMA,
                    FEBRUARY,
                    MARCH,
                    None,
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                ),
                segment(
                    BILLING_DOMAIN_PRO_FORMA,
                    MARCH,
                    APRIL,
                    None,
                    Some(SECOND_GROUP_PRIMARY_ACCOUNT_ID),
                ),
            ],
        },
        Row {
            case: "both relationships, changing at different instants",
            world: both_relationships,
            want: vec![
                segment(BILLING_DOMAIN_BILLABLE, JANUARY, JOINED, None, None),
                segment(
                    BILLING_DOMAIN_BILLABLE,
                    JOINED,
                    MARCH,
                    Some(MANAGEMENT_ACCOUNT_ID),
                    None,
                ),
                segment(
                    BILLING_DOMAIN_PRO_FORMA,
                    MARCH,
                    APRIL,
                    Some(MANAGEMENT_ACCOUNT_ID),
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                ),
            ],
        },
        Row {
            case: "a membership beginning in the period's own first month",
            world: |engine| {
                join_billing_group(
                    engine,
                    "100000000008",
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                    JANUARY_MONTH,
                    None,
                )
            },
            want: vec![segment(
                BILLING_DOMAIN_PRO_FORMA,
                JANUARY,
                APRIL,
                None,
                Some(GROUP_PRIMARY_ACCOUNT_ID),
            )],
        },
    ]
}

/// The world both the period table and the pagination walk read: the account
/// joined an organization inside February and a billing group from March, so its
/// relationships changed TWICE inside `[JANUARY, APRIL)` and at instants of two
/// different kinds (an instant of the clock, and a billing-period boundary).
fn both_relationships(engine: &Arc<engine::InMemoryEngine>) {
    join_organization(engine, MANAGEMENT_ACCOUNT_ID, JOINED);
    join_billing_group(
        engine,
        "100000000009",
        Some(GROUP_PRIMARY_ACCOUNT_ID),
        MARCH_MONTH,
        None,
    );
}

/// A fresh emulator over its DEFAULT state — the one open-ended recorded
/// segment, and no organization or billing group anywhere — with `world`
/// recorded in the owning services' stores.
fn world(shape: fn(&Arc<engine::InMemoryEngine>)) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>)
{
    let (router, engine) = standalone_emulator_with_engine();
    shape(&engine);
    (router, engine)
}

#[tokio::test]
async fn a_relationship_change_answers_with_several_segments() {
    for row in relationship_rows() {
        let case = row.case;
        let (router, _engine) = world(row.world);
        let (got, token) = list(&router, case, &over(JANUARY, APRIL)).await;
        assert_eq!(
            got, row.want,
            "case {case:?}: expected {:?}, got {got:?}",
            row.want
        );
        assert_eq!(
            token, None,
            "case {case:?}: one page holds every segment, got {token:?}"
        );
    }
}

#[tokio::test]
async fn the_requested_period_selects_the_relationships_it_covers() {
    // One world, read over periods that cut it in every way: before every
    // change, straddling one, inside one period of unchanged relationships, and
    // one second on either side of the instant a relationship changed.
    let rows: Vec<(&str, i64, i64, Vec<Reported>)> = vec![
        (
            "a period entirely before every change",
            JANUARY,
            FEBRUARY,
            vec![segment(BILLING_DOMAIN_BILLABLE, JANUARY, FEBRUARY, None, None)],
        ),
        (
            "a period straddling the organization change",
            at(2025, 2, 9, 0),
            at(2025, 2, 11, 0),
            vec![
                segment(
                    BILLING_DOMAIN_BILLABLE,
                    at(2025, 2, 9, 0),
                    JOINED,
                    None,
                    None,
                ),
                segment(
                    BILLING_DOMAIN_BILLABLE,
                    JOINED,
                    at(2025, 2, 11, 0),
                    Some(MANAGEMENT_ACCOUNT_ID),
                    None,
                ),
            ],
        ),
        (
            "a period straddling the billing-period boundary the membership began at",
            at(2025, 2, 25, 0),
            at(2025, 3, 5, 0),
            vec![
                segment(
                    BILLING_DOMAIN_BILLABLE,
                    at(2025, 2, 25, 0),
                    MARCH,
                    Some(MANAGEMENT_ACCOUNT_ID),
                    None,
                ),
                segment(
                    BILLING_DOMAIN_PRO_FORMA,
                    MARCH,
                    at(2025, 3, 5, 0),
                    Some(MANAGEMENT_ACCOUNT_ID),
                    Some(GROUP_PRIMARY_ACCOUNT_ID),
                ),
            ],
        ),
        (
            "a period inside one period of unchanged relationships",
            at(2025, 3, 5, 0),
            at(2025, 3, 10, 0),
            vec![segment(
                BILLING_DOMAIN_PRO_FORMA,
                at(2025, 3, 5, 0),
                at(2025, 3, 10, 0),
                Some(MANAGEMENT_ACCOUNT_ID),
                Some(GROUP_PRIMARY_ACCOUNT_ID),
            )],
        ),
        (
            "one second at the instant the organization was joined",
            JOINED,
            JOINED + 1,
            vec![segment(
                BILLING_DOMAIN_BILLABLE,
                JOINED,
                JOINED + 1,
                Some(MANAGEMENT_ACCOUNT_ID),
                None,
            )],
        ),
        (
            "one second before it",
            JOINED - 1,
            JOINED,
            vec![segment(
                BILLING_DOMAIN_BILLABLE,
                JOINED - 1,
                JOINED,
                None,
                None,
            )],
        ),
    ];

    let (router, _engine) = world(both_relationships);
    for (case, begin, end, want) in rows {
        let (got, _) = list(&router, case, &over(begin, end)).await;
        assert_eq!(got, want, "case {case:?}: expected {want:?}, got {got:?}");
    }
}

#[tokio::test]
async fn a_record_that_states_a_relationship_itself_reports_that_one() {
    // What a record STATES, it reports: the relationships the owning services
    // hold fill only the members a record leaves absent. Both records below are
    // recorded over periods the world's own changes fall inside, so a split
    // would show if the owning service's fact overrode the record's.
    let (router, engine) = world(both_relationships);
    {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        // Replaces the account's one open-ended record (same key: a record
        // bounded on neither begin side), stating its own management account
        // over a period the organization was joined inside.
        assert!(
            state.record_billing_view_segment(BillingViewSegment {
                end_date_exclusive: Some(instant(MARCH)),
                management_account_id: Some(RECORDED_MANAGEMENT_ACCOUNT_ID.to_string()),
                ..BillingViewSegment::in_effect(PRIMARY_ARN, BILLING_DOMAIN_BILLABLE)
            }),
            "setup: the recorded segment must fit in the store"
        );
        // A second record, stating the billing group it was in over a period the
        // world's membership covers — and stating no management account, so that
        // member is still read from the organization.
        assert!(
            state.record_billing_view_segment(BillingViewSegment {
                begin_date_inclusive: Some(instant(MARCH)),
                billing_group_primary_account_id: Some(
                    RECORDED_GROUP_PRIMARY_ACCOUNT_ID.to_string()
                ),
                ..BillingViewSegment::in_effect(PRIMARY_ARN, BILLING_DOMAIN_BILLABLE)
            }),
            "setup: the second recorded segment must fit in the store"
        );
    }

    let case = "records stating their own relationships";
    let (got, _) = list(&router, case, &over(JANUARY, APRIL)).await;
    let want = vec![
        segment(
            BILLING_DOMAIN_BILLABLE,
            JANUARY,
            MARCH,
            Some(RECORDED_MANAGEMENT_ACCOUNT_ID),
            None,
        ),
        segment(
            BILLING_DOMAIN_BILLABLE,
            MARCH,
            APRIL,
            Some(MANAGEMENT_ACCOUNT_ID),
            Some(RECORDED_GROUP_PRIMARY_ACCOUNT_ID),
        ),
    ];
    assert_eq!(got, want, "case {case:?}: expected {want:?}, got {got:?}");
}

/// Epoch seconds as the instant the store holds.
fn instant(secs: i64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(secs).expect("a post-epoch instant"))
}

#[tokio::test]
async fn the_derived_segments_are_paged_oldest_first() {
    // The whole walk, and the same walk one entry at a time: the segments a
    // relationship change cut are paged like any others, so a page resumes
    // strictly after the last entry the previous one returned even when two
    // entries come from the same recorded segment.
    let (router, _engine) = world(both_relationships);
    let case = "the whole walk";
    let (every, token) = list(&router, case, &over(JANUARY, APRIL)).await;
    assert_eq!(
        (every.len(), token),
        (3, None),
        "case {case:?}: expected the three segments of this world, got {every:?}"
    );

    for page_size in [1, 2, 3] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<Reported> = Vec::new();
        let mut next: Option<String> = None;
        // One page more than the walk can hold, so a walk that never ends is a
        // failure rather than a hang.
        for _ in 0..every.len() + 1 {
            let mut request = over(JANUARY, APRIL);
            request["maxResults"] = json!(page_size);
            if let Some(token) = &next {
                request["nextToken"] = json!(token);
            }
            let (page, token) = list(&router, &case, &request).await;
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
            );
            walked.extend(page);
            next = token;
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            next, None,
            "case {case:?}: the walk ends without a token, got {next:?}"
        );
        assert_eq!(
            walked, every,
            "case {case:?}: the paged walk reports the unpaged one, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_derived_segments_records_nothing() {
    // The relationships are read off the owning stores on every read, so
    // nothing is copied here: the recorded timeline a later caller sees is the
    // one that stood before.
    let (router, engine) = world(both_relationships);
    let before = timeline(&engine);
    assert_eq!(
        before.len(),
        1,
        "setup: a fresh account's PRIMARY view has one recorded segment, got {before:?}"
    );

    for (case, request) in [
        ("the whole period", over(JANUARY, APRIL)),
        ("the default period", json!({ "arn": PRIMARY_ARN })),
        ("a period inside one segment", over(MARCH, APRIL)),
    ] {
        let (got, _) = list(&router, case, &request).await;
        assert!(
            !got.is_empty(),
            "case {case:?}: the read answers with segments, got {got:?}"
        );
        let after = timeline(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: the read records nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_answer_follows_what_the_owning_services_record() {
    // The gap this suite is about: before anything records a relationship, the
    // account's PRIMARY view answers with the one segment of a standalone
    // account; each record the owning services take then CHANGES what the very
    // same request answers.
    let (router, engine) = standalone_emulator_with_engine();
    let request = over(JANUARY, APRIL);

    let case = "before any relationship is recorded";
    let (standalone, _) = list(&router, case, &request).await;
    assert_eq!(
        standalone,
        vec![segment(BILLING_DOMAIN_BILLABLE, JANUARY, APRIL, None, None)],
        "case {case:?}: expected one BILLABLE segment naming no account, got {standalone:?}"
    );

    join_organization(&engine, MANAGEMENT_ACCOUNT_ID, JOINED);
    let case = "after the organization is recorded";
    let (in_organization, _) = list(&router, case, &request).await;
    assert_eq!(
        in_organization,
        vec![
            segment(BILLING_DOMAIN_BILLABLE, JANUARY, JOINED, None, None),
            segment(
                BILLING_DOMAIN_BILLABLE,
                JOINED,
                APRIL,
                Some(MANAGEMENT_ACCOUNT_ID),
                None,
            ),
        ],
        "case {case:?}: expected the timeline cut at the instant the account joined, \
         got {in_organization:?}"
    );

    join_billing_group(
        &engine,
        "100000000010",
        Some(GROUP_PRIMARY_ACCOUNT_ID),
        MARCH_MONTH,
        None,
    );
    let case = "after the billing group membership is recorded";
    let (in_group, _) = list(&router, case, &request).await;
    assert_eq!(
        in_group,
        vec![
            segment(BILLING_DOMAIN_BILLABLE, JANUARY, JOINED, None, None),
            segment(
                BILLING_DOMAIN_BILLABLE,
                JOINED,
                MARCH,
                Some(MANAGEMENT_ACCOUNT_ID),
                None,
            ),
            segment(
                BILLING_DOMAIN_PRO_FORMA,
                MARCH,
                APRIL,
                Some(MANAGEMENT_ACCOUNT_ID),
                Some(GROUP_PRIMARY_ACCOUNT_ID),
            ),
        ],
        "case {case:?}: expected the PRO_FORMA period the membership shapes, got {in_group:?}"
    );
}

#[tokio::test]
async fn the_default_period_reports_the_relationships_in_effect() {
    // A request naming no period is answered for "the current billing period,
    // which is the calendar month in UTC". Every relationship this world
    // records is still in force, and all of them began before that month, so the
    // month is ONE period of unchanged relationships whenever the clock reads it.
    let (router, _engine) = world(both_relationships);
    let case = "no timeRange at all";
    let (status, body) = call(&router, "ListBillingViewSegments", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let got = reported(&body);
    assert_eq!(
        got.len(),
        1,
        "case {case:?}: the current month is one unchanged period, got {got:?}"
    );
    let month = &got[0];
    assert_eq!(
        (
            month.domain.as_deref(),
            month.management_account_id.as_deref(),
            month.billing_group_primary_account_id.as_deref(),
        ),
        (
            Some(BILLING_DOMAIN_PRO_FORMA),
            Some(MANAGEMENT_ACCOUNT_ID),
            Some(GROUP_PRIMARY_ACCOUNT_ID),
        ),
        "case {case:?}: expected the relationships in force, got {month:?}"
    );
    assert!(
        month.begin < month.end,
        "case {case:?}: the reported month is a half-open period, got {month:?}"
    );
}

/// The segments the caller's PRIMARY view reports, as the one value that
/// identifies each of them — so a refusal below can be told from an answer.
async fn items(router: &MultiServiceRouter, request: &Value) -> Value {
    let (_, body) = call(router, "ListBillingViewSegments", request).await;
    body["items"].clone()
}

#[tokio::test]
async fn a_cut_point_of_one_period_is_not_a_cut_point_of_another() {
    // A reported segment's cut point is a position in ONE walk: the same recorded
    // record reports different entries under a different period, so a token of
    // one period is refused for another rather than resuming it.
    let (router, _engine) = world(both_relationships);
    let mut first = over(JANUARY, APRIL);
    first["maxResults"] = json!(1);
    let (page, token) = list(&router, "the first page", &first).await;
    assert_eq!(
        page.len(),
        1,
        "setup: the first page holds one segment, got {page:?}"
    );
    let token = token.expect("setup: a first page of three segments carries a nextToken");

    let mut other_period = over(FEBRUARY, APRIL);
    other_period["nextToken"] = json!(token);
    let (status, refused) = call(&router, "ListBillingViewSegments", &other_period).await;
    assert_eq!(
        status, 400,
        "case \"another period\": expected the token to be refused, got {status} {refused:?}"
    );
    let served = items(&router, &other_period).await;
    assert_eq!(
        served,
        Value::Null,
        "case \"another period\": a refusal reports no segments, got {served:?}"
    );
}
