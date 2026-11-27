// SPDX-License-Identifier: CC0-1.0

//! Handing an IAM ROLE to the service that owns a resource, end to end over the awsQuery
//! wire: the `iam:PassRole` the CALLING PRINCIPAL needs for it.
//!
//! RULE: "to pass a role (and its permissions) to an AWS service, a user must have
//! permissions to pass the role to the service" — the permission is `iam:PassRole` on the
//! role's own ARN, and the service the role is handed to refuses the create otherwise. A
//! stack that names no `RoleARN` makes its calls with the CALLER's credentials, so a template
//! declaring a resource whose role the calling principal may not pass fails THAT resource
//! (`CREATE_FAILED`, carrying the authorization refusal) and the stack rolls back; a
//! principal the role may be passed by deploys the same template unchanged.
//!
//! The rows differ along what the rule generalises over: what the caller's policy says about
//! `iam:PassRole` (nothing at all, a grant on the role the template passes, a grant scoped to
//! ANOTHER role, a wildcard grant, a wildcard grant with an explicit deny over the role),
//! WHICH property hands the role over (an `AWS::Lambda::Function`'s `Role` and an
//! `AWS::StepFunctions::StateMachine`'s `RoleArn`), whether the role is named by ARN or
//! through `Fn::GetAtt` of a role the same stack creates, whether the template passes a role
//! at all — a stack that passes none is not gated by this at all, whatever the caller holds —
//! and whether the request hands the STACK a service role, which is what decides WHOSE
//! credentials the deployment's calls are made with.
//!
//! Nothing here sleeps for a span the emulator owes: the stack is polled for the CONDITION
//! (a status no operation is still in flight in) under the shared waiter's generous deadline.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_as, member, router_with_engine, xml_text};
use crate::stack_lifecycle::{
    acknowledging_iam, iam, resource_event_reason, settled_stack, user_with_inline_policy,
    ACCOUNT_ID,
};

/// The action a resource that hands a role to its owning service needs of the caller.
const PASS_ROLE: &str = "iam:PassRole";

/// The logical ids the templates declare, so one reader finds the resource that failed.
const FUNCTION: &str = "Worker";
const MACHINE: &str = "Workflow";
const TOPIC: &str = "Signals";
const EXEC_ROLE: &str = "ExecRole";

/// The role every row that names a role BY ARN passes, created in IAM before the stack is
/// deployed — the position a template that references a role somebody else manages is in.
const PASSED_ROLE: &str = "cfn-passrole-passed";

/// A second role no template passes, which the scoped-grant row's permission is about.
const OTHER_ROLE: &str = "cfn-passrole-other";

/// The role one row hands the STACK itself (`RoleARN`), whose credentials that stack's calls
/// are then made with instead of the caller's.
const SERVICE_ROLE: &str = "cfn-passrole-service";

/// The trust policy a role created for this suite carries. What the rows decide is the
/// CALLER's `iam:PassRole`, not what the role itself trusts, so every role trusts the
/// services these templates hand it to.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "sts:AssumeRole",
            "Principal": {"Service": [
                "cloudformation.amazonaws.com",
                "lambda.amazonaws.com",
                "states.amazonaws.com",
            ]},
        }],
    })
    .to_string()
}

/// The workflow the state-machine rows declare — a one-state definition, since what the rows
/// decide is the role rather than anything the workflow does.
const DEFINITION: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// The ARN of the role `name`, as IAM mints it for this account.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{name}")
}

/// WHICH resource of a template hands the role over, and how the declaration names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passes {
    /// An `AWS::Lambda::Function`'s `Role`, naming a role IAM already holds by ARN.
    FunctionRoleByArn,
    /// An `AWS::Lambda::Function`'s `Role`, naming a role the SAME stack creates through
    /// `Fn::GetAtt` — the shape a template that provisions its own execution role has.
    FunctionRoleFromTheStack,
    /// An `AWS::StepFunctions::StateMachine`'s `RoleArn`.
    MachineRoleArn,
    /// Nothing: the template passes no role to anybody.
    NoRole,
}

impl Passes {
    /// The logical id of the resource whose create the rule decides, or `None` for a template
    /// that hands no role over.
    fn gated_resource(self) -> Option<&'static str> {
        match self {
            Self::FunctionRoleByArn | Self::FunctionRoleFromTheStack => Some(FUNCTION),
            Self::MachineRoleArn => Some(MACHINE),
            Self::NoRole => None,
        }
    }
}

/// The template one row deploys.
fn template(row: &Row) -> String {
    let passed = role_arn(PASSED_ROLE);
    let resources = match row.passes {
        Passes::FunctionRoleByArn => json!({
            FUNCTION: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": row.stack,
                    "Code": {"ZipFile": "def handler(event, context):\n    return event\n"},
                    "Handler": "index.handler",
                    "Role": passed,
                    "Runtime": "python3.12",
                },
            },
        }),
        Passes::FunctionRoleFromTheStack => json!({
            EXEC_ROLE: {
                "Type": "AWS::IAM::Role",
                "Properties": {"AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "sts:AssumeRole",
                        "Principal": {"Service": "lambda.amazonaws.com"},
                    }],
                }},
            },
            FUNCTION: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": row.stack,
                    "Code": {"ZipFile": "def handler(event, context):\n    return event\n"},
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": [EXEC_ROLE, "Arn"]},
                    "Runtime": "python3.12",
                },
            },
        }),
        Passes::MachineRoleArn => json!({
            MACHINE: {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "StateMachineName": row.stack,
                    "DefinitionString": DEFINITION,
                    "RoleArn": passed,
                },
            },
        }),
        Passes::NoRole => json!({
            TOPIC: {
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": row.stack},
            },
        }),
    };
    json!({"Resources": resources}).to_string()
}

/// One row: what the caller's policy says about `iam:PassRole`, what the template hands over,
/// and whether the deployment goes through.
struct Row {
    case: &'static str,
    user: &'static str,
    stack: &'static str,
    passes: Passes,
    /// The `iam:PassRole` statements the caller's policy carries beside its CloudFormation
    /// grant, as a row states them.
    pass_role: fn() -> Value,
    /// Whether the request hands the STACK a service role (`RoleARN`), which is what decides
    /// WHOSE permissions the deployment's calls are made with.
    service_role: bool,
    /// Whether the stack reaches `CREATE_COMPLETE`, or fails the gated resource and rolls
    /// back.
    deploys: bool,
}

/// No statement about `iam:PassRole` at all.
fn no_statement() -> Value {
    json!([])
}

/// A grant of `iam:PassRole` on the role the templates pass.
fn granted_on_the_passed_role() -> Value {
    json!([{
        "Effect": "Allow",
        "Action": PASS_ROLE,
        "Resource": role_arn(PASSED_ROLE),
    }])
}

/// A grant of `iam:PassRole` on ANOTHER role, which says nothing about the one passed.
fn granted_on_another_role() -> Value {
    json!([{
        "Effect": "Allow",
        "Action": PASS_ROLE,
        "Resource": role_arn(OTHER_ROLE),
    }])
}

/// A wildcard grant of every IAM action on every resource.
fn granted_on_everything() -> Value {
    json!([{"Effect": "Allow", "Action": "iam:*", "Resource": "*"}])
}

/// A wildcard grant with an explicit deny over the role the templates pass: "an explicit deny
/// in any policy overrides any allows".
fn denied_over_the_passed_role() -> Value {
    json!([
        {"Effect": "Allow", "Action": "iam:*", "Resource": "*"},
        {"Effect": "Deny", "Action": PASS_ROLE, "Resource": role_arn(PASSED_ROLE)},
    ])
}

const ROWS: &[Row] = &[
    Row {
        case: "a caller holding no iam:PassRole at all, a function's Role",
        user: "cfn-passrole-nogrant",
        stack: "cfnpass-nogrant",
        passes: Passes::FunctionRoleByArn,
        pass_role: no_statement,
        service_role: false,
        deploys: false,
    },
    Row {
        case: "the same caller, a role the stack creates and passes through Fn::GetAtt",
        user: "cfn-passrole-getatt",
        stack: "cfnpass-getatt",
        passes: Passes::FunctionRoleFromTheStack,
        pass_role: no_statement,
        service_role: false,
        deploys: false,
    },
    Row {
        case: "the same caller, a state machine's RoleArn",
        user: "cfn-passrole-machine",
        stack: "cfnpass-machine",
        passes: Passes::MachineRoleArn,
        pass_role: no_statement,
        service_role: false,
        deploys: false,
    },
    Row {
        case: "a grant of iam:PassRole on the role the template passes",
        user: "cfn-passrole-granted",
        stack: "cfnpass-granted",
        passes: Passes::FunctionRoleByArn,
        pass_role: granted_on_the_passed_role,
        service_role: false,
        deploys: true,
    },
    Row {
        case: "the same grant, carried by the state machine's row",
        user: "cfn-passrole-machinegrant",
        stack: "cfnpass-machinegrant",
        passes: Passes::MachineRoleArn,
        pass_role: granted_on_the_passed_role,
        service_role: false,
        deploys: true,
    },
    Row {
        case: "a grant of iam:PassRole scoped to another role",
        user: "cfn-passrole-otherrole",
        stack: "cfnpass-otherrole",
        passes: Passes::FunctionRoleByArn,
        pass_role: granted_on_another_role,
        service_role: false,
        deploys: false,
    },
    Row {
        case: "a wildcard grant of every IAM action",
        user: "cfn-passrole-wildcard",
        stack: "cfnpass-wildcard",
        passes: Passes::FunctionRoleByArn,
        pass_role: granted_on_everything,
        service_role: false,
        deploys: true,
    },
    Row {
        case: "a wildcard grant with an explicit deny over the role",
        user: "cfn-passrole-denied",
        stack: "cfnpass-denied",
        passes: Passes::FunctionRoleByArn,
        pass_role: denied_over_the_passed_role,
        service_role: false,
        deploys: false,
    },
    Row {
        case: "a template that hands no role to anybody, under no iam:PassRole at all",
        user: "cfn-passrole-norole",
        stack: "cfnpass-norole",
        passes: Passes::NoRole,
        pass_role: no_statement,
        service_role: false,
        deploys: true,
    },
    Row {
        // The same caller and the same template as the first row, with a SERVICE ROLE handed
        // to the stack: the deployment's calls are then that role's rather than the caller's,
        // so the caller's own iam:PassRole does not decide them.
        case: "a caller holding no iam:PassRole, deploying under a stack service role",
        user: "cfn-passrole-servicerole",
        stack: "cfnpass-servicerole",
        passes: Passes::FunctionRoleByArn,
        pass_role: no_statement,
        service_role: true,
        deploys: true,
    },
];

/// The grants every row needs beside its own `iam:PassRole` statements: the CloudFormation
/// operation itself, and the creates the templates' own resources make in the services that OWN
/// them.
///
/// A stack that names no `RoleARN` is carried out with the CALLER's credentials, so those creates
/// are decided against this same principal — a principal that may not make one of them fails that
/// resource for THAT reason, which is not what these rows decide. No `iam:` action is granted
/// here: what each row states about `iam:PassRole` is the whole of what it holds over IAM.
fn grants_the_templates_need() -> Vec<Value> {
    vec![
        json!({"Effect": "Allow", "Action": "cloudformation:*", "Resource": "*"}),
        json!({
            "Effect": "Allow",
            "Action": ["lambda:*", "states:*", "sns:*"],
            "Resource": "*",
        }),
    ]
}

/// Create an IAM user carrying one inline policy — the grants every row needs to reach the
/// operation and deploy its template ([`grants_the_templates_need`]), plus the row's own
/// `iam:PassRole` statements — and answer the access key id a request of that user signs with.
async fn caller_for(router: &MultiServiceRouter, row: &Row) -> String {
    let statements: Vec<Value> = grants_the_templates_need()
        .into_iter()
        .chain((row.pass_role)().as_array().cloned().unwrap_or_default())
        .collect();
    let document = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
    user_with_inline_policy(
        router,
        row.case,
        row.user,
        "what-the-caller-may-pass",
        &document,
    )
    .await
}

/// Create the two roles the rows name by ARN, as a client's own `iam:CreateRole` creates them
/// before a template references one.
async fn create_the_roles(router: &MultiServiceRouter) {
    for name in [PASSED_ROLE, OTHER_ROLE, SERVICE_ROLE] {
        let (code, body) = iam(
            router,
            "CreateRole",
            &[
                ("RoleName", name),
                ("AssumeRolePolicyDocument", &trust_policy()),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "setup: CreateRole({name}) must answer, got {code} {body}"
        );
        let arn = xml_text(&body, "Arn").unwrap_or_default();
        assert_eq!(
            arn,
            role_arn(name),
            "setup: expected IAM to mint {name} under the account's role ARN, got {arn:?}"
        );
    }
}

/// RULE: a resource that hands an IAM role to its owning service is created only if the
/// calling principal may pass that role; one it may not pass fails that resource and rolls the
/// stack back.
#[tokio::test]
async fn a_resource_passing_a_role_is_created_only_by_a_principal_that_may_pass_it() {
    let (router, _engine) = router_with_engine();
    create_the_roles(&router).await;
    for row in ROWS {
        let case = row.case;
        let access_key_id = caller_for(&router, row).await;
        let mut members = vec![
            member("StackName", row.stack),
            member("TemplateBody", &template(row)),
        ];
        if row.service_role {
            members.push(member("RoleARN", &role_arn(SERVICE_ROLE)));
        }
        let (code, body) = call_as(
            &router,
            &access_key_id,
            "CreateStack",
            &acknowledging_iam(&members),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the create must be accepted — the decision is the RESOURCE's, not \
             the request's — got {code} {body}"
        );
        let settled = settled_stack(&router, case, row.stack).await;
        let reported = xml_text(&settled, "StackStatus").unwrap_or_default();
        if row.deploys {
            assert_eq!(
                reported, "CREATE_COMPLETE",
                "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {reported:?} \
                 with {settled}"
            );
            continue;
        }
        assert_eq!(
            reported, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the stack to roll back after the denied {PASS_ROLE}, got \
             {reported:?} with {settled}"
        );
        // ...and the resource that failed is the one that handed the role over, carrying the
        // refusal the service it was handed to answers.
        let gated = row
            .passes
            .gated_resource()
            .expect("a row that does not deploy hands a role over");
        let reason = resource_event_reason(&router, row.stack, gated, "CREATE_FAILED").await;
        assert!(
            reason.contains(PASS_ROLE),
            "case {case:?}: expected the failed {gated} to report the {PASS_ROLE} refusal, got \
             {reason:?}"
        );
        assert!(
            reason.contains(&role_arn(PASSED_ROLE)) || reason.contains(":role/"),
            "case {case:?}: expected the refusal to name the role it is about, got {reason:?}"
        );
    }
}
