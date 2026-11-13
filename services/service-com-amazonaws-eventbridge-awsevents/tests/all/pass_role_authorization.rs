// SPDX-License-Identifier: CC0-1.0

//! HANDING this service an IAM role needs `iam:PassRole` on that role, end to end over the
//! wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter` with IAM mounted beside EventBridge on one engine — so the
//! caller's policies are read where `iam:PutUserPolicy` wrote them and the outcome is
//! asserted where a client sees it: in the refusal, and in what the configured resource
//! reports back.
//!
//! The RULE asserted here: a request that configures a resource of this service with a
//! role ARN asks EventBridge to act AS that role later, which AWS authorizes separately
//! from the call itself — "to pass a role to a service, a user must have permission to
//! pass the role to the service" — so a caller the identity-based policies in force on it
//! grant the operation but not `iam:PassRole` on the role it names is refused with
//! `AccessDeniedException` and nothing of the request is stored. Granting the permission
//! afterwards lets the identical call through with nothing else changed, which is what
//! makes the decision a fact of the policies rather than of the request.
//!
//! The rows span the class rather than the one recorded case:
//!
//! * [`every_operation_that_hands_this_service_a_role_needs_pass_role_on_it`] varies the
//!   OPERATION — every operation of this service that takes a role ARN, a rule's own role,
//!   the role of one of a rule's targets, and the role a global endpoint replicates with —
//!   and asserts the same permission decides each of them, with the resource left without
//!   the role by a refused call and carrying it after an accepted one.
//! * [`a_pass_role_statement_is_decided_against_the_role_and_the_service`] varies the
//!   STATEMENT: the resource it names (this role, a wildcard, another role), the action
//!   (the action itself, a wildcard over the IAM namespace), its effect, the condition key
//!   AWS publishes for the hand-over (`iam:PassedToService`, naming this service and
//!   naming another), a document that states nothing about the hand-over at all, and the
//!   account's own published credential, which holds no identity-based policies.
//!
//! The principal is an IAM USER presenting a key `iam:CreateAccessKey` minted for it,
//! because that is the identity a least-privilege policy is attached to; the setup calls
//! are the account's own credential, which these rows never bound.
//!
//! The harness (the router with EventBridge, IAM and the target services on one engine) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};

use crate::target_dispatch::{emulator, events, events_as, iam, xml_text, Emulator, REGION};

/// The bus every resource below lives on, and the rule the target rows write.
const BUS: &str = "pass-role-bus";
const RULE: &str = "pass-role-rule";

/// The endpoint the endpoint rows write, and the Region its pair's other half is in.
const ENDPOINT: &str = "pass-role-endpoint";
const SECONDARY_REGION: &str = "us-west-2";

/// The IAM user every row calls as, and the name of the inline policy it holds.
const USER: &str = "target-admin";
const POLICY_NAME: &str = "configure-eventbridge";

/// The role a request hands over, and a second role named only as another statement's
/// `Resource` — no call below hands that one over.
const PASSED_ROLE: &str = "events-target-role";
const OTHER_ROLE: &str = "some-other-role";

/// The IAM action a caller performs by handing a role to a service, as a statement names
/// it and as a refusal reports it.
const PASS_ROLE_ACTION: &str = "iam:PassRole";

/// The condition key AWS publishes for that action: the service the role is handed TO.
const PASSED_TO_SERVICE_KEY: &str = "iam:PassedToService";

/// This service's principal, which is the value of [`PASSED_TO_SERVICE_KEY`] for a role
/// handed to EventBridge, and another service's, which is not.
const SERVICE_PRINCIPAL: &str = "events.amazonaws.com";
const OTHER_SERVICE_PRINCIPAL: &str = "scheduler.amazonaws.com";

/// The pattern every rule below is written with.
const PATTERN: &str = r#"{"source":["com.demo.orders"]}"#;

/// The id of the one target the target rows install.
const TARGET_ID: &str = "t1";

/// The account every ARN below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the IAM role `name` of this account — the resource `iam:PassRole` is
/// performed on.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", account())
}

/// The ARN of the IAM user the rows call as, the spelling a refusal blames.
fn user_arn() -> String {
    format!("arn:aws:iam::{}:user/{USER}", account())
}

/// The ARN of the bus `name` in `region`.
fn bus_arn(region: &str, name: &str) -> String {
    format!("arn:aws:events:{region}:{}:event-bus/{name}", account())
}

/// The ARN of a Step Functions state machine — a target kind "EventBridge relies on IAM
/// roles that you specify in the `RoleARN` argument in `PutTargets`" for.
fn state_machine_arn() -> String {
    format!("arn:aws:states:{REGION}:{}:stateMachine:orders", account())
}

/// The sentence a hand-over no readable identity-based document allows is refused with.
fn no_grant_message(role: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {PASS_ROLE_ACTION} on resource: {role} \
         because no identity-based policy allows the {PASS_ROLE_ACTION} action",
        user_arn()
    )
}

/// The sentence a hand-over an identity-based document explicitly denies is refused with.
fn explicit_deny_message(role: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {PASS_ROLE_ACTION} on resource: {role} \
         with an explicit deny in an identity-based policy",
        user_arn()
    )
}

/// An identity-based document holding `statements`.
fn document(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// The statement every document below carries beside the row's own: the whole of this
/// service's actions, so the only permission a row can be missing is the hand-over.
fn events_statement() -> Value {
    json!({ "Effect": "Allow", "Action": "events:*", "Resource": "*" })
}

/// One statement about handing over a role: `effect` of `action` over `resource`, scoped by
/// `condition` when the row states one.
fn pass_role_statement(
    effect: &str,
    action: &str,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A clause over [`PASSED_TO_SERVICE_KEY`] under `operator`.
fn on_passed_to_service(operator: &str, service: &str) -> Value {
    json!({ operator: { PASSED_TO_SERVICE_KEY: service } })
}

/// Create the IAM user and answer the access key id a request calls as it with. The user
/// starts out holding `policy`.
async fn given_user(emulator: &Emulator, case: &str, policy: &Value) -> String {
    let (status, body) = iam(emulator, "CreateUser", &[("UserName", USER)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser answered {status}: {body}"
    );
    put_user_policy(emulator, case, policy).await;
    let (status, body) = iam(emulator, "CreateAccessKey", &[("UserName", USER)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccessKey answered {status}: {body}"
    );
    xml_text(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: the minted credential carries no AccessKeyId"))
}

/// Make `policy` the user's whole inline policy from now on.
async fn put_user_policy(emulator: &Emulator, case: &str, policy: &Value) {
    let (status, body) = iam(
        emulator,
        "PutUserPolicy",
        &[
            ("UserName", USER),
            ("PolicyName", POLICY_NAME),
            ("PolicyDocument", &policy.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutUserPolicy answered {status}: {body}"
    );
}

/// One EventBridge call the account's own credential makes, which must be served.
async fn served(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = events(emulator, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} by the account's own credential answered {status}: \
         {answered}"
    );
    answered
}

/// The operations of this service that take a role ARN, which is what handing one over
/// means here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handover {
    /// `PutRule$RoleArn` — "the Amazon Resource Name (ARN) of the IAM role associated with
    /// the rule".
    PutRule,
    /// `Target$RoleArn` — the role EventBridge assumes to invoke that target.
    PutTargets,
    /// `CreateEndpoint$RoleArn` — "the ARN of the role used for replication".
    CreateEndpoint,
    /// `UpdateEndpoint$RoleArn` — the same role, written by the other writer of it.
    UpdateEndpoint,
}

impl Handover {
    /// The operation's own name, as the wire names it.
    fn operation(self) -> &'static str {
        match self {
            Self::PutRule => "PutRule",
            Self::PutTargets => "PutTargets",
            Self::CreateEndpoint => "CreateEndpoint",
            Self::UpdateEndpoint => "UpdateEndpoint",
        }
    }

    /// The request that hands `role` over.
    fn request(self, role: &str) -> Value {
        match self {
            Self::PutRule => json!({
                "Name": RULE,
                "EventBusName": BUS,
                "EventPattern": PATTERN,
                "RoleArn": role,
            }),
            Self::PutTargets => json!({
                "Rule": RULE,
                "EventBusName": BUS,
                "Targets": [{
                    "Id": TARGET_ID,
                    "Arn": state_machine_arn(),
                    "RoleArn": role,
                }],
            }),
            Self::CreateEndpoint => endpoint_request(Some(role), "ENABLED"),
            Self::UpdateEndpoint => json!({ "Name": ENDPOINT, "RoleArn": role }),
        }
    }
}

/// A `CreateEndpoint` request: the pair of buses is ONE name in TWO Regions and the
/// failover policy routes to the other one, which is what the operation requires of it.
fn endpoint_request(role: Option<&str>, replication: &str) -> Value {
    let mut request = json!({
        "Name": ENDPOINT,
        "RoutingConfig": {
            "FailoverConfig": {
                "Primary": { "HealthCheck": "arn:aws:route53:::healthcheck/abcdef0123456789" },
                "Secondary": { "Route": SECONDARY_REGION },
            },
        },
        "EventBuses": [
            { "EventBusArn": bus_arn(REGION, BUS) },
            { "EventBusArn": bus_arn(SECONDARY_REGION, BUS) },
        ],
        "ReplicationConfig": { "State": replication },
    });
    if let Some(role) = role {
        request["RoleArn"] = json!(role);
    }
    request
}

/// Everything a row's operation needs to exist before it is called, written by the
/// account's own credential.
async fn given_resources(emulator: &Emulator, case: &str, handover: Handover) {
    served(emulator, case, "CreateEventBus", &json!({ "Name": BUS })).await;
    if handover == Handover::PutRule || handover == Handover::PutTargets {
        // The rule exists already and carries NO role, so a refused `PutRule` leaves a
        // rule to read the absence of one off.
        served(
            emulator,
            case,
            "PutRule",
            &json!({ "Name": RULE, "EventBusName": BUS, "EventPattern": PATTERN }),
        )
        .await;
    }
    if handover == Handover::UpdateEndpoint {
        // An endpoint replicating nothing needs no role, so this one starts without one.
        served(
            emulator,
            case,
            "CreateEndpoint",
            &endpoint_request(None, "DISABLED"),
        )
        .await;
    }
}

/// The role the resource the row configures now names, or `None` when it names none —
/// which includes a resource the refused call never created.
async fn stored_role(emulator: &Emulator, handover: Handover) -> Option<String> {
    let (operation, request) = match handover {
        Handover::PutRule => (
            "DescribeRule",
            json!({ "Name": RULE, "EventBusName": BUS }),
        ),
        Handover::PutTargets => (
            "ListTargetsByRule",
            json!({ "Rule": RULE, "EventBusName": BUS }),
        ),
        Handover::CreateEndpoint | Handover::UpdateEndpoint => {
            ("DescribeEndpoint", json!({ "Name": ENDPOINT }))
        }
    };
    let (status, body) = events(emulator, operation, &request).await;
    if status != 200 {
        return None;
    }
    let reported = match handover {
        Handover::PutTargets => &body["Targets"][0]["RoleArn"],
        _ => &body["RoleArn"],
    };
    reported.as_str().map(str::to_string)
}

/// Every operation of this service that takes a role ARN is refused while the caller holds
/// no `iam:PassRole` on that role, and serves the identical call once it does.
#[tokio::test]
async fn every_operation_that_hands_this_service_a_role_needs_pass_role_on_it() {
    let handovers = [
        Handover::PutRule,
        Handover::PutTargets,
        Handover::CreateEndpoint,
        Handover::UpdateEndpoint,
    ];
    let passed = role_arn(PASSED_ROLE);

    for handover in handovers {
        let case = handover.operation();
        let emulator = emulator();
        given_resources(&emulator, case, handover).await;
        // The caller may do everything of this service's own and nothing of IAM's.
        let key = given_user(&emulator, case, &document(vec![events_statement()])).await;

        let (status, refused) =
            events_as(&emulator, &key, handover.operation(), &handover.request(&passed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: handing over a role with no iam:PassRole must be refused, got \
             {status}: {refused}"
        );
        assert_eq!(
            refused["__type"],
            json!("AccessDeniedException"),
            "case {case:?}: expected AccessDeniedException, got {refused}"
        );
        assert_eq!(
            refused["message"],
            json!(no_grant_message(&passed)),
            "case {case:?}: expected the refusal to name the action and the role, got {refused}"
        );
        let stored = stored_role(&emulator, handover).await;
        assert!(
            stored.is_none(),
            "case {case:?}: a refused hand-over must store no role, got {stored:?}"
        );

        put_user_policy(
            &emulator,
            case,
            &document(vec![
                events_statement(),
                pass_role_statement("Allow", PASS_ROLE_ACTION, &passed, None),
            ]),
        )
        .await;
        let (status, served) =
            events_as(&emulator, &key, handover.operation(), &handover.request(&passed)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the identical call must be served once iam:PassRole is granted, \
             got {status}: {served}"
        );
        let stored = stored_role(&emulator, handover).await;
        assert_eq!(
            stored.as_deref(),
            Some(passed.as_str()),
            "case {case:?}: the configured resource must name the role handed over, got \
             {stored:?}"
        );
    }
}

/// Who a row of the statement table calls as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caller {
    /// An IAM user holding the row's document.
    User,
    /// The account's own published credential, which holds no identity-based policies.
    AccountCredential,
}

/// One row of the statement table.
struct Row {
    case: &'static str,
    caller: Caller,
    /// The statement the row's document carries beside [`events_statement`], or `None` for
    /// a document that states nothing about the hand-over.
    statement: Option<Value>,
    /// The refusal the hand-over earns, or `None` when it is served.
    refusal: Option<Refusal>,
}

/// Which rule of IAM's order refused the hand-over, which is what the refusal's closing
/// clause names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Nothing readable allowed it.
    NoGrant,
    /// An explicit deny in the caller's identity-based policy.
    ExplicitDeny,
}

/// The hand-over of one role to this service is decided by the statements in force, over a
/// table of statement shapes.
#[tokio::test]
async fn a_pass_role_statement_is_decided_against_the_role_and_the_service() {
    let passed = role_arn(PASSED_ROLE);
    let other = role_arn(OTHER_ROLE);
    let rows = vec![
        Row {
            case: "a document stating nothing about the hand-over",
            caller: Caller::User,
            statement: None,
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an allow naming the role itself",
            caller: Caller::User,
            statement: Some(pass_role_statement("Allow", PASS_ROLE_ACTION, &passed, None)),
            refusal: None,
        },
        Row {
            case: "an allow over every resource",
            caller: Caller::User,
            statement: Some(pass_role_statement("Allow", PASS_ROLE_ACTION, "*", None)),
            refusal: None,
        },
        Row {
            case: "an allow naming another role",
            caller: Caller::User,
            statement: Some(pass_role_statement("Allow", PASS_ROLE_ACTION, &other, None)),
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an allow over a wildcard of the IAM namespace",
            caller: Caller::User,
            statement: Some(pass_role_statement("Allow", "iam:*", &passed, None)),
            refusal: None,
        },
        Row {
            case: "an explicit deny naming the role",
            caller: Caller::User,
            statement: Some(pass_role_statement("Deny", PASS_ROLE_ACTION, &passed, None)),
            refusal: Some(Refusal::ExplicitDeny),
        },
        Row {
            case: "an allow scoped to this service by iam:PassedToService",
            caller: Caller::User,
            statement: Some(pass_role_statement(
                "Allow",
                PASS_ROLE_ACTION,
                &passed,
                Some(on_passed_to_service("StringEquals", SERVICE_PRINCIPAL)),
            )),
            refusal: None,
        },
        Row {
            case: "an allow scoped to another service by iam:PassedToService",
            caller: Caller::User,
            statement: Some(pass_role_statement(
                "Allow",
                PASS_ROLE_ACTION,
                &passed,
                Some(on_passed_to_service("StringEquals", OTHER_SERVICE_PRINCIPAL)),
            )),
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an allow excluding this service by iam:PassedToService",
            caller: Caller::User,
            statement: Some(pass_role_statement(
                "Allow",
                PASS_ROLE_ACTION,
                &passed,
                Some(on_passed_to_service("StringNotEquals", SERVICE_PRINCIPAL)),
            )),
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "the account's own published credential, which holds no documents",
            caller: Caller::AccountCredential,
            statement: None,
            refusal: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = emulator();
        given_resources(&emulator, case, Handover::PutTargets).await;
        let mut statements = vec![events_statement()];
        statements.extend(row.statement.clone());
        let key = given_user(&emulator, case, &document(statements)).await;
        let calling = match row.caller {
            Caller::User => key,
            // The account holds no record of this key, so it resolves to its own published
            // credential — the one every setup call above presented.
            Caller::AccountCredential => crate::target_dispatch::AMBIENT_KEY.to_string(),
        };

        let (status, answered) = events_as(
            &emulator,
            &calling,
            "PutTargets",
            &Handover::PutTargets.request(&passed),
        )
        .await;
        let stored = stored_role(&emulator, Handover::PutTargets).await;
        match row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the hand-over must be served, got {status}: {answered}"
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(passed.as_str()),
                    "case {case:?}: the installed target must name the role, got {stored:?}"
                );
            }
            Some(refusal) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the hand-over must be refused, got {status}: {answered}"
                );
                assert_eq!(
                    answered["__type"],
                    json!("AccessDeniedException"),
                    "case {case:?}: expected AccessDeniedException, got {answered}"
                );
                let expected = match refusal {
                    Refusal::NoGrant => no_grant_message(&passed),
                    Refusal::ExplicitDeny => explicit_deny_message(&passed),
                };
                assert_eq!(
                    answered["message"],
                    json!(expected),
                    "case {case:?}: expected the message {expected:?}, got {answered}"
                );
                assert!(
                    stored.is_none(),
                    "case {case:?}: a refused hand-over must install no target, got {stored:?}"
                );
            }
        }
    }
}
