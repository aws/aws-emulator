// SPDX-License-Identifier: CC0-1.0

//! The scaling policy a CloudFormation stack declares on an Auto Scaling group, and what a
//! client does with it afterwards: read it back, EXECUTE it, and watch the group move.
//!
//! The rule under test: **a stack that reaches a successful terminal state has really created
//! the scaling policy it declares in the service that OWNS it, so the identifier it publishes
//! for that policy resolves in Amazon EC2 Auto Scaling — `DescribePolicies` reports it with
//! the configuration the declaration stated — and `ExecutePolicy` on that identifier applies
//! the policy's own adjustment to the group's `DesiredCapacity`, held inside the group's
//! `MinSize`..`MaxSize`. Deleting the stack removes the policy from that service too.**
//!
//! `AWS::AutoScaling::ScalingPolicy` is the type that makes the rule visible end to end: the
//! stack publishes a handle for a resource THREE other crates have to agree about (the policy
//! and the group live in Amazon EC2 Auto Scaling, the group launches Amazon EC2 instances from
//! a launch template Amazon EC2 holds), and the only way to see the handle is real is to
//! perform the policy and read the group back. A stack that reported the policy created while
//! the owning service held nothing would answer that execution with a refusal.
//!
//! `cloudformation_asg_launch_fault_diagnosis.rs` covers the GROUP's own read from the name a
//! stack published; what this suite is about is the policy declared on such a group.
//!
//! The rows differ along what the rule generalises over: the policy's type (`SimpleScaling`
//! and `StepScaling`), the adjustment type the declaration states (`ChangeInCapacity` and
//! `ExactCapacity`), the sign of the adjustment, how the execution NAMES the policy (the name
//! the stack publishes for it with the group beside it, the ARN the owning service minted, or
//! the identifier the stack publishes — which IS that ARN, so it names the policy on its own),
//! and the capacity the group is left maintaining.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the identities the owning services mint (the policy's
//! name and ARN, the launch template's id), which are checked by being the SAME value
//! everywhere the stack and those services report them.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, elements, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// How long a poll waits between reads, and how many times it reads. The bound is generous:
/// what the rows are about is that the stack settles and the group converges at all, not how
/// quickly the host gets there.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_ATTEMPTS: usize = 300;

/// The network and the launch source the group of every row launches from.
const VPC_CIDR: &str = "10.199.0.0/16";
const SUBNET_CIDR: &str = "10.199.1.0/24";
const ZONE: &str = "us-east-1a";
const IMAGE_ID: &str = "ami-00000000000000002";
const INSTANCE_TYPE: &str = "t3.micro";

/// How the execution names the policy it performs: "The name or ARN of the policy"
/// (`ExecutePolicyType$PolicyName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The policy's NAME, as the stack publishes it through `Fn::GetAtt`, with the group
    /// beside it — which a name needs, being unique only within its group.
    PublishedNameWithGroup,
    /// The ARN Amazon EC2 Auto Scaling minted, with the group beside it.
    ArnWithGroup,
    /// The identifier the stack publishes for the resource, ALONE: "Ref returns the Amazon
    /// Resource Name (ARN) of the scaling policy", so it names the policy wherever it lives.
    PublishedIdAlone,
}

/// One row: the estate one stack declares, how its policy is performed, and the capacity the
/// group is left maintaining.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// `(MinSize, MaxSize, DesiredCapacity)` as the group's declaration states them.
    sizes: (&'static str, &'static str, &'static str),
    /// The properties the policy's declaration carries beyond the group it is on.
    policy: fn() -> Value,
    /// The members the execution carries beyond the policy's name.
    graded: &'static [(&'static str, &'static str)],
    named: Named,
    /// The `DesiredCapacity` the group must report once the policy has been performed.
    expected_capacity: &'static str,
}

/// A `SimpleScaling` policy adding one instance, with its own cooldown stated as none.
fn simple_scale_out() -> Value {
    json!({
        "PolicyType": "SimpleScaling",
        "AdjustmentType": "ChangeInCapacity",
        "ScalingAdjustment": "1",
        "Cooldown": "0",
    })
}

/// A `SimpleScaling` policy naming the capacity outright.
fn simple_exact_capacity() -> Value {
    json!({
        "PolicyType": "SimpleScaling",
        "AdjustmentType": "ExactCapacity",
        "ScalingAdjustment": "3",
        "Cooldown": "0",
    })
}

/// A `StepScaling` policy whose one step covers the metric values at or above the breach
/// threshold and removes an instance.
fn step_scale_in() -> Value {
    json!({
        "PolicyType": "StepScaling",
        "AdjustmentType": "ChangeInCapacity",
        "MetricAggregationType": "Average",
        "StepAdjustments": [{
            "MetricIntervalLowerBound": "0",
            "ScalingAdjustment": "-1",
        }],
    })
}

const ROWS: &[Row] = &[
    Row {
        case: "a SimpleScaling policy adding one instance, performed by the name the stack \
               published",
        stack: "asg-policy-scale-out",
        sizes: ("1", "3", "1"),
        policy: simple_scale_out,
        graded: &[],
        named: Named::PublishedNameWithGroup,
        expected_capacity: "2",
    },
    Row {
        case: "a SimpleScaling policy naming the capacity outright, performed by the ARN the \
               owning service minted",
        stack: "asg-policy-exact",
        sizes: ("0", "4", "1"),
        policy: simple_exact_capacity,
        graded: &[],
        named: Named::ArnWithGroup,
        expected_capacity: "3",
    },
    Row {
        case: "a StepScaling policy removing one instance, performed by the identifier the \
               stack published alone and graded by a metric value past its breach threshold",
        stack: "asg-policy-step-in",
        sizes: ("0", "4", "2"),
        policy: step_scale_in,
        graded: &[("MetricValue", "70"), ("BreachThreshold", "50")],
        named: Named::PublishedIdAlone,
        expected_capacity: "1",
    },
];

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One form-encoded call on a query wire, as its HTTP status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon EC2 Auto Scaling operation over its awsQuery wire.
async fn autoscaling(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "autoscaling", "2011-01-01", action, members).await
}

/// The estate one row deploys: the network the group launches into, the launch template it
/// launches from, the group, and the scaling policy declared on that group.
fn template(row: &Row) -> Value {
    let mut policy_properties = (row.policy)();
    policy_properties["AutoScalingGroupName"] = json!({ "Ref": "Fleet" });
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": { "CidrBlock": VPC_CIDR },
            },
            "Subnet": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": SUBNET_CIDR,
                    "AvailabilityZone": ZONE,
                },
            },
            "LaunchTemplate": {
                "Type": "AWS::EC2::LaunchTemplate",
                "Properties": {
                    "LaunchTemplateName": row.stack,
                    "LaunchTemplateData": {
                        "ImageId": IMAGE_ID,
                        "InstanceType": INSTANCE_TYPE,
                    },
                },
            },
            "Fleet": {
                "Type": "AWS::AutoScaling::AutoScalingGroup",
                "Properties": {
                    "MinSize": row.sizes.0,
                    "MaxSize": row.sizes.1,
                    "DesiredCapacity": row.sizes.2,
                    "VPCZoneIdentifier": [{ "Ref": "Subnet" }],
                    "LaunchTemplate": {
                        "LaunchTemplateId": { "Ref": "LaunchTemplate" },
                        "Version": { "Fn::GetAtt": ["LaunchTemplate", "LatestVersionNumber"] },
                    },
                },
            },
            "ScalePolicy": {
                "Type": "AWS::AutoScaling::ScalingPolicy",
                "Properties": policy_properties,
            },
        },
        "Outputs": {
            "AutoScalingGroupName": { "Value": { "Ref": "Fleet" } },
            "ScalingPolicyId": { "Value": { "Ref": "ScalePolicy" } },
            "ScalingPolicyName": { "Value": { "Fn::GetAtt": ["ScalePolicy", "PolicyName"] } },
        },
    })
}

/// Create a stack, poll it to a terminal status the way a client does, and answer its
/// `DescribeStacks` body.
async fn deploy(router: &MultiServiceRouter, case: &str, stack: &str, body: &Value) -> String {
    let body = body.to_string();
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &body)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );
    poll_to_terminal(router, stack).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, stack: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        if status != 200 {
            // A deleted stack is no longer described, which is a terminal answer too.
            return body;
        }
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    described
}

/// The reason the stack's own events carry for every resource that failed, so a stack that
/// did not settle where a row expects says WHICH resource its owning service refused.
async fn failures(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    super::query_wire::members_of(&events)
        .into_iter()
        .filter(|member| member.contains("_FAILED"))
        .map(|member| {
            format!(
                "{:?} {:?} {:?}",
                opt_element(member, "LogicalResourceId").unwrap_or_default(),
                opt_element(member, "ResourceStatus").unwrap_or_default(),
                opt_element(member, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// One output of the settled stack, asserted PRESENT and non-empty.
fn published(case: &str, described: &str, key: &str) -> String {
    let outputs = outputs_of(described);
    let value = outputs
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no output named {key:?}, got {outputs:?}"));
    assert!(
        !value.is_empty(),
        "case {case:?}: expected the output {key:?} to carry a value, got {outputs:?}"
    );
    value
}

/// The policies Amazon EC2 Auto Scaling reports for `group` right now, as that read's body.
async fn described_policies(router: &MultiServiceRouter, case: &str, group: &str) -> String {
    let (status, body) =
        autoscaling(router, "DescribePolicies", &[("AutoScalingGroupName", group)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePolicies({group}), got {status} with body \
         {body}"
    );
    body
}

/// The `DesiredCapacity` the group reports right now.
async fn desired_capacity(router: &MultiServiceRouter, case: &str, group: &str) -> String {
    let (status, body) = autoscaling(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAutoScalingGroups({group}), got {status} \
         with body {body}"
    );
    element(&body, "DesiredCapacity")
}

#[tokio::test]
async fn a_deployed_scaling_policy_is_readable_executable_and_removable() {
    for row in ROWS {
        let case = row.case;
        let router = build_router();
        let described = deploy(&router, case, row.stack, &template(row)).await;
        let settled = opt_element(&described, "StackStatus").unwrap_or_default();
        if settled != "CREATE_COMPLETE" {
            panic!(
                "case {case:?}: expected the stack to settle at CREATE_COMPLETE, got \
                 {settled:?}; its failed resources: {}",
                failures(&router, row.stack).await
            );
        }

        let group = published(case, &described, "AutoScalingGroupName");
        let policy_id = published(case, &described, "ScalingPolicyId");
        let policy_name = published(case, &described, "ScalingPolicyName");

        // The policy the stack published resolves in the service that owns it, carrying the
        // configuration the declaration stated.
        let policies = described_policies(&router, case, &group).await;
        assert_eq!(
            elements(&policies, "PolicyName"),
            vec![policy_name.clone()],
            "case {case:?}: expected Auto Scaling to report exactly the policy {policy_name:?} \
             for {group:?}, got {policies}"
        );
        let declared = (row.policy)();
        for member in ["PolicyType", "AdjustmentType"] {
            let stated = declared
                .get(member)
                .and_then(Value::as_str)
                .expect("every row states this member");
            assert_eq!(
                element(&policies, member),
                stated,
                "case {case:?}: expected the policy's {member} to be the declared {stated:?}, got \
                 {policies}"
            );
        }
        assert_eq!(
            elements(&policies, "AutoScalingGroupName"),
            vec![group.clone()],
            "case {case:?}: expected the policy to be reported on {group:?}, got {policies}"
        );
        let policy_arn = element(&policies, "PolicyARN");
        assert!(
            policy_arn.contains(":autoscaling:") && policy_arn.contains(&policy_name),
            "case {case:?}: expected the reported ARN to be the policy's own, got \
             {policy_arn:?} in {policies}"
        );
        // "Ref returns the Amazon Resource Name (ARN) of the scaling policy": the identifier
        // the stack publishes for the resource is that ARN, not the policy's name.
        assert_eq!(
            policy_id, policy_arn,
            "case {case:?}: expected the identifier the stack publishes for the policy to be the \
             ARN {policy_arn:?} the owning service minted, got {policy_id:?}"
        );

        // Performing it applies the policy's own adjustment to the group's capacity.
        let mut members: Vec<(&str, &str)> = match row.named {
            Named::PublishedNameWithGroup => {
                vec![
                    ("AutoScalingGroupName", &group),
                    ("PolicyName", &policy_name),
                ]
            }
            Named::ArnWithGroup => {
                vec![("AutoScalingGroupName", &group), ("PolicyName", &policy_arn)]
            }
            Named::PublishedIdAlone => vec![("PolicyName", &policy_id)],
        };
        members.extend(row.graded.iter().copied());
        let (status, body) = autoscaling(&router, "ExecutePolicy", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ExecutePolicy({policy_name}), got {status} \
             with body {body}"
        );
        let held = desired_capacity(&router, case, &group).await;
        assert_eq!(
            held, row.expected_capacity,
            "case {case:?}: expected the group to maintain {:?} after the policy was performed, \
             got {held:?}",
            row.expected_capacity
        );

        // Deleting the stack removes the policy from the owning service too.
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        poll_to_terminal(&router, row.stack).await;
        // Read until the owning service reports no policy of the group — or no group at all,
        // which went with the stack and can report no policy of its own.
        let mut remaining = String::new();
        for _ in 0..POLL_ATTEMPTS {
            let (status, body) =
                autoscaling(&router, "DescribePolicies", &[("AutoScalingGroupName", &group)]).await;
            if status != 200 {
                remaining = String::new();
                break;
            }
            remaining = body;
            if elements(&remaining, "PolicyName").is_empty() {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        assert!(
            elements(&remaining, "PolicyName").is_empty(),
            "case {case:?}: expected the deleted stack to leave no scaling policy behind, got \
             {remaining}"
        );
    }
}
