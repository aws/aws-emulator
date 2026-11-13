// SPDX-License-Identifier: CC0-1.0

//! A stack's declared Elastic Load Balancing v2 resources EXIST in Elastic Load Balancing —
//! and every attribute the stack publishes for them resolves.
//!
//! The rule under test: **a stack whose template declares an
//! `AWS::ElasticLoadBalancingV2::LoadBalancer`, `::TargetGroup`, `::Listener` or
//! `::ListenerRule` creates that resource in Elastic Load Balancing itself, so the first read
//! a client makes against the ARN the stack publishes resolves there** — and every attribute
//! the resource type publishes carries the value that service reports, so a declared output
//! over one arrives with its `OutputValue` rather than with a key and nothing.
//!
//! Both halves are what a client does with a deployed stack: it reads `Outputs` into a map
//! (`{o["OutputKey"]: o["OutputValue"]}`, which an output with no value raises `KeyError` on)
//! and then drives the estate through the owning service's own API by the identifiers it
//! found. A `CREATE_COMPLETE` stack whose listener `DescribeListeners` answers
//! `ListenerNotFound` for is not a state AWS can be in.
//!
//! The rows differ along what the rule generalises over: the load balancer FAMILY, which
//! decides the ARN segment, the host form and whether a hosted zone is published at all
//! (application, network, gateway), its SCHEME, whether the template names each resource or
//! leaves the name to CloudFormation, and how far down the chain the template goes (a load
//! balancer alone, with a target group, with a listener, and with a rule on that listener).
//! Deleting the stack deletes them again: a resource a stack created and no longer records
//! must not be left behind in the service that holds it.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, elbv2, member, resource_event_reason, router,
    wait_for_the_transition, xml_blocks, xml_text, xml_texts, ACCOUNT_ID, REGION,
};
use crate::stack_outputs::wire_outputs;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

/// How many reads a stack is polled through before the case gives up on it: the load balancer
/// its template declares comes up in `provisioning` and the deployment is held open until
/// Elastic Load Balancing reports it `active`, so the terminal status is not due on the first
/// read.
const POLL_ATTEMPTS: usize = 12;

/// The network every row's load balancer is placed in: a VPC and two subnets of it, in two
/// Availability Zones, since Elastic Load Balancing resolves a placement against Amazon EC2
/// and an Application Load Balancer needs subnets in at least two zones.
pub(crate) const NETWORK: &str = concat!(
    "  Vpc:\n",
    "    Type: AWS::EC2::VPC\n",
    "    Properties:\n",
    "      CidrBlock: 10.9.0.0/16\n",
    "  SubnetA:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.9.1.0/24\n",
    "      AvailabilityZone: us-east-1a\n",
    "  SubnetB:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.9.2.0/24\n",
    "      AvailabilityZone: us-east-1b\n",
);

/// One row: a template, and what the stack it deploys must report and leave behind in Elastic
/// Load Balancing.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The resources declared beside [`NETWORK`], and the template's whole `Outputs` section.
    template: &'static str,
    /// The `app`/`net`/`gwy` segment the load balancer's own ARN carries.
    family_segment: &'static str,
    /// The load balancer's name, where the template states one; `None` where CloudFormation
    /// generated it (the row then only asserts the form of what the stack reports).
    load_balancer_name: Option<&'static str>,
    /// Whether the row declares a target group, a listener and a rule on it.
    target_group: bool,
    listener: bool,
    rule: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "an internet-facing application load balancer with a target group, a listener \
               and a rule, every one named by the template",
        stack_name: "elbv2-alb-chain",
        template: concat!(
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: chain-alb\n",
            "      Type: application\n",
            "      Scheme: internet-facing\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "      Tags:\n",
            "        - Key: app\n",
            "          Value: shop\n",
            "  Targets:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Name: chain-targets\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      TargetType: ip\n",
            "      VpcId: !Ref Vpc\n",
            "      HealthCheckPath: /health\n",
            "  Listener:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "  Rule:\n",
            "    Type: AWS::ElasticLoadBalancingV2::ListenerRule\n",
            "    Properties:\n",
            "      ListenerArn: !Ref Listener\n",
            "      Priority: 10\n",
            "      Conditions:\n",
            "        - Field: path-pattern\n",
            "          Values:\n",
            "            - /api/*\n",
            "      Actions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "Outputs:\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref Alb\n",
            "  LoadBalancerName:\n",
            "    Value: !GetAtt Alb.LoadBalancerName\n",
            "  LoadBalancerFullName:\n",
            "    Value: !GetAtt Alb.LoadBalancerFullName\n",
            "  DnsName:\n",
            "    Value: !GetAtt Alb.DNSName\n",
            "  CanonicalHostedZoneID:\n",
            "    Value: !GetAtt Alb.CanonicalHostedZoneID\n",
            "  SecurityGroups:\n",
            "    Value: !GetAtt Alb.SecurityGroups\n",
            // A list-valued attribute is a LIST of values: "for attributes that return a
            // list, use Fn::Select" — so the functions that read one read its entries.
            "  FirstSecurityGroup:\n",
            "    Value: !Select [0, !GetAtt Alb.SecurityGroups]\n",
            "  JoinedSecurityGroups:\n",
            "    Value: !Join [\"|\", !GetAtt Alb.SecurityGroups]\n",
            "  TargetGroupArn:\n",
            "    Value: !Ref Targets\n",
            "  TargetGroupFullName:\n",
            "    Value: !GetAtt Targets.TargetGroupFullName\n",
            "  ListenerArn:\n",
            "    Value: !Ref Listener\n",
            "  RuleArn:\n",
            "    Value: !Ref Rule\n",
        ),
        family_segment: "app",
        load_balancer_name: Some("chain-alb"),
        target_group: true,
        listener: true,
        rule: true,
    },
    Row {
        case: "an internal network load balancer with a TCP listener, neither named by the \
               template",
        stack_name: "elbv2-nlb-internal",
        template: concat!(
            "  Nlb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Type: network\n",
            "      Scheme: internal\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "  Targets:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Port: 5000\n",
            "      Protocol: TCP\n",
            "      TargetType: ip\n",
            "      VpcId: !Ref Vpc\n",
            "  Listener:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Nlb\n",
            "      Port: 5000\n",
            "      Protocol: TCP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "Outputs:\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref Nlb\n",
            "  LoadBalancerName:\n",
            "    Value: !GetAtt Nlb.LoadBalancerName\n",
            "  LoadBalancerFullName:\n",
            "    Value: !GetAtt Nlb.LoadBalancerFullName\n",
            "  DnsName:\n",
            "    Value: !GetAtt Nlb.DNSName\n",
            "  CanonicalHostedZoneID:\n",
            "    Value: !GetAtt Nlb.CanonicalHostedZoneID\n",
            "  TargetGroupArn:\n",
            "    Value: !Ref Targets\n",
            "  TargetGroupFullName:\n",
            "    Value: !GetAtt Targets.TargetGroupFullName\n",
            "  TargetGroupName:\n",
            "    Value: !GetAtt Targets.TargetGroupName\n",
            "  LoadBalancerArns:\n",
            "    Value: !GetAtt Targets.LoadBalancerArns\n",
            // The same rule where the list is EMPTY, which a target group no load balancer
            // routes to yet really has: the join of no entries is the empty value, not an
            // output with no value at all.
            "  JoinedLoadBalancerArns:\n",
            "    Value: !Join [\",\", !GetAtt Targets.LoadBalancerArns]\n",
            "  ListenerArn:\n",
            "    Value: !Ref Listener\n",
        ),
        family_segment: "net",
        load_balancer_name: None,
        target_group: true,
        listener: true,
        rule: false,
    },
    Row {
        case: "a gateway load balancer alone, with the attributes its declaration asks for",
        stack_name: "elbv2-gwlb-alone",
        template: concat!(
            "  Glb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: alone-gwlb\n",
            "      Type: gateway\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "      LoadBalancerAttributes:\n",
            "        - Key: deletion_protection.enabled\n",
            "          Value: \"false\"\n",
            "Outputs:\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref Glb\n",
            "  LoadBalancerName:\n",
            "    Value: !GetAtt Glb.LoadBalancerName\n",
            "  LoadBalancerFullName:\n",
            "    Value: !GetAtt Glb.LoadBalancerFullName\n",
            "  SecurityGroups:\n",
            "    Value: !GetAtt Glb.SecurityGroups\n",
        ),
        family_segment: "gwy",
        load_balancer_name: Some("alone-gwlb"),
        target_group: false,
        listener: false,
        rule: false,
    },
    Row {
        case: "a target group with every health-check member, its attributes and a \
               registered target",
        stack_name: "elbv2-tg-full",
        template: concat!(
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: full-alb\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "  Targets:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Name: full-targets\n",
            "      Port: 8080\n",
            "      Protocol: HTTP\n",
            "      ProtocolVersion: HTTP1\n",
            "      TargetType: ip\n",
            "      IpAddressType: ipv4\n",
            "      VpcId: !Ref Vpc\n",
            "      HealthCheckEnabled: true\n",
            "      HealthCheckProtocol: HTTP\n",
            "      HealthCheckPort: traffic-port\n",
            "      HealthCheckPath: /healthz\n",
            "      HealthCheckIntervalSeconds: 15\n",
            "      HealthCheckTimeoutSeconds: 5\n",
            "      HealthyThresholdCount: 3\n",
            "      UnhealthyThresholdCount: 3\n",
            "      Matcher:\n",
            "        HttpCode: \"200-299\"\n",
            "      TargetGroupAttributes:\n",
            "        - Key: deregistration_delay.timeout_seconds\n",
            "          Value: \"30\"\n",
            "        - Key: stickiness.enabled\n",
            "          Value: \"true\"\n",
            "      Targets:\n",
            "        - Id: 10.9.1.25\n",
            "          Port: 8080\n",
            "      Tags:\n",
            "        - Key: tier\n",
            "          Value: web\n",
            "  Listener:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "Outputs:\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref Alb\n",
            "  LoadBalancerName:\n",
            "    Value: !GetAtt Alb.LoadBalancerName\n",
            "  LoadBalancerFullName:\n",
            "    Value: !GetAtt Alb.LoadBalancerFullName\n",
            "  TargetGroupArn:\n",
            "    Value: !Ref Targets\n",
            "  TargetGroupFullName:\n",
            "    Value: !GetAtt Targets.TargetGroupFullName\n",
            "  TargetGroupName:\n",
            "    Value: !GetAtt Targets.TargetGroupName\n",
            "  LoadBalancerArns:\n",
            "    Value: !GetAtt Targets.LoadBalancerArns\n",
            // The same rule where the list is EMPTY, which a target group no load balancer
            // routes to yet really has: the join of no entries is the empty value, not an
            // output with no value at all.
            "  JoinedLoadBalancerArns:\n",
            "    Value: !Join [\",\", !GetAtt Targets.LoadBalancerArns]\n",
            "  ListenerArn:\n",
            "    Value: !Ref Listener\n",
        ),
        family_segment: "app",
        load_balancer_name: Some("full-alb"),
        target_group: true,
        listener: true,
        rule: false,
    },
];

/// The whole template of one row: the network, the row's own resources and its outputs.
fn template_of(row: &Row) -> String {
    ["Resources:\n", NETWORK, row.template].concat()
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
pub(crate) async fn poll_to_terminal(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (status_code, body) = describe_stack(router, stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStacks must answer 200, got {status_code} {body}"
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// Every output of the described stack as the key/value pairs a client reads it as — the very
/// read the issue filed: an output reported with a key and no value is missing from this map,
/// which is what the panic below reports.
pub(crate) fn outputs_of(case: &str, described: &str) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for output in wire_outputs(described) {
        let key = output.key.clone().unwrap_or_default();
        let Some(value) = output.value.clone() else {
            panic!(
                "case {case:?}: every Output of a CREATE_COMPLETE stack carries an OutputValue; \
                 {key:?} carries none"
            );
        };
        values.push((key, value));
    }
    values
}

/// The value of one output, which the row declared and so the stack must report.
pub(crate) fn output(case: &str, values: &[(String, String)], key: &str) -> String {
    values
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| panic!("case {case:?}: no output named {key:?}, got {values:?}"))
}

#[tokio::test]
async fn a_stacks_declared_load_balancing_resources_take_reads_in_elastic_load_balancing() {
    let router = router();
    for row in ROWS {
        let case = row.case;
        let (status_code, body) =
            create_stack(&router, row.stack_name, &template_of(row), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
        );
        let described = poll_to_terminal(&router, case, row.stack_name).await;
        if xml_text(&described, "StackStatus").as_deref() != Some(status::CREATE_COMPLETE) {
            // What the owning service refused, as the stack's own event stream reports it per
            // resource: the summary on the stack says only WHICH resource failed.
            let (_, events) = call(
                &router,
                "DescribeStackEvents",
                &[member("StackName", row.stack_name)],
            )
            .await;
            panic!(
                "case {case:?}: the stack must reach CREATE_COMPLETE, got {:?} ({:?}); its \
                 events: {events}",
                xml_text(&described, "StackStatus"),
                xml_text(&described, "StackStatusReason"),
            );
        }

        // Every declared output carries a value, and each is the value Elastic Load Balancing
        // reports for the resource.
        let values = outputs_of(case, &described);
        let load_balancer_arn = output(case, &values, "LoadBalancerArn");
        let name = output(case, &values, "LoadBalancerName");
        let full_name = output(case, &values, "LoadBalancerFullName");
        if let Some(declared) = row.load_balancer_name {
            assert_eq!(
                name, declared,
                "case {case:?}: expected the load balancer the template named"
            );
        } else {
            // "CloudFormation generates a unique physical ID and uses that ID for the
            // resource name", in the `<stack>-<logical id>-<unique id>` form, held to the
            // 32 characters a load balancer's name may carry.
            assert!(
                name.contains("-Nlb-")
                    && name.len() <= 32
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "case {case:?}: expected CloudFormation's generated name for the resource, \
                 got {name:?}"
            );
        }
        assert_eq!(
            full_name,
            format!("{}/{name}/{}", row.family_segment, assigned_id(case, &full_name)),
            "case {case:?}: expected the load balancer's full name to be \
             <family>/<name>/<assigned id>, got {full_name:?}"
        );
        assert_eq!(
            load_balancer_arn,
            format!(
                "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:loadbalancer/{full_name}"
            ),
            "case {case:?}: expected the ARN to carry the full name the stack reports"
        );

        // The read the issue filed, for the load balancer: the ARN the stack published
        // resolves in the service that owns it.
        let (status_code, described_lb) = elbv2(
            &router,
            "DescribeLoadBalancers",
            &[member("LoadBalancerArns.member.1", &load_balancer_arn)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeLoadBalancers on the stack's ARN \
             {load_balancer_arn:?} to answer 200, got {status_code} {described_lb}"
        );
        assert_eq!(
            xml_text(&described_lb, "LoadBalancerName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected Elastic Load Balancing to hold the load balancer the \
             stack reports, got {described_lb}"
        );
        // …in the state that service reports a load balancer ready in, which is what the
        // stack's completion waited for.
        assert_eq!(
            xml_text(&described_lb, "Code").as_deref(),
            Some("active"),
            "case {case:?}: expected the load balancer to be active once the stack completed, \
             got {described_lb}"
        );
        // The attributes the type publishes are the ones that service reports for it.
        if let Some(dns_name) = values.iter().find(|(key, _)| key == "DnsName") {
            assert_eq!(
                xml_text(&described_lb, "DNSName").as_deref(),
                Some(dns_name.1.as_str()),
                "case {case:?}: expected the stack's DNSName output to be the host Elastic \
                 Load Balancing publishes, got {described_lb}"
            );
        }
        if let Some(zone) = values.iter().find(|(key, _)| key == "CanonicalHostedZoneID") {
            assert_eq!(
                xml_text(&described_lb, "CanonicalHostedZoneId").as_deref(),
                Some(zone.1.as_str()),
                "case {case:?}: expected the stack's CanonicalHostedZoneID output to be the \
                 hosted zone Elastic Load Balancing publishes, got {described_lb}"
            );
        }

        if let Some(groups) = values.iter().find(|(key, _)| key == "SecurityGroups") {
            // The list-valued attribute carries the entries that service reports, in the
            // comma-separated form a string context takes a list in: an Application Load
            // Balancer whose declaration names no group is given its VPC's default one, and a
            // Gateway Load Balancer carries none at all.
            let reported_groups = xml_blocks(&described_lb, "SecurityGroups")
                .into_iter()
                .next()
                .map(|block| xml_texts(&block, "member").join(","))
                .unwrap_or_default();
            assert_eq!(
                groups.1, reported_groups,
                "case {case:?}: expected the stack's SecurityGroups output to be the groups \
                 Elastic Load Balancing reports, got {described_lb}"
            );
            // …and the functions that read a list read its ENTRIES, which is what a template
            // does with an attribute that returns one.
            if values.iter().any(|(key, _)| key == "FirstSecurityGroup") {
                assert_eq!(
                    output(case, &values, "FirstSecurityGroup"),
                    reported_groups.split(',').next().unwrap_or_default(),
                    "case {case:?}: expected Fn::Select over the attribute to answer its \
                     first entry"
                );
                assert_eq!(
                    output(case, &values, "JoinedSecurityGroups"),
                    reported_groups.replace(',', "|"),
                    "case {case:?}: expected Fn::Join over the attribute to answer its \
                     entries"
                );
            }
        }

        if row.target_group {
            let target_group_arn = output(case, &values, "TargetGroupArn");
            let (status_code, described_tg) = elbv2(
                &router,
                "DescribeTargetGroups",
                &[member("TargetGroupArns.member.1", &target_group_arn)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected DescribeTargetGroups on the stack's ARN \
                 {target_group_arn:?} to answer 200, got {status_code} {described_tg}"
            );
            assert_eq!(
                xml_text(&described_tg, "TargetGroupArn").as_deref(),
                Some(target_group_arn.as_str()),
                "case {case:?}: expected the target group the stack reports, got {described_tg}"
            );
            assert_eq!(
                output(case, &values, "TargetGroupFullName"),
                target_group_arn
                    .rsplit_once(":targetgroup/")
                    .map(|(_, path)| format!("targetgroup/{path}"))
                    .unwrap_or_default(),
                "case {case:?}: expected the group's full name to be its ARN's resource path"
            );
            // The join of the group's own list-valued attribute: the entries it carries,
            // which is the empty value while no load balancer routes to the group.
            if values.iter().any(|(key, _)| key == "JoinedLoadBalancerArns") {
                assert_eq!(
                    output(case, &values, "JoinedLoadBalancerArns"),
                    output(case, &values, "LoadBalancerArns"),
                    "case {case:?}: expected Fn::Join over the attribute to answer its \
                     entries"
                );
            }
        }

        if row.listener {
            // Cause 1 of the issue, exactly: the listener ARN a CREATE_COMPLETE stack exports
            // describes successfully instead of answering ListenerNotFound.
            let listener_arn = output(case, &values, "ListenerArn");
            let (status_code, described_listener) = elbv2(
                &router,
                "DescribeListeners",
                &[member("ListenerArns.member.1", &listener_arn)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected DescribeListeners on the stack's listener ARN \
                 {listener_arn:?} to answer 200, got {status_code} {described_listener}"
            );
            assert_eq!(
                xml_text(&described_listener, "ListenerArn").as_deref(),
                Some(listener_arn.as_str()),
                "case {case:?}: expected the listener the stack reports, got \
                 {described_listener}"
            );
            assert_eq!(
                xml_text(&described_listener, "LoadBalancerArn").as_deref(),
                Some(load_balancer_arn.as_str()),
                "case {case:?}: expected the listener to be on the load balancer the stack \
                 reports, got {described_listener}"
            );
            // …and the listener is one of the load balancer's own, which is the read the
            // recorded flow makes: DescribeListeners by LOAD BALANCER.
            let (status_code, listed) = elbv2(
                &router,
                "DescribeListeners",
                &[member("LoadBalancerArn", &load_balancer_arn)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected DescribeListeners by load balancer to answer 200, \
                 got {status_code} {listed}"
            );
            assert_eq!(
                xml_text(&listed, "ListenerArn").as_deref(),
                Some(listener_arn.as_str()),
                "case {case:?}: expected the load balancer's listeners to be the stack's, got \
                 {listed}"
            );
        }

        if row.rule {
            let rule_arn = output(case, &values, "RuleArn");
            let listener_arn = output(case, &values, "ListenerArn");
            let (status_code, described_rules) = elbv2(
                &router,
                "DescribeRules",
                &[member("ListenerArn", &listener_arn)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected DescribeRules on the stack's listener to answer 200, \
                 got {status_code} {described_rules}"
            );
            assert!(
                described_rules.contains(&rule_arn),
                "case {case:?}: expected the listener's rules to carry the stack's rule \
                 {rule_arn:?}, got {described_rules}"
            );
        }
    }
}

/// The id Elastic Load Balancing assigned, read off the full name the stack reports: its last
/// segment, which no request fixes — so the rule is its FORM, the 16 lowercase hexadecimal
/// characters every published example of one carries
/// (`app/my-load-balancer/50dc6c495c0c9188`).
fn assigned_id<'a>(case: &str, full_name: &'a str) -> &'a str {
    let assigned = full_name.rsplit('/').next().unwrap_or_default();
    assert!(
        assigned.len() == 16
            && assigned
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case:?}: expected the last segment of {full_name:?} to be the 16 lowercase \
         hexadecimal characters Elastic Load Balancing assigns, got {assigned:?}"
    );
    assigned
}

/// Deleting the stack deletes the resources it created: the listener a client would go on to
/// read is gone from Elastic Load Balancing, which is what says the stack owned it.
#[tokio::test]
async fn deleting_the_stack_deletes_the_load_balancing_resources_it_created() {
    let router = router();
    let case = "a stack whose listener and load balancer are deleted with it";
    let template = [
        "Resources:\n",
        NETWORK,
        concat!(
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: transient-alb\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "  Targets:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Name: transient-targets\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      TargetType: ip\n",
            "      VpcId: !Ref Vpc\n",
            "  Listener:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "Outputs:\n",
            "  ListenerArn:\n",
            "    Value: !Ref Listener\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref Alb\n",
        ),
    ]
    .concat();
    let (status_code, body) = create_stack(&router, "transient-elbv2", &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
    );
    let described = poll_to_terminal(&router, case, "transient-elbv2").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
    );
    let values = outputs_of(case, &described);
    let listener_arn = output(case, &values, "ListenerArn");

    let (status_code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "transient-elbv2")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DeleteStack must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, _) = describe_stack(&router, "transient-elbv2").await;
    wait_for_the_transition().await;

    let (status_code, answered) = elbv2(
        &router,
        "DescribeListeners",
        &[member("ListenerArns.member.1", &listener_arn)],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the listener to be gone with the stack, got {status_code} \
         {answered}"
    );
    assert_eq!(
        xml_text(&answered, "Code").as_deref(),
        Some("ListenerNotFound"),
        "case {case:?}: expected ListenerNotFound once the stack deleted its listener, got \
         {answered}"
    );
}

/// The complement of the rule, from the other side: a declaration Elastic Load Balancing
/// REFUSES fails the resource and rolls the stack back, rather than completing with a resource
/// that exists nowhere — which is the state the issue reported.
#[tokio::test]
async fn a_declaration_the_owning_service_refuses_fails_the_stack() {
    let router = router();
    let case = "an application load balancer placed in one subnet";
    let template = [
        "Resources:\n",
        NETWORK,
        concat!(
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: one-subnet-alb\n",
            "      Type: application\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "Outputs:\n",
            "  LoadBalancerArn:\n",
            "    Value: !Ref Alb\n",
        ),
    ]
    .concat();
    let (status_code, body) = create_stack(&router, "refused-elbv2", &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
    );
    let described = poll_to_terminal(&router, case, "refused-elbv2").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "case {case:?}: expected the stack to roll back on the refusal, got {described}"
    );
    let reason =
        resource_event_reason(&router, "refused-elbv2", "Alb", resource_status::CREATE_FAILED)
            .await;
    assert!(
        reason.contains("two subnets"),
        "case {case:?}: expected the stack to report the owning service's own refusal, got \
         {reason:?}"
    );

    // Nothing was created for the failed resource: the load balancer the stack would have
    // published is in no Elastic Load Balancing read.
    let (status_code, answered) = elbv2(
        &router,
        "DescribeLoadBalancers",
        &[member("Names.member.1", "one-subnet-alb")],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected no load balancer to have been created, got {status_code} \
         {answered}"
    );
}
