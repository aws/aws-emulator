// SPDX-License-Identifier: CC0-1.0

//! The scheduled-action family — `CreateScheduledAction`,
//! `DescribeScheduledActions`, `ModifyScheduledAction` and
//! `DeleteScheduledAction` — end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! Three rules are under test, and each row is one member of the class the rule
//! generalises over rather than one recorded request:
//!
//! 1. A scheduled action is a RECORD: what a create stores is what a read
//!    reports and what a modify changes, and a delete leaves nothing to read.
//! 2. A `Schedule` is judged BEFORE anything else and against the two published
//!    expression types: an expression of neither type is a parameter value the
//!    service cannot honour (`InvalidParameterValue`), while one of a published
//!    type that states no schedule earns the fault the model words for that
//!    (`InvalidSchedule`).
//! 3. A name this account holds no action under is `ScheduledActionNotFound`
//!    (400) for every operation that names one.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// A role ARN a scheduled action assumes. An IAM role is a resource of another
/// service, which this one does not resolve, so the value is only carried.
const SCHEDULER_ROLE: &str = "arn:aws:iam::123456789012:role/RedshiftScheduler";

/// A cron expression of the format `ScheduledAction$Schedule` documents —
/// "cron(Minutes Hours Day-of-month Month Day-of-week Year)", whose own example
/// is `cron(0 10 ? * MON *)`.
const WEEKLY_CRON: &str = "cron(0 10 ? * MON *)";

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Create one cluster and wait for it to be serving, by POLLING the read.
async fn available_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    emulator
        .poll_until(
            &format!("seed cluster {identifier}"),
            "DescribeClusters",
            &[("ClusterIdentifier", identifier)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
}

/// One `CreateScheduledAction` naming a `PauseCluster` target action.
async fn create_pause_action(
    emulator: &Emulator,
    name: &str,
    cluster: &str,
    schedule: &str,
) -> Answer {
    let params: Vec<(String, String)> = vec![
        ("ScheduledActionName".to_string(), name.to_string()),
        ("Schedule".to_string(), schedule.to_string()),
        ("IamRole".to_string(), SCHEDULER_ROLE.to_string()),
        (
            "TargetAction.PauseCluster.ClusterIdentifier".to_string(),
            cluster.to_string(),
        ),
    ];
    emulator
        .call("CreateScheduledAction", &borrowed(&params))
        .await
}

/// `DescribeScheduledActions` for one action by name.
async fn describe_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DescribeScheduledActions",
            &[("ScheduledActionName", name)],
        )
        .await
}

/// One row of the round-trip table: the case name, the action's name, the target
/// action's parameters, the schedule it runs on, and the operation it triggers.
type ActionCase = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    &'static str,
    &'static str,
);

#[tokio::test]
async fn a_scheduled_action_is_stored_read_back_modified_and_deleted() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "scheduler-cluster").await;

    // The rows differ along what the rule generalises over: which operation the
    // action triggers, and the form of the schedule it triggers it on.
    let rows: Vec<ActionCase> = vec![
        (
            "a recurring pause",
            "nightly-pause",
            vec![(
                "TargetAction.PauseCluster.ClusterIdentifier".to_string(),
                "scheduler-cluster".to_string(),
            )],
            WEEKLY_CRON,
            "PauseCluster",
        ),
        (
            "a one-time resume",
            "one-off-resume",
            vec![(
                "TargetAction.ResumeCluster.ClusterIdentifier".to_string(),
                "scheduler-cluster".to_string(),
            )],
            // The other published expression type: "format of at expressions is
            // at(yyyy-mm-ddThh:mm:ss)".
            "at(2099-03-04T17:27:00)",
            "ResumeCluster",
        ),
        (
            "a recurring resize",
            "quarterly-resize",
            vec![
                (
                    "TargetAction.ResizeCluster.ClusterIdentifier".to_string(),
                    "scheduler-cluster".to_string(),
                ),
                (
                    "TargetAction.ResizeCluster.NumberOfNodes".to_string(),
                    "4".to_string(),
                ),
            ],
            // The shortest cron expression the service's own examples write.
            "cron(30 12 *)",
            "ResizeCluster",
        ),
    ];

    for (case, name, target, schedule, kind) in rows {
        let mut params: Vec<(String, String)> = vec![
            ("ScheduledActionName".to_string(), name.to_string()),
            ("Schedule".to_string(), schedule.to_string()),
            ("IamRole".to_string(), SCHEDULER_ROLE.to_string()),
            (
                "ScheduledActionDescription".to_string(),
                format!("{case} of scheduler-cluster"),
            ),
        ];
        params.extend(target);
        let created = emulator
            .call("CreateScheduledAction", &borrowed(&params))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            (
                created.text("ScheduledActionName"),
                created.text("Schedule"),
                created.text("IamRole"),
                created.text("State"),
            ),
            (
                Some(name.to_string()),
                Some(schedule.to_string()),
                Some(SCHEDULER_ROLE.to_string()),
                // No `Enable` was named, so the action is the one the caller
                // asked to be scheduled: it triggers.
                Some(state::SCHEDULED_ACTION_STATE_ACTIVE.to_string()),
            ),
            "case {case}: expected the create to report what it stored, got body {}",
            created.body
        );
        // An active action whose schedule still has instants ahead of it reports
        // when it will next run. The list's entries travel as
        // `ScheduledActionTime`, which is the list member's own element name.
        assert!(
            !all_text(&created.body, "ScheduledActionTime").is_empty(),
            "case {case}: expected an upcoming invocation, got body {}",
            created.body
        );

        // ROUND TRIP: the record a read reports is the one the create stored.
        let read = describe_one(&emulator, name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read to answer 200, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            (
                read.text("ScheduledActionName"),
                read.text("Schedule"),
                read.text("ScheduledActionDescription"),
            ),
            (
                Some(name.to_string()),
                Some(schedule.to_string()),
                Some(format!("{case} of scheduler-cluster")),
            ),
            "case {case}: expected the read to report the stored action, got body {}",
            read.body
        );
        // The target action travels back whole, so the read says which
        // operation the action triggers.
        assert!(
            read.body.contains(&format!("<{kind}>")),
            "case {case}: expected the read to report the {kind} target action, got body {}",
            read.body
        );

        // A modify changes the part it names and NO other.
        let disabled = emulator
            .call(
                "ModifyScheduledAction",
                &[("ScheduledActionName", name), ("Enable", "false")],
            )
            .await;
        assert_eq!(
            (disabled.status, disabled.text("State")),
            (
                200,
                Some(state::SCHEDULED_ACTION_STATE_DISABLED.to_string())
            ),
            "case {case}: expected the modify to disable the action, got {} with body {}",
            disabled.status,
            disabled.body
        );
        assert_eq!(
            disabled.text("Schedule"),
            Some(schedule.to_string()),
            "case {case}: expected the schedule to survive a modify that did not name one, got \
             {:?}",
            disabled.text("Schedule")
        );
        // "If false, the scheduled action does not trigger", so a disabled
        // action has no invocation ahead of it.
        assert!(
            all_text(&disabled.body, "ScheduledActionTime").is_empty(),
            "case {case}: expected a disabled action to report no upcoming invocation, got body {}",
            disabled.body
        );
        let after_modify = describe_one(&emulator, name).await;
        assert_eq!(
            after_modify.text("State"),
            Some(state::SCHEDULED_ACTION_STATE_DISABLED.to_string()),
            "case {case}: expected a later read to report the modify, got {:?}",
            after_modify.text("State")
        );

        // And a delete leaves nothing to read.
        let deleted = emulator
            .call("DeleteScheduledAction", &[("ScheduledActionName", name)])
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected the delete to answer 200, got {} with body {}",
            deleted.status, deleted.body
        );
        let gone = describe_one(&emulator, name).await;
        assert_eq!(
            (gone.status, gone.error_code().as_deref()),
            (400, Some("ScheduledActionNotFound")),
            "case {case}: expected the deleted action to be gone, got {} / {:?} with body {}",
            gone.status,
            gone.error_code(),
            gone.body
        );
    }
}

#[tokio::test]
async fn a_schedule_expression_outside_the_two_published_types_is_refused_as_a_bad_parameter() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "schedule-cluster").await;

    // The class: expressions of NEITHER published type. A rate expression is the
    // sharpest row — it is a published type for a snapshot SCHEDULE and not for a
    // scheduled ACTION, whose two types are `at` and `cron`.
    let rows: Vec<(&str, &str)> = vec![
        ("prose rather than an expression", "every 5 minutes"),
        ("a rate expression, which this member does not publish", "rate(12 hours)"),
        ("an empty schedule", ""),
        ("a bare cron body with no expression type", "0 10 ? * MON *"),
        ("an expression type nothing publishes", "weekly(MON)"),
    ];

    for (case, schedule) in rows {
        let refused =
            create_pause_action(&emulator, "bad-schedule", "schedule-cluster", schedule).await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {case:?}: expected 400 InvalidParameterValue, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        let message = refused.text("Message").unwrap_or_default();
        assert!(
            message.contains("Schedule") && message.contains("cron(") && message.contains("at("),
            "case {case:?}: expected a refusal naming the member and both published formats, \
             got {message:?}"
        );
        // Nothing was stored: the schedule is judged before the name is.
        let read = describe_one(&emulator, "bad-schedule").await;
        assert_eq!(
            (read.status, read.error_code().as_deref()),
            (400, Some("ScheduledActionNotFound")),
            "case {case:?}: expected the refused create to store nothing, got {} / {:?}",
            read.status,
            read.error_code()
        );
    }
}

#[tokio::test]
async fn a_schedule_of_a_published_type_that_states_no_schedule_is_refused_as_invalid() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "invalid-schedule-cluster").await;

    // The class: expressions written in one of the two published types whose
    // body states no schedule — too few fields, a field outside its range, and
    // an instant that is not one.
    let rows: Vec<(&str, &str)> = vec![
        ("a cron expression with no fields", "cron()"),
        ("a cron expression with too few fields", "cron(0 10)"),
        ("a cron expression with too many fields", "cron(0 10 ? * MON * *)"),
        ("a cron field outside its range", "cron(99 99 ? * MON *)"),
        ("an at expression carrying no instant", "at()"),
        ("an at expression carrying something else", "at(not-an-instant)"),
        ("an at instant in another format", "at(04/03/2099 17:27)"),
    ];

    for (case, schedule) in rows {
        let refused = create_pause_action(
            &emulator,
            "invalid-schedule",
            "invalid-schedule-cluster",
            schedule,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (400, Some("InvalidSchedule")),
            "case {case:?}: expected 400 InvalidSchedule, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused
                .text("Message")
                .is_some_and(|message| message.contains(schedule)),
            "case {case:?}: expected the refusal to name the schedule it is about, got {:?}",
            refused.text("Message")
        );
    }
}

#[tokio::test]
async fn an_action_this_account_does_not_hold_is_not_found_for_every_operation_naming_one() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "notfound-cluster").await;

    // One action that DOES exist, so a refusal below cannot be an "everything is
    // refused" artefact, and one created-then-deleted name.
    let present = create_pause_action(&emulator, "present-action", "notfound-cluster", WEEKLY_CRON)
        .await;
    assert_eq!(
        present.status, 200,
        "case seed the present action: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = create_pause_action(&emulator, "doomed-action", "notfound-cluster", WEEKLY_CRON)
        .await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed action: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let deleted = emulator
        .call(
            "DeleteScheduledAction",
            &[("ScheduledActionName", "doomed-action")],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the doomed action: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // The class is the OPERATION crossed with the name: every operation that
    // names one action answers the same code, status and sentence.
    let names: Vec<(&str, &str)> = vec![
        ("a name nothing created", "no-such-action"),
        ("a name one character off a stored one", "present-actio"),
        ("a name in another case", "Present-Action"),
        ("a created-then-deleted name", "doomed-action"),
    ];

    for (case, name) in names {
        for action in [
            "DescribeScheduledActions",
            "ModifyScheduledAction",
            "DeleteScheduledAction",
        ] {
            let refused = emulator
                .call(action, &[("ScheduledActionName", name)])
                .await;
            assert_eq!(
                (refused.status, refused.error_code().as_deref()),
                (400, Some("ScheduledActionNotFound")),
                "case {case:?} on {action}: expected 400 ScheduledActionNotFound, got {} / {:?} \
                 with body {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let expected = format!("ScheduledAction not found: {name}");
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?} on {action}: expected the message {expected:?}, got {:?}",
                refused.text("Message")
            );
        }
    }

    // The stored action is still there: the rows above refused the NAME, not the
    // operations.
    let read = describe_one(&emulator, "present-action").await;
    assert_eq!(
        read.status, 200,
        "case the stored action survives: expected 200, got {} with body {}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_create_naming_no_cluster_or_no_single_target_action_is_refused() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "target-cluster").await;

    // A target action naming a cluster this account does not hold.
    let missing_cluster =
        create_pause_action(&emulator, "no-cluster-action", "no-such-cluster", WEEKLY_CRON).await;
    assert_eq!(
        (
            missing_cluster.status,
            missing_cluster.error_code().as_deref()
        ),
        (404, Some("ClusterNotFound")),
        "case a target action naming no cluster of this account: expected 404 ClusterNotFound, \
         got {} / {:?} with body {}",
        missing_cluster.status,
        missing_cluster.error_code(),
        missing_cluster.body
    );
    assert_eq!(
        missing_cluster.text("Message").as_deref(),
        Some("Cluster no-such-cluster not found."),
        "case a target action naming no cluster of this account: expected the identifier in the \
         sentence, got {:?}",
        missing_cluster.text("Message")
    );

    // A `TargetAction` naming two operations, and one naming three: an action
    // triggers exactly ONE of the operations the scheduler supports, so a
    // request naming more states no action to run.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "a target action naming two operations",
            vec![
                (
                    "TargetAction.PauseCluster.ClusterIdentifier".to_string(),
                    "target-cluster".to_string(),
                ),
                (
                    "TargetAction.ResumeCluster.ClusterIdentifier".to_string(),
                    "target-cluster".to_string(),
                ),
            ],
        ),
        (
            "a target action naming every operation",
            vec![
                (
                    "TargetAction.PauseCluster.ClusterIdentifier".to_string(),
                    "target-cluster".to_string(),
                ),
                (
                    "TargetAction.ResumeCluster.ClusterIdentifier".to_string(),
                    "target-cluster".to_string(),
                ),
                (
                    "TargetAction.ResizeCluster.ClusterIdentifier".to_string(),
                    "target-cluster".to_string(),
                ),
            ],
        ),
    ];

    for (case, target) in rows {
        let mut params: Vec<(String, String)> = vec![
            (
                "ScheduledActionName".to_string(),
                "bad-target".to_string(),
            ),
            ("Schedule".to_string(), WEEKLY_CRON.to_string()),
            ("IamRole".to_string(), SCHEDULER_ROLE.to_string()),
        ];
        params.extend(target);
        let refused = emulator
            .call("CreateScheduledAction", &borrowed(&params))
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_deref()),
            (400, Some("InvalidScheduledAction")),
            "case {case:?}: expected 400 InvalidScheduledAction, got {} / {:?} with body {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused
                .text("Message")
                .is_some_and(|message| message.contains("TargetAction")),
            "case {case:?}: expected the refusal to name the member, got {:?}",
            refused.text("Message")
        );
    }

    // A request naming no `TargetAction` at all never reaches the handler: the
    // member's own `@required` trait is what refuses it, at the boundary.
    let silent = emulator
        .call(
            "CreateScheduledAction",
            &[
                ("ScheduledActionName", "no-target"),
                ("Schedule", WEEKLY_CRON),
                ("IamRole", SCHEDULER_ROLE),
            ],
        )
        .await;
    assert_eq!(
        (silent.status, silent.error_code().as_deref()),
        (400, Some("MalformedInput")),
        "case a request naming no target action: expected the boundary to refuse it, got {} / \
         {:?} with body {}",
        silent.status,
        silent.error_code(),
        silent.body
    );
}

#[tokio::test]
async fn a_second_action_under_one_name_is_refused_and_the_first_is_unchanged() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "unique-cluster").await;

    let first =
        create_pause_action(&emulator, "unique-action", "unique-cluster", WEEKLY_CRON).await;
    assert_eq!(
        first.status, 200,
        "case the first create: expected 200, got {} with body {}",
        first.status, first.body
    );
    let second = create_pause_action(
        &emulator,
        "unique-action",
        "unique-cluster",
        "at(2099-01-01T00:00:00)",
    )
    .await;
    assert_eq!(
        (second.status, second.error_code().as_deref()),
        (400, Some("ScheduledActionAlreadyExists")),
        "case a second create under one name: expected 400 ScheduledActionAlreadyExists, got {} / \
         {:?} with body {}",
        second.status,
        second.error_code(),
        second.body
    );
    let read = describe_one(&emulator, "unique-action").await;
    assert_eq!(
        read.text("Schedule"),
        Some(WEEKLY_CRON.to_string()),
        "case a second create under one name: expected the first action untouched, got {:?}",
        read.text("Schedule")
    );
}

/// One row of the filter table: the case name, the members the read carries, and
/// the actions it must report, in order.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<&'static str>,
);

#[tokio::test]
async fn a_read_narrows_the_account_s_actions_by_every_member_it_carries() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "filter-cluster-one").await;
    available_cluster(&emulator, "filter-cluster-two").await;

    // Three actions spanning what the filters select on: the operation the
    // action triggers, the cluster it runs against, and whether it triggers.
    for (name, cluster, member, enable) in [
        (
            "filter-pause",
            "filter-cluster-one",
            "TargetAction.PauseCluster.ClusterIdentifier",
            "true",
        ),
        (
            "filter-resume",
            "filter-cluster-two",
            "TargetAction.ResumeCluster.ClusterIdentifier",
            "true",
        ),
        (
            "filter-disabled-pause",
            "filter-cluster-two",
            "TargetAction.PauseCluster.ClusterIdentifier",
            "false",
        ),
    ] {
        let created = emulator
            .call(
                "CreateScheduledAction",
                &[
                    ("ScheduledActionName", name),
                    ("Schedule", WEEKLY_CRON),
                    ("IamRole", SCHEDULER_ROLE),
                    (member, cluster),
                    ("Enable", enable),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case seed {name}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }

    // Each row is one member of the request narrowing the same list.
    let rows: Vec<FilterCase> = vec![
        (
            "no member at all reports every action, in creation order",
            vec![],
            vec!["filter-pause", "filter-resume", "filter-disabled-pause"],
        ),
        (
            "the operation the action triggers",
            vec![("TargetActionType", "ResumeCluster")],
            vec!["filter-resume"],
        ),
        (
            "only the actions that trigger",
            vec![("Active", "true")],
            vec!["filter-pause", "filter-resume"],
        ),
        (
            "only the actions that do not",
            vec![("Active", "false")],
            vec!["filter-disabled-pause"],
        ),
        (
            "the cluster the action runs against",
            vec![
                ("Filters.ScheduledActionFilter.1.Name", "cluster-identifier"),
                (
                    "Filters.ScheduledActionFilter.1.Values.item.1",
                    "filter-cluster-two",
                ),
            ],
            vec!["filter-resume", "filter-disabled-pause"],
        ),
        (
            "the role the action assumes",
            vec![
                ("Filters.ScheduledActionFilter.1.Name", "iam-role"),
                ("Filters.ScheduledActionFilter.1.Values.item.1", SCHEDULER_ROLE),
            ],
            vec!["filter-pause", "filter-resume", "filter-disabled-pause"],
        ),
        (
            "a role no action assumes",
            vec![
                ("Filters.ScheduledActionFilter.1.Name", "iam-role"),
                (
                    "Filters.ScheduledActionFilter.1.Values.item.1",
                    "arn:aws:iam::123456789012:role/Nobody",
                ),
            ],
            vec![],
        ),
        (
            "two members together",
            vec![
                ("TargetActionType", "PauseCluster"),
                ("Active", "true"),
            ],
            vec!["filter-pause"],
        ),
    ];

    for (case, params, expected) in rows {
        let read = emulator.call("DescribeScheduledActions", &params).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            read.status, read.body
        );
        let reported = all_text(&read.body, "ScheduledActionName");
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected {expected:?}, got {reported:?} with body {}",
            read.body
        );
    }
}
