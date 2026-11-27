// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation stack deploys for a Gateway Load Balancer estate, across the two
//! services that own it: CloudFormation drives the deployment and Elastic Load Balancing owns
//! the load balancer and the target group.
//!
//! Two rules, read off the members' own documentation
//! (`CreateLoadBalancerInput$Scheme`, `CreateTargetGroupInput$Port`):
//!
//! * a load balancer of the family the `Scheme` member is documented AGAINST — "you can't
//!   specify a scheme for a Gateway Load Balancer" — carries no scheme, so a template that
//!   declares none gets none and the owning service reports no `Scheme` member for it. A
//!   client that branches on `Scheme` to tell internet-facing from internal load balancers
//!   must not read one for a load balancer that cannot have one;
//! * a target group whose protocol fixes one supported port — "if the protocol is GENEVE, the
//!   supported port is 6081" — is refused on any other port, and a declaration CloudFormation
//!   cannot create fails its resource and ROLLS THE STACK BACK, with the owning service's own
//!   refusal relayed in the resource-handler envelope. A deployment that completes with an
//!   appliance fleet whose traffic can never reach its targets is worse than one that fails:
//!   the template is believed deployable and the failure arrives at traffic time.
//!
//! The rows are the two declarations the rule separates: the same estate on the port the
//! protocol supports, which deploys, and on a port it does not, which rolls back. Nothing is
//! asserted from a recording: the stack's own outputs name the resources, and every value is
//! read back out of Elastic Load Balancing, which owns them.
//!
//! The router, the wire calls and the stack poller are
//! [`crate::cloudformation_monitored_alb_search_estate`]'s, shared rather than copied.

use crate::cloudformation_monitored_alb_search_estate::{
    build_router, cloudformation, elbv2, resource_reasons, settle_stack,
};
use crate::query_wire::{elements, opt_element, outputs_of};

/// The terminal statuses the two rows settle in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The protocol the model fixes one port for, that port, and a port inside `Port`'s own
/// `1..65535` range that the protocol does not support.
const GENEVE: &str = "GENEVE";
const SUPPORTED_PORT: &str = "6081";
const UNSUPPORTED_PORT: &str = "6082";

/// The sentence Elastic Load Balancing refuses the unsupported port with, which the relayed
/// reason has to carry.
const REFUSAL: &str = "Port must be 6081 for GENEVE target groups.";

/// What the resource-handler envelope around that refusal states: the service that answered,
/// the HTTP status it answered with, and the handler's classification of a declaration the
/// caller has to fix.
const REFUSING_SERVICE: &str = "Service: ElasticLoadBalancingV2";
const REFUSED_STATUS: &str = "Status Code: 400";
const REFUSAL_CLASS: &str = "HandlerErrorCode: InvalidRequest";

/// The network each row's estate is placed in: a VPC and two subnets of it in two Availability
/// Zones of the Region the calls are signed for.
const VPC_CIDR: &str = "10.80.0.0/16";

/// One row: the stack, the port its GENEVE target group is declared on, the names it declares
/// for the two Elastic Load Balancing resources, and whether the deployment completes.
struct Row {
    case: &'static str,
    stack: &'static str,
    port: &'static str,
    load_balancer_name: &'static str,
    target_group_name: &'static str,
    completes: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a GENEVE target group on the port its protocol supports",
        stack: "gwlb-geneve-supported",
        port: SUPPORTED_PORT,
        load_balancer_name: "gwlb-supported",
        target_group_name: "geneve-supported",
        completes: true,
    },
    Row {
        case: "a GENEVE target group on a port its protocol does not support",
        stack: "gwlb-geneve-unsupported",
        port: UNSUPPORTED_PORT,
        load_balancer_name: "gwlb-unsupported",
        target_group_name: "geneve-unsupported",
        completes: false,
    },
];

/// The template a row deploys: the network, a Gateway Load Balancer declaring NO scheme, and
/// the GENEVE target group on the row's port.
fn template(row: &Row) -> String {
    format!(
        "Resources:
  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: {VPC_CIDR}
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.80.1.0/24
      AvailabilityZone: us-east-1a
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.80.2.0/24
      AvailabilityZone: us-east-1b
  Gwlb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Name: {name}
      Type: gateway
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
  Appliances:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Name: {group}
      Protocol: {GENEVE}
      Port: {port}
      TargetType: ip
      VpcId: !Ref Vpc
Outputs:
  LoadBalancerArn:
    Value: !Ref Gwlb
  TargetGroupArn:
    Value: !Ref Appliances
",
        name = row.load_balancer_name,
        group = row.target_group_name,
        port = row.port,
    )
}

/// Every target group name Elastic Load Balancing holds, read through its own list.
async fn held_target_group_names(router: &smithy_server_runtime::MultiServiceRouter, case: &str) -> Vec<String> {
    let listed = elbv2(router, case, "DescribeTargetGroups", &[]).await;
    elements(&listed, "TargetGroupName")
}

#[tokio::test]
async fn a_declared_gateway_estate_carries_no_scheme_and_only_the_port_geneve_supports() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", row.stack), ("TemplateBody", &template(row))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );
        let described = settle_stack(&router, case, row.stack).await;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();

        if !row.completes {
            assert_eq!(
                reported, ROLLBACK_COMPLETE,
                "case {case:?}: a declaration the owning service refuses must roll the stack \
                 back, got {reported:?}; its reasons were {:?}",
                resource_reasons(&router, row.stack).await
            );
            // The refusal that failed the resource is the owning service's own, relayed in the
            // resource-handler envelope.
            let reasons = resource_reasons(&router, row.stack).await;
            assert!(
                reasons.iter().any(|reason| {
                    [REFUSAL, REFUSING_SERVICE, REFUSED_STATUS, REFUSAL_CLASS]
                        .iter()
                        .all(|part| reason.contains(part))
                }),
                "case {case:?}: expected a reason carrying {REFUSAL:?} under {REFUSING_SERVICE}, \
                 {REFUSED_STATUS} and {REFUSAL_CLASS}, got {reasons:?}"
            );
            // And nothing was left behind for it: the rolled-back stack holds no target group
            // of the name it declared.
            let held = held_target_group_names(&router, case).await;
            assert!(
                !held.iter().any(|name| name == row.target_group_name),
                "case {case:?}: a refused declaration must leave no target group, got {held:?}"
            );
            continue;
        }

        assert_eq!(
            reported, CREATE_COMPLETE,
            "case {case:?}: the deployment must complete, got {reported:?}; its reasons were {:?}",
            resource_reasons(&router, row.stack).await
        );
        let outputs = outputs_of(&described);
        let load_balancer = outputs
            .get("LoadBalancerArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no LoadBalancerArn"));
        let group = outputs
            .get("TargetGroupArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no TargetGroupArn"));

        // What Elastic Load Balancing holds for the load balancer the stack deployed: the
        // family it was declared as, and NO scheme at all — the member is absent, which is how
        // an optional member says this load balancer has none.
        let described_lb = elbv2(
            &router,
            case,
            "DescribeLoadBalancers",
            &[("LoadBalancerArns.member.1", &load_balancer)],
        )
        .await;
        assert_eq!(
            opt_element(&described_lb, "Type").as_deref(),
            Some("gateway"),
            "case {case:?}: expected the declared family to be reported, got {described_lb}"
        );
        assert_eq!(
            opt_element(&described_lb, "Scheme"),
            None,
            "case {case:?}: a gateway load balancer carries no Scheme, got {described_lb}"
        );

        // And the target group the stack deployed carries the protocol and the port it
        // declared.
        let described_group = elbv2(
            &router,
            case,
            "DescribeTargetGroups",
            &[("TargetGroupArns.member.1", &group)],
        )
        .await;
        assert_eq!(
            (
                opt_element(&described_group, "Protocol").as_deref(),
                opt_element(&described_group, "Port").as_deref()
            ),
            (Some(GENEVE), Some(row.port)),
            "case {case:?}: expected the declared protocol and port, got {described_group}"
        );
    }
}
