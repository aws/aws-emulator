// SPDX-License-Identifier: CC0-1.0

//! The per-resource events a stack's TEARDOWN records, end to end over the awsQuery wire.
//!
//! The rule under test: **a stack operation that tears a stack's graph down records, for
//! every resource the stack OWNS, a `DELETE_IN_PROGRESS` event when the walk reaches it and
//! a `DELETE_COMPLETE` one when it settles** — exactly as the create phase records the
//! build, and whether or not any service of this emulator held something to delete for that
//! resource, since a stack's event stream reports what became of the resources it owned and
//! not which of them had an owning API. Those stamps are what make the teardown's order
//! observable: the walk runs against the dependency relation, so a dependent's
//! `DELETE_COMPLETE` comes at or before its dependency's `DELETE_IN_PROGRESS`, and
//! resources with no dependency between them come down together.
//!
//! Without them a client cannot verify or debug a teardown: nothing says which resource is
//! being deleted, in what order, which one is slow or which one failed, so a CLI event tail
//! or a CI gate that waits for a dependant to be gone before its dependency sees nothing.
//!
//! The table spans what the rule generalises over — the resource TYPES (ones an owning
//! service of this emulator really holds, and ones it provisions nowhere), the SHAPE of the
//! graph (independent resources, a chain, a diamond with a join), how a dependency is
//! STATED (`DependsOn`, `Ref`, `Fn::GetAtt`), the COUNT of resources, and a create held
//! open at wait conditions until `SignalResource` releases them — and the last case is the
//! negative half: a resource a failed create never reached is owned by nobody, so its
//! teardown records nothing.
//!
//! Every case drives the real request path through the router `stack_lifecycle.rs` builds,
//! so what is asserted is what a client calling `cloudformation describe-stack-events`
//! sees; the timeline readers are `stack_event_timeline.rs`'s, so the two suites read one
//! event stream the same way.

use crate::stack_event_timeline::{events_of, finished, instant, scheduled, signal, Event};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
};

/// `DELETE_IN_PROGRESS`/`DELETE_COMPLETE` from the model's `ResourceStatus` enum, as the
/// crate itself spells them.
use service_com_amazonaws_cloudformation_cloudformation::state::resource_status;

/// One row: a stack to build and tear down, and what its teardown must have recorded.
struct Teardown {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The wait conditions whose signals the create waits for, each with the `UniqueId` the
    /// signal carries.
    signals: &'static [(&'static str, &'static str)],
    /// Every resource the stack owns: each owes both of its delete events.
    owned: &'static [&'static str],
    /// `(dependency, dependent)`: the teardown walks the relation BACKWARDS, so the
    /// dependent is gone before the dependency's deletion starts.
    edges: &'static [(&'static str, &'static str)],
    /// Groups of resources with no dependency between them, whose deletions therefore
    /// overlap.
    concurrent: &'static [&'static [&'static str]],
}

/// A policy document an `AWS::SNS::TopicPolicy` declares, spelled as the template's own
/// nested JSON object.
const TOPIC_POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[
    {"Effect":"Allow","Principal":"*","Action":"sns:Publish","Resource":"*"}]}"#;

/// The shape the recorded teardown walks: two independent branches, each held open at a
/// wait condition, and a join over both of them.
///
/// Both kinds of resource are on every level of it — the handles, the gates and the join are
/// types no owning service of this emulator holds, the topics are really created in Amazon
/// SNS and the parameters in AWS Systems Manager — and the dependencies are stated three
/// ways: `DependsOn`, a `Ref` in a property and a `Ref` inside a property LIST.
fn gated_diamond() -> String {
    format!(
        r#"{{"Resources":{{
            "HandleA":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
            "HandleB":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
            "GateA":{{"Type":"AWS::CloudFormation::WaitCondition","Properties":{{
              "Handle":{{"Ref":"HandleA"}},"Timeout":"300","Count":"1"}}}},
            "GateB":{{"Type":"AWS::CloudFormation::WaitCondition","Properties":{{
              "Handle":{{"Ref":"HandleB"}},"Timeout":"300","Count":"1"}}}},
            "TopicA":{{"Type":"AWS::SNS::Topic","DependsOn":"GateA",
              "Properties":{{"TopicName":"teardown-diamond-a"}}}},
            "TopicB":{{"Type":"AWS::SNS::Topic","DependsOn":"GateB",
              "Properties":{{"TopicName":"teardown-diamond-b"}}}},
            "ParamA":{{"Type":"AWS::SSM::Parameter","Properties":{{
              "Name":"/teardown-diamond/a","Type":"String",
              "Value":{{"Ref":"TopicA"}}}}}},
            "ParamB":{{"Type":"AWS::SSM::Parameter","Properties":{{
              "Name":"/teardown-diamond/b","Type":"String",
              "Value":{{"Ref":"TopicB"}}}}}},
            "JoinPolicy":{{"Type":"AWS::SNS::TopicPolicy",
              "DependsOn":["ParamA","ParamB"],"Properties":{{
              "Topics":[{{"Ref":"TopicA"}},{{"Ref":"TopicB"}}],
              "PolicyDocument":{TOPIC_POLICY_DOCUMENT}}}}}}}}}"#
    )
}

fn teardown_rows() -> Vec<Teardown> {
    vec![
        Teardown {
            case: "three resources with no dependency between them, two of types no owning \
                   service of this emulator holds",
            stack: "teardown-independent",
            template: r#"{"Resources":{
                "Igw":{"Type":"AWS::EC2::InternetGateway"},
                "Logs":{"Type":"AWS::Logs::LogGroup",
                  "Properties":{"LogGroupName":"teardown-independent-logs"}},
                "Topic":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"teardown-independent-topic"}}}}"#
                .to_string(),
            signals: &[],
            owned: &["Igw", "Logs", "Topic"],
            edges: &[],
            concurrent: &[&["Igw", "Logs", "Topic"]],
        },
        Teardown {
            case: "a chain whose middle link no owning service holds, stated by Ref and by \
                   DependsOn",
            stack: "teardown-chain",
            template: r#"{"Resources":{
                "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
                "Igw":{"Type":"AWS::EC2::InternetGateway"},
                "Attach":{"Type":"AWS::EC2::VPCGatewayAttachment","Properties":{
                  "VpcId":{"Ref":"Vpc"},"InternetGatewayId":{"Ref":"Igw"}}},
                "Topic":{"Type":"AWS::SNS::Topic","DependsOn":"Attach",
                  "Properties":{"TopicName":"teardown-chain-topic"}}}}"#
                .to_string(),
            signals: &[],
            owned: &["Vpc", "Igw", "Attach", "Topic"],
            edges: &[
                ("Vpc", "Attach"),
                ("Igw", "Attach"),
                ("Attach", "Topic"),
            ],
            concurrent: &[&["Vpc", "Igw"]],
        },
        Teardown {
            case: "a diamond held open at two wait conditions: a topic behind each gate, a \
                   parameter naming each topic and a policy joining both branches",
            stack: "teardown-diamond",
            template: gated_diamond(),
            signals: &[("GateA", "signal-a"), ("GateB", "signal-b")],
            owned: &[
                "HandleA",
                "HandleB",
                "GateA",
                "GateB",
                "TopicA",
                "TopicB",
                "ParamA",
                "ParamB",
                "JoinPolicy",
            ],
            edges: &[
                ("HandleA", "GateA"),
                ("HandleB", "GateB"),
                ("GateA", "TopicA"),
                ("GateB", "TopicB"),
                ("TopicA", "ParamA"),
                ("TopicB", "ParamB"),
                ("ParamA", "JoinPolicy"),
                ("ParamB", "JoinPolicy"),
                ("TopicA", "JoinPolicy"),
                ("TopicB", "JoinPolicy"),
            ],
            concurrent: &[
                &["GateA", "GateB"],
                &["TopicA", "TopicB"],
                &["ParamA", "ParamB"],
            ],
        },
    ]
}

/// How many events name `logical_id` in `status`.
fn occurrences(events: &[Event], logical_id: &str, status: &str) -> usize {
    events
        .iter()
        .filter(|event| event.logical_id == logical_id && event.status == status)
        .count()
}

#[tokio::test]
async fn a_teardown_records_both_delete_events_for_every_resource_the_stack_owns() {
    for row in teardown_rows() {
        let case = row.case;
        let router = router();
        let (status_code, created) = create_stack(&router, row.stack, &row.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateStack must answer a StackId, got {created}")
        });
        // A create held at a wait condition completes only once its signals arrive.
        for (gate, unique_id) in row.signals {
            let (status_code, signalled) = signal(&router, row.stack, gate, unique_id).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: the signal for {gate} must be accepted, got {status_code} \
                 {signalled}"
            );
        }
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: the create must have completed, got {described}"
        );

        let (status_code, deleted) =
            call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DeleteStack must answer 200, got {status_code} {deleted}"
        );
        wait_for_the_transition().await;
        // A deleted stack is addressable by its id only.
        let (_, described) = describe_stack(&router, &stack_id).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("DELETE_COMPLETE"),
            "case {case:?}: the delete must have completed, got {described}"
        );

        let events = events_of(&router, &stack_id).await;
        // The stack's own events bracket the teardown.
        let stack_opened = scheduled(case, &events, row.stack, resource_status::DELETE_IN_PROGRESS);
        let stack_completed = finished(case, &events, row.stack, resource_status::DELETE_COMPLETE);

        // Every resource the stack owned has an interval of its own, recorded exactly once,
        // inside the operation's.
        for logical_id in row.owned {
            let opened =
                scheduled(case, &events, logical_id, resource_status::DELETE_IN_PROGRESS);
            let ended = finished(case, &events, logical_id, resource_status::DELETE_COMPLETE);
            assert!(
                ended > opened,
                "case {case:?}: {logical_id} must finish deleting after its deletion was \
                 scheduled, got {ended} against {opened} in {events:?}"
            );
            assert!(
                opened >= stack_opened && ended <= stack_completed,
                "case {case:?}: {logical_id}'s deletion must fall inside the stack's, got \
                 {opened}..{ended} against {stack_opened}..{stack_completed} in {events:?}"
            );
            for status in [
                resource_status::DELETE_IN_PROGRESS,
                resource_status::DELETE_COMPLETE,
            ] {
                let count = occurrences(&events, logical_id, status);
                assert_eq!(
                    count, 1,
                    "case {case:?}: {logical_id} transitioned through its deletion once, so \
                     exactly one {status} event may name it, got {count} in {events:?}"
                );
            }
            assert!(
                instant(case, &events, logical_id, resource_status::DELETE_FAILED).is_none(),
                "case {case:?}: nothing refused {logical_id}'s deletion, so no DELETE_FAILED \
                 event may name it, got {events:?}"
            );
        }

        // The walk runs against the dependency relation: a dependent is gone before its
        // dependency's deletion starts.
        for (dependency, dependent) in row.edges {
            let dependent_ended =
                finished(case, &events, dependent, resource_status::DELETE_COMPLETE);
            let dependency_opened =
                scheduled(case, &events, dependency, resource_status::DELETE_IN_PROGRESS);
            assert!(
                dependent_ended <= dependency_opened,
                "case {case:?}: {dependent} depends on {dependency}, so its DELETE_COMPLETE \
                 must come at or before {dependency}'s DELETE_IN_PROGRESS, got \
                 {dependent_ended} against {dependency_opened} in {events:?}"
            );
        }

        // Resources with no dependency between them come down together.
        for group in row.concurrent {
            let last_scheduled = group
                .iter()
                .map(|logical_id| {
                    scheduled(case, &events, logical_id, resource_status::DELETE_IN_PROGRESS)
                })
                .max()
                .unwrap_or_default();
            let first_finished = group
                .iter()
                .map(|logical_id| {
                    finished(case, &events, logical_id, resource_status::DELETE_COMPLETE)
                })
                .min()
                .unwrap_or_default();
            assert!(
                first_finished > last_scheduled,
                "case {case:?}: {group:?} have no dependency between them, so their deletions \
                 must overlap, got the first finishing at {first_finished} and the last \
                 scheduled at {last_scheduled} in {events:?}"
            );
        }
    }
}

/// A template whose middle resource Amazon RDS REFUSES — the name AWS reserves for the DB
/// subnet group of the account's default VPC — with a resource of a type no owning service
/// of this emulator holds behind it, which the walk therefore never reaches.
const REFUSED_BEFORE_A_DECLARED_ONLY_RESOURCE: &str = r#"{"Resources":{
    "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
    "Subnet":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.1.0/24"}},
    "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
      "DBSubnetGroupName":"default",
      "DBSubnetGroupDescription":"the teardown suite's subnets",
      "SubnetIds":[{"Ref":"Subnet"}]}},
    "Igw":{"Type":"AWS::EC2::InternetGateway","DependsOn":"SubnetGroup"}}}"#;

#[tokio::test]
async fn a_resource_the_walk_never_reached_is_owned_by_nobody_and_records_no_teardown() {
    const CASE: &str = "a resource of a type nothing provisions behind a refused create";
    let stack = "teardown-never-reached";
    let router = router();
    let (status_code, created) = create_stack(
        &router,
        stack,
        REFUSED_BEFORE_A_DECLARED_ONLY_RESOURCE,
        // The refusal this case is about is Amazon RDS's, part way down the walk: with the
        // pre-deployment validations on, the existence check fails a create whose declared
        // name the owning service already holds before the walk starts
        // (`state::early_validation`).
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {CASE:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    let stack_id = xml_text(&created, "StackId").unwrap_or_else(|| {
        panic!("case {CASE:?}: CreateStack must answer a StackId, got {created}")
    });
    // Twice: the failed create also owes the rollback its terminal status.
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {CASE:?}: the refused create must roll the stack back, got {described}"
    );

    let events = events_of(&router, &stack_id).await;
    // What the create DID make is torn down and reported, whether an owning service held it
    // (the VPC and the subnet, in Amazon EC2) or not.
    for logical_id in ["Vpc", "Subnet"] {
        let opened = scheduled(CASE, &events, logical_id, resource_status::DELETE_IN_PROGRESS);
        let ended = finished(CASE, &events, logical_id, resource_status::DELETE_COMPLETE);
        assert!(
            ended > opened,
            "case {CASE:?}: the rollback must report {logical_id}'s removal, got {ended} \
             against {opened} in {events:?}"
        );
    }
    // The walk stopped at the refusal, so nothing was ever brought into being under the
    // declaration behind it and no event names it.
    assert!(
        !events.iter().any(|event| event.logical_id == "Igw"),
        "case {CASE:?}: the walk never reached Igw, so no event may name it, got {events:?}"
    );
}
