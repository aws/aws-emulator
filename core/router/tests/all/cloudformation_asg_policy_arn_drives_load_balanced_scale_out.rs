// SPDX-License-Identifier: CC0-1.0

//! Driving the scale-out of a load-balanced Auto Scaling estate with NOTHING but the
//! identifier its stack published for the scaling policy.
//!
//! RULE: a scaling policy named by its ARN is resolved on its own, because that ARN carries
//! the group it belongs to — "the name or ARN of the policy" (`ExecutePolicyType$PolicyName`,
//! required) beside an OPTIONAL `ExecutePolicyType$AutoScalingGroupName` — so a request that
//! spells the policy as an ARN and carries no group is SERVED, while a request that spells it
//! as a bare NAME, unique only inside a group, still needs that group stated. A stack's
//! reference to an `AWS::AutoScaling::ScalingPolicy` IS that ARN ("Ref returns the Amazon
//! Resource Name (ARN) of the scaling policy"), so the one identifier a client holding only
//! the stack's outputs has is enough to perform the policy. Performing it applies the
//! policy's own adjustment to the group's `DesiredCapacity`, held inside its
//! `MinSize`..`MaxSize`, and the group then converges exactly as it does for any other change
//! of its size: it launches or scales in instances, each launched one reaches `InService`, and
//! every attached target group ends up SERVING exactly the group's in-service instances —
//! whose health the load balancer's own checks decide — while the registration of one the
//! change took out drains out of it.
//!
//! Five crates have to agree for that to hold, which is why the suite lives here:
//! CloudFormation makes the creates, Amazon EC2 holds the network and the launch template,
//! Amazon EC2 Auto Scaling holds the group and the policy and launches the instances, and
//! Elastic Load Balancing v2 holds the target group, the load balancer and the health verdict.
//! Every assertion reads what one of those services persisted, through the operation a client
//! polls.
//!
//! `cloudformation_asg_scaling_policy_execution.rs` covers the policy a stack declares being
//! read back, performed and removed; `cloudformation_asg_target_group_convergence.rs` covers a
//! stack-declared group converging into its target group. What this suite adds is the two of
//! them JOINED, which is where a client that deploys the template-driven load-balanced service
//! and then forces the scale-out lives: the policy is performed by the published ARN ALONE and
//! the capacity change it makes is followed all the way into the target group's health.
//!
//! The rows differ along what the rule generalises over: the adjustment type the policy states
//! (`ChangeInCapacity`, `ExactCapacity`), the SIGN of the change (out and in), how many
//! instances the change moves, and WHERE the ARN performed came from — the identifier the
//! stack published, or the `PolicyARN` the owning service's own `DescribePolicies` reports,
//! which the rows also assert are the same value, so a row cannot pass by the stack having
//! published something only the stack understands.
//!
//! No expected value is taken from a recording: each is either a literal this file's own
//! template declares or an identity read out of the service that minted it.

use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The ONE form encoder and XML element readers this test crate keeps for query suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{block_of, elements, form_encode, members_of, opt_element, outputs_of};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions the three services' query requests carry.
const CFN_VERSION: &str = "2010-05-15";
const ASG_VERSION: &str = "2011-01-01";
const ELBV2_VERSION: &str = "2015-12-01";

/// The AMI and the instance type the declared launch data names.
const IMAGE_ID: &str = "ami-0abcdef1234567890";
const INSTANCE_TYPE: &str = "t3.micro";

/// The stack status a create must finish in, and the ones it is still in flight in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const ROLLBACK_IN_PROGRESS: &str = "ROLLBACK_IN_PROGRESS";

/// The lifecycle state a group reports an instance in once it is serving ("`InService` — The
/// instance is running and in service", `autoscaling#Instance$LifecycleState`).
const IN_SERVICE: &str = "InService";

/// The `TargetHealth$State` a registration the load balancer's checks reach settles on.
const HEALTHY: &str = "healthy";

/// The `TargetHealth$State` of a registration that is being DEREGISTERED — "the load balancer
/// is draining connections to the target" — which is where a target an instance the group
/// scaled in leaves behind sits until the group's deregistration delay ("the amount of time for
/// Elastic Load Balancing to wait before changing the state of a deregistering target from
/// `draining` to `unused`") has run. A draining registration is therefore not one the load
/// balancer serves, which is what the rows below read the serving set against.
const DRAINING: &str = "draining";

/// The error code this service answers a bad request under, and the whole sentence an
/// operation acting on ONE scaling policy refuses a bare policy name carrying no
/// `AutoScalingGroupName` with. Spelled out here rather than imported, so a wrong constant in
/// the implementation cannot make a row pass.
const VALIDATION_ERROR: &str = "ValidationError";
const GROUP_REQUIRED_FOR_POLICY_NAME: &str =
    "The AutoScaling Group name is required when specifying a policy by its name";

/// The cadence and the threshold the target group of every row is checked at: the shortest
/// the members admit ("the range is 5-300" for `CreateTargetGroup$HealthCheckIntervalSeconds`,
/// "the range is 2-10" for `$HealthyThresholdCount`), so a verdict is a threshold of checks
/// away rather than a longer wait.
const CHECK_INTERVAL: &str = "5";
const HEALTHY_THRESHOLD: &str = "2";

/// How long a row waits for the clock-owed moves it depends on — the stack's resources coming
/// up, the instances the group launches, their registration and the checks of each
/// registration — and how often it looks again. Generous, because the deadline is only here so
/// a failure is a failure rather than a hang: every wait below looks for its CONDITION, so
/// machine load decides when a row finishes and never whether it passes.
const DEADLINE: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(100);

/// Where a row takes the policy ARN it performs from — the two values a client can hold, which
/// the rule says are one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnFrom {
    /// The identifier the stack publishes for the resource, which is all a client holding the
    /// deployment's outputs has.
    StackOutput,
    /// The `PolicyARN` the owning service reports for the policy through its own
    /// `DescribePolicies`.
    DescribePolicies,
}

/// One row: the estate one stack declares, the policy it declares on the group, where the ARN
/// performed is read from, and the capacity the group is left maintaining.
struct Row {
    case: &'static str,
    stack: &'static str,
    group: &'static str,
    target_group: &'static str,
    load_balancer: &'static str,
    /// `MinSize`, `MaxSize` and the `DesiredCapacity` the group is declared with.
    min: usize,
    max: usize,
    desired: usize,
    /// `PutScalingPolicyType$AdjustmentType` and `$ScalingAdjustment` the declaration states.
    adjustment_type: &'static str,
    scaling_adjustment: &'static str,
    arn_from: ArnFrom,
    /// The `DesiredCapacity` the group must maintain once the policy has been performed, which
    /// is also the number of instances it must then hold in service and register.
    expected: usize,
}

/// The recorded shape: one instance behind an internal Application Load Balancer, scaled out by
/// one through the identifier the stack published.
const SCALE_OUT_BY_ONE: Row = Row {
    case: "a ChangeInCapacity +1 policy performed by the identifier the stack published",
    stack: "asgpolarn-out",
    group: "asgpolarn-out-asg",
    target_group: "asgpolarn-out-tg",
    load_balancer: "asgpolarn-out-alb",
    min: 1,
    max: 3,
    desired: 1,
    adjustment_type: "ChangeInCapacity",
    scaling_adjustment: "1",
    arn_from: ArnFrom::StackOutput,
    expected: 2,
};

/// The same estate whose policy names the capacity outright, performed by the ARN the owning
/// service itself reports.
const SCALE_OUT_TO_EXACT_CAPACITY: Row = Row {
    case: "an ExactCapacity policy performed by the ARN DescribePolicies reports",
    stack: "asgpolarn-exact",
    group: "asgpolarn-exact-asg",
    target_group: "asgpolarn-exact-tg",
    load_balancer: "asgpolarn-exact-alb",
    min: 1,
    max: 4,
    desired: 1,
    adjustment_type: "ExactCapacity",
    scaling_adjustment: "3",
    arn_from: ArnFrom::DescribePolicies,
    expected: 3,
};

/// The other sign of the same rule: a policy that takes the group DOWN, which has to leave the
/// target group holding only the instances that remain.
const SCALE_IN_BY_ONE: Row = Row {
    case: "a ChangeInCapacity -1 policy performed by the identifier the stack published",
    stack: "asgpolarn-in",
    group: "asgpolarn-in-asg",
    target_group: "asgpolarn-in-tg",
    load_balancer: "asgpolarn-in-alb",
    min: 1,
    max: 3,
    desired: 2,
    adjustment_type: "ChangeInCapacity",
    scaling_adjustment: "-1",
    arn_from: ArnFrom::StackOutput,
    expected: 1,
};

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// One form body: the action, the version and the parameters.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// Send one form-encoded request to the service `signing_name` addresses, and answer its status
/// and body.
async fn send(router: &MultiServiceRouter, signing_name: &str, body: String) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/\
                 {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation action that must succeed.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = send(router, "cloudformation", form(action, CFN_VERSION, params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One Amazon EC2 Auto Scaling action, as its status and body — the form a row that asserts a
/// REFUSAL needs.
async fn asg_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    send(router, "autoscaling", form(action, ASG_VERSION, params)).await
}

/// One Amazon EC2 Auto Scaling action that must succeed.
async fn asg(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = asg_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One Elastic Load Balancing v2 action that must succeed. Both Elastic Load Balancing models
/// share one signing name, so the v2 API version is what names this one.
async fn elbv2(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = send(
        router,
        "elasticloadbalancing",
        form(action, ELBV2_VERSION, params),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The template one row deploys: the network, the one security group whose self-referencing
/// inbound rule admits the load balancer's checks, the launch template, the target group, the
/// internal Application Load Balancer and its listener, the Auto Scaling group that names the
/// target group under `TargetGroupARNs`, and the scaling policy declared on that group.
fn template(row: &Row) -> String {
    format!(
        r##"Resources:
  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: 10.40.0.0/16
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.40.1.0/24
      AvailabilityZone: {REGION}a
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.40.2.0/24
      AvailabilityZone: {REGION}b
  InstanceSecurityGroup:
    Type: AWS::EC2::SecurityGroup
    Properties:
      GroupDescription: the instances of the group and the load balancer in front of them
      VpcId: !Ref Vpc
  InstanceIngress:
    Type: AWS::EC2::SecurityGroupIngress
    Properties:
      GroupId: !Ref InstanceSecurityGroup
      IpProtocol: tcp
      FromPort: 80
      ToPort: 80
      SourceSecurityGroupId: !Ref InstanceSecurityGroup
  LaunchTemplate:
    Type: AWS::EC2::LaunchTemplate
    Properties:
      LaunchTemplateName: {group}-lt
      LaunchTemplateData:
        ImageId: {IMAGE_ID}
        InstanceType: {INSTANCE_TYPE}
        SecurityGroupIds:
          - !Ref InstanceSecurityGroup
  TargetGroup:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Name: {target_group}
      Port: 80
      Protocol: HTTP
      TargetType: instance
      VpcId: !Ref Vpc
      HealthCheckIntervalSeconds: {CHECK_INTERVAL}
      HealthyThresholdCount: {HEALTHY_THRESHOLD}
  LoadBalancer:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Name: {load_balancer}
      Scheme: internal
      Type: application
      SecurityGroups:
        - !Ref InstanceSecurityGroup
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
  Listener:
    Type: AWS::ElasticLoadBalancingV2::Listener
    Properties:
      LoadBalancerArn: !Ref LoadBalancer
      Port: 80
      Protocol: HTTP
      DefaultActions:
        - Type: forward
          TargetGroupArn: !Ref TargetGroup
  AutoScalingGroup:
    Type: AWS::AutoScaling::AutoScalingGroup
    DependsOn: Listener
    Properties:
      AutoScalingGroupName: {group}
      MinSize: "{min}"
      MaxSize: "{max}"
      DesiredCapacity: "{desired}"
      VPCZoneIdentifier:
        - !Ref SubnetA
        - !Ref SubnetB
      LaunchTemplate:
        LaunchTemplateId: !Ref LaunchTemplate
        Version: !GetAtt LaunchTemplate.LatestVersionNumber
      TargetGroupARNs:
        - !Ref TargetGroup
  ScalePolicy:
    Type: AWS::AutoScaling::ScalingPolicy
    Properties:
      AutoScalingGroupName: !Ref AutoScalingGroup
      PolicyType: SimpleScaling
      AdjustmentType: {adjustment_type}
      ScalingAdjustment: "{scaling_adjustment}"
      Cooldown: "0"
Outputs:
  TargetGroupArn:
    Value: !Ref TargetGroup
  AutoScalingGroupName:
    Value: !Ref AutoScalingGroup
  ScalingPolicyArn:
    Value: !Ref ScalePolicy
  ScalingPolicyName:
    Value: !GetAtt ScalePolicy.PolicyName
"##,
        group = row.group,
        target_group = row.target_group,
        load_balancer = row.load_balancer,
        min = row.min,
        max = row.max,
        desired = row.desired,
        adjustment_type = row.adjustment_type,
        scaling_adjustment = row.scaling_adjustment,
    )
}

/// The stack as `DescribeStacks` reports it once the create in flight on it has SETTLED: read
/// again until its status is terminal, or the deadline passes.
async fn settled_stack(router: &MultiServiceRouter, row: &Row) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = cloudformation(
            router,
            row.case,
            "DescribeStacks",
            &[("StackName", row.stack)],
        )
        .await;
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        let in_flight = reported == CREATE_IN_PROGRESS || reported == ROLLBACK_IN_PROGRESS;
        if !in_flight || Instant::now() >= deadline {
            return body;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Every `ResourceStatusReason` one stack's event stream carries, which is where the refusal a
/// failing resource earned is reported.
async fn stack_failures(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let body = cloudformation(
        router,
        row.case,
        "DescribeStackEvents",
        &[("StackName", row.stack)],
    )
    .await;
    elements(&body, "ResourceStatusReason")
}

/// One output of the settled stack, asserted present and non-empty.
fn published(row: &Row, described: &str, key: &str) -> String {
    let outputs = outputs_of(described);
    let value = outputs.get(key).cloned().unwrap_or_else(|| {
        panic!(
            "case {:?}: expected the stack to publish an output named {key:?}, got {outputs:?}",
            row.case
        )
    });
    assert!(
        !value.is_empty(),
        "case {:?}: expected the output {key:?} to carry a value, got {outputs:?}",
        row.case
    );
    value
}

/// The one policy the owning service reports for the row's group, as `(PolicyName, PolicyARN)`.
async fn reported_policy(router: &MultiServiceRouter, row: &Row) -> (String, String) {
    let body = asg(
        router,
        row.case,
        "DescribePolicies",
        &[("AutoScalingGroupName", row.group)],
    )
    .await;
    let reported: Vec<(String, String)> = block_of(&body, "ScalingPolicies")
        .map(members_of)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|member| {
            Some((
                elements(member, "PolicyName").into_iter().next()?,
                elements(member, "PolicyARN").into_iter().next()?,
            ))
        })
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "case {:?}: expected the owning service to report exactly the declared policy for {:?}, \
         got {reported:?}",
        row.case,
        row.group
    );
    reported.into_iter().next().expect("one reported policy")
}

/// The group as the owning service reports it right now.
async fn described_group(router: &MultiServiceRouter, row: &Row) -> String {
    asg(
        router,
        row.case,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", row.group)],
    )
    .await
}

/// The `DesiredCapacity` the group maintains right now.
async fn desired_capacity(router: &MultiServiceRouter, row: &Row) -> String {
    let body = described_group(router, row).await;
    opt_element(&body, "DesiredCapacity").unwrap_or_default()
}

/// The instance ids the group reports in the `InService` lifecycle state, read off the member
/// that carries BOTH the id and the state, so an instance in another state is never mistaken
/// for one in service.
async fn in_service_instances(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let body = described_group(router, row).await;
    let Some(instances) = block_of(&body, "Instances") else {
        return Vec::new();
    };
    let mut found: Vec<String> = members_of(instances)
        .into_iter()
        .filter(|member| {
            elements(member, "LifecycleState")
                .first()
                .is_some_and(|state| state == IN_SERVICE)
        })
        .filter_map(|member| elements(member, "InstanceId").into_iter().next())
        .collect();
    found.sort();
    found
}

/// Wait until the group holds exactly `wanted` instances in service, and answer their ids.
async fn settled_capacity(router: &MultiServiceRouter, row: &Row, wanted: usize) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = in_service_instances(router, row).await;
        if found.len() == wanted || Instant::now() >= deadline {
            return found;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// One registration as `DescribeTargetHealth` reports it: the target's id and the state it is
/// in, read off the description that carries both.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registered {
    id: String,
    state: String,
}

/// Every registration the target group `arn` holds right now, in id order.
async fn target_health(router: &MultiServiceRouter, row: &Row, arn: &str) -> Vec<Registered> {
    let body = elbv2(
        router,
        row.case,
        "DescribeTargetHealth",
        &[("TargetGroupArn", arn)],
    )
    .await;
    let Some(descriptions) = block_of(&body, "TargetHealthDescriptions") else {
        return Vec::new();
    };
    let mut found: Vec<Registered> = members_of(descriptions)
        .into_iter()
        .filter_map(|member| {
            let target = block_of(member, "Target")?;
            let health = block_of(member, "TargetHealth").unwrap_or_default();
            Some(Registered {
                id: elements(target, "Id").into_iter().next()?,
                state: elements(health, "State").into_iter().next()?,
            })
        })
        .collect();
    found.sort_by(|left, right| left.id.cmp(&right.id));
    found
}

/// The registrations of `found` the load balancer SERVES: every one that is not being
/// deregistered.
fn serving(found: &[Registered]) -> Vec<&Registered> {
    found
        .iter()
        .filter(|entry| entry.state != DRAINING)
        .collect()
}

/// Wait until the target group holds exactly `wanted` serving registrations and every one of
/// them is healthy, and answer every registration last read — the draining ones included, so a
/// row can say what became of a target its scale-in removed.
async fn settled_health(
    router: &MultiServiceRouter,
    row: &Row,
    arn: &str,
    wanted: usize,
) -> Vec<Registered> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = target_health(router, row, arn).await;
        let served = serving(&found);
        let settled = served.len() == wanted && served.iter().all(|entry| entry.state == HEALTHY);
        if settled || Instant::now() >= deadline {
            return found;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Deploy one row's estate and drive the whole chain the rule states: the published identifier
/// IS the policy's ARN, a bare NAME with no group is refused and changes nothing, the ARN alone
/// performs the policy, and the capacity change it makes settles into the attached target
/// group.
async fn scales_through_the_published_policy_arn(row: &Row) {
    let case = row.case;
    let router = router();
    let body = template(row);
    cloudformation(
        &router,
        case,
        "CreateStack",
        &[("StackName", row.stack), ("TemplateBody", &body)],
    )
    .await;

    let described = settled_stack(&router, row).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: the stack must complete; the reasons it recorded were {:?}",
        stack_failures(&router, row).await,
    );

    let group = published(row, &described, "AutoScalingGroupName");
    assert_eq!(
        group, row.group,
        "case {case:?}: expected the stack to publish the group it declared, got {group:?}"
    );
    let target_group_arn = published(row, &described, "TargetGroupArn");
    let published_id = published(row, &described, "ScalingPolicyArn");
    let policy_name = published(row, &described, "ScalingPolicyName");

    // The identifier the stack publishes for the policy is the ARN of a scaling policy, in the
    // documented form whose resource half carries the group and the policy — which is what
    // makes the value name one policy with no group beside it.
    let account = ::engine::emulator_account_id();
    let expected_prefix = format!("arn:aws:autoscaling:{REGION}:{account}:scalingPolicy:");
    assert!(
        published_id.starts_with(&expected_prefix)
            && published_id.contains(&format!("autoScalingGroupName/{group}"))
            && published_id.ends_with(&format!("policyName/{policy_name}")),
        "case {case:?}: expected the published identifier to be the policy's ARN \
         ({expected_prefix}<id>:autoScalingGroupName/{group}:policyName/{policy_name}), got \
         {published_id:?}"
    );

    // ...and it is the very ARN the owning service minted, which is the other value a client
    // can hold for the policy.
    let (reported_name, reported_arn) = reported_policy(&router, row).await;
    assert_eq!(
        reported_name, policy_name,
        "case {case:?}: expected the owning service to report the policy under the name the \
         stack published, got {reported_name:?}"
    );
    assert_eq!(
        published_id, reported_arn,
        "case {case:?}: expected the identifier the stack published to be the ARN the owning \
         service minted, got {published_id:?} against {reported_arn:?}"
    );

    // The estate the stack declared converges before anything is asked of the policy.
    let before = settled_capacity(&router, row, row.desired).await;
    assert_eq!(
        before.len(),
        row.desired,
        "case {case:?}: expected the declared capacity of {} instance(s) {IN_SERVICE} before the \
         policy is performed, got {before:?}",
        row.desired
    );

    // The bare NAME names no one policy on its own — it is unique only inside a group — so the
    // same call spelled that way is refused for the member it leaves out, and the group is
    // untouched by it.
    let (status, refused) = asg_call(&router, "ExecutePolicy", &[("PolicyName", &policy_name)]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a policy named by its bare name with no group, got \
         {status} with body {refused}"
    );
    let code = elements(&refused, "Code").into_iter().next().unwrap_or_default();
    let message = elements(&refused, "Message")
        .into_iter()
        .next()
        .unwrap_or_default();
    assert_eq!(
        (code.as_str(), message.as_str()),
        (VALIDATION_ERROR, GROUP_REQUIRED_FOR_POLICY_NAME),
        "case {case:?}: expected the refusal {VALIDATION_ERROR}/\
         {GROUP_REQUIRED_FOR_POLICY_NAME:?}, got {code:?}/{message:?}"
    );
    let held = desired_capacity(&router, row).await;
    assert_eq!(
        held,
        row.desired.to_string(),
        "case {case:?}: a refused execution changes nothing, expected the group to still \
         maintain {}, got {held:?}",
        row.desired
    );

    // The ARN performs the policy with NO group beside it, whichever of the two places the
    // client read it from.
    let performed = match row.arn_from {
        ArnFrom::StackOutput => published_id.clone(),
        ArnFrom::DescribePolicies => reported_arn.clone(),
    };
    let (status, answered) = asg_call(
        &router,
        "ExecutePolicy",
        &[("PolicyName", &performed), ("HonorCooldown", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ExecutePolicy({performed}) with no \
         AutoScalingGroupName, got {status} with body {answered}"
    );
    assert!(
        !answered.contains("<Error"),
        "case {case:?}: expected the performed policy to answer no error, got {answered}"
    );

    // The policy's own adjustment is what the group now maintains.
    let held = desired_capacity(&router, row).await;
    assert_eq!(
        held,
        row.expected.to_string(),
        "case {case:?}: expected the group to maintain {} after the policy was performed, got \
         {held:?}",
        row.expected
    );

    // ...and the group converges on it: exactly that many instances in service.
    let after = settled_capacity(&router, row, row.expected).await;
    assert_eq!(
        after.len(),
        row.expected,
        "case {case:?}: expected {} instance(s) {IN_SERVICE} after the policy was performed, got \
         {after:?}",
        row.expected
    );

    // ...and the attached target group serves exactly those instances, each one healthy once the
    // load balancer's checks the declared inbound rule admits have decided.
    let registered = settled_health(&router, row, &target_group_arn, row.expected).await;
    let ids: Vec<String> = serving(&registered)
        .into_iter()
        .map(|entry| entry.id.clone())
        .collect();
    assert_eq!(
        ids, after,
        "case {case:?}: expected the attached target group to serve exactly the group's \
         in-service instances {after:?}, got {registered:?}"
    );
    for entry in serving(&registered) {
        assert_eq!(
            entry.state, HEALTHY,
            "case {case:?}: expected every served registration to settle {HEALTHY}, got \
             {registered:?}"
        );
    }

    // An instance the change took OUT of the group is deregistered: its registration is either
    // gone from the target group or draining out of it, never still being served.
    for gone in before.iter().filter(|id| !after.contains(id)) {
        let state = registered
            .iter()
            .find(|entry| &entry.id == gone)
            .map(|entry| entry.state.clone());
        assert!(
            state.is_none_or(|state| state == DRAINING),
            "case {case:?}: expected the registration of the scaled-in instance {gone:?} to be \
             gone or {DRAINING}, got {registered:?}"
        );
    }
}

/// The recorded shape: a `ChangeInCapacity` policy adding one instance, performed by the
/// identifier the stack published and nothing else.
#[tokio::test]
async fn a_published_policy_arn_alone_scales_a_load_balanced_group_out_by_one() {
    scales_through_the_published_policy_arn(&SCALE_OUT_BY_ONE).await;
}

/// The same rule for a policy that names the capacity outright, performed by the ARN the owning
/// service itself reports.
#[tokio::test]
async fn a_policy_arn_the_owning_service_reports_scales_a_group_to_an_exact_capacity() {
    scales_through_the_published_policy_arn(&SCALE_OUT_TO_EXACT_CAPACITY).await;
}

/// The other sign: a policy that takes the group down leaves the target group holding only the
/// instances that remain.
#[tokio::test]
async fn a_published_policy_arn_alone_scales_a_load_balanced_group_in_by_one() {
    scales_through_the_published_policy_arn(&SCALE_IN_BY_ONE).await;
}
