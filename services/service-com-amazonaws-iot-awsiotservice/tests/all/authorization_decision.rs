// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `TestAuthorization`: the authorization decision for one
//! principal, computed from the persisted policy-attachment relation and the documents of
//! the policies in effect.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: one `authInfo` is one question and one `authResults` entry is its
//! answer, in the order asked; a statement of a policy in effect bears on a question only
//! when it names the action asked about AND matches the resource ARN asked about, where
//! `*` stands for any run of characters and `?` for one; an explicit `Deny` that bears on
//! the question overrides every `Allow` (`EXPLICIT_DENY`), an `Allow` with no such `Deny`
//! decides it `ALLOWED`, and a question no statement bears on is `IMPLICIT_DENY` with no
//! policy under `allowed.policies`. Deciding changes nothing, so the same question
//! answers the same way however often it is asked.
//!
//! The tables span the class the rule generalises over rather than one recorded request:
//! all four `ActionType` values, the resource namespaces each is granted in
//! (`topicfilter/`, `topic/`, `client/`), exact and wildcard resource patterns, exact and
//! wildcard action patterns, a grant of one action asked about another, several resources
//! in one question, several policies in effect at once, the `policyNamesToAdd` /
//! `policyNamesToSkip` overrides, the modelled 1..10 bound on `authInfos` and one past it,
//! a policy variable with and without the context it needs, and each principal form.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name and
/// the region: the scope is how the router picks the claiming service. The signature
/// itself is never checked here — the harness turns request authentication off, exactly as
/// an unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// Every `authResults` entry, in the order the answer reported them.
    fn decisions(&self) -> Vec<Decision> {
        self.body
            .get("authResults")
            .and_then(Value::as_array)
            .map(|entries| entries.iter().map(Decision::read).collect())
            .unwrap_or_default()
    }

    /// The single decision an answer to one question carries.
    fn decision(&self) -> Decision {
        let mut decisions = self.decisions();
        assert_eq!(
            decisions.len(),
            1,
            "one authInfo must answer with exactly one authResult, got {:?}",
            self.body
        );
        decisions.remove(0)
    }
}

/// One `AuthResult`, reduced to what a client branches on.
#[derive(Debug)]
struct Decision {
    /// `authDecision`.
    decision: String,
    /// The `policyName` of every entry of `allowed.policies`.
    allowed: Vec<String>,
    /// The `policyName` of every entry of `denied.implicitDeny.policies`.
    implicit_deny: Vec<String>,
    /// The `policyName` of every entry of `denied.explicitDeny.policies`.
    explicit_deny: Vec<String>,
    /// `missingContextValues`.
    missing_context_values: Vec<String>,
    /// `authInfo`, the question this result answers.
    auth_info: Value,
    /// The `policyArn` of every entry of `allowed.policies`.
    allowed_arns: Vec<String>,
}

impl Decision {
    fn read(entry: &Value) -> Self {
        let names = |path: &[&str]| -> Vec<String> { policy_member(entry, path, "policyName") };
        Self {
            decision: entry
                .get("authDecision")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            allowed: names(&["allowed"]),
            implicit_deny: names(&["denied", "implicitDeny"]),
            explicit_deny: names(&["denied", "explicitDeny"]),
            missing_context_values: entry
                .get("missingContextValues")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            auth_info: entry.get("authInfo").cloned().unwrap_or(Value::Null),
            allowed_arns: policy_member(entry, &["allowed"], "policyArn"),
        }
    }
}

/// One member of every `Policy` under `path`'s `policies` list, in order.
fn policy_member(entry: &Value, path: &[&str], member: &str) -> Vec<String> {
    let mut current = entry;
    for step in path {
        match current.get(step) {
            Some(next) => current = next,
            None => return Vec::new(),
        }
    }
    current
        .get("policies")
        .and_then(Value::as_array)
        .map(|policies| {
            policies
                .iter()
                .filter_map(|policy| policy.get(member).and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreatePolicy`: `POST /policies/{policyName}`.
    async fn create_policy(&self, policy_name: &str, document: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(&json!({ "policyDocument": document.to_string() })),
        )
        .await
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, which issues the
    /// certificate a policy is attached to.
    async fn create_certificate(&self) -> Reply {
        self.call("POST", "/keys-and-certificate?setAsActive=true", None)
            .await
    }

    /// `CreateThingGroup`: `POST /thing-groups/{thingGroupName}`.
    async fn create_thing_group(&self, name: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/thing-groups/{name}"), Some(request))
            .await
    }

    /// `AttachPolicy`: `PUT /target-policies/{policyName}` carrying the target.
    async fn attach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
        )
        .await
    }

    /// `ListAttachedPolicies`: `POST /attached-policies/{target}`, whose target is ONE URI
    /// path segment. Read here only to prove `TestAuthorization` left the relation alone.
    async fn list_attached_policies(&self, target: &str) -> Reply {
        let segment = target
            .replace('%', "%25")
            .replace(':', "%3A")
            .replace('/', "%2F");
        self.call("POST", &format!("/attached-policies/{segment}"), None)
            .await
    }

    /// `TestAuthorization`: `POST /test-authorization`, whose `clientId` is `@httpQuery`.
    async fn test_authorization(&self, request: &Value, client_id: Option<&str>) -> Reply {
        let path = match client_id {
            Some(client_id) => format!("/test-authorization?clientId={client_id}"),
            None => "/test-authorization".to_string(),
        };
        self.call("POST", &path, Some(request)).await
    }

    /// One question about `principal`, the shape most cases need.
    async fn ask(&self, principal: &str, action_type: &str, resources: &[String]) -> Reply {
        self.test_authorization(
            &json!({
                "principal": principal,
                "authInfos": [{ "actionType": action_type, "resources": resources }],
            }),
            None,
        )
        .await
    }
}

/// An IoT policy document allowing or denying `action` on `resource`.
fn statement(effect: &str, action: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": resource }],
    })
}

/// The ARN of an MQTT topic filter, the namespace a subscription is authorized against.
fn topic_filter_arn(filter: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:topicfilter/{filter}")
}

/// The ARN of an MQTT topic, the namespace a publication or a delivery is authorized
/// against.
fn topic_arn(topic: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:topic/{topic}")
}

/// The ARN of an MQTT client id, the namespace a connection is authorized against.
fn client_arn(client_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:client/{client_id}")
}

/// The ARN a policy created in this suite's region carries.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:policy/{policy_name}")
}

/// Create `policy_name` with `document`, asserting the create succeeded.
async fn given_policy(emulator: &Emulator, policy_name: &str, document: &Value) {
    let created = emulator.create_policy(policy_name, document).await;
    assert_eq!(
        created.status, 200,
        "case {policy_name:?}: CreatePolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
}

/// Issue a certificate, answering its `certificateArn`.
async fn given_certificate(emulator: &Emulator) -> String {
    let issued = emulator.create_certificate().await;
    assert_eq!(
        issued.status, 200,
        "CreateKeysAndCertificate must answer 200, got {} {:?}",
        issued.status, issued.body
    );
    issued
        .text("certificateArn")
        .expect("an issued certificate carries a certificateArn")
}

/// Attach `policy_name` to `target`, asserting the attach succeeded.
async fn given_attachment(emulator: &Emulator, policy_name: &str, target: &str) {
    let attached = emulator.attach_policy(policy_name, target).await;
    assert_eq!(
        attached.status, 200,
        "case {policy_name:?}->{target:?}: AttachPolicy must answer 200, got {} {:?}",
        attached.status, attached.body
    );
}

/// Create `policy_name` with `document` and attach it to `target`.
async fn given_attached_policy(
    emulator: &Emulator,
    policy_name: &str,
    document: &Value,
    target: &str,
) {
    given_policy(emulator, policy_name, document).await;
    given_attachment(emulator, policy_name, target).await;
}

/// One row of the decision table: a question, and the decision the rule gives it.
struct Question {
    /// What the row is called in an assertion.
    label: &'static str,
    /// `AuthInfo$actionType`.
    action_type: &'static str,
    /// `AuthInfo$resources`.
    resources: Vec<String>,
    /// The `authDecision` the rule gives it.
    decision: &'static str,
    /// The `policyName` of every policy that must appear under `allowed.policies`.
    allowed: Vec<&'static str>,
}

/// Ask every row of a table about one principal and assert its decision.
async fn assert_decisions(emulator: &Emulator, principal: &str, rows: &[Question]) {
    for row in rows {
        let answer = emulator
            .ask(principal, row.action_type, &row.resources)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {}: TestAuthorization must answer 200, got {} {:?}",
            row.label, answer.status, answer.body
        );
        let decided = answer.decision();
        assert_eq!(
            decided.decision, row.decision,
            "case {}: expected authDecision {:?}, got {:?} ({:?})",
            row.label, row.decision, decided.decision, answer.body
        );
        assert_eq!(
            decided.allowed, row.allowed,
            "case {}: expected allowed.policies {:?}, got {:?} ({:?})",
            row.label, row.allowed, decided.allowed, answer.body
        );
        if row.decision != "ALLOWED" {
            assert!(
                decided.allowed.is_empty(),
                "case {}: a denied decision reports no allowing policy, got {:?}",
                row.label,
                decided.allowed
            );
        }
        assert_eq!(
            decided.auth_info,
            json!({ "actionType": row.action_type, "resources": row.resources }),
            "case {}: the result must echo the authInfo it answers, got {:?}",
            row.label,
            decided.auth_info
        );
    }
}

/// A grant bears on a question only when it names the action asked about and matches the
/// resource ARN asked about, so the `topicfilter/` and `topic/` namespaces and the action
/// names decide on their own.
#[tokio::test]
async fn a_grant_decides_only_the_action_and_the_resource_it_names() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "subscribe-home-kitchen",
        &statement(
            "Allow",
            json!("iot:Subscribe"),
            json!(topic_filter_arn("home/kitchen")),
        ),
        &certificate,
    )
    .await;

    // The namespace each action is authorized against: a subscription against
    // `topicfilter/`, a publication and a delivery against `topic/`, a connection against
    // `client/` — the resource types of AWS IoT Core's policy actions.
    let rows = vec![
        Question {
            label: "SUBSCRIBE on the granted topicfilter/ ARN",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("home/kitchen")],
            decision: "ALLOWED",
            allowed: vec!["subscribe-home-kitchen"],
        },
        Question {
            label: "SUBSCRIBE on the topic/ ARN of the same path",
            action_type: "SUBSCRIBE",
            resources: vec![topic_arn("home/kitchen")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "PUBLISH on the topic/ ARN of the same path",
            action_type: "PUBLISH",
            resources: vec![topic_arn("home/kitchen")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "PUBLISH on the granted topicfilter/ ARN",
            action_type: "PUBLISH",
            resources: vec![topic_filter_arn("home/kitchen")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "RECEIVE on the topic/ ARN of the same path",
            action_type: "RECEIVE",
            resources: vec![topic_arn("home/kitchen")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "CONNECT on a client/ ARN",
            action_type: "CONNECT",
            resources: vec![client_arn("kitchen-lamp")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "SUBSCRIBE on a topicfilter/ ARN of another path",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("home/garage")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "SUBSCRIBE on the granted path repeated after the denials",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("home/kitchen")],
            decision: "ALLOWED",
            allowed: vec!["subscribe-home-kitchen"],
        },
    ];

    assert_decisions(&emulator, &certificate, &rows).await;

    // A decision is a READ: the relation it was computed from is the relation it leaves.
    let attached = emulator.list_attached_policies(&certificate).await;
    assert_eq!(
        attached.status, 200,
        "ListAttachedPolicies must answer 200 after the decisions, got {} {:?}",
        attached.status, attached.body
    );
    let still_attached = policy_member(&attached.body, &[], "policyName");
    assert_eq!(
        still_attached,
        vec!["subscribe-home-kitchen".to_string()],
        "the attachment relation must be unchanged by a decision, got {:?}",
        attached.body
    );
}

/// Every `ActionType` is decided against the grant of its own action, and a grant of one
/// action never confers another.
#[tokio::test]
async fn each_action_type_is_decided_against_the_grant_of_that_action() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "device.grants",
        &json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": "iot:Connect",
                    "Resource": client_arn("sensor-7"),
                },
                {
                    "Effect": "Allow",
                    "Action": ["iot:Publish", "iot:Receive"],
                    "Resource": topic_arn("sensor/7/telemetry"),
                },
            ],
        }),
        &certificate,
    )
    .await;

    let rows = vec![
        Question {
            label: "CONNECT on the granted client/ ARN",
            action_type: "CONNECT",
            resources: vec![client_arn("sensor-7")],
            decision: "ALLOWED",
            allowed: vec!["device.grants"],
        },
        Question {
            label: "CONNECT on another client/ ARN",
            action_type: "CONNECT",
            resources: vec![client_arn("sensor-8")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "PUBLISH on the granted topic/ ARN",
            action_type: "PUBLISH",
            resources: vec![topic_arn("sensor/7/telemetry")],
            decision: "ALLOWED",
            allowed: vec!["device.grants"],
        },
        Question {
            label: "RECEIVE on the granted topic/ ARN",
            action_type: "RECEIVE",
            resources: vec![topic_arn("sensor/7/telemetry")],
            decision: "ALLOWED",
            allowed: vec!["device.grants"],
        },
        Question {
            label: "SUBSCRIBE on the topicfilter/ of the granted topic",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("sensor/7/telemetry")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "PUBLISH on two granted and ungranted resources together",
            action_type: "PUBLISH",
            resources: vec![
                topic_arn("sensor/7/telemetry"),
                topic_arn("sensor/8/telemetry"),
            ],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "PUBLISH on the granted resource named twice",
            action_type: "PUBLISH",
            resources: vec![
                topic_arn("sensor/7/telemetry"),
                topic_arn("sensor/7/telemetry"),
            ],
            decision: "ALLOWED",
            allowed: vec!["device.grants"],
        },
    ];

    assert_decisions(&emulator, &certificate, &rows).await;
}

/// A wildcard in a statement stands for a run of characters (`*`) or exactly one (`?`), in
/// the action as in the resource; the MQTT wildcards are literal text.
#[tokio::test]
async fn wildcards_in_a_statement_are_the_iam_wildcards() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "wildcards",
        &json!({
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": "iot:Sub*",
                    "Resource": topic_filter_arn("home/*"),
                },
                {
                    "Effect": "Allow",
                    "Action": "iot:*",
                    "Resource": topic_arn("floor/?/status"),
                },
                {
                    "Effect": "Allow",
                    "Action": "iot:Subscribe",
                    "Resource": topic_filter_arn("literal/+"),
                },
            ],
        }),
        &certificate,
    )
    .await;

    let rows = vec![
        Question {
            label: "a run wildcard covers a deeper path",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("home/kitchen/lamp")],
            decision: "ALLOWED",
            allowed: vec!["wildcards"],
        },
        Question {
            label: "a run wildcard does not cover another prefix",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("office/kitchen")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "an action wildcard covers the actions it spells",
            action_type: "PUBLISH",
            resources: vec![topic_arn("floor/3/status")],
            decision: "ALLOWED",
            allowed: vec!["wildcards"],
        },
        Question {
            label: "a single-character wildcard covers exactly one character",
            action_type: "PUBLISH",
            resources: vec![topic_arn("floor/33/status")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "an action wildcard does not widen the resource",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("floor/3/status")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
        Question {
            label: "the MQTT single-level wildcard is literal text",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("literal/+")],
            decision: "ALLOWED",
            allowed: vec!["wildcards"],
        },
        Question {
            label: "the MQTT single-level wildcard covers no other level",
            action_type: "SUBSCRIBE",
            resources: vec![topic_filter_arn("literal/anything")],
            decision: "IMPLICIT_DENY",
            allowed: Vec::new(),
        },
    ];

    assert_decisions(&emulator, &certificate, &rows).await;
}

/// An explicit `Deny` that bears on the question overrides every `Allow`, and names the
/// policy that denied it.
#[tokio::test]
async fn an_explicit_deny_overrides_every_allow() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "allow-every-topic",
        &statement("Allow", json!("iot:Publish"), json!(topic_arn("*"))),
        &certificate,
    )
    .await;
    given_attached_policy(
        &emulator,
        "deny-secrets",
        &statement(
            "Deny",
            json!("iot:Publish"),
            json!(topic_arn("secret/keys")),
        ),
        &certificate,
    )
    .await;

    let open = emulator
        .ask(&certificate, "PUBLISH", &[topic_arn("open/news")])
        .await;
    let decided = open.decision();
    assert_eq!(
        decided.decision, "ALLOWED",
        "case a topic no Deny names: expected ALLOWED, got {:?} ({:?})",
        decided.decision, open.body
    );
    assert_eq!(
        decided.allowed,
        vec!["allow-every-topic".to_string()],
        "case a topic no Deny names: expected the allowing policy, got {:?}",
        decided.allowed
    );
    assert_eq!(
        decided.allowed_arns,
        vec![policy_arn("allow-every-topic")],
        "case a topic no Deny names: an allowing policy carries the ARN it was created \
         with, got {:?}",
        decided.allowed_arns
    );

    let denied = emulator
        .ask(&certificate, "PUBLISH", &[topic_arn("secret/keys")])
        .await;
    let decided = denied.decision();
    assert_eq!(
        decided.decision, "EXPLICIT_DENY",
        "case the denied topic: expected EXPLICIT_DENY, got {:?} ({:?})",
        decided.decision, denied.body
    );
    assert_eq!(
        decided.explicit_deny,
        vec!["deny-secrets".to_string()],
        "case the denied topic: expected the denying policy under explicitDeny, got {:?}",
        decided.explicit_deny
    );
    assert!(
        decided.allowed.is_empty(),
        "case the denied topic: nothing allowed the authorization, got {:?}",
        decided.allowed
    );

    // One of the two resources denied denies the question as a whole, because one result
    // answers the whole `authInfo`.
    let together = emulator
        .ask(
            &certificate,
            "PUBLISH",
            &[topic_arn("open/news"), topic_arn("secret/keys")],
        )
        .await;
    let decided = together.decision();
    assert_eq!(
        decided.decision, "EXPLICIT_DENY",
        "case both topics in one question: expected EXPLICIT_DENY, got {:?} ({:?})",
        decided.decision, together.body
    );
}

/// A question no statement in effect bears on reports, under `implicitDeny`, the policies
/// that were in effect and bore on nothing.
#[tokio::test]
async fn an_implicit_deny_names_the_policies_that_bore_on_nothing() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "connect-only",
        &statement("Allow", json!("iot:Connect"), json!(client_arn("robot-1"))),
        &certificate,
    )
    .await;
    given_attached_policy(
        &emulator,
        "subscribe-only",
        &statement(
            "Allow",
            json!("iot:Subscribe"),
            json!(topic_filter_arn("robot/1/commands")),
        ),
        &certificate,
    )
    .await;

    let answer = emulator
        .ask(&certificate, "PUBLISH", &[topic_arn("robot/1/telemetry")])
        .await;
    let decided = answer.decision();
    assert_eq!(
        decided.decision, "IMPLICIT_DENY",
        "case an action neither policy grants: expected IMPLICIT_DENY, got {:?} ({:?})",
        decided.decision, answer.body
    );
    assert_eq!(
        decided.implicit_deny,
        vec!["connect-only".to_string(), "subscribe-only".to_string()],
        "case an action neither policy grants: implicitDeny must name both policies in \
         effect, got {:?}",
        decided.implicit_deny
    );
    assert!(
        decided.explicit_deny.is_empty(),
        "case an action neither policy grants: nothing explicitly denied it, got {:?}",
        decided.explicit_deny
    );

    // A principal with no policy at all is denied with nothing to report.
    let bare = given_certificate(&emulator).await;
    let answer = emulator
        .ask(&bare, "CONNECT", &[client_arn("robot-1")])
        .await;
    let decided = answer.decision();
    assert_eq!(
        decided.decision, "IMPLICIT_DENY",
        "case a principal with no attachment: expected IMPLICIT_DENY, got {:?} ({:?})",
        decided.decision, answer.body
    );
    assert!(
        decided.implicit_deny.is_empty() && decided.allowed.is_empty(),
        "case a principal with no attachment: no policy is reported at all, got {:?}",
        answer.body
    );
}

/// `policyNamesToAdd` and `policyNamesToSkip` decide the question against a policy set the
/// relation does not hold, and leave the relation where it was.
#[tokio::test]
async fn the_policy_name_overrides_change_the_set_in_effect_only() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;
    let resource = topic_filter_arn("fleet/updates");

    given_attached_policy(
        &emulator,
        "attached-subscribe",
        &statement("Allow", json!("iot:Subscribe"), json!(resource.clone())),
        &certificate,
    )
    .await;
    // Created but attached to nothing: it decides a question only when the request treats
    // it as attached.
    given_policy(
        &emulator,
        "detached-subscribe",
        &statement("Allow", json!("iot:Subscribe"), json!(resource.clone())),
    )
    .await;

    let question = json!([{ "actionType": "SUBSCRIBE", "resources": [resource.clone()] }]);
    let cases: Vec<(&str, Value, &str, Vec<String>)> = vec![
        (
            "the attached policy alone",
            json!({ "principal": certificate, "authInfos": question }),
            "ALLOWED",
            vec!["attached-subscribe".to_string()],
        ),
        (
            "the attached policy skipped",
            json!({
                "principal": certificate,
                "authInfos": question,
                "policyNamesToSkip": ["attached-subscribe"],
            }),
            "IMPLICIT_DENY",
            Vec::new(),
        ),
        (
            "the detached policy added",
            json!({
                "principal": certificate,
                "authInfos": question,
                "policyNamesToAdd": ["detached-subscribe"],
            }),
            "ALLOWED",
            vec![
                "attached-subscribe".to_string(),
                "detached-subscribe".to_string(),
            ],
        ),
        (
            "the detached policy added and the attached one skipped",
            json!({
                "principal": certificate,
                "authInfos": question,
                "policyNamesToAdd": ["detached-subscribe"],
                "policyNamesToSkip": ["attached-subscribe"],
            }),
            "ALLOWED",
            vec!["detached-subscribe".to_string()],
        ),
        (
            "no principal at all, deciding on the added policy only",
            json!({
                "authInfos": question,
                "policyNamesToAdd": ["detached-subscribe"],
            }),
            "ALLOWED",
            vec!["detached-subscribe".to_string()],
        ),
    ];

    for (label, request, expected, allowed) in &cases {
        let answer = emulator.test_authorization(request, None).await;
        assert_eq!(
            answer.status, 200,
            "case {label}: TestAuthorization must answer 200, got {} {:?}",
            answer.status, answer.body
        );
        let decided = answer.decision();
        assert_eq!(
            decided.decision, *expected,
            "case {label}: expected authDecision {expected:?}, got {:?} ({:?})",
            decided.decision, answer.body
        );
        assert_eq!(
            decided.allowed, *allowed,
            "case {label}: expected allowed.policies {allowed:?}, got {:?}",
            decided.allowed
        );
    }

    // Neither override is a write: the relation after the five calls is the one before.
    let attached = emulator.list_attached_policies(&certificate).await;
    assert_eq!(
        policy_member(&attached.body, &[], "policyName"),
        vec!["attached-subscribe".to_string()],
        "the overrides must leave the attachment relation alone, got {:?}",
        attached.body
    );
}

/// A statement written in terms of a value the question does not carry decides nothing and
/// reports the context key it needed.
#[tokio::test]
async fn a_statement_needing_absent_context_reports_the_missing_key() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "own-client-topic",
        &statement(
            "Allow",
            json!("iot:Publish"),
            json!(topic_arn("devices/${iot:ClientId}/telemetry")),
        ),
        &certificate,
    )
    .await;
    given_attached_policy(
        &emulator,
        "conditional-subscribe",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "iot:Subscribe",
                "Resource": topic_filter_arn("devices/shared"),
                "Condition": {
                    "Bool": { "iot:Connection.Thing.IsAttached": ["true"] },
                },
            }],
        }),
        &certificate,
    )
    .await;

    let with_client_id = emulator
        .test_authorization(
            &json!({
                "principal": certificate,
                "authInfos": [{
                    "actionType": "PUBLISH",
                    "resources": [topic_arn("devices/lamp-9/telemetry")],
                }],
            }),
            Some("lamp-9"),
        )
        .await;
    let decided = with_client_id.decision();
    assert_eq!(
        decided.decision, "ALLOWED",
        "case the client id the variable names: expected ALLOWED, got {:?} ({:?})",
        decided.decision, with_client_id.body
    );
    assert!(
        decided.missing_context_values.is_empty(),
        "case the client id the variable names: nothing was missing, got {:?}",
        decided.missing_context_values
    );

    let other_client_id = emulator
        .test_authorization(
            &json!({
                "principal": certificate,
                "authInfos": [{
                    "actionType": "PUBLISH",
                    "resources": [topic_arn("devices/lamp-9/telemetry")],
                }],
            }),
            Some("lamp-8"),
        )
        .await;
    let decided = other_client_id.decision();
    assert_eq!(
        decided.decision, "IMPLICIT_DENY",
        "case another client's topic: expected IMPLICIT_DENY, got {:?} ({:?})",
        decided.decision, other_client_id.body
    );

    let without_client_id = emulator
        .ask(
            &certificate,
            "PUBLISH",
            &[topic_arn("devices/lamp-9/telemetry")],
        )
        .await;
    let decided = without_client_id.decision();
    assert_eq!(
        decided.decision, "IMPLICIT_DENY",
        "case no client id at all: expected IMPLICIT_DENY, got {:?} ({:?})",
        decided.decision, without_client_id.body
    );
    assert!(
        decided.allowed.is_empty(),
        "case no client id at all: a resource pattern whose variable has no value grants \
         nothing, got {:?}",
        decided.allowed
    );

    let conditional = emulator
        .ask(
            &certificate,
            "SUBSCRIBE",
            &[topic_filter_arn("devices/shared")],
        )
        .await;
    let decided = conditional.decision();
    assert_eq!(
        decided.decision, "IMPLICIT_DENY",
        "case a condition nothing supplies values for: expected IMPLICIT_DENY, got {:?} \
         ({:?})",
        decided.decision, conditional.body
    );
    assert!(
        decided
            .missing_context_values
            .contains(&"iot:Connection.Thing.IsAttached".to_string()),
        "case a condition nothing supplies values for: the condition's key must be \
         reported, got {:?}",
        decided.missing_context_values
    );
}

/// Each principal form the relation is keyed by decides its own questions, and a thing
/// group's decision includes the policies attached above it.
#[tokio::test]
async fn every_principal_form_is_decided_from_its_own_attachments() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;
    let cognito_identity = format!("{REGION}:11112222-3333-4444-5555-666677778888");
    let resource = topic_filter_arn("shared/feed");

    let parent = emulator.create_thing_group("plant", &json!({})).await;
    assert_eq!(
        parent.status, 200,
        "CreateThingGroup must answer 200, got {} {:?}",
        parent.status, parent.body
    );
    let child = emulator
        .create_thing_group("floor-2", &json!({ "parentGroupName": "plant" }))
        .await;
    assert_eq!(
        child.status, 200,
        "CreateThingGroup must answer 200 for a child group, got {} {:?}",
        child.status, child.body
    );
    let child_arn = child
        .text("thingGroupArn")
        .expect("a created thing group carries a thingGroupArn");

    let subscribe = statement("Allow", json!("iot:Subscribe"), json!(resource.clone()));
    given_attached_policy(&emulator, "cert-subscribe", &subscribe, &certificate).await;
    given_attached_policy(
        &emulator,
        "cognito-subscribe",
        &subscribe,
        &cognito_identity,
    )
    .await;
    // Attached to the PARENT group: it governs the group beneath it.
    given_attached_policy(
        &emulator,
        "plant-subscribe",
        &subscribe,
        &format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/plant"),
    )
    .await;

    let cases = vec![
        ("a certificate ARN", certificate.clone(), "cert-subscribe"),
        (
            "a Cognito identity",
            cognito_identity.clone(),
            "cognito-subscribe",
        ),
        (
            "a thing group ARN governed from above",
            child_arn.clone(),
            "plant-subscribe",
        ),
    ];

    for (label, principal, expected_policy) in &cases {
        let answer = emulator
            .ask(principal, "SUBSCRIBE", std::slice::from_ref(&resource))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label}: TestAuthorization must answer 200, got {} {:?}",
            answer.status, answer.body
        );
        let decided = answer.decision();
        assert_eq!(
            decided.decision, "ALLOWED",
            "case {label}: expected ALLOWED, got {:?} ({:?})",
            decided.decision, answer.body
        );
        assert_eq!(
            decided.allowed,
            vec![(*expected_policy).to_string()],
            "case {label}: expected the policy attached to that principal, got {:?}",
            decided.allowed
        );
    }

    // A Cognito identity pool is named in the same form, and names the same key space.
    let by_pool = emulator
        .test_authorization(
            &json!({
                "cognitoIdentityPoolId": cognito_identity,
                "authInfos": [{ "actionType": "SUBSCRIBE", "resources": [resource] }],
            }),
            None,
        )
        .await;
    let decided = by_pool.decision();
    assert_eq!(
        decided.decision, "ALLOWED",
        "case a Cognito identity pool id: expected ALLOWED, got {:?} ({:?})",
        decided.decision, by_pool.body
    );
}

/// One `authResults` entry per `authInfo`, in the order asked, over the whole modelled
/// 1..10 bound — and a request outside that bound is refused.
#[tokio::test]
async fn one_result_answers_each_question_in_order() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;

    given_attached_policy(
        &emulator,
        "even-floors",
        &statement("Allow", json!("iot:Publish"), json!(topic_arn("floor/*/even"))),
        &certificate,
    )
    .await;

    // `authInfos` is modelled `@length(min: 1, max: 10)`, so the bound itself and one past
    // it are both asked.
    for count in [1usize, 10] {
        let questions: Vec<Value> = (0..count)
            .map(|index| {
                let path = if index % 2 == 0 { "even" } else { "odd" };
                json!({
                    "actionType": "PUBLISH",
                    "resources": [topic_arn(&format!("floor/{index}/{path}"))],
                })
            })
            .collect();
        let answer = emulator
            .test_authorization(
                &json!({ "principal": certificate, "authInfos": questions }),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {count} questions: TestAuthorization must answer 200, got {} {:?}",
            answer.status, answer.body
        );
        let decisions = answer.decisions();
        assert_eq!(
            decisions.len(),
            count,
            "case {count} questions: one result answers each question, got {:?}",
            answer.body
        );
        for (index, decided) in decisions.iter().enumerate() {
            let expected = if index % 2 == 0 {
                "ALLOWED"
            } else {
                "IMPLICIT_DENY"
            };
            assert_eq!(
                decided.decision, expected,
                "case {count} questions, result {index}: expected {expected}, got {:?} \
                 ({:?})",
                decided.decision, answer.body
            );
        }
    }

    for count in [0usize, 11] {
        let questions: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "actionType": "PUBLISH",
                    "resources": [topic_arn(&format!("floor/{index}/even"))],
                })
            })
            .collect();
        let answer = emulator
            .test_authorization(
                &json!({ "principal": certificate, "authInfos": questions }),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {count} questions: a list outside the modelled 1..10 bound is refused \
             400, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// A principal or a policy name the account does not hold is refused, never decided from
/// an empty policy set.
#[tokio::test]
async fn an_identity_or_policy_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;
    let question = json!([{
        "actionType": "SUBSCRIBE",
        "resources": [topic_filter_arn("home/kitchen")],
    }]);

    let cases: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "a certificate ARN this account does not hold",
            json!({
                "principal": format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert/{}", "a".repeat(64)),
                "authInfos": question,
            }),
            404,
            "ResourceNotFoundException",
            "The certificate given in the principal does not exist.".to_string(),
        ),
        (
            "a principal of no supported form",
            json!({ "principal": "not-a-principal", "authInfos": question }),
            400,
            "InvalidRequestException",
            "Invalid policy target: not-a-principal.".to_string(),
        ),
        (
            "a thing group ARN naming no group",
            json!({
                "principal": format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/absent"),
                "authInfos": question,
            }),
            404,
            "ResourceNotFoundException",
            format!("ThingGroup {ACCOUNT_ID}:absent not found."),
        ),
        (
            "a cognitoIdentityPoolId of no supported form",
            json!({ "cognitoIdentityPoolId": "nope", "authInfos": question }),
            400,
            "InvalidRequestException",
            "Invalid CognitoIdentityPoolId: nope.".to_string(),
        ),
        (
            "a policyNamesToAdd naming no policy",
            json!({
                "principal": certificate,
                "authInfos": question,
                "policyNamesToAdd": ["absent-policy"],
            }),
            404,
            "ResourceNotFoundException",
            "Policy absent-policy does not exist".to_string(),
        ),
    ];

    for (label, request, status, code, message) in &cases {
        let answer = emulator.test_authorization(request, None).await;
        assert_eq!(
            answer.status, *status,
            "case {label}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(*code),
            "case {label}: expected error code {code}, got {:?}",
            answer.error_code
        );
        assert!(
            answer.message().contains(message),
            "case {label}: expected a message carrying {message:?}, got {:?}",
            answer.message()
        );
    }

    // A name only SKIPPED needs no document, so it is not resolved against the registry:
    // skipping a policy nothing holds simply removes nothing.
    let skipped = emulator
        .test_authorization(
            &json!({
                "principal": certificate,
                "authInfos": question,
                "policyNamesToSkip": ["absent-policy"],
            }),
            None,
        )
        .await;
    assert_eq!(
        skipped.status, 200,
        "case a policyNamesToSkip naming no policy: expected HTTP 200, got {} {:?}",
        skipped.status, skipped.body
    );
    assert_eq!(
        skipped.decision().decision,
        "IMPLICIT_DENY",
        "case a policyNamesToSkip naming no policy: expected IMPLICIT_DENY, got {:?}",
        skipped.body
    );
}

/// How much evaluation one request may ask for is bounded: at the ceiling it is answered,
/// one past it refused with the modelled `LimitExceededException` (HTTP 410).
#[tokio::test]
async fn a_request_past_the_evaluation_ceilings_is_refused() {
    let emulator = Emulator::new();
    let certificate = given_certificate(&emulator).await;
    given_attached_policy(
        &emulator,
        "every-topic",
        &statement("Allow", json!("iot:Publish"), json!(topic_arn("*"))),
        &certificate,
    )
    .await;
    // The ceiling the handler states for one question's resources, and the one it states
    // for each policy-name list (`state::MAX_AUTHORIZED_RESOURCES`,
    // `state::MAX_POLICIES_PER_TARGET`); both are the emulator's own, since neither member
    // carries a modelled bound.
    const RESOURCE_CEILING: usize = 10;
    const POLICY_NAME_CEILING: usize = 10;

    let resources = |count: usize| -> Vec<String> {
        (0..count).map(|index| topic_arn(&format!("t/{index}"))).collect()
    };
    let names = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("every-topic-{index}")).collect()
    };

    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "resources at the ceiling",
            json!({
                "principal": certificate,
                "authInfos": [{ "actionType": "PUBLISH", "resources": resources(RESOURCE_CEILING) }],
            }),
            200,
        ),
        (
            "resources one past the ceiling",
            json!({
                "principal": certificate,
                "authInfos": [{
                    "actionType": "PUBLISH",
                    "resources": resources(RESOURCE_CEILING + 1),
                }],
            }),
            410,
        ),
        (
            "policyNamesToSkip one past the ceiling",
            json!({
                "principal": certificate,
                "authInfos": [{ "actionType": "PUBLISH", "resources": resources(1) }],
                "policyNamesToSkip": names(POLICY_NAME_CEILING + 1),
            }),
            410,
        ),
        (
            "policyNamesToSkip at the ceiling",
            json!({
                "principal": certificate,
                "authInfos": [{ "actionType": "PUBLISH", "resources": resources(1) }],
                "policyNamesToSkip": names(POLICY_NAME_CEILING),
            }),
            200,
        ),
    ];

    for (label, request, status) in &cases {
        let answer = emulator.test_authorization(request, None).await;
        assert_eq!(
            answer.status, *status,
            "case {label}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        if *status == 410 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("LimitExceededException"),
                "case {label}: expected the modelled limit error, got {:?}",
                answer.error_code
            );
            assert!(
                answer.message().contains("at most 10"),
                "case {label}: the refusal must name the limit, got {:?}",
                answer.message()
            );
        } else {
            assert_eq!(
                answer.decision().decision,
                "ALLOWED",
                "case {label}: the request at the ceiling is still decided, got {:?}",
                answer.body
            );
        }
    }
}
