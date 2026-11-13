// SPDX-License-Identifier: CC0-1.0

//! The TIMELINE a stack operation's per-resource events carry, end to end over the awsQuery
//! wire.
//!
//! The rule under test: **each transition a stack operation makes a resource go through is
//! recorded at the instant that transition happened** — a resource's `*_IN_PROGRESS` event
//! at the instant the operation SCHEDULED it and its terminal event at the instant it
//! FINISHED (`StackEvent$Timestamp`: "Time the status was updated") — so the event log of
//! one operation is the walk over the template's dependency graph:
//!
//! * resources with no dependency between them are scheduled together, so their
//!   `[scheduled, finished]` intervals OVERLAP;
//! * a resource that depends on another is scheduled at or after that one finished, and a
//!   join at or after the last of its predecessors;
//! * a resource that genuinely takes time — an `AWS::CloudFormation::WaitCondition` held
//!   unsignalled — finishes when its signals arrive, so its interval is as long as the wait
//!   and nothing behind it is scheduled, or even reported, before that;
//! * a DELETION walks the same graph backwards: a dependent is gone before its dependency's
//!   deletion starts, and independent branches come down together.
//!
//! Without that, a client reading `DescribeStackEvents` to see what a deploy is doing, how
//! long a resource took, which resource is blocking it or in what order a teardown
//! proceeded gets a log in which everything happened at once.
//!
//! Every case drives the real request path through the router `stack_lifecycle.rs` builds,
//! so what is asserted is what a client calling `cloudformation describe-stack-events`
//! sees. The table spans the graph shapes the rule generalises over — independent
//! resources, a chain, a diamond with a join, a dependency stated by `DependsOn` and one
//! stated by a `Ref`, both template spellings, and resource types the stack really creates
//! beside ones it records from the template alone — and the second case is the one the rule
//! is hardest on: a graph held open at two wait conditions and then torn down.

use std::time::Duration;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};
use crate::stack_resources::element;

use smithy_server_runtime::MultiServiceRouter;

/// How long the wait-condition case holds its gates before signalling them.
///
/// Long enough that the overlap it measures cannot be an artifact of the emulator's own
/// per-resource spans (which fit inside one stack transition), short enough for a test.
const HOLD: Duration = Duration::from_millis(1_000);

/// The overlap the two gates held [`HOLD`] apart must show at least.
///
/// Half the hold: the hold is a floor on the real overlap (the create starts before the
/// sleep and the signals arrive after it), so a loaded machine can only make the measured
/// overlap larger.
const OVERLAP_FLOOR: Duration = Duration::from_millis(500);

/// One `StackEvent` as the wire reports it, reduced to what a timeline is read from.
#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub(crate) logical_id: String,
    pub(crate) status: String,
    pub(crate) timestamp: String,
}

/// Every event a stack has recorded, newest first as `DescribeStackEvents` answers them.
pub(crate) async fn events_of(router: &MultiServiceRouter, name_or_id: &str) -> Vec<Event> {
    let (status_code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", name_or_id)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackEvents({name_or_id}) must answer 200, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| Event {
            logical_id: element(entry, "LogicalResourceId").unwrap_or_default(),
            status: element(entry, "ResourceStatus").unwrap_or_default(),
            timestamp: element(entry, "Timestamp").unwrap_or_default(),
        })
        .collect()
}

/// The instant one event's `Timestamp` names, in microseconds since the epoch.
///
/// `StackEvent$Timestamp` goes on the wire as an ISO 8601 date-time, read back here with the
/// same parser the crate itself uses for one (`aws_smithy_types::DateTime`), so what the
/// cases compare is the instant the emulator published rather than text.
fn micros(case: &str, event: &Event) -> i128 {
    let instant = ::aws_smithy_types::DateTime::from_str(
        &event.timestamp,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected an ISO 8601 Timestamp, got {:?} ({error})",
            event.timestamp
        )
    });
    i128::from(instant.secs()) * 1_000_000 + i128::from(instant.subsec_nanos() / 1_000)
}

/// The instant the event of `logical_id` in `status` carries, or `None` where the log holds
/// no such event.
///
/// The OLDEST such event, because a resource can be reported in one status more than once:
/// a wait condition is recorded `CREATE_IN_PROGRESS` when the walk schedules it and again
/// when a signal it waited for arrives, and the instant it was SCHEDULED is the first of
/// them.
pub(crate) fn instant(
    case: &str,
    events: &[Event],
    logical_id: &str,
    status: &str,
) -> Option<i128> {
    events
        .iter()
        .filter(|event| event.logical_id == logical_id && event.status == status)
        .map(|event| micros(case, event))
        .min()
}

/// The instant a resource's transition was SCHEDULED, which the case requires the log to
/// carry.
pub(crate) fn scheduled(
    case: &str,
    events: &[Event],
    logical_id: &str,
    in_progress: &str,
) -> i128 {
    instant(case, events, logical_id, in_progress).unwrap_or_else(|| {
        panic!("case {case:?}: {logical_id} must have a {in_progress} event, got {events:?}")
    })
}

/// The instant a resource's transition FINISHED, which the case requires the log to carry.
pub(crate) fn finished(case: &str, events: &[Event], logical_id: &str, terminal: &str) -> i128 {
    instant(case, events, logical_id, terminal).unwrap_or_else(|| {
        panic!("case {case:?}: {logical_id} must have a {terminal} event, got {events:?}")
    })
}

/// One row of the graph table: a template, the dependency edges it states and the resources
/// it leaves independent.
struct GraphCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// `(dependency, dependent)`: the dependent is scheduled at or after the dependency
    /// finished.
    edges: &'static [(&'static str, &'static str)],
    /// Groups of resources with no dependency between them: the members of each group are
    /// in flight at the same time, so their intervals overlap.
    concurrent: &'static [&'static [&'static str]],
    /// Every resource the deployment acts on, each of which owes both of its events.
    acted_on: &'static [&'static str],
}

fn graph_cases() -> Vec<GraphCase> {
    vec![
        GraphCase {
            case: "two resources with no dependency between them",
            stack: "timeline-independent",
            template: r#"{"Resources":{
                "TopicA":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"timeline-independent-a"}},
                "TopicB":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"timeline-independent-b"}}}}"#,
            edges: &[],
            concurrent: &[&["TopicA", "TopicB"]],
            acted_on: &["TopicA", "TopicB"],
        },
        GraphCase {
            case: "a chain of three, stated by DependsOn and by a Ref",
            stack: "timeline-chain",
            template: r#"{"Resources":{
                "Bucket":{"Type":"AWS::S3::Bucket",
                  "Properties":{"BucketName":"timeline-chain-bucket"}},
                "Topic":{"Type":"AWS::SNS::Topic","DependsOn":"Bucket",
                  "Properties":{"TopicName":"timeline-chain-topic"}},
                "Last":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"timeline-chain-last",
                    "DisplayName":{"Ref":"Topic"}}}}}"#,
            edges: &[("Bucket", "Topic"), ("Topic", "Last")],
            concurrent: &[],
            acted_on: &["Bucket", "Topic", "Last"],
        },
        GraphCase {
            case: "a diamond: two independent branches over one root and a join over both",
            stack: "timeline-diamond",
            template: r#"{"Resources":{
                "Root":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"timeline-diamond-root"}},
                "BranchA":{"Type":"AWS::S3::Bucket","DependsOn":"Root",
                  "Properties":{"BucketName":"timeline-diamond-a"}},
                "BranchB":{"Type":"AWS::S3::Bucket","DependsOn":"Root",
                  "Properties":{"BucketName":"timeline-diamond-b"}},
                "Join":{"Type":"AWS::SNS::Topic","DependsOn":["BranchA","BranchB"],
                  "Properties":{"TopicName":"timeline-diamond-join"}}}}"#,
            edges: &[
                ("Root", "BranchA"),
                ("Root", "BranchB"),
                ("BranchA", "Join"),
                ("BranchB", "Join"),
            ],
            concurrent: &[&["BranchA", "BranchB"]],
            acted_on: &["Root", "BranchA", "BranchB", "Join"],
        },
        GraphCase {
            case: "a YAML template whose dependency runs through a type nothing provisions",
            stack: "timeline-yaml",
            template: concat!(
                "Resources:\n",
                "  Vpc:\n",
                "    Type: AWS::EC2::VPC\n",
                "    Properties:\n",
                "      CidrBlock: 10.0.0.0/16\n",
                "  SubnetA:\n",
                "    Type: AWS::EC2::Subnet\n",
                "    Properties:\n",
                "      VpcId: !Ref Vpc\n",
                "      CidrBlock: 10.0.1.0/24\n",
                "  SubnetB:\n",
                "    Type: AWS::EC2::Subnet\n",
                "    Properties:\n",
                "      VpcId: !Ref Vpc\n",
                "      CidrBlock: 10.0.2.0/24\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    DependsOn:\n",
                "      - SubnetA\n",
                "      - SubnetB\n",
                "    Properties:\n",
                "      TopicName: timeline-yaml-topic\n",
            ),
            edges: &[
                ("Vpc", "SubnetA"),
                ("Vpc", "SubnetB"),
                ("SubnetA", "Topic"),
                ("SubnetB", "Topic"),
            ],
            concurrent: &[&["SubnetA", "SubnetB"]],
            acted_on: &["Vpc", "SubnetA", "SubnetB", "Topic"],
        },
    ]
}

#[tokio::test]
async fn a_deployments_events_are_the_timeline_of_its_walk_over_the_dependency_graph() {
    for case in graph_cases() {
        let router = router();
        let (status_code, created) =
            create_stack(&router, case.stack, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, case.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: the create must have completed, got {described}",
            case.case
        );

        let events = events_of(&router, case.stack).await;
        let case_name = case.case;

        // Every resource the walk acted on has an interval of its own, and it is not an
        // instant: the resource was scheduled and then it finished.
        for logical_id in case.acted_on {
            let opened = scheduled(case_name, &events, logical_id, "CREATE_IN_PROGRESS");
            let ended = finished(case_name, &events, logical_id, "CREATE_COMPLETE");
            assert!(
                ended > opened,
                "case {case_name:?}: {logical_id} must finish after it was scheduled, got \
                 {ended} against {opened} in {events:?}"
            );
        }

        // A resource that depends on another is scheduled only once that one has finished.
        for (dependency, dependent) in case.edges {
            let dependency_ended =
                finished(case_name, &events, dependency, "CREATE_COMPLETE");
            let dependent_opened =
                scheduled(case_name, &events, dependent, "CREATE_IN_PROGRESS");
            assert!(
                dependent_opened >= dependency_ended,
                "case {case_name:?}: {dependent} must be scheduled at or after \
                 {dependency} finished, got {dependent_opened} against {dependency_ended} \
                 in {events:?}"
            );
        }

        // Resources with no dependency between them are in flight together.
        for group in case.concurrent {
            let last_scheduled = group
                .iter()
                .map(|logical_id| scheduled(case_name, &events, logical_id, "CREATE_IN_PROGRESS"))
                .max()
                .unwrap_or_default();
            let first_finished = group
                .iter()
                .map(|logical_id| finished(case_name, &events, logical_id, "CREATE_COMPLETE"))
                .min()
                .unwrap_or_default();
            assert!(
                first_finished > last_scheduled,
                "case {case_name:?}: {group:?} have no dependency between them, so their \
                 intervals must overlap, got the first finishing at {first_finished} and \
                 the last scheduled at {last_scheduled} in {events:?}"
            );
        }

        // The stack's own events bracket the walk: the operation was opened before it
        // scheduled anything and reported complete after the last resource finished.
        let stack_opened = scheduled(case_name, &events, case.stack, "CREATE_IN_PROGRESS");
        let stack_completed = finished(case_name, &events, case.stack, "CREATE_COMPLETE");
        for logical_id in case.acted_on {
            let opened = scheduled(case_name, &events, logical_id, "CREATE_IN_PROGRESS");
            let ended = finished(case_name, &events, logical_id, "CREATE_COMPLETE");
            assert!(
                opened >= stack_opened && ended <= stack_completed,
                "case {case_name:?}: {logical_id}'s interval must fall inside the \
                 operation's, got {opened}..{ended} against {stack_opened}..\
                 {stack_completed} in {events:?}"
            );
        }
    }
}

/// A template whose fourth resource the service that owns it REFUSES, with a fifth the walk
/// therefore never reaches.
///
/// `AWS::EC2::VPC` and `AWS::EC2::Subnet` are types this emulator provisions nowhere, so the
/// stack records them from the template alone, each under the `vpc-`/`subnet-` identifier
/// its type's `Ref` returns; the `AWS::RDS::DBSubnetGroup` is really created in Amazon RDS,
/// which refuses the name AWS reserves for the DB subnet group of the account's default VPC.
const REFUSED_CHAIN: &str = r#"{"Resources":{
    "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
    "SubnetA":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.1.0/24"}},
    "SubnetB":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.2.0/24"}},
    "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
      "DBSubnetGroupName":"default",
      "DBSubnetGroupDescription":"the timeline suite's subnets",
      "SubnetIds":[{"Ref":"SubnetA"},{"Ref":"SubnetB"}]}},
    "Topic":{"Type":"AWS::SNS::Topic","DependsOn":"SubnetGroup",
      "Properties":{"TopicName":"timeline-refused-topic"}}}}"#;

/// A deployment an owning service refuses ends when it is refused, which is sooner than the
/// span a walk is laid out across — so the walk is reported compressed into the span the
/// operation really took: in the order it happened, inside the operation's own events, and
/// never past the failure that ended it.
#[tokio::test]
async fn a_refused_deployments_events_keep_the_order_of_the_walk_that_was_cut_short() {
    const CASE: &str = "a deployment an owning service refused part way down the walk";
    let stack = "timeline-refused";
    let router = router();
    let (status_code, created) = create_stack(
        &router,
        stack,
        REFUSED_CHAIN,
        // The refusal this case needs is the OWNING SERVICE's, part way down the walk: with
        // the pre-deployment validations on, the existence check fails a create whose
        // declared name is already held before the walk starts at all
        // (`state::early_validation`), and there is no walk to read.
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    // Twice: the failed create also owes the rollback its terminal status.
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {CASE:?}: the refused deployment must roll the stack back, got {described}"
    );

    let events = events_of(&router, stack).await;
    // The operation's own bracket: nothing the walk recorded falls outside it. A failed
    // create that CloudFormation rolls back never rests in `CREATE_FAILED` — the stack goes
    // from the operation's `CREATE_IN_PROGRESS` straight into `ROLLBACK_IN_PROGRESS`, whose
    // event carries the failure — so the rollback's own event is where the operation ends.
    let stack_opened = scheduled(CASE, &events, stack, "CREATE_IN_PROGRESS");
    let stack_failed = finished(CASE, &events, stack, "ROLLBACK_IN_PROGRESS");
    assert!(
        !events
            .iter()
            .any(|event| event.logical_id == stack && event.status == "CREATE_FAILED"),
        "case {CASE:?}: a rolled-back create rests in no CREATE_FAILED status, so no event of \
         the stack's may report it, got {events:?}"
    );
    for logical_id in ["Vpc", "SubnetA", "SubnetB", "SubnetGroup"] {
        let opened = scheduled(CASE, &events, logical_id, "CREATE_IN_PROGRESS");
        assert!(
            opened >= stack_opened && opened <= stack_failed,
            "case {CASE:?}: {logical_id} was scheduled inside the operation, got {opened} \
             against {stack_opened}..{stack_failed} in {events:?}"
        );
    }
    // The walk's order survives the compression: each resource finished at or after it was
    // scheduled, a dependent was scheduled at or after its dependency finished, and the
    // resource the owning service refused failed at or before the operation did.
    for (dependency, dependent) in [
        ("Vpc", "SubnetA"),
        ("Vpc", "SubnetB"),
        ("SubnetA", "SubnetGroup"),
        ("SubnetB", "SubnetGroup"),
    ] {
        let dependency_ended = finished(CASE, &events, dependency, "CREATE_COMPLETE");
        let dependency_opened = scheduled(CASE, &events, dependency, "CREATE_IN_PROGRESS");
        let dependent_opened = scheduled(CASE, &events, dependent, "CREATE_IN_PROGRESS");
        assert!(
            dependency_opened <= dependency_ended && dependency_ended <= dependent_opened,
            "case {CASE:?}: {dependency} must be scheduled, finish, and only then let \
             {dependent} be scheduled, got {dependency_opened} / {dependency_ended} / \
             {dependent_opened} in {events:?}"
        );
    }
    let refused = finished(CASE, &events, "SubnetGroup", "CREATE_FAILED");
    assert!(
        refused <= stack_failed,
        "case {CASE:?}: the refusal of SubnetGroup must be reported at or before the \
         operation it failed, got {refused} against {stack_failed} in {events:?}"
    );
    // The walk stopped at the refusal, so the resource behind it was never scheduled and no
    // event names it.
    assert!(
        !events.iter().any(|event| event.logical_id == "Topic"),
        "case {CASE:?}: the walk never reached Topic, so no event may name it, got {events:?}"
    );
}

/// `SignalResource` for one resource of one stack.
pub(crate) async fn signal(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    unique_id: &str,
) -> (u16, String) {
    call(
        router,
        "SignalResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
            member("UniqueId", unique_id),
            member("Status", "SUCCESS"),
        ],
    )
    .await
}

/// The diamond the rule is hardest on: two independent wait conditions, a topic behind each
/// of them, and a topic joining both branches.
const GATED_DIAMOND: &str = r#"{"Resources":{
    "HandleA":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "HandleB":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "GateA":{"Type":"AWS::CloudFormation::WaitCondition",
      "Properties":{"Handle":{"Ref":"HandleA"},"Timeout":"300","Count":"1"}},
    "GateB":{"Type":"AWS::CloudFormation::WaitCondition",
      "Properties":{"Handle":{"Ref":"HandleB"},"Timeout":"300","Count":"1"}},
    "BranchA":{"Type":"AWS::SNS::Topic","DependsOn":"GateA",
      "Properties":{"TopicName":"timeline-gated-a"}},
    "BranchB":{"Type":"AWS::SNS::Topic","DependsOn":"GateB",
      "Properties":{"TopicName":"timeline-gated-b"}},
    "Join":{"Type":"AWS::SNS::Topic","DependsOn":["BranchA","BranchB"],
      "Properties":{"TopicName":"timeline-gated-join"}}}}"#;

#[tokio::test]
async fn two_gates_held_open_overlap_for_as_long_as_they_held_and_tear_down_in_reverse() {
    const CASE: &str = "two wait conditions held open, then signalled";
    let stack = "timeline-gated";
    let router = router();
    let (status_code, created) = create_stack(&router, stack, GATED_DIAMOND, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: CreateStack must answer 200, got {status_code} {created}"
    );

    // While the gates hold, the walk has got as far as them and no further: each gate is
    // reported as scheduled and not finished, and nothing behind it is reported at all.
    tokio::time::sleep(HOLD).await;
    let holding = events_of(&router, stack).await;
    for gate in ["GateA", "GateB"] {
        assert!(
            instant(CASE, &holding, gate, "CREATE_IN_PROGRESS").is_some(),
            "case {CASE:?}: {gate} must be reported as scheduled while it holds, got \
             {holding:?}"
        );
        assert!(
            instant(CASE, &holding, gate, "CREATE_COMPLETE").is_none(),
            "case {CASE:?}: {gate} has not finished while it holds, so no event may say it \
             did, got {holding:?}"
        );
    }
    for behind in ["BranchA", "BranchB", "Join"] {
        assert!(
            !holding.iter().any(|event| event.logical_id == behind),
            "case {CASE:?}: {behind} waits behind a gate that still holds, so the walk has \
             not scheduled it and no event may name it, got {holding:?}"
        );
    }

    for (gate, unique_id) in [("GateA", "signal-a"), ("GateB", "signal-b")] {
        let (status_code, signalled) = signal(&router, stack, gate, unique_id).await;
        assert_eq!(
            status_code, 200,
            "case {CASE:?}: the signal for {gate} must be accepted, got {status_code} \
             {signalled}"
        );
    }
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {CASE:?}: the signalled create must complete, got {described}"
    );

    let events = events_of(&router, stack).await;
    // The two gates were in flight together for as long as they were held: each was
    // scheduled before either was signalled, and each finished when its own signal arrived.
    let last_scheduled = ["GateA", "GateB"]
        .iter()
        .map(|gate| scheduled(CASE, &events, gate, "CREATE_IN_PROGRESS"))
        .max()
        .unwrap_or_default();
    let first_finished = ["GateA", "GateB"]
        .iter()
        .map(|gate| finished(CASE, &events, gate, "CREATE_COMPLETE"))
        .min()
        .unwrap_or_default();
    let overlap = first_finished - last_scheduled;
    let floor = i128::from(OVERLAP_FLOOR.as_micros() as u64);
    assert!(
        overlap >= floor,
        "case {CASE:?}: the gates were held {HOLD:?} before being signalled, so they must \
         overlap by at least {OVERLAP_FLOOR:?}, got {overlap}us in {events:?}"
    );

    // Each branch was scheduled only once its own gate had finished, and the join only
    // once both branches had.
    for (gate, branch) in [("GateA", "BranchA"), ("GateB", "BranchB")] {
        let gate_finished = finished(CASE, &events, gate, "CREATE_COMPLETE");
        let branch_scheduled = scheduled(CASE, &events, branch, "CREATE_IN_PROGRESS");
        assert!(
            branch_scheduled >= gate_finished,
            "case {CASE:?}: {branch} must be scheduled at or after {gate} finished, got \
             {branch_scheduled} against {gate_finished} in {events:?}"
        );
    }
    let branches_finished = ["BranchA", "BranchB"]
        .iter()
        .map(|branch| finished(CASE, &events, branch, "CREATE_COMPLETE"))
        .max()
        .unwrap_or_default();
    let join_scheduled = scheduled(CASE, &events, "Join", "CREATE_IN_PROGRESS");
    assert!(
        join_scheduled >= branches_finished,
        "case {CASE:?}: Join must be scheduled at or after the last of its predecessors \
         finished, got {join_scheduled} against {branches_finished} in {events:?}"
    );

    // The teardown walks the same graph BACKWARDS: the join is gone before either branch's
    // deletion starts, and the two branches come down together.
    let (status_code, deleted) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: DeleteStack must answer 200, got {status_code} {deleted}"
    );
    let stack_id = xml_text(&created, "StackId").unwrap_or_default();
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("DELETE_COMPLETE"),
        "case {CASE:?}: the delete must have completed, got {described}"
    );

    let events = events_of(&router, &stack_id).await;
    let join_deleted = finished(CASE, &events, "Join", "DELETE_COMPLETE");
    for branch in ["BranchA", "BranchB"] {
        let branch_delete_scheduled = scheduled(CASE, &events, branch, "DELETE_IN_PROGRESS");
        assert!(
            join_deleted <= branch_delete_scheduled,
            "case {CASE:?}: Join depends on {branch}, so its DELETE_COMPLETE must come at \
             or before {branch}'s deletion starts, got {join_deleted} against \
             {branch_delete_scheduled} in {events:?}"
        );
    }
    let last_delete_scheduled = ["BranchA", "BranchB"]
        .iter()
        .map(|branch| scheduled(CASE, &events, branch, "DELETE_IN_PROGRESS"))
        .max()
        .unwrap_or_default();
    let first_delete_finished = ["BranchA", "BranchB"]
        .iter()
        .map(|branch| finished(CASE, &events, branch, "DELETE_COMPLETE"))
        .min()
        .unwrap_or_default();
    assert!(
        first_delete_finished > last_delete_scheduled,
        "case {CASE:?}: the two branches have no dependency between them, so their \
         deletions must overlap, got the first finishing at {first_delete_finished} and \
         the last scheduled at {last_delete_scheduled} in {events:?}"
    );
}
