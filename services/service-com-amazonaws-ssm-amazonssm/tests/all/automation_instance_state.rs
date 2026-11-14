// SPDX-License-Identifier: CC0-1.0

//! The `aws:changeInstanceState` step of an Automation runbook, end to end over the front
//! door: the step really moves the Amazon EC2 instances it names, in that service itself.
//!
//! RULE under test: an Automation action whose work belongs to another service is performed
//! BY that service — `aws:changeInstanceState` brings the instances its `InstanceIds` name to
//! its `DesiredState` through Amazon EC2's own operations, the step is `InProgress` until that
//! service reports every one of them at a state the step accepts, and the step's outcome is
//! what that service answered: it succeeds when they are there, and fails when that service
//! refuses the call or the window the action's `DesiredStateTransitionTimeout` names runs out.
//! Nothing is reported as having happened to an instance that did not happen in Amazon EC2.
//!
//! Every case goes through the wire a client uses: `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body for Systems Manager, and an
//! `ec2Query` form POST for every Amazon EC2 call — so the instance each case reads back is
//! read through the service that owns it. Nothing reaches into a state struct.
//!
//! The rows differ along what the rule generalises over: the state the step asks for (each
//! value `DesiredState` admits), whether the step changes the state or only asserts it, the
//! number of instances one step names, and the two ways a step fails (an instance that
//! service holds none of, and a transition window that runs out).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, REGION};

/// The EC2 API version an `ec2Query` body carries beside its `Action`.
const EC2_API_VERSION: &str = "2016-11-15";

/// A well-formed AMI id to launch from: the issued form Amazon EC2's own `ImageId` takes
/// (`ami-` and 17 hexadecimal characters).
const IMAGE: &str = "ami-0123456789abcdef0";

/// How long a case waits for a condition another component's clock owes.
///
/// Generous on purpose: the automation's own pickup span and Amazon EC2's state transition
/// are each a stored due instant, and nothing sleeps for either, so a case LOOKS AGAIN until
/// the condition holds rather than waiting a fixed time. The only tight window in this suite
/// is the one a case tests ([`SHORT_TRANSITION_WINDOW`]).
const DEADLINE: Duration = Duration::from_secs(120);

/// How long one look-again cycle pauses before polling again.
const POLL_PAUSE: Duration = Duration::from_millis(50);

/// The `DesiredStateTransitionTimeout` the timing-out row gives its step: the window THAT
/// case is about, and the only tight one in this suite.
const SHORT_TRANSITION_WINDOW: u64 = 1;

/// A router with Systems Manager AND Amazon EC2 mounted on ONE shared engine.
///
/// EC2 is there because the step's work is that service's: the instance is its resource, the
/// transition is its own, and what the step waits for is its answer.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope, which is where each call's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// One Amazon EC2 call over the `ec2Query` wire its model declares, as its status and XML
/// body.
async fn ec2(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in parameters {
        body.push_str(&format!("&{name}={value}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed ec2Query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<tag>` of an XML body after `from`, with where it ended.
fn element_after(body: &str, tag: &str, from: usize) -> Option<(String, usize)> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body[from..].find(&open)? + from + open.len();
    let end = body[start..].find(&close)? + start;
    Some((body[start..end].to_string(), end + close.len()))
}

/// Launch `count` instances through Amazon EC2's own `RunInstances`, as the ids it answered.
async fn launch(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let count = count.to_string();
    let (status, body) = ec2(
        router,
        "RunInstances",
        &[
            ("ImageId", IMAGE),
            ("MinCount", &count),
            ("MaxCount", &count),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RunInstances to launch, got {status} with {body}"
    );
    let mut ids = Vec::new();
    let mut at = 0usize;
    while let Some((id, next)) = element_after(&body, "instanceId", at) {
        ids.push(id);
        at = next;
    }
    assert_eq!(
        ids.len(),
        count.parse::<usize>().expect("a count"),
        "case {case:?}: expected {count} launched instances, got {ids:?} from {body}"
    );
    ids
}

/// The state Amazon EC2 reports `instance_id` in, read through its own `DescribeInstances`.
async fn instance_state(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let (status, body) = ec2(
        router,
        "DescribeInstances",
        &[("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeInstances({instance_id}) to answer, got {status} with \
         {body}"
    );
    let state = body
        .find("<instanceState>")
        .and_then(|at| element_after(&body, "name", at))
        .map(|(state, _)| state);
    state.unwrap_or_else(|| {
        panic!("case {case:?}: DescribeInstances({instance_id}) reported no state: {body}")
    })
}

/// Wait until Amazon EC2 reports `instance_id` in `state`, looking again until it does or
/// [`DEADLINE`] passes — the transition is a stored due instant of that service, and nothing
/// sleeps for it.
async fn wait_for_instance_state(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
    state: &str,
) {
    let started = Instant::now();
    loop {
        let reported = instance_state(router, case, instance_id).await;
        if reported == state {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: expected Amazon EC2 to report {instance_id} {state:?}, it reports \
             {reported:?}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Register one Automation runbook of this account, through `CreateDocument`.
async fn create_runbook(router: &MultiServiceRouter, case: &str, name: &str, content: &Value) {
    let (status, body) = call(
        router,
        "CreateDocument",
        &json!({
            "Name": name,
            "DocumentType": "Automation",
            "DocumentFormat": "JSON",
            "Content": content.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateDocument({name}) to answer, got {status} with {body}"
    );
}

/// A runbook of one `aws:changeInstanceState` step, taking the instances it acts on as a
/// `StringList` parameter and carrying whatever other inputs a row states.
fn runbook(desired_state: &str, extra: &[(&str, Value)]) -> Value {
    let mut inputs = json!({
        "InstanceIds": ["{{ Instances }}"],
        "DesiredState": desired_state,
    });
    for (name, value) in extra {
        inputs[*name] = value.clone();
    }
    json!({
        "schemaVersion": "0.3",
        "description": "Bring the named instances to a state",
        "parameters": {
            "Instances": {"type": "StringList", "description": "the instances to move"}
        },
        "mainSteps": [{"name": "changeState", "action": "aws:changeInstanceState",
                       "inputs": inputs}]
    })
}

/// Start one automation and answer the execution id Systems Manager minted for it.
async fn start(
    router: &MultiServiceRouter,
    case: &str,
    document_name: &str,
    instance_ids: &[String],
) -> String {
    let (status, body) = call(
        router,
        "StartAutomationExecution",
        &json!({
            "DocumentName": document_name,
            "Parameters": {"Instances": instance_ids},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartAutomationExecution({document_name}) to answer an \
         execution id, got {status} with {body}"
    );
    body["AutomationExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the start answered no execution id: {body}"))
        .to_string()
}

/// The execution Systems Manager reports for `execution_id`.
async fn execution(router: &MultiServiceRouter, case: &str, execution_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetAutomationExecution",
        &json!({"AutomationExecutionId": execution_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetAutomationExecution({execution_id}) to answer, got {status} \
         with {body}"
    );
    body["AutomationExecution"].clone()
}

/// Wait until the execution reaches a status it never leaves, looking again until it does or
/// [`DEADLINE`] passes — the walk is driven by the clock and by what Amazon EC2 answers, and
/// nothing sleeps for either.
async fn wait_for_the_outcome(
    router: &MultiServiceRouter,
    case: &str,
    execution_id: &str,
) -> Value {
    let started = Instant::now();
    loop {
        let reported = execution(router, case, execution_id).await;
        let status = reported["AutomationExecutionStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if ["Success", "Failed", "Cancelled", "TimedOut"].contains(&status.as_str()) {
            return reported;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: expected the execution to reach an outcome, it reports \
             {status:?}: {reported}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// One row of the table: a runbook, how many instances it is started for, and what the rule
/// says must then be true of the execution and of those instances in Amazon EC2.
struct Row {
    case: &'static str,
    document: &'static str,
    desired_state: &'static str,
    extra_inputs: Vec<(&'static str, Value)>,
    instances: usize,
    /// The `AutomationExecutionStatus` the execution must reach.
    status: &'static str,
    /// The state Amazon EC2 must then report every named instance in.
    instance_state: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "stopping the one instance the step names",
            document: "stop-one-instance",
            desired_state: "stopped",
            extra_inputs: Vec::new(),
            instances: 1,
            status: "Success",
            instance_state: "stopped",
        },
        Row {
            case: "stopping every instance a step naming several of them names",
            document: "stop-three-instances",
            desired_state: "stopped",
            extra_inputs: Vec::new(),
            instances: 3,
            status: "Success",
            instance_state: "stopped",
        },
        Row {
            case: "terminating the instance the step names",
            document: "terminate-one-instance",
            desired_state: "terminated",
            extra_inputs: Vec::new(),
            instances: 1,
            status: "Success",
            instance_state: "terminated",
        },
        Row {
            case: "asking for the state a launched instance is already reaching",
            document: "start-one-instance",
            desired_state: "running",
            extra_inputs: Vec::new(),
            instances: 1,
            status: "Success",
            instance_state: "running",
        },
        Row {
            case: "ASSERTING the state rather than changing it, which changes nothing",
            document: "assert-one-instance-running",
            desired_state: "running",
            extra_inputs: vec![("CheckStateOnly", Value::Bool(true))],
            instances: 1,
            status: "Success",
            instance_state: "running",
        },
        Row {
            case: "a step whose transition window runs out before the state it asserts holds",
            document: "assert-one-instance-stopped",
            desired_state: "stopped",
            extra_inputs: vec![
                ("CheckStateOnly", Value::Bool(true)),
                (
                    "DesiredStateTransitionTimeout",
                    json!(SHORT_TRANSITION_WINDOW),
                ),
            ],
            instances: 1,
            status: "Failed",
            instance_state: "running",
        },
    ]
}

/// RULE: the step's outcome is what Amazon EC2 answered about the instances it names, and the
/// state those instances are in afterwards is the state that service reports.
#[tokio::test]
async fn a_change_instance_state_step_moves_the_instances_in_amazon_ec2() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let instances = launch(&router, case, row.instances).await;
        // The launch answers the instances `pending`; the step acts on the fleet a client
        // would act on, so each case waits for that service to report them usable first.
        for instance_id in &instances {
            wait_for_instance_state(&router, case, instance_id, "running").await;
        }
        create_runbook(
            &router,
            case,
            row.document,
            &runbook(row.desired_state, &row.extra_inputs),
        )
        .await;

        let execution_id = start(&router, case, row.document, &instances).await;
        let reported = wait_for_the_outcome(&router, case, &execution_id).await;
        let status = reported["AutomationExecutionStatus"].as_str().unwrap_or_default();
        assert_eq!(
            status, row.status,
            "case {case:?}: expected the execution to end {:?}, got {status:?}: {reported}",
            row.status
        );
        for instance_id in &instances {
            let state = instance_state(&router, case, instance_id).await;
            assert_eq!(
                state, row.instance_state,
                "case {case:?}: expected Amazon EC2 to report {instance_id} {:?}, it reports \
                 {state:?}",
                row.instance_state
            );
        }
        // A step that failed says why, and a step that succeeded reports no failure at all.
        let step = reported["StepExecutions"][0].clone();
        let failure = step["FailureMessage"].as_str().unwrap_or_default();
        assert_eq!(
            failure.is_empty(),
            row.status == "Success",
            "case {case:?}: expected a failure message only where the step failed, got \
             {failure:?} for {status:?}"
        );
        assert_eq!(
            step["Action"].as_str(),
            Some("aws:changeInstanceState"),
            "case {case:?}: expected the step to report the action it ran, got {step}"
        );
    }
}

/// RULE (the other half): a step naming an instance Amazon EC2 holds none of cannot bring it
/// to any state, so the step fails with what that service said rather than waiting its window
/// out or reporting a move nothing made.
#[tokio::test]
async fn a_step_naming_an_instance_amazon_ec2_does_not_hold_fails() {
    let case = "an instance id no launch of this account issued";
    let router = emulator();
    let absent = "i-0fedcba9876543210".to_string();
    create_runbook(&router, case, "stop-an-absent-instance", &runbook("stopped", &[])).await;
    let execution_id = start(
        &router,
        case,
        "stop-an-absent-instance",
        std::slice::from_ref(&absent),
    )
    .await;
    let reported = wait_for_the_outcome(&router, case, &execution_id).await;
    assert_eq!(
        reported["AutomationExecutionStatus"].as_str(),
        Some("Failed"),
        "case {case:?}: expected the execution to fail, got {reported}"
    );
    let failure = reported["FailureMessage"].as_str().unwrap_or_default();
    assert!(
        failure.contains(&absent),
        "case {case:?}: expected the failure to name the instance it could not move, got \
         {failure:?}"
    );
}

/// RULE: `AutomationDefinitionNotFoundException` is for a runbook that does NOT exist — a
/// name no document of this endpoint carries — and is never the answer for one this account
/// just created.
#[tokio::test]
async fn a_registered_runbook_is_runnable_and_only_an_absent_one_is_not() {
    let router = emulator();
    let case = "a runbook this account registered";
    create_runbook(&router, case, "a-registered-runbook", &runbook("stopped", &[])).await;
    let instances = launch(&router, case, 1).await;
    let (status, body) = call(
        &router,
        "StartAutomationExecution",
        &json!({
            "DocumentName": "a-registered-runbook",
            "Parameters": {"Instances": instances},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the start to answer an execution id, got {status} with {body}"
    );

    let case = "a name no document of this endpoint carries";
    let absent = "no-such-runbook";
    let (status, body) = call(
        &router,
        "StartAutomationExecution",
        &json!({"DocumentName": absent}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the start to be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "AutomationDefinitionNotFoundException",
        "case {case:?}: expected the modelled not-found refusal, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(format!("Document {absent} does not exist.").as_str()),
        "case {case:?}: expected the refusal to word the missing document, got {body}"
    );
}
