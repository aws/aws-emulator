// SPDX-License-Identifier: CC0-1.0

//! A listener rule's PRIORITY namespace, as a stack that declares rules runs into it.
//!
//! RULE under test: **a listener's rule priorities are one namespace per listener — "a
//! listener can't have multiple rules with the same priority" — so a declared
//! `AWS::ElasticLoadBalancingV2::ListenerRule` asking for a priority the listener already
//! evaluates a rule at is REFUSED by Elastic Load Balancing, which fails that resource and
//! rolls the whole stack back; the same priority on another listener is free, and the failed
//! resource reports that service's own refusal as the resource-handler message its SDK raised,
//! classified under the `HandlerErrorCode` the provider puts an already-taken place in.**
//!
//! Why it matters: a template with a genuine routing conflict must not deploy. A stack
//! reported `CREATE_COMPLETE` over rules the service never accepted leaves a client's gate
//! green on routing that is not what the template describes, and a failure reason that names
//! neither the refusing service nor its HTTP status leaves automation nothing to branch on.
//!
//! The rows differ along what the rule generalises over: the PRIORITY the two rules collide at
//! — a value in the middle of the range and each published bound of `RulePriority` (range
//! 1..50000, `smithy_query(query="operation",operation="CreateRule")`) — whether the two rules
//! are on ONE listener or on two listeners of the same load balancer, and whether their
//! priorities collide at all.

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_resource_provisioning::{output, outputs_of, poll_to_terminal, NETWORK};
use crate::refused_resource_sdk_exception::parts;
use crate::stack_lifecycle::{
    call, create_stack, elbv2, member, resource_event_reason, router, wait_for_the_transition,
    xml_blocks, xml_text, xml_texts,
};

/// The name Elastic Load Balancing v2's own SDK goes by in the diagnostic it appends to a
/// refused call's message — that model's `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")`).
const SDK_SERVICE: &str = "Service: ElasticLoadBalancingV2";

/// The HTTP status Elastic Load Balancing refuses a priority already in use with:
/// `CreateRule`'s `PriorityInUseException` carries `awsQueryError {code: 'PriorityInUse',
/// httpResponseCode: 400}`.
const SDK_STATUS: &str = "Status Code: 400";

/// The `HandlerErrorCode` the rule's provider classifies a priority already in use under: the
/// place the declaration asks for is one a rule already occupies.
const EXPECTED_CLASS: &str = "AlreadyExists";

/// How many reads the load balancer is looked for through once the stack has rolled back: the
/// rollback deletes the resources the failed create had made, and the last of those deletions
/// is not due on the first read.
const REMOVAL_ATTEMPTS: usize = 12;

/// What the deployment of one row must come to.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Elastic Load Balancing refused the second rule at this priority, so the stack rolled
    /// back.
    RolledBackAt(i64),
    /// Nothing collided: the stack deployed and both rules are on their listeners.
    Deployed,
}

/// One row: the two priorities the template declares, whether the second rule is on a listener
/// of its own, and what the stack must come to.
struct Row {
    case: &'static str,
    /// The stack's name, which is also the stem of the load balancer's and the target group's.
    stack: &'static str,
    first_priority: i64,
    second_priority: i64,
    /// Whether the second rule is declared on a SECOND listener of the same load balancer.
    second_listener: bool,
    outcome: Outcome,
}

const ROWS: &[Row] = &[
    Row {
        case: "two rules of one listener at one priority",
        stack: "rules-collide-mid",
        first_priority: 10,
        second_priority: 10,
        second_listener: false,
        outcome: Outcome::RolledBackAt(10),
    },
    // The lowest priority a rule may evaluate at (`RulePriority` range 1..50000), taken twice.
    Row {
        case: "two rules of one listener at the lowest priority a rule may take",
        stack: "rules-collide-low",
        first_priority: 1,
        second_priority: 1,
        second_listener: false,
        outcome: Outcome::RolledBackAt(1),
    },
    // …and the highest.
    Row {
        case: "two rules of one listener at the highest priority a rule may take",
        stack: "rules-collide-high",
        first_priority: 50_000,
        second_priority: 50_000,
        second_listener: false,
        outcome: Outcome::RolledBackAt(50_000),
    },
    Row {
        case: "two rules of one listener at different priorities",
        stack: "rules-apart",
        first_priority: 10,
        second_priority: 50,
        second_listener: false,
        outcome: Outcome::Deployed,
    },
    // The priority lives in its LISTENER's own namespace, so the same number on another
    // listener of the same load balancer is free.
    Row {
        case: "one priority on two listeners of one load balancer",
        stack: "rules-two-listeners",
        first_priority: 10,
        second_priority: 10,
        second_listener: true,
        outcome: Outcome::Deployed,
    },
];

/// The template one row deploys: the network, an application load balancer with a target group
/// and a listener (a second listener where the row asks for one), and the two rules.
fn template_of(row: &Row) -> String {
    let mut template = String::from("Resources:\n");
    template.push_str(NETWORK);
    template.push_str(&format!(
        concat!(
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: {stack}-alb\n",
            "      Type: application\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "  Targets:\n",
            "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
            "    Properties:\n",
            "      Name: {stack}-tg\n",
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
        ),
        stack = row.stack,
    ));
    if row.second_listener {
        template.push_str(concat!(
            "  SecondListener:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 8080\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
        ));
    }
    template.push_str(&rule_of("FirstRule", "Listener", row.first_priority, "/a"));
    template.push_str(&rule_of(
        "SecondRule",
        if row.second_listener {
            "SecondListener"
        } else {
            "Listener"
        },
        row.second_priority,
        "/b",
    ));
    template.push_str(concat!(
        "Outputs:\n",
        "  ListenerArn:\n",
        "    Value: !Ref Listener\n",
    ));
    if row.second_listener {
        template.push_str(concat!(
            "  SecondListenerArn:\n",
            "    Value: !Ref SecondListener\n",
        ));
    }
    template
}

/// One rule declaration, on the listener its logical id names.
fn rule_of(logical_id: &str, listener: &str, priority: i64, path: &str) -> String {
    format!(
        concat!(
            "  {logical_id}:\n",
            "    Type: AWS::ElasticLoadBalancingV2::ListenerRule\n",
            "    Properties:\n",
            "      ListenerArn: !Ref {listener}\n",
            "      Priority: {priority}\n",
            "      Conditions:\n",
            "        - Field: path-pattern\n",
            "          Values:\n",
            "            - {path}\n",
            "      Actions:\n",
            "        - Type: forward\n",
            "          TargetGroupArn: !Ref Targets\n",
        ),
        logical_id = logical_id,
        listener = listener,
        priority = priority,
        path = path,
    )
}

/// Hold the quoted exception of a refused rule to the message Elastic Load Balancing worded
/// and the diagnostic its SDK appends: the service, the HTTP status and the id of the call,
/// each of the FORM that service answers rather than one recorded value, and nothing past the
/// envelope.
fn assert_elbv2_diagnostic(case: &str, priority: i64, quoted: &str) {
    // The fixed text of `PriorityInUseException`'s message, with the colliding priority of
    // the REQUEST in it.
    let worded = format!("Priority '{priority}' is currently in use");
    let diagnostic = quoted.strip_prefix(&worded).map(str::trim_start).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the quoted text to open with the service's own message \
             {worded:?}, got {quoted:?}"
        )
    });
    let envelope = diagnostic
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the SDK diagnostic and nothing after it, got {quoted:?}")
        });
    let fields: Vec<&str> = envelope.split(", ").collect();
    assert_eq!(
        fields.len(),
        3,
        "case {case:?}: expected the service, the status and the call's id, got {envelope:?}"
    );
    assert_eq!(
        fields[0], SDK_SERVICE,
        "case {case:?}: expected the SDK's own name for the refusing service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1], SDK_STATUS,
        "case {case:?}: expected the status Elastic Load Balancing answered, got {:?}",
        fields[1]
    );
    let request_id = fields[2]
        .strip_prefix("Request ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Request ID, got {:?}", fields[2]));
    assert!(
        !request_id.is_empty()
            && request_id
                .chars()
                .all(|held| held.is_ascii_hexdigit() || held == '-'),
        "case {case:?}: expected the opaque id of the call, got {request_id:?}"
    );
}

/// The priorities Elastic Load Balancing reports for the rules of one listener, which is the
/// one rule store `DescribeRules` walks.
async fn reported_priorities(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
) -> Vec<String> {
    let (status_code, described) = elbv2(
        router,
        "DescribeRules",
        &[member("ListenerArn", listener_arn)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeRules on {listener_arn:?} to answer 200, got \
         {status_code} {described}"
    );
    xml_texts(&described, "Priority")
}

/// Whether Elastic Load Balancing still holds a load balancer under `name`.
async fn load_balancer_is_held(router: &MultiServiceRouter, name: &str) -> bool {
    let (status_code, _) = elbv2(
        router,
        "DescribeLoadBalancers",
        &[member("Names.member.1", name)],
    )
    .await;
    status_code == 200
}

#[tokio::test]
async fn a_priority_already_in_use_on_the_listener_rolls_the_stack_back() {
    let router = router();
    for row in ROWS {
        let case = row.case;
        let (status_code, body) = create_stack(&router, row.stack, &template_of(row), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
        );
        let described = poll_to_terminal(&router, case, row.stack).await;
        let reported = xml_text(&described, "StackStatus").unwrap_or_default();
        let (_, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;

        match row.outcome {
            Outcome::RolledBackAt(priority) => {
                assert_eq!(
                    reported, status::ROLLBACK_COMPLETE,
                    "case {case:?}: expected the stack to roll back on the refused rule, got \
                     {reported:?}; its events: {events}"
                );
                // The refusal, as the resource that ran into it reports it: the service's own
                // message, its SDK's diagnostic of the call, and the class a client branches
                // on.
                let reason = resource_event_reason(
                    &router,
                    row.stack,
                    "SecondRule",
                    resource_status::CREATE_FAILED,
                )
                .await;
                let (quoted, class) = parts(case, &reason);
                assert_elbv2_diagnostic(case, priority, &quoted);
                assert_eq!(
                    class, EXPECTED_CLASS,
                    "case {case:?}: expected the {EXPECTED_CLASS} class for a priority a rule \
                     already holds, got {class:?} in {reason:?}"
                );
                // The rule that was refused exists nowhere, and the rollback took the
                // resources the create had made with it: the load balancer the stack would
                // have published is in no Elastic Load Balancing read.
                let name = format!("{}-alb", row.stack);
                let mut held = true;
                for _ in 0..REMOVAL_ATTEMPTS {
                    held = load_balancer_is_held(&router, &name).await;
                    if !held {
                        break;
                    }
                    wait_for_the_transition().await;
                }
                assert!(
                    !held,
                    "case {case:?}: expected the rolled-back stack to leave no load balancer \
                     {name:?} behind"
                );
            }
            Outcome::Deployed => {
                assert_eq!(
                    reported, status::CREATE_COMPLETE,
                    "case {case:?}: nothing collided, so the stack must complete, got \
                     {reported:?}; its events: {events}"
                );
                let values = outputs_of(case, &described);
                let listener_arn = output(case, &values, "ListenerArn");
                let first = reported_priorities(&router, case, &listener_arn).await;
                assert!(
                    first.contains(&row.first_priority.to_string()),
                    "case {case:?}: expected the listener to evaluate a rule at \
                     {}, got {first:?}",
                    row.first_priority
                );
                let second_listener_arn = if row.second_listener {
                    output(case, &values, "SecondListenerArn")
                } else {
                    listener_arn.clone()
                };
                let second = reported_priorities(&router, case, &second_listener_arn).await;
                assert!(
                    second.contains(&row.second_priority.to_string()),
                    "case {case:?}: expected the rule's listener to evaluate it at {}, got \
                     {second:?}",
                    row.second_priority
                );
                // No resource of a completed stack failed: the whole point of the two
                // namespaces being separate.
                let failed = resource_event_reason(
                    &router,
                    row.stack,
                    "SecondRule",
                    resource_status::CREATE_FAILED,
                )
                .await;
                assert!(
                    failed.is_empty(),
                    "case {case:?}: expected no refusal of the second rule, got {failed:?}"
                );
            }
        }
    }
}

/// The complement, from inside the stack's event stream: the rule the deployment got to FIRST
/// is the one that was created, and only the one that ran into the priority it had taken
/// failed — so the reason names the resource a client has to change.
#[tokio::test]
async fn only_the_rule_that_ran_into_the_taken_priority_fails() {
    let router = router();
    let case = "the second of two rules at one priority";
    let row = Row {
        case,
        stack: "rules-collide-one-fails",
        first_priority: 7,
        second_priority: 7,
        second_listener: false,
        outcome: Outcome::RolledBackAt(7),
    };
    let (status_code, body) = create_stack(&router, row.stack, &template_of(&row), &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
    );
    let described = poll_to_terminal(&router, case, row.stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "case {case:?}: expected the stack to roll back, got {described}"
    );
    let (status_code, events) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", row.stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeStackEvents must answer 200, got {status_code} {events}"
    );
    let mut failed: Vec<String> = xml_blocks(&events, "member")
        .iter()
        .filter(|event| {
            xml_text(event, "ResourceStatus").as_deref() == Some(resource_status::CREATE_FAILED)
        })
        .filter_map(|event| xml_text(event, "LogicalResourceId"))
        .collect();
    failed.sort();
    failed.dedup();
    assert_eq!(
        failed,
        vec!["SecondRule".to_string()],
        "case {case:?}: expected the rule that asked for the taken priority to be the one \
         failed resource, got {failed:?}; its events: {events}"
    );
}
