// SPDX-License-Identifier: CC0-1.0

//! The PER-RESOURCE events a stack operation records, end to end over the awsQuery wire.
//!
//! The rule under test: **a stack operation records a `StackEvent` for every transition of
//! every resource it acts on** — the `*_IN_PROGRESS` event that opens the resource's
//! transition and the `*_COMPLETE` or `*_FAILED` one that ends it, each naming that
//! resource's `LogicalResourceId` and `ResourceType`, carrying its `PhysicalResourceId`
//! once one exists, and, where the service that owns the resource refused, a
//! `ResourceStatusReason` reporting that service's own message and the `HandlerErrorCode`
//! class its refusal falls in. The stack's own status events are the same stream's records
//! for the operation as a whole, and the whole stream is answered newest first.
//!
//! Without those records a client cannot tell WHICH resource a failed deployment failed on
//! or WHY: every deploy tool diagnoses a failed deploy by scanning the event stream for the
//! first `*_FAILED` resource event and printing its `ResourceStatusReason`.
//!
//! Every case drives the real request path through the router `stack_lifecycle.rs` builds,
//! so what is asserted is what a client calling `cloudformation describe-stack-events`
//! sees. The resources are read back through `DescribeStackResources` with that suite's own
//! reader, so the event stream and the stack's resource view are held to ONE answer.

use service_com_amazonaws_cloudformation_cloudformation::ops::types::HANDLER_ERROR_CODE_VALUES;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};
use crate::stack_resources::{element, resources_of, wire_resources};
use crate::stack_resource_provisioning::s3;

/// One `StackEvent` as the wire reports it.
#[derive(Debug, Clone)]
struct WireEvent {
    event_id: Option<String>,
    stack_id: Option<String>,
    stack_name: Option<String>,
    logical_id: Option<String>,
    physical_id: Option<String>,
    resource_type: Option<String>,
    status: Option<String>,
    status_reason: Option<String>,
    timestamp: Option<String>,
}

/// Every event a `DescribeStackEvents` response carries, in wire order (newest first).
///
/// A `StackEvent` holds no list of its own, so each `<member>` of the response is one
/// event and the shared leaf reader
/// ([`element`](crate::stack_resources::element)) reads its members.
fn wire_events(xml: &str) -> Vec<WireEvent> {
    xml_blocks(xml, "member")
        .iter()
        .map(|entry| WireEvent {
            event_id: element(entry, "EventId"),
            stack_id: element(entry, "StackId"),
            stack_name: element(entry, "StackName"),
            logical_id: element(entry, "LogicalResourceId"),
            physical_id: element(entry, "PhysicalResourceId"),
            resource_type: element(entry, "ResourceType"),
            status: element(entry, "ResourceStatus"),
            status_reason: element(entry, "ResourceStatusReason"),
            timestamp: element(entry, "Timestamp"),
        })
        .collect()
}

/// The events a stack has recorded, newest first.
async fn events_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    name_or_id: &str,
) -> Vec<WireEvent> {
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
    wire_events(&body)
}

/// The position of the event for `logical_id` in `status`, in the newest-first list.
fn position(events: &[WireEvent], logical_id: &str, status: &str) -> Option<usize> {
    events.iter().position(|event| {
        event.logical_id.as_deref() == Some(logical_id)
            && event.status.as_deref() == Some(status)
    })
}

/// The `HandlerErrorCode` a failed resource's reason reports, and the provider message it
/// quotes.
///
/// The class is held to [`HANDLER_ERROR_CODE_VALUES`] — the crate's own generated list of
/// what the model's `HandlerErrorCode` enum accepts — so this assertion cannot drift from
/// the model.
fn reported_failure(case: &str, logical_id: &str, reason: &str) -> (String, String) {
    const OPENS: &str = "Resource handler returned message: \"";
    const CLASS: &str = "HandlerErrorCode: ";
    let quoted = reason
        .split_once(OPENS)
        .and_then(|(_, rest)| rest.rsplit_once('"'))
        .map(|(message, _)| message.to_string());
    let class = reason
        .split_once(CLASS)
        .map(|(_, rest)| rest.trim_end_matches(')').to_string());
    assert!(
        quoted.is_some() && class.is_some(),
        "case {case:?}: the reason for {logical_id} must quote the provider's message and \
         name its HandlerErrorCode, got {reason:?}"
    );
    let (quoted, class) = (quoted.unwrap_or_default(), class.unwrap_or_default());
    assert!(
        HANDLER_ERROR_CODE_VALUES.contains(&class.as_str()),
        "case {case:?}: {class:?} is not a HandlerErrorCode the model declares, in {reason:?}"
    );
    assert!(
        !quoted.is_empty(),
        "case {case:?}: the reason for {logical_id} must quote the service's own message, \
         got {reason:?}"
    );
    (class, quoted)
}

/// What one declared resource's events must say.
struct ExpectedResource<'a> {
    logical_id: &'a str,
    resource_type: &'a str,
    /// The status the resource's transition ended in: `CREATE_COMPLETE` for one the
    /// deployment carried through, `CREATE_FAILED` for one the owning service refused.
    terminal: &'a str,
}

fn expected<'a>(
    logical_id: &'a str,
    resource_type: &'a str,
    terminal: &'a str,
) -> ExpectedResource<'a> {
    ExpectedResource {
        logical_id,
        resource_type,
        terminal,
    }
}

/// One row of the event table.
struct EventCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    resources: Vec<ExpectedResource<'a>>,
    /// The logical ids the deployment never reached, which no event may name: a walk that
    /// stops at a refusal has acted on nothing after it.
    unreached: &'a [&'a str],
    /// Whether the deployment failed, and so left the stack rolled back with no resources
    /// to read back.
    rolled_back: bool,
}

/// A VPC, two subnets in it and a DB subnet group over those subnets, in JSON.
///
/// The VPC and the subnets are really created in Amazon EC2 — each under the
/// `vpc-`/`subnet-` identifier that service assigned it — while the
/// `AWS::RDS::DBSubnetGroup` is really created in Amazon RDS, which refuses the name AWS
/// reserves for the DB subnet group of the account's default VPC. `Topic` is declared to
/// depend on that group, so the walk never reaches it.
const NETWORK_TEMPLATE: &str = r#"{"Resources":{
    "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
    "SubnetA":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.1.0/24",
      "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]}}},
    "SubnetB":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.2.0/24",
      "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]}}},
    "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
      "DBSubnetGroupName":"default",
      "DBSubnetGroupDescription":"the probe stack's subnets",
      "SubnetIds":[{"Ref":"SubnetA"},{"Ref":"SubnetB"}]}},
    "Topic":{"Type":"AWS::SNS::Topic","DependsOn":"SubnetGroup"}}}"#;

#[tokio::test]
async fn every_resource_a_stack_operation_acts_on_records_its_own_events() {
    // The rows differ along what the rule generalises over: how MANY resources one
    // deployment acts on, whether the service that owns a type is called for it at all or
    // the stack records it from the template alone, the template spelling (YAML and JSON),
    // and whether the deployment carried every resource through or an owning service
    // refused one of them part way down the walk.
    let cases: Vec<EventCase<'_>> = vec![
        EventCase {
            case: "one resource, created in the service that owns it",
            stack_name: "events-one-topic",
            template: concat!(
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: events-one-topic\n",
            ),
            resources: vec![expected("Topic", "AWS::SNS::Topic", "CREATE_COMPLETE")],
            unreached: &[],
            rolled_back: false,
        },
        EventCase {
            case: "a resource created in the service that owns it beside one the stack \
                   records from the template alone",
            stack_name: "events-mixed",
            template: r#"{"Resources":{
                "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
                "SubnetA":{"Type":"AWS::EC2::Subnet","Properties":{
                  "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.1.0/24"}},
                "Igw":{"Type":"AWS::EC2::InternetGateway"},
                "Topic":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"events-mixed-topic"}}}}"#,
            resources: vec![
                expected("Vpc", "AWS::EC2::VPC", "CREATE_COMPLETE"),
                expected("SubnetA", "AWS::EC2::Subnet", "CREATE_COMPLETE"),
                expected("Igw", "AWS::EC2::InternetGateway", "CREATE_COMPLETE"),
                expected("Topic", "AWS::SNS::Topic", "CREATE_COMPLETE"),
            ],
            unreached: &[],
            rolled_back: false,
        },
        EventCase {
            case: "a deployment an owning service refused part way down the walk",
            stack_name: "events-refused",
            template: NETWORK_TEMPLATE,
            resources: vec![
                expected("Vpc", "AWS::EC2::VPC", "CREATE_COMPLETE"),
                expected("SubnetA", "AWS::EC2::Subnet", "CREATE_COMPLETE"),
                expected("SubnetB", "AWS::EC2::Subnet", "CREATE_COMPLETE"),
                expected(
                    "SubnetGroup",
                    "AWS::RDS::DBSubnetGroup",
                    "CREATE_FAILED",
                ),
            ],
            unreached: &["Topic"],
            rolled_back: true,
        },
    ];

    for case in &cases {
        let router = router();
        // A case whose deployment is REFUSED is refused by the service that owns the
        // resource, which the create reaches only with the pre-deployment validations off:
        // with them on, the existence check fails a create whose declared name that service
        // already holds before any resource is asked for (`state::early_validation`).
        let extra: Vec<(String, String)> = if case.rolled_back {
            vec![member("DisableValidation", "true")]
        } else {
            Vec::new()
        };
        let (status_code, created) =
            create_stack(&router, case.stack_name, case.template, &extra).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_default();
        // Twice: a create that failed also owes the rollback its terminal status, and both
        // are on the stack's clock.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let events = events_of(&router, case.stack_name).await;

        // The stack's OWN events are in the same stream, and the oldest of all of them is
        // the one that opened the create.
        let oldest = events.last().expect("a created stack has events");
        assert_eq!(
            (oldest.logical_id.as_deref(), oldest.status.as_deref()),
            (Some(case.stack_name), Some("CREATE_IN_PROGRESS")),
            "case {:?}: the oldest event must be the stack's own CREATE_IN_PROGRESS, got \
             {events:?}",
            case.case
        );

        for resource in &case.resources {
            let opened = position(&events, resource.logical_id, "CREATE_IN_PROGRESS");
            let ended = position(&events, resource.logical_id, resource.terminal);
            assert!(
                opened.is_some(),
                "case {:?}: {} must have a CREATE_IN_PROGRESS event, got {events:?}",
                case.case,
                resource.logical_id
            );
            assert!(
                ended.is_some(),
                "case {:?}: {} must have a {} event, got {events:?}",
                case.case,
                resource.logical_id,
                resource.terminal
            );
            // Newest first, so the transition that ENDED comes before the one that opened
            // it.
            assert!(
                ended < opened,
                "case {:?}: {}'s {} must be reported before its CREATE_IN_PROGRESS, got \
                 {ended:?} against {opened:?} in {events:?}",
                case.case,
                resource.logical_id,
                resource.terminal
            );
            let event = &events[ended.unwrap_or_default()];
            assert_eq!(
                event.resource_type.as_deref(),
                Some(resource.resource_type),
                "case {:?}: {}'s event must carry its declared type, got {event:?}",
                case.case,
                resource.logical_id
            );
            assert_eq!(
                event.stack_name.as_deref(),
                Some(case.stack_name),
                "case {:?}: {}'s event must name the stack, got {event:?}",
                case.case,
                resource.logical_id
            );
            assert_eq!(
                event.stack_id.as_deref(),
                Some(stack_id.as_str()),
                "case {:?}: {}'s event must carry the stack id, got {event:?}",
                case.case,
                resource.logical_id
            );
            assert!(
                event
                    .timestamp
                    .as_deref()
                    .is_some_and(|timestamp| !timestamp.is_empty()),
                "case {:?}: {}'s event must carry a Timestamp, got {event:?}",
                case.case,
                resource.logical_id
            );

            if resource.terminal == "CREATE_FAILED" {
                let reason = event.status_reason.clone().unwrap_or_default();
                reported_failure(case.case, resource.logical_id, &reason);
                assert!(
                    event
                        .physical_id
                        .as_deref()
                        .is_none_or(|physical| physical.is_empty()),
                    "case {:?}: nothing was created for {}, so its event carries no \
                     PhysicalResourceId, got {event:?}",
                    case.case,
                    resource.logical_id
                );
                continue;
            }

            assert!(
                event
                    .physical_id
                    .as_deref()
                    .is_some_and(|physical| !physical.is_empty()),
                "case {:?}: {}'s completed event must carry the identifier it was created \
                 under, got {event:?}",
                case.case,
                resource.logical_id
            );
            assert!(
                event.status_reason.is_none(),
                "case {:?}: a completed resource states no failure reason, got {event:?}",
                case.case
            );
        }

        for logical_id in case.unreached {
            assert!(
                !events
                    .iter()
                    .any(|event| event.logical_id.as_deref() == Some(*logical_id)),
                "case {:?}: the walk never reached {logical_id}, so no event may name it, \
                 got {events:?}",
                case.case
            );
        }

        // Every event of the stream is one record: two events never share an EventId.
        let mut ids: Vec<String> = events
            .iter()
            .map(|event| event.event_id.clone().unwrap_or_default())
            .collect();
        let recorded = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            recorded,
            "case {:?}: every EventId is distinct, got {events:?}",
            case.case
        );

        if case.rolled_back {
            let (_, described) = describe_stack(&router, &stack_id).await;
            assert_eq!(
                xml_text(&described, "StackStatus").as_deref(),
                Some("ROLLBACK_COMPLETE"),
                "case {:?}: a refused resource rolls the stack back, got {described}",
                case.case
            );
            continue;
        }

        // The event stream and the stack's resource view are ONE answer: the identifier a
        // resource's completed event carries is the one `DescribeStackResources` reports
        // for it.
        let (status_code, body) = resources_of(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStackResources must answer 200, got {status_code} {body}",
            case.case
        );
        for reported in wire_resources(&body) {
            let logical_id = reported.logical_id.clone().unwrap_or_default();
            let Some(index) = position(&events, &logical_id, "CREATE_COMPLETE") else {
                continue;
            };
            assert_eq!(
                events[index].physical_id, reported.physical_id,
                "case {:?}: the PhysicalResourceId of {logical_id}'s event must be the one \
                 the stack reports for it, got {:?} against {:?}",
                case.case, events[index].physical_id, reported.physical_id
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_stack_records_an_event_per_resource_it_owns() {
    let router = router();
    // Two topics Amazon SNS really holds, a VPC Amazon EC2 really holds, and a resource
    // this emulator provisions nowhere: the deletion asks SNS for the topics and EC2 for
    // the VPC, and nothing for the internet gateway — and records the teardown of all four,
    // because which of them an owning service held is not something the event stream says.
    let template = r#"{"Resources":{
        "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
        "Igw":{"Type":"AWS::EC2::InternetGateway"},
        "First":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"events-deleted-1"}},
        "Second":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"events-deleted-2"}}}}"#;
    let (status_code, created) = create_stack(&router, "events-deleted", template, &[]).await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {created}"
    );
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;

    let (status_code, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "events-deleted")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DeleteStack must answer 200, got {status_code} {deleted}"
    );
    wait_for_the_transition().await;

    // A deleted stack is addressable by its id only.
    let events = events_of(&router, &stack_id).await;
    for logical_id in ["First", "Second"] {
        let opened = position(&events, logical_id, "DELETE_IN_PROGRESS");
        let ended = position(&events, logical_id, "DELETE_COMPLETE");
        assert!(
            opened.is_some() && ended.is_some(),
            "case \"deleting the stack\": {logical_id} must have both delete events, got \
             {events:?}"
        );
        assert!(
            ended < opened,
            "case \"deleting the stack\": {logical_id}'s DELETE_COMPLETE is newer than its \
             DELETE_IN_PROGRESS, got {ended:?} against {opened:?} in {events:?}"
        );
        // The deletion did not re-open a transition it had already recorded: three events
        // for the create (it opens, the create is initiated in the owning service, it
        // completes) and two for the delete.
        assert_eq!(
            events
                .iter()
                .filter(|event| event.logical_id.as_deref() == Some(logical_id))
                .count(),
            5,
            "case \"deleting the stack\": {logical_id} transitioned once through its create \
             and once through its delete, got {events:?}"
        );
    }
    // The VPC is one the deletion really asks Amazon EC2 for, and the internet gateway one
    // it asks nobody for: both are resources the stack owned, so both record the same pair
    // of transitions a topic does.
    for logical_id in ["Vpc", "Igw"] {
        assert!(
            position(&events, logical_id, "DELETE_IN_PROGRESS").is_some()
                && position(&events, logical_id, "DELETE_COMPLETE").is_some(),
            "case \"deleting the stack\": the stack owned {logical_id}, so its teardown \
             records both events, got {events:?}"
        );
    }
    let newest = events.first().expect("a deleted stack has events");
    assert_eq!(
        (newest.logical_id.as_deref(), newest.status.as_deref()),
        (Some("events-deleted"), Some("DELETE_COMPLETE")),
        "case \"deleting the stack\": the newest event is the stack's own DELETE_COMPLETE, \
         got {events:?}"
    );
}

/// One refusal, and the `HandlerErrorCode` class it falls in.
struct RefusalCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The logical id of the resource the owning service refuses.
    logical_id: &'a str,
    /// The class the event must report, from the model's `HandlerErrorCode` enum.
    class: &'a str,
    /// A fragment of the owning service's OWN message the quoted text must carry, so the
    /// reason is that service's wording rather than CloudFormation's.
    quotes: &'a str,
}

#[tokio::test]
async fn a_refused_resource_reports_the_owning_services_message_and_its_error_class() {
    // The rows differ along the CLASS of refusal — the value a client branches on to decide
    // whether a failed resource is worth retrying — and along which service refused.
    let cases: &[RefusalCase<'_>] = &[
        RefusalCase {
            case: "a declaration the owning service will not accept",
            stack_name: "refusal-invalid",
            template: r#"{"Resources":{"SubnetGroup":{
                "Type":"AWS::RDS::DBSubnetGroup","Properties":{
                  "DBSubnetGroupDescription":"a group over values that name no subnet",
                  "SubnetIds":["not-a-subnet-id","also-not-a-subnet-id"]}}}}"#,
            logical_id: "SubnetGroup",
            class: "InvalidRequest",
            quotes: "subnet",
        },
        RefusalCase {
            case: "a declaration naming a resource the owning service does not hold",
            stack_name: "refusal-not-found",
            template: r#"{"Resources":{"Database":{
                "Type":"AWS::RDS::DBInstance","Properties":{
                  "DBInstanceIdentifier":"refusal-not-found-db",
                  "Engine":"mysql","DBInstanceClass":"db.t3.micro",
                  "DBSubnetGroupName":"no-such-subnet-group"}}}}"#,
            logical_id: "Database",
            class: "NotFound",
            quotes: "no-such-subnet-group",
        },
    ];

    for case in cases {
        let router = router();
        let (status_code, created) =
            create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let events = events_of(&router, case.stack_name).await;
        let index = position(&events, case.logical_id, "CREATE_FAILED");
        assert!(
            index.is_some(),
            "case {:?}: {} must have a CREATE_FAILED event, got {events:?}",
            case.case,
            case.logical_id
        );
        let reason = events[index.unwrap_or_default()]
            .status_reason
            .clone()
            .unwrap_or_default();
        let (class, quoted) = reported_failure(case.case, case.logical_id, &reason);
        assert_eq!(
            class, case.class,
            "case {:?}: expected the {} class, got {class:?} in {reason:?}",
            case.case, case.class
        );
        assert!(
            quoted.contains(case.quotes),
            "case {:?}: the quoted message must be the owning service's own, carrying {:?}, \
             got {quoted:?}",
            case.case,
            case.quotes
        );
    }
}

#[tokio::test]
async fn a_rollback_that_keeps_re_asking_records_one_opening_and_one_terminal() {
    let router = router();
    // A bucket the caller already owns and has written into: the stack's create accepts it
    // ("if you re-create an existing bucket that you already own in the us-east-1 Region,
    // Amazon S3 returns 200 OK"), so the stack records it as a resource it created — and the
    // rollback below cannot delete it, because Amazon S3 refuses to delete a bucket that
    // still holds objects however many times it is asked.
    let seeded = s3(&router, "PUT", "/events-held-bucket", b"").await;
    assert_eq!(
        seeded.status, 200,
        "the bucket must exist before the stack, got {} {}",
        seeded.status, seeded.body
    );
    let written = s3(
        &router,
        "PUT",
        "/events-held-bucket/held/object.txt",
        b"an object the rollback cannot remove",
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the object must be written before the stack, got {} {}",
        written.status, written.body
    );

    // The subnet group fails the create, so the rollback has to remove the bucket the
    // create had already taken over — which is the deletion S3 keeps refusing.
    let template = r#"{"Resources":{
        "Bucket":{"Type":"AWS::S3::Bucket",
          "Properties":{"BucketName":"events-held-bucket"}},
        "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","DependsOn":"Bucket",
          "Properties":{"DBSubnetGroupDescription":"a group over values that name no subnet",
            "SubnetIds":["not-a-subnet-id","also-not-a-subnet-id"]}}}}"#;
    let (status_code, created) = create_stack(
        &router,
        "events-held",
        template,
        // The create TAKES OVER the bucket seeded above, which it reaches only with the
        // pre-deployment validations off (`state::early_validation`).
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {created}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let events = events_of(&router, "events-held").await;
    // One opening, however many times the rollback re-asked: the deletion the re-asks are
    // about is the SAME one, still in progress.
    assert_eq!(
        events
            .iter()
            .filter(|event| event.logical_id.as_deref() == Some("Bucket")
                && event.status.as_deref() == Some("DELETE_IN_PROGRESS"))
            .count(),
        1,
        "case \"a rollback that keeps re-asking\": the bucket's deletion is opened once, got \
         {events:?}"
    );
    // And one terminal, from the attempt nothing follows: an intermediate refusal was not
    // that resource's outcome.
    assert_eq!(
        events
            .iter()
            .filter(|event| event.logical_id.as_deref() == Some("Bucket")
                && event.status.as_deref() == Some("DELETE_FAILED"))
            .count(),
        1,
        "case \"a rollback that keeps re-asking\": the bucket reports one DELETE_FAILED, got \
         {events:?}"
    );
    let index = position(&events, "Bucket", "DELETE_FAILED");
    let reason = events[index.unwrap_or_default()]
        .status_reason
        .clone()
        .unwrap_or_default();
    reported_failure(
        "a rollback that keeps re-asking",
        "Bucket",
        &reason,
    );
    assert!(
        position(&events, "Bucket", "DELETE_COMPLETE").is_none(),
        "case \"a rollback that keeps re-asking\": nothing deleted the bucket, so it never \
         completed, got {events:?}"
    );
    // The object the bucket held is still there: the refusal was real.
    let read = s3(&router, "GET", "/events-held-bucket/held/object.txt", b"").await;
    assert_eq!(
        read.status, 200,
        "case \"a rollback that keeps re-asking\": the bucket's object must survive, got {} \
         {}",
        read.status, read.body
    );
}
