// SPDX-License-Identifier: CC0-1.0

//! The Application Load Balancer a CloudFormation stack deploys in front of TWO Amazon ECS
//! services, driven on the emulator a client STARTS: the built binary, on its own port, with
//! request authentication on, reached over TLS with SigV4-signed calls — and the address the
//! stack published for the load balancer DIALLED in the clear, as a client handed that address
//! dials it.
//!
//! RULE: **the address a stack publishes for a load balancer (`LoadBalancer$DNSName`, "the
//! public DNS name of the load balancer") is one a client can open a connection to, and what
//! answers there is decided by the listener the request reaches: the actions of the FIRST rule,
//! in priority order, whose conditions the request meets, and the listener's own
//! `DefaultActions` when none are — so a `forward` rule's request is answered by the workload
//! registered in THAT rule's target group, with the document that workload's own program put
//! where its server serves from, however that program put it there.**
//!
//! The grounds. `LoadBalancer$DNSName` is the address a client sends traffic to, and a name in
//! Elastic Load Balancing's own zone resolves to nothing on the machine this emulator runs on,
//! so the address a stack hands a client has to be one of the emulator's own. "Rules are
//! evaluated in priority order, from the lowest value to the highest value ... When the
//! conditions for a rule are met, its actions are performed. If the conditions for no rules are
//! met, the actions for the default rule are performed" (`CreateRule`), and nothing in a
//! `forward` rewrites the request path, so the path the rule matched is the path the target is
//! asked for. What the target answers with is its own container's filesystem: an official
//! web-server image provides the directory its shipped configuration serves and the page inside
//! it, and a container's `command` — "the command that's passed to the container" — is what puts
//! a document where that server serves it from, whether it COPIES the page the image shipped or
//! writes one of its own.
//!
//! Six crates have to agree for that to hold, which is why this suite lives here:
//! CloudFormation makes every create call the template declares, Amazon EC2 owns the network and
//! the security-group rule that lets a check and a request arrive, Amazon ECS runs each
//! service's container and records the server it listens with at its task's own address, IAM
//! holds the execution role, and Elastic Load Balancing v2 publishes the address, holds the
//! rules and the registrations, and answers the client.
//!
//! What the siblings leave open. [`crate::alb_path_rules_fan_out_to_two_ecs_services`] measures
//! the fan-out on a router assembled in process, through each service's own API, over documents
//! every container WROTE; [`crate::cfn_deployed_ecs_alb_stack_routes_traffic_to_its_task`]
//! deploys one service behind one listener with no rules to choose between. The deploy-and-dial
//! sequence with SEVERAL services split by path, on a started process, where one of them serves
//! what its IMAGE shipped, is the state no suite held.
//!
//! The rows differ along what the rule generalises over: which rule matches (each of the two
//! `forward` rules, and the default action when none does), and where the document the answer
//! carries came from (the page the image shipped, copied under the path the rule routes to it,
//! and one the container's own program wrote).
//!
//! Every clock-owed move — the stack's walk, the tasks' start, the registrations they owe and
//! the health the load balancer's checks determine — is POLLED for until it holds, never slept
//! for, and each deadline is generous: a row asserts that the move HAPPENS, not how fast this
//! host runs.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

use crate::cfn_deployed_pipeline_on_a_started_emulator::Started;
// The ONE reading this test crate keeps of whether an address a load balancer PUBLISHED is one
// of this emulator's own — a `host:port` on the interface the core binds rather than a name in
// Elastic Load Balancing's zone, which resolves to nothing on this machine.
use crate::elbv2_published_address_enforces_its_web_acl::dialable_port;
use crate::query_wire::{elements, members_of, opt_element, outputs_of, target_health_of};

/// The stack this suite deploys, and the status its create settles in.
const STACK: &str = "pathfanout";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long a step waits for the condition it needs — the stack's terminal status, the answer
/// the published address gives — and how often it looks again. Generous on purpose: the deadline
/// is only the backstop that turns something nothing will ever satisfy into a readable failure
/// instead of a hang.
const SETTLE_DEADLINE: Duration = Duration::from_secs(180);
const LOOK: Duration = Duration::from_millis(200);

/// How long one exchange on the published address may take.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// The page the Apache HTTP Server's official image ships in the root its configuration serves,
/// which the first service's program copies under the path its rule routes to it — spelled out
/// here rather than imported, so a document this suite wrote cannot stand in for the image's.
const SHIPPED_PAGE: &str = "<html><body><h1>It works!</h1></body></html>";

/// The document the second service's program writes itself.
const WRITTEN_PAGE: &str = "hello-from-b";

/// The status and body the listener's DEFAULT action answers with, which is what a request no
/// rule matches gets.
const DEFAULT_STATUS: u16 = 404;
const DEFAULT_BODY: &str = "no-match";

/// One row: the path a client asks the published address for, and the status and body it is
/// answered with.
struct Row {
    case: &'static str,
    path: &'static str,
    status: u16,
    body: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "the first rule's path, answered with the page that service's image shipped",
        path: "/svc-a/",
        status: 200,
        body: SHIPPED_PAGE,
    },
    Row {
        case: "the second rule's path, answered by the other service with no leak",
        path: "/svc-b/",
        status: 200,
        body: WRITTEN_PAGE,
    },
    Row {
        case: "a path under neither pattern, which is the listener's default action",
        path: "/no-such-path",
        status: DEFAULT_STATUS,
        body: DEFAULT_BODY,
    },
];

/// The template the stack is deployed with: the network and the two security groups, the ECS
/// cluster and the execution role, an internet-facing Application Load Balancer with a
/// fixed-response default action and one `path-pattern` rule per service, and the two Fargate
/// services, each bound to its own target group.
///
/// The first service's container COPIES the page its image ships under the path its rule routes
/// to it; the second writes its own document there.
fn template() -> String {
    let copies = service_resources(
        "A",
        "svc-a",
        10,
        "mkdir -p /usr/local/apache2/htdocs/svc-a && \
         cp /usr/local/apache2/htdocs/index.html /usr/local/apache2/htdocs/svc-a/index.html && \
         httpd-foreground",
    );
    let writes = service_resources(
        "B",
        "svc-b",
        20,
        &format!(
            "mkdir -p /usr/local/apache2/htdocs/svc-b && echo {WRITTEN_PAGE} > \
             /usr/local/apache2/htdocs/svc-b/index.html && httpd-foreground"
        ),
    );
    let services = format!("{copies}{writes}");
    format!(
        "Resources:
  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: 10.91.0.0/16
  Igw:
    Type: AWS::EC2::InternetGateway
  IgwAttach:
    Type: AWS::EC2::VPCGatewayAttachment
    Properties:
      VpcId: !Ref Vpc
      InternetGatewayId: !Ref Igw
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.91.1.0/24
      AvailabilityZone: us-east-1a
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.91.2.0/24
      AvailabilityZone: us-east-1b
  Routes:
    Type: AWS::EC2::RouteTable
    Properties:
      VpcId: !Ref Vpc
  DefaultRoute:
    Type: AWS::EC2::Route
    DependsOn: IgwAttach
    Properties:
      RouteTableId: !Ref Routes
      DestinationCidrBlock: 0.0.0.0/0
      GatewayId: !Ref Igw
  AssocA:
    Type: AWS::EC2::SubnetRouteTableAssociation
    Properties:
      RouteTableId: !Ref Routes
      SubnetId: !Ref SubnetA
  AlbSg:
    Type: AWS::EC2::SecurityGroup
    Properties:
      GroupDescription: the group the load balancer is placed in
      VpcId: !Ref Vpc
      SecurityGroupIngress:
        - IpProtocol: tcp
          FromPort: 80
          ToPort: 80
          CidrIp: 0.0.0.0/0
  TaskSg:
    Type: AWS::EC2::SecurityGroup
    Properties:
      GroupDescription: the group both services tasks are placed in
      VpcId: !Ref Vpc
      SecurityGroupIngress:
        - IpProtocol: tcp
          FromPort: 80
          ToPort: 80
          SourceSecurityGroupId: !Ref AlbSg
  ExecRole:
    Type: AWS::IAM::Role
    Properties:
      AssumeRolePolicyDocument:
        Version: \"2012-10-17\"
        Statement:
          - Effect: Allow
            Principal:
              Service: ecs-tasks.amazonaws.com
            Action: sts:AssumeRole
  Cluster:
    Type: AWS::ECS::Cluster
    Properties:
      ClusterName: pathfanout
  Alb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Name: pathfanout-alb
      Type: application
      Scheme: internet-facing
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
      SecurityGroups:
        - !Ref AlbSg
  Listener:
    Type: AWS::ElasticLoadBalancingV2::Listener
    Properties:
      LoadBalancerArn: !Ref Alb
      Port: 80
      Protocol: HTTP
      DefaultActions:
        - Type: fixed-response
          FixedResponseConfig:
            StatusCode: \"{DEFAULT_STATUS}\"
            ContentType: text/plain
            MessageBody: {DEFAULT_BODY}
{services}Outputs:
  AlbDns:
    Value: !GetAtt Alb.DNSName
  GroupAArn:
    Value: !Ref GroupA
  GroupBArn:
    Value: !Ref GroupB
"
    )
}

/// The four resources ONE service of the fan-out declares: its target group, the rule that
/// forwards the paths under `prefix` to it, the revision whose container runs `program`, and the
/// service bound to that group.
fn service_resources(suffix: &str, prefix: &str, priority: u32, program: &str) -> String {
    format!(
        "  Group{suffix}:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Name: pathfanout-{prefix}
      Port: 80
      Protocol: HTTP
      TargetType: ip
      VpcId: !Ref Vpc
      HealthCheckPath: /
      HealthCheckIntervalSeconds: 10
      HealthyThresholdCount: 2
  Rule{suffix}:
    Type: AWS::ElasticLoadBalancingV2::ListenerRule
    Properties:
      ListenerArn: !Ref Listener
      Priority: {priority}
      Conditions:
        - Field: path-pattern
          Values:
            - /{prefix}*
      Actions:
        - Type: forward
          TargetGroupArn: !Ref Group{suffix}
  Task{suffix}:
    Type: AWS::ECS::TaskDefinition
    Properties:
      Family: pathfanout-{prefix}
      NetworkMode: awsvpc
      RequiresCompatibilities:
        - FARGATE
      Cpu: \"256\"
      Memory: \"512\"
      ExecutionRoleArn: !GetAtt ExecRole.Arn
      ContainerDefinitions:
        - Name: app
          Image: public.ecr.aws/docker/library/httpd:2.4
          PortMappings:
            - ContainerPort: 80
          Command:
            - /bin/sh
            - -c
            - {program}
  Service{suffix}:
    Type: AWS::ECS::Service
    DependsOn: Rule{suffix}
    Properties:
      ServiceName: pathfanout-{prefix}
      Cluster: !Ref Cluster
      TaskDefinition: !Ref Task{suffix}
      DesiredCount: 1
      LaunchType: FARGATE
      NetworkConfiguration:
        AwsvpcConfiguration:
          Subnets:
            - !Ref SubnetA
          SecurityGroups:
            - !Ref TaskSg
          AssignPublicIp: ENABLED
      LoadBalancers:
        - TargetGroupArn: !Ref Group{suffix}
          ContainerName: app
          ContainerPort: 80
"
    )
}

/// Read the stack until it reports a status that is no longer in progress, and answer that
/// read's body.
fn settled(emulator: &Started) -> String {
    let started = Instant::now();
    loop {
        let (code, body) = emulator.cloudformation("DescribeStacks", &[("StackName", STACK)]);
        assert_eq!(
            code, 200,
            "expected 200 from DescribeStacks, got {code} with {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return body;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "the create never settled; last status {reported:?}, and the reasons its events \
             carried were {:?}",
            reasons(emulator)
        );
        std::thread::sleep(LOOK);
    }
}

/// Every reason the stack's events carry, for the message of a wait that gave up.
fn reasons(emulator: &Started) -> Vec<String> {
    let (_, body) = emulator.cloudformation("DescribeStackEvents", &[("StackName", STACK)]);
    elements(&body, "ResourceStatusReason")
}

/// The `TargetHealth$State` of every registration a target group reports.
fn reported_states(emulator: &Started, group: &str) -> Vec<String> {
    let (code, body) = emulator.form_call(
        "elasticloadbalancing",
        "2015-12-01",
        "DescribeTargetHealth",
        &[("TargetGroupArn", group)],
    );
    assert_eq!(
        code, 200,
        "expected 200 from DescribeTargetHealth on {group}, got {code} with {body}"
    );
    // One state per registration, read inside that registration's own health block: the
    // administrative override reported beside it carries a `State` of its own
    // ([`target_health_of`]).
    members_of(&body)
        .into_iter()
        .flat_map(|description| elements(target_health_of(description), "State"))
        .collect()
}

/// One exchange on the address the stack published: a connection in the clear, HTTP/1.1, closed
/// after the one request — what a client handed that address sends, carrying no AWS credential.
fn exchange(case: &str, published: &str, path: &str) -> String {
    let port = dialable_port(case, published);
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .unwrap_or_else(|error| panic!("case {case:?}: {published} refused a connection: {error}"));
    stream
        .set_read_timeout(Some(EXCHANGE_TIMEOUT))
        .expect("set the read timeout");
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {published}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .unwrap_or_else(|error| panic!("case {case:?}: the request was not sent: {error}"));
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .unwrap_or_else(|error| panic!("case {case:?}: the answer was not read: {error}"));
    String::from_utf8_lossy(&answer).to_string()
}

/// The BODY of one answer: what follows the blank line that ends its header block, and the whole
/// of it where an answer carried no such line at all.
fn body_of(answered: &str) -> &str {
    answered
        .split_once("\r\n\r\n")
        .or_else(|| answered.split_once("\n\n"))
        .map_or(answered, |(_, body)| body)
}

/// RULE: the address a stack publishes for its load balancer is one a client can dial, and the
/// listener's rules decide which service's own document answers each path.
#[test]
fn a_stack_deployed_load_balancer_answers_each_path_from_its_own_services_document() {
    let emulator = Started::start();
    let (code, body) = emulator.cloudformation(
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template()),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    );
    assert_eq!(
        code, 200,
        "expected 200 from CreateStack, got {code} with {body}"
    );
    let described = settled(&emulator);
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some(CREATE_COMPLETE),
        "the create must complete; its reasons were {:?}",
        reasons(&emulator)
    );
    let outputs = outputs_of(&described);
    let published = outputs
        .get("AlbDns")
        .cloned()
        .unwrap_or_else(|| panic!("the stack published no AlbDns, got {described}"));

    // Each service's own task is registered in its OWN group and reaches the state a load
    // balancer may send requests to — polled for, since the task's start, the registration it
    // owes and the health its group's checks determine are all clock-owed.
    for key in ["GroupAArn", "GroupBArn"] {
        let group = outputs
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("the stack published no {key}, got {described}"));
        let started = Instant::now();
        let states = loop {
            let states = reported_states(&emulator, &group);
            if states == ["healthy"] {
                break states;
            }
            if started.elapsed() >= SETTLE_DEADLINE {
                break states;
            }
            std::thread::sleep(LOOK);
        };
        assert_eq!(
            states,
            vec!["healthy".to_string()],
            "case {key:?}: expected the group to hold one registration a request may reach, got \
             {states:?}"
        );
    }

    for row in ROWS {
        let case = row.case;
        let expected_status = format!("HTTP/1.1 {}", row.status);
        // The answer is looked for again until it holds: a container's program runs over spans
        // the clock owes, and a change takes effect at the next request.
        let started = Instant::now();
        let answered = loop {
            let answered = exchange(case, &published, row.path);
            let served =
                answered.starts_with(&expected_status) && body_of(&answered).trim() == row.body;
            if served || started.elapsed() >= SETTLE_DEADLINE {
                break answered;
            }
            std::thread::sleep(LOOK);
        };
        assert!(
            answered.starts_with(&expected_status),
            "case {case:?}: expected {published}{} to answer {}, got {answered:?}",
            row.path,
            row.status
        );
        assert_eq!(
            body_of(&answered).trim(),
            row.body,
            "case {case:?}: expected {published}{} to answer {:?}, got {answered:?}",
            row.path,
            row.body
        );
    }
}
