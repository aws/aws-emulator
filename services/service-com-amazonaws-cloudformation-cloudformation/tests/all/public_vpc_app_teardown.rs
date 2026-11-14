// SPDX-License-Identifier: CC0-1.0

//! The whole life of the public two-Availability-Zone network a VPC application deploys: the
//! create, what an operator does to the real resources afterwards, the two surfaces that
//! answer for it, and the teardown that has to END.
//!
//! **The rule under test.** A stack whose resources are EC2 networking types reaches a
//! TERMINAL status on BOTH of its operations from the clock alone — the create on
//! `CREATE_COMPLETE`, with every declared resource created in the service that owns it and
//! the stack's `Outputs` carrying the identifiers that service assigned, and the delete on
//! `DELETE_COMPLETE`, whatever state a change made OUTSIDE the stack has left those resources
//! in: one changed outside the stack is deleted anyway, and one the owning service no longer
//! holds is the state the delete was asking for. And the two surfaces that answer for such a
//! stack are independent: a mutation of a real resource moves the template CloudFormation
//! holds in no way at all — that template is the side a template diff compares a locally
//! synthesized one against — while a drift detection over the same stack reports it, so
//! neither a detection nor the drift it reports changes what the next operation does.
//!
//! The arm of that rule nothing else here pins is the END: a client that deploys a network,
//! changes one of its resources and then deletes the stack polls the deletion until it is no
//! longer in progress, so a teardown left short of a terminal status is a poll that never
//! stops — whatever the out-of-band world did to the resources beforehand.
//! `ec2_network_stack_drift.rs` pins what a detection over such a network REPORTS (and the
//! helpers for reading it are shared from there rather than copied); `vpc_network_teardown.rs`
//! pins the ORDER a teardown asks the owning service in. The axis left, and the one these rows
//! differ along, is WHAT HAPPENED OUTSIDE THE STACK before the teardown: nothing, a property
//! of a live resource changed, a resource of the stack already taken away, and an attachment
//! the stack made already undone — with and without a drift detection in between.
//!
//! Everything goes through the one router `stack_lifecycle.rs` builds: CloudFormation over
//! awsQuery, Amazon EC2 over its own ec2Query wire.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift::difference_type, drift_detection_status, resource_drift_status, DRIFT_STATUS_DRIFTED,
    DRIFT_STATUS_IN_SYNC,
};
use service_com_amazonaws_ec2_amazonec2::state::{
    INVALID_INTERNET_GATEWAY_ID_NOT_FOUND, INVALID_SUBNET_ID_NOT_FOUND, INVALID_VPC_ID_NOT_FOUND,
};

use crate::ec2_network_stack_drift::{
    drift_report, held_template, modify_subnet_attribute, physical_id, record_of,
    settled_detection, Difference,
};
use crate::stack_lifecycle::{
    call, create_stack, ec2, is_service_assigned_id, member, output_value, router, xml_text,
};
use crate::stack_owned_secret_teardown::poll_to_terminal;

/// The logical ids the template below declares, as each row addresses them.
const VPC: &str = "Vpc";
const PUBLIC_SUBNET_ONE: &str = "PublicSubnetOne";
const GATEWAY: &str = "Gateway";
const PUBLIC_ROUTE_TABLE: &str = "PublicRouteTable";

/// The subnet property this application declares its public subnets with, and the value it
/// declares it as: "Indicates whether instances launched in this subnet receive a public IPv4
/// address" — a property of `AWS::EC2::Subnet` that is also a member of
/// `ModifySubnetAttribute`, so it is one an operator can really change out of band and one
/// `DescribeSubnets` reports back.
const PUBLIC_IP_ATTRIBUTE: &str = "MapPublicIpOnLaunch";
const DECLARED_PUBLIC_IP: bool = true;

/// The destination the public route table's default route matches on.
const ALL_IPV4: &str = "0.0.0.0/0";

/// The outputs this application publishes, which are what a deploy writes to its outputs
/// file: the VPC's id and the public subnets' ids.
const VPC_ID_OUTPUT: &str = "VpcId";
const PUBLIC_SUBNET_IDS_OUTPUT: &str = "PublicSubnetIds";

/// How many public subnets the application declares — one per Availability Zone it spans.
const PUBLIC_SUBNETS: usize = 2;

/// The template every row deploys: the resource set a VPC application with two public
/// Availability Zones and no NAT is made of — the VPC, one public subnet per zone with the
/// public-IP-on-launch switch on, the internet gateway and the attachment that puts it on the
/// VPC, the public route table, its default route out through that gateway, and the
/// association that puts each subnet on that table.
const PUBLIC_VPC_APP: &str = r#"{"Resources":{
  "Vpc":{"Type":"AWS::EC2::VPC","Properties":{
    "CidrBlock":"10.60.0.0/16","EnableDnsSupport":true,"EnableDnsHostnames":true}},
  "PublicSubnetOne":{"Type":"AWS::EC2::Subnet","Properties":{
    "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.60.0.0/18",
    "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]},
    "MapPublicIpOnLaunch":true}},
  "PublicSubnetTwo":{"Type":"AWS::EC2::Subnet","Properties":{
    "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.60.64.0/18",
    "AvailabilityZone":{"Fn::Select":[1,{"Fn::GetAZs":""}]},
    "MapPublicIpOnLaunch":true}},
  "Gateway":{"Type":"AWS::EC2::InternetGateway"},
  "GatewayAttachment":{"Type":"AWS::EC2::VPCGatewayAttachment","Properties":{
    "VpcId":{"Ref":"Vpc"},"InternetGatewayId":{"Ref":"Gateway"}}},
  "PublicRouteTable":{"Type":"AWS::EC2::RouteTable","Properties":{
    "VpcId":{"Ref":"Vpc"}}},
  "PublicDefaultRoute":{"Type":"AWS::EC2::Route","DependsOn":"GatewayAttachment",
    "Properties":{
      "RouteTableId":{"Ref":"PublicRouteTable"},"DestinationCidrBlock":"0.0.0.0/0",
      "GatewayId":{"Ref":"Gateway"}}},
  "PublicSubnetOneAssociation":{"Type":"AWS::EC2::SubnetRouteTableAssociation","Properties":{
    "SubnetId":{"Ref":"PublicSubnetOne"},"RouteTableId":{"Ref":"PublicRouteTable"}}},
  "PublicSubnetTwoAssociation":{"Type":"AWS::EC2::SubnetRouteTableAssociation","Properties":{
    "SubnetId":{"Ref":"PublicSubnetTwo"},"RouteTableId":{"Ref":"PublicRouteTable"}}}},
  "Outputs":{
    "VpcId":{"Value":{"Ref":"Vpc"}},
    "PublicSubnetIds":{"Value":{"Fn::Join":[",",
      [{"Ref":"PublicSubnetOne"},{"Ref":"PublicSubnetTwo"}]]}}}}"#;

/// What an operator did to the deployed network OUTSIDE the stack, before its teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutsideTheStack {
    /// Nothing at all: the baseline the other rows are read against.
    Nothing,
    /// The public-IP-on-launch switch turned off on the first public subnet — a live resource
    /// whose configuration no longer matches the declaration.
    SubnetAttributeChanged,
    /// The default route taken out of the public route table: a resource the teardown asks
    /// Amazon EC2 to delete is already gone.
    DefaultRouteDeleted,
    /// The internet gateway detached from the VPC: the attachment the teardown asks Amazon
    /// EC2 to undo is already undone.
    GatewayDetached,
}

/// One deployment, what happened to it outside the stack, and whether a drift detection ran
/// before it was torn down.
struct TeardownRow {
    case: &'static str,
    stack: &'static str,
    outside: OutsideTheStack,
    /// Whether the row runs a drift detection between the out-of-band change and the delete.
    detect: bool,
}

const TEARDOWN_ROWS: &[TeardownRow] = &[
    TeardownRow {
        case: "a network nothing touched, detected and then torn down",
        stack: "public-vpc-app-untouched",
        outside: OutsideTheStack::Nothing,
        detect: true,
    },
    TeardownRow {
        case: "a network whose public subnet was turned private out of band, detected and \
               then torn down",
        stack: "public-vpc-app-drifted-detected",
        outside: OutsideTheStack::SubnetAttributeChanged,
        detect: true,
    },
    TeardownRow {
        case: "a network whose public subnet was turned private out of band and torn down \
               without a detection",
        stack: "public-vpc-app-drifted-undetected",
        outside: OutsideTheStack::SubnetAttributeChanged,
        detect: false,
    },
    TeardownRow {
        case: "a network whose default route was deleted out of band",
        stack: "public-vpc-app-route-gone",
        outside: OutsideTheStack::DefaultRouteDeleted,
        detect: false,
    },
    TeardownRow {
        case: "a network whose internet gateway was detached out of band",
        stack: "public-vpc-app-gateway-detached",
        outside: OutsideTheStack::GatewayDetached,
        detect: false,
    },
];

/// Deploy the application and hold it to a completed create, answering the stack id the
/// deletion is read back by — a name leaves the live set with the stack, the id does not.
///
/// The create is POLLED to a status that is no longer in progress, so this waits for the
/// CONDITION the rule is about rather than for a length of time.
async fn deploy_the_application(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (status, body) = create_stack(router, stack, PUBLIC_VPC_APP, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack answered {status} with {body}"
    );
    let described = poll_to_terminal(router, case, stack).await;
    let settled = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        settled, "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {settled:?} from {described}"
    );
    let stack_id = xml_text(&described, "StackId").unwrap_or_default();
    assert!(
        !stack_id.is_empty(),
        "case {case:?}: expected the stack to report its id, got {described}"
    );
    stack_id
}

/// The ids the completed application published: its VPC's, and its public subnets' in
/// declaration order — the values a deploy writes to its outputs file.
async fn published_ids(case: &str, router: &MultiServiceRouter, stack: &str) -> (String, Vec<String>) {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks answered {status} with {body}"
    );
    let vpc_id = output_value(&body, VPC_ID_OUTPUT).unwrap_or_default();
    assert!(
        is_service_assigned_id(&vpc_id, "vpc"),
        "case {case:?}: expected the {VPC_ID_OUTPUT} output to carry the id EC2 assigned, got \
         {vpc_id:?} from {body}"
    );
    let joined = output_value(&body, PUBLIC_SUBNET_IDS_OUTPUT).unwrap_or_default();
    let subnets: Vec<String> = joined
        .split(',')
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        subnets.len(),
        PUBLIC_SUBNETS,
        "case {case:?}: expected {PUBLIC_SUBNETS} public subnet id(s) in the \
         {PUBLIC_SUBNET_IDS_OUTPUT} output, got {joined:?}"
    );
    for subnet_id in &subnets {
        assert!(
            is_service_assigned_id(subnet_id, "subnet"),
            "case {case:?}: expected each published subnet id to be one EC2 assigned, got \
             {subnet_id:?}"
        );
    }
    (vpc_id, subnets)
}

/// Make one row's change to the deployed resources, outside the stack.
async fn change_it_outside_the_stack(
    case: &str,
    router: &MultiServiceRouter,
    row: &TeardownRow,
    subnets: &[String],
) {
    match row.outside {
        OutsideTheStack::Nothing => {}
        OutsideTheStack::SubnetAttributeChanged => {
            modify_subnet_attribute(
                case,
                router,
                &subnets[0],
                PUBLIC_IP_ATTRIBUTE,
                !DECLARED_PUBLIC_IP,
            )
            .await;
        }
        OutsideTheStack::DefaultRouteDeleted => {
            let table = physical_id(case, router, row.stack, PUBLIC_ROUTE_TABLE).await;
            let (status, body) = ec2(
                router,
                "DeleteRoute",
                &[
                    ("RouteTableId", table.as_str()),
                    ("DestinationCidrBlock", ALL_IPV4),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteRoute({table}, {ALL_IPV4}) answered {status} with {body}"
            );
        }
        OutsideTheStack::GatewayDetached => {
            let gateway = physical_id(case, router, row.stack, GATEWAY).await;
            let vpc = physical_id(case, router, row.stack, VPC).await;
            let (status, body) = ec2(
                router,
                "DetachInternetGateway",
                &[
                    ("InternetGatewayId", gateway.as_str()),
                    ("VpcId", vpc.as_str()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DetachInternetGateway({gateway}, {vpc}) answered {status} with \
                 {body}"
            );
        }
    }
}

/// Run one drift detection over the stack and assert what it concluded: a row whose change
/// left a live resource differing from its declaration is `DRIFTED` at exactly that leaf,
/// every other row is `IN_SYNC`.
async fn assert_the_detection(case: &str, router: &MultiServiceRouter, row: &TeardownRow) {
    let (detection_status, stack_drift, drifted_count) =
        settled_detection(case, router, row.stack).await;
    assert_eq!(
        detection_status,
        drift_detection_status::DETECTION_COMPLETE,
        "case {case:?}: expected the detection to complete, got {detection_status:?}"
    );
    let drifted = row.outside == OutsideTheStack::SubnetAttributeChanged;
    let (expected_drift, expected_count) = if drifted {
        (DRIFT_STATUS_DRIFTED, "1")
    } else {
        (DRIFT_STATUS_IN_SYNC, "0")
    };
    assert_eq!(
        stack_drift, expected_drift,
        "case {case:?}: expected the stack to report {expected_drift}, got {stack_drift:?}"
    );
    assert_eq!(
        drifted_count, expected_count,
        "case {case:?}: expected {expected_count} drifted resource(s), got {drifted_count:?}"
    );
    if !drifted {
        return;
    }
    // The property path, the two values and the kind of difference an operator acts on: the
    // declaration's side, the service's side, and that the two are not equal.
    let report = drift_report(case, router, row.stack).await;
    let record = record_of(case, &report, PUBLIC_SUBNET_ONE);
    assert_eq!(
        record.status,
        resource_drift_status::MODIFIED,
        "case {case:?}: expected the changed subnet to be MODIFIED, got {status:?}",
        status = record.status,
    );
    let expected: Vec<Difference> = vec![(
        format!("/{PUBLIC_IP_ATTRIBUTE}"),
        DECLARED_PUBLIC_IP.to_string(),
        (!DECLARED_PUBLIC_IP).to_string(),
        difference_type::NOT_EQUAL.to_string(),
    )];
    assert_eq!(
        record.differences, expected,
        "case {case:?}: expected exactly {expected:?}, got {differences:?}",
        differences = record.differences,
    );
}

/// What Amazon EC2 answers for one resource id once the stack that created it is gone.
async fn assert_gone(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    parameter: &str,
    id: &str,
    code: &str,
) {
    let (status, refused) = ec2(router, action, &[(parameter, id)]).await;
    assert_eq!(
        (status, xml_text(&refused, "Code")),
        (400, Some(code.to_string())),
        "case {case:?}: expected {action} on the deleted {id} to be refused with {code}, got \
         {status} {refused}"
    );
}

/// The whole life of the application: a create that completes, a change made to the real
/// resources outside the stack that the held template is blind to and a detection reports, and
/// a delete that REACHES `DELETE_COMPLETE` and takes the resources away in Amazon EC2.
#[tokio::test]
async fn a_public_vpc_application_completes_its_create_and_its_delete_whatever_changed_outside_it()
{
    for row in TEARDOWN_ROWS {
        let case = row.case;
        let router = router();
        let stack_id = deploy_the_application(case, &router, row.stack).await;
        let (vpc_id, subnets) = published_ids(case, &router, row.stack).await;
        let gateway_id = physical_id(case, &router, row.stack, GATEWAY).await;

        // The side of a template diff CloudFormation owns, before anything touches the
        // resources.
        let held_before = held_template(case, &router, row.stack).await;

        change_it_outside_the_stack(case, &router, row, &subnets).await;

        // A mutation of a REAL resource is not a change to the template the stack holds, so a
        // diff against a locally synthesized template sees nothing of it.
        let held_after_the_change = held_template(case, &router, row.stack).await;
        assert_eq!(
            held_after_the_change, held_before,
            "case {case:?}: expected the held template to be unmoved by the out-of-band change"
        );

        if row.detect {
            assert_the_detection(case, &router, row).await;
            // ...and a detection is a READ of those resources: it moves that template no more
            // than the change itself did.
            let held_after_the_detection = held_template(case, &router, row.stack).await;
            assert_eq!(
                held_after_the_detection, held_before,
                "case {case:?}: expected the held template to be unmoved by the detection"
            );
        }

        // The arm this suite is for: the deletion REACHES a terminal status, whatever the
        // out-of-band world left the resources in, so a client's poll stops.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStack answered {status} with {body}"
        );
        let described = poll_to_terminal(&router, case, &stack_id).await;
        let settled = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            settled, "DELETE_COMPLETE",
            "case {case:?}: expected the teardown to reach DELETE_COMPLETE, got {settled:?} \
             from {described}"
        );
        // Terminal means it STAYS: a second read reports the same conclusion rather than
        // moving back into progress.
        let (status, again) = call(&router, "DescribeStacks", &[member("StackName", &stack_id)]).await;
        assert_eq!(
            (status, xml_text(&again, "StackStatus")),
            (200, Some("DELETE_COMPLETE".to_string())),
            "case {case:?}: expected the terminal status to stand on the next read, got \
             {status} {again}"
        );

        // ...and what the teardown completed means: Amazon EC2 holds none of the resources the
        // application created, each refused with that resource's own not-found code.
        for subnet_id in &subnets {
            assert_gone(
                case,
                &router,
                "DescribeSubnets",
                "SubnetId.1",
                subnet_id,
                INVALID_SUBNET_ID_NOT_FOUND,
            )
            .await;
        }
        assert_gone(
            case,
            &router,
            "DescribeVpcs",
            "VpcId.1",
            &vpc_id,
            INVALID_VPC_ID_NOT_FOUND,
        )
        .await;
        assert_gone(
            case,
            &router,
            "DescribeInternetGateways",
            "InternetGatewayId.1",
            &gateway_id,
            INVALID_INTERNET_GATEWAY_ID_NOT_FOUND,
        )
        .await;
    }
}
