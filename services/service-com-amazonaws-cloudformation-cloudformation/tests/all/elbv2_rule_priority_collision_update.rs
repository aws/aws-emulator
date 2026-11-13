// SPDX-License-Identifier: CC0-1.0

//! A listener rule MOVED onto a priority another rule of the same listener already holds, as
//! a stack UPDATE runs into it.
//!
//! RULE under test: **a listener's rule priorities are one namespace per listener — "a
//! listener can't have multiple rules with the same priority" — so re-pointing an existing
//! rule at a priority another rule of that listener holds is refused by Elastic Load
//! Balancing whichever call asks for it, which fails that resource's UPDATE and rolls the
//! whole stack back to the configuration it had: the rule keeps its ARN, its priority and the
//! action body it was created with, and the load balancer keeps serving that body.**
//!
//! Why it matters: a client gates its deployment on the stack reaching a TERMINAL status. An
//! update that is accepted where AWS refuses it leaves a routing conflict deployed; one that
//! is neither refused nor rolled back leaves the stack parked `*_IN_PROGRESS`, so the gate
//! never returns at all.
//!
//! `elbv2_rule_priority_collision.rs` beside this suite pins the CREATE half of the same
//! namespace (two rules declared at one priority in one template); this one is the UPDATE
//! half, where the rule already exists and the template moves it.
//!
//! The rows differ along what the rule generalises over: the priority the move TARGETS — a
//! value in the middle of the range and each published bound of `RulePriority` (range 1..50000,
//! `smithy_query(query="operation",operation="CreateRule")`) — and whether the target is held
//! by another rule at all, which is the control row the same template deploys and updates
//! cleanly.

use std::time::{Duration, Instant};

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_resource_provisioning::{output, outputs_of, NETWORK};
use crate::refused_resource_sdk_exception::parts;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, elbv2, list_members, member, resource_event_reason, router,
    xml_text,
};

/// How long a case waits for a stack operation's terminal status, or for the load balancer to
/// serve the body its rule renders, before it gives up.
///
/// Generous on purpose: every case polls for the CONDITION it needs (a status that is no
/// longer `*_IN_PROGRESS`, a body the data plane answers), and this deadline is only the
/// backstop that turns something nothing will ever settle into a readable failure instead of a
/// hang.
const DEADLINE: Duration = Duration::from_secs(90);

/// How long a poll loop waits between two looks.
const SPACING: Duration = Duration::from_millis(50);

/// How many answers in a row the published address must give before a case takes the body as
/// the load balancer's own: one read could come out of any rule order, a run cannot.
const CONSECUTIVE: usize = 5;

/// The `HandlerErrorCode` the rule's provider classifies a priority already in use under: the
/// place the declaration asks for is one another rule already occupies. One of the model's own
/// `HandlerErrorCode` values.
const EXPECTED_CLASS: &str = "AlreadyExists";

/// The body the first rule is CREATED rendering, which is what the load balancer serves at its
/// path for as long as that rule is the one the listener evaluates there.
const CREATED_BODY: &str = "a1";

/// The body the UPDATE asks the first rule to render instead.
const UPDATED_BODY: &str = "a2";

/// The body the second rule renders, which is what the load balancer serves at the shared
/// path whenever that rule is the one its listener evaluates there first.
const SECOND_BODY: &str = "b1";

/// The path both rules of a row match, so the two really compete and the priority decides
/// which of them answers.
const PATH: &str = "/a";

/// One row: the priorities the two rules are created at, the priority the update MOVES the
/// first rule to, and whether that move collides with the second rule.
struct Row {
    case: &'static str,
    stack: &'static str,
    first_priority: i64,
    second_priority: i64,
    /// The priority the stack's update re-points the first rule at.
    target_priority: i64,
}

impl Row {
    /// Whether the move asks for a priority the OTHER rule of the listener holds.
    fn collides(&self) -> bool {
        self.target_priority == self.second_priority
    }

    /// The priority the first rule evaluates at once the stack has settled: the one it was
    /// created at where the move was refused and rolled back, the target where it was applied.
    fn settled_priority(&self) -> i64 {
        if self.collides() {
            self.first_priority
        } else {
            self.target_priority
        }
    }

    /// The body the first rule renders once the stack has settled.
    fn settled_body(&self) -> &'static str {
        if self.collides() {
            CREATED_BODY
        } else {
            UPDATED_BODY
        }
    }

    /// The body the published address serves at the shared path BEFORE the update: the rules
    /// are evaluated in ascending priority order, so the lower of the two answers.
    fn served_before(&self) -> &'static str {
        if self.first_priority < self.second_priority {
            CREATED_BODY
        } else {
            SECOND_BODY
        }
    }

    /// The body the published address serves once the stack has settled, by the same order
    /// over the priorities the stack ended up with.
    fn served_after(&self) -> &'static str {
        if self.settled_priority() < self.second_priority {
            self.settled_body()
        } else {
            SECOND_BODY
        }
    }
}

const ROWS: &[Row] = &[
    Row {
        case: "a rule moved onto the priority its sibling holds",
        stack: "rule-move-mid",
        first_priority: 10,
        second_priority: 20,
        target_priority: 20,
    },
    // The lowest priority a rule may evaluate at (`RulePriority` range 1..50000), already held.
    Row {
        case: "a rule moved onto the lowest priority a rule may take, which its sibling holds",
        stack: "rule-move-low",
        first_priority: 10,
        second_priority: 1,
        target_priority: 1,
    },
    // …and the highest.
    Row {
        case: "a rule moved onto the highest priority a rule may take, which its sibling holds",
        stack: "rule-move-high",
        first_priority: 10,
        second_priority: 50_000,
        target_priority: 50_000,
    },
    // The control: nothing holds the priority the move asks for, so the update is applied.
    Row {
        case: "a rule moved onto a priority no rule holds",
        stack: "rule-move-free",
        first_priority: 10,
        second_priority: 20,
        target_priority: 5,
    },
];

/// The template of one row: the network, an internet-facing Application Load Balancer, an
/// HTTP:80 listener answering a fixed 404 by default, and two rules of that listener matching
/// the same path at the priorities asked for.
fn template_of(stack: &str, first_priority: i64, first_body: &str, second_priority: i64) -> String {
    let mut template = String::from("Resources:\n");
    template.push_str(NETWORK);
    template.push_str(&format!(
        concat!(
            "  Igw:\n",
            "    Type: AWS::EC2::InternetGateway\n",
            "  Attachment:\n",
            "    Type: AWS::EC2::VPCGatewayAttachment\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "      InternetGatewayId: !Ref Igw\n",
            "  RouteTable:\n",
            "    Type: AWS::EC2::RouteTable\n",
            "    Properties:\n",
            "      VpcId: !Ref Vpc\n",
            "  DefaultRoute:\n",
            "    Type: AWS::EC2::Route\n",
            "    DependsOn: Attachment\n",
            "    Properties:\n",
            "      RouteTableId: !Ref RouteTable\n",
            "      DestinationCidrBlock: 0.0.0.0/0\n",
            "      GatewayId: !Ref Igw\n",
            "  Sg:\n",
            "    Type: AWS::EC2::SecurityGroup\n",
            "    Properties:\n",
            "      GroupDescription: the group the load balancer is placed in\n",
            "      VpcId: !Ref Vpc\n",
            "      SecurityGroupIngress:\n",
            "        - IpProtocol: tcp\n",
            "          FromPort: 80\n",
            "          ToPort: 80\n",
            "          CidrIp: 0.0.0.0/0\n",
            "  Alb:\n",
            "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
            "    Properties:\n",
            "      Name: {stack}-alb\n",
            "      Type: application\n",
            "      Scheme: internet-facing\n",
            "      Subnets:\n",
            "        - !Ref SubnetA\n",
            "        - !Ref SubnetB\n",
            "      SecurityGroups:\n",
            "        - !Ref Sg\n",
            "  Listener:\n",
            "    Type: AWS::ElasticLoadBalancingV2::Listener\n",
            "    Properties:\n",
            "      LoadBalancerArn: !Ref Alb\n",
            "      Port: 80\n",
            "      Protocol: HTTP\n",
            "      DefaultActions:\n",
            "        - Type: fixed-response\n",
            "          FixedResponseConfig:\n",
            "            StatusCode: '404'\n",
            "            ContentType: text/plain\n",
            "            MessageBody: none\n",
        ),
        stack = stack,
    ));
    template.push_str(&rule_of("RuleA", first_priority, first_body));
    template.push_str(&rule_of("RuleB", second_priority, SECOND_BODY));
    template.push_str(concat!(
        "Outputs:\n",
        "  AlbDns:\n",
        "    Value: !GetAtt Alb.DNSName\n",
        "  ListenerArn:\n",
        "    Value: !Ref Listener\n",
        "  RuleAArn:\n",
        "    Value: !Ref RuleA\n",
        "  RuleBArn:\n",
        "    Value: !Ref RuleB\n",
    ));
    template
}

/// One rule declaration: the same path as its sibling, a fixed response of its own.
fn rule_of(logical_id: &str, priority: i64, body: &str) -> String {
    format!(
        concat!(
            "  {logical_id}:\n",
            "    Type: AWS::ElasticLoadBalancingV2::ListenerRule\n",
            "    Properties:\n",
            "      ListenerArn: !Ref Listener\n",
            "      Priority: {priority}\n",
            "      Conditions:\n",
            "        - Field: path-pattern\n",
            "          Values:\n",
            "            - {path}\n",
            "      Actions:\n",
            "        - Type: fixed-response\n",
            "          FixedResponseConfig:\n",
            "            StatusCode: '200'\n",
            "            ContentType: text/plain\n",
            "            MessageBody: {body}\n",
        ),
        logical_id = logical_id,
        priority = priority,
        path = PATH,
        body = body,
    )
}

/// Read a stack until it reports a status that is no longer in progress, and answer that
/// read's body.
///
/// The wait is for the CONDITION: the loop looks again until the status has settled, and
/// [`DEADLINE`] is only the point at which a status that never settles is reported as such.
async fn settle(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let started = Instant::now();
    loop {
        let (status_code, body) = describe_stack(router, stack).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStacks({stack}) must answer 200, got {status_code} {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return body;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: the stack never left {reported:?} — a client that gates on a \
             terminal status waits forever"
        );
        tokio::time::sleep(SPACING).await;
    }
}

/// One data-plane request to the address the load balancer published, exactly as a client of
/// that name sends it: no credential, no body.
async fn get(router: &MultiServiceRouter, host: &str, target: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://{host}{target}"))
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("a well-formed data-plane request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    (status, body)
}

/// Poll the published address until it answers 200 with `expected` [`CONSECUTIVE`] times in a
/// row, and answer what was seen.
///
/// The wait is for the CONDITION — the body the listener's rules render — with [`DEADLINE`] as
/// the backstop, so a load balancer that never serves it is reported rather than waited on.
/// The run of answers is what makes the body the load balancer's CONFIGURATION rather than one
/// lucky read: a plane that answers out of a rule order it does not hold would serve the other
/// rule's body in between.
async fn served(router: &MultiServiceRouter, host: &str, expected: &str) -> Observed {
    let started = Instant::now();
    let mut consecutive = 0usize;
    loop {
        let last = get(router, host, PATH).await;
        if last.0 == 200 && last.1 == expected {
            consecutive += 1;
            if consecutive >= CONSECUTIVE {
                return Observed { consecutive, last };
            }
        } else {
            consecutive = 0;
        }
        if started.elapsed() >= DEADLINE {
            return Observed { consecutive, last };
        }
        tokio::time::sleep(SPACING).await;
    }
}

/// What a run of data-plane reads saw: how many answers in a row carried the expected body (up
/// to [`CONSECUTIVE`]), and the last answer as the client saw it.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    consecutive: usize,
    last: (u16, String),
}

impl Observed {
    /// The run a settled load balancer owes: every read answers the body its rules render.
    fn run_of(body: &str) -> Self {
        Observed {
            consecutive: CONSECUTIVE,
            last: (200, body.to_string()),
        }
    }
}

/// Every rule the listener holds, as the (`RuleArn`, `Priority`, `MessageBody`) triples
/// `DescribeRules` reports — each read INSIDE the `Rules` entry that carries it, so values
/// belonging to two different rules cannot be paired.
async fn rules_of(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
) -> Vec<(String, String, String)> {
    let (status_code, described) =
        elbv2(router, "DescribeRules", &[member("ListenerArn", listener_arn)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeRules on {listener_arn:?} must answer 200, got {status_code} \
         {described}"
    );
    list_members(&described, "Rules")
        .iter()
        .map(|rule| {
            (
                xml_text(rule, "RuleArn").unwrap_or_default(),
                xml_text(rule, "Priority").unwrap_or_default(),
                xml_text(rule, "MessageBody").unwrap_or_default(),
            )
        })
        .collect()
}

/// The rule of that listener carrying `rule_arn`, as the service reports it now.
fn reported<'a>(
    case: &str,
    rules: &'a [(String, String, String)],
    rule_arn: &str,
) -> &'a (String, String, String) {
    rules
        .iter()
        .find(|(arn, _, _)| arn == rule_arn)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the listener to still hold {rule_arn:?}, got {rules:?}")
        })
}

/// RULE: a stack update that moves a listener rule onto a priority another rule of the same
/// listener holds is refused, fails that resource and rolls the stack back — leaving the rule's
/// ARN, its priority and its action body as they were, and the data plane serving that body.
#[tokio::test]
async fn a_rule_moved_onto_a_taken_priority_rolls_the_stack_back() {
    for row in ROWS {
        let case = row.case;
        // One router — so one engine — per row: a row's estate is its own, and the names its
        // template declares are account-unique in the service that holds them.
        let router = router();
        let created = template_of(
            row.stack,
            row.first_priority,
            CREATED_BODY,
            row.second_priority,
        );
        let (status_code, body) = create_stack(&router, row.stack, &created, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
        );
        let described = settle(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
        );
        let values = outputs_of(case, &described);
        let dns_name = output(case, &values, "AlbDns");
        let listener_arn = output(case, &values, "ListenerArn");
        let first_arn = output(case, &values, "RuleAArn");
        let second_arn = output(case, &values, "RuleBArn");

        // The rule with the lower priority is the one the listener evaluates at the path both
        // of them match, so the body it renders is what the published address serves.
        let observed = served(&router, &dns_name, row.served_before()).await;
        assert_eq!(
            observed,
            Observed::run_of(row.served_before()),
            "case {case:?}: expected the deployed rules to serve {:?} at {PATH:?} on \
             {dns_name:?} on every read",
            row.served_before()
        );

        // The update: the first rule moves to the row's target priority and renders a new body.
        let updated = template_of(
            row.stack,
            row.target_priority,
            UPDATED_BODY,
            row.second_priority,
        );
        let (status_code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &updated),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: UpdateStack must answer 200, got {status_code} {body}"
        );
        let described = settle(&router, case, row.stack).await;
        let reported_status = xml_text(&described, "StackStatus").unwrap_or_default();
        let expected_status = if row.collides() {
            status::UPDATE_ROLLBACK_COMPLETE
        } else {
            status::UPDATE_COMPLETE
        };
        let (_, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            reported_status, expected_status,
            "case {case:?}: expected the stack at {expected_status}, got \
             {reported_status:?}; its events: {events}"
        );

        let reason = resource_event_reason(
            &router,
            row.stack,
            "RuleA",
            resource_status::UPDATE_FAILED,
        )
        .await;
        if row.collides() {
            // The refusal, as the resource that ran into it reports it: Elastic Load
            // Balancing's own message naming the priority the REQUEST asked for, and the class
            // a client branches on.
            let (quoted, class) = parts(case, &reason);
            let worded = format!("Priority '{}' is currently in use", row.target_priority);
            assert!(
                quoted.starts_with(&worded),
                "case {case:?}: expected the quoted exception to open with {worded:?}, got \
                 {quoted:?}"
            );
            assert_eq!(
                class, EXPECTED_CLASS,
                "case {case:?}: expected the {EXPECTED_CLASS} class for a priority a rule \
                 already holds, got {class:?} in {reason:?}"
            );
        } else {
            assert!(
                reason.is_empty(),
                "case {case:?}: nothing holds the priority the move asked for, so no resource \
                 may fail, got {reason:?}"
            );
        }

        // Whatever the update came to, the rules are the ones a client already holds: the
        // rollback puts a refused move back rather than replacing the rule.
        let rules = rules_of(&router, case, &listener_arn).await;
        let (_, first_priority, first_body) = reported(case, &rules, &first_arn);
        assert_eq!(
            (first_priority.as_str(), first_body.as_str()),
            (
                row.settled_priority().to_string().as_str(),
                row.settled_body()
            ),
            "case {case:?}: expected the moved rule at priority {} rendering {:?}, got \
             {rules:?}",
            row.settled_priority(),
            row.settled_body()
        );
        let (_, second_priority, _) = reported(case, &rules, &second_arn);
        assert_eq!(
            second_priority,
            &row.second_priority.to_string(),
            "case {case:?}: the rule the update did not name keeps its priority, got {rules:?}"
        );

        // And the data plane serves what the settled rules say: the body the rollback put back,
        // or the one an applied update rendered.
        let observed = served(&router, &dns_name, row.served_after()).await;
        assert_eq!(
            observed,
            Observed::run_of(row.served_after()),
            "case {case:?}: expected {:?} at {PATH:?} on {dns_name:?} on every read once the \
             stack settled",
            row.served_after()
        );
    }
}
