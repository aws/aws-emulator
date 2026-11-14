// SPDX-License-Identifier: CC0-1.0

//! The `ERROR_CODE` EventBridge stamps on a dead-lettered event names the STEP
//! of the delivery that failed, not merely that the delivery failed.
//!
//! The RULE asserted here: EventBridge invokes a role-governed target by ACTING
//! AS the target's `RoleArn`, which is two steps — assume the role, then make the
//! target's API call as it — and the dead-letter message's `ERROR_CODE` names the
//! step that stopped the delivery: a role the service principal cannot assume at
//! all is `FAILED_TO_ASSUME_ROLE`, while a role that CAN be assumed but whose
//! identity policies withhold the target's API call is `NO_PERMISSIONS`. The
//! assume-role step is decided first, so a role failing both is reported as the
//! earlier one. The `ERROR_MESSAGE` goes with it: the assume-role step's names the
//! role and the trust it lacks, while the permission step's is the authorization
//! sentence about the SESSION principal the call would have been made as, closing
//! on whether a policy denied the call or none allowed it.
//!
//! The table spans the class rather than one case: its rows differ along WHICH of
//! the two steps the IAM records stop, and along the FORM in which they stop it —
//! a trust policy naming another principal, one explicitly denying the principal,
//! one naming an account principal instead of the service, one naming another
//! action for it, against an identity policy that allows another action, allows
//! nothing at all, or explicitly denies the call — plus a row where neither step
//! would pass, which pins the precedence.
//!
//! The harness (the router with EventBridge, SQS, Step Functions, IAM and
//! CloudWatch on one engine, and the rule/queue/role fixtures) is the one
//! `target_dispatch` owns; only the classification is asserted here.

use serde_json::json;
use service_com_amazonaws_eventbridge_awsevents::state::{
    InvocationOutcome, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_rule, given_state_machine, iam, invocations, publish,
    trust_policy, xml_text, Emulator,
};

/// The role every row's target names, the rule every row's target belongs to,
/// and that target's id — the three the refusal's principal and sentence are
/// assembled from.
const ROLE_NAME: &str = "delivery-role";
const RULE_NAME: &str = "classified";
const TARGET_ID: &str = "t1";

/// The target action a state-machine target's invocation performs, and so the one
/// the target's role has to be allowed to make.
const TARGET_ACTION: &str = "states:StartExecution";

/// The action a trust-policy statement has to name for the principal it names to
/// be able to assume the role.
const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// The rule the assume is made FOR, which the assume-role refusal names: the ARN
/// EventBridge mints for a rule of the default bus.
fn rule_arn() -> String {
    format!(
        "arn:aws:events:{}:{}:rule/{RULE_NAME}",
        crate::target_dispatch::REGION,
        engine::emulator_account_id()
    )
}

/// A trust policy with one statement, stating `effect` for `principal` on
/// `action`.
fn trust_statement(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
    .to_string()
}

/// An identity policy with one statement, stating `effect` for `action` on every
/// resource.
fn identity_statement(effect: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": "*" }],
    })
    .to_string()
}

/// Which step of the handover the row's IAM records stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// EventBridge cannot act as the role at all.
    AssumeRole,
    /// It can act as the role, which may not make the target's API call — either
    /// because an identity policy denies it (`denied`) or because none allows it.
    Permissions { denied: bool },
}

impl Step {
    /// The `ERROR_CODE` EventBridge reports for a delivery stopped at this step,
    /// as the LITERAL it stamps rather than through the constant the service
    /// words it with, so a change to that constant's value fails these rows.
    fn code(self) -> &'static str {
        match self {
            Self::AssumeRole => "FAILED_TO_ASSUME_ROLE",
            Self::Permissions { .. } => "NO_PERMISSIONS",
        }
    }
}

/// One row: the two documents in force on the target's role, and the step they
/// stop the delivery at.
struct Row {
    case: &'static str,
    /// The role's trust document.
    trust: String,
    /// The role's one inline identity document, or `None` for a role carrying no
    /// inline policy at all.
    inline: Option<String>,
    expected: Step,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the trust policy names another service principal",
            trust: trust_policy("lambda.amazonaws.com"),
            inline: Some(identity_statement("Allow", TARGET_ACTION)),
            expected: Step::AssumeRole,
        },
        Row {
            case: "the trust policy explicitly denies the EventBridge principal",
            trust: trust_statement("Deny", SERVICE_PRINCIPAL, ASSUME_ROLE_ACTION),
            inline: Some(identity_statement("Allow", TARGET_ACTION)),
            expected: Step::AssumeRole,
        },
        Row {
            case: "the trust policy names an account principal rather than the service",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {
                        "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
                    },
                    "Action": ASSUME_ROLE_ACTION,
                }],
            })
            .to_string(),
            inline: Some(identity_statement("Allow", TARGET_ACTION)),
            expected: Step::AssumeRole,
        },
        Row {
            case: "the trust policy names another action for the principal",
            trust: trust_statement("Allow", SERVICE_PRINCIPAL, "sts:TagSession"),
            inline: Some(identity_statement("Allow", TARGET_ACTION)),
            expected: Step::AssumeRole,
        },
        Row {
            // Neither step would pass: the EARLIER one is the one reported.
            case: "the role neither trusts the principal nor allows the call",
            trust: trust_policy("lambda.amazonaws.com"),
            inline: None,
            expected: Step::AssumeRole,
        },
        Row {
            case: "the role's policy allows another action",
            trust: trust_policy(SERVICE_PRINCIPAL),
            inline: Some(identity_statement("Allow", "sqs:SendMessage")),
            expected: Step::Permissions { denied: false },
        },
        Row {
            case: "the role carries no identity policy at all",
            trust: trust_policy(SERVICE_PRINCIPAL),
            inline: None,
            expected: Step::Permissions { denied: false },
        },
        Row {
            case: "the role's policy explicitly denies the target's call",
            trust: trust_policy(SERVICE_PRINCIPAL),
            inline: Some(identity_statement("Deny", TARGET_ACTION)),
            expected: Step::Permissions { denied: true },
        },
        Row {
            // The trust step passes through a WILDCARD action, so what is left is
            // the permission step.
            case: "the trust policy admits the principal through a wildcard action",
            trust: trust_statement("Allow", SERVICE_PRINCIPAL, "sts:*"),
            inline: Some(identity_statement("Allow", "sqs:SendMessage")),
            expected: Step::Permissions { denied: false },
        },
    ]
}

/// A role carrying exactly the two documents a row names; answers the ARN IAM
/// persisted.
async fn given_role(
    emulator: &Emulator,
    case: &str,
    trust: &str,
    inline: Option<&str>,
) -> String {
    let (status, created) = iam(
        emulator,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    let arn =
        xml_text(&created, "Arn").unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));
    if let Some(document) = inline {
        let (status, written) = iam(
            emulator,
            "PutRolePolicy",
            &[
                ("RoleName", ROLE_NAME),
                ("PolicyName", "delivery"),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutRolePolicy answered {status}: {written}"
        );
    }
    arn
}

#[tokio::test]
async fn the_dead_letter_error_code_names_the_step_the_delivery_failed_at() {
    for row in rows() {
        let emulator = emulator();
        let target_arn = given_state_machine(&emulator, "classified-flow").await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        let role_arn = given_role(&emulator, row.case, &row.trust, row.inline.as_deref()).await;
        given_rule(
            &emulator,
            RULE_NAME,
            json!([{
                "Id": TARGET_ID,
                "Arn": target_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;

        publish(&emulator).await;

        let expected_code = row.expected.code();
        let recorded = invocations(&emulator);
        let outcome = recorded
            .first()
            .map(|(_, _, outcome)| outcome.clone())
            .unwrap_or_else(|| panic!("case {:?}: nothing was recorded", row.case));
        let InvocationOutcome::Failed {
            code: actual,
            message,
        } = &outcome
        else {
            panic!(
                "case {:?}: expected a failed invocation coded {expected_code}, got {outcome:?}",
                row.case
            );
        };
        assert_eq!(
            actual, expected_code,
            "case {:?}: expected the condition {expected_code}, got {outcome:?}",
            row.case
        );

        let dead_letters = drain(&emulator, &dead_letter_url).await;
        assert_eq!(
            dead_letters.len(),
            1,
            "case {:?}: an undeliverable event goes to the dead-letter queue, got {dead_letters:?}",
            row.case
        );
        let attributes = &dead_letters[0]["MessageAttributes"];
        assert_eq!(
            attributes["ERROR_CODE"]["StringValue"],
            json!(expected_code),
            "case {:?}: expected the dead-letter ERROR_CODE {expected_code}, got {:?}",
            row.case,
            dead_letters[0]
        );
        let reported = attributes["ERROR_MESSAGE"]["StringValue"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported, *message,
            "case {:?}: the dead-letter ERROR_MESSAGE is the recorded refusal, got {reported:?}",
            row.case
        );
        // The whole sentence, in the terms the failed step is about: the
        // assume-role step names the role, the principal, the action it may not
        // take and the RULE the assume was for, while the permission step names
        // the SESSION principal the
        // call would have been made as, the call and the resource, and closes on
        // which of the two permission verdicts it was.
        let expected_message = match row.expected {
            Step::AssumeRole => format!(
                "Event bus failed to assume role {role_arn}: its trust policy does not let \
                 {SERVICE_PRINCIPAL} perform {ASSUME_ROLE_ACTION} for {}.",
                rule_arn()
            ),
            Step::Permissions { denied } => {
                let reason = if denied {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    format!("because no identity-based policy allows the {TARGET_ACTION} action")
                };
                format!(
                    "User: arn:aws:sts::{}:assumed-role/{ROLE_NAME}/AWSEvents_{RULE_NAME}_\
                     {TARGET_ID} is not authorized to perform: {TARGET_ACTION} on resource: \
                     {target_arn} {reason}",
                    engine::emulator_account_id()
                )
            }
        };
        assert_eq!(
            reported, expected_message,
            "case {:?}: expected the ERROR_MESSAGE {expected_message:?}, got {reported:?}",
            row.case
        );
    }
}
