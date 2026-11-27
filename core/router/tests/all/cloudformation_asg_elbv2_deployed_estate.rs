// SPDX-License-Identifier: CC0-1.0

//! What a client can DO with a load-balanced Auto Scaling estate the moment its stack reports
//! `CREATE_COMPLETE` — the two rules a deployment-then-drive client breaks on.
//!
//! RULE 1 (the estate is addressable). A stack whose create COMPLETED has every resource it
//! declared in the service that OWNS it, under the identifier the stack published, so that
//! service's own read resolves it: `DescribeLoadBalancers(LoadBalancerArns=[<the ARN an
//! output carries>])` answers 200 with that load balancer, whose `State$Code` reaches
//! `active` ("after the load balancer is fully set up and ready to route traffic, its state is
//! `active`", `elasticloadbalancingv2#LoadBalancerState$Code`), and the same holds for the
//! target group and the listener the stack declared beside it. The
//! `LoadBalancerNotFound` refusal stays reserved for an ARN that genuinely names nothing,
//! which the last leg pins — so this rule is "the published ARN resolves", not "the read never
//! refuses".
//!
//! RULE 2 (an ASG launch carries its launch template's instance tags). A launch-template
//! version's `TagSpecifications` are "the tags specification for the resources that are
//! created during instance launch"
//! (`ec2#LaunchTemplateTagSpecificationRequest`), and each entry tags the resources of its own
//! `ResourceType` — "when you create a launch template, you can specify tags for the following
//! resource types only: `instance` | `volume` | `network-interface` |
//! `spot-instances-request`" (`LaunchTemplateTagSpecificationRequest$ResourceType`). So the
//! instance an Auto Scaling group launches from that version is created already carrying every
//! key/value pair of its `instance` entries and none of the pairs scoped to another resource
//! type, whichever way the launch was made, and `DescribeInstances` reports them in the
//! instance's tag set. The group's own tags whose `PropagateAtLaunch` is true ("determines
//! whether the tag is added to new instances as they are launched in the group",
//! `autoscaling#Tag$PropagateAtLaunch`) are applied ON TOP, so a key both sources spell
//! carries the group's value and a group tag that does not propagate reaches nothing.
//!
//! Both rules are driven the way a deploy-then-drive client drives them — one CloudFormation
//! stack, then CloudFormation's, Amazon EC2's, Amazon EC2 Auto Scaling's and Elastic Load
//! Balancing's own wires — which is why the suite lives here: it only means anything with the
//! four services over ONE engine.
//!
//! The rows span what each rule generalises over rather than restating one deployment:
//!
//! * for rule 1, the load-balancer FAMILY and SCHEME the stack declares (an internal Network
//!   Load Balancer, an internal Application Load Balancer, an internet-facing one) and the
//!   other two ELBv2 types a stack publishes an identifier for (a target group, a listener);
//! * for rule 2, WHERE a key comes from (an `instance` specification, a `volume` one, a
//!   `network-interface` one, the group's propagating tags, the group's non-propagating ones,
//!   both the template and the group), the FORM of the key (a namespaced one, a plain one) and
//!   of the value (a word, text carrying spaces), and a group whose template declares no tag
//!   specifications at all.
//!
//! No expected value is taken from a recording: each is either the literal the template
//! declares here or an identity read out of the service that minted it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for query suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{
    block_of, blocks_of, element, elements, form_encode, items_of, members_of, opt_element,
    outputs_of,
};

/// The Region every request below is signed for, and so the Region the estate belongs to.
const REGION: &str = "us-west-2";

/// The API versions the four services' query requests carry.
const CFN_VERSION: &str = "2010-05-15";
const ASG_VERSION: &str = "2011-01-01";
const EC2_VERSION: &str = "2016-11-15";
/// Elastic Load Balancing's two models share one signing name and one endpoint, so the ELBv2
/// model is addressed by the routing key that names IT and by the API version only it declares
/// (`smithy_query(query="service_info")` of that model).
const ELBV2_KEY: &str = "elasticloadbalancingv2";
const ELBV2_VERSION: &str = "2015-12-01";

/// The stack the suite deploys.
const STACK: &str = "deployed-estate";

/// The AMI and the instance type the declared launch data names.
const IMAGE_ID: &str = "ami-0abcdef1234567890";
const INSTANCE_TYPE: &str = "t3.micro";

/// The keys and values the TAGGED launch template's `instance` specification declares: a
/// namespaced key with a one-word value, and a second whose value carries spaces.
const COMPONENT_KEY: &str = "app:component";
const COMPONENT_VALUE: &str = "api-gateway";
const REVISION_KEY: &str = "app:revision";
const REVISION_VALUE: &str = "release 2 of 3";

/// The key BOTH the template's `instance` specification and the group's propagating tags
/// declare, and the value each gives it: the group is the nearer source, so the instance
/// carries the group's.
const SHARED_KEY: &str = "Name";
const TEMPLATE_SHARED_VALUE: &str = "from-the-template";
const GROUP_SHARED_VALUE: &str = "from-the-group";

/// The keys the template scopes to ANOTHER resource type — the volume and the network
/// interfaces a launch creates — which therefore reach no instance.
const VOLUME_KEY: &str = "app:volume-only";
const VOLUME_VALUE: &str = "data";
const INTERFACE_KEY: &str = "app:interface-only";
const INTERFACE_VALUE: &str = "front";

/// The group tag whose declaration says `PropagateAtLaunch: false`, which therefore reaches no
/// instance either.
const UNPROPAGATED_KEY: &str = "app:cost-centre";
const UNPROPAGATED_VALUE: &str = "ledger";

/// The reserved tag key an Auto Scaling group's launch stamps the group's name under, spelled
/// out here rather than imported so a wrong constant in the implementation cannot make this
/// suite pass.
const GROUP_NAME_TAG: &str = "aws:autoscaling:groupName";

/// The lifecycle state a group reports an instance of its own capacity in once it is serving
/// ("`InService` — The instance is running and in service",
/// `autoscaling#Instance$LifecycleState`), which is the state a client waits for before
/// reading the instance.
const IN_SERVICE: &str = "InService";

/// The state code a load balancer that is ready to route traffic carries.
const ACTIVE: &str = "active";

/// The refusal Elastic Load Balancing answers a `DescribeLoadBalancers` naming an ARN it holds
/// no load balancer for with — correct for an ARN that names nothing, and what the last leg
/// holds the service to. The message names the identifier the read could not resolve, which is
/// the one form both of that read's selectors answer with; it is compared as the SERIALIZER
/// writes it, since the quotes around the list are XML entities on the wire.
const NOT_FOUND_CODE: &str = "LoadBalancerNotFound";
fn not_found_message(identifier: &str) -> String {
    crate::query_wire::xml_escape(&format!("Load balancers '[{identifier}]' not found"))
}

/// How long a wait gives a state the clock owes (a stack's status, a launch reaching
/// `InService`, a load balancer reaching `active`) and how often it looks. Generous — a stack
/// settles only once each level of its dependency chain has, which no fixed span names — and
/// only here so a failure is a failure rather than a hang: every wait below waits for its
/// CONDITION, never for a time. The same budget as the sibling suite
/// `cloudformation_asg_instance_launch_template_provenance.rs`, which settles one of these.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// Call one query operation of the service `key` addresses, answered with its status and XML
/// body. The key is both the host label and the credential scope, which are the two signals the
/// router resolves a service by.
async fn query(
    router: &MultiServiceRouter,
    key: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{key}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{key}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One CloudFormation operation that must succeed.
async fn cfn(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "cloudformation", CFN_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "CloudFormation {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One Amazon EC2 Auto Scaling operation that must succeed.
async fn asg(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "autoscaling", ASG_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "Auto Scaling {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One Amazon EC2 operation that must succeed.
async fn ec2(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "ec2", EC2_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "Amazon EC2 {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One Elastic Load Balancing v2 read that must succeed — which is the whole of rule 1 for the
/// resource it names: a read refused here is a stack that reported a resource its own service
/// does not hold.
async fn elbv2(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(router, ELBV2_KEY, ELBV2_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Elastic Load Balancing must hold what the CREATE_COMPLETE stack \
         published, so {action} naming {members:?} must answer 200; got {status} with body \
         {body}"
    );
    body
}

/// The template the stack declares: the network, two launch templates — one declaring instance,
/// volume and network-interface tag specifications and one declaring none — three load
/// balancers spanning family and scheme, the target group and listener of the Network Load
/// Balancer, and the two Auto Scaling groups that launch from the two templates.
fn template() -> String {
    format!(
        r#"Resources:
  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: 10.0.0.0/16
  Igw:
    Type: AWS::EC2::InternetGateway
  IgwAttachment:
    Type: AWS::EC2::VPCGatewayAttachment
    Properties:
      VpcId: !Ref Vpc
      InternetGatewayId: !Ref Igw
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.0.1.0/24
      AvailabilityZone: "{REGION}a"
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.0.2.0/24
      AvailabilityZone: "{REGION}b"
  Sg:
    Type: AWS::EC2::SecurityGroup
    Properties:
      GroupDescription: "the instances of the declared groups"
      VpcId: !Ref Vpc
  TaggedLt:
    Type: AWS::EC2::LaunchTemplate
    Properties:
      LaunchTemplateData:
        ImageId: "{IMAGE_ID}"
        InstanceType: "{INSTANCE_TYPE}"
        SecurityGroupIds:
          - !GetAtt Sg.GroupId
        TagSpecifications:
          - ResourceType: instance
            Tags:
              - Key: "{COMPONENT_KEY}"
                Value: "{COMPONENT_VALUE}"
              - Key: "{REVISION_KEY}"
                Value: "{REVISION_VALUE}"
              - Key: "{SHARED_KEY}"
                Value: "{TEMPLATE_SHARED_VALUE}"
          - ResourceType: volume
            Tags:
              - Key: "{VOLUME_KEY}"
                Value: "{VOLUME_VALUE}"
          - ResourceType: network-interface
            Tags:
              - Key: "{INTERFACE_KEY}"
                Value: "{INTERFACE_VALUE}"
  PlainLt:
    Type: AWS::EC2::LaunchTemplate
    Properties:
      LaunchTemplateData:
        ImageId: "{IMAGE_ID}"
        InstanceType: "{INSTANCE_TYPE}"
        SecurityGroupIds:
          - !GetAtt Sg.GroupId
  Nlb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Type: network
      Scheme: internal
      Subnets:
        - !Ref SubnetA
  InternalAlb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Type: application
      Scheme: internal
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
  PublicAlb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Type: application
      Scheme: internet-facing
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
  Tg:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Port: 80
      Protocol: TCP
      TargetType: instance
      VpcId: !Ref Vpc
  NlbListener:
    Type: AWS::ElasticLoadBalancingV2::Listener
    Properties:
      LoadBalancerArn: !Ref Nlb
      Port: 80
      Protocol: TCP
      DefaultActions:
        - Type: forward
          TargetGroupArn: !Ref Tg
  TaggedAsg:
    Type: AWS::AutoScaling::AutoScalingGroup
    Properties:
      MinSize: "1"
      MaxSize: "1"
      DesiredCapacity: "1"
      LaunchTemplate:
        LaunchTemplateId: !Ref TaggedLt
        Version: !GetAtt TaggedLt.LatestVersionNumber
      VPCZoneIdentifier:
        - !Ref SubnetA
      TargetGroupARNs:
        - !Ref Tg
      Tags:
        - Key: "{SHARED_KEY}"
          Value: "{GROUP_SHARED_VALUE}"
          PropagateAtLaunch: true
        - Key: "{UNPROPAGATED_KEY}"
          Value: "{UNPROPAGATED_VALUE}"
          PropagateAtLaunch: false
  PlainAsg:
    Type: AWS::AutoScaling::AutoScalingGroup
    Properties:
      MinSize: "1"
      MaxSize: "1"
      DesiredCapacity: "1"
      LaunchTemplate:
        LaunchTemplateId: !Ref PlainLt
        Version: !GetAtt PlainLt.LatestVersionNumber
      VPCZoneIdentifier:
        - !Ref SubnetB
      Tags:
        - Key: "{SHARED_KEY}"
          Value: "{GROUP_SHARED_VALUE}"
          PropagateAtLaunch: true
Outputs:
  VpcId:
    Value: !Ref Vpc
  NlbArn:
    Value: !Ref Nlb
  InternalAlbArn:
    Value: !Ref InternalAlb
  PublicAlbArn:
    Value: !Ref PublicAlb
  TgArn:
    Value: !Ref Tg
  ListenerArn:
    Value: !Ref NlbListener
  TaggedAsgName:
    Value: !Ref TaggedAsg
  PlainAsgName:
    Value: !Ref PlainAsg
"#
    )
}

/// Whether a stack status is one the stack settles at, rather than one it passes through.
fn settled(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack settles, and answer `(status, body)`.
async fn settle(router: &MultiServiceRouter) -> (String, String) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = cfn(router, "DescribeStacks", &[("StackName", STACK)]).await;
        let status = element(&body, "StackStatus");
        if settled(&status) {
            return (status, body);
        }
        assert!(
            Instant::now() < deadline,
            "the stack never settled: it is {status:?} in {body}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The stack's FAILED resource events, each as `logical id: status: reason` — what a deployment
/// that did not complete has to say for itself.
async fn failed_events(router: &MultiServiceRouter) -> Vec<String> {
    let body = cfn(router, "DescribeStackEvents", &[("StackName", STACK)]).await;
    members_of(&body)
        .into_iter()
        .filter_map(|event| {
            let status = elements(event, "ResourceStatus").into_iter().next()?;
            if !status.contains("FAILED") {
                return None;
            }
            let logical_id = elements(event, "LogicalResourceId").into_iter().next()?;
            let reason = elements(event, "ResourceStatusReason")
                .into_iter()
                .next()
                .unwrap_or_default();
            Some(format!("{logical_id}: {status}: {reason}"))
        })
        .collect()
}

/// The value of one declared output, which a CREATE_COMPLETE stack must carry.
fn output<'a>(outputs: &'a BTreeMap<String, String>, case: &str, key: &str) -> &'a str {
    outputs
        .get(key)
        .map(String::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the stack must publish {key:?}, got {outputs:?}"))
}

/// The `DescribeLoadBalancers` answer for the ONE load balancer the stack published under
/// `arn`, polled until its `State$Code` is `active` — the state the clock owes a load balancer
/// its create brought up.
///
/// Every look is a read of rule 1 as well, since the read is refused outright for an ARN the
/// service holds nothing for. The answer must report that ARN and no other: the whole list of
/// `LoadBalancerArn` elements is compared, so an answer carrying a second load balancer fails
/// rather than being read as this one.
async fn active_load_balancer(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = elbv2(
            router,
            case,
            "DescribeLoadBalancers",
            &[("LoadBalancerArns.member.1", arn)],
        )
        .await;
        let reported = elements(&body, "LoadBalancerArn");
        assert_eq!(
            reported,
            vec![arn.to_string()],
            "case {case:?}: a read selecting the one ARN {arn} must report exactly that load \
             balancer, got {reported:?} in {body}"
        );
        let code = block_of(&body, "State").and_then(|state| opt_element(state, "Code"));
        if code.as_deref() == Some(ACTIVE) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the load balancer {arn} never reached {ACTIVE:?}: its state code is \
             {code:?} in {body}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The ids of the instances the group `group` reports IN SERVICE, polled until it reports at
/// least `wanted` of them.
///
/// Each entry is read inside the `<member>` that carries BOTH its id and its lifecycle state,
/// scoped to the group's own `Instances` list, so an id is never paired with another entry's
/// state.
async fn in_service_instances(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    wanted: usize,
) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = asg(
            router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", group)],
        )
        .await;
        let mut serving = Vec::new();
        for entry in block_of(&body, "Instances")
            .map(members_of)
            .unwrap_or_default()
        {
            let state = elements(entry, "LifecycleState").into_iter().next();
            let instance_id = elements(entry, "InstanceId").into_iter().next();
            if let (Some(state), Some(instance_id)) = (state, instance_id) {
                if state == IN_SERVICE {
                    serving.push(instance_id);
                }
            }
        }
        if serving.len() >= wanted {
            return serving;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {group} to report {wanted} {IN_SERVICE} instance(s), got \
             {serving:?} in {body}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The tags Amazon EC2 holds for ONE instance, as the map a client reads them as.
///
/// Read through `DescribeInstances` naming exactly that instance, which is where a client reads
/// an instance's tags: the read is scoped to the answer's TAG SETS, so no other keyed list of
/// the document is folded in, and each pair is read inside the `<item>` that carries both
/// halves, so a tag never takes another's value.
async fn instance_tags(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> BTreeMap<String, String> {
    let body = ec2(router, "DescribeInstances", &[("InstanceId.1", instance_id)]).await;
    assert!(
        body.contains(instance_id),
        "case {case:?}: expected Amazon EC2 to report {instance_id} in {REGION}, got {body}"
    );
    blocks_of(&body, "tagSet")
        .into_iter()
        .flat_map(items_of)
        .filter_map(|item| {
            let key = elements(item, "key").into_iter().next()?;
            let value = elements(item, "value").into_iter().next()?;
            Some((key, value))
        })
        .collect()
}

/// One load-balancer row of rule 1: the output the stack publishes its ARN under, and the
/// family and scheme the declaration asked for — which that service must report back.
struct LoadBalancerRow {
    case: &'static str,
    output: &'static str,
    family: &'static str,
    scheme: &'static str,
}

/// The load balancers the stack declares, spanning family and scheme.
const LOAD_BALANCERS: &[LoadBalancerRow] = &[
    LoadBalancerRow {
        case: "an internal network load balancer",
        output: "NlbArn",
        family: "network",
        scheme: "internal",
    },
    LoadBalancerRow {
        case: "an internal application load balancer",
        output: "InternalAlbArn",
        family: "application",
        scheme: "internal",
    },
    LoadBalancerRow {
        case: "an internet-facing application load balancer",
        output: "PublicAlbArn",
        family: "application",
        scheme: "internet-facing",
    },
];

/// One tag row of rule 2: the key, and what the instance must carry for it — `None` where the
/// key must be ABSENT from the instance's tag set.
struct TagRow {
    case: &'static str,
    key: &'static str,
    expected: Option<&'static str>,
}

/// What the instance of the group launching from the TAGGED template must carry.
const TAGGED_ROWS: &[TagRow] = &[
    TagRow {
        case: "a namespaced key of the template's instance specification",
        key: COMPONENT_KEY,
        expected: Some(COMPONENT_VALUE),
    },
    TagRow {
        case: "a second key of the same specification, whose value carries spaces",
        key: REVISION_KEY,
        expected: Some(REVISION_VALUE),
    },
    TagRow {
        case: "a key the template declares and the group propagates, which the group wins",
        key: SHARED_KEY,
        expected: Some(GROUP_SHARED_VALUE),
    },
    TagRow {
        case: "a key the template scopes to the launch's volume",
        key: VOLUME_KEY,
        expected: None,
    },
    TagRow {
        case: "a key the template scopes to the launch's network interfaces",
        key: INTERFACE_KEY,
        expected: None,
    },
    TagRow {
        case: "a group tag whose PropagateAtLaunch is false",
        key: UNPROPAGATED_KEY,
        expected: None,
    },
];

/// What the instance of the group whose template declares NO tag specifications must carry: the
/// group's own propagating tag, and none of the other template's keys.
const PLAIN_ROWS: &[TagRow] = &[
    TagRow {
        case: "the group's propagating tag, with no template tag to merge",
        key: SHARED_KEY,
        expected: Some(GROUP_SHARED_VALUE),
    },
    TagRow {
        case: "a key only the OTHER template's instance specification declares",
        key: COMPONENT_KEY,
        expected: None,
    },
    TagRow {
        case: "a second key only the other template declares",
        key: REVISION_KEY,
        expected: None,
    },
];

/// One group row of rule 2: the group the stack declared, the output publishing its name, and
/// what the instance it launches must carry.
struct GroupRow {
    case: &'static str,
    output: &'static str,
    rows: &'static [TagRow],
}

/// The two groups the stack declares: one launching from the template that declares tag
/// specifications, one from the template that declares none.
const GROUPS: &[GroupRow] = &[
    GroupRow {
        case: "the group launching from the template that declares tag specifications",
        output: "TaggedAsgName",
        rows: TAGGED_ROWS,
    },
    GroupRow {
        case: "the group launching from the template that declares none",
        output: "PlainAsgName",
        rows: PLAIN_ROWS,
    },
];

/// Hold one instance's tag set to `rows`, and to the reserved tag naming its group.
async fn assert_instance_tags(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    instance_id: &str,
    rows: &[TagRow],
) {
    let read = instance_tags(router, case, instance_id).await;
    for row in rows {
        let case = format!("{case}: {}", row.case);
        let held = read.get(row.key).map(String::as_str);
        assert_eq!(
            held,
            row.expected,
            "case {case:?}: expected the instance {instance_id} of {group} to carry {}={:?}, \
             got {held:?} among {read:?}",
            row.key,
            row.expected
        );
    }
    let held = read.get(GROUP_NAME_TAG).map(String::as_str);
    let case = format!("{case}: the reserved tag naming the group");
    assert_eq!(
        held,
        Some(group),
        "case {case:?}: expected the instance {instance_id} to carry \
         {GROUP_NAME_TAG}={group:?}, got {held:?} among {read:?}"
    );
}

/// The published ARN with its last hexadecimal digit changed: the partition, the Region, the
/// account and the family are the real one's, so it is a well-formed load-balancer ARN that
/// nothing created — which is the only thing `LoadBalancerNotFound` is for.
fn an_arn_nothing_created(arn: &str) -> String {
    let (head, last) = arn.split_at(arn.len() - 1);
    let replacement = if last == "0" { "1" } else { "0" };
    format!("{head}{replacement}")
}

#[tokio::test]
async fn a_create_complete_estate_is_served_by_the_services_that_own_it() {
    let router = router();

    cfn(
        &router,
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", template().as_str())],
    )
    .await;
    let (status, settled_body) = settle(&router).await;
    // The failure events are read for the message's sake, and only where there is a failure to
    // explain: a create that did not complete says which resource refused and why.
    let failures = if status == "CREATE_COMPLETE" {
        Vec::new()
    } else {
        failed_events(&router).await
    };
    assert_eq!(
        status, "CREATE_COMPLETE",
        "expected the estate to deploy, got {status:?} with the failures {failures:?}"
    );
    let outputs: BTreeMap<String, String> = outputs_of(&settled_body);
    let vpc_id = output(&outputs, "the deployed estate", "VpcId").to_string();

    // RULE 1, the load balancers: every ARN the stack published resolves in Elastic Load
    // Balancing, which reports the family, the scheme and the VPC the declaration asked for and
    // brings the load balancer to `active`.
    for row in LOAD_BALANCERS {
        let case = row.case;
        let arn = output(&outputs, case, row.output).to_string();
        let described = active_load_balancer(&router, case, &arn).await;
        for (member, expected) in [
            ("Type", row.family),
            ("Scheme", row.scheme),
            ("VpcId", vpc_id.as_str()),
        ] {
            let reported = opt_element(&described, member);
            assert_eq!(
                reported.as_deref(),
                Some(expected),
                "case {case:?}: expected Elastic Load Balancing to report {member}={expected:?} \
                 for the load balancer the stack published, got {reported:?} in {described}"
            );
        }
    }

    // RULE 1, the other two ELBv2 types the stack published an identifier for: the target group
    // the Network Load Balancer routes to, which that service reports as the load balancer's
    // own, and the listener on it.
    let nlb_arn = output(&outputs, "the deployed estate", "NlbArn").to_string();
    let case = "the target group a declared listener routes to";
    let tg_arn = output(&outputs, case, "TgArn").to_string();
    let described_group = elbv2(
        &router,
        case,
        "DescribeTargetGroups",
        &[("TargetGroupArns.member.1", tg_arn.as_str())],
    )
    .await;
    assert_eq!(
        opt_element(&described_group, "TargetGroupArn").as_deref(),
        Some(tg_arn.as_str()),
        "case {case:?}: expected the target group the stack published to resolve in Elastic Load \
         Balancing, got {described_group}"
    );
    let routing = block_of(&described_group, "LoadBalancerArns")
        .map(|list| elements(list, "member"))
        .unwrap_or_default();
    assert!(
        routing.iter().any(|arn| arn == &nlb_arn),
        "case {case:?}: expected the target group to report the stack's load balancer \
         {nlb_arn:?} among the load balancers routing to it, got {routing:?} in {described_group}"
    );

    let case = "the listener a declared load balancer accepts connections on";
    let listener_arn = output(&outputs, case, "ListenerArn").to_string();
    let described_listener = elbv2(
        &router,
        case,
        "DescribeListeners",
        &[("ListenerArns.member.1", listener_arn.as_str())],
    )
    .await;
    for (member, expected) in [
        ("ListenerArn", listener_arn.as_str()),
        ("LoadBalancerArn", nlb_arn.as_str()),
    ] {
        let reported = opt_element(&described_listener, member);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected the listener the stack published to report \
             {member}={expected:?}, got {reported:?} in {described_listener}"
        );
    }

    // RULE 2: the instance each declared group launched carries its launch template's instance
    // tags, and nothing the template scoped elsewhere.
    for row in GROUPS {
        let case = row.case;
        let group = output(&outputs, case, row.output).to_string();
        let serving = in_service_instances(&router, case, &group, 1).await;
        for instance_id in &serving {
            assert_instance_tags(&router, case, &group, instance_id, row.rows).await;
        }
    }

    // The counter-row: `LoadBalancerNotFound` is still what an ARN that names nothing is refused
    // with, so rule 1 is "the published ARN resolves" rather than "this read never refuses".
    let case = "a well-formed load-balancer ARN nothing created";
    let absent = an_arn_nothing_created(&nlb_arn);
    let (status, body) = query(
        &router,
        ELBV2_KEY,
        ELBV2_VERSION,
        "DescribeLoadBalancers",
        &[("LoadBalancerArns.member.1", absent.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {absent} to be refused 400, got {status} with body {body}"
    );
    let not_found = not_found_message(&absent);
    for (member, expected) in [("Code", NOT_FOUND_CODE), ("Message", not_found.as_str())] {
        let reported = opt_element(&body, member);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected the refusal to carry {member}={expected:?}, got \
             {reported:?} in {body}"
        );
    }
}
