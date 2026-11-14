// SPDX-License-Identifier: CC0-1.0

//! The `aws:changeInstanceState` action of an Automation runbook: the step that really
//! changes the state of the Amazon EC2 instances it names, and waits until that service
//! reports them there.
//!
//! "aws:changeInstanceState — changes or asserts the state of one or more instances" (the
//! Systems Manager Automation action reference, the same reference
//! [`super::ACTION_BRANCH`] and its siblings come from). An instance is not this service's
//! resource: it belongs to Amazon EC2, which is the only component that can move it between
//! states and the only one that can say which state it is in. So the step CALLS that
//! service's own operations — `StopInstances`, `StartInstances` or `TerminateInstances` for
//! the `DesiredState` it asks for, then `DescribeInstances` to read the state back — and
//! stays `InProgress` until that service reports every named instance at a state the step
//! accepts, which is exactly what the action's `DesiredStateTransitionTimeout` bounds.
//!
//! # Where the work happens
//!
//! The walk itself is pure computation under this store's lock ([`super`]), so it cannot
//! make a call: it records WHAT the step asked for ([`InstanceStateChange`]) and leaves the
//! step running. [`drive_due`] is the half that calls Amazon EC2 — on the operation's own
//! task, with no lock held — and feeds that service's answer back into the step through
//! [`super::AutomationExecutionRecord::record_instance_state`], which lets the walk go on
//! from where it stopped. The operations that REPORT an automation drive it, because the
//! step was due at the same instant whether or not anybody called: what the clock owed the
//! execution is materialised before anything is read, exactly as the rest of the walk is.
//!
//! Nothing here opens a socket or leaves the host: the request is signed with the credential
//! the framework hands this service and dispatched over the emulator's own in-process
//! connector, so it reaches Amazon EC2's own handlers — and the instance's state is that
//! service's to change, never written here.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::serde_json::{Map, Value};

/// The action this module runs, as the Automation action reference names it.
pub const ACTION_CHANGE_INSTANCE_STATE: &str = "aws:changeInstanceState";

/// The `DesiredState` values the action takes: "running | stopped | terminated" (same
/// reference) — each the `InstanceStateName` Amazon EC2 reports an instance resting in.
pub const DESIRED_STATE_RUNNING: &str = "running";
pub const DESIRED_STATE_STOPPED: &str = "stopped";
pub const DESIRED_STATE_TERMINATED: &str = "terminated";

/// Every value `DesiredState` admits, in the order the reference lists them — which is what
/// a step naming anything else is refused against.
pub const DESIRED_STATES: &[&str] = &[
    DESIRED_STATE_RUNNING,
    DESIRED_STATE_STOPPED,
    DESIRED_STATE_TERMINATED,
];

/// How long the step waits for the instances to reach the desired state when it names no
/// window of its own: "DesiredStateTransitionTimeout … The amount of time to complete the
/// desired state transition. Default: 3600 (seconds)" (same reference), so the value is the
/// action's documented default rather than one this emulator chose.
pub const DEFAULT_TRANSITION_TIMEOUT: Duration = Duration::from_secs(3600);

/// The `InstanceIds` input, which names the instances the step acts on (same reference).
const INPUT_INSTANCE_IDS: &str = "InstanceIds";

/// The `DesiredState` input: the state the step brings those instances to (same reference).
const INPUT_DESIRED_STATE: &str = "DesiredState";

/// The `CheckStateOnly` input: "If false, sets the instance state to the desired state. If
/// true, asserts the desired state using polling" (same reference).
const INPUT_CHECK_STATE_ONLY: &str = "CheckStateOnly";

/// The `Force` input, which forces the instances to stop (same reference).
const INPUT_FORCE: &str = "Force";

/// The `AdditionalInstanceStates` input: "Other acceptable states for the instances" beside
/// the desired one (same reference).
const INPUT_ADDITIONAL_STATES: &str = "AdditionalInstanceStates";

/// The `DesiredStateTransitionTimeout` input, in seconds (same reference).
const INPUT_TRANSITION_TIMEOUT: &str = "DesiredStateTransitionTimeout";

/// How many instances one step names here.
///
/// The emulator's own guard, not an AWS quota: the ids travel as one Amazon EC2 request and
/// the states come back as one answer, so this bounds what a runbook's content can make one
/// call read however many references it carries.
pub const MAX_INSTANCES_PER_STEP: usize = 200;

/// What one `aws:changeInstanceState` step asked Amazon EC2 for, and how far that service
/// has got with it.
///
/// Written when the walk enters the step — resolved from the step's own inputs, so a
/// `{{ … }}` reference is already the value it stands for — and read by [`drive_due`], which
/// is what makes the call and records the answer.
#[derive(Debug, Clone)]
pub struct InstanceStateChange {
    /// The instances the step names, in the order it names them.
    pub instance_ids: Vec<String>,
    /// The state the step brings them to: one of [`DESIRED_STATES`].
    pub desired_state: String,
    /// The other states the step accepts as having reached it.
    pub additional_states: Vec<String>,
    /// Whether the step only ASSERTS the state rather than changing it.
    pub check_state_only: bool,
    /// Whether a stop is forced.
    pub force: bool,
    /// How long the step waits for the transition.
    pub transition_timeout: Duration,
    /// Whether the state change has already been asked of Amazon EC2: the change is asked
    /// for ONCE, and everything after it is that service's own transition.
    pub asked: bool,
    /// Whether that service reports every named instance at a state the step accepts.
    pub settled: bool,
    /// Why the step cannot be completed, when Amazon EC2 refused the call or holds no such
    /// instance — the walk fails the step with it.
    pub failure: Option<String>,
}

impl InstanceStateChange {
    /// Whether `state` is a state the step counts as having reached: the `DesiredState`
    /// itself, or one of the `AdditionalInstanceStates` the step also accepts.
    pub fn accepts(&self, state: &str) -> bool {
        self.desired_state == state
            || self
                .additional_states
                .iter()
                .any(|accepted| accepted == state)
    }

    /// The Amazon EC2 operation that brings an instance to this step's desired state, or
    /// `None` for a step that only asserts the state.
    fn operation(&self) -> Option<&'static str> {
        if self.check_state_only {
            return None;
        }
        match self.desired_state.as_str() {
            DESIRED_STATE_RUNNING => Some(START_INSTANCES),
            DESIRED_STATE_STOPPED => Some(STOP_INSTANCES),
            DESIRED_STATE_TERMINATED => Some(TERMINATE_INSTANCES),
            _ => None,
        }
    }
}

/// What one step's inputs ask for, or why they are not a state change this action can run.
///
/// `InstanceIds` and `DesiredState` are the action's required inputs, so a step naming
/// neither names nothing to act on; a `DesiredState` outside the documented set names no
/// state Amazon EC2 holds an instance in. The failure is the STEP's, as every other
/// unrunnable step's is.
pub fn change_of(
    inputs: &Map<String, Value>,
    step_name: &str,
) -> Result<InstanceStateChange, String> {
    let instance_ids = text_list(inputs.get(INPUT_INSTANCE_IDS));
    if instance_ids.is_empty() {
        return Err(format!(
            "Step {step_name} declares no {INPUT_INSTANCE_IDS} to change the state of"
        ));
    }
    if instance_ids.len() > MAX_INSTANCES_PER_STEP {
        return Err(format!(
            "Step {step_name} names {} instances; a step of this emulator names at most \
             {MAX_INSTANCES_PER_STEP}",
            instance_ids.len()
        ));
    }
    let desired_state = inputs
        .get(INPUT_DESIRED_STATE)
        .map(text_of)
        .unwrap_or_default();
    if !DESIRED_STATES.contains(&desired_state.as_str()) {
        return Err(format!(
            "The {INPUT_DESIRED_STATE} {desired_state:?} of step {step_name} is not one of \
             [{}]",
            DESIRED_STATES.join(", ")
        ));
    }
    let transition_timeout = match inputs.get(INPUT_TRANSITION_TIMEOUT) {
        None => DEFAULT_TRANSITION_TIMEOUT,
        Some(declared) => match whole_seconds(declared) {
            Some(seconds) => Duration::from_secs(seconds),
            None => {
                return Err(format!(
                    "The {INPUT_TRANSITION_TIMEOUT} of step {step_name} is not a number of \
                     seconds"
                ))
            }
        },
    };
    Ok(InstanceStateChange {
        instance_ids,
        desired_state,
        additional_states: text_list(inputs.get(INPUT_ADDITIONAL_STATES)),
        check_state_only: truth(inputs.get(INPUT_CHECK_STATE_ONLY)),
        force: truth(inputs.get(INPUT_FORCE)),
        transition_timeout,
        asked: false,
        settled: false,
        failure: None,
    })
}

/// How the walk words the step's own timeout: the window the action asked for has passed and
/// the instances are not at the state it asked for, which is the failure the action's
/// `DesiredStateTransitionTimeout` names.
pub fn transition_timed_out_message(change: &InstanceStateChange, step_name: &str) -> String {
    format!(
        "Step {step_name} waited {} seconds for {} to reach the state {} and Amazon EC2 did \
         not report them there",
        change.transition_timeout.as_secs(),
        change.instance_ids.join(", "),
        change.desired_state
    )
}

/// What Amazon EC2 answered about one step's instances.
#[derive(Debug, Clone)]
pub enum Answered {
    /// The change was asked of that service, which has not reported the instances at the
    /// state yet: the step goes on waiting for the transition.
    Asked,
    /// Every named instance is at a state the step accepts.
    Settled,
    /// That service refused the call, or holds no instance of a named id: the step fails
    /// with this, as the action does on AWS.
    Failed(String),
}

/// How many steps one call drives.
///
/// Each one is a call to another service, so this bounds the work one request does however
/// many executions this store holds — the rest are driven by the next report, exactly as the
/// rest of the walk is.
pub const MAX_STEPS_DRIVEN_PER_CALL: usize = 8;

/// Amazon EC2's signing name and the API version its awsQuery wire names beside the action,
/// from that service's own model.
const EC2_SIGNING_NAME: &str = "ec2";
const EC2_API_VERSION: &str = "2016-11-15";

/// The Amazon EC2 operations this module asks for, by name: the three that move an instance
/// to one of the documented desired states, and the one that reports which state it is in.
const START_INSTANCES: &str = "StartInstances";
const STOP_INSTANCES: &str = "StopInstances";
const TERMINATE_INSTANCES: &str = "TerminateInstances";
const DESCRIBE_INSTANCES: &str = "DescribeInstances";

/// The awsQuery spelling of `InstanceIds` on each of those calls: that member is flattened
/// and named `InstanceId` on the wire (`StopInstancesRequest$InstanceIds`), so the ids travel
/// as `InstanceId.1`, `InstanceId.2`, … .
const INSTANCE_ID_PARAMETER: &str = "InstanceId";

/// The media type an awsQuery request carries: its parameters are a form body, not a
/// document.
const FORM_URLENCODED: &str = "application/x-www-form-urlencoded";

/// How many bytes of Amazon EC2's answer this module reads.
///
/// The EMULATOR's own bound: the answer is a handful of members per instance, and the ids
/// are bounded by [`MAX_INSTANCES_PER_STEP`], so what a read of it costs is fixed here
/// rather than by whatever that service answers. A longer answer is not read at all — it
/// fails the step, naming its size.
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

/// Drive every `aws:changeInstanceState` step of this Region that is waiting on Amazon EC2.
///
/// Called by the operations that report an automation, BEFORE they read the store: the step
/// was owed at the same instant whether or not anybody called, so what the clock owes the
/// execution — the steps of its walk and the calls one of those steps makes — is
/// materialised first. Nothing is reported from here: the step's own record is what a read
/// answers from.
pub async fn drive_due<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    let region = crate::state::request_region(ctx.parts());
    // A step this call has already asked about is not asked again: the instances it waits
    // for reach the state on Amazon EC2's own clock, which a second call in this request
    // would not move.
    let mut driven: Vec<(String, String)> = Vec::new();
    for _ in 0..MAX_STEPS_DRIVEN_PER_CALL {
        let Some((execution_id, step_execution_id, change)) =
            next_waiting(&state, &region, &driven, SystemTime::now())
        else {
            return;
        };
        driven.push((execution_id.clone(), step_execution_id.clone()));
        let answered = perform(ctx, &region, &change).await;
        let now = SystemTime::now();
        let mut store = state.lock();
        if let Some(execution) = store.automation_execution_mut(&region, &execution_id) {
            execution.record_instance_state(&step_execution_id, answered, now);
        }
    }
}

/// The next step of `region` that is waiting on Amazon EC2, as
/// `(execution id, step execution id, what it asked for)`.
///
/// Everything the clock owes is materialised first, so a step that has only just come due is
/// among the ones this reports. `driven` names the steps this call has already asked about.
fn next_waiting(
    state: &::engine::ServiceState<crate::state::ComAmazonawsSsmAmazonssmState>,
    region: &str,
    driven: &[(String, String)],
    now: SystemTime,
) -> Option<(String, String, InstanceStateChange)> {
    let mut store = state.lock();
    store.materialize_due(now);
    for execution in store.automation_executions_in_order(region) {
        // An execution that has ended asks for nothing more: a step of one is at the
        // outcome the walk recorded for it.
        if execution.ended() {
            continue;
        }
        for step in &execution.steps {
            let Some(change) = step.instance_state.as_ref() else {
                continue;
            };
            if change.settled || change.failure.is_some() {
                continue;
            }
            let key = (
                execution.execution_id.clone(),
                step.step_execution_id.clone(),
            );
            if driven.contains(&key) {
                continue;
            }
            return Some((key.0, key.1, change.clone()));
        }
    }
    None
}

/// Ask Amazon EC2 for the state change the step wants, then read back what state it reports
/// the named instances in.
///
/// The change is asked for once ([`InstanceStateChange::asked`]) and the read happens every
/// time: the transition is that service's, so what ends the step's wait is its own answer.
async fn perform(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    change: &InstanceStateChange,
) -> Answered {
    if !change.asked {
        if let Some(operation) = change.operation() {
            let mut parameters = instance_id_parameters(&change.instance_ids);
            // "Force … Forces the instances to stop", which that call takes as its own
            // member (`StopInstancesRequest$Force`).
            if change.force && operation == STOP_INSTANCES {
                parameters.push(("Force".to_string(), "true".to_string()));
            }
            if let Err(why) = call_ec2(ctx, region, operation, &parameters).await {
                return Answered::Failed(why);
            }
        }
    }
    let reported = match call_ec2(
        ctx,
        region,
        DESCRIBE_INSTANCES,
        &instance_id_parameters(&change.instance_ids),
    )
    .await
    {
        Ok(answer) => reported_states(&answer),
        Err(why) => return Answered::Failed(why),
    };
    for instance_id in &change.instance_ids {
        match reported.get(instance_id) {
            // An instance Amazon EC2 reports nothing about is one the step cannot bring to
            // any state, which is a failure of the step rather than a wait with no end.
            None => {
                return Answered::Failed(format!(
                    "Amazon EC2 reports no state for instance {instance_id}"
                ))
            }
            Some(state) if change.accepts(state) => {}
            // The instance is on its way there (`stopping`, `pending`, `shutting-down`), so
            // the step goes on waiting for that service's own transition.
            Some(_) => return Answered::Asked,
        }
    }
    Answered::Settled
}

/// The ids as the awsQuery parameters of one of those calls: `InstanceId.1`, `InstanceId.2`,
/// … , which is how that flattened member travels.
fn instance_id_parameters(instance_ids: &[String]) -> Vec<(String, String)> {
    instance_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                format!("{INSTANCE_ID_PARAMETER}.{}", index + 1),
                id.clone(),
            )
        })
        .collect()
}

/// Call one Amazon EC2 operation on the awsQuery wire its model declares, in process, and
/// read the XML document it answered with. `Err` is what the step failed for: that service's
/// own refusal, or a call this process could not make.
///
/// The emulator verifies every request signature, so the call carries one: the credential the
/// framework hands this service, signed through the tree's one in-process signer
/// (`service_common::in_process_sigv4`), which is never reported or logged. The connector is
/// the emulator's own, so nothing is dialled and no name is resolved.
async fn call_ec2(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    parameters: &[(String, String)],
) -> Result<String, String> {
    let mut body = format!("Action={operation}&Version={EC2_API_VERSION}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&::percent_encoding::utf8_percent_encode(name, FORM_VALUE).to_string());
        body.push('=');
        body.push_str(&::percent_encoding::utf8_percent_encode(value, FORM_VALUE).to_string());
    }
    let host = format!("{EC2_SIGNING_NAME}.{region}.amazonaws.com");
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("content-type", FORM_URLENCODED)
        .body(body.into_bytes())
        .map_err(|error| error.to_string())?;
    ::service_common::in_process_sigv4::sign_in_process_request(
        &mut request,
        EC2_SIGNING_NAME,
        region,
        ctx,
    )
    .await?;
    let (parts, body) = request.into_parts();
    let request = ::aws_smithy_runtime_api::http::Request::try_from(::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    ))
    .map_err(|error| error.to_string())?;
    let connector = ctx.callback().http_connector();
    let answer = {
        use ::aws_smithy_runtime_api::client::http::HttpConnector;
        HttpConnector::call(&connector, request)
            .await
            .map_err(|error| error.to_string())?
    };
    let status = answer.status().as_u16();
    let payload = answer.body().bytes().unwrap_or_default();
    // An answer past the cap is one this reader does not read: the bytes beyond it are
    // dropped, and a document the drop cut in half would be read as an answer carrying
    // nothing — so the step is told the answer was too long rather than told the instances
    // have no state.
    if payload.len() > MAX_ANSWER_BYTES {
        return Err(format!(
            "Amazon EC2 answered operation '{operation}' with {} bytes, more than the \
             {MAX_ANSWER_BYTES} this step reads",
            payload.len()
        ));
    }
    let document = String::from_utf8_lossy(payload).to_string();
    if (200..300).contains(&status) {
        return Ok(document);
    }
    // That service's own refusal, reported beside the operation it refused: the step's
    // failure message says both which call failed and why.
    Err(format!(
        "Error occurred during operation '{operation}': {}",
        refusal_message(&document)
    ))
}

/// Everything outside RFC 3986's unreserved set, which is what an AWS SDK escapes in an
/// awsQuery parameter and what the SigV4 canonical request assumes.
const FORM_VALUE: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The message an awsQuery refusal carries: its `Error`'s own `Message`, or the whole
/// document's `Code` when it words no message.
fn refusal_message(document: &str) -> String {
    let message = xml_text(document, "Message");
    if !message.is_empty() {
        return message;
    }
    let code = xml_text(document, "Code");
    if code.is_empty() {
        "Amazon EC2 refused the call".to_string()
    } else {
        code
    }
}

/// The state Amazon EC2 reports each described instance in, keyed by instance id.
///
/// `DescribeInstances` answers reservations holding `instancesSet` items, each carrying its
/// own `instanceId` and the `instanceState` structure whose `name` is the `InstanceStateName`
/// — so an element is read as an instance only when it carries both.
fn reported_states(document: &str) -> BTreeMap<String, String> {
    let Ok(tree) = ::roxmltree::Document::parse(document) else {
        return BTreeMap::new();
    };
    tree.descendants()
        .filter_map(|element| {
            let instance_id = child_text(element, "instanceId")?;
            let state = element
                .children()
                .find(|child| child.has_tag_name("instanceState"))
                .and_then(|state| child_text(state, "name"))?;
            Some((instance_id, state))
        })
        .collect()
}

/// The text of `element`'s own child named `name`, when it has one carrying any.
fn child_text(element: ::roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    element
        .children()
        .find(|child| child.has_tag_name(name))
        .and_then(|child| child.text())
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

/// The text of the FIRST element named `name` anywhere in `document`, for the two members a
/// refusal is read from. Empty for a document carrying none.
fn xml_text(document: &str, name: &str) -> String {
    let Ok(tree) = ::roxmltree::Document::parse(document) else {
        return String::new();
    };
    tree.descendants()
        .find(|element| element.has_tag_name(name))
        .and_then(|element| element.text())
        .unwrap_or_default()
        .to_string()
}

/// One input as the text it stands for: a resolved input is a scalar or the document it is,
/// and the action's own inputs are scalars and lists of them.
fn text_of(value: &Value) -> String {
    super::runbook::rendered(value)
}

/// One input as the list of values it names: the list a `StringList` input carries, the one
/// value a scalar names, or the comma-separated values a resolved reference to a `StringList`
/// parameter leaves behind — in a list entry as well as on its own, since a list position is
/// where a runbook writes such a reference
/// (`super::AutomationExecutionRecord::variables`, which joins such a parameter's values with
/// commas, so one entry carrying them all stands for every one of them).
fn text_list(value: Option<&Value>) -> Vec<String> {
    let listed: Vec<String> = match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => values.iter().map(text_of).collect(),
        Some(other) => vec![text_of(other)],
    };
    listed
        .iter()
        .flat_map(|entry| entry.split(','))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

/// One input as the truth it states: a boolean, or the text a template or a resolved
/// reference spells one as.
fn truth(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(truth)) => *truth,
        Some(other) => text_of(other).eq_ignore_ascii_case("true"),
        None => false,
    }
}

/// One input as a whole number of seconds, in either spelling: the number it is, or the text
/// a quoted scalar leaves behind. `None` for a value that is neither.
fn whole_seconds(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inputs of a step, as the walk resolves them before the action runs.
    fn inputs(members: &[(&str, Value)]) -> Map<String, Value> {
        members
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    /// RULE: a step's `InstanceIds` names the instances it acts on, in every spelling a
    /// resolved input carries them in; a step that names none, or names a `DesiredState`
    /// outside the documented set, declares no state change this action can run.
    #[test]
    fn a_steps_inputs_name_the_instances_and_the_state_it_brings_them_to() {
        /// (case, the inputs, the ids it names, the state it asks for)
        type Row<'a> = (&'a str, Map<String, Value>, Vec<&'a str>, &'a str);
        let rows: Vec<Row<'_>> = vec![
            (
                "one instance named as a list, the shape the input is modelled",
                inputs(&[
                    (INPUT_INSTANCE_IDS, serde_json::json!(["i-0123456789abcdef0"])),
                    (INPUT_DESIRED_STATE, Value::String("stopped".to_string())),
                ]),
                vec!["i-0123456789abcdef0"],
                DESIRED_STATE_STOPPED,
            ),
            (
                "several instances, in the order the step names them",
                inputs(&[
                    (
                        INPUT_INSTANCE_IDS,
                        serde_json::json!(["i-aaa", " i-bbb ", "i-ccc"]),
                    ),
                    (INPUT_DESIRED_STATE, Value::String("running".to_string())),
                ]),
                vec!["i-aaa", "i-bbb", "i-ccc"],
                DESIRED_STATE_RUNNING,
            ),
            (
                "the comma-separated text a resolved StringList reference leaves behind",
                inputs(&[
                    (
                        INPUT_INSTANCE_IDS,
                        Value::String("i-aaa,i-bbb".to_string()),
                    ),
                    (INPUT_DESIRED_STATE, Value::String("terminated".to_string())),
                ]),
                vec!["i-aaa", "i-bbb"],
                DESIRED_STATE_TERMINATED,
            ),
            (
                "that same reference written where a runbook writes it: one list entry",
                inputs(&[
                    (
                        INPUT_INSTANCE_IDS,
                        serde_json::json!(["i-aaa,i-bbb,i-ccc"]),
                    ),
                    (INPUT_DESIRED_STATE, Value::String("stopped".to_string())),
                ]),
                vec!["i-aaa", "i-bbb", "i-ccc"],
                DESIRED_STATE_STOPPED,
            ),
        ];
        for (case, inputs, ids, state) in rows {
            let change = change_of(&inputs, "stop").unwrap_or_else(|why| {
                panic!("case {case:?}: expected a runnable step, got {why:?}")
            });
            assert_eq!(
                change.instance_ids, ids,
                "case {case:?}: expected {ids:?}, got {:?}",
                change.instance_ids
            );
            assert_eq!(
                change.desired_state, state,
                "case {case:?}: expected {state:?}, got {:?}",
                change.desired_state
            );
            assert_eq!(
                change.transition_timeout, DEFAULT_TRANSITION_TIMEOUT,
                "case {case:?}: expected the documented default window, got {:?}",
                change.transition_timeout
            );
        }
    }

    /// RULE: a step whose inputs are not a state change the action can run fails the STEP,
    /// with a message naming what is wrong with them.
    #[test]
    fn inputs_that_are_no_state_change_fail_the_step() {
        /// (case, the inputs)
        type Row<'a> = (&'a str, Map<String, Value>);
        let rows: Vec<Row<'_>> = vec![
            (
                "no instance at all",
                inputs(&[(INPUT_DESIRED_STATE, Value::String("stopped".to_string()))]),
            ),
            (
                "an empty instance list",
                inputs(&[
                    (INPUT_INSTANCE_IDS, serde_json::json!([])),
                    (INPUT_DESIRED_STATE, Value::String("stopped".to_string())),
                ]),
            ),
            (
                "no desired state",
                inputs(&[(INPUT_INSTANCE_IDS, serde_json::json!(["i-aaa"]))]),
            ),
            (
                "a state no instance rests in",
                inputs(&[
                    (INPUT_INSTANCE_IDS, serde_json::json!(["i-aaa"])),
                    (INPUT_DESIRED_STATE, Value::String("rebooted".to_string())),
                ]),
            ),
            (
                "a transition window that is no number of seconds",
                inputs(&[
                    (INPUT_INSTANCE_IDS, serde_json::json!(["i-aaa"])),
                    (INPUT_DESIRED_STATE, Value::String("stopped".to_string())),
                    (INPUT_TRANSITION_TIMEOUT, Value::String("soon".to_string())),
                ]),
            ),
            (
                "more instances than one step names",
                inputs(&[
                    (
                        INPUT_INSTANCE_IDS,
                        Value::Array(
                            (0..=MAX_INSTANCES_PER_STEP)
                                .map(|index| Value::String(format!("i-{index:017x}")))
                                .collect(),
                        ),
                    ),
                    (INPUT_DESIRED_STATE, Value::String("stopped".to_string())),
                ]),
            ),
        ];
        for (case, inputs) in rows {
            let answered = change_of(&inputs, "stop");
            assert!(
                answered.is_err(),
                "case {case:?}: expected a refused step, got {:?}",
                answered.map(|change| change.desired_state)
            );
        }
    }

    /// RULE: the step counts an instance as having reached the state when Amazon EC2 reports
    /// it at the desired state or at one of the other states the step accepts.
    #[test]
    fn a_step_accepts_the_desired_state_and_the_states_it_also_names() {
        let change = change_of(
            &inputs(&[
                (INPUT_INSTANCE_IDS, serde_json::json!(["i-aaa"])),
                (INPUT_DESIRED_STATE, Value::String("stopped".to_string())),
                (
                    INPUT_ADDITIONAL_STATES,
                    serde_json::json!(["terminated"]),
                ),
                (INPUT_CHECK_STATE_ONLY, Value::Bool(true)),
            ]),
            "assert",
        )
        .expect("a runnable step");
        for (case, state, accepted) in [
            ("the desired state", "stopped", true),
            ("a state the step also accepts", "terminated", true),
            ("a state on the way there", "stopping", false),
            ("the state it started in", "running", false),
        ] {
            assert_eq!(
                change.accepts(state),
                accepted,
                "case {case:?}: expected accepts({state:?}) to be {accepted}, got {}",
                change.accepts(state)
            );
        }
        assert!(
            change.operation().is_none(),
            "case \"a step that only asserts the state\": expected no call to Amazon EC2, \
             got {:?}",
            change.operation()
        );
    }

    /// RULE: the state of each described instance is read out of that instance's own
    /// `instanceState`, so an element elsewhere in the answer is not mistaken for one.
    #[test]
    fn the_described_state_is_read_from_the_instances_own_state() {
        let answer = r#"<DescribeInstancesResponse><reservationSet><item>
            <instancesSet>
              <item><instanceId>i-aaa</instanceId>
                <instanceState><code>80</code><name>stopped</name></instanceState>
                <tagSet><item><key>Name</key><value>probe</value></item></tagSet></item>
              <item><instanceId>i-bbb</instanceId>
                <instanceState><code>64</code><name>stopping</name></instanceState></item>
            </instancesSet>
        </item></reservationSet></DescribeInstancesResponse>"#;
        let reported = reported_states(answer);
        for (case, instance_id, state) in [
            ("an instance at rest", "i-aaa", Some("stopped")),
            ("an instance in transition", "i-bbb", Some("stopping")),
            ("an instance the answer does not carry", "i-ccc", None),
        ] {
            let actual = reported.get(instance_id).map(String::as_str);
            assert_eq!(
                actual, state,
                "case {case:?}: expected {state:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a refusal's message is the one the called service worded, and a refusal that
    /// words none is reported by its code.
    #[test]
    fn a_refusal_is_reported_by_what_the_called_service_said() {
        for (case, document, expected) in [
            (
                "an awsQuery refusal",
                r#"<Response><Errors><Error><Code>InvalidInstanceID.NotFound</Code>
                   <Message>The instance ID 'i-aaa' does not exist</Message></Error></Errors>
                   </Response>"#,
                "The instance ID 'i-aaa' does not exist",
            ),
            (
                "a refusal that words no message",
                "<Response><Errors><Error><Code>RequestLimitExceeded</Code></Error></Errors>\
                 </Response>",
                "RequestLimitExceeded",
            ),
            ("no document at all", "", "Amazon EC2 refused the call"),
        ] {
            let actual = refusal_message(document);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
