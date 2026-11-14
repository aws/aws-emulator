// SPDX-License-Identifier: CC0-1.0

//! The stack outputs whose value is built from ANOTHER resource's identity: an Amazon ECS
//! service and an Elastic Load Balancing v2 listener and listener rule.
//!
//! The rule under test is the one `provisioned_resources.rs` states, applied to the three
//! catalogued types whose published identifier is not a function of their own name alone:
//! `DescribeStacks` reports each declared `Output` as a COMPLETE pair — an `OutputKey` AND
//! its resolved `OutputValue` — and that value is the return value the resource TYPE
//! publishes, in the identifier form the OWNING service produces. For these three the form
//! EMBEDS another resource: an ECS service's ARN carries the cluster that hosts it
//! (`service/<cluster>/<name>`), a listener's nests inside its load balancer's
//! (`listener/<type>/<lb>/<lb id>/<listener id>`) and a rule's inside its listener's — so
//! the value is only right once the resource the declaration names has an identity.
//!
//! What the rows span: the chain at both depths (a listener in a load balancer, a rule in
//! that listener), both template spellings, all the ways a service's declaration can name
//! its cluster (a literal short name, a cluster ARN, a template parameter, and not at all
//! — which is the `default` cluster), two of the three load balancer types, a named and an
//! unnamed resource, a declaration naming NO parent and one naming a value that is not an
//! ARN (which must not be turned into one), and an attribute nothing publishes.
//!
//! The one case this suite cannot drive is a service whose cluster is another RESOURCE of
//! the template: `AWS::ECS::Cluster` is created in Amazon ECS, which a crate-local suite
//! cannot mount, so it lives in
//! `core/router/tests/all/cloudformation_provisions_container_platform_resources.rs`.
//!
//! The load balancers these rows declare are CREATED in Elastic Load Balancing itself, which
//! resolves their placement against Amazon EC2, so each row declares the VPC and the subnets
//! its load balancer is placed in. What a NESTED resource's identity is when its declaration
//! names no parent at all is therefore not a deployment this suite can drive any more — such
//! a listener names no load balancer, which the owning service refuses — and is held to by the
//! naming rules' own suite (`state::template`'s tests).

use std::collections::BTreeMap;

use crate::stack_lifecycle::{
    create_stack, describe_stack, member, router, wait_for_the_transition, xml_text, ACCOUNT_ID,
    REGION,
};
use crate::stack_outputs::{output_for, wire_outputs, WireOutput};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How many lowercase hexadecimal characters the id Elastic Load Balancing assigns a
/// resource carries inside its ARN (`loadbalancer/app/my-lb/50dc6c495c0c9188`).
const ELB_ID_CHARACTERS: usize = 16;

/// How many uppercase hexadecimal characters CloudFormation's own generated physical name
/// ends with (`<stack name>-<logical id>-<unique id>`).
const GENERATED_NAME_CHARACTERS: usize = 12;

/// The ARN of an Amazon ECS resource: `arn:aws:ecs:<region>:<account>:<resource>`.
fn ecs_arn(resource: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// The ARN of an Elastic Load Balancing resource — both generations of the service share
/// the `elasticloadbalancing` namespace.
fn elb_arn(resource: &str) -> String {
    format!("arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// Whether `value` is `characters` LOWERCASE hexadecimal characters.
///
/// Shared with `ecs_elbv2_stack_identifiers.rs`, which holds the same ids to the same
/// alphabet.
pub(crate) fn is_lower_hex(value: &str, characters: usize) -> bool {
    value.len() == characters
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
}

/// Whether `value` is `characters` UPPERCASE hexadecimal characters, the alphabet of
/// CloudFormation's own generated name segment.
pub(crate) fn is_upper_hex(value: &str, characters: usize) -> bool {
    value.len() == characters
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'A'..='F'))
}

/// What one declared output must carry. Every variant but `Exact` states a FORM: the id the
/// owning service assigns is fixed by no request, so no row can spell it.
enum Expect {
    /// Exactly this text.
    Exact(String),
    /// `prefix`, then an id Elastic Load Balancing assigned.
    Minted { prefix: String },
    /// The value of output `parent` with its `from` ARN segment replaced by `to` and an
    /// assigned id appended — the form of a resource Elastic Load Balancing nests inside
    /// another, which is what makes a listener's ARN its load balancer's and a rule's its
    /// listener's.
    Nested {
        parent: &'static str,
        from: &'static str,
        to: &'static str,
    },
    /// CloudFormation's generated physical name for a resource the template does not name,
    /// which is also what a nested resource keeps when its declaration names no parent:
    /// `<stack name>-<logical id>-<unique id>`.
    Generated { prefix: String },
    /// Reported with its key and NO value, because nothing here can compute it.
    NoValue,
}

impl Expect {
    /// Whether `value` satisfies this expectation, and the English of what was required.
    fn check(&self, value: Option<&str>, outputs: &BTreeMap<String, String>) -> (bool, String) {
        match self {
            Self::Exact(text) => (value == Some(text.as_str()), format!("exactly {text:?}")),
            Self::Minted { prefix } => (
                value.is_some_and(|actual| {
                    actual
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|id| is_lower_hex(id, ELB_ID_CHARACTERS))
                }),
                format!("{prefix:?} + an assigned {ELB_ID_CHARACTERS}-character id"),
            ),
            Self::Nested { parent, from, to } => {
                let parent_value = outputs.get(*parent);
                let required = format!(
                    "the {parent} output ({parent_value:?}) with {from:?} replaced by {to:?} \
                     and an assigned id appended"
                );
                let ok = parent_value.is_some_and(|parent_value| {
                    let Some(nested) = nested_prefix(parent_value, from, to) else {
                        return false;
                    };
                    value.is_some_and(|actual| {
                        actual
                            .strip_prefix(&nested)
                            .is_some_and(|id| is_lower_hex(id, ELB_ID_CHARACTERS))
                    })
                });
                (ok, required)
            }
            Self::Generated { prefix } => (
                value.is_some_and(|actual| {
                    actual
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|unique| is_upper_hex(unique, GENERATED_NAME_CHARACTERS))
                }),
                format!("{prefix:?} + a {GENERATED_NAME_CHARACTERS}-character unique id"),
            ),
            Self::NoValue => (value.is_none(), "no OutputValue at all".to_string()),
        }
    }
}

/// The parent ARN with the `from` segment replaced by `to` and a `/` appended, which is
/// everything a nested Elastic Load Balancing ARN carries before its own assigned id.
fn nested_prefix(parent: &str, from: &str, to: &str) -> Option<String> {
    let marker = format!(":{from}/");
    parent
        .contains(&marker)
        .then(|| format!("{}/", parent.replacen(&marker, &format!(":{to}/"), 1)))
}

/// The network a row's load balancer is placed in: a VPC and two subnets of it, in two
/// Availability Zones, since Elastic Load Balancing resolves a placement against Amazon EC2
/// and an Application Load Balancer needs subnets in at least two zones.
const NETWORK: &str = concat!(
    "  Vpc:\n",
    "    Type: AWS::EC2::VPC\n",
    "    Properties:\n",
    "      CidrBlock: 10.0.0.0/16\n",
    "  SubnetA:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.0.1.0/24\n",
    "      AvailabilityZone: us-east-1a\n",
    "  SubnetB:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.0.2.0/24\n",
    "      AvailabilityZone: us-east-1b\n",
);

/// One row: a template, the parameters the create supplies, and what each of its declared
/// outputs must carry.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    expected: Vec<(&'static str, Expect)>,
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // The shape the issue's own check deploys, minus the cluster resource the
            // router suite owns: an ECS service on a named cluster, with an Application
            // Load Balancer, a target group, a listener and a listener rule, in the YAML
            // short-form spelling a CFN-driven integration writes. Every output is a `Ref`
            // or an `Fn::GetAtt` of a created resource, so every one must carry a value.
            case: "yaml short form: a listener and a rule under an application load balancer",
            stack_name: "ecs-elb-chain",
            template: [
                "Resources:\n",
                NETWORK,
                concat!(
                    "  Alb:\n",
                    "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
                    "    Properties:\n",
                    "      Name: chain-alb\n",
                    "      Scheme: internet-facing\n",
                    "      Subnets:\n",
                    "        - !Ref SubnetA\n",
                    "        - !Ref SubnetB\n",
                    "  TargetGroup:\n",
                    "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
                    "    Properties:\n",
                    "      Name: chain-tg\n",
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
                    "          TargetGroupArn: !Ref TargetGroup\n",
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
                    "          TargetGroupArn: !Ref TargetGroup\n",
                    "  Service:\n",
                    "    Type: AWS::ECS::Service\n",
                    "    Properties:\n",
                    "      ServiceName: chain-service\n",
                    "      Cluster: !Ref Cluster\n",
                    "      TaskDefinition: !Ref Task\n",
                    "      DesiredCount: 1\n",
                    "  Cluster:\n",
                    "    Type: AWS::ECS::Cluster\n",
                    "    Properties:\n",
                    "      ClusterName: chain-cluster\n",
                    "  Task:\n",
                    "    Type: AWS::ECS::TaskDefinition\n",
                    "    Properties:\n",
                    "      Family: chain-task\n",
                    "      ContainerDefinitions:\n",
                    "        - Name: app\n",
                    "          Image: public.ecr.aws/nginx/nginx:latest\n",
                    "          Memory: 256\n",
                    "Outputs:\n",
                    "  AlbRef:\n",
                    "    Value: !Ref Alb\n",
                    "  ListenerRef:\n",
                    "    Value: !Ref Listener\n",
                    "  ListenerArn:\n",
                    "    Value: !GetAtt Listener.ListenerArn\n",
                    "  RuleRef:\n",
                    "    Value: !Ref Rule\n",
                    "  RuleArn:\n",
                    "    Value: !GetAtt Rule.RuleArn\n",
                    "  RuleIsDefault:\n",
                    "    Value: !GetAtt Rule.IsDefault\n",
                    "  ServiceRef:\n",
                    "    Value: !Ref Service\n",
                    "  ServiceName:\n",
                    "    Value: !GetAtt Service.Name\n",
                ),
            ]
            .concat(),
            parameters: Vec::new(),
            expected: vec![
                (
                    "AlbRef",
                    Expect::Minted {
                        prefix: elb_arn("loadbalancer/app/chain-alb/"),
                    },
                ),
                // "Ref returns the ARN of the listener", which nests inside the load
                // balancer's own.
                (
                    "ListenerRef",
                    Expect::Nested {
                        parent: "AlbRef",
                        from: "loadbalancer",
                        to: "listener",
                    },
                ),
                (
                    "ListenerArn",
                    Expect::Nested {
                        parent: "AlbRef",
                        from: "loadbalancer",
                        to: "listener",
                    },
                ),
                // …and the rule's inside the listener's, one level further down.
                (
                    "RuleRef",
                    Expect::Nested {
                        parent: "ListenerRef",
                        from: "listener",
                        to: "listener-rule",
                    },
                ),
                (
                    "RuleArn",
                    Expect::Nested {
                        parent: "ListenerRef",
                        from: "listener",
                        to: "listener-rule",
                    },
                ),
                // A rule a template declares is never a listener's DEFAULT rule: that one
                // is created with the listener, as its `DefaultActions`.
                ("RuleIsDefault", Expect::Exact("false".to_string())),
                // A service ARN carries the cluster that hosts it, named here by its short
                // name.
                (
                    "ServiceRef",
                    Expect::Exact(ecs_arn("service/chain-cluster/chain-service")),
                ),
                ("ServiceName", Expect::Exact("chain-service".to_string())),
            ],
        },
        Case {
            // The JSON spelling with long-form intrinsics: a listener under an INTERNAL
            // NETWORK load balancer (so the ARN it nests in carries the `net` segment), a
            // service whose cluster is named by the ARN form the property also takes, and
            // a service that names NO cluster, which is the `default` one.
            case: "json long form: a listener under a network load balancer, a cluster by arn",
            stack_name: "ecs-elb-json",
            template: r#"{"Resources":{
                 "Vpc":{"Type":"AWS::EC2::VPC",
                   "Properties":{"CidrBlock":"10.2.0.0/16"}},
                 "Subnet":{"Type":"AWS::EC2::Subnet",
                   "Properties":{"VpcId":{"Ref":"Vpc"},"CidrBlock":"10.2.1.0/24",
                     "AvailabilityZone":"us-east-1a"}},
                 "Nlb":{"Type":"AWS::ElasticLoadBalancingV2::LoadBalancer",
                   "Properties":{"Name":"json-nlb","Type":"network","Scheme":"internal",
                     "Subnets":[{"Ref":"Subnet"}]}},
                 "NlbTargets":{"Type":"AWS::ElasticLoadBalancingV2::TargetGroup",
                   "Properties":{"Name":"json-nlb-tg","Port":5000,"Protocol":"TCP",
                     "TargetType":"ip","VpcId":{"Ref":"Vpc"}}},
                 "Listener":{"Type":"AWS::ElasticLoadBalancingV2::Listener",
                   "Properties":{"LoadBalancerArn":{"Ref":"Nlb"},"Port":5000,
                     "Protocol":"TCP",
                     "DefaultActions":[{"Type":"forward",
                       "TargetGroupArn":{"Ref":"NlbTargets"}}]}},
                 "ByArnCluster":{"Type":"AWS::ECS::Cluster",
                   "Properties":{"ClusterName":"by-arn-cluster"}},
                 "DefaultCluster":{"Type":"AWS::ECS::Cluster",
                   "Properties":{"ClusterName":"default"}},
                 "Task":{"Type":"AWS::ECS::TaskDefinition",
                   "Properties":{"Family":"json-task",
                     "ContainerDefinitions":[{"Name":"app",
                       "Image":"public.ecr.aws/nginx/nginx:latest","Memory":256}]}},
                 "ByArn":{"Type":"AWS::ECS::Service",
                   "Properties":{"ServiceName":"by-arn-service",
                     "TaskDefinition":{"Ref":"Task"},"DesiredCount":1,
                     "Cluster":{"Fn::GetAtt":["ByArnCluster","Arn"]}}},
                 "Solo":{"Type":"AWS::ECS::Service","DependsOn":"DefaultCluster",
                   "Properties":{"ServiceName":"solo-service",
                     "TaskDefinition":{"Ref":"Task"},"DesiredCount":1}}},
               "Outputs":{
                 "NlbRef":{"Value":{"Ref":"Nlb"}},
                 "ListenerRef":{"Value":{"Fn::GetAtt":["Listener","ListenerArn"]}},
                 "ByArnRef":{"Value":{"Ref":"ByArn"}},
                 "SoloRef":{"Value":{"Ref":"Solo"}},
                 "SoloStatus":{"Value":{"Fn::GetAtt":["Solo","Status"]}}}}"#
            .to_string(),
            parameters: Vec::new(),
            expected: vec![
                (
                    "NlbRef",
                    Expect::Minted {
                        prefix: elb_arn("loadbalancer/net/json-nlb/"),
                    },
                ),
                (
                    "ListenerRef",
                    Expect::Nested {
                        parent: "NlbRef",
                        from: "loadbalancer",
                        to: "listener",
                    },
                ),
                // The `Cluster` property takes "the short name or full Amazon Resource
                // Name (ARN) of the cluster", and the service ARN carries the NAME.
                (
                    "ByArnRef",
                    Expect::Exact(ecs_arn("service/by-arn-cluster/by-arn-service")),
                ),
                // "If you do not specify a cluster, the default cluster is assumed."
                (
                    "SoloRef",
                    Expect::Exact(ecs_arn("service/default/solo-service")),
                ),
                // The complement: a service's RUNNING state is not something this emulator
                // provisioned a service to be in, so the output is reported with its key
                // and no value rather than an invented one.
                ("SoloStatus", Expect::NoValue),
            ],
        },
        Case {
            // The cluster named by a template PARAMETER and an UNNAMED service, whose name
            // CloudFormation generates.
            case: "a cluster by parameter and an unnamed service",
            stack_name: "ecs-elb-edges",
            template: concat!(
                "Parameters:\n",
                "  ClusterName:\n",
                "    Type: String\n",
                "Resources:\n",
                "  Cluster:\n",
                "    Type: AWS::ECS::Cluster\n",
                "    Properties:\n",
                "      ClusterName: !Ref ClusterName\n",
                "  Task:\n",
                "    Type: AWS::ECS::TaskDefinition\n",
                "    Properties:\n",
                "      Family: edges-task\n",
                "      ContainerDefinitions:\n",
                "        - Name: app\n",
                "          Image: public.ecr.aws/nginx/nginx:latest\n",
                "          Memory: 256\n",
                "  Parameterised:\n",
                "    Type: AWS::ECS::Service\n",
                "    Properties:\n",
                "      ServiceName: parameterised-service\n",
                "      Cluster: !Ref Cluster\n",
                "      TaskDefinition: !Ref Task\n",
                "      DesiredCount: 1\n",
                "  Unnamed:\n",
                "    Type: AWS::ECS::Service\n",
                "    Properties:\n",
                "      Cluster: !Ref Cluster\n",
                "      TaskDefinition: !Ref Task\n",
                "      DesiredCount: 1\n",
                "Outputs:\n",
                "  ParameterisedRef:\n",
                "    Value: !Ref Parameterised\n",
                "  UnnamedName:\n",
                "    Value: !GetAtt Unnamed.Name\n",
            )
            .to_string(),
            parameters: params(&[("ClusterName", "parameterised-cluster")]),
            expected: vec![
                (
                    "ParameterisedRef",
                    Expect::Exact(ecs_arn(
                        "service/parameterised-cluster/parameterised-service",
                    )),
                ),
                (
                    "UnnamedName",
                    Expect::Generated {
                        prefix: "ecs-elb-edges-Unnamed-".to_string(),
                    },
                ),
            ],
        },
    ]
}

/// The values the response carries, keyed by output key, for the expectations that are
/// stated against ANOTHER output's value.
fn values_of(outputs: &[WireOutput]) -> BTreeMap<String, String> {
    outputs
        .iter()
        .filter_map(|output| Some((output.key.clone()?, output.value.clone()?)))
        .collect()
}

#[tokio::test]
async fn outputs_built_from_another_resources_identity_carry_their_resolved_values() {
    let router = router();

    for case in cases() {
        let (status_code, body) =
            create_stack(&router, case.stack_name, &case.template, &case.parameters).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected CreateStack to succeed, got {status_code} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status_code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        assert_eq!(
            outputs.len(),
            case.expected.len(),
            "case {:?}: expected one Output per declared output ({} of them), got {outputs:?}",
            case.case,
            case.expected.len()
        );
        let values = values_of(&outputs);
        for (key, expected) in &case.expected {
            let output = output_for(&outputs, key).unwrap_or_else(|| {
                panic!(
                    "case {:?}: output {key:?} must be reported, got {outputs:?}",
                    case.case
                )
            });
            let (ok, required) = expected.check(output.value.as_deref(), &values);
            assert!(
                ok,
                "case {:?}: output {key:?} must carry {required}, got {:?}",
                case.case, output.value
            );
        }
    }
}

/// Two listeners under one load balancer get DIFFERENT ids inside the SAME parent ARN, and
/// a second read reports the same ones: a nested identifier is derived from the resource
/// and its parent, not drawn fresh per read.
#[tokio::test]
async fn nested_listener_identifiers_are_unique_per_resource_and_stable() {
    let router = router();
    let template = [
        "Resources:\n",
        NETWORK,
        concat!(
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: twin-alb\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "  Targets:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Name: twin-tg\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      TargetType: ip\n",
            "      VpcId: !Ref Vpc\n",
            "  First:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "  Second:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 8080\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
            "Outputs:\n",
            "  AlbRef:\n",
            "    Value: !Ref Alb\n",
            "  FirstArn:\n",
            "    Value: !GetAtt First.ListenerArn\n",
            "  SecondArn:\n",
            "    Value: !GetAtt Second.ListenerArn\n",
        ),
    ]
    .concat();
    let case = "two listeners under one load balancer";
    let (status_code, body) = create_stack(&router, "elb-two-listeners", &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let (status_code, body) = describe_stack(&router, "elb-two-listeners").await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {status_code} {body}"
    );
    let outputs = values_of(&wire_outputs(&body));
    let load_balancer = outputs.get("AlbRef").cloned().unwrap_or_default();
    let nested = nested_prefix(&load_balancer, "loadbalancer", "listener").unwrap_or_else(|| {
        panic!("case {case:?}: expected a load balancer ARN, got {load_balancer:?}")
    });
    let first = outputs.get("FirstArn").cloned();
    let second = outputs.get("SecondArn").cloned();
    for (key, value) in [("FirstArn", first.clone()), ("SecondArn", second.clone())] {
        let value = value.unwrap_or_default();
        assert!(
            value
                .strip_prefix(&nested)
                .is_some_and(|id| is_lower_hex(id, ELB_ID_CHARACTERS)),
            "case {case:?}: output {key:?} must be {nested:?} + an assigned id, got {value:?}"
        );
    }
    assert_ne!(
        first, second,
        "case {case:?}: two listeners of one load balancer must not share an identifier, got \
         {first:?} and {second:?}"
    );

    let (status_code, body) = describe_stack(&router, "elb-two-listeners").await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the second DescribeStacks to succeed, got {status_code} {body}"
    );
    let again = values_of(&wire_outputs(&body));
    assert_eq!(
        again.get("FirstArn").cloned(),
        first,
        "case {case:?}: a second read must report the same identifier, got {again:?}"
    );
}
