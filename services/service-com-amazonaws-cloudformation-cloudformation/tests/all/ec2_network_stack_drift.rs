// SPDX-License-Identifier: CC0-1.0

//! A deployed Amazon EC2 network, the drift an out-of-band attribute change leaves on ONE of
//! its subnets, and the template the stack keeps holding — end to end over each service's own
//! wire.
//!
//! **The rule under test.** A stack whose resources are EC2 networking types settles to a
//! TERMINAL status, and a drift detection over it settles to a terminal
//! `DetectionStatus`: every covered resource is read back from the service that OWNS it and
//! compared with the properties the template declared, so a resource that service reports as
//! declared is `IN_SYNC` and only one it reports differently is `MODIFIED`, carrying one
//! `PropertyDifference` per declared leaf the two sides disagree about — the leaf's path,
//! the template's value, the service's value and `NOT_EQUAL`. Putting the resource back the
//! way the template declares it makes the next detection report that resource `IN_SYNC` and
//! the stack `IN_SYNC` again, so drift is read off the owning service on every check rather
//! than latched; and neither a detection nor a drift read changes the template
//! CloudFormation holds, which is what a template diff reads.
//!
//! The rows differ along what the rule generalises over rather than repeating one recorded
//! case: WHICH subnet attribute is changed outside the stack, in WHICH direction (a declared
//! `true` turned off and a declared `false` turned on), and a deployment nothing touched at
//! all. The attributes are the three boolean ones that are both properties of
//! `AWS::EC2::Subnet` and members of `ModifySubnetAttribute`
//! (`smithy_query(query="operation",operation="ModifySubnetAttribute")` against that type's
//! property list: `MapPublicIpOnLaunch`, `EnableDns64`, `AssignIpv6AddressOnCreation`), each
//! reported back by `DescribeSubnets` under the same name — so each row's change is one a
//! client can really make and really see.
//!
//! Everything goes through the one router `stack_lifecycle.rs` builds: CloudFormation over
//! awsQuery, Amazon EC2 over its own ec2Query wire.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, ec2, list_members, member, router, xml_text,
};
use crate::stack_owned_secret_teardown::poll_to_terminal;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    drift::difference_type, drift_detection_status, resource_drift_status, DRIFT_STATUS_DRIFTED,
    DRIFT_STATUS_IN_SYNC,
};

/// The logical ids of the two public subnets every row declares. The change is made to the
/// FIRST one, so the second is what says a detection reports the one resource that differs
/// rather than every resource of its type.
const SUBNET_ONE: &str = "PublicSubnetOne";
const SUBNET_TWO: &str = "PublicSubnetTwo";

/// Every logical id a row's template declares, in the order the template writes them.
const DECLARED_RESOURCES: &[&str] = &[
    "Vpc",
    SUBNET_ONE,
    SUBNET_TWO,
    "Gateway",
    "GatewayAttachment",
    "PublicRouteTable",
];

/// How long the poll below waits for the conclusion a detection owes. Generous on purpose:
/// what the case asserts is that the outcome ARRIVES, so the deadline only has to be long
/// enough that a loaded host cannot fail it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long one poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One deployment, and the change made to it outside the stack.
struct DriftRow {
    case: &'static str,
    stack: &'static str,
    /// The first two octets every address block of this row's network is written from, so
    /// each row's network is its own.
    network: &'static str,
    /// The subnet attribute the template declares and the out-of-band call changes: one
    /// property of `AWS::EC2::Subnet` that is also a member of `ModifySubnetAttribute`.
    attribute: &'static str,
    /// The value the template declares that attribute as, on BOTH subnets.
    declared: bool,
    /// What the attribute is set to on [`SUBNET_ONE`] outside the stack, or `None` for a
    /// deployment nothing touched.
    changed: Option<bool>,
}

const DRIFT_ROWS: &[DriftRow] = &[
    DriftRow {
        case: "the public-IP-on-launch switch turned off on one public subnet",
        stack: "ec2-drift-public-ip",
        network: "10.40",
        attribute: "MapPublicIpOnLaunch",
        declared: true,
        changed: Some(false),
    },
    DriftRow {
        case: "a declared-off DNS64 switch turned on on one public subnet",
        stack: "ec2-drift-dns64",
        network: "10.41",
        attribute: "EnableDns64",
        declared: false,
        changed: Some(true),
    },
    DriftRow {
        case: "the IPv6-address-on-creation switch turned off on one public subnet",
        stack: "ec2-drift-assign-ipv6",
        network: "10.42",
        attribute: "AssignIpv6AddressOnCreation",
        declared: true,
        changed: Some(false),
    },
    DriftRow {
        case: "a network nothing was changed on outside the stack",
        stack: "ec2-drift-untouched",
        network: "10.43",
        attribute: "MapPublicIpOnLaunch",
        declared: true,
        changed: None,
    },
];

/// The template one row deploys: a VPC, two public subnets carrying the row's declared
/// attribute, an internet gateway with its attachment, and a route table — the EC2
/// networking types a VPC application stack is made of.
fn network_template(row: &DriftRow) -> String {
    let DriftRow {
        network,
        attribute,
        declared,
        ..
    } = row;
    format!(
        r#"{{"Resources":{{
  "Vpc":{{"Type":"AWS::EC2::VPC","Properties":{{"CidrBlock":"{network}.0.0/16"}}}},
  "{SUBNET_ONE}":{{"Type":"AWS::EC2::Subnet","Properties":{{
    "VpcId":{{"Ref":"Vpc"}},"CidrBlock":"{network}.1.0/24",
    "AvailabilityZone":{{"Fn::Select":[0,{{"Fn::GetAZs":""}}]}},
    "{attribute}":{declared}}}}},
  "{SUBNET_TWO}":{{"Type":"AWS::EC2::Subnet","Properties":{{
    "VpcId":{{"Ref":"Vpc"}},"CidrBlock":"{network}.2.0/24",
    "AvailabilityZone":{{"Fn::Select":[1,{{"Fn::GetAZs":""}}]}},
    "{attribute}":{declared}}}}},
  "Gateway":{{"Type":"AWS::EC2::InternetGateway"}},
  "GatewayAttachment":{{"Type":"AWS::EC2::VPCGatewayAttachment","Properties":{{
    "VpcId":{{"Ref":"Vpc"}},"InternetGatewayId":{{"Ref":"Gateway"}}}}}},
  "PublicRouteTable":{{"Type":"AWS::EC2::RouteTable","Properties":{{
    "VpcId":{{"Ref":"Vpc"}}}}}}}}}}"#
    )
}

/// Deploy one row's template and hold it to a completed create.
///
/// The stack is POLLED to a status that is no longer in progress through the loop a sibling
/// suite already carries ([`poll_to_terminal`]), so this suite waits for the condition the
/// issue is about — a terminal status arriving — rather than carrying a second copy of that
/// loop.
async fn deploy_the_network(case: &str, router: &MultiServiceRouter, stack: &str, template: &str) {
    let (status, body) = create_stack(router, stack, template, &[]).await;
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
}

/// The physical resource id the stack publishes for one of its logical resources.
///
/// `pub(crate)`: a sibling suite that drives the SAME deployed network through its teardown
/// reads a resource's published id the same way, so it reads this one rather than carrying a
/// second copy of it.
pub(crate) async fn physical_id(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource({logical_id}) answered {status} with {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a physical id for {logical_id}, got {body}")
    })
}

/// An ec2Query answer spells a member of the `Subnet` it reports in lower camel case
/// (`<mapPublicIpOnLaunch>`), while the property a template declares it as — and this
/// suite's rows are written in — is that name with an upper-case initial.
fn reported_element(attribute: &str) -> String {
    let mut characters = attribute.chars();
    match characters.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + characters.as_str(),
        None => String::new(),
    }
}

/// What Amazon EC2 itself reports for one attribute of one subnet.
async fn subnet_attribute(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
    attribute: &str,
) -> String {
    let (status, body) = ec2(router, "DescribeSubnets", &[("SubnetId.1", subnet_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSubnets({subnet_id}) answered {status} with {body}"
    );
    xml_text(&body, &reported_element(attribute)).unwrap_or_else(|| {
        panic!("case {case:?}: expected EC2 to report {attribute} for {subnet_id}, got {body}")
    })
}

/// Set one boolean attribute of one subnet outside the stack, the way an operator does.
///
/// `pub(crate)`: shared with the sibling suite that makes the same out-of-band change before
/// a teardown.
pub(crate) async fn modify_subnet_attribute(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
    attribute: &str,
    value: bool,
) {
    let (status, body) = ec2(
        router,
        "ModifySubnetAttribute",
        &[
            ("SubnetId", subnet_id),
            (&format!("{attribute}.Value"), &value.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ModifySubnetAttribute({attribute}={value}) answered {status} with {body}"
    );
}

/// One drift detection over a stack, polled through its own status read until it is no longer
/// in progress: the `DetectionStatus`, the `StackDriftStatus` and the
/// `DriftedStackResourceCount` it settled on.
///
/// `pub(crate)`: the ONE drift-detection poll of this crate's suites, so a sibling suite that
/// detects drift before tearing a stack down waits for the same condition here rather than
/// carrying a second copy of the loop.
pub(crate) async fn settled_detection(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
) -> (String, String, String) {
    let (status, body) = call(router, "DetectStackDrift", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DetectStackDrift answered {status} with {body}"
    );
    let detection_id = xml_text(&body, "StackDriftDetectionId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a detection id, got {body}"));

    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = call(
            router,
            "DescribeStackDriftDetectionStatus",
            &[member("StackDriftDetectionId", &detection_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStackDriftDetectionStatus answered {status} with {body}"
        );
        let detection_status = xml_text(&body, "DetectionStatus").unwrap_or_default();
        if detection_status != drift_detection_status::DETECTION_IN_PROGRESS {
            return (
                detection_status,
                xml_text(&body, "StackDriftStatus").unwrap_or_default(),
                xml_text(&body, "DriftedStackResourceCount").unwrap_or_default(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the detection to conclude, it is still \
             {detection_status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One difference a report carries: the path, the expected value, the actual value and the
/// kind of difference.
pub(crate) type Difference = (String, String, String, String);

/// What one resource's record in a drift report says: its physical id, its drift status and
/// the differences it carries.
pub(crate) struct ReportedDrift {
    pub(crate) physical_resource_id: String,
    pub(crate) status: String,
    pub(crate) differences: Vec<Difference>,
}

/// `DescribeStackResourceDrifts` for one stack, as the record of each logical id it reports.
///
/// `pub(crate)`: shared with the sibling suite that reads the same report before a teardown.
pub(crate) async fn drift_report(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
) -> Vec<(String, ReportedDrift)> {
    let (status, body) = call(
        router,
        "DescribeStackResourceDrifts",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResourceDrifts answered {status} with {body}"
    );
    list_members(&body, "StackResourceDrifts")
        .into_iter()
        .map(|block| {
            let differences = list_members(&block, "PropertyDifferences")
                .into_iter()
                .map(|difference| {
                    (
                        xml_text(&difference, "PropertyPath").unwrap_or_default(),
                        xml_text(&difference, "ExpectedValue").unwrap_or_default(),
                        xml_text(&difference, "ActualValue").unwrap_or_default(),
                        xml_text(&difference, "DifferenceType").unwrap_or_default(),
                    )
                })
                .collect();
            (
                xml_text(&block, "LogicalResourceId").unwrap_or_default(),
                ReportedDrift {
                    physical_resource_id: xml_text(&block, "PhysicalResourceId")
                        .unwrap_or_default(),
                    status: xml_text(&block, "StackResourceDriftStatus").unwrap_or_default(),
                    differences,
                },
            )
        })
        .collect()
}

/// The record one logical id carries in a report.
pub(crate) fn record_of<'report>(
    case: &str,
    report: &'report [(String, ReportedDrift)],
    logical_id: &str,
) -> &'report ReportedDrift {
    report
        .iter()
        .find(|(reported, _)| reported == logical_id)
        .map(|(_, record)| record)
        .unwrap_or_else(|| {
            let reported: Vec<&str> = report.iter().map(|(id, _)| id.as_str()).collect();
            panic!("case {case:?}: expected a drift record for {logical_id}, got {reported:?}")
        })
}

/// The template body the stack holds, which is the side of a template diff CloudFormation
/// owns.
///
/// `pub(crate)`: shared with the sibling suite that asserts the same side is unmoved by what
/// happens to the resources.
pub(crate) async fn held_template(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (status, body) = call(router, "GetTemplate", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTemplate answered {status} with {body}"
    );
    xml_text(&body, "TemplateBody")
        .unwrap_or_else(|| panic!("case {case:?}: expected a template body, got {body}"))
}

/// Every resource a completed EC2 network stack declares is compared against the template on
/// a detection: the one subnet an attribute was changed on outside the stack is `MODIFIED`
/// with exactly that leaf's difference, every other resource is `IN_SYNC`, and the stack is
/// `DRIFTED` only while that one difference stands.
#[tokio::test]
async fn an_out_of_band_subnet_attribute_change_is_reported_as_that_subnets_drift() {
    let router = router();
    for row in DRIFT_ROWS {
        let case = row.case;
        deploy_the_network(case, &router, row.stack, &network_template(row)).await;

        // The template CloudFormation holds before anything reads the resources' drift: a
        // diff compares the local template against THIS, so it must not move.
        let held_before = held_template(case, &router, row.stack).await;

        let subnet_one = physical_id(case, &router, row.stack, SUBNET_ONE).await;
        let subnet_two = physical_id(case, &router, row.stack, SUBNET_TWO).await;
        // The deployment really applied the declared switch in the service that owns the
        // subnet, which is the expected side of every comparison below.
        for subnet in [&subnet_one, &subnet_two] {
            let reported = subnet_attribute(case, &router, subnet, row.attribute).await;
            assert_eq!(
                reported,
                row.declared.to_string(),
                "case {case:?}: expected EC2 to report {attribute}={declared} for {subnet}, got \
                 {reported:?}",
                attribute = row.attribute,
                declared = row.declared,
            );
        }

        if let Some(changed) = row.changed {
            modify_subnet_attribute(case, &router, &subnet_one, row.attribute, changed).await;
            let reported = subnet_attribute(case, &router, &subnet_one, row.attribute).await;
            assert_eq!(
                reported,
                changed.to_string(),
                "case {case:?}: expected the out-of-band change to stand in EC2, got {reported:?}"
            );
        }

        let (detection_status, stack_drift, drifted_count) =
            settled_detection(case, &router, row.stack).await;
        assert_eq!(
            detection_status,
            drift_detection_status::DETECTION_COMPLETE,
            "case {case:?}: expected the detection to complete, got {detection_status:?}"
        );
        let (expected_stack_drift, expected_count) = match row.changed {
            Some(_) => (DRIFT_STATUS_DRIFTED, "1"),
            None => (DRIFT_STATUS_IN_SYNC, "0"),
        };
        assert_eq!(
            stack_drift, expected_stack_drift,
            "case {case:?}: expected the stack to report {expected_stack_drift}, got \
             {stack_drift:?}"
        );
        assert_eq!(
            drifted_count, expected_count,
            "case {case:?}: expected {expected_count} drifted resource(s), got {drifted_count:?}"
        );

        let report = drift_report(case, &router, row.stack).await;
        // Every resource the template declares and an owning service reports is compared, so
        // the comparison the case asserts is not one the report left out.
        let reported_ids: Vec<&str> = report.iter().map(|(id, _)| id.as_str()).collect();
        for logical_id in [SUBNET_ONE, SUBNET_TWO, "Vpc"] {
            assert!(
                reported_ids.contains(&logical_id),
                "case {case:?}: expected {logical_id} to be checked, got {reported_ids:?}"
            );
        }
        for (logical_id, record) in &report {
            assert!(
                DECLARED_RESOURCES.contains(&logical_id.as_str()),
                "case {case:?}: expected only declared resources in the report, got \
                 {logical_id:?}"
            );
            let changed_subnet = logical_id == SUBNET_ONE && row.changed.is_some();
            if !changed_subnet {
                assert_eq!(
                    record.status,
                    resource_drift_status::IN_SYNC,
                    "case {case:?}: expected {logical_id} to match its declaration, got {status:?} \
                     with {differences:?}",
                    status = record.status,
                    differences = record.differences,
                );
                assert!(
                    record.differences.is_empty(),
                    "case {case:?}: expected {logical_id} to differ in nothing, got \
                     {differences:?}",
                    differences = record.differences,
                );
            }
        }

        if let Some(changed) = row.changed {
            let record = record_of(case, &report, SUBNET_ONE);
            assert_eq!(
                record.status,
                resource_drift_status::MODIFIED,
                "case {case:?}: expected the changed subnet to be MODIFIED, got {status:?}",
                status = record.status,
            );
            assert_eq!(
                record.physical_resource_id, subnet_one,
                "case {case:?}: expected the record to name the changed subnet, got {id:?}",
                id = record.physical_resource_id,
            );
            let expected: Vec<Difference> = vec![(
                format!("/{}", row.attribute),
                row.declared.to_string(),
                changed.to_string(),
                difference_type::NOT_EQUAL.to_string(),
            )];
            assert_eq!(
                record.differences, expected,
                "case {case:?}: expected exactly {expected:?}, got {differences:?}",
                differences = record.differences,
            );

            // Put the attribute back the way the template declares it: the next detection
            // reads the owning service again, so the same subnet is in sync and so is the
            // stack.
            modify_subnet_attribute(case, &router, &subnet_one, row.attribute, row.declared)
                .await;
            let (detection_status, stack_drift, drifted_count) =
                settled_detection(case, &router, row.stack).await;
            assert_eq!(
                detection_status,
                drift_detection_status::DETECTION_COMPLETE,
                "case {case:?}: expected the second detection to complete, got \
                 {detection_status:?}"
            );
            assert_eq!(
                stack_drift, DRIFT_STATUS_IN_SYNC,
                "case {case:?}: expected the restored stack to report IN_SYNC, got \
                 {stack_drift:?}"
            );
            assert_eq!(
                drifted_count, "0",
                "case {case:?}: expected no drifted resource once restored, got \
                 {drifted_count:?}"
            );
            let restored = drift_report(case, &router, row.stack).await;
            let record = record_of(case, &restored, SUBNET_ONE);
            assert_eq!(
                record.status,
                resource_drift_status::IN_SYNC,
                "case {case:?}: expected the restored subnet to be IN_SYNC, got {status:?} with \
                 {differences:?}",
                status = record.status,
                differences = record.differences,
            );
            assert!(
                record.differences.is_empty(),
                "case {case:?}: expected the restored subnet to differ in nothing, got \
                 {differences:?}",
                differences = record.differences,
            );
        }

        // A detection and a drift read are READS of the resources: the template the stack
        // holds is the one it was deployed with, byte for byte.
        let held_after = held_template(case, &router, row.stack).await;
        assert_eq!(
            held_after, held_before,
            "case {case:?}: expected the held template to be unchanged by the drift reads"
        );
    }
}
