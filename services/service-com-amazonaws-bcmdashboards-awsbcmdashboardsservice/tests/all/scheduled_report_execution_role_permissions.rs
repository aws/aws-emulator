// SPDX-License-Identifier: CC0-1.0

//! The health a scheduled report reports for what its execution role may DO — end to end
//! over the wire, with IAM and EventBridge mounted beside this service on one engine.
//!
//! The RULE asserted below: a scheduled report runs ACTING AS the role its
//! `scheduledReportExecutionRoleArn` names, so being able to assume that role is not
//! enough — `HealthStatus$statusCode` documents `HEALTHY` as the report being "configured
//! properly and has all required permissions to execute". Where the policies the account's
//! IAM holds in force on an assumable execution role DENY it the one call an execution makes
//! under that role — the publication of the execution's notification onto the account's
//! default event bus, which is what the model defines a scheduled report's execution by —
//! every view of the report reports `healthStatus.statusCode` `UNHEALTHY` with
//! `EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS` among its `statusReasons`, no execution is
//! triggered and `lastExecutionAt` does not move. The verdict is re-derived from IAM on every
//! resolution, never cached on the record, so withdrawing the deny restores the report's
//! health with no further call to this service — and the call that reports it is a health
//! verdict, HTTP 200 throughout, not an API failure.
//!
//! What the emulator does NOT report is the other side of the same rule: a request the
//! documents this store holds say nothing about is evidence about the store (an allow is
//! routinely provisioned out of band) rather than about the role, so it leaves the health
//! alone, while an explicit deny decides the call whatever else is in force.
//!
//! The table spans the forms the rule generalises over rather than one document: where the
//! deny is carried (an inline role policy, a second inline policy beside an allow, a managed
//! policy attached to the role, the role's permissions boundary), how it spells the action
//! (the call itself, a service wildcard) and the resource (every resource, the bus's own ARN,
//! another Region's bus), and the rows that must stay healthy (no policy at all, an allow of
//! the call, a policy about another action, a deny of another action, a deny of another
//! Region's bus).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::execution_role::{
    EXECUTION_ACTION, SERVICE_PRINCIPAL,
};

use super::scheduled_report_execution::{stored_report, HEALTHY, UNHEALTHY};
use super::scheduled_report_execution_role::{
    assert_every_view, create_report_on_role, emulator, execute, given_role_trusting, grant, iam,
    xml_text, REGION,
};
use super::scheduled_report_lifecycle::source_dashboard;

/// The `StatusReason` a report whose execution role may not do the execution's work carries.
/// Source: `smithy_query(query="shape",shape="StatusReason")` — the value that names what the
/// role's own policies allow, beside `EXECUTION_ROLE_ASSUME_FAILED`, which names a failure of
/// the assume itself.
const EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS: &str = "EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS";

/// The reasons a report whose execution role may not do the execution's work is reported
/// with.
const INSUFFICIENT: &[&str] = &[EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS];

/// The ARN of the default event bus of the Region every request below is signed for: the bus
/// an execution's notification goes to, and so the resource the role's permission to publish
/// it is judged against.
fn default_event_bus_arn() -> String {
    ::engine::emulator_arn("events", REGION, "event-bus/default")
}

/// A statement with one effect over one action and one resource, as a client writes one in an
/// identity-based policy.
fn statement(effect: &str, action: &str, resource: &str) -> Value {
    json!({"Effect": effect, "Action": action, "Resource": resource})
}

/// A policy document holding `statements`.
fn document(statements: Vec<Value>) -> Value {
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// Where one policy document of a row is carried: the three ways a document comes to be in
/// force on a role.
#[derive(Clone, Copy, Debug)]
enum Carrier {
    /// An inline policy embedded in the role (`PutRolePolicy`).
    Inline,
    /// A customer-managed policy attached to the role (`CreatePolicy` + `AttachRolePolicy`).
    Attached,
    /// The managed policy set as the role's permissions boundary
    /// (`CreatePolicy` + `PutRolePermissionsBoundary`).
    Boundary,
}

/// One document a row puts in force on its role, built from the bus ARN the execution
/// publishes to so a row can name that resource exactly.
struct Carried {
    carrier: Carrier,
    name: &'static str,
    document: fn(&str) -> Value,
}

/// Put `carried` in force on the role `role_name`, through the IAM calls a client uses.
async fn put_in_force(
    router: &MultiServiceRouter,
    label: &str,
    role_name: &str,
    carried: &Carried,
) {
    let document = (carried.document)(&default_event_bus_arn()).to_string();
    match carried.carrier {
        Carrier::Inline => {
            let (status, answer) = iam(
                router,
                "PutRolePolicy",
                &[
                    ("RoleName", role_name),
                    ("PolicyName", carried.name),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: PutRolePolicy answered {status}: {answer}"
            );
        }
        Carrier::Attached | Carrier::Boundary => {
            let (status, created) = iam(
                router,
                "CreatePolicy",
                &[
                    ("PolicyName", carried.name),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: CreatePolicy answered {status}: {created}"
            );
            let policy_arn = xml_text(&created, "Arn")
                .unwrap_or_else(|| panic!("case {label:?}: no policy Arn in {created}"));
            let (action, parameter) = match carried.carrier {
                Carrier::Attached => ("AttachRolePolicy", "PolicyArn"),
                _ => ("PutRolePermissionsBoundary", "PermissionsBoundary"),
            };
            let (status, answer) = iam(
                router,
                action,
                &[("RoleName", role_name), (parameter, &policy_arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: {action} answered {status}: {answer}"
            );
        }
    }
}

/// Withdraw the inline policy `name` from the role `role_name`, the way a client repairs a
/// role's permissions outside this service.
async fn delete_inline(router: &MultiServiceRouter, label: &str, role_name: &str, name: &str) {
    let (status, answer) = iam(
        router,
        "DeleteRolePolicy",
        &[("RoleName", role_name), ("PolicyName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: DeleteRolePolicy answered {status}: {answer}"
    );
}

/// One row: the documents in force on the report's execution role (whose trust policy always
/// admits this service, so the assume itself succeeds), and the health every view must report.
struct PermissionCase {
    label: &'static str,
    carried: Vec<Carried>,
    expected_code: &'static str,
    /// `None` means `statusReasons` must be absent.
    expected_reasons: Option<&'static [&'static str]>,
}

fn rows() -> Vec<PermissionCase> {
    vec![
        PermissionCase {
            label: "no policy in force at all",
            carried: vec![],
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        PermissionCase {
            label: "an inline allow of the execution's call",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "allow-execution",
                document: |bus| document(vec![statement("Allow", EXECUTION_ACTION, bus)]),
            }],
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        PermissionCase {
            label: "an inline policy about another action only",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "allow-elsewhere",
                document: |_| {
                    document(vec![statement(
                        "Allow",
                        "s3:GetObject",
                        "arn:aws:s3:::reports/*",
                    )])
                },
            }],
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        PermissionCase {
            label: "an inline deny of the execution's call on every resource",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "deny-execution",
                document: |_| document(vec![statement("Deny", EXECUTION_ACTION, "*")]),
            }],
            expected_code: UNHEALTHY,
            expected_reasons: Some(INSUFFICIENT),
        },
        PermissionCase {
            label: "an allow and a deny of the call in one document",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "allow-then-deny",
                document: |bus| {
                    document(vec![
                        statement("Allow", EXECUTION_ACTION, bus),
                        statement("Deny", EXECUTION_ACTION, bus),
                    ])
                },
            }],
            expected_code: UNHEALTHY,
            expected_reasons: Some(INSUFFICIENT),
        },
        PermissionCase {
            label: "an allow in one inline policy and a deny in a second",
            carried: vec![
                Carried {
                    carrier: Carrier::Inline,
                    name: "allow-execution",
                    document: |bus| document(vec![statement("Allow", EXECUTION_ACTION, bus)]),
                },
                Carried {
                    carrier: Carrier::Inline,
                    name: "deny-execution",
                    document: |bus| document(vec![statement("Deny", EXECUTION_ACTION, bus)]),
                },
            ],
            expected_code: UNHEALTHY,
            expected_reasons: Some(INSUFFICIENT),
        },
        PermissionCase {
            label: "an inline deny with a service wildcard on the bus's own ARN",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "deny-every-events-call",
                document: |bus| document(vec![statement("Deny", "events:*", bus)]),
            }],
            expected_code: UNHEALTHY,
            expected_reasons: Some(INSUFFICIENT),
        },
        PermissionCase {
            label: "a deny carried by a managed policy attached to the role",
            carried: vec![Carried {
                carrier: Carrier::Attached,
                name: "attached-deny",
                document: |_| document(vec![statement("Deny", EXECUTION_ACTION, "*")]),
            }],
            expected_code: UNHEALTHY,
            expected_reasons: Some(INSUFFICIENT),
        },
        PermissionCase {
            label: "a deny carried by the role's permissions boundary",
            carried: vec![Carried {
                carrier: Carrier::Boundary,
                name: "boundary-deny",
                document: |_| document(vec![statement("Deny", EXECUTION_ACTION, "*")]),
            }],
            expected_code: UNHEALTHY,
            expected_reasons: Some(INSUFFICIENT),
        },
        PermissionCase {
            label: "an inline deny naming another Region's default bus",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "deny-another-region",
                document: |_| {
                    document(vec![statement(
                        "Deny",
                        EXECUTION_ACTION,
                        &::engine::emulator_arn("events", "eu-west-1", "event-bus/default"),
                    )])
                },
            }],
            expected_code: HEALTHY,
            expected_reasons: None,
        },
        PermissionCase {
            label: "an inline deny of another action on the bus",
            carried: vec![Carried {
                carrier: Carrier::Inline,
                name: "deny-another-action",
                document: |bus| document(vec![statement("Deny", "events:DeleteRule", bus)]),
            }],
            expected_code: HEALTHY,
            expected_reasons: None,
        },
    ]
}

/// RULE: every view of a scheduled report reports `UNHEALTHY` with
/// `EXECUTION_ROLE_INSUFFICIENT_PERMISSIONS` exactly while the policies the account's IAM
/// holds in force on the role it executes as deny that role the call an execution makes — and
/// a non-dry-run execution of such a report triggers nothing, while one whose role is not
/// denied that call is triggered.
#[tokio::test]
async fn a_reports_health_follows_what_its_execution_role_may_do() {
    for (index, row) in rows().into_iter().enumerate() {
        let label = row.label;
        // A fresh store per row, so one row's policies cannot answer for another's.
        let router = emulator();
        let (dashboard_arn, _) = source_dashboard(&router, "PermissionSource").await;
        let role_name = format!("permission-role-{index}");
        // The trust half always holds: what varies is what the role may DO.
        let role_arn =
            given_role_trusting(&router, label, &role_name, &grant(SERVICE_PRINCIPAL)).await;
        for carried in &row.carried {
            put_in_force(&router, label, &role_name, carried).await;
        }
        let arn = create_report_on_role(
            &router,
            &dashboard_arn,
            &format!("Permission Case {index}"),
            &role_arn,
        )
        .await;

        assert_every_view(
            &router,
            label,
            &arn,
            row.expected_code,
            row.expected_reasons,
        )
        .await;

        // The execution itself: a report whose role may not make the execution's call has
        // nothing it could do under it, so nothing is triggered and `lastExecutionAt` does
        // not move.
        let (status, executed) = execute(&router, &arn, false).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an execution request is answered, not refused: {executed}"
        );
        let healthy = row.expected_code == HEALTHY;
        assert_eq!(
            executed.get("executionTriggered").and_then(Value::as_bool),
            Some(healthy),
            "case {label:?}: expected executionTriggered {healthy}, got {executed}"
        );
        let stored = stored_report(&router, &arn).await;
        assert_eq!(
            stored.get("lastExecutionAt").is_some(),
            healthy,
            "case {label:?}: expected lastExecutionAt present={healthy}, got {stored}"
        );
    }
}

/// RULE, the other direction: the verdict is re-derived from IAM on every resolution and
/// never cached, so withdrawing the deny from a role a report was reported unhealthy for
/// makes the report `HEALTHY` again — and the reason the unhealthy refresh stored is gone
/// rather than carried forward — with no call to this service in between.
#[tokio::test]
async fn withdrawing_the_deny_restores_the_reports_health() {
    let label = "deny withdrawn";
    let router = emulator();
    let (dashboard_arn, _) = source_dashboard(&router, "WithdrawnSource").await;
    let role_name = "withdrawn-deny-role";
    let role_arn = given_role_trusting(&router, label, role_name, &grant(SERVICE_PRINCIPAL)).await;
    let denial = Carried {
        carrier: Carrier::Inline,
        name: "deny-execution",
        document: |_| document(vec![statement("Deny", EXECUTION_ACTION, "*")]),
    };
    put_in_force(&router, label, role_name, &denial).await;
    let arn = create_report_on_role(&router, &dashboard_arn, "Withdrawn Deny", &role_arn).await;

    assert_every_view(&router, label, &arn, UNHEALTHY, Some(INSUFFICIENT)).await;
    let (_, refused) = execute(&router, &arn, false).await;
    assert_eq!(
        refused.get("executionTriggered").and_then(Value::as_bool),
        Some(false),
        "case {label:?}: a report whose role may not publish the notification triggers \
         nothing: {refused}"
    );
    assert!(
        stored_report(&router, &arn).await.get("lastExecutionAt").is_none(),
        "case {label:?}: an execution that was not triggered must not set lastExecutionAt"
    );

    // The repair, made outside this service: the deny is withdrawn from the role.
    delete_inline(&router, label, role_name, denial.name).await;
    assert_every_view(&router, label, &arn, HEALTHY, None).await;
    let (_, executed) = execute(&router, &arn, false).await;
    assert_eq!(
        executed.get("executionTriggered").and_then(Value::as_bool),
        Some(true),
        "case {label:?}: the repaired report must execute: {executed}"
    );
    assert!(
        stored_report(&router, &arn).await.get("lastExecutionAt").is_some(),
        "case {label:?}: the triggered execution must record lastExecutionAt"
    );
}

/// RULE: the two role reasons name different failures of ONE role and the assume comes
/// first — a report whose role this service cannot assume at all never reaches the
/// permissions in force on it, so such a report reports the assume reason alone however its
/// policies read.
#[tokio::test]
async fn a_role_that_cannot_be_assumed_reports_the_assume_reason_alone() {
    let label = "trust broken and the call denied";
    let router = emulator();
    let (dashboard_arn, _) = source_dashboard(&router, "PrecedenceSource").await;
    let role_name = "unassumable-denied-role";
    let role_arn =
        given_role_trusting(&router, label, role_name, &grant("ec2.amazonaws.com")).await;
    put_in_force(
        &router,
        label,
        role_name,
        &Carried {
            carrier: Carrier::Inline,
            name: "deny-execution",
            document: |_| document(vec![statement("Deny", EXECUTION_ACTION, "*")]),
        },
    )
    .await;
    let arn = create_report_on_role(&router, &dashboard_arn, "Precedence", &role_arn).await;

    assert_every_view(
        &router,
        label,
        &arn,
        UNHEALTHY,
        // The assume reason alone: the execution never gets as far as the role's own
        // permissions.
        Some(&["EXECUTION_ROLE_ASSUME_FAILED"]),
    )
    .await;
}
