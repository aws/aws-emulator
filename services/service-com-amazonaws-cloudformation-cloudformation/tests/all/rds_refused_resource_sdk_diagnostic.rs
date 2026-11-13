// SPDX-License-Identifier: CC0-1.0

//! What a resource AMAZON RDS refused reports on its `CREATE_FAILED` event, end to end over
//! the awsQuery wire.
//!
//! RULE under test: **a resource provider reports a refused call as the exception its SDK
//! raised — the owning service's own sentence followed by that SDK's diagnostic of the call,
//! naming the service as its SDK names it and the HTTP status that service answered beside
//! the id of the call — so the failure is attributed to the service that refused it, whatever
//! type of that service failed and whichever of its calls was refused. A service whose
//! reported wording this emulator has no record of has no diagnostic invented for it.**
//!
//! Why it matters: a client reading a rolled-back deployment diagnoses it from the failed
//! resource's `ResourceStatusReason`. Without the attribution it cannot tell a refusal the
//! owning service answered from CloudFormation's own validation of the declaration, so a
//! template whose real fault is the thing that service named — too few Availability Zones
//! across a DB subnet group's subnets — reads as a fault in CloudFormation's handling of it,
//! and a refusal-taxonomy check that keys on the originating service cannot see it at all.
//!
//! The rows differ along what the rule generalises over: WHICH type of the owning service
//! failed (a DB subnet group and a DB instance), which call of it was refused, and the HTTP
//! STATUS that service answered (400 for a declaration it will not take, 404 for a resource
//! it does not hold — each from the fault's own `httpError` in RDS's model,
//! `smithy_query(query="operation",operation="CreateDBSubnetGroup,CreateDBInstance")`:
//! `DBSubnetGroupDoesNotCoverEnoughAZs` and `InvalidSubnet` are 400,
//! `DBSubnetGroupNotFoundFault` is 404). The control row is a type owned by a service this
//! emulator has recorded no SDK wording for, whose refusal is therefore reported exactly as
//! that service worded it.

use std::time::Duration;

use crate::stack_lifecycle::{
    create_stack, resource_event_reason, router, wait_for_the_transition, REGION,
};

/// The SDK's own name for Amazon RDS, as the exception its SDK raises states it.
const RDS_SERVICE: &str = "Rds";

/// How long a row waits for the failed resource's event to be there before giving up.
///
/// Generous on purpose: the only thing being waited FOR is the stack reaching its terminal
/// status, which is on the stack's own clock, and the wait ends as soon as the event is
/// readable rather than after a fixed time.
const EVENT_DEADLINE: Duration = Duration::from_secs(60);

/// How long one look-again waits before asking the event stream once more.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// One row: the stack to deploy, the resource the owning service refuses, and what its
/// reason must report.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    logical_id: &'static str,
    /// The service name and HTTP status the SDK diagnostic must state, or `None` for a row
    /// whose owning service has no recorded SDK wording — which must then carry no
    /// diagnostic at all.
    diagnostic: Option<(&'static str, u16)>,
    /// A fragment of the owning service's OWN sentence the quoted message must open with, or
    /// `None` for a row whose only subject is the diagnostic FORM: the sentence that service
    /// words for the refusal is then not what this suite is about, so it is not frozen here.
    sentence: Option<&'static str>,
    /// The `HandlerErrorCode` class the refusal falls in.
    class: &'static str,
}

/// A VPC, two subnets pinned to the SAME Availability Zone and a DB subnet group over both:
/// the group is created in Amazon RDS, which counts the zones its subnets lie in and refuses
/// a group that covers fewer than the minimum.
const SINGLE_ZONE_TEMPLATE: &str = r#"{"Resources":{
    "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
    "SubnetA":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.1.0/24",
      "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]}}},
    "SubnetB":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.0.2.0/24",
      "AvailabilityZone":{"Fn::Select":[0,{"Fn::GetAZs":""}]}}},
    "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
      "DBSubnetGroupName":"rds-diag-single-zone",
      "DBSubnetGroupDescription":"two subnets of one zone",
      "SubnetIds":[{"Ref":"SubnetA"},{"Ref":"SubnetB"}]}}}}"#;

/// The same type over subnet ids of the right FORM that no subnet of the account answers to,
/// which that service refuses for a different reason — and with a different error code — at
/// the same status.
const ABSENT_SUBNETS_TEMPLATE: &str = r#"{"Resources":{
    "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
      "DBSubnetGroupName":"rds-diag-absent-subnets",
      "DBSubnetGroupDescription":"a group over ids no subnet answers to",
      "SubnetIds":["subnet-0123456789abcdef0","subnet-0fedcba9876543210"]}}}}"#;

/// And the same type over values that are no subnet id at all, which that service refuses on
/// the FORM of the ids before anything about the subnets they would name.
const MALFORMED_SUBNETS_TEMPLATE: &str = r#"{"Resources":{
    "SubnetGroup":{"Type":"AWS::RDS::DBSubnetGroup","Properties":{
      "DBSubnetGroupName":"rds-diag-malformed-subnets",
      "DBSubnetGroupDescription":"a group over values that name no subnet",
      "SubnetIds":["not-a-subnet-id","also-not-a-subnet-id"]}}}}"#;

/// Another of that service's types, whose create names a DB subnet group Amazon RDS does not
/// hold: the same service, a different call, and the status it answers for a resource that is
/// not there.
const ABSENT_GROUP_TEMPLATE: &str = r#"{"Resources":{
    "Database":{"Type":"AWS::RDS::DBInstance","Properties":{
      "DBInstanceIdentifier":"rds-diag-absent-group-db",
      "Engine":"mysql","DBInstanceClass":"db.t3.micro",
      "DBSubnetGroupName":"rds-diag-no-such-group"}}}}"#;

/// The control row's template: a subnet in a VPC Amazon EC2 holds nothing for. EC2 refuses
/// the create, and this emulator has no record of how that service's SDK words a diagnostic,
/// so the reason carries that service's sentence and nothing more.
const ABSENT_VPC_TEMPLATE: &str = r#"{"Resources":{
    "Subnet":{"Type":"AWS::EC2::Subnet","Properties":{
      "VpcId":"vpc-0123456789abcdef0","CidrBlock":"10.0.1.0/24"}}}}"#;

/// The class a refusal of the REQUEST falls in, and the class of one about a resource that is
/// not there — both values of the model's `HandlerErrorCode` enum.
const INVALID_REQUEST: &str = "InvalidRequest";
const NOT_FOUND: &str = "NotFound";

const ROWS: &[Row] = &[
    Row {
        case: "a DB subnet group whose subnets cover one Availability Zone",
        stack: "rds-diag-single-zone",
        template: SINGLE_ZONE_TEMPLATE,
        logical_id: "SubnetGroup",
        diagnostic: Some((RDS_SERVICE, 400)),
        sentence: Some("Availability Zone (AZ) coverage requirement"),
        class: INVALID_REQUEST,
    },
    Row {
        case: "a DB subnet group over subnet ids no subnet answers to",
        stack: "rds-diag-absent-subnets",
        template: ABSENT_SUBNETS_TEMPLATE,
        logical_id: "SubnetGroup",
        diagnostic: Some((RDS_SERVICE, 400)),
        sentence: Some("are invalid."),
        class: INVALID_REQUEST,
    },
    Row {
        case: "a DB subnet group over values that are no subnet id at all",
        stack: "rds-diag-malformed-subnets",
        template: MALFORMED_SUBNETS_TEMPLATE,
        logical_id: "SubnetGroup",
        diagnostic: Some((RDS_SERVICE, 400)),
        // Only the diagnostic: how the owning service words a refusal of the id's FORM is
        // that service's own concern, and not something this suite should pin.
        sentence: None,
        class: INVALID_REQUEST,
    },
    Row {
        case: "a DB instance naming a DB subnet group the service does not hold",
        stack: "rds-diag-absent-group",
        template: ABSENT_GROUP_TEMPLATE,
        logical_id: "Database",
        diagnostic: Some((RDS_SERVICE, 404)),
        sentence: Some("rds-diag-no-such-group"),
        class: NOT_FOUND,
    },
    Row {
        case: "a subnet in a VPC the owning service does not hold",
        stack: "rds-diag-absent-vpc",
        template: ABSENT_VPC_TEMPLATE,
        logical_id: "Subnet",
        diagnostic: None,
        sentence: Some("vpc-0123456789abcdef0"),
        class: NOT_FOUND,
    },
];

/// The quoted provider message and the reported `HandlerErrorCode` of one
/// `ResourceStatusReason`.
fn quoted_and_class(case: &str, reason: &str) -> (String, String) {
    const OPENS: &str = "Resource handler returned message: \"";
    const TOKEN: &str = "\" (RequestToken: ";
    const CLASS: &str = "HandlerErrorCode: ";
    let rest = reason.strip_prefix(OPENS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the reason to open with {OPENS:?}, got {reason:?}")
    });
    let (quoted, tail) = rest.split_once(TOKEN).unwrap_or_else(|| {
        panic!("case {case:?}: expected a quoted message and a RequestToken, got {reason:?}")
    });
    let class = tail
        .split_once(CLASS)
        .and_then(|(_, class)| class.strip_suffix(')'))
        .unwrap_or_else(|| panic!("case {case:?}: expected a HandlerErrorCode, got {reason:?}"));
    (quoted.to_string(), class.to_string())
}

/// Hold one quoted exception to the diagnostic the owning service's SDK appends: the sentence
/// that service worded, then the service, the status and the id of the call — and nothing
/// beyond the call itself.
fn assert_diagnostic(case: &str, quoted: &str, service: &str, status: u16) {
    const OPENS: &str = " (Service: ";
    let (sentence, diagnostic) = quoted.rsplit_once(OPENS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the SDK diagnostic after the service's own sentence, got {quoted:?}")
    });
    assert!(
        !sentence.trim().is_empty(),
        "case {case:?}: expected the service's own sentence ahead of the diagnostic, got \
         {quoted:?}"
    );
    let envelope = diagnostic.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case:?}: expected the diagnostic to close, got {quoted:?}")
    });
    let fields: Vec<&str> = envelope.split(", ").collect();
    assert_eq!(
        fields.len(),
        3,
        "case {case:?}: expected the service, the status and the call's id and nothing else, \
         got {envelope:?}"
    );
    assert_eq!(
        fields[0], service,
        "case {case:?}: expected the SDK's own name for the service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1],
        format!("Status Code: {status}"),
        "case {case:?}: expected the status the owning service answered, got {:?}",
        fields[1]
    );
    let request_id = fields[2]
        .strip_prefix("Request ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Request ID, got {:?}", fields[2]));
    assert!(
        is_a_request_id(request_id),
        "case {case:?}: expected the id of the call in the form that service's answers carry, \
         got {request_id:?}"
    );
}

/// Whether `value` has the 8-4-4-4-12 lowercase-hexadecimal form an AWS request id carries.
fn is_a_request_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|held| held.is_ascii_hexdigit() && !held.is_ascii_uppercase()))
}

/// The `ResourceStatusReason` of `logical_id`'s `CREATE_FAILED` event, looked for again until
/// the stack's own clock has recorded it or [`EVENT_DEADLINE`] passes.
async fn failure_reason(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + EVENT_DEADLINE;
    loop {
        let reason = resource_event_reason(router, stack, logical_id, "CREATE_FAILED").await;
        if !reason.is_empty() {
            return reason;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: {logical_id} never recorded a CREATE_FAILED reason"
        );
        tokio::time::sleep(EVENT_POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_refused_resource_is_attributed_to_the_service_that_refused_it() {
    for row in ROWS {
        let router = router();
        let (status_code, created) = create_stack(&router, row.stack, row.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            row.case
        );
        // The create's own terminal status, and the rollback's: both are on the stack's clock.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let reason = failure_reason(&router, row.case, row.stack, row.logical_id).await;
        let (quoted, class) = quoted_and_class(row.case, &reason);
        if let Some(sentence) = row.sentence {
            assert!(
                quoted.contains(sentence),
                "case {:?}: expected the owning service's own sentence carrying {sentence:?}, \
                 got {quoted:?}",
                row.case,
            );
        }
        assert_eq!(
            class, row.class,
            "case {:?}: expected the {} class, got {class:?} in {reason:?}",
            row.case, row.class
        );
        match row.diagnostic {
            Some((service, status)) => assert_diagnostic(row.case, &quoted, service, status),
            None => assert!(
                !quoted.contains("(Service:"),
                "case {:?}: expected no diagnostic invented for a service none is recorded \
                 for, got {quoted:?}",
                row.case
            ),
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_zone_coverage_the_declared_subnets_add_up_to() {
    // The variable part of that service's sentence comes from the state of the subnets the
    // declaration named, not from the request: two subnets of one zone cover ONE zone, which
    // the refusal names.
    let case = "two subnets of one Availability Zone";
    let router = router();
    let (status_code, created) =
        create_stack(&router, "rds-diag-zone-named", SINGLE_ZONE_TEMPLATE, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let reason = failure_reason(&router, case, "rds-diag-zone-named", "SubnetGroup").await;
    let (quoted, _) = quoted_and_class(case, &reason);
    const COVERAGE: &str = "Current AZ coverage: ";
    let covered = quoted
        .split_once(COVERAGE)
        .and_then(|(_, rest)| rest.split_once('.'))
        .map(|(covered, _)| covered)
        .unwrap_or_else(|| panic!("case {case:?}: expected {COVERAGE:?}, got {quoted:?}"));
    assert!(
        covered.starts_with(REGION) && covered.len() > REGION.len(),
        "case {case:?}: expected a zone of the Region the subnets were created in, got \
         {covered:?}"
    );
    assert!(
        !covered.contains(','),
        "case {case:?}: expected the ONE zone both subnets lie in, got {covered:?}"
    );
}
