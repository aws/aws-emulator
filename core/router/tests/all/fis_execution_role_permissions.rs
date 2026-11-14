// SPDX-License-Identifier: CC0-1.0

//! FIS's execution-role PERMISSION check, across the two services it spans.
//!
//! RULE: FIS runs an experiment's actions under the template's `roleArn`, so an action
//! whose own API calls that role is not allowed to make cannot be carried out: the
//! experiment reaches the terminal status `failed`, and its `ExperimentState` ATTRIBUTES
//! that failure to insufficient privileges — in the reason, and in the `ExperimentError`
//! the model pairs with it ("the error information of the experiment when the action has
//! failed") — so a client can tell a run AWS would have refused on IAM from one that failed
//! for any other reason. An otherwise identical template whose role DOES carry the
//! permission is not failed for privileges: it runs.
//!
//! The decision comes from the ACCOUNT's own IAM records, which is why this suite lives
//! here: the role, its inline policies and the managed policies attached to it are all
//! created through the real IAM operations on the same engine that serves FIS, so what the
//! check reads is what IAM persisted and not a copy. The rows differ along the form of the
//! permission in force on the role — which action a statement names, whether it is a
//! wildcard, which resources it covers, whether an explicit `Deny` stands beside the
//! `Allow`, whether the document is inline or a managed policy attached to the role, and
//! whether that managed policy is one this account can read at all — and along whether the
//! account holds the role in the first place.
//!
//! The IAM action an FIS action needs is the one the FIS actions reference states for it;
//! these rows use `aws:cloudwatch:assert-alarm-state` (`cloudwatch:DescribeAlarms`),
//! `aws:ec2:stop-instances` (`ec2:StopInstances`) and the `aws:fis:inject-api-*` family
//! (`ec2:InjectApiError`, which the reference scopes with the `ec2:FisActionId` condition
//! key — the gate decides with that key in the request's condition context, so a policy
//! written the documented way grants the action it names and not another), so the rule is
//! not asserted for a single action id.
//!
//! The gate decides about the actions the run CARRIES OUT, which is what the experiment
//! options select: the last case below starts one under-privileged template twice, under
//! `actionsMode` `run-all` and `skip-all`, and starts a second one whose action names a
//! target that resolved to nothing under `emptyTargetResolutionMode: skip` — an action a
//! run leaves out cannot be refused a privilege, because nothing ran it.
//!
//! # What the gate this suite drives does, and what it cannot do
//!
//! Recorded here because it is what the last case below pins, and because it is the whole
//! shape of the work the change adds:
//!
//! * THE BOUND. The gate's cost is the product of four factors, each a constant declared
//!   in the service rather than a function of what a caller stored: the actions of one
//!   template (`state::MAX_ACTIONS_PER_TEMPLATE` = 20, refused at create time), the IAM
//!   actions one of them needs (`state::ActionCatalogEntry::permissions`, a constant table
//!   with at most one entry per action today), the resources one action is decided against
//!   (`state::action_permissions::MAX_ACTION_RESOURCES` = 8, applied in the one
//!   `push_bounded` site in `action_resources`), and the documents in force on the role
//!   (`state::action_permissions::MAX_ROLE_DOCUMENTS` = 32, past which the gate shows
//!   nothing and the run goes ahead). The PRODUCT is bounded too:
//!   `state::action_permissions::MAX_PERMISSION_DECISIONS` = 32 decisions per start, after
//!   which the gate stops deciding. Each document is parsed ONCE per start
//!   (`service_common::iam_authorization::prepare_documents`), so a decision is a walk over
//!   statements already parsed rather than a JSON parse, and the first grant ends the walk
//!   over an action's resources. Nothing in the walk reads the stored `parameters` map's
//!   size, which is the one thing a caller can grow without limit — the last case below
//!   drives exactly that (a 200-entry map packed with ARNs, ≈14,000 candidates if nothing
//!   capped them) and asserts the start still answers promptly.
//! * NO OUTBOUND ANYTHING. The gate decides from PERSISTED state: the role, its inline
//!   policies and the documents of the policies attached to it, read out of the IAM store
//!   in the shared engine. It opens no connection, spawns no process, reads no environment
//!   and touches no file, and it does not reach the service an action's id names: a value
//!   the caller put in a parameter (`alarmArns`, `service`, `operations`) is compared
//!   against policy statements and never dialled. What an action's effect on another
//!   service is carried out through, where this emulator can carry one out, is that
//!   service's own store in the same engine — never a connection out of it — and the gate
//!   this suite drives decides whether the action runs at all, so there is no delivery path
//!   here to confine.
//! * NO CALLER PROGRAM IS EVALUATED. An `actionId` resolves through a fixed catalog
//!   (`state::ACTION_CATALOG`, a constant table; an id it does not hold is decided by
//!   nothing and failed for nothing) and its `parameters` are read for two documented
//!   values only: `duration`, as an ISO 8601 span, and ARNs, as resource candidates for the
//!   policy decision. Nothing is interpreted, compiled or spawned.
//! * NOTHING SECRET IS WRITTEN. The gate reads policy documents and role ARNs and compares
//!   them; it writes neither to a log nor to disk. The only value of theirs that leaves the
//!   service is the role ARN inside the experiment's own `roleArn` member and its failure
//!   reason, both of which the API already reports to the caller that supplied it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder for an `application/x-www-form-urlencoded` component, which
// `all/cloudfront_key_value_store_handshake.rs` already owns.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// A SigV4 credential scope naming fis, which is what routes a request to that service.
const FIS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/fis/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The alarm the `aws:cloudwatch:assert-alarm-state` action in these templates names, which
/// is the resource a statement may scope the permission to.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:fis-probe";

/// Another alarm, which the action does NOT name: a statement scoped to it covers nothing
/// the action does.
const OTHER_ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:unrelated";

/// How long after the start the experiment's first transition must have been materialised.
/// The emulator's transition span is a fraction of a second.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// How long a row waits for a run that fails before it starts to report that failure: one
/// transition span, so this is generous.
const FAILURE_BUDGET: Duration = Duration::from_secs(5);

/// The attribution AWS words an insufficient-privileges experiment failure with, as the
/// recorded run reports it. What the rule requires is that the reason says the privileges
/// were insufficient — this is the phrase that says it.
const PRIVILEGES_ATTRIBUTION: &str = "not enough privileges";

/// One emulator carrying both services over ONE shared engine, exactly as the production
/// router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// The text of the first `<tag>` element of an XML body.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"))
        + start;
    body[start..end].to_string()
}

/// One restJson1 call to FIS, answered with the HTTP status and the JSON body.
async fn fis(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", FIS_AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A trust policy that lets FIS assume the role, which every row's role carries: the start
/// is refused outright without it, and what these rows are about is what happens after the
/// start.
fn fis_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "fis.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// An identity policy document with one statement.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": resource }]
    })
    .to_string()
}

/// How a row's permission is put in force on its role.
enum Grant {
    /// No policy at all: the role carries only its trust policy.
    None,
    /// One inline role policy (`PutRolePolicy`) carrying this document.
    Inline(String),
    /// A customer-managed policy created by `CreatePolicy` and attached to the role.
    Attached(String),
    /// An AWS-MANAGED policy attached to the role: AWS owns the document, so this account
    /// cannot read it.
    AwsManaged(&'static str),
}

/// One row: what is in force on the experiment's execution role, and whether the run is
/// failed for privileges because of it.
struct Row {
    case: &'static str,
    /// The FIS action id the template's single action carries.
    action_id: &'static str,
    /// The IAM action that action needs, which the reason has to name when it fails.
    permission: &'static str,
    grant: Grant,
    /// `true` when the run must end `failed` with an insufficient-privileges attribution.
    denied: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the action allowed on every resource",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Inline(statement("Allow", "cloudwatch:DescribeAlarms", "*")),
            denied: false,
        },
        Row {
            case: "the action allowed through a service wildcard",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Inline(statement("Allow", "cloudwatch:*", "*")),
            denied: false,
        },
        Row {
            case: "the action allowed on the alarm the action names",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Inline(statement("Allow", "cloudwatch:DescribeAlarms", ALARM_ARN)),
            denied: false,
        },
        Row {
            case: "the action allowed only on an alarm the action does not name",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Inline(statement(
                "Allow",
                "cloudwatch:DescribeAlarms",
                OTHER_ALARM_ARN,
            )),
            denied: true,
        },
        Row {
            case: "another action allowed, and not this one",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Inline(statement("Allow", "cloudwatch:PutMetricAlarm", "*")),
            denied: true,
        },
        Row {
            case: "no policy on the role at all",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::None,
            denied: true,
        },
        Row {
            // IAM's deny precedence, which the shared evaluator applies: an explicit
            // `Deny` wins over any `Allow`.
            case: "the action allowed and explicitly denied",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Inline(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        { "Effect": "Allow", "Action": "cloudwatch:*", "Resource": "*" },
                        {
                            "Effect": "Deny",
                            "Action": "cloudwatch:DescribeAlarms",
                            "Resource": "*"
                        }
                    ]
                })
                .to_string(),
            ),
            denied: true,
        },
        Row {
            case: "a customer-managed policy attached to the role",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::Attached(statement("Allow", "cloudwatch:DescribeAlarms", "*")),
            denied: false,
        },
        Row {
            // A policy AWS owns: this account holds no document for it, so it MIGHT be the
            // one that carries the grant and nothing can be shown against the role.
            case: "an AWS-managed policy attached to the role",
            action_id: "aws:cloudwatch:assert-alarm-state",
            permission: "cloudwatch:DescribeAlarms",
            grant: Grant::AwsManaged("arn:aws:iam::aws:policy/CloudWatchReadOnlyAccess"),
            denied: false,
        },
        Row {
            // Another action, so the rule is not about one action id: the permission the
            // catalog records for it is the one that has to be allowed.
            case: "another FIS action, allowed",
            action_id: "aws:ec2:stop-instances",
            permission: "ec2:StopInstances",
            grant: Grant::Inline(statement("Allow", "ec2:StopInstances", "*")),
            denied: false,
        },
        Row {
            case: "another FIS action, not allowed",
            action_id: "aws:ec2:stop-instances",
            permission: "ec2:StopInstances",
            grant: Grant::Inline(statement("Allow", "ec2:DescribeInstances", "*")),
            denied: true,
        },
        Row {
            // The api-fault family: the FIS actions reference states `ec2:InjectApiError`
            // for it, so that is the permission the catalog records and the one a role has
            // to carry.
            case: "an api-fault injection allowed on every resource",
            action_id: "aws:fis:inject-api-internal-error",
            permission: "ec2:InjectApiError",
            grant: Grant::Inline(statement("Allow", "ec2:InjectApiError", "*")),
            denied: false,
        },
        Row {
            // The documented form of that grant: scoped by the `ec2:FisActionId` condition
            // key. The gate decides with that key in the request's condition context, so a
            // policy written the way the reference writes it GRANTS the action it names.
            case: "an api-fault injection allowed under the documented condition",
            action_id: "aws:fis:inject-api-internal-error",
            permission: "ec2:InjectApiError",
            grant: Grant::Inline(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "ec2:InjectApiError",
                        "Resource": "*",
                        "Condition": {
                            "StringEquals": {
                                "ec2:FisActionId": "aws:fis:inject-api-internal-error"
                            }
                        }
                    }]
                })
                .to_string(),
            ),
            denied: false,
        },
        Row {
            // The same scoped grant, naming ANOTHER action: the condition demonstrably
            // does not hold for this run, so the statement does not apply and nothing
            // allows it.
            case: "an api-fault injection allowed only for another action id",
            action_id: "aws:fis:inject-api-throttle-error",
            permission: "ec2:InjectApiError",
            grant: Grant::Inline(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "ec2:InjectApiError",
                        "Resource": "*",
                        "Condition": {
                            "StringEquals": {
                                "ec2:FisActionId": "aws:fis:inject-api-internal-error"
                            }
                        }
                    }]
                })
                .to_string(),
            ),
            denied: true,
        },
    ]
}

/// The template body for one row: a single action of the row's action id, which for the
/// CloudWatch assert names the alarm a scoped statement may cover and for the api-fault
/// family carries the parameters that action documents as required.
fn template_body(index: usize, row: &Row, role_arn: &str) -> Value {
    let action = if row.action_id == "aws:cloudwatch:assert-alarm-state" {
        json!({
            "actionId": row.action_id,
            "parameters": { "alarmArns": ALARM_ARN, "alarmStates": "INSUFFICIENT_DATA" }
        })
    } else if row.action_id.starts_with("aws:fis:inject-api-") {
        json!({
            "actionId": row.action_id,
            "parameters": {
                "duration": "PT2M",
                "service": "ec2",
                "operations": "DescribeAvailabilityZones"
            }
        })
    } else {
        json!({ "actionId": row.action_id })
    };
    json!({
        "clientToken": format!("permission-template-{index}"),
        "description": format!("role permissions: {}", row.case),
        "roleArn": role_arn,
        "stopConditions": [{ "source": "none" }],
        // `targets` is a member FIS requires of a create request.
        "targets": {},
        "actions": { "underTest": action }
    })
}

#[tokio::test]
async fn an_experiment_whose_role_may_not_perform_an_action_fails_on_privileges() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        // The role, created through the real `CreateRole` so FIS reads the record IAM
        // persisted.
        let role_name = format!("fis-permissions-{index}");
        let (status, created) = iam(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", fis_trust().as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateRole to persist the role, got {status}: {created}",
            row.case
        );
        let role_arn = element(&created, "Arn");

        match &row.grant {
            Grant::None => {}
            Grant::Inline(document) => {
                let (status, answer) = iam(
                    &router,
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.as_str()),
                        ("PolicyName", "fis-permission"),
                        ("PolicyDocument", document.as_str()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected PutRolePolicy to persist the policy, \
                     got {status}: {answer}",
                    row.case
                );
            }
            Grant::Attached(document) => {
                let policy_name = format!("fis-managed-{index}");
                let (status, answer) = iam(
                    &router,
                    "CreatePolicy",
                    &[
                        ("PolicyName", policy_name.as_str()),
                        ("PolicyDocument", document.as_str()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected CreatePolicy to persist the policy, \
                     got {status}: {answer}",
                    row.case
                );
                let policy_arn = element(&answer, "Arn");
                let (status, attached) = iam(
                    &router,
                    "AttachRolePolicy",
                    &[
                        ("RoleName", role_name.as_str()),
                        ("PolicyArn", policy_arn.as_str()),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected AttachRolePolicy to attach it, \
                     got {status}: {attached}",
                    row.case
                );
            }
            Grant::AwsManaged(policy_arn) => {
                let (status, attached) = iam(
                    &router,
                    "AttachRolePolicy",
                    &[("RoleName", role_name.as_str()), ("PolicyArn", policy_arn)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: expected AttachRolePolicy to attach the AWS-managed policy, \
                     got {status}: {attached}",
                    row.case
                );
            }
        }

        let (status, template) = fis(
            &router,
            "POST",
            "/experimentTemplates",
            Some(template_body(index, row, &role_arn)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the template to be created, got {status}: {template}",
            row.case
        );
        let template_id = template["experimentTemplate"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The start is ACCEPTED either way: AWS accepts an experiment under an
        // under-privileged role and then fails it, so the attribution is read off the
        // experiment's state and not off a refusal.
        let (status, started) = fis(
            &router,
            "POST",
            "/experiments",
            Some(json!({
                "clientToken": format!("permission-start-{index}"),
                "experimentTemplateId": template_id
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the start to be accepted, got {status}: {started}",
            row.case
        );
        let experiment_id = started["experiment"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            started["experiment"]["state"]["status"].as_str(),
            Some("initiating"),
            "case {:?}: expected a started experiment to be initiating, got {started}",
            row.case
        );

        let state = settled_state(&router, &experiment_id, row.denied).await;
        let status_reported = state["status"].as_str().unwrap_or_default().to_string();
        let reason = state["reason"].as_str().unwrap_or_default().to_string();

        if row.denied {
            assert_eq!(
                status_reported, "failed",
                "case {:?}: expected a run the role may not perform to fail, got {state}",
                row.case
            );
            assert!(
                reason.to_lowercase().contains(PRIVILEGES_ATTRIBUTION),
                "case {:?}: expected the reason to attribute the failure to insufficient \
                 privileges, got {reason:?}",
                row.case
            );
            // Built from the request and the account's own records: the role that could not
            // perform it, the action that could not run, and the IAM action it needed.
            for named in [role_arn.as_str(), "underTest", row.action_id, row.permission] {
                assert!(
                    reason.contains(named),
                    "case {:?}: expected the reason to name {named:?}, got {reason:?}",
                    row.case
                );
            }
            let error = state["error"].clone();
            assert!(
                error.is_object(),
                "case {:?}: expected a failed experiment to report an ExperimentError, \
                 got {state}",
                row.case
            );
            assert_eq!(
                error["location"].as_str(),
                Some("actions.underTest"),
                "case {:?}: expected the error to locate the action that failed, got {error}",
                row.case
            );
            assert!(
                error["code"]
                    .as_str()
                    .is_some_and(|code| !code.is_empty() && !code.contains(char::is_whitespace)),
                "case {:?}: expected an ExperimentErrorCode within its modelled pattern, \
                 got {error}",
                row.case
            );
            assert_eq!(
                error["accountId"].as_str(),
                Some("000000000000"),
                "case {:?}: expected the error to name the account the run failed in, \
                 got {error}",
                row.case
            );
        } else {
            assert_eq!(
                status_reported, "running",
                "case {:?}: expected a role that carries the permission to run, got {state}",
                row.case
            );
            assert!(
                !reason.to_lowercase().contains(PRIVILEGES_ATTRIBUTION),
                "case {:?}: a run nothing was shown against carries no privileges reason, \
                 got {reason:?}",
                row.case
            );
            assert!(
                state["error"].is_null(),
                "case {:?}: a run that has not failed reports no error, got {state}",
                row.case
            );
            // This row is done with its experiment: stop it, so no later row can be
            // refused for having reached the account's active-experiment quota.
            let (status, stopped) =
                fis(&router, "DELETE", &format!("/experiments/{experiment_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the stop to be accepted, got {status}: {stopped}",
                row.case
            );
        }
    }
}

/// RULE (the complement): a `roleArn` the account's IAM store holds no role for shows
/// NOTHING about its permissions — the role may live in another account, or in an IAM this
/// emulator was never told about — so the run is not failed for privileges.
#[tokio::test]
async fn an_experiment_whose_role_the_iam_store_does_not_hold_is_not_failed_for_privileges() {
    let router = emulator();

    // The rows differ along the shape of the unheld ARN: this account's own namespace, a
    // role with a path, another account, and another partition.
    let rows = [
        ("this account", "arn:aws:iam::000000000000:role/not-created"),
        (
            "a role with a path",
            "arn:aws:iam::000000000000:role/team/not-created",
        ),
        ("another account", "arn:aws:iam::123456789012:role/remote"),
        (
            "another partition",
            "arn:aws-us-gov:iam::000000000000:role/remote",
        ),
    ];

    for (index, (case, role_arn)) in rows.iter().enumerate() {
        let (status, template) = fis(
            &router,
            "POST",
            "/experimentTemplates",
            Some(json!({
                "clientToken": format!("unheld-permission-template-{index}"),
                "description": format!("role not in iam: {case}"),
                "roleArn": role_arn,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": {
                    "underTest": {
                        "actionId": "aws:cloudwatch:assert-alarm-state",
                        "parameters": {
                            "alarmArns": ALARM_ARN,
                            "alarmStates": "INSUFFICIENT_DATA"
                        }
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the template to be created, got {status}: {template}"
        );
        let template_id = template["experimentTemplate"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, started) = fis(
            &router,
            "POST",
            "/experiments",
            Some(json!({
                "clientToken": format!("unheld-permission-start-{index}"),
                "experimentTemplateId": template_id
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the start to go ahead, got {status}: {started}"
        );
        let experiment_id = started["experiment"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        tokio::time::sleep(PAST_TRANSITION).await;
        let (status, read) = fis(
            &router,
            "GET",
            &format!("/experiments/{experiment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetExperiment to answer, got {status}: {read}"
        );
        assert_eq!(
            read["experiment"]["state"]["status"].as_str(),
            Some("running"),
            "case {case:?}: expected a role nothing is known about to run, got {read}"
        );

        let (status, stopped) =
            fis(&router, "DELETE", &format!("/experiments/{experiment_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stop to be accepted, got {status}: {stopped}"
        );
    }
}

/// RULE (the bound): the privileges gate's cost is a CONSTANT of the service, not a
/// function of what a caller stored. An action's `parameters` are persisted exactly as the
/// create sent them and nothing bounds that map, so a template may name thousands of ARNs
/// in one parameter; the start still answers promptly, and the role that carries the
/// permission still runs.
#[tokio::test]
async fn the_privileges_gate_is_bounded_by_the_service_and_not_by_the_request() {
    let router = emulator();

    let (status, created) = iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", "fis-permissions-bounded"),
            ("AssumeRolePolicyDocument", fis_trust().as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateRole to persist the role, got {status}: {created}"
    );
    let role_arn = element(&created, "Arn");
    let (status, answer) = iam(
        &router,
        "PutRolePolicy",
        &[
            ("RoleName", "fis-permissions-bounded"),
            ("PolicyName", "fis-permission"),
            (
                "PolicyDocument",
                statement("Allow", "cloudwatch:DescribeAlarms", "*").as_str(),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected PutRolePolicy to persist the policy, got {status}: {answer}"
    );

    // The rows differ along how much the action's `parameters` map carries: the two
    // parameters the action is really about, and then two sizes of map packed with ARNs far
    // past anything a real template names. `ExperimentTemplateActionParameterMap` bounds a
    // parameter's NAME at 64 characters and its VALUE at 1,024, but not how many entries
    // the map holds — which is the growth a bounded gate has to be immune to.
    const ARNS_PER_VALUE: usize = 70;
    for (case, extra_parameters) in [
        ("no extra parameters", 0usize),
        ("fifty", 50),
        ("two hundred", 200),
    ] {
        // A value packed with the shortest ARNs that parse, inside the modelled 1,024.
        let packed = (0..ARNS_PER_VALUE)
            .map(|index| format!("arn:a:b::c:{index}"))
            .collect::<Vec<String>>()
            .join(",");
        assert!(
            packed.len() <= 1_024,
            "case {case:?}: the test built a parameter value past the modelled ceiling \
             ({} characters)",
            packed.len()
        );
        let mut parameters = serde_json::Map::new();
        parameters.insert("alarmArns".to_string(), json!(ALARM_ARN));
        parameters.insert("alarmStates".to_string(), json!("INSUFFICIENT_DATA"));
        for index in 0..extra_parameters {
            parameters.insert(format!("packed{index}"), json!(packed));
        }
        let (status, template) = fis(
            &router,
            "POST",
            "/experimentTemplates",
            Some(json!({
                "clientToken": format!("bounded-template-{extra_parameters}"),
                "description": format!("a bounded gate: {case}"),
                "roleArn": role_arn,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": {
                    "underTest": {
                        "actionId": "aws:cloudwatch:assert-alarm-state",
                        "parameters": Value::Object(parameters)
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the template to be created, got {status}: {template}"
        );
        let template_id = template["experimentTemplate"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let began = std::time::Instant::now();
        let (status, started) = fis(
            &router,
            "POST",
            "/experiments",
            Some(json!({
                "clientToken": format!("bounded-start-{extra_parameters}"),
                "experimentTemplateId": template_id
            })),
        )
        .await;
        let elapsed = began.elapsed();
        assert_eq!(
            status, 200,
            "case {case:?}: expected the start to be accepted, got {status}: {started}"
        );
        // The gate reads at most a fixed number of candidates and decides each against
        // documents read once, so the start does not slow down as the stored parameter
        // grows. Generous enough for a loaded machine, and orders of magnitude under what
        // an unbounded walk over this list would cost.
        assert!(
            elapsed < Duration::from_secs(2),
            "case {case:?}: expected the start to answer promptly however much the \
             parameter carries, got {elapsed:?}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let experiment_id = started["experiment"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, read) = fis(
            &router,
            "GET",
            &format!("/experiments/{experiment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetExperiment to answer, got {status}: {read}"
        );
        assert_eq!(
            read["experiment"]["state"]["status"].as_str(),
            Some("running"),
            "case {case:?}: expected the permissioned role to run, got {read}"
        );
        let (status, stopped) =
            fis(&router, "DELETE", &format!("/experiments/{experiment_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stop to be accepted, got {status}: {stopped}"
        );
    }
}

/// The experiment's `ExperimentState` once the transition the clock owes has fallen due: a
/// row that must fail is polled until it does (its run never starts, so that is one
/// transition span), and a row that must not is read once past the transition.
async fn settled_state(router: &MultiServiceRouter, experiment_id: &str, denied: bool) -> Value {
    const INTERVAL: Duration = Duration::from_millis(100);
    let began = std::time::Instant::now();
    loop {
        tokio::time::sleep(if denied { INTERVAL } else { PAST_TRANSITION }).await;
        let (status, read) = fis(
            router,
            "GET",
            &format!("/experiments/{experiment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "expected GetExperiment to answer for {experiment_id}, got {status}: {read}"
        );
        let state = read["experiment"]["state"].clone();
        let reported = state["status"].as_str().unwrap_or_default().to_string();
        if !denied || reported == "failed" || began.elapsed() >= FAILURE_BUDGET {
            return state;
        }
    }
}

/// RULE (the complement the experiment options make): the gate decides only about the
/// actions the run CARRIES OUT, so an action the options leave out of the run earns no
/// failure attributed to it — the experiment is not failed for a privilege the role lacks
/// for an action nothing ran, and it reports that action as `skipped` instead.
///
/// The rows are the two ways an option leaves an action out, against ONE role that carries
/// no permission at all: `actionsMode: skip-all` (the same template as the row that fails,
/// started under the other mode, so the option is the only difference) and
/// `emptyTargetResolutionMode: skip` on an action whose target resolved to no resources.
#[tokio::test]
async fn an_action_the_experiment_options_skip_earns_no_privilege_failure() {
    /// How long a row waits for a run that completes: the compressed floor plus the
    /// transitions around it and slack for a loaded machine.
    const COMPLETION_BUDGET: Duration = Duration::from_secs(20);

    let router = emulator();

    // One role, created through the real `CreateRole`, carrying NO identity policy: the
    // account's own records therefore show it may perform neither
    // `cloudwatch:DescribeAlarms`, which `aws:cloudwatch:assert-alarm-state` needs, nor
    // `ec2:StopInstances`, which `aws:ec2:stop-instances` needs.
    let (status, created) = iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", "fis-permissions-skipped"),
            ("AssumeRolePolicyDocument", fis_trust().as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateRole to persist the role, got {status}: {created}"
    );
    let role_arn = element(&created, "Arn");

    let action = json!({
        "actionId": "aws:cloudwatch:assert-alarm-state",
        "parameters": { "alarmArns": ALARM_ARN, "alarmStates": "INSUFFICIENT_DATA" }
    });
    // Two templates: one plain, and one whose action names a target identified by TAG —
    // which no instance of this engine carries, so it resolves to nothing — under the mode
    // that skips such an action.
    // The alarm assert takes no target, so the tagged one is an action that does: stopping
    // instances, whose `ec2:StopInstances` this role is no more allowed to perform.
    let tagged_action = json!({
        "actionId": "aws:ec2:stop-instances",
        "targets": { "Instances": "tagged" }
    });
    let templates = [
        (
            "plain",
            json!({
                "clientToken": "skipped-permission-template-plain",
                "description": "an under-privileged action, carried out or skipped",
                "roleArn": role_arn,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "underTest": action }
            }),
        ),
        (
            "empty-target-skip",
            json!({
                "clientToken": "skipped-permission-template-empty",
                "description": "an under-privileged action whose target resolves to nothing",
                "roleArn": role_arn,
                "stopConditions": [{ "source": "none" }],
                "experimentOptions": { "emptyTargetResolutionMode": "skip" },
                "targets": {
                    "tagged": {
                        "resourceType": "aws:ec2:instance",
                        "resourceTags": { "Stage": "test" },
                        "selectionMode": "ALL"
                    }
                },
                "actions": { "underTest": tagged_action }
            }),
        ),
    ];
    let mut template_ids = std::collections::BTreeMap::new();
    for (label, body) in templates {
        let (status, template) = fis(&router, "POST", "/experimentTemplates", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the template to be created, got {status}: {template}"
        );
        template_ids.insert(
            label,
            template["experimentTemplate"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // (case, template, the actionsMode the start names, whether the run must fail on
    // privileges, whether the action must report `skipped`)
    let rows: [(&str, &str, Option<&str>, bool, bool); 3] = [
        ("run-all carries the action out", "plain", Some("run-all"), true, false),
        ("skip-all carries it out not at all", "plain", Some("skip-all"), false, true),
        (
            "an empty target under skip leaves it out",
            "empty-target-skip",
            None,
            false,
            true,
        ),
    ];

    for (index, (case, template, actions_mode, must_fail, must_skip)) in rows.iter().enumerate() {
        let (case, template, actions_mode, must_fail, must_skip) =
            (*case, *template, *actions_mode, *must_fail, *must_skip);
        let mut body = json!({
            "clientToken": format!("skipped-permission-start-{index}"),
            "experimentTemplateId": template_ids[template]
        });
        if let Some(mode) = actions_mode {
            body["experimentOptions"] = json!({ "actionsMode": mode });
        }
        let (status, started) = fis(&router, "POST", "/experiments", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the start to be accepted, got {status}: {started}"
        );
        let experiment_id = started["experiment"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The action the options SKIP reports the modelled `skipped` status, which is the
        // decision those options made. An action the run CARRIES OUT reports where the run
        // is in its own span — at the start, the beginning of it — unless the start already
        // showed a failure the run cannot get past, in which case nothing is carried out and
        // no progress is claimed for it.
        let expected_status = if must_skip {
            Some("skipped")
        } else if must_fail {
            None
        } else {
            Some("pending")
        };
        assert_eq!(
            started["experiment"]["actions"]["underTest"]["state"]["status"].as_str(),
            expected_status,
            "case {case:?}: expected the action's state at the start to say whether the run \
             carries it out, got {started}"
        );

        let experiment = poll_terminal(&router, &experiment_id, COMPLETION_BUDGET).await;
        let state = experiment["state"].clone();
        let reason = state["reason"].as_str().unwrap_or_default().to_string();
        if must_fail {
            assert_eq!(
                state["status"].as_str(),
                Some("failed"),
                "case {case:?}: expected the run that carries the action out to fail on \
                 privileges, got {experiment}"
            );
            assert!(
                reason.to_lowercase().contains(PRIVILEGES_ATTRIBUTION),
                "case {case:?}: expected the privileges attribution, got {reason:?}"
            );
            assert_eq!(
                state["error"]["location"].as_str(),
                Some("actions.underTest"),
                "case {case:?}: expected the error to locate the action, got {state}"
            );
        } else {
            assert_eq!(
                state["status"].as_str(),
                Some("completed"),
                "case {case:?}: expected a run that carried the action out not at all to \
                 complete, got {experiment}"
            );
            assert!(
                !reason.to_lowercase().contains(PRIVILEGES_ATTRIBUTION),
                "case {case:?}: no privilege can be lacked for an action nothing ran, \
                 got {reason:?}"
            );
            assert!(
                state["error"].is_null(),
                "case {case:?}: a run that completed reports no error, got {state}"
            );
        }
        // At the end of the run: the skipped action still reports the decision the options
        // made, the action of a run that never got going claims no progress, and the one a
        // run carried out reports that it was completed.
        let expected_final = if must_skip {
            Some("skipped")
        } else if must_fail {
            None
        } else {
            Some("completed")
        };
        assert_eq!(
            experiment["actions"]["underTest"]["state"]["status"].as_str(),
            expected_final,
            "case {case:?}: expected the action's state to survive the run, got {experiment}"
        );
    }
}

/// The experiment once it has reached a terminal status, or as it was last read when
/// `budget` ran out.
async fn poll_terminal(
    router: &MultiServiceRouter,
    experiment_id: &str,
    budget: Duration,
) -> Value {
    const INTERVAL: Duration = Duration::from_millis(100);
    let began = std::time::Instant::now();
    loop {
        let (status, read) = fis(
            router,
            "GET",
            &format!("/experiments/{experiment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "expected GetExperiment to answer for {experiment_id}, got {status}: {read}"
        );
        let experiment = read["experiment"].clone();
        let reported = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if ["completed", "failed", "stopped", "cancelled"].contains(&reported.as_str())
            || began.elapsed() >= budget
        {
            return experiment;
        }
        tokio::time::sleep(INTERVAL).await;
    }
}
