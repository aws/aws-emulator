// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed EventBridge rule whose Amazon SQS target queue carries no
//! resource policy admitting this service DEAD-LETTERS the envelope, and a stack update that
//! adds the queue policy restores delivery — end to end over the real `MultiServiceRouter`
//! with CloudFormation, EventBridge and Amazon SQS mounted on one engine, which is where the
//! deployment's own calls into the services that own each declared resource land.
//!
//! The rule pinned here: EventBridge invokes an Amazon SQS target by calling
//! `sqs:SendMessage` as the service principal `events.amazonaws.com`, so the TARGET queue's
//! own resource policy is what admits the send; a send no statement admits is refused by that
//! service without disclosing the queue — it models no authorization failure on
//! `SendMessage` at all, and answers `QueueDoesNotExist` (HTTP 400, `awsQueryError` code
//! `AWS.SimpleQueueService.NonExistentQueue`) under the fixed sentence that states the absence
//! and the want of access without saying which holds — nothing is
//! enqueued on the target, and — because the target carries a `DeadLetterConfig` — the
//! envelope is written to that queue instead with the message attributes `RULE_ARN`,
//! `TARGET_ARN`, `ERROR_CODE` and `ERROR_MESSAGE`, under the class that answer's own code
//! belongs to (`NO_RESOURCE`) and with no retries. None of that depends on how
//! the rule, its target or the queue policy came to exist: a stack's `AWS::Events::Rule` and
//! `AWS::SQS::QueuePolicy` leave behind the same rule, target and document a client's own
//! `PutRule`/`PutTargets`/`SetQueueAttributes` do, and a stack UPDATE that adds the missing
//! grant makes the next matching event deliver normally with the rule and the queue
//! untouched.
//!
//! The rows differ along what the rule generalises over, and nothing else: the FORM the
//! dead-letter queue's grant takes (unconditioned, scoped by `aws:SourceArn` to the deployed
//! rule, scoped by `aws:SourceAccount` to its account, naming every resource with the
//! wildcard, spelling the action in the console's casing), the FORM the stack update takes to
//! restore delivery (a new `AWS::SQS::QueuePolicy` resource for the target queue, or the
//! resource the stack already has with its `Queues` extended to name it), WHO names the
//! deployed resources (the template, CloudFormation when the declaration names none, or the
//! two together where only the queues are left unnamed) and how the bus is ADDRESSED on the
//! calls that take one (`EventBusNameOrArn` admits the bus's name or its ARN).
//!
//! Three crates have to agree for that to hold — CloudFormation provisions the bus, the two
//! queues, the policy and the rule with its per-target `RetryPolicy`/`DeadLetterConfig`;
//! EventBridge matches, decides the send against the queue's document and dead-letters what
//! it could not hand over; Amazon SQS holds both documents and both queues — so the suite
//! lives here rather than in any one crate. Every observation is made where a client makes
//! one: on the two queues the stack publishes.
//!
//! `service-com-amazonaws-eventbridge-awsevents`'s own
//! `tests/all/queue_target_policy_authorization.rs` and
//! `tests/all/queue_target_withheld_send_answer.rs` are the suites for which document shapes
//! admit the send and what the refused delivery reports; this one is the suite for the whole
//! DEPLOYED pipeline, including the stack update that restores delivery.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, outputs_of};

/// The Region every request here is signed for — the one `wait_for_stack_status` polls in.
const REGION: &str = "us-east-1";

/// The `source` the deployed rule matches on, and one it does not name.
const SOURCE: &str = "sim2real.events.cfn.queue-target";
const OTHER_SOURCE: &str = "sim2real.events.cfn.unmatched";

/// The `detail-type` every event this suite publishes carries.
const DETAIL_TYPE: &str = "Queue Target Probe";

/// The service principal EventBridge hands a resource-policy-governed target over as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The action a delivery to a queue target performs on that queue.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The casing the console writes the same action in — IAM compares an action name
/// case-insensitively, so a grant spelled this way grants the same send.
const CONSOLE_CASED_SEND_MESSAGE: &str = "SQS:SendMessage";

/// The wildcard an IAM resource element may name instead of an ARN.
const EVERY_RESOURCE: &str = "*";

/// The stack statuses this suite waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// How long between two looks at a queue, and how many looks one wait makes: the wait is on
/// the CONDITION (a message is there), not on a span, and the count is generous enough that a
/// host running many suites at once reaches it rather than the deadline.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);
const LOOKS: usize = 2400;

/// How many looks a queue that must stay EMPTY is given before it is believed: a delivery
/// EventBridge made is enqueued by the `PutEvents` that decided it, which has already
/// answered before any look here, so what such a queue holds is settled at the first look and
/// the rest are margin.
const QUIET_LOOKS: usize = 4;

/// The dead-letter message attributes "EventBridge adds … to the dead-letter message".
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// The two further attributes a RETRIED delivery's dead letter carries, which a refusal
/// nothing retries does not.
const RETRY_ATTEMPTS_ATTRIBUTE: &str = "RETRY_ATTEMPTS";
const EXHAUSTED_RETRY_CONDITION_ATTRIBUTE: &str = "EXHAUSTED_RETRY_CONDITION";

/// The class EventBridge records for a delivery the target's own document refused: the class
/// of the code that service answered under, which for Amazon SQS is its own not-found one,
/// because it does not disclose a queue it will not let the caller write to.
const NO_RESOURCE: &str = "NO_RESOURCE";

/// The error code and status that answer carries
/// (`smithy_query(query="shape", shape="QueueDoesNotExist",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`),
/// and the fixed sentence beside it — stated here rather than read from any implementation,
/// so a wrong answer cannot make this suite pass.
const QUEUE_NOT_DISCLOSED_CODE: &str = "AWS.SimpleQueueService.NonExistentQueue";
const QUEUE_NOT_DISCLOSED_STATUS: u16 = 400;
const QUEUE_NOT_DISCLOSED_SENTENCE: &str =
    "The specified queue does not exist or you do not have access to it.";

/// The answer Amazon SQS gives a sender its documents did not permit, up to the id of the
/// refused call: its fixed text inside the metadata block naming that service, the status and
/// the code — so nothing of the refused send appears in it.
fn withheld_answer_head() -> String {
    format!(
        "{QUEUE_NOT_DISCLOSED_SENTENCE} (Service: AmazonSQS; Status Code: \
         {QUEUE_NOT_DISCLOSED_STATUS}; Error Code: {QUEUE_NOT_DISCLOSED_CODE}; Request ID: "
    )
}

/// What closes that answer, after the id of the refused call.
const ANSWER_TAIL: &str = "; Proxy: null)";

/// CloudFormation, EventBridge and Amazon SQS over ONE engine, as `core/router/src/lib.rs`
/// mounts them: the three services that have to agree for the rule above to hold, and the
/// three the deployment's own calls reach.
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
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One CloudFormation `awsQuery` call that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {text}"
    );
    text
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Every message the queue at `url` holds right now, with its message attributes — which is
/// how a client reads what EventBridge recorded on a dead letter.
async fn received(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answered = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    answered["Messages"].as_array().cloned().unwrap_or_default()
}

/// Look at the queue at `url` until it holds at least one message, and answer everything it
/// handed over.
async fn wait_for_messages(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    for _ in 0..LOOKS {
        held.extend(received(router, case, url).await);
        if !held.is_empty() {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    held
}

/// Everything the queue at `url` held over a run of looks, for a queue that must hold
/// nothing.
async fn stayed_quiet(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    for _ in 0..QUIET_LOOKS {
        held.extend(received(router, case, url).await);
        if !held.is_empty() {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    held
}

/// Take `message` off the queue at `url`, so a later look at that queue reports what arrived
/// AFTER it rather than the same message again.
async fn delete(router: &MultiServiceRouter, case: &str, url: &str, message: &Value) {
    let handle = message["ReceiptHandle"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected a received message to carry a ReceiptHandle, got {message:?}")
    });
    sqs(
        router,
        case,
        "DeleteMessage",
        &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
    )
    .await;
}

/// The string value of one message attribute.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How a row's declared grant is scoped — the three forms a least-privilege template writes
/// one in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// No `Condition` at all.
    Unscoped,
    /// `ArnEquals` on `aws:SourceArn`, the ARN of the rule whose delivery is being made.
    ItsRule,
    /// `StringEquals` on `aws:SourceAccount`, the account that rule belongs to.
    ItsAccount,
}

/// How a row's stack UPDATE grants the target queue the send it was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restore {
    /// A second `AWS::SQS::QueuePolicy` resource, naming the target queue.
    NewResource(Scope),
    /// The `Queues` of the resource the stack already has, extended to name the target queue
    /// beside the dead-letter queue.
    ExtendTheDeclaredPolicy,
}

/// The `Condition` element a scope writes, or `None` for an unscoped grant. The rule ARN is
/// spelled out rather than read with `Fn::GetAtt`, because the grant on the DEAD-LETTER queue
/// is one the rule itself depends on.
fn condition(scope: Scope, rule_arn: &str, account: &str) -> Option<Value> {
    match scope {
        Scope::Unscoped => None,
        Scope::ItsRule => Some(json!({ "ArnEquals": { "aws:SourceArn": rule_arn } })),
        Scope::ItsAccount => Some(json!({ "StringEquals": { "aws:SourceAccount": account } })),
    }
}

/// How a row spells the two halves of the grant that are not its principal: the resources it
/// names, and the case the action's service prefix is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `Resource` is the queue's own ARN, `Action` is `sqs:SendMessage`.
    QueueArn,
    /// `Resource` is the wildcard that stands for every resource.
    WildcardResource,
    /// `Action` is spelled `SQS:SendMessage`, the casing the console writes.
    ConsoleCasedAction,
}

/// How a row addresses the event bus the stack created: by the name `Ref` returns, or by the
/// ARN `Fn::GetAtt` reads — `EventBusNameOrArn` admits either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusAddress {
    Name,
    Arn,
}

/// One statement allowing this service the send on the queues `resources` names, scoped as
/// the row asks and spelled in the row's form.
fn grant(
    resources: Value,
    scope: Scope,
    form: Form,
    rule_arn: &str,
    account: &str,
) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": EVENTS_PRINCIPAL },
        "Action": match form {
            Form::ConsoleCasedAction => CONSOLE_CASED_SEND_MESSAGE,
            _ => SEND_MESSAGE,
        },
        "Resource": match form {
            Form::WildcardResource => json!(EVERY_RESOURCE),
            _ => resources,
        },
    });
    if let Some(condition) = condition(scope, rule_arn, account) {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// Whether the row's template NAMES the resources it declares, or leaves their names to
/// CloudFormation — which decides every identifier another resource of the same stack then
/// refers to through `Ref` and `Fn::GetAtt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    /// Every queue, the bus and the rule carry a declared name.
    Declared,
    /// The two queues and the rule carry none, so CloudFormation generates them.
    Generated,
    /// The bus and the rule carry declared names and the two QUEUES carry none, which is the
    /// shape a template takes when only the resources a client addresses by name are named.
    QueuesGenerated,
}

/// One row of the table.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// How the grant the DEAD-LETTER queue carries from the start is scoped.
    dead_letter_scope: Scope,
    /// What the stack update does to restore delivery to the target queue.
    restore: Restore,
    /// Whether the template names the resources it declares.
    naming: Naming,
    /// How the row spells the grant's action and resource.
    form: Form,
    /// How the row addresses the bus on the calls that take one.
    bus_address: BusAddress,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an unscoped dead-letter grant, delivery restored by a new policy resource",
            stack: "cfn-queue-target-dlq-plain",
            dead_letter_scope: Scope::Unscoped,
            restore: Restore::NewResource(Scope::Unscoped),
            naming: Naming::Declared,
            form: Form::QueueArn,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "a dead-letter grant scoped to the rule, delivery restored by a grant \
                   scoped to the rule",
            stack: "cfn-queue-target-dlq-rule-scoped",
            dead_letter_scope: Scope::ItsRule,
            restore: Restore::NewResource(Scope::ItsRule),
            naming: Naming::Declared,
            form: Form::QueueArn,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "a dead-letter grant scoped to the account, delivery restored by extending \
                   the declared policy",
            stack: "cfn-queue-target-dlq-account-scoped",
            dead_letter_scope: Scope::ItsAccount,
            restore: Restore::ExtendTheDeclaredPolicy,
            naming: Naming::Declared,
            form: Form::QueueArn,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "the queues and the rule are named by CloudFormation, not by the template",
            stack: "cfn-queue-target-dlq-generated",
            dead_letter_scope: Scope::Unscoped,
            restore: Restore::NewResource(Scope::Unscoped),
            naming: Naming::Generated,
            form: Form::QueueArn,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "the bus and the rule are named by the template while the queues are not",
            stack: "cfn-queue-target-dlq-queues-generated",
            dead_letter_scope: Scope::ItsRule,
            restore: Restore::NewResource(Scope::Unscoped),
            naming: Naming::QueuesGenerated,
            form: Form::QueueArn,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "both grants name every resource with the wildcard",
            stack: "cfn-queue-target-dlq-wildcard",
            dead_letter_scope: Scope::Unscoped,
            restore: Restore::NewResource(Scope::Unscoped),
            naming: Naming::Declared,
            form: Form::WildcardResource,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "both grants spell the action with the console's casing",
            stack: "cfn-queue-target-dlq-console-cased",
            dead_letter_scope: Scope::Unscoped,
            restore: Restore::NewResource(Scope::Unscoped),
            naming: Naming::Declared,
            form: Form::ConsoleCasedAction,
            bus_address: BusAddress::Name,
        },
        Row {
            case: "the bus is addressed by the ARN Fn::GetAtt reads rather than by its name",
            stack: "cfn-queue-target-dlq-bus-arn",
            dead_letter_scope: Scope::Unscoped,
            restore: Restore::NewResource(Scope::Unscoped),
            naming: Naming::Declared,
            form: Form::QueueArn,
            bus_address: BusAddress::Arn,
        },
    ]
}

/// The template one row deploys: a custom event bus, the target queue, the dead-letter queue
/// with the row's grant on IT alone, and a rule on that bus whose one target is the target
/// queue, carrying a `RetryPolicy` and the `DeadLetterConfig` naming the dead-letter queue.
///
/// `restored` is `false` for the create — where the target queue carries no document at all —
/// and `true` for the update that grants it the send.
fn template(row: &Row, rule_arn: &str, account: &str, restored: bool) -> String {
    let stack = row.stack;
    let dead_letter_resources = if restored && row.restore == Restore::ExtendTheDeclaredPolicy {
        json!([{ "Ref": "Dlq" }, { "Ref": "TargetQueue" }])
    } else {
        json!([{ "Ref": "Dlq" }])
    };
    let dead_letter_grant = if restored && row.restore == Restore::ExtendTheDeclaredPolicy {
        grant(
            json!([
                { "Fn::GetAtt": ["Dlq", "Arn"] },
                { "Fn::GetAtt": ["TargetQueue", "Arn"] },
            ]),
            row.dead_letter_scope,
            row.form,
            rule_arn,
            account,
        )
    } else {
        grant(
            json!({ "Fn::GetAtt": ["Dlq", "Arn"] }),
            row.dead_letter_scope,
            row.form,
            rule_arn,
            account,
        )
    };
    let mut resources = json!({
        "Bus": {
            "Type": "AWS::Events::EventBus",
            "Properties": { "Name": format!("{stack}-bus") },
        },
        "TargetQueue": { "Type": "AWS::SQS::Queue" },
        "Dlq": { "Type": "AWS::SQS::Queue" },
        "DlqPolicy": {
            "Type": "AWS::SQS::QueuePolicy",
            "Properties": {
                "Queues": dead_letter_resources,
                "PolicyDocument": dead_letter_grant,
            },
        },
        "Rule": {
            "Type": "AWS::Events::Rule",
            "Properties": {
                "EventBusName": { "Ref": "Bus" },
                "EventPattern": { "source": [SOURCE] },
                "State": "ENABLED",
                "Targets": [{
                    "Id": "queue",
                    "Arn": { "Fn::GetAtt": ["TargetQueue", "Arn"] },
                    "RetryPolicy": {
                        "MaximumRetryAttempts": 3,
                        "MaximumEventAgeInSeconds": 3600,
                    },
                    "DeadLetterConfig": { "Arn": { "Fn::GetAtt": ["Dlq", "Arn"] } },
                }],
            },
        },
    });
    if row.naming == Naming::Declared {
        resources["TargetQueue"]["Properties"] =
            json!({ "QueueName": format!("{stack}-target") });
        resources["Dlq"]["Properties"] = json!({ "QueueName": format!("{stack}-dlq") });
    }
    if row.naming != Naming::Generated {
        resources["Rule"]["Properties"]["Name"] = json!(format!("{stack}-rule"));
    }
    if restored {
        if let Restore::NewResource(scope) = row.restore {
            resources["TargetPolicy"] = json!({
                "Type": "AWS::SQS::QueuePolicy",
                "Properties": {
                    "Queues": [{ "Ref": "TargetQueue" }],
                    "PolicyDocument": grant(
                        json!({ "Fn::GetAtt": ["TargetQueue", "Arn"] }),
                        scope,
                        row.form,
                        rule_arn,
                        account,
                    ),
                },
            });
        }
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": {
            "TargetUrl": { "Value": { "Ref": "TargetQueue" } },
            "TargetArn": { "Value": { "Fn::GetAtt": ["TargetQueue", "Arn"] } },
            "DlqUrl": { "Value": { "Ref": "Dlq" } },
            "DlqArn": { "Value": { "Fn::GetAtt": ["Dlq", "Arn"] } },
            "RuleName": { "Value": { "Ref": "Rule" } },
            "RuleArn": { "Value": { "Fn::GetAtt": ["Rule", "Arn"] } },
            "BusName": { "Value": { "Ref": "Bus" } },
            "BusArn": { "Value": { "Fn::GetAtt": ["Bus", "Arn"] } },
        },
    })
    .to_string()
}

/// Publish one entry onto `bus` and answer the `EventId` EventBridge minted for it: the
/// envelope a target is handed — and the one a dead letter parks — carries that id as its
/// `id`.
async fn publish(
    router: &MultiServiceRouter,
    case: &str,
    bus: &str,
    source: &str,
    marker: &str,
) -> String {
    let answered = events(
        router,
        case,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": source,
                "DetailType": DETAIL_TYPE,
                "EventBusName": bus,
                "Detail": json!({ "probe": marker }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        answered["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the entry to be accepted, got {answered}"
    );
    answered["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the accepted entry to carry an EventId, got {answered}")
        })
        .to_string()
}

/// Every row: the deployed target queue carrying no grant dead-letters the envelope with the
/// four attributes, nothing reaches that queue — and the stack update that grants it the send
/// makes the next matching event deliver, with the rule and the queue unchanged.
#[tokio::test]
async fn a_deployed_queue_target_without_a_policy_dead_letters_until_an_update_grants_the_send() {
    for row in rows() {
        let case = row.case;
        let router = build_router();
        let account = engine::emulator_account_id();
        // The ARN the rule will carry, which a grant scoped to its deliveries names. A rule
        // on a bus other than the account's `default` one names that bus. A row whose
        // template does not name its rule cannot spell that ARN out, so its grants name no
        // rule and the ARN is read off the stack's own output below.
        let declared_rule_arn = match row.naming {
            Naming::Declared | Naming::QueuesGenerated => format!(
                "arn:aws:events:{REGION}:{account}:rule/{}-bus/{}-rule",
                row.stack, row.stack
            ),
            Naming::Generated => String::new(),
        };

        cloudformation(
            &router,
            case,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(&row, &declared_rule_arn, account, false)),
            ],
        )
        .await;
        let (status, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to deploy, got {status} with {described}"
        );
        let outputs = outputs_of(&described);
        let published = |name: &str| {
            outputs.get(name).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: expected the stack to publish {name}, got {described}")
            })
        };
        let target_url = published("TargetUrl");
        let target_arn = published("TargetArn");
        let dead_letter_url = published("DlqUrl");
        let dead_letter_arn = published("DlqArn");
        let bus = published("BusName");
        let bus_arn = published("BusArn");
        // How this row addresses the bus on the calls that take one: `EventBusNameOrArn`
        // admits either the name or the bus's ARN.
        let addressed = match row.bus_address {
            BusAddress::Name => bus.clone(),
            BusAddress::Arn => bus_arn.clone(),
        };
        let rule_name = published("RuleName");
        let rule_arn = published("RuleArn");
        if row.naming == Naming::Declared {
            assert_eq!(
                rule_arn, declared_rule_arn,
                "case {case:?}: expected the deployed rule to carry the ARN a grant names it by"
            );
        }
        assert_eq!(
            rule_arn,
            format!("arn:aws:events:{REGION}:{account}:rule/{bus}/{rule_name}"),
            "case {case:?}: expected the rule on a custom bus to carry that bus in its ARN"
        );

        // The deployed target reads back with both per-target documents, which is what the
        // dead-letter path is governed by.
        let listed = events(
            &router,
            case,
            "ListTargetsByRule",
            &json!({ "Rule": rule_name, "EventBusName": addressed }),
        )
        .await;
        let target = &listed["Targets"][0];
        assert_eq!(
            target["DeadLetterConfig"]["Arn"].as_str(),
            Some(dead_letter_arn.as_str()),
            "case {case:?}: expected the target's DeadLetterConfig to name the deployed \
             dead-letter queue, got {listed}"
        );
        assert_eq!(
            target["Arn"].as_str(),
            Some(target_arn.as_str()),
            "case {case:?}: expected the target to name the deployed queue, got {listed}"
        );
        assert_eq!(
            target["RetryPolicy"],
            json!({ "MaximumRetryAttempts": 3, "MaximumEventAgeInSeconds": 3600 }),
            "case {case:?}: expected the target to read back with its RetryPolicy, got {listed}"
        );

        // An entry the rule's pattern does not name, published first so the waits below give
        // it every chance to arrive somewhere it must not.
        let unmatched = publish(&router, case, &addressed, OTHER_SOURCE, "unmatched").await;

        let withheld = publish(&router, case, &addressed, SOURCE, "withheld").await;
        let dead_letters = wait_for_messages(&router, case, &dead_letter_url).await;
        assert_eq!(
            dead_letters.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {dead_letters:?}"
        );
        let parked = &dead_letters[0];
        assert!(
            parked["Body"]
                .as_str()
                .is_some_and(|body| body.contains(&withheld)),
            "case {case:?}: expected the dead-lettered body to be the envelope carrying the \
             published event's id {withheld:?}, got {parked:?}"
        );
        assert!(
            parked["Body"].as_str().is_some_and(|body| body.contains(SOURCE)),
            "case {case:?}: expected the dead-lettered body to carry the published source, \
             got {parked:?}"
        );
        for (name, expected) in [
            (RULE_ARN_ATTRIBUTE, rule_arn.clone()),
            (TARGET_ARN_ATTRIBUTE, target_arn.clone()),
            (ERROR_CODE_ATTRIBUTE, NO_RESOURCE.to_string()),
        ] {
            let actual = attribute(parked, name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the dead letter's {name} {expected:?}, got {actual:?} \
                 in {parked:?}"
            );
        }
        let reported = attribute(parked, ERROR_MESSAGE_ATTRIBUTE);
        let head = withheld_answer_head();
        assert!(
            reported.starts_with(head.as_str()) && reported.ends_with(ANSWER_TAIL),
            "case {case:?}: expected the answer that service gives a send it will not disclose \
             the queue of, got {reported:?}"
        );
        // What varies in that answer is the id of the ONE refused call, and nothing else: an
        // AWS request id is a UUID in the canonical 8-4-4-4-12 form.
        let request_id = reported
            .strip_prefix(head.as_str())
            .and_then(|rest| rest.strip_suffix(ANSWER_TAIL))
            .unwrap_or_default();
        let widths: Vec<usize> = request_id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected the refused call's own request id between the fixed text, \
             got {request_id:?}"
        );
        // That answer discloses nothing of the send it refused, so neither the queue nor the
        // principal is named in it.
        for withheld in [target_arn.as_str(), EVENTS_PRINCIPAL] {
            assert!(
                !reported.contains(withheld),
                "case {case:?}: expected {withheld:?} NOT to be disclosed by that answer, got \
                 {reported:?}"
            );
        }
        // A refusal of the send is not retryable, so the target's `RetryPolicy` adds no
        // attempts and the dead letter names no retry count and no exhausted bound.
        for name in [RETRY_ATTEMPTS_ATTRIBUTE, EXHAUSTED_RETRY_CONDITION_ATTRIBUTE] {
            assert!(
                parked["MessageAttributes"][name].is_null(),
                "case {case:?}: a refusal nothing retries carries no {name}, got {parked:?}"
            );
        }
        // Taken off the queue now it has been read, so the look after the update reports what
        // the RESTORED delivery left there rather than this same message again.
        delete(&router, case, &dead_letter_url, parked).await;

        let delivered = stayed_quiet(&router, case, &target_url).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: expected nothing on the target queue no document admits the send \
             to, got {delivered:?}"
        );

        // The update adds only the missing grant: the rule and the queue stay where they are.
        cloudformation(
            &router,
            case,
            "UpdateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(&row, &declared_rule_arn, account, true)),
            ],
        )
        .await;
        let (status, described) = wait_for_stack_status(&router, row.stack, UPDATE_COMPLETE).await;
        assert_eq!(
            status, UPDATE_COMPLETE,
            "case {case:?}: expected the update to settle, got {status} with {described}"
        );
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("RuleArn").cloned().unwrap_or_default(),
            rule_arn,
            "case {case:?}: expected the update to leave the rule in place, got {described}"
        );
        assert_eq!(
            outputs.get("TargetArn").cloned().unwrap_or_default(),
            target_arn,
            "case {case:?}: expected the update to leave the target queue in place, got \
             {described}"
        );
        // The queues the UPDATED stack reports are the queues it reported before: a client
        // that re-reads the outputs after an update addresses the same two queues, so the
        // urls it reads now are the ones the create left behind.
        for (name, url) in [("TargetUrl", &target_url), ("DlqUrl", &dead_letter_url)] {
            assert_eq!(
                outputs.get(name).cloned().unwrap_or_default(),
                *url,
                "case {case:?}: expected the update to leave the {name} output naming the same \
                 queue, got {described}"
            );
        }

        let restored = publish(&router, case, &addressed, SOURCE, "restored").await;
        let delivered = wait_for_messages(&router, case, &target_url).await;
        assert!(
            delivered
                .iter()
                .any(|message| message["Body"]
                    .as_str()
                    .is_some_and(|body| body.contains(&restored))),
            "case {case:?}: expected the event published after the update to be delivered to \
             the target queue, got {delivered:?}"
        );

        // Nothing of the entry the pattern does not name reached either queue, and the
        // delivery the update restored was not ALSO dead-lettered: the dead letter read above
        // was taken off that queue, so what it holds now is only what arrived since.
        let parked_after = stayed_quiet(&router, case, &dead_letter_url).await;
        assert!(
            parked_after.is_empty(),
            "case {case:?}: expected the restored delivery to dead-letter nothing, got \
             {parked_after:?}"
        );
        for (queue, held) in [
            ("the target queue", &delivered),
            ("the dead-letter queue", &dead_letters),
        ] {
            assert!(
                !held.iter().any(|message| message["Body"]
                    .as_str()
                    .is_some_and(|body| body.contains(&unmatched))),
                "case {case:?}: expected the event the pattern does not name to reach nothing, \
                 but {queue} holds {held:?}"
            );
        }
    }
}
