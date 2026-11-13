// SPDX-License-Identifier: CC0-1.0

//! The scaling activities of the account are a stored log, and a read reports it.
//!
//! The rule under test: every capacity change of an Auto Scaling group records one
//! scaling activity per instance it launches or terminates, and
//! `DescribeScalingActivities` reports that log — the activities still in progress
//! first and the rest newest-started first, limited to the ones the request's criteria
//! (a group name, activity ids, a status, the start-time bounds, whether deleted groups
//! count) admit, one page at a time.
//!
//! The tables below span what that rule generalises over: how many instances a change
//! moves and in which direction, each criterion a request may state and each form its
//! values take, the page sizes and cursors a read pages with, every criterion the
//! service refuses, and how old an activity may be and still be reported.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{borrowed, new_engine, p, query, router_with, tag_value, tag_values, xml_unescape};

/// The launch template the groups in this suite launch their instances from.
const LAUNCH_TEMPLATE: &str = "lt-activities";

/// The zone the groups in this suite place their instances in.
const ZONE: &str = "us-west-2a";

/// The largest capacity any group in this suite is created with room for.
const MAX_SIZE: &str = "5";

/// Wait until the clock has certainly brought the lifecycle transitions one capacity
/// change started due. Derived from the service's own span, so this and the
/// implementation cannot drift apart.
async fn past_lifecycle() {
    tokio::time::sleep(autoscaling::state::LIFECYCLE_TRANSITION * 2 + Duration::from_millis(250))
        .await;
}

/// Create one group holding `capacity` instances.
async fn create_group(router: &MultiServiceRouter, name: &str, capacity: i32) {
    let capacity = capacity.to_string();
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            ("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
            ("MinSize", "0"),
            ("MaxSize", MAX_SIZE),
            ("DesiredCapacity", capacity.as_str()),
            ("AvailabilityZones.member.1", ZONE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
}

/// Set one group's desired capacity.
async fn set_capacity(router: &MultiServiceRouter, name: &str, capacity: i32) {
    let capacity = capacity.to_string();
    let (status, body) = query(
        router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", name),
            ("DesiredCapacity", capacity.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "SetDesiredCapacity {capacity} for {name} expected 200, got {status}: {body}"
    );
}

/// Read the scaling activities with the criteria `params` state.
async fn activities(router: &MultiServiceRouter, params: &[(&str, &str)]) -> (u16, String) {
    query(router, "DescribeScalingActivities", params).await
}

/// Read the scaling activities and answer the body of a call that must have succeeded.
async fn reported(router: &MultiServiceRouter, case: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = activities(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeScalingActivities expected 200, got {status}: {body}"
    );
    body
}

/// The RFC 3339 `Z` rendering of one instant, the form an ISO 8601 bound takes.
fn rendered(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("an instant renders in the date-time form")
}

/// The same instant written as a civil date-time with NO zone, which the operation
/// documents as being read as UTC.
fn rendered_without_zone(at: SystemTime) -> String {
    rendered(at).trim_end_matches('Z').to_string()
}

/// The same instant written with the numeric UTC offset `+01:00`: the civil part is an
/// hour ahead, so a reader that ignored the offset would name a different instant.
fn rendered_with_offset(at: SystemTime) -> String {
    format!(
        "{}+01:00",
        rendered_without_zone(at + Duration::from_secs(3_600))
    )
}

/// One row of the capacity-change table.
struct Change {
    case: &'static str,
    /// The capacity the group is created with.
    created: i32,
    /// The capacity a later request sets.
    then_set: i32,
}

/// Every capacity change records one activity per instance it moves, and the read
/// reports each one with the progress it has actually made.
#[tokio::test]
async fn every_capacity_change_records_an_activity_per_instance() {
    let rows = [
        Change {
            case: "a group created empty and scaled out to one instance",
            created: 0,
            then_set: 1,
        },
        Change {
            case: "a group created holding one instance and left at that capacity",
            created: 1,
            then_set: 1,
        },
        Change {
            case: "a group created holding two instances and scaled out to three",
            created: 2,
            then_set: 3,
        },
        Change {
            case: "a group created holding three instances and scaled in to one",
            created: 3,
            then_set: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: each row is its own account, so the activities of one
        // row never explain another's.
        let router = router_with(new_engine());
        let name = format!("asg-activity-{index}");
        create_group(&router, &name, row.created).await;

        let created = usize::try_from(row.created).expect("a capacity is a count");
        // Read while the clock still owes the completion of what the creation started.
        let body = reported(&router, row.case, &[("AutoScalingGroupName", name.as_str())]).await;
        let starting = tag_values(&body, "ActivityId");
        assert_eq!(
            starting.len(),
            created,
            "case {:?}: expected {created} activities for the creation, got {starting:?}",
            row.case
        );
        assert_eq!(
            starting
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            starting.len(),
            "case {:?}: each activity carries its own id, got {starting:?}",
            row.case
        );
        for status_code in tag_values(&body, "StatusCode") {
            assert_eq!(
                status_code, "InProgress",
                "case {:?}: an activity the clock still owes is in progress, got {body}",
                row.case
            );
        }
        assert!(
            tag_values(&body, "EndTime").is_empty(),
            "case {:?}: an unfinished activity reports no end time, got {body}",
            row.case
        );
        for group in tag_values(&body, "AutoScalingGroupName") {
            assert_eq!(
                group, name,
                "case {:?}: every activity belongs to the group it was read for, got {body}",
                row.case
            );
        }
        for arn in tag_values(&body, "AutoScalingGroupARN") {
            assert!(
                arn.ends_with(&format!("autoScalingGroupName/{name}")),
                "case {:?}: an activity names its group's own ARN, got {arn}",
                row.case
            );
        }
        for state in tag_values(&body, "AutoScalingGroupState") {
            assert_eq!(
                state, "InService",
                "case {:?}: the group is still held, got {body}",
                row.case
            );
        }
        let expected_cause = format!("changing the desired capacity from 0 to {}", row.created);
        for cause in tag_values(&body, "Cause") {
            assert!(
                xml_unescape(&cause).contains(&expected_cause),
                "case {:?}: expected the cause to state {expected_cause:?}, got {cause:?}",
                row.case
            );
        }

        // Once the clock has brought them due every one of them has finished.
        past_lifecycle().await;
        let body = reported(&router, row.case, &[("AutoScalingGroupName", name.as_str())]).await;
        for status_code in tag_values(&body, "StatusCode") {
            assert_eq!(
                status_code, "Successful",
                "case {:?}: a finished activity succeeded, got {body}",
                row.case
            );
        }
        assert_eq!(
            tag_values(&body, "Progress").len(),
            created,
            "case {:?}: every activity reports its progress, got {body}",
            row.case
        );
        for progress in tag_values(&body, "Progress") {
            assert_eq!(
                progress, "100",
                "case {:?}: a finished activity is complete, got {body}",
                row.case
            );
        }
        assert_eq!(
            tag_values(&body, "EndTime").len(),
            created,
            "case {:?}: every finished activity reports when it ended, got {body}",
            row.case
        );

        // The later change records one activity per instance it moves, in either
        // direction, and those are the ones still in progress — so they are reported
        // first.
        set_capacity(&router, &name, row.then_set).await;
        let moved = usize::try_from((row.then_set - row.created).abs()).expect("a count");
        let body = reported(&router, row.case, &[("AutoScalingGroupName", name.as_str())]).await;
        let after = tag_values(&body, "ActivityId");
        assert_eq!(
            after.len(),
            created + moved,
            "case {:?}: expected {} activities after the change, got {after:?}",
            row.case,
            created + moved
        );
        let codes = tag_values(&body, "StatusCode");
        assert_eq!(
            codes.iter().filter(|code| *code == "InProgress").count(),
            moved,
            "case {:?}: expected {moved} activities still in progress, got {codes:?}",
            row.case
        );
        assert!(
            codes[..moved].iter().all(|code| code == "InProgress"),
            "case {:?}: the activities still in progress are described first, got {codes:?}",
            row.case
        );
        // Every activity the creation recorded is still in the log, under the id it was
        // given.
        for id in &starting {
            assert!(
                after.contains(id),
                "case {:?}: the activity {id} must still be reported, got {after:?}",
                row.case
            );
        }

        past_lifecycle().await;
        let body = reported(&router, row.case, &[("AutoScalingGroupName", name.as_str())]).await;
        for status_code in tag_values(&body, "StatusCode") {
            assert_eq!(
                status_code, "Successful",
                "case {:?}: every activity has finished by now, got {body}",
                row.case
            );
        }
    }
}

/// The activities a read reports are the ones its criteria admit.
#[tokio::test]
async fn the_criteria_a_read_states_select_the_activities_reported() {
    let router = router_with(new_engine());
    // One group the account keeps, holding two instances, and one it deletes.
    create_group(&router, "asg-kept", 2).await;
    create_group(&router, "asg-deleted", 1).await;
    past_lifecycle().await;

    let kept = tag_values(
        &reported(&router, "the kept group", &[("AutoScalingGroupName", "asg-kept")]).await,
        "ActivityId",
    );
    assert_eq!(
        kept.len(),
        2,
        "the kept group must have recorded one activity per instance, got {kept:?}"
    );

    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-deleted"),
            ("ForceDelete", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAutoScalingGroup expected 200, got {status}: {body}"
    );
    past_lifecycle().await;

    let now = SystemTime::now();
    let hour = Duration::from_secs(3_600);
    let later = rendered(now + hour);
    let earlier = rendered(now - hour);
    let unknown_id = "00000000-0000-4000-8000-ffffffffffff";

    // One row of the criteria table: what a failing assertion calls it, the criteria the
    // request states, and how many activities it must report.
    type Row = (&'static str, Vec<(String, String)>, usize);

    let rows: Vec<Row> = vec![
        (
            "no criterion at all: every activity of the groups the account holds",
            vec![],
            2,
        ),
        (
            "the activity of a group the account deleted, asked for",
            vec![p("IncludeDeletedGroups", "true")],
            4,
        ),
        (
            "one group by name",
            vec![p("AutoScalingGroupName", "asg-kept")],
            2,
        ),
        (
            "a deleted group by name, without asking for deleted groups",
            vec![p("AutoScalingGroupName", "asg-deleted")],
            0,
        ),
        (
            "a deleted group by name, asking for deleted groups",
            vec![
                p("AutoScalingGroupName", "asg-deleted"),
                p("IncludeDeletedGroups", "true"),
            ],
            2,
        ),
        (
            "one activity by id",
            vec![p("ActivityIds.member.1", kept[0].clone())],
            1,
        ),
        (
            "an id the account does not hold beside one it does",
            vec![
                p("ActivityIds.member.1", kept[0].clone()),
                p("ActivityIds.member.2", unknown_id),
            ],
            1,
        ),
        (
            "only an id the account does not hold",
            vec![p("ActivityIds.member.1", unknown_id)],
            0,
        ),
        (
            "a lower bound later than every activity",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p("Filters.member.1.Values.member.1", later.clone()),
            ],
            0,
        ),
        (
            "a lower bound earlier than every activity",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p("Filters.member.1.Values.member.1", earlier.clone()),
            ],
            2,
        ),
        (
            "an upper bound later than every activity",
            vec![
                p("Filters.member.1.Name", "StartTimeUpperBound"),
                p("Filters.member.1.Values.member.1", later.clone()),
            ],
            2,
        ),
        (
            "an upper bound earlier than every activity",
            vec![
                p("Filters.member.1.Name", "StartTimeUpperBound"),
                p("Filters.member.1.Values.member.1", earlier.clone()),
            ],
            0,
        ),
        (
            "both bounds around every activity",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p("Filters.member.1.Values.member.1", earlier.clone()),
                p("Filters.member.2.Name", "StartTimeUpperBound"),
                p("Filters.member.2.Values.member.1", later.clone()),
            ],
            2,
        ),
        (
            "a bound stating no zone, which is read as UTC",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p(
                    "Filters.member.1.Values.member.1",
                    rendered_without_zone(now - hour),
                ),
            ],
            2,
        ),
        (
            "a bound carrying a numeric UTC offset",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p(
                    "Filters.member.1.Values.member.1",
                    rendered_with_offset(now - hour),
                ),
            ],
            2,
        ),
        (
            "two values of one bound widen it",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p("Filters.member.1.Values.member.1", later.clone()),
                p("Filters.member.1.Values.member.2", earlier.clone()),
            ],
            2,
        ),
        (
            "two bounds of the same kind tighten each other",
            vec![
                p("Filters.member.1.Name", "StartTimeLowerBound"),
                p("Filters.member.1.Values.member.1", earlier.clone()),
                p("Filters.member.2.Name", "StartTimeLowerBound"),
                p("Filters.member.2.Values.member.1", later.clone()),
            ],
            0,
        ),
        (
            "the status the activities are in, with the group they belong to",
            vec![
                p("AutoScalingGroupName", "asg-kept"),
                p("Filters.member.1.Name", "Status"),
                p("Filters.member.1.Values.member.1", "Successful"),
            ],
            2,
        ),
        (
            "a status none of them is in",
            vec![
                p("AutoScalingGroupName", "asg-kept"),
                p("Filters.member.1.Name", "Status"),
                p("Filters.member.1.Values.member.1", "Failed"),
            ],
            0,
        ),
        (
            "a status filter carrying the status they are in among others",
            vec![
                p("AutoScalingGroupName", "asg-kept"),
                p("Filters.member.1.Name", "Status"),
                p("Filters.member.1.Values.member.1", "Cancelled"),
                p("Filters.member.1.Values.member.2", "Successful"),
            ],
            2,
        ),
    ];

    for (case, params, expected) in rows {
        let body = reported(&router, case, &borrowed(&params)).await;
        let ids = tag_values(&body, "ActivityId");
        assert_eq!(
            ids.len(),
            expected,
            "case {case:?}: expected {expected} activities, got {ids:?} from {body}"
        );
    }

    // A group the account no longer holds is reported as deleted on every activity of
    // its own, whatever the state recorded when the activity was written.
    let body = reported(
        &router,
        "the state of a deleted group",
        &[
            ("AutoScalingGroupName", "asg-deleted"),
            ("IncludeDeletedGroups", "true"),
        ],
    )
    .await;
    let states = tag_values(&body, "AutoScalingGroupState");
    assert_eq!(
        states,
        vec!["Deleted".to_string(), "Deleted".to_string()],
        "a deleted group's activities report it deleted, got {body}"
    );
}

/// The activities are reported one page at a time, and a cursor this service never
/// issued names no page to resume from.
#[tokio::test]
async fn the_activities_are_paged_and_an_unissued_cursor_is_refused() {
    let router = router_with(new_engine());
    create_group(&router, "asg-paged", 3).await;
    past_lifecycle().await;

    let body = reported(&router, "every activity", &[]).await;
    let all = tag_values(&body, "ActivityId");
    assert_eq!(
        all.len(),
        3,
        "the group must have recorded one activity per instance, got {all:?}"
    );
    assert!(
        tag_value(&body, "NextToken").is_empty(),
        "a page holding everything names no next one, got {body}"
    );

    // One page at a time, in the order the unpaged read reported them.
    let mut paged = Vec::new();
    let mut token = String::new();
    for page in 0..3 {
        let mut params = vec![p("MaxRecords", "1")];
        if !token.is_empty() {
            params.push(p("NextToken", token.clone()));
        }
        let body = reported(&router, "one page", &borrowed(&params)).await;
        let ids = tag_values(&body, "ActivityId");
        assert_eq!(
            ids.len(),
            1,
            "page {page}: a page of one holds one activity, got {body}"
        );
        paged.extend(ids);
        token = tag_value(&body, "NextToken");
        let expected_more = page < 2;
        assert_eq!(
            !token.is_empty(),
            expected_more,
            "page {page}: expected a further page: {expected_more}, got {body}"
        );
    }
    assert_eq!(
        paged, all,
        "the pages together hold the activities in the order the unpaged read reports them"
    );

    // A cursor this service never issued, and one that points past the end of the list
    // it would resume: neither names a page.
    for token in [
        "not-a-token-this-service-issued",
        "",
        &autoscaling::state::encode_next_token(9),
    ] {
        let (status, body) = activities(&router, &[("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "InvalidNextToken",
            "case {token:?}: expected the InvalidNextToken code, got {body}"
        );
        assert_eq!(
            xml_unescape(&tag_value(&body, "Message")),
            autoscaling::state::invalid_next_token_message(token),
            "case {token:?}: the refusal must quote the token the caller sent, got {body}"
        );
    }
}

/// One row of the refusal table: the criteria the request states, and the message the
/// service refuses it with — or `None` for a request it accepts.
struct Refusal {
    case: &'static str,
    params: Vec<(String, String)>,
    message: Option<String>,
}

/// The criteria the service refuses, and the neighbouring ones it accepts.
#[tokio::test]
async fn the_criteria_a_read_refuses() {
    let router = router_with(new_engine());
    create_group(&router, "asg-refusals", 0).await;

    let ids = |count: usize| -> Vec<(String, String)> {
        (1..=count)
            .map(|index| p(format!("ActivityIds.member.{index}").as_str(), format!("id-{index}")))
            .collect()
    };

    let rows = vec![
        Refusal {
            case: "no criterion at all",
            params: vec![],
            message: None,
        },
        Refusal {
            case: "a filter carrying no name",
            params: vec![p("Filters.member.1.Values.member.1", "Successful")],
            message: Some(
                "The filter criteria isn't valid. The Name attribute is required to specify \
                 the type of filter being applied."
                    .to_string(),
            ),
        },
        Refusal {
            case: "a filter naming no documented type",
            params: vec![
                p("Filters.member.1.Name", "StartTime"),
                p("Filters.member.1.Values.member.1", "Successful"),
            ],
            message: Some(
                "The filter criteria isn't valid. Valid filter types for the Name attribute \
                 of filters are: StartTimeLowerBound, StartTimeUpperBound and Status."
                    .to_string(),
            ),
        },
        Refusal {
            case: "a documented filter carrying no value",
            params: vec![p("Filters.member.1.Name", "StartTimeLowerBound")],
            message: Some(
                "The filter criteria isn't valid. The Values attribute and one or more \
                 filter values are required."
                    .to_string(),
            ),
        },
        Refusal {
            case: "the status filter without the group it may only be used with",
            params: vec![
                p("Filters.member.1.Name", "Status"),
                p("Filters.member.1.Values.member.1", "Successful"),
            ],
            message: Some(
                "The filter criteria isn't valid. The Status filter can only be used in \
                 combination with the AutoScalingGroupName parameter."
                    .to_string(),
            ),
        },
        Refusal {
            case: "the status filter with the group it may be used with",
            params: vec![
                p("AutoScalingGroupName", "asg-refusals"),
                p("Filters.member.1.Name", "Status"),
                p("Filters.member.1.Values.member.1", "Successful"),
            ],
            message: None,
        },
        Refusal {
            case: "a status no activity can be in",
            params: vec![
                p("AutoScalingGroupName", "asg-refusals"),
                p("Filters.member.1.Name", "Status"),
                p("Filters.member.1.Values.member.1", "Finished"),
            ],
            message: Some(
                "The filter criteria isn't valid. 'Finished' isn't a valid value for the \
                 Status filter."
                    .to_string(),
            ),
        },
        Refusal {
            case: "a start-time bound that names no instant",
            params: vec![
                p("Filters.member.1.Name", "StartTimeUpperBound"),
                p("Filters.member.1.Values.member.1", "yesterday"),
            ],
            message: Some(
                "The filter criteria isn't valid. The StartTimeUpperBound filter value \
                 'yesterday' isn't an ISO 8601 formatted timestamp."
                    .to_string(),
            ),
        },
        Refusal {
            case: "a page smaller than one item",
            params: vec![p("MaxRecords", "0")],
            message: Some(autoscaling::state::max_records_violation_message(0)),
        },
        Refusal {
            case: "a page at the largest size the operation documents",
            params: vec![p("MaxRecords", "100")],
            message: None,
        },
        Refusal {
            case: "a page one past the largest size the operation documents",
            params: vec![p("MaxRecords", "101")],
            message: Some(autoscaling::state::max_records_violation_message(101)),
        },
        Refusal {
            case: "the most activity ids one call may name",
            params: ids(autoscaling::state::MAX_ACTIVITY_IDS_PER_CALL),
            message: None,
        },
        Refusal {
            case: "one activity id more than a call may name",
            params: ids(autoscaling::state::MAX_ACTIVITY_IDS_PER_CALL + 1),
            message: Some(autoscaling::state::too_many_activity_ids_message(
                autoscaling::state::MAX_ACTIVITY_IDS_PER_CALL + 1,
            )),
        },
        Refusal {
            case: "a group the account has never held",
            params: vec![p("AutoScalingGroupName", "asg-never-existed")],
            message: Some(autoscaling::state::absent_group_message("asg-never-existed")),
        },
        Refusal {
            case: "a group the account holds",
            params: vec![p("AutoScalingGroupName", "asg-refusals")],
            message: None,
        },
    ];

    for row in rows {
        let (status, body) = activities(&router, &borrowed(&row.params)).await;
        match &row.message {
            None => assert_eq!(
                status, 200,
                "case {:?}: expected the read to be accepted, got {status}: {body}",
                row.case
            ),
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected the read to be refused, got {status}: {body}",
                    row.case
                );
                assert_eq!(
                    tag_value(&body, "Code"),
                    "ValidationError",
                    "case {:?}: expected the ValidationError code, got {body}",
                    row.case
                );
                assert_eq!(
                    &xml_unescape(&tag_value(&body, "Message")),
                    message,
                    "case {:?}: expected the message {message:?}, got {body}",
                    row.case
                );
            }
        }
    }
}

/// Store one activity of a known age directly, so the window a read applies can be
/// exercised on an activity older than this process.
fn seed_activity(
    engine: &Arc<engine::InMemoryEngine>,
    group: &str,
    activity_id: &str,
    started: SystemTime,
) {
    let state = autoscaling::state::service_state(&**engine)
        .expect("the Auto Scaling store is mounted on this engine");
    state
        .lock()
        .activities
        .push(autoscaling::state::StoredActivity {
            activity_id: activity_id.to_string(),
            auto_scaling_group_name: group.to_string(),
            auto_scaling_group_arn: None,
            description: Some("Launching a new EC2 instance".to_string()),
            cause: "the suite recorded an activity of a known age".to_string(),
            start_time: started,
            end_time: Some(started),
            status_code: autoscaling::state::ACTIVITY_SUCCESSFUL.to_string(),
            status_message: None,
            progress: 100,
            details: None,
            auto_scaling_group_state: Some(
                autoscaling::state::GROUP_STATE_IN_SERVICE.to_string(),
            ),
            completes_at: None,
        });
}

/// Only an activity started inside the window the operation documents is reported.
#[tokio::test]
async fn an_activity_older_than_the_window_is_not_reported() {
    let window = autoscaling::state::ACTIVITY_WINDOW;
    // (case, how long before now the activity started, whether it is reported)
    let rows = [
        ("an activity started now", Duration::ZERO, true),
        (
            "an activity started a minute inside the window",
            window - Duration::from_secs(60),
            true,
        ),
        (
            "an activity started a second past the window",
            window + Duration::from_secs(1),
            false,
        ),
        (
            "an activity twice as old as the window",
            window * 2,
            false,
        ),
    ];

    for (case, age, is_reported) in rows {
        // One emulator per row: the activity seeded for one row is not the next row's.
        let engine = new_engine();
        let router = router_with(Arc::clone(&engine));
        create_group(&router, "asg-window", 0).await;
        let started = SystemTime::now()
            .checked_sub(age)
            .expect("an age inside the span of the clock");
        seed_activity(&engine, "asg-window", "seeded-activity", started);

        let body = reported(&router, case, &[("AutoScalingGroupName", "asg-window")]).await;
        let ids = tag_values(&body, "ActivityId");
        assert_eq!(
            ids.contains(&"seeded-activity".to_string()),
            is_reported,
            "case {case:?}: expected reported: {is_reported}, got {ids:?} from {body}"
        );
    }
}
