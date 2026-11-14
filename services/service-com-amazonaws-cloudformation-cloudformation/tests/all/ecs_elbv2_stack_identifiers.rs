// SPDX-License-Identifier: CC0-1.0

//! The identifier a deployed stack hands a client, read BOTH ways: out of the stack's
//! `Outputs` and out of `DescribeStackResources`.
//!
//! The rule under test: **the identifier a stack reports for a resource is the one that
//! resource type's published "Return values" define — and the `OutputValue` an output
//! carries, the physical resource id `Ref` answers and the `PhysicalResourceId`
//! `DescribeStackResources` reports are all that same identifier.** `Fn::GetAtt` answers
//! every string-valued attribute that section publishes, so an output composed of those
//! resolves instead of arriving with a key and no value, which is what a client reading
//! `Outputs[i]["OutputValue"]` cannot survive.
//!
//! What this suite adds to the two beside it — `provisioned_resources.rs`, which states
//! the rule, and `nested_resource_outputs.rs`, which drives the three types whose
//! identifier embeds another resource's — is the OTHER half of the same rule: that the
//! identifier a client reads out of the outputs is the one the stack's own resource read
//! reports, and the two derivations an observability stack performs on it — a CloudWatch
//! alarm's `AWS/ApplicationELB` `LoadBalancer` dimension value is the part of its load
//! balancer's ARN after `loadbalancer/`, and an alarm's own ARN spells its resource with a
//! colon. The last row is the issue's whole shape in one deployment: an ECS service behind
//! an ALB, its task logs going to a log group, a task role, and the 5xx alarm on the load
//! balancer.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, the `Output` reader
//! `stack_outputs.rs` builds and the `StackResource` reader `stack_resources.rs` builds,
//! so what it asserts is what a client reading the stack it just deployed sees. The rows
//! span the class the rule generalises over: the load-balancer kind that decides the
//! `app`/`net`/`gwy` segment of its ARN, the scheme that decides whether its DNS name
//! begins `internal-`, whether the template NAMES the resource or leaves CloudFormation to
//! generate the name, how deep the chain of embedded identifiers runs (load balancer →
//! listener → rule) and both template spellings.
//!
//! An `AWS::ECS::Cluster` is CREATED in Amazon ECS, which a crate-local suite cannot
//! mount, so the rows here name their cluster rather than declaring one; the cluster as a
//! resource of the same template is
//! `core/router/tests/all/cloudformation_provisions_container_platform_resources.rs`.
//!
//! The load balancers, target groups, listeners and rules the rows declare are CREATED in
//! Elastic Load Balancing itself, so every identifier asserted here is the one that service
//! minted and each row declares the VPC and the subnets its load balancer is placed in — a
//! placement that service resolves against Amazon EC2 before it creates anything. That is
//! also why the host a load balancer publishes is asserted as an ADDRESS a client can dial
//! for the APPLICATION family, whose traffic this emulator serves — the `DNSName` is then the
//! `host:port` of the listener it opened for the load balancer — and as the name that family
//! publishes for the others, which it serves no traffic for
//! (`my-nlb-1234567890abcdef.elb.us-west-2.amazonaws.com`).

use std::collections::BTreeMap;

use crate::nested_resource_outputs::{is_lower_hex, is_upper_hex};
use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, router, wait_for_the_transition, xml_text,
    ACCOUNT_ID, REGION,
};
use crate::stack_outputs::wire_outputs;
use crate::stack_resources::{resources_of, wire_resources};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How many hexadecimal characters the service-assigned id inside an ELBv2 identifier
/// carries: sixteen, as every published example spells it
/// (`loadbalancer/app/my-load-balancer/50dc6c495c0c9188`,
/// `targetgroup/my-targets/73e2d6bc24d8a067`).
const ELBV2_ID_DIGITS: usize = 16;

/// The network declaration every row's load balancer is placed in: a VPC and two subnets of
/// it, in two Availability Zones, since Elastic Load Balancing resolves a placement against
/// Amazon EC2 and an Application Load Balancer needs subnets in at least two zones.
const NETWORK: &str = r#"  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: 10.0.0.0/16
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.0.1.0/24
      AvailabilityZone: us-east-1a
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.0.2.0/24
      AvailabilityZone: us-east-1b
"#;

/// How many characters the unique segment of a generated physical name carries
/// (`<stack name>-<logical id>-<unique id>`).
const GENERATED_ID_DIGITS: usize = 12;

/// The head every ELBv2 ARN carries: the partition, the `elasticloadbalancing` namespace,
/// the Region and the account.
fn elb_head() -> String {
    format!("arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:")
}

/// What an Amazon ECS ARN carries before its resource path.
fn ecs_head() -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:")
}

/// What one declared output must carry.
///
/// A value is a literal `head`, then — for an identifier that embeds another resource's —
/// the text another output carries, then a `separator`, then the run of `digits` characters
/// ASSIGNED rather than requested (no request fixes those, so the rule is the form), then a
/// literal `tail`. Expressing an embedded identifier as the other output's own value is
/// what holds the two to the SAME assigned id, which is the derivation a client performs.
///
/// The two kinds of assigned run differ in their alphabet, and each row is held to its own:
/// an id Elastic Load Balancing assigns is LOWERCASE hexadecimal, CloudFormation's own
/// generated name segment UPPERCASE — the same two checks `nested_resource_outputs.rs`
/// applies, reused from it.
#[derive(Debug, Clone, Default)]
struct Expect {
    head: String,
    /// The `OutputKey` whose value comes next, if any.
    embeds: Option<&'static str>,
    /// Which PART of that output's value is embedded: the text after this marker, or the
    /// whole value when empty.
    embeds_after: &'static str,
    separator: &'static str,
    digits: usize,
    /// Whether the assigned run is CloudFormation's generated name segment (uppercase)
    /// rather than an id Elastic Load Balancing assigned (lowercase).
    generated: bool,
    /// Whether the whole value is an ADDRESS a client on this machine can dial — the
    /// `host:port` of the listener the emulator opened for a load balancer, which is what an
    /// APPLICATION load balancer publishes as its `DNSName` because this emulator serves
    /// that family's traffic. Nothing else about such a value is fixed: the port is the one
    /// the socket was given.
    address: bool,
    tail: String,
}

/// Exactly this value: everything about it follows from the request and the template.
fn exact(text: impl Into<String>) -> Expect {
    Expect {
        head: text.into(),
        ..Expect::default()
    }
}

/// `<head><id Elastic Load Balancing assigned>`.
fn with_id(head: impl Into<String>, digits: usize) -> Expect {
    Expect {
        head: head.into(),
        digits,
        ..Expect::default()
    }
}

/// `<head><the `<stack name>-<logical id>-<unique id>` name CloudFormation generated>`,
/// for a resource the template does not name.
fn with_generated_name(head: impl Into<String>) -> Expect {
    Expect {
        head: head.into(),
        digits: GENERATED_ID_DIGITS,
        generated: true,
        ..Expect::default()
    }
}

/// `<head><the whole value of `from`>`: an identifier that is another output's value
/// behind a literal head, which is how an ARN carries the full name of the resource below
/// it.
fn embedding(head: impl Into<String>, from: &'static str) -> Expect {
    Expect {
        head: head.into(),
        embeds: Some(from),
        ..Expect::default()
    }
}

/// `<head><the value of `from`>/<service-assigned id>`.
fn embedding_with_id(head: impl Into<String>, from: &'static str, digits: usize) -> Expect {
    Expect {
        head: head.into(),
        embeds: Some(from),
        separator: "/",
        digits,
        ..Expect::default()
    }
}

/// Whether `actual` is the value `expected` describes, against the other outputs of the
/// same stack.
fn holds(expected: &Expect, actual: &str, values: &BTreeMap<String, String>) -> bool {
    if expected.address {
        return actual.parse::<std::net::SocketAddr>().is_ok();
    }
    let Some(mut rest) = actual.strip_prefix(&expected.head) else {
        return false;
    };
    if let Some(key) = expected.embeds {
        let Some(value) = values.get(key) else {
            return false;
        };
        let embedded = if expected.embeds_after.is_empty() {
            value.as_str()
        } else {
            match value.split_once(expected.embeds_after) {
                Some((_, tail)) => tail,
                None => return false,
            }
        };
        match rest.strip_prefix(embedded) {
            Some(remaining) => rest = remaining,
            None => return false,
        }
    }
    match rest.strip_prefix(expected.separator) {
        Some(remaining) => rest = remaining,
        None => return false,
    }
    let Some(assigned) = rest.strip_suffix(&expected.tail) else {
        return false;
    };
    if expected.generated {
        is_upper_hex(assigned, expected.digits)
    } else {
        is_lower_hex(assigned, expected.digits)
    }
}

/// The form `expected` describes, for an assertion message.
fn described(expected: &Expect, values: &BTreeMap<String, String>) -> String {
    if expected.address {
        return "the host:port of the listener the emulator opened for it".to_string();
    }
    let embedded = expected
        .embeds
        .map(|key| match values.get(key) {
            Some(value) if expected.embeds_after.is_empty() => value.clone(),
            Some(value) => format!("<{key} after {:?}: {value}>", expected.embeds_after),
            None => format!("<the value of {key}, which the stack did not report>"),
        })
        .unwrap_or_default();
    let assigned = if expected.digits == 0 {
        String::new()
    } else if expected.generated {
        format!("<{} uppercase hexadecimal characters>", expected.digits)
    } else {
        format!("<{} lowercase hexadecimal characters>", expected.digits)
    };
    format!(
        "{}{embedded}{}{assigned}{}",
        expected.head, expected.separator, expected.tail
    )
}

/// One row: a template, and what the stack it deploys must report.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// What each declared output must carry.
    expected: Vec<(&'static str, Expect)>,
    /// `(whole, marker, part)`: the value of `whole` after `marker` is exactly the value
    /// of `part` — the derivation a client performs to turn a reported identifier into a
    /// CloudWatch dimension value or into the resource path of the resource below it.
    derivations: Vec<(&'static str, &'static str, &'static str)>,
    /// `(logical id, output key)`: the physical resource id `DescribeStackResources`
    /// reports for that resource is the value that output carries, which is what `Ref`
    /// answered for it.
    physical_ids: Vec<(&'static str, &'static str)>,
}

/// The container definition every task definition row declares, as a template spells it.
///
/// It reserves `Memory`, because every task definition declared here is now really
/// registered in Amazon ECS: "at least one of 'memory' or 'memoryReservation' must be
/// specified" for a container of a task that reserves none itself, which is the rule that
/// service holds a registration to.
const CONTAINER_DEFINITIONS: &str = r#"
        - Name: app
          Image: public.ecr.aws/nginx/nginx:latest
          Memory: 256
          PortMappings:
            - ContainerPort: 80"#;

fn cases() -> Vec<Case> {
    let elb = elb_head();
    let ecs = ecs_head();
    vec![
        Case {
            case: "an application load balancer, its target group, listener and rule, \
                   every one named by the template",
            stack_name: "alb-routing",
            template: format!(
                r#"Resources:
{NETWORK}  Alb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Name: probe-alb
      Type: application
      Scheme: internet-facing
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
  Tg:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Name: probe-tg
      Port: 80
      Protocol: HTTP
      TargetType: ip
      VpcId: !Ref Vpc
  Listener:
    Type: AWS::ElasticLoadBalancingV2::Listener
    Properties:
      LoadBalancerArn: !Ref Alb
      Port: 80
      Protocol: HTTP
      DefaultActions:
        - Type: forward
          TargetGroupArn: !Ref Tg
  Rule:
    Type: AWS::ElasticLoadBalancingV2::ListenerRule
    Properties:
      ListenerArn: !Ref Listener
      Priority: 10
      Conditions:
        - Field: path-pattern
          Values:
            - /api/*
      Actions:
        - Type: forward
          TargetGroupArn: !Ref Tg
Outputs:
  AlbRef:
    Value: !Ref Alb
  AlbArn:
    Value: !GetAtt Alb.LoadBalancerArn
  AlbFullName:
    Value: !GetAtt Alb.LoadBalancerFullName
  AlbName:
    Value: !GetAtt Alb.LoadBalancerName
  AlbDns:
    Value: !GetAtt Alb.DNSName
  TgRef:
    Value: !Ref Tg
  TgArn:
    Value: !GetAtt Tg.TargetGroupArn
  TgFullName:
    Value: !GetAtt Tg.TargetGroupFullName
  TgName:
    Value: !GetAtt Tg.TargetGroupName
  ListenerRef:
    Value: !Ref Listener
  ListenerArn:
    Value: !GetAtt Listener.ListenerArn
  RuleRef:
    Value: !Ref Rule
  RuleArn:
    Value: !GetAtt Rule.RuleArn
  RuleIsDefault:
    Value: !GetAtt Rule.IsDefault
"#
            ),
            expected: vec![
                ("AlbName", exact("probe-alb")),
                ("AlbFullName", with_id("app/probe-alb/", ELBV2_ID_DIGITS)),
                // "Ref returns the ARN of the load balancer", and the ARN's resource path
                // is `loadbalancer/` plus the load balancer's full name.
                ("AlbRef", embedding(format!("{elb}loadbalancer/"), "AlbFullName")),
                ("AlbArn", embedding(format!("{elb}loadbalancer/"), "AlbFullName")),
                (
                    "AlbDns",
                    // The host an Application Load Balancer publishes is the address the
                    // emulator serves it at, which is what a client dials.
                    Expect {
                        address: true,
                        ..Expect::default()
                    },
                ),
                ("TgName", exact("probe-tg")),
                (
                    "TgFullName",
                    with_id("targetgroup/probe-tg/", ELBV2_ID_DIGITS),
                ),
                ("TgRef", embedding(elb.clone(), "TgFullName")),
                ("TgArn", embedding(elb.clone(), "TgFullName")),
                // A listener's ARN carries the full name of the load balancer it is on.
                (
                    "ListenerRef",
                    embedding_with_id(format!("{elb}listener/"), "AlbFullName", ELBV2_ID_DIGITS),
                ),
                (
                    "ListenerArn",
                    embedding_with_id(format!("{elb}listener/"), "AlbFullName", ELBV2_ID_DIGITS),
                ),
                // And a rule's carries its listener's, two resources deep.
                (
                    "RuleRef",
                    Expect {
                        head: format!("{elb}listener-rule/"),
                        embeds: Some("ListenerArn"),
                        embeds_after: ":listener/",
                        separator: "/",
                        digits: ELBV2_ID_DIGITS,
                        ..Expect::default()
                    },
                ),
                (
                    "RuleArn",
                    Expect {
                        head: format!("{elb}listener-rule/"),
                        embeds: Some("ListenerArn"),
                        embeds_after: ":listener/",
                        separator: "/",
                        digits: ELBV2_ID_DIGITS,
                        ..Expect::default()
                    },
                ),
                // The default rule is the one created with the listener, so a declared
                // rule is not it.
                ("RuleIsDefault", exact("false")),
            ],
            derivations: vec![("AlbArn", "loadbalancer/", "AlbFullName")],
            physical_ids: vec![
                ("Alb", "AlbRef"),
                ("Tg", "TgRef"),
                ("Listener", "ListenerRef"),
                ("Rule", "RuleRef"),
            ],
        },
        Case {
            // The kind of load balancer decides the segment its ARN carries, and the
            // scheme decides whether its DNS name begins `internal-`. Neither is named by
            // the template, so each identifier is built from the name CloudFormation
            // generated. JSON spelling, long-form intrinsics.
            case: "a network and a gateway load balancer the template does not name",
            stack_name: "unnamed-lbs",
            template: r#"{"Resources":{
                   "Vpc":{"Type":"AWS::EC2::VPC",
                     "Properties":{"CidrBlock":"10.1.0.0/16"}},
                   "Subnet":{"Type":"AWS::EC2::Subnet",
                     "Properties":{"VpcId":{"Ref":"Vpc"},"CidrBlock":"10.1.1.0/24",
                       "AvailabilityZone":"us-east-1a"}},
                   "Nlb":{"Type":"AWS::ElasticLoadBalancingV2::LoadBalancer",
                     "Properties":{"Type":"network","Scheme":"internal",
                       "Subnets":[{"Ref":"Subnet"}]}},
                   "Glb":{"Type":"AWS::ElasticLoadBalancingV2::LoadBalancer",
                     "Properties":{"Type":"gateway",
                       "Subnets":[{"Ref":"Subnet"}]}}},
                 "Outputs":{
                   "NlbName":{"Value":{"Fn::GetAtt":["Nlb","LoadBalancerName"]}},
                   "NlbFullName":{"Value":{"Fn::GetAtt":["Nlb","LoadBalancerFullName"]}},
                   "NlbRef":{"Value":{"Ref":"Nlb"}},
                   "NlbArn":{"Value":{"Fn::GetAtt":["Nlb","LoadBalancerArn"]}},
                   "NlbConventionalArn":{"Value":{"Fn::GetAtt":["Nlb","Arn"]}},
                   "NlbDns":{"Value":{"Fn::GetAtt":["Nlb","DNSName"]}},
                   "GlbName":{"Value":{"Fn::GetAtt":["Glb","LoadBalancerName"]}},
                   "GlbFullName":{"Value":{"Fn::GetAtt":["Glb","LoadBalancerFullName"]}},
                   "GlbArn":{"Value":{"Fn::GetAtt":["Glb","LoadBalancerArn"]}}}}"#
                .to_string(),
            expected: vec![
                ("NlbName", with_generated_name("unnamed-lbs-Nlb-")),
                (
                    "NlbFullName",
                    embedding_with_id("net/", "NlbName", ELBV2_ID_DIGITS),
                ),
                ("NlbRef", embedding(format!("{elb}loadbalancer/"), "NlbFullName")),
                ("NlbArn", embedding(format!("{elb}loadbalancer/"), "NlbFullName")),
                // The uniform `Arn` convention never contradicts the ARN the type
                // publishes.
                (
                    "NlbConventionalArn",
                    embedding(format!("{elb}loadbalancer/"), "NlbFullName"),
                ),
                // The host a NETWORK load balancer publishes is the name that family's own
                // guide publishes — its name, the very hexadecimal id its ARN carries, and
                // the `elb`-first host of its Region, with the scheme stated nowhere in it.
                // This emulator serves the traffic of the APPLICATION family, and publishes
                // an address only for the family it serves ([`Expect::address`]).
                (
                    "NlbDns",
                    Expect {
                        head: String::new(),
                        embeds: Some("NlbName"),
                        separator: "-",
                        digits: ELBV2_ID_DIGITS,
                        tail: format!(".elb.{REGION}.amazonaws.com"),
                        ..Expect::default()
                    },
                ),
                ("GlbName", with_generated_name("unnamed-lbs-Glb-")),
                (
                    "GlbFullName",
                    embedding_with_id("gwy/", "GlbName", ELBV2_ID_DIGITS),
                ),
                ("GlbArn", embedding(format!("{elb}loadbalancer/"), "GlbFullName")),
                // A gateway load balancer's DNS NAME is not asserted here: a Gateway Load
                // Balancer is reached through a VPC endpoint rather than by host name, so
                // the one `DNSName` an uncertain form would entrench is left to the arm
                // that mints it.
            ],
            derivations: vec![
                ("NlbArn", "loadbalancer/", "NlbFullName"),
                ("GlbArn", "loadbalancer/", "GlbFullName"),
            ],
            physical_ids: vec![("Nlb", "NlbRef")],
        },
        Case {
            // The issue's own shape: an ECS service behind an ALB, its task logs going to
            // a log group, and a CloudWatch alarm on the load balancer's 5xx count. Every
            // output is an identifier the client that deployed the stack addresses
            // something by, and the alarm's dimension is derived from the load balancer's
            // ARN.
            case: "an ecs service behind an alb with a log group and an alb 5xx alarm",
            stack_name: "observability",
            template: format!(
                r#"Parameters:
  App:
    Type: String
    Default: web
  ClusterName:
    Type: String
    Default: web-cluster
Resources:
{NETWORK}  Cluster:
    Type: AWS::ECS::Cluster
    Properties:
      ClusterName: !Ref ClusterName
  LogGroup:
    Type: AWS::Logs::LogGroup
    Properties:
      LogGroupName: !Sub "/ecs/${{App}}"
      RetentionInDays: 7
  TaskRole:
    Type: AWS::IAM::Role
    Properties:
      RoleName: !Sub "${{App}}-task-role"
      AssumeRolePolicyDocument:
        Version: "2012-10-17"
        Statement:
          - Effect: Allow
            Principal:
              Service: ecs-tasks.amazonaws.com
            Action: sts:AssumeRole
  Task:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: !Sub "${{App}}-task"
      TaskRoleArn: !GetAtt TaskRole.Arn
      ContainerDefinitions:{CONTAINER_DEFINITIONS}
  Alb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Name: !Sub "${{App}}-alb"
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
  Tg:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Name: !Sub "${{App}}-tg"
      Port: 80
      Protocol: HTTP
      VpcId: !Ref Vpc
  Listener:
    Type: AWS::ElasticLoadBalancingV2::Listener
    Properties:
      LoadBalancerArn: !Ref Alb
      Port: 80
      Protocol: HTTP
      DefaultActions:
        - Type: forward
          TargetGroupArn: !Ref Tg
  Service:
    Type: AWS::ECS::Service
    DependsOn: Listener
    Properties:
      ServiceName: !Ref App
      Cluster: !Ref Cluster
      TaskDefinition: !Ref Task
      DesiredCount: 1
      LoadBalancers:
        - ContainerName: app
          ContainerPort: 80
          TargetGroupArn: !Ref Tg
  Alarm:
    Type: AWS::CloudWatch::Alarm
    Properties:
      AlarmName: !Sub "${{App}}-alb-5xx"
      Namespace: AWS/ApplicationELB
      MetricName: HTTPCode_ELB_5XX_Count
      ComparisonOperator: GreaterThanThreshold
      EvaluationPeriods: 1
      Period: 60
      Statistic: Sum
      Threshold: 0
      Dimensions:
        - Name: LoadBalancer
          Value: !GetAtt Alb.LoadBalancerFullName
  UnnamedAlarm:
    Type: AWS::CloudWatch::Alarm
    Properties:
      Namespace: AWS/ApplicationELB
      MetricName: TargetResponseTime
      ComparisonOperator: GreaterThanThreshold
      EvaluationPeriods: 1
      Period: 60
      Statistic: Average
      Threshold: 1
      Dimensions:
        - Name: LoadBalancer
          Value: !GetAtt Alb.LoadBalancerFullName
Outputs:
  ClusterName:
    Value: !Ref ClusterName
  ServiceArn:
    Value: !Ref Service
  DeployedServiceName:
    Value: !GetAtt Service.Name
  LoadBalancerArn:
    Value: !Ref Alb
  LoadBalancerFullName:
    Value: !GetAtt Alb.LoadBalancerFullName
  LoadBalancerDnsName:
    Value: !GetAtt Alb.DNSName
  TargetGroupArn:
    Value: !Ref Tg
  ListenerArn:
    Value: !Ref Listener
  LogGroupName:
    Value: !Ref LogGroup
  LogGroupArn:
    Value: !GetAtt LogGroup.Arn
  TaskDefinitionArn:
    Value: !Ref Task
  TaskRoleArn:
    Value: !GetAtt TaskRole.Arn
  AlarmName:
    Value: !Ref Alarm
  AlarmArn:
    Value: !GetAtt Alarm.Arn
  UnnamedAlarmRef:
    Value: !Ref UnnamedAlarm
  UnnamedAlarmArn:
    Value: !GetAtt UnnamedAlarm.Arn
"#
            ),
            expected: vec![
                ("ClusterName", exact("web-cluster")),
                ("ServiceArn", exact(format!("{ecs}service/web-cluster/web"))),
                ("DeployedServiceName", exact("web")),
                (
                    "LoadBalancerFullName",
                    with_id("app/web-alb/", ELBV2_ID_DIGITS),
                ),
                (
                    "LoadBalancerArn",
                    embedding(format!("{elb}loadbalancer/"), "LoadBalancerFullName"),
                ),
                (
                    "LoadBalancerDnsName",
                    Expect {
                        address: true,
                        ..Expect::default()
                    },
                ),
                (
                    "TargetGroupArn",
                    with_id(format!("{elb}targetgroup/web-tg/"), ELBV2_ID_DIGITS),
                ),
                (
                    "ListenerArn",
                    embedding_with_id(
                        format!("{elb}listener/"),
                        "LoadBalancerFullName",
                        ELBV2_ID_DIGITS,
                    ),
                ),
                ("LogGroupName", exact("/ecs/web")),
                (
                    "LogGroupArn",
                    exact(format!(
                        "arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:/ecs/web:*"
                    )),
                ),
                (
                    "TaskDefinitionArn",
                    exact(format!("{ecs}task-definition/web-task:1")),
                ),
                (
                    "TaskRoleArn",
                    exact(format!("arn:aws:iam::{ACCOUNT_ID}:role/web-task-role")),
                ),
                ("AlarmName", exact("web-alb-5xx")),
                (
                    "AlarmArn",
                    exact(format!(
                        "arn:aws:cloudwatch:{REGION}:{ACCOUNT_ID}:alarm:web-alb-5xx"
                    )),
                ),
                // The same rule for an alarm the template does NOT name: `Ref` is the
                // name CloudFormation generated and the ARN carries that name after the
                // `alarm:` separator, whatever the name is.
                (
                    "UnnamedAlarmRef",
                    with_generated_name("observability-UnnamedAlarm-"),
                ),
                (
                    "UnnamedAlarmArn",
                    embedding(
                        format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT_ID}:alarm:"),
                        "UnnamedAlarmRef",
                    ),
                ),
            ],
            // The derivation the ALB 5xx check performs: the AWS/ApplicationELB
            // `LoadBalancer` dimension value is the part of the load balancer's ARN after
            // `loadbalancer/`.
            derivations: vec![(
                "LoadBalancerArn",
                "loadbalancer/",
                "LoadBalancerFullName",
            )],
            physical_ids: vec![
                ("Alb", "LoadBalancerArn"),
                ("Service", "ServiceArn"),
                ("Alarm", "AlarmName"),
                ("UnnamedAlarm", "UnnamedAlarmRef"),
                ("Listener", "ListenerArn"),
            ],
        },
    ]
}

#[tokio::test]
async fn a_stack_reports_the_published_identifier_of_every_resource_it_declares() {
    let router = router();

    for case in cases() {
        // A row's template declares the task-execution role its ECS task definition runs
        // as, so the request acknowledges the template's IAM resources.
        let (status_code, body) =
            create_stack(&router, case.stack_name, &case.template, &acknowledging_iam(&[])).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must accept the template, got {status_code}: {body}",
            case.case
        );

        wait_for_the_transition().await;
        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code}: {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE before its outputs \
             are read, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        // The rule a client depends on first: a CREATE_COMPLETE stack reports a VALUE for
        // every output its template declares.
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for output in &outputs {
            let key = output.key.clone().unwrap_or_default();
            let Some(value) = output.value.clone() else {
                panic!(
                    "case {:?}: every Output of a CREATE_COMPLETE stack must carry an \
                     OutputValue; {key:?} carries none ({outputs:?})",
                    case.case
                );
            };
            values.insert(key, value);
        }

        for (key, expected) in &case.expected {
            let Some(actual) = values.get(*key) else {
                panic!(
                    "case {:?}: no Output with OutputKey {key:?}; got {values:?}",
                    case.case
                );
            };
            assert!(
                holds(expected, actual, &values),
                "case {:?}: output {key:?} must carry {}, got {actual:?}",
                case.case,
                described(expected, &values)
            );
        }

        for (whole, marker, part) in &case.derivations {
            let (Some(whole_value), Some(part_value)) = (values.get(*whole), values.get(*part))
            else {
                panic!(
                    "case {:?}: the derivation {whole:?} -> {part:?} needs both outputs; \
                     got {values:?}",
                    case.case
                );
            };
            let derived = whole_value
                .split_once(marker)
                .map(|(_, tail)| tail.to_string());
            assert_eq!(
                derived.as_deref(),
                Some(part_value.as_str()),
                "case {:?}: the part of {whole:?} ({whole_value:?}) after {marker:?} must \
                 be {part:?} ({part_value:?}), got {derived:?}",
                case.case
            );
        }

        // And the identifier the stack's own resource read reports is the one `Ref`
        // answered: a client that goes on to address the resource by either gets the same
        // thing.
        let (status_code, body) = resources_of(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStackResources must answer 200, got {status_code}: {body}",
            case.case
        );
        let reported = wire_resources(&body);
        for (logical_id, key) in &case.physical_ids {
            let Some(resource) = reported
                .iter()
                .find(|resource| resource.logical_id.as_deref() == Some(*logical_id))
            else {
                panic!(
                    "case {:?}: no StackResource with LogicalResourceId {logical_id:?}; \
                     got {reported:?}",
                    case.case
                );
            };
            assert_eq!(
                resource.physical_id.as_deref(),
                values.get(*key).map(String::as_str),
                "case {:?}: the PhysicalResourceId of {logical_id:?} must be the \
                 identifier output {key:?} carries, got {:?}",
                case.case,
                resource.physical_id
            );
        }
    }
}
