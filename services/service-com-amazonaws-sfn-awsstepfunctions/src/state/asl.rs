// SPDX-License-Identifier: CC0-1.0

//! The Amazon States Language (ASL) this emulator EXECUTES.
//!
//! ASL is Step Functions' own definition language; no crates.io crate runs it, so
//! the walk below is written here, over `serde_json` — the maintained parser for the
//! JSON the language is written in — and the two query languages its `QueryLanguage`
//! member selects between, each evaluated by the maintained implementation of it:
//! [`serde_json_path`], the RFC 9535 implementation of JSONPath, which is the
//! language every `*Path` member and every `"<name>.$"` payload-template key is
//! written in, and [`super::jsonata`] for JSONata, which is the language a JSONata
//! state's `Output`, `Assign`, `Arguments`, `Items` and `Condition` are written in.
//!
//! ## What is executed, and what is not
//!
//! Every state whose semantics are a function of the execution's own data is RUN,
//! not faked: `Pass`, `Wait`, `Choice`, `Succeed`, `Fail`, `Map` (inline and
//! Distributed) and `Parallel`, together with the data-flow members that decide what
//! each of them reads and writes (`InputPath`, `Parameters`, `ItemSelector`,
//! `ItemsPath`, `ResultSelector`, `ResultPath`, `OutputPath`, and the `Arguments`,
//! `Output` and `Assign` of a JSONata state), the context object
//! (`$$`), the intrinsic functions, `Retry` and `Catch`. A state is evaluated in the
//! query language IT declares, else the one its definition does, so a JSONata state
//! anywhere in a JSONPath machine — inside a `Parallel` branch or a `Map`'s
//! processor included — is evaluated with that language's semantics.
//!
//! A `Map` state's items come from the DATASET its `ItemReader` names when it declares
//! one — read out of S3 for real ([`super::item_reader`]) — else from its own input;
//! an `ItemBatcher` then groups them into the batches its iterations are handed
//! ([`super::item_batcher`]). A Distributed Map's `ResultWriter` WRITES what its items
//! produced back to S3, just as really ([`super::result_writer`]), and the state then
//! puts out where they were written.
//!
//! A `Task` state's work is RUN too, in the one way this process can run each kind:
//!
//! * a `Resource` naming a service INTEGRATION is called — the call goes to the
//!   other service's own handler in this same emulator, with the document the state
//!   built as that operation's request and the operation's response as the state's
//!   result (see [`super::integration`], which also states which integrations are
//!   made and which are not). A `Resource` asking for the run-a-job (`.sync`)
//!   pattern is called the same way, and the task is then PARKED at the job that
//!   call started ([`ParkedJob`]) until the invoked service reports it finished,
//!   which is what resolves the task ([`super::run_a_job`]);
//! * a `Resource` asking for the callback (`.waitForTaskToken`) pattern is called
//!   the same way too, carrying the `taskToken` the state's parameters read out of
//!   the context object, and the walk then SUSPENDS at that token
//!   ([`Run::waiting`]) until a `SendTaskSuccess` or `SendTaskFailure` quotes it,
//!   which is what resolves the task;
//! * a `Resource` naming an ACTIVITY ARN belongs to a worker that polls this
//!   emulator, so the state SCHEDULES a task carrying the state's effective input and
//!   the walk SUSPENDS there ([`Run::waiting`]), which is the execution waiting for a
//!   worker exactly as it does on the real service. `GetActivityTask` hands that task
//!   out; the worker's reply closes it.
//!
//! Two things decide whether that call is made at all, and both answer before it
//! would be: the WORKFLOW TYPE, since the run-a-job and callback patterns are served
//! for `STANDARD` only and an EXPRESS execution fails at the state that asks for one
//! ([`EXPRESS_RUN_A_JOB_UNSUPPORTED`], [`EXPRESS_CALLBACK_UNSUPPORTED`]); and what
//! the state machine's EXECUTION ROLE
//! may do, since the call is made as that role and one its policies do not allow
//! fails the task with the invoked service's own refusal
//! ([`super::task_authorization`], asked through [`Tasks::authorize`]). Neither
//! starts anything first.
//!
//! Every state records the pair of events the language names it with — entered, and
//! left — each carrying the state's own data: the document it was entered WITH, and
//! the one it EMITTED once its result-side filters had been applied, which is what a
//! caller reconstructs an execution's data flow from.
//!
//! What is NOT executed is what cannot be executed inside this process:
//!
//! * an integration [`super::integration::read`] makes no call for — a service whose
//!   request is not one JSON document,
//!   a run-a-job pattern over an integration whose job cannot be reported or
//!   stopped, a `Resource` naming no integration at all;
//! * a dataset a Distributed Map's `ItemReader` names that this emulator does not read
//!   (an `InputType` outside `CSV`/`JSON`/`JSONL`), and a `ResultWriter` asking for a
//!   dataset it does not write (a `WriterConfig` outside the `JSON` output written with
//!   no transformation) or declared by a `Map` state that starts no Map Run to write the
//!   results of — the datasets it DOES read and write are read from and written
//!   to S3 for real ([`super::item_reader`], [`super::result_writer`]).
//!
//! Reaching one of those ends the execution in the service's own failure state:
//! `FAILED`, with the ASL-documented [`RUNTIME_ERROR`] name and a cause that says
//! exactly what was not run. Nothing is synthesised to look as though it ran.
//!
//! ## What is validated
//!
//! A definition is VALIDATED in both of the language's query languages, whether or
//! not this emulator evaluates its expressions: [`diagnose`] reports every problem
//! one carries — the errors that would stop `CreateStateMachine` from accepting it
//! and the warnings that would not — and [`validate`] is the first of those errors,
//! which is what the create and the update refuse a definition with. A definition
//! this emulator cannot RUN is still valid ASL, so it is created and the refusal
//! surfaces at execution time instead.
//!
//! ## What bounds a run
//!
//! Every input the walk reads — the definition, the execution input, each path and
//! each intrinsic invocation inside them — is a caller's, and the definition is
//! bounded only at the modelled 1 MiB, so each dimension of a run carries its own
//! ceiling and a run past one ends in a modelled failure rather than in the thread's
//! stack or the process's memory:
//!
//! * [`MAX_STATE_TRANSITIONS`] — steps, spent by every nested iteration, every
//!   branch and every retry, and charged BEFORE an iteration's own work is done;
//! * [`MAX_PAYLOAD_BYTES`] and [`MAX_PAYLOAD_DEPTH`] — what one state may produce,
//!   measured at EVERY state, as a `Map`'s result array grows, as a payload template
//!   renders each member, and over an intrinsic's arguments and its result, so the
//!   PRODUCT of a template's member count and each member's size is bounded as the
//!   members are built rather than after;
//! * [`MAX_NESTING_DEPTH`] — nested `Map`/`Parallel` levels;
//! * [`MAX_EXPRESSION_DEPTH`] and [`MAX_EXPRESSION_CHARS`] — one intrinsic
//!   invocation, whose nesting lives inside a single JSON string, and one `Choice`
//!   rule's `And`/`Or`/`Not` condition tree;
//! * [`MAX_PATH_CHARS`] and [`MAX_PATH_SEGMENTS`] — one path, checked BEFORE the
//!   JSONPath engine evaluates it, since a node list it has already built cannot be
//!   refused after the fact. Every path must be a REFERENCE path, which identifies
//!   one node, so the cost of an evaluation is its segment count rather than the
//!   size of the document;
//! * [`MAX_CHOICE_RULES`] and [`MAX_TEMPLATE_PATHS`] — how many expressions ONE
//!   visit to a state may evaluate, and [`MAX_GLOB_CHARS`] what one `StringMatches`
//!   may cost;
//! * [`MAX_INTERPRET_WORK`] — the WORK the run may do, checked at every transition
//!   and every `Map` item and charged by the primitives that spend it, which bounds
//!   the PRODUCT of all of the above without putting a clock between a definition and
//!   its own outcome ([`work_meter`]); the walk runs on a blocking thread, so a run
//!   that reaches it never held an async worker;
//! * [`MAX_STATE_NAME_CHARS`] — a state's name, which every history event of that
//!   state carries, enforced by [`validate`] over every state list the definition
//!   declares (nested processors and branches included) and again in the walk.
//!
//! The two walks over caller-shaped data that could otherwise recurse as deeply as
//! the data nests — measuring a document and assigning through a reference path — are
//! written ITERATIVELY.
//!
//! A Distributed Map's items are interpreted in this process, by the same walk that
//! runs an inline Map: the state's output is therefore computed from the items the
//! definition selected. Each Distributed Map state the walk entered is reported as a
//! [`StartedMapRun`], which the call that started the execution stores as a Map Run
//! of its own, and each of that state's items is reported as the child EXECUTION it
//! started ([`StartedChildExecution`]): one per item, carrying the `ItemSelector`
//! projection as its own input and the item's own result as its output, under an ARN
//! of its own that the item's states read through `$$.Execution.Id`. Those starts are
//! made by the service as the state machine's execution role, so a role that may not
//! make them is a Map Run that fails having started none ([`RUNTIME_ERROR`], carrying
//! the refusal the start came back with). An
//! INLINE Map starts neither, exactly as the service starts neither for one.
//!
//! A walk that CONTINUES an attempt of an execution — which is what a redrive of a
//! failed Map Run's parent is — enters the same `Map` state again as the SAME Map Run,
//! and the items whose child execution already succeeded are not run a second time:
//! "Step Functions preserves the results and execution history of the successful steps,
//! and doesn't rerun these steps" ([`super::integration::ResumedMapRun`]).
//!
//! ## Time
//!
//! The walk never sleeps. Every `Wait` it passes adds to the offset at which the
//! outcome becomes due ([`Run::elapsed`]), which `StartExecution` parks in a
//! `PendingOutcome`; the states of a `Map` iteration or a `Parallel` branch run
//! concurrently on the real service, so a batch's span is its longest member's
//! rather than the sum.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use serde_json_path::JsonPath;

use super::integration::{Failed, Tasks, TASK_FAILED_ERROR};

mod diagnose;
mod echo;
mod intrinsics;
mod jsonpath;
mod walk;
mod work_events;
mod work_meter;
pub use diagnose::*;
// The ceiling on the PRODUCT the ceilings below bound factor by factor, and the
// primitives that spend it: one concern in a file of its own, whose names stay
// reachable unqualified from here and from the walk, and which the OTHER query
// language's evaluator charges its expressions to ([`super::jsonata`]).
pub use work_meter::*;
// The OTHER query language's own evaluation — one path, one payload template, one
// `Choice` comparator — is this module's too, in a file of its own, and its names stay
// reachable unqualified from here and from the walk ([`jsonpath`]), exactly as the two
// below are.
use jsonpath::*;
// How a document a walk reports is ENCODED, and the intrinsic-function evaluator, are
// this module's own: each holds one concern in its own file, and the names stay
// reachable unqualified from here and from the walk, as they were when they lived in
// this file.
use echo::*;
use intrinsics::*;
use work_events::*;

/// The ASL predefined error name for a failure the interpreter itself raises
/// ("States.Runtime — an execution failed due to some exception that could not be
/// processed").
///
/// It is the name an INTRINSIC-function invocation that could not be evaluated
/// carries too — a definition the walk could not run, exactly as the rest of this
/// name's failures are — and it is TERMINAL: it is neither retried nor caught
/// ([`errors_match`]), so a definition cannot recover from one.
pub const RUNTIME_ERROR: &str = "States.Runtime";

/// The `Resource` of the optimized integration that STARTS an execution of a state
/// machine, which is the call a Distributed Map's child workflow executions are
/// started by: it names the access request such a start is subject to
/// ([`super::integration::TARGET_RESOURCES`]), so the grant the service needs to start
/// a child is read in exactly one place.
const CHILD_START_RESOURCE: &str = "arn:aws:states:::states:startExecution";

/// The ASL predefined error name for a payload that outgrew what a state may carry
/// ("States.DataLimitExceeded").
pub const DATA_LIMIT_ERROR: &str = "States.DataLimitExceeded";

/// The ASL predefined error name for a `ResultPath` that is not a reference path this
/// service follows at all ("States.ResultPathMatchFailure").
pub const RESULT_PATH_ERROR: &str = "States.ResultPathMatchFailure";

/// The ASL predefined error name for a reference path that CANNOT BE APPLIED to the
/// document it names a place in ("States.ReferencePathConflict").
///
/// A `ResultPath` is a reference path the service places the state's result at, and a
/// path whose own segments conflict with the document it is applied to — a member of a
/// value that is not an object, an index of a value that is not an array — names no
/// place in it. That is the conflict this name reports, and it is what a definition's
/// `Retry` and `Catch` answer for by name; a path that is not a reference path at all
/// is a different failure ([`RESULT_PATH_ERROR`]), since nothing was applied to
/// anything.
pub const REFERENCE_PATH_CONFLICT_ERROR: &str = "States.ReferencePathConflict";

/// The ASL predefined error name for a `Parameters`/`ItemSelector` payload template
/// whose path could not be resolved ("States.ParameterPathFailure").
pub const PARAMETER_PATH_ERROR: &str = "States.ParameterPathFailure";

/// The ASL predefined error name for a Distributed Map whose failed items went past
/// its tolerated-failure threshold ("States.ExceedToleratedFailureThreshold").
pub const TOLERATED_FAILURE_ERROR: &str = "States.ExceedToleratedFailureThreshold";

/// The ASL predefined error name for an execution that ran longer than the
/// `TimeoutSeconds` its definition declares ("States.Timeout").
pub const TIMEOUT_ERROR: &str = "States.Timeout";

/// The ASL predefined error name for an expression that could not be evaluated
/// ("States.QueryEvaluationError — the JSONata expression evaluation failed").
///
/// It is the name a JSONata state's own `Output`, `Assign`, `Arguments`, `Items` or
/// `Condition` fails with, so a definition's `Retry` and `Catch` can answer for it by
/// name exactly as they answer for a failed task.
pub const QUERY_EVALUATION_ERROR: &str = "States.QueryEvaluationError";

/// The refusal an EXPRESS workflow answers a `Task` state that asks for the
/// run-a-job (`.sync`) integration pattern with.
///
/// The pattern is served only for the STANDARD workflow type: an Express state
/// machine is created from such a definition, and every execution of it fails AT that
/// state with [`RUNTIME_ERROR`] carrying this sentence, having started no job. The
/// state it failed at is named beside it ([`express_run_a_job_refusal`]), because a
/// definition may carry several.
pub const EXPRESS_RUN_A_JOB_UNSUPPORTED: &str =
    "Express state machine does not support '.sync' service integration";

/// [`EXPRESS_RUN_A_JOB_UNSUPPORTED`] as the state `name`, entered at the event
/// `entered_at`, reports it ([`executing_state`]).
fn express_run_a_job_refusal(name: &str, entered_at: i64) -> Failure {
    runtime(executing_state(
        name,
        entered_at,
        EXPRESS_RUN_A_JOB_UNSUPPORTED,
    ))
}

/// The refusal an EXPRESS workflow answers a `Task` state that asks for the callback
/// (`.waitForTaskToken`) integration pattern with.
///
/// The pattern needs an answer from outside the execution, which only the STANDARD
/// workflow type waits for: an Express state machine is created from such a
/// definition, and every execution of it fails AT that state with [`RUNTIME_ERROR`],
/// having made no call and handed out no token. The sentence states the unsupported
/// pattern the same way the service's own sentence for the other one does, with the
/// pattern the `Resource` asked for named in it.
pub const EXPRESS_CALLBACK_UNSUPPORTED: &str =
    "Express state machine does not support '.waitForTaskToken' service integration";

/// [`EXPRESS_CALLBACK_UNSUPPORTED`] as the state `name`, entered at the event
/// `entered_at`, reports it ([`executing_state`]).
fn express_callback_refusal(name: &str, entered_at: i64) -> Failure {
    runtime(executing_state(name, entered_at, EXPRESS_CALLBACK_UNSUPPORTED))
}

/// How this service reports a diagnosis the interpreter raised WHILE EXECUTING a
/// state: the state's own name, the id of the history event that state was ENTERED at,
/// and then the diagnosis itself.
///
/// It is the ONE wrapper every such diagnosis is reported under, because the diagnosis
/// alone says only WHAT could not be run and not WHERE: a definition may declare many
/// states that evaluate the same expression, so the state name and the event id are
/// what a reader — an operator or an automated triage — follows back to the state that
/// failed and to its place in the execution's own history.
fn executing_state(name: &str, entered_at: i64, diagnosis: &str) -> String {
    format!(
        "An error occurred while executing the state '{name}' (entered at the event id \
         #{entered_at}). {diagnosis}"
    )
}

/// The id the FIRST event of an execution's history carries: the service numbers them
/// from one, which is where every walk's own numbering starts from.
pub const FIRST_EVENT_ID: i64 = 1;

/// The opening of the diagnosis [`no_results_for_path`] words, which is what tells a
/// cause this module raised ABOUT A STATE'S OWN EXPRESSION from every other failure a
/// state reports ([`diagnosed`]).
const INVALID_PATH_OPENING: &str = "Invalid path '";

/// The diagnosis for a reference path that selected no value: the path AS THE
/// DEFINITION WROTE IT, and then the path that was RESOLVED, rendered the way the
/// service renders a resolved path ([`resolved_path`]).
///
/// It names no state, because the path is written in a state and the state names
/// itself around this ([`executing_state`]) — the one place that knows which state is
/// being executed and which history event entered it.
fn no_results_for_path(path: &str) -> String {
    format!(
        "{INVALID_PATH_OPENING}{path}' : No results for path: {}",
        resolved_path(path)
    )
}

/// The path a diagnosis reports as the one that was RESOLVED: the BRACKET notation
/// every segment of a reference path is written in (`$['a'][0]`), whichever of the two
/// forms the definition wrote it in.
///
/// It is the canonical rendering of the segments the path names ([`canonical_path`]),
/// which is also the only text the JSONPath engine is handed, so the path a diagnosis
/// reports is the path that was evaluated. A path into the CONTEXT OBJECT is resolved
/// against that object, with its first `$` naming the object itself, so what was
/// resolved is the rest of it; a text that names no reference-path segments at all was
/// never resolved, so it is reported as written.
fn resolved_path(path: &str) -> String {
    let evaluated = match is_context_path(path) {
        true => &path[1..],
        false => path,
    };
    reference_segments(evaluated)
        .map(|segments| canonical_path(&segments))
        .unwrap_or_else(|_| evaluated.to_string())
}

/// The diagnosis for a state the walk reached and could not LEAVE: the state decided
/// no next state, so the execution ends at it.
///
/// It is one diagnosis for every way a state can fail to transition — a `Choice` no
/// rule matched and that declares no `Default`, and a state carrying neither `Next`
/// nor `"End": true` — because what a reader has to be told is the same in each: the
/// run stopped at this state because nothing said where to go next. The state names
/// itself around it ([`executing_state`], through [`names_its_state`]), so the cause
/// carries the stuck state and the id of the event it was entered at, which is what
/// tells it apart from the other failures raised under [`RUNTIME_ERROR`].
const NO_NEXT_STATE: &str =
    "Failed to transition out of the state. The state does not point to a next state.";

/// The diagnosis for a `Choice` rule whose condition reads a path the state's input
/// does not resolve.
///
/// RECORDED text: it states that the condition path of the choice state references a
/// value that is not there, which is a different outcome from the state deciding no
/// transition ([`NO_NEXT_STATE`]), so a caller classifying a [`RUNTIME_ERROR`] by its
/// cause can tell a condition it could not evaluate from a `Choice` that matched
/// nothing. The path that could not be resolved is named beside it
/// ([`invalid_choice_condition_path`]), and the state names itself around the whole
/// diagnosis ([`executing_state`], through [`names_its_state`]).
const CHOICE_CONDITION_PATH_INVALID: &str =
    "The choice state's condition path references an invalid value.";

/// The diagnosis for a `Choice` rule's condition path that resolved to nothing: the
/// path the condition was written with, worded as every other unresolvable path is
/// ([`no_results_for_path`]), and then what the service states about a condition whose
/// path references a value that is not there ([`CHOICE_CONDITION_PATH_INVALID`]).
///
/// A `Choice` state reads several paths — one `Variable` per rule and the `*Path` form
/// of each operand — so the sentence alone says only that SOME condition could not be
/// evaluated. Naming the path is what tells the failing operand from every other path
/// in the state, which is what a reader follows back to the rule that owns it.
fn invalid_choice_condition_path(path: &str) -> String {
    format!("{INVALID_PATH_OPENING}{path}': {CHOICE_CONDITION_PATH_INVALID}")
}

/// Whether `cause` is the diagnosis `path` SELECTING NOTHING reports
/// ([`no_results_for_path`]), as opposed to any other reason a path could not be
/// evaluated — one this service refuses, a syntax it does not evaluate — each of which
/// already says what it was.
///
/// The one place that knowledge of the diagnosis' wording lives, for the two callers
/// that report an unresolved path under something more specific
/// ([`choice_path_failure`], [`member_path_diagnosis`]).
fn is_unresolved_path(cause: &str, path: &str) -> bool {
    cause == no_results_for_path(path)
}

/// `failure`, as a `Choice` rule's condition reading `path` reports it.
///
/// A path that resolved to nothing is a failure of the CONDITION rather than of the
/// state's own data flow, so it is reported under the service's choice-condition
/// diagnosis, naming the path the condition was written with
/// ([`invalid_choice_condition_path`]). Every other failure is carried as raised.
fn choice_path_failure(failure: Failure, path: &str) -> Failure {
    let unresolved = failure
        .cause
        .as_deref()
        .is_some_and(|cause| is_unresolved_path(cause, path));
    if !unresolved {
        return failure;
    }
    Failure {
        error: failure.error,
        cause: Some(invalid_choice_condition_path(path)),
    }
}

/// `why`, as the state member `member` reading `path` reports it.
///
/// A member and its `*Path` form are ALTERNATIVES — a definition carrying a
/// `SecondsPath` carries no `Seconds` — so what a reader has to be told is which of the
/// two the definition wrote and that the path it carries selected nothing. That is the
/// diagnosis the service words for such a member ([`parameter_without_input_value`]),
/// which NAMES the member rather than reporting the generic unresolved-path text; every
/// other failure is carried as raised.
fn member_path_diagnosis(member: &str, path: &str, why: String) -> String {
    if is_unresolved_path(&why, path) {
        return parameter_without_input_value(member, path);
    }
    why
}

/// How the diagnosis of a `*Path` MEMBER whose path selected nothing OPENS, and the
/// clause that follows the member's own name in it: together they tell it from every
/// other cause ([`names_its_state`], [`parameter_without_input_value`]).
const PARAMETER_OPENING: &str = "The ";
const NO_INPUT_VALUE_CLAUSE: &str = " parameter does not reference an input value: ";

/// The diagnosis for the `*Path` member `member` whose path selected no value: the
/// member named as the parameter it is, and the path the definition wrote in it.
///
/// The member is what points a reader at the field to correct, and a member of this
/// kind is one of a PAIR of alternatives a definition carries only one of, so the
/// diagnosis says which of the two was read. The state names itself around the whole of
/// it ([`executing_state`], through [`names_its_state`]).
fn parameter_without_input_value(member: &str, path: &str) -> String {
    format!("{PARAMETER_OPENING}{member}{NO_INPUT_VALUE_CLAUSE}{path}")
}

/// The diagnosis for a `Fail` state's `*Path` member that resolved to a value of
/// another type: the member, the type the language requires there, and the value it
/// found.
///
/// A `Fail` state's `Error` and `Cause` are strings, so a path that selects a number, an
/// array or an object names no value the state can fail with. The state names itself
/// around it ([`names_its_state`]).
fn referenced_value_must_be_a_string(member: &str, value: &Value) -> String {
    referenced_value_must_be(member, "a string", value)
}

/// The diagnosis for a member whose value is not one the language accepts there: the
/// member, what it must carry, and the value found.
///
/// One sentence for every member of that kind, because what a reader has to be told is
/// the same in each: which field of the state was read, what the language requires in
/// it, and what reading it produced.
fn referenced_value_must_be(member: &str, must_be: &str, value: &Value) -> String {
    format!("{REFERENCED_VALUE_OPENING}{member} must be {must_be}, but the value was: {value}")
}

/// What a `Wait` state's `Seconds` must carry: "a time, in seconds, the state machine
/// waits before continuing" — a whole number of them, and never a negative one.
pub(super) const WHOLE_SECONDS: &str = "a non-negative whole number of seconds";

/// What a `Task` state's `TimeoutSeconds` and `HeartbeatSeconds` must carry: each is
/// documented as "a positive integer" of seconds.
pub(super) const POSITIVE_SECONDS: &str = "a positive whole number of seconds";

/// What a `Wait` state's `Timestamp` must carry: "an absolute expiry time ... must
/// conform to the RFC3339 profile of ISO 8601".
pub(super) const ISO_INSTANT: &str = "an ISO 8601 instant";

/// What a retrier's own figures (`IntervalSeconds`, `BackoffRate`, `MaxDelaySeconds`)
/// must carry: a number, and never a negative one.
pub(super) const NON_NEGATIVE_NUMBER: &str = "a non-negative number";

/// How [`referenced_value_must_be_a_string`] opens, which is what tells it from every
/// other cause ([`names_its_state`]).
const REFERENCED_VALUE_OPENING: &str = "The value referenced by ";

/// The diagnosis for a `Map` state whose items are not an array: the reference path
/// that was supposed to name them, and what a `Map` state's items must be.
///
/// `path` is the state's `ItemsPath` where it declares one and [`WHOLE_INPUT_PATH`] —
/// "the default value is `$`, which selects the state's whole input" — where it does
/// not. The value itself is NOT quoted: what a reader has to be told is which path
/// named something that is not an array, and the value is the execution's own data.
fn reference_path_must_point_to_array(path: &str) -> String {
    format!("Reference path \"{path}\" must point to array.")
}

/// The reference path that selects a state's WHOLE input, which is the default of every
/// `*Path` member the language gives one: "the default value is `$`, which matches the
/// entire input".
pub(super) const WHOLE_INPUT_PATH: &str = "$";

/// `fault` as the state it happened IN reports it: a diagnosis this module worded
/// about that state's own expression is reported under [`executing_state`], naming the
/// state and the event id it was entered at.
///
/// Every OTHER failure is carried exactly as it was raised, because it already says
/// where it belongs: a `Fail` state's declared error and cause are the definition's
/// own, a task's failure is what the invoked service answered, and a failure a nested
/// `Map` iteration or `Parallel` branch raised was already wrapped by the state inside
/// it that raised it.
fn diagnosed(name: &str, entered_at: i64, fault: Fault) -> Fault {
    // A document past what a state may carry, measured where the state that produced it
    // was not in scope: the state being executed is the one that produced it, and the
    // sentence names it here.
    let failure = produced_by_state(name, fault.failure);
    let worded = match failure.cause.as_deref() {
        Some(cause) if names_its_state(cause) => Some(executing_state(name, entered_at, cause)),
        _ => None,
    };
    match worded {
        Some(cause) => Fault {
            at: fault.at,
            failure: Failure {
                error: failure.error,
                cause: Some(cause),
            },
        },
        None => Fault {
            at: fault.at,
            failure,
        },
    }
}

/// `failure` as the state `name` reports it, for the quota sentence raised with no
/// producer named ([`UNNAMED_PRODUCER`]): the state being executed is what produced the
/// document, so that state names itself in the sentence.
///
/// The ONE place that naming is applied, read wherever such a failure is REPORTED — by
/// the state that ends the execution with it ([`diagnosed`]) and by the catcher that
/// takes it over ([`Interpreter::catch_or_fail`], whose error output a handler reads the
/// cause out of) — so a client told about the same failure twice is told the same bytes.
/// Every other failure is carried exactly as it was raised.
fn produced_by_state(name: &str, failure: Failure) -> Failure {
    match failure.cause.as_deref() {
        Some(cause) if is_unnamed_producer(cause) => Failure {
            error: failure.error,
            cause: Some(result_too_large(name)),
        },
        _ => failure,
    }
}

/// Whether `cause` is a diagnosis this module worded about the state being executed —
/// its own expression, the condition path of a `Choice` rule, a `*Path` member that
/// selected nothing, an intrinsic invocation that could not be evaluated, or its
/// failure to transition out of it — so that state names itself around it
/// ([`diagnosed`]) rather than carrying a name of its own.
fn names_its_state(cause: &str) -> bool {
    cause.starts_with(INVALID_PATH_OPENING)
        || cause == NO_NEXT_STATE
        || cause.starts_with(super::jsonata::DIAGNOSIS_OPENING)
        || cause.starts_with(CONTEXT_PATH_OPENING)
        || cause.starts_with(INTRINSIC_OPENING)
        || cause.starts_with(FUNCTION_ERROR_OPENING)
        || cause.starts_with(REFERENCED_VALUE_OPENING)
        || cause.starts_with(super::integration::UNUSABLE_PARAMETERS_OPENING)
        || names_a_parameter(cause)
}

/// Whether `cause` is the diagnosis of a `*Path` member whose path selected nothing
/// ([`parameter_without_input_value`]).
///
/// The member's name is the only caller-chosen part, and it is written between the
/// opening and the clause, so the reading is anchored at the START of the cause rather
/// than searching it: a value this module echoes elsewhere in a cause cannot make
/// another diagnosis look like this one.
fn names_a_parameter(cause: &str) -> bool {
    cause
        .strip_prefix(PARAMETER_OPENING)
        .and_then(|rest| rest.split_once(NO_INPUT_VALUE_CLAUSE))
        .is_some_and(|(member, _)| !member.is_empty() && !member.contains(' '))
}

/// How the diagnosis of a PAYLOAD-TEMPLATE member whose path could not be found opens
/// ([`template_expression`]), which is what tells it from every other cause
/// ([`names_its_state`]).
pub(super) const CONTEXT_PATH_OPENING: &str = "The JSONPath '";

/// How the diagnosis of an intrinsic-function invocation that could not be evaluated
/// opens ([`intrinsic_failure`]).
pub(super) const INTRINSIC_OPENING: &str =
    "There was an error while evaluating the intrinsic function: ";

/// The diagnosis for an intrinsic-function invocation that could not be evaluated: the
/// invocation AS THE DEFINITION WROTE IT, and then what the function itself reported.
///
/// The invocation is what identifies the failing expression — a payload template may
/// carry many, and a nested invocation is one of the arguments of another — so it is
/// named whatever the detail says. The state names itself around the whole of it
/// ([`executing_state`], through [`names_its_state`]).
pub(super) fn intrinsic_failure(invocation: &str, detail: &str) -> String {
    format!("{INTRINSIC_OPENING}{invocation}. {detail}")
}

/// The detail an intrinsic function whose ARGUMENTS are not the ones it takes reports:
/// the service's own sentence, naming the function.
///
/// It is the one detail for every way an argument can be wrong — a missing argument, a
/// value of another type, a figure out of range — because what a reader has to be told
/// is the same in each: this function cannot be applied to these arguments.
pub(super) fn invalid_intrinsic_arguments(name: &str) -> String {
    format!("Invalid arguments in {name}")
}

/// The detail an intrinsic function that FAILED on its arguments reports: the
/// invocation, and the error the function itself raised.
///
/// It is the WHOLE of what a `*Path` argument that selected no value reports, because
/// such a failure is the function's own rather than one of evaluating the invocation
/// ([`intrinsic_failure`] is what every other one is reported under).
pub(super) fn intrinsic_function_error(invocation: &str, why: &str) -> String {
    format!("{FUNCTION_ERROR_OPENING}{invocation}' had the following error: {why}")
}

/// How [`intrinsic_function_error`] opens, which is what tells it from every other
/// cause ([`names_its_state`]).
pub(super) const FUNCTION_ERROR_OPENING: &str = "The function '";

/// The error a `*Path` ARGUMENT of an intrinsic function that selected no value
/// reports: the argument's own path, and the document it was resolved against.
///
/// The document is the one the path was evaluated against — the context object for a
/// path that begins with `$$`, the state's own input for every other — and it is
/// reported WHOLE: the message exists to show what the path was looked for in.
pub(super) fn intrinsic_path_not_found(path: &str, searched: &Value) -> String {
    format!(
        "The JsonPath argument for the field '{path}' could not be found in the input '{}'",
        reported_document(searched)
    )
}

/// What a `Wait` state's members failing to evaluate reports.
///
/// A diagnosis the state names itself around ([`diagnosed`]) — an unresolved path, an
/// expression that misevaluated — is carried exactly as it was raised, under the error
/// name its own class carries; every other reason — a figure out of range, a member of
/// the wrong type — names the state here, which is the only place a `Wait`'s own
/// reading of its members knows it from.
fn wait_failure(name: &str, failure: Failure) -> Failure {
    let Some(why) = failure.cause else {
        return Failure {
            error: failure.error,
            cause: None,
        };
    };
    if names_its_state(&why) {
        return Failure {
            error: failure.error,
            cause: Some(why),
        };
    }
    Failure {
        error: failure.error,
        cause: Some(format!("state '{name}': {why}")),
    }
}

/// The most state transitions one execution makes before the interpreter stops it.
///
/// Emulator step budget, NOT an AWS quota: it is what bounds a definition whose
/// `Next` chain forms a cycle, and — because every `Map` iteration and `Parallel`
/// branch spends from the same budget — what bounds the total work one execution
/// does. An execution that exceeds it ends `FAILED` with [`RUNTIME_ERROR`].
pub const MAX_STATE_TRANSITIONS: usize = 1_000;

/// The most characters a state name may carry.
///
/// From the Amazon States Language: a state's name "must be unique within the scope
/// of the whole state machine and has a maximum length of 80 characters". A longer
/// one is refused at create time, which is also what keeps an execution's history
/// bounded in BYTES: every history event carries the name of the state it happened
/// in.
pub const MAX_STATE_NAME_CHARS: usize = 80;

/// The most bytes a state's output may carry: the maximum the modelled
/// `SensitiveData` shape that `input`/`output` target accepts (0..262144).
///
/// It is checked at EVERY state, on the document that state produced, not only on
/// the one the run ends with: a definition that grows its document — a `Pass` that
/// duplicates its own input and loops back to itself, say — is stopped at the first
/// state past the limit, with ASL's own [`DATA_LIMIT_ERROR`], rather than being
/// allowed to grow until the run finishes.
pub const MAX_PAYLOAD_BYTES: usize = 262_144;

/// The most levels a state's document may nest.
///
/// Emulator bound, NOT an AWS quota: `serde_json` refuses to PARSE a document nested
/// deeper than its own 128-level recursion limit, so this is the same ceiling applied
/// to a document the walk BUILT (a payload template or a `Map` result can nest one
/// level deeper per transition). Every later reader — `serde_json`'s own serializer
/// included — walks a document recursively, so a document past this is reported with
/// [`DATA_LIMIT_ERROR`] instead of being carried on.
pub const MAX_PAYLOAD_DEPTH: usize = 128;

/// The most levels of `Map` iterations and `Parallel` branches one run may nest.
///
/// Emulator bound: each level is an interpreter frame, so the nesting a caller's
/// definition can ask for is capped rather than left to the thread's stack. A
/// definition past it ends `FAILED` with [`RUNTIME_ERROR`].
pub const MAX_NESTING_DEPTH: usize = 16;

/// The most levels an intrinsic-function invocation may nest
/// (`States.Array(States.Array(…))`).
///
/// Emulator bound on a hand-written parser: the nesting lives inside ONE JSON string,
/// so `serde_json`'s own recursion limit does not bound it. A deeper invocation is
/// reported with [`RUNTIME_ERROR`].
pub const MAX_EXPRESSION_DEPTH: usize = 16;

/// The most characters one intrinsic-function invocation may carry.
///
/// Emulator bound, for the same reason: the invocation text is a value of a
/// definition the model bounds only at 1 MiB, and this parser is this crate's own.
pub const MAX_EXPRESSION_CHARS: usize = 4_096;

/// The most segments a reference path (`ResultPath`, `$.a.b[0]`) may carry.
///
/// Emulator bound: ASL reference paths name a place in a document, so they are short,
/// and the path text comes from a definition bounded only at 1 MiB. A longer one is
/// reported with [`RESULT_PATH_ERROR`].
pub const MAX_PATH_SEGMENTS: usize = 64;

/// The most characters any `*Path` member, `Variable` or payload-template path may
/// carry.
///
/// Emulator bound on EVERY path, before it reaches the JSONPath engine: the text is a
/// value of a definition bounded only at 1 MiB and it is evaluated once per visit to
/// its state, so its length — and the reference-path form [`select_path`] requires —
/// is what bounds the work one path can ask for.
pub const MAX_PATH_CHARS: usize = 1_024;

/// The most choice rules one `Choice` state, or one `And`/`Or` condition, may carry —
/// and the most branches one `Parallel` state may declare.
///
/// Emulator bound: a `Choice` evaluates its rules until one matches and a `Parallel`
/// runs every branch, so the count multiplies the work one visit to that state does.
/// It is otherwise bounded only by the 1 MiB definition.
pub const MAX_CHOICE_RULES: usize = 64;

/// The most `"<name>.$"` members one payload template may carry.
///
/// Emulator bound, for the same reason: each one is an expression evaluated on every
/// visit to the state, so the member count multiplies that state's work.
pub const MAX_TEMPLATE_PATHS: usize = 64;

/// The most characters a `StringMatches` pattern may carry.
///
/// Emulator bound: matching a glob against a value costs the pattern's length times
/// the value's, and the value is bounded by [`MAX_PAYLOAD_BYTES`].
pub const MAX_GLOB_CHARS: usize = 256;

/// The longest ONE interpretation spends WAITING for the service-integration calls its
/// `Task` states' work is made of, across the whole walk.
///
/// Emulator bound, NOT an AWS duration: one interpretation holds one blocking thread
/// for its whole duration, including while a call it asked for is in flight, so what
/// has to be bounded is the time that thread can be held.
/// [`super::integration::MAX_CALL_DURATION`] bounds ONE call and
/// [`MAX_STATE_TRANSITIONS`] bounds how many a run may make; this bounds their product,
/// by CLAMPING what each call may still wait for to what is left of it
/// ([`Interpreter::call_within`]), so the thread a run holds is held for at most this
/// however many calls the definition asks for.
///
/// What it bounds is the WAITING, never the execution: a call that does not answer
/// inside the span it was given is the task's own timeout — ASL's
/// [`TIMEOUT_ERROR`], which the state's `Retry` and `Catch` may answer for — so the
/// outcome stays the definition's. It is wide enough for the calls a real workload
/// makes, a poll loop whose every revisit invokes a function that runs a caller's own
/// code included.
pub const MAX_CALL_WAIT_DURATION: Duration = Duration::from_secs(120);

/// The most values `States.ArrayRange` builds: its published limit of "1000 items",
/// past which the function fails rather than returning an oversized array.
///
/// Each value is a number, so this also keeps the array inside what a state's output
/// may carry — which matters because the array is built from three numbers, so its
/// length is not bounded by the size of anything the caller sent.
pub const MAX_ARRAY_RANGE_VALUES: usize = 1_000;

/// The most items an array an intrinsic function walks may carry.
///
/// Emulator bound: an array an intrinsic is handed comes out of the execution input,
/// which is bounded in BYTES and not in element COUNT, so an array of tiny elements
/// can carry on the order of 10^5 of them. Every intrinsic that walks one is linear
/// in the count, and this is the ceiling on that count, reported with
/// [`RUNTIME_ERROR`].
pub const MAX_ARRAY_ITEMS: usize = 10_000;

/// The most levels of nested state machines (`ItemProcessor`, `Iterator`,
/// `Branches`) a definition may declare for `validate` to walk.
///
/// `serde_json` refuses to parse a document nested deeper than its own 128-level
/// recursion limit, so this is that same ceiling stated explicitly, which is what
/// bounds the structural walk below.
pub const MAX_DEFINITION_DEPTH: usize = 128;

/// The most characters `States.Base64Encode`/`States.Base64Decode` accept: "You can
/// encode input data up to 10,000 characters."
pub const MAX_BASE64_CHARS: usize = 10_000;

/// The most characters `States.Hash` hashes: its published maximum input size of
/// 10,000 characters.
pub const MAX_HASH_CHARS: usize = 10_000;

/// The most bytes a `Fail` state's `Error` and `Cause` may carry: the maxima of the
/// modelled `SensitiveError` (0..256) and `SensitiveCause` (0..32768) shapes
/// `DescribeExecution` reports them through. A longer one is refused at create time.
pub const MAX_ERROR_BYTES: usize = 256;

/// See [`MAX_ERROR_BYTES`].
pub const MAX_CAUSE_BYTES: usize = 32_768;

/// The longest a `Wait` state waits: Step Functions' published maximum `Wait`
/// duration of one year, in seconds. A definition asking for more is out of range,
/// and ends the execution with [`RUNTIME_ERROR`] rather than being silently
/// shortened to this.
pub const MAX_WAIT_SECONDS: u64 = 31_622_400;

/// Every state type the Amazon States Language DEFINES: the language's `Type` member
/// selects one of these eight and nothing else, so a state declaring any other type
/// is not a state the language describes and no state machine can be created with it
/// ([`diagnose_state`] reports it).
///
/// The list is the union of the two groups the transition rule is keyed on —
/// [`TERMINAL_STATE_TYPES`] and [`CONTINUING_STATE_TYPES`] — plus `Choice`, which
/// transitions through its own `Choices`/`Default` members.
pub const STATE_TYPES: &[&str] = &[
    "Choice", "Fail", "Map", "Parallel", "Pass", "Succeed", "Task", "Wait",
];

/// The state types this emulator executes, which is every type the language defines.
///
/// `Task` is one of them for both kinds of work a `Resource` can name: an activity
/// task, which a worker polls for, and a service-integration call this emulator
/// makes ([`super::integration`]). A `Resource` naming neither still ends the
/// execution with [`RUNTIME_ERROR`].
pub const EXECUTED_STATE_TYPES: &[&str] = STATE_TYPES;

/// One of the two query languages ASL's `QueryLanguage` member selects between,
/// evaluated by [`serde_json_path`]: the language every `*Path` member and every
/// `"<name>.$"` payload-template key is written in, and the one a definition that
/// declares no `QueryLanguage` is written in.
pub const JSONPATH: &str = "JSONPath";

/// The other query language `QueryLanguage` selects: its expressions are evaluated
/// by [`super::jsonata`], and its members are the ones that language declares rather
/// than JSONPath's.
pub const JSONATA: &str = "JSONata";

/// The two query languages `QueryLanguage` selects between, at the definition
/// level and at one state's.
pub const QUERY_LANGUAGES: &[&str] = &[JSONPATH, JSONATA];

/// State types that end an execution, so they must NOT carry `Next`.
const TERMINAL_STATE_TYPES: &[&str] = &["Fail", "Succeed"];

/// State types that continue an execution, so each must carry `Next` or
/// `"End": true`. `Choice` is absent because it branches through its own
/// `Choices`/`Default` members instead.
const CONTINUING_STATE_TYPES: &[&str] = &["Map", "Parallel", "Pass", "Task", "Wait"];

/// `Retry`'s documented defaults: "IntervalSeconds (Optional) ... default 1",
/// "MaxAttempts (Optional) ... default 3", "BackoffRate (Optional) ... default 2.0".
const DEFAULT_RETRY_INTERVAL_SECONDS: f64 = 1.0;
const DEFAULT_RETRY_MAX_ATTEMPTS: u64 = 3;
const DEFAULT_RETRY_BACKOFF_RATE: f64 = 2.0;

/// The members each executed state type may carry. A state that carries any other
/// member is not executed: the member changes what the state does, and running the
/// state while ignoring it would report an outcome the definition does not describe.
///
/// A `Task`'s members depend on WHICH work its `Resource` names, so the type alone
/// does not decide them: this answers the ACTIVITY task's set, and
/// [`INTEGRATION_TASK_MEMBERS`] the service-integration call's.
/// [`unexecutable_member`] is what picks between the two.
fn executable_members(state_type: &str) -> &'static [&'static str] {
    match state_type {
        "Pass" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "Parameters",
            "Result",
            "ResultPath",
            "OutputPath",
            "Next",
            "End",
        ],
        "Wait" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "OutputPath",
            "Seconds",
            "SecondsPath",
            "Timestamp",
            "TimestampPath",
            "Next",
            "End",
        ],
        "Choice" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "OutputPath",
            "Choices",
            "Default",
        ],
        "Succeed" => &["Type", "Comment", "QueryLanguage", "InputPath", "OutputPath"],
        "Fail" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Error",
            "ErrorPath",
            "Cause",
            "CausePath",
        ],
        // An ACTIVITY task's own work is done by a worker, so the members below are
        // the ones that decide what is scheduled for it, what the state does with the
        // reply, and what happens when the task fails or runs out of time: a `Retry`
        // re-schedules the task for its next attempt, which is a task a worker polls
        // for exactly as the attempt before it was. `Catch` is deliberately absent:
        // handing the failure to a catcher continues the walk from the catcher's
        // `Next`, and a state after it may be a service integration whose call is made
        // from the REQUEST the walk runs inside — a task the clock closes has no
        // request around it, so a state that declares one is not run rather than run as
        // though it declared none. `Credentials` is absent for the same reason it is on
        // every other type: it changes who the invocation is made as.
        "Task" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Resource",
            "InputPath",
            "Parameters",
            "TimeoutSeconds",
            "TimeoutSecondsPath",
            "HeartbeatSeconds",
            "HeartbeatSecondsPath",
            "ResultSelector",
            "ResultPath",
            "OutputPath",
            "Retry",
            "Next",
            "End",
        ],
        // `ResultWriter` is interpreted too: it WRITES an S3 dataset, and the manifest
        // of result files it writes is written there for real
        // ([`super::result_writer`]), as `ItemReader` and `ItemBatcher` are — the
        // dataset is read from S3 ([`super::item_reader`]) and its items are grouped as
        // declared ([`super::item_batcher`]).
        "Map" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "Parameters",
            "ItemReader",
            "ItemBatcher",
            "ItemSelector",
            "ItemsPath",
            "Items",
            "ItemProcessor",
            "Iterator",
            "MaxConcurrency",
            "MaxConcurrencyPath",
            "ResultSelector",
            "ResultWriter",
            "ResultPath",
            "OutputPath",
            "Retry",
            "Catch",
            "ToleratedFailureCount",
            "ToleratedFailureCountPath",
            "ToleratedFailurePercentage",
            "ToleratedFailurePercentagePath",
            "Label",
            "Next",
            "End",
        ],
        "Parallel" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "InputPath",
            "Parameters",
            "Branches",
            "ResultSelector",
            "ResultPath",
            "OutputPath",
            "Retry",
            "Catch",
            "Next",
            "End",
        ],
        _ => &[],
    }
}

/// The members a `Task` state whose `Resource` names a SERVICE INTEGRATION may
/// carry: the ones that decide the request the call carries, what becomes of its
/// answer, and how a failed call is handled.
///
/// `HeartbeatSeconds`/`HeartbeatSecondsPath` and `Credentials` are deliberately
/// absent: a heartbeat is owed by an activity or a callback task, whose outcome
/// arrives after the call this emulator makes ([`CALLBACK_TASK_MEMBERS`] is that
/// task's own set), and `Credentials` asks for the call to be made under another
/// role, which this emulator does not assume.
const INTEGRATION_TASK_MEMBERS: &[&str] = &[
    "Type",
    "Comment",
    "QueryLanguage",
    "Resource",
    "InputPath",
    "Parameters",
    "ResultSelector",
    "ResultPath",
    "OutputPath",
    "Retry",
    "Catch",
    "TimeoutSeconds",
    "TimeoutSecondsPath",
    "Next",
    "End",
];

/// The members a `Task` state asking for the callback (`.waitForTaskToken`) pattern
/// may carry: an integration task's, plus the heartbeat this one CAN owe.
///
/// "If more time than the specified seconds elapses between heartbeats from the task,
/// this state fails with a States.Timeout error name": the answer to a callback task
/// comes from outside the execution, so the span between the `SendTaskHeartbeat`
/// calls that report it still running is a span this pattern has — which the
/// request-response form, finished by its own call, does not.
const CALLBACK_TASK_MEMBERS: &[&str] = &[
    "Type",
    "Comment",
    "QueryLanguage",
    "Resource",
    "InputPath",
    "Parameters",
    "ResultSelector",
    "ResultPath",
    "OutputPath",
    "Retry",
    "Catch",
    "TimeoutSeconds",
    "TimeoutSecondsPath",
    "HeartbeatSeconds",
    "HeartbeatSecondsPath",
    "Next",
    "End",
];

/// The members a `Task` state written in JSONata that asks for the callback pattern
/// may carry: the JSONata integration task's, plus the same heartbeat bound.
const JSONATA_CALLBACK_TASK_MEMBERS: &[&str] = &[
    "Type",
    "Comment",
    "QueryLanguage",
    "Assign",
    "Output",
    "Resource",
    "Arguments",
    "TimeoutSeconds",
    "HeartbeatSeconds",
    "Retry",
    "Catch",
    "Next",
    "End",
];

/// The first member of `state` this emulator does not execute, if any.
///
/// `language` is the query language the state is written in, which decides WHICH set
/// of data-flow members it may carry: a JSONata state reads and writes its data
/// through the members of that language ([`jsonata_members`]) and a JSONPath one
/// through the `*Path` family and the payload templates.
fn unexecutable_member(
    state_type: &str,
    state: &Map<String, Value>,
    language: &str,
) -> Option<String> {
    let resource = task_resource(state);
    let activity = super::names_an_activity(resource);
    // A callback task's work is answered from OUTSIDE the execution, so it carries
    // the heartbeat bound neither of the other two kinds of integration task does.
    let callback = state_type == "Task"
        && !activity
        && super::integration::asks_for_callback(resource);
    let allowed = match (language, state_type) {
        (JSONATA, "Task") if callback => JSONATA_CALLBACK_TASK_MEMBERS,
        (JSONATA, _) => jsonata_members(state_type, activity),
        (_, "Task") if callback => CALLBACK_TASK_MEMBERS,
        // The two kinds of `Task` work carry different members, and the `Resource`
        // is what says which kind this state is.
        (_, "Task") if !activity => INTEGRATION_TASK_MEMBERS,
        _ => executable_members(state_type),
    };
    state
        .keys()
        .find(|member| !allowed.contains(&member.as_str()))
        .cloned()
}

/// The members a state written in JSONata may carry.
///
/// The language's own data-flow members replace JSONPath's: `Arguments` is the request
/// a `Task` builds (`Parameters`), `Items` the array a `Map` iterates (`ItemsPath`),
/// a `Choice` rule's `Condition` its test (`Variable` and the comparators) and
/// `Output` what a state puts out (`ResultSelector`, `ResultPath`, `OutputPath`) —
/// beside `Assign`, which every type may carry and which only this language has. The
/// `*Path` family and the payload templates belong to the other language
/// ([`JSONPATH_ONLY_MEMBERS`]), which is what a definition carrying one is refused
/// for at create time.
fn jsonata_members(state_type: &str, activity: bool) -> &'static [&'static str] {
    match state_type {
        "Pass" => &["Type", "Comment", "QueryLanguage", "Assign", "Output", "Next", "End"],
        "Wait" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Assign",
            "Output",
            "Seconds",
            "Timestamp",
            "Next",
            "End",
        ],
        "Choice" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Assign",
            "Output",
            "Choices",
            "Default",
        ],
        "Succeed" => &["Type", "Comment", "QueryLanguage", "Assign", "Output"],
        "Fail" => &["Type", "Comment", "QueryLanguage", "Error", "Cause"],
        // An ACTIVITY task's work is a worker's, so it carries the heartbeat an
        // integration call cannot owe and no `Catch` — the same split
        // [`executable_members`] makes for the other language, for the same reasons.
        "Task" if activity => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Assign",
            "Output",
            "Resource",
            "Arguments",
            "TimeoutSeconds",
            "HeartbeatSeconds",
            "Retry",
            "Next",
            "End",
        ],
        "Task" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Assign",
            "Output",
            "Resource",
            "Arguments",
            "TimeoutSeconds",
            "Retry",
            "Catch",
            "Next",
            "End",
        ],
        "Map" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Assign",
            "Output",
            "Items",
            "ItemReader",
            "ItemBatcher",
            "ItemSelector",
            "ItemProcessor",
            "Iterator",
            "MaxConcurrency",
            "ResultWriter",
            "Retry",
            "Catch",
            "ToleratedFailureCount",
            "ToleratedFailurePercentage",
            "Label",
            "Next",
            "End",
        ],
        "Parallel" => &[
            "Type",
            "Comment",
            "QueryLanguage",
            "Assign",
            "Output",
            "Arguments",
            "Branches",
            "Retry",
            "Catch",
            "Next",
            "End",
        ],
        _ => &[],
    }
}

/// The `Resource` a `Task` state names, as the text the language writes it as — the
/// empty string for a state that carries none, which names no work either way.
fn task_resource(state: &Map<String, Value>) -> &str {
    state
        .get("Resource")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One history event the walk produced.
#[derive(Debug, Clone)]
pub struct Step {
    pub event_type: &'static str,
    pub state_name: Option<String>,
    /// How far into the run this event happened.
    pub after: Duration,
    /// Whether this is the event the execution ends on.
    pub terminal: bool,
    /// What the walk knew about this event, which is what the history reports as that
    /// event type's own detail member: the work a `Task` state named and the document
    /// it was handed, the error and cause its work failed with, the item one `Map`
    /// iteration was handed and how many a `Map` state is iterating. A step whose
    /// event type carries no detail of its own leaves every member unset.
    pub details: super::EventDetails,
}

/// One item of a `Map` state's iteration, as the walk ran it.
///
/// The offset is into the RUN (the total of every `Wait` the walk passed before the
/// iteration ended), so the caller that stores it turns it into an instant against
/// the execution's own start — the walk never sleeps. It is what tells the item's
/// outcome apart from the ones that are still to come: an item is finished from this
/// offset on and not before.
#[derive(Debug, Clone, Copy)]
pub struct MapItemOutcome {
    /// How far into the run this item's iteration finished.
    pub finished_at: Duration,
    /// Whether the iteration FAILED, which is what a Map Run's failure budget counts.
    pub failed: bool,
}

/// One Distributed Map state the walk ran, as the Map Run it started.
///
/// A Distributed Map "starts a Map Run", a resource of its own that reports the
/// state's configuration and what its items produced. The offsets are into the RUN
/// (the total of every `Wait` the walk passed), so the caller that stores this turns
/// them into instants against the execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct StartedMapRun {
    /// The Map Run's own ARN, resolved as its `Map` state was entered: the one the
    /// attempt being continued already reports ([`super::integration::ResumedMapRun`])
    /// and a fresh one for a state starting a Map Run of its own, so the results its
    /// `ResultWriter` writes and the output its state puts out name the Map Run the
    /// store goes on to record.
    pub arn: String,
    /// The state's `Label` when it declares one, else the state's own name: "a
    /// user-defined or an auto-generated string that identifies a Map state".
    pub label: String,
    /// The status the state's items left the Map Run in: `SUCCEEDED` when the
    /// iterations produced a result, `FAILED` when more of them failed than the
    /// state tolerates.
    pub status: &'static str,
    /// How far into the run the state was entered.
    pub started_at: Duration,
    /// How far into the run its last item finished.
    pub stopped_at: Duration,
    /// `MaxConcurrency` as the state resolved it; `0` is "no limit", which is the
    /// member's own default.
    pub max_concurrency: i32,
    /// `ToleratedFailureCount` as the state resolved it.
    pub tolerated_failure_count: i64,
    /// `ToleratedFailurePercentage` as the state resolved it.
    pub tolerated_failure_percentage: f32,
    /// How many items the state iterated.
    pub total_items: i64,
    /// How each item the state iterated turned out, in the order the iterations
    /// finished: what the Map Run counts an item under, and from which offset.
    pub items: Vec<MapItemOutcome>,
    /// The child execution each item of the state was run as, in the order the
    /// iterations were started: what `ListExecutions` reports for this Map Run.
    pub children: Vec<StartedChildExecution>,
}

/// One child workflow execution a Distributed Map state started: the execution ONE
/// of its items ran as.
///
/// A Distributed Map "starts a child workflow execution for each item", so each item
/// is an execution of its own — with its own ARN, which that item's states read
/// through `$$.Execution.Id`, the `ItemSelector` projection as its own input and
/// what its iteration produced as its own output. The offsets are into the RUN (the
/// total of every `Wait` the walk passed), so the caller that stores this turns them
/// into instants against the parent execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct StartedChildExecution {
    /// The execution name, which the ARN below carries as its last field.
    pub name: String,
    /// The child's own execution ARN, under the state machine the Map Run runs and
    /// the Map state's label.
    pub arn: String,
    /// "The string that contains the JSON input data" of this execution: the
    /// `ItemSelector` projection of its item, or the item itself where the state
    /// declares no selector.
    pub input: String,
    /// What its iteration produced, for a child that SUCCEEDED.
    pub output: Option<String>,
    /// The error and cause its iteration failed with, for one that did not.
    pub error: Option<String>,
    pub cause: Option<String>,
    /// `SUCCEEDED` or `FAILED`, as its own iteration ended.
    pub status: &'static str,
    /// How far into the run its iteration started, and how far into the run it
    /// finished.
    pub started_at: Duration,
    pub finished_at: Duration,
    /// "The total number of items processed in a child workflow execution": one for
    /// each item of the state's array, and the size of the BATCH for a state that
    /// declares an `ItemBatcher`, whose children are each handed one batch
    /// ([`super::item_batcher::item_count`]).
    pub item_count: i32,
    /// The instants this execution really ran between, for one an EARLIER attempt
    /// started and this one KEPT: a redrive "preserves the results and execution
    /// history of the successful steps, and doesn't rerun these steps", so the item it
    /// ran was not run again, the record it already has is left exactly as it is, and
    /// the dates it reports are the ones that attempt recorded. `None` for an execution
    /// THIS attempt started, whose instants are its own offsets into the run.
    pub kept: Option<(SystemTime, SystemTime)>,
}

/// What one walk of a definition produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// `SUCCEEDED`, `FAILED` or `TIMED_OUT` — or `RUNNING`, when the walk reached a
    /// task it waits at and [`Run::waiting`] carries it: the execution has no outcome
    /// yet, because the next thing that happens to it is an answer from outside.
    pub status: &'static str,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub steps: Vec<Step>,
    /// The offset into the run at which the outcome became due: the total of every
    /// `Wait` the walk passed.
    pub elapsed: Duration,
    /// The task the walk SUSPENDED on, when it reached one: an activity's task, or
    /// the token a callback integration's call carried. A run that carries this
    /// reached no terminal status: that task is what the execution is waiting for.
    pub waiting: Option<ScheduledActivity>,
    /// The run-a-job integration's job the walk PARKED at, when it started one. A run
    /// that carries this reached no terminal status either: what the execution is
    /// waiting for is that job finishing.
    pub parked: Option<ParkedJob>,
    /// The jobs this walk started and then ABANDONED: a job a `Parallel` branch
    /// started that the state around it failed before the job could finish, so no
    /// task is waiting at it any more. The caller STOPS each one through its own
    /// integration's stopping call, so a job this emulator cannot resolve is not left
    /// running behind an execution that has already failed.
    pub abandoned: Vec<ParkedJob>,
    /// The Map Run each Distributed Map state the walk entered started, in the order
    /// they were entered.
    pub map_runs: Vec<StartedMapRun>,
    /// The step this walk ended UNSUCCESSFULLY at: the state whose failure ended the
    /// execution, and the raw input that state was entered with.
    ///
    /// It is what a later `RedriveExecution` continues from — "when you redrive an
    /// execution, it continues the failed execution from the unsuccessful step and
    /// uses the same input" — so a walk that failed inside a state reports which
    /// state that was instead of leaving a redrive to walk the whole definition
    /// again. `None` for a walk that reached no failure of a state's own: one that
    /// succeeded, one that could not be run at all, and one the definition-level
    /// `TimeoutSeconds` ended.
    pub stopped_at: Option<RedriveStep>,
}

impl Run {
    /// This run, with the Map Runs its Distributed Map states started.
    fn with_map_runs(mut self, map_runs: Vec<StartedMapRun>) -> Self {
        self.map_runs = map_runs;
        self
    }
}

/// The task a `Task` state scheduled and SUSPENDED at, as the walk computed it:
/// an activity's task, waiting for the worker that polls `GetActivityTask` for it,
/// or a callback (`.waitForTaskToken`) task, waiting for the `SendTask*` that quotes
/// the token the integration call already carried.
///
/// The offsets are into the RUN (the total of every `Wait` the walk passed before the
/// state), so the caller that parks this turns them into instants against the
/// execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct ScheduledActivity {
    /// The activity ARN the state's `Resource` named, which is the queue a worker
    /// polls with `GetActivityTask`; `None` for a CALLBACK task, whose work was
    /// already handed over by the integration call this walk made.
    pub activity_arn: Option<String>,
    /// The `taskToken` the integration call CARRIED, for a callback task: the walk
    /// minted it before making the call, because that call is what delivered it, so
    /// the store records the task under the token its answer will quote rather than
    /// minting one of its own. `None` for an activity's task, whose token is minted
    /// when the task is queued.
    pub token: Option<String>,
    /// The state that scheduled it, so the reply resumes the execution there.
    pub state_name: String,
    /// "The string that contains the JSON input data for the task": the state's
    /// effective input, after `InputPath` and the `Parameters` payload template, as
    /// this service REPORTS it — the caller's own bytes where the document is the one
    /// they supplied ([`Interpreter::echo`]).
    pub input: String,
    /// The RAW input of the state that scheduled it, before `InputPath` and
    /// `Parameters`: what the state's `ResultPath` combines a worker's reply with, so
    /// the reply can be placed when it arrives rather than guessed at.
    pub raw_input: Value,
    /// How far into the run the state scheduled the task.
    pub at: Duration,
    /// `TimeoutSeconds`: "If the task runs longer than the specified seconds, this
    /// state fails with a States.Timeout error name."
    pub timeout_seconds: Option<u64>,
    /// `HeartbeatSeconds`: "If more time than the specified seconds elapses between
    /// heartbeats from the task, this state fails with a States.Timeout error name."
    pub heartbeat_seconds: Option<u64>,
    /// How many attempts each retrier of the scheduling state has ALREADY spent on
    /// this task, in the order the state declares them.
    ///
    /// The budget a `Retry` gives a state is the state's, not one attempt's, so it
    /// has to outlive the suspension: the walk that continues from the answer seeds
    /// its retrier counts from this, and the retrier that has spent its `MaxAttempts`
    /// retries nothing further. EMPTY for the first attempt of a task, which has
    /// spent none.
    pub retries_taken: Vec<u64>,
}

/// The job a `Task` state's run-a-job (`.sync`) integration STARTED, which parked the
/// walk: the task is finished when that job is, so what happens next is the job
/// reaching a terminal status rather than another state of this definition.
///
/// The offset is into the RUN (the total of every `Wait` the walk passed before the
/// state), so the caller that parks this turns it into an instant against the
/// execution's own start — the walk never sleeps.
#[derive(Debug, Clone)]
pub struct ParkedJob {
    /// The state that started the job, so the job's outcome resumes the execution
    /// there.
    pub state_name: String,
    /// The RAW input of that state, before `InputPath` and `Parameters`: what its
    /// `ResultPath` combines the job's own report with once the job has ended.
    pub raw_input: Value,
    /// The integration the job belongs to, and the service whose API reports and
    /// stops it.
    pub job: &'static super::integration::RunAJob,
    pub api: &'static super::integration::Api,
    /// Whether the `Resource` asked for the pattern's second revision.
    pub parsed_output: bool,
    /// The job's own identifier, as the call that started it reported it.
    pub job_id: String,
    /// What the starting call's own request said the job belongs to, for an
    /// integration whose reporting and stopping calls do not name it by its
    /// identifier alone ([`super::integration::RunAJob::naming`]).
    pub naming: super::integration::JobNaming,
    /// How that request ADDRESSED the resource the pattern's own calls are taken on,
    /// which is what those calls are authorized over
    /// ([`super::integration::RunAJob::acts_on_named`]).
    pub acts_on: Option<String>,
    /// The document that call answered, which is what the task reports as its
    /// SUBMISSION.
    pub submitted: Value,
    /// How far into the run the state started the job.
    pub at: Duration,
    /// `TimeoutSeconds`: "If the task runs longer than the specified seconds, this
    /// state fails with a States.Timeout error name" — which for this pattern bounds
    /// how long the job may stay unfinished, because the task is not finished until
    /// it is.
    pub timeout_seconds: Option<u64>,
    /// The concurrent states the parked task is nested INSIDE, outermost first, and
    /// empty for a task at the top level of the definition.
    ///
    /// A `Parallel` branch is walked inside its state's own turn, so a task that
    /// parks inside one parks the whole execution: what resumes it has to re-enter
    /// that `Parallel` and carry on in the branch that parked, with the branches that
    /// had already finished keeping what they produced. Each frame is one such level
    /// ([`ParkedFrame`]).
    pub frames: Vec<ParkedFrame>,
}

/// One concurrent state a parked task is nested inside: the `Parallel` state, which
/// of its branches the task is in, and what the OTHER branches produced before the
/// walk parked.
///
/// It is what lets the walk that continues the execution re-enter the state without
/// running anything twice: the branches that finished are not walked again (their
/// results stand, and their events are already in the history), and the branch that
/// parked carries on from the state that parked it.
#[derive(Debug, Clone)]
pub struct ParkedFrame {
    /// The `Parallel` state's own name, as the state list that declares it spells it.
    pub state_name: String,
    /// The RAW input that state was entered with, so the walk that re-enters it sees
    /// the data it saw.
    pub state_input: Value,
    /// Which branch of it the task that parked is in.
    pub branch: usize,
    /// What each branch produced, in branch order, with no value at `branch`: the
    /// branch that parked has produced nothing yet.
    pub done: Vec<Option<Value>>,
}

/// What the job an execution was PARKED at ended as, which is the result of the state
/// that started it.
///
/// `Ok` carries the document the job's own reporting call answered — the job did its
/// work, so that report IS the task's result — and `Err` why it ended without doing
/// it, which is the task's failure.
#[derive(Debug, Clone)]
pub struct JobReply {
    /// The state that started the job, as the parked record names it.
    pub state_name: String,
    /// The RAW input that state was entered with, as the parked record carries it.
    pub state_input: String,
    /// The concurrent states that state is nested inside, as the parked record
    /// carries them ([`ParkedJob::frames`]).
    pub frames: Vec<ParkedFrame>,
    /// What the job ended as.
    pub outcome: Result<Value, Failure>,
}

/// A worker's reply to the activity task an execution SUSPENDED at.
///
/// `SendTaskSuccess` is "used by activity workers [...] to report that the task
/// identified by the taskToken completed successfully": the output it carries IS the
/// result of the state that scheduled the task, so the state's own result-side data
/// flow is applied to it and the execution continues from there rather than waiting a
/// second time.
#[derive(Debug, Clone)]
pub struct ActivityReply {
    /// The state that scheduled the task being answered, which the stored task names.
    pub state_name: String,
    /// The RAW input that state was entered with, as the stored task carries it: what
    /// the state's `ResultPath` combines the result with.
    pub state_input: String,
    /// "The JSON output of the task", as the worker reported it.
    pub result: Value,
}

/// What a `SendTaskSuccess` or `SendTaskFailure` reported for the callback
/// (`.waitForTaskToken`) task an execution is SUSPENDED at.
///
/// The answer is the outcome of the state whose integration call carried the token,
/// so the walk continues from that state with it: `Ok` is the `output` a success
/// reported, which the state's own result-side data flow places, and `Err` the
/// `error` and `cause` a failure reported, which the state's `Retry` and `Catch`
/// answer for exactly as they answer for a call that failed.
#[derive(Debug, Clone)]
pub struct CallbackReply {
    /// The state whose call carried the token, which the stored task names.
    pub state_name: String,
    /// The RAW input that state was entered with, as the stored task carries it: what
    /// the state's `ResultPath` combines the result with.
    pub state_input: String,
    /// What the answer reported.
    pub outcome: Result<Value, Failure>,
    /// How many attempts each retrier of that state had already spent on the task,
    /// as the stored task carries it ([`ScheduledActivity::retries_taken`]).
    ///
    /// The continuation seeds its own retrier counts from this, so a state's
    /// `MaxAttempts` counts every attempt the EXECUTION has made at it rather than
    /// restarting at each answer.
    pub retries_taken: Vec<u64>,
}

/// The step a redrive continues an execution from: the state the attempt stopped at,
/// and the raw input it was entered with.
///
/// Recorded when an attempt stops at a state rather than getting past it, so a later
/// `RedriveExecution` continues "the failed execution from the unsuccessful step" with
/// the data that step saw instead of running the whole definition again.
#[derive(Debug, Clone)]
pub struct RedriveStep {
    /// The state the attempt stopped at, which the redriven attempt re-enters.
    pub state_name: String,
    /// The RAW input that state was entered with, as the attempt recorded it: what the
    /// results of the steps before it produced, so those steps are not re-run to reach
    /// them.
    pub state_input: String,
}

impl RedriveStep {
    /// The bytes this record retains, counted into the execution's own.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>() + self.state_name.len() + self.state_input.len()
    }
}

/// What the context object (`$$`) reports about the run being interpreted.
///
/// Every field is a value the caller's own request or the execution's own record
/// already fixed, so a definition that reads `$$.Execution.Id` reads this execution's
/// ARN rather than a placeholder.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub execution_arn: String,
    pub execution_name: String,
    pub state_machine_arn: String,
    pub state_machine_name: String,
    pub role_arn: String,
    /// How many times the execution being interpreted has been redriven, which it
    /// reads through `$$.Execution.RedriveCount`: `0` on the attempt a start began,
    /// and the count the record carries on an attempt a redrive began. It is the SAME
    /// integer `DescribeExecution` projects as `redriveCount`, because both come from
    /// the one execution record.
    pub redrive_count: i32,
    /// Whether the state machine being run is an EXPRESS workflow.
    ///
    /// Not a member of the context object a definition reads: it is the workflow type
    /// the execution's own record carries, and the walk needs it because one
    /// integration PATTERN is served only for the other workflow type — a `Task`
    /// state asking for the run-a-job (`.sync`) pattern inside an Express workflow
    /// fails at that state instead of starting a job
    /// ([`EXPRESS_RUN_A_JOB_UNSUPPORTED`]).
    pub express: bool,
}

impl Context {
    /// The context object a definition reads through `$$`, without the per-state
    /// members [`Interpreter::context_for`] adds.
    fn value(&self, input: &Value, start: SystemTime) -> Value {
        json!({
            "Execution": {
                "Id": self.execution_arn,
                "Input": input,
                "Name": self.execution_name,
                "RoleArn": self.role_arn,
                "StartTime": iso(start),
                "RedriveCount": self.redrive_count,
            },
            "StateMachine": {
                "Id": self.state_machine_arn,
                "Name": self.state_machine_name,
            },
        })
    }
}

/// Why an execution, a `Map` iteration or a `Parallel` branch failed: the `error` and
/// `cause` the service reports, which `Catch` and `Retry` match against.
#[derive(Debug, Clone)]
pub struct Failure {
    pub error: Option<String>,
    pub cause: Option<String>,
}

impl Failure {
    /// This failure, at the offset into the run where it happened.
    fn at(self, at: Duration) -> Fault {
        Fault { at, failure: self }
    }

    /// The error name `Retry`'s and `Catch`'s `ErrorEquals` match against.
    fn name(&self) -> &str {
        self.error.as_deref().unwrap_or("")
    }

    /// The `{Error, Cause}` document a `Catch` hands to the state it transitions to.
    fn payload(&self) -> Value {
        let mut object = Map::new();
        if let Some(error) = &self.error {
            object.insert("Error".to_string(), Value::String(error.clone()));
        }
        if let Some(cause) = &self.cause {
            object.insert("Cause".to_string(), Value::String(cause.clone()));
        }
        Value::Object(object)
    }
}

/// A failure, and where in the run it happened.
#[derive(Debug, Clone)]
struct Fault {
    at: Duration,
    failure: Failure,
}

/// What one piece of a state's work produced: its outcome, and the offset into the
/// run at which it ended. This is what `Retry` re-runs and what `Catch` catches.
type Attempt = (Result<Value, Failure>, Duration);

/// A failure the interpreter itself raised, carrying ASL's own [`RUNTIME_ERROR`] name
/// and a cause that says what was not run.
fn runtime(cause: String) -> Failure {
    named(RUNTIME_ERROR, cause)
}

/// A failure carrying one of ASL's other predefined error names.
fn named(error: &str, cause: String) -> Failure {
    Failure {
        error: Some(error.to_string()),
        cause: Some(cause),
    }
}

/// What a state REPORTS for work outside the walk that failed: the error name and the
/// cause that work answered, which is what `Retry` and `Catch` are matched against.
///
/// The one conversion between the two halves of a failure — what a call or a read
/// answered ([`super::integration::Failed`]) and what a state reports ([`Failure`]) —
/// so neither side restates the other's shape.
fn reported(failed: super::integration::Failed) -> Failure {
    Failure {
        error: Some(failed.error),
        cause: Some(failed.cause),
    }
}

/// A JSONata template of a state that was not evaluated, under the ASL error name
/// for the class the refusal belongs to: the language's data-limit failure for a
/// template that outgrew what a state may carry, and
/// [`QUERY_EVALUATION_ERROR`] for every expression that was not evaluated.
fn jsonata_failure(refusal: super::jsonata::Refusal) -> Failure {
    match refusal {
        // What the expressions of one state build is that state's own result, so a
        // template past the ceiling is the same failure a state whose document outgrew
        // what it may carry raises, worded the one way the service words it: the state
        // itself is named by the state being executed ([`diagnosed`]).
        super::jsonata::Refusal::TooLarge(_) => {
            named(DATA_LIMIT_ERROR, result_too_large(UNNAMED_PRODUCER))
        }
        super::jsonata::Refusal::NotEvaluated(why) => named(QUERY_EVALUATION_ERROR, why),
        // An expression that THREW and was not reported under the field it is written
        // in — a template whose own evaluation is not a field of the state — carries
        // the engine's account alone, under the same error name every expression that
        // was not evaluated fails with.
        super::jsonata::Refusal::Threw(account) => named(QUERY_EVALUATION_ERROR, account),
    }
}

/// What one state's turn produced.
struct StateStep {
    /// The state's output, which is the next state's input.
    value: Value,
    /// The offset into the run at which the state finished.
    at: Duration,
    next: Next,
}

/// What a `Choice` state decided: the state it transitions to, and the rule that
/// decided it.
struct Chosen {
    next: String,
    /// The rule that MATCHED, for a JSONata `Choice`: its own POSITION in the state's
    /// `Choices` array, which is where its members are written, and the rule itself —
    /// whose `Assign` and `Output` are the state's, applied over the state's own
    /// ([`Interpreter::choice_output`]). `None` where the `Default` was taken, and for
    /// a JSONPath rule, which carries neither.
    rule: Option<(usize, Map<String, Value>)>,
}

/// Where the walk goes after a state.
enum Next {
    /// The named state.
    Named(String),
    /// Nowhere: this branch (or the execution) ends with the state's output.
    EndBranch,
    /// Nowhere YET: the state scheduled an activity task
    /// ([`Interpreter::waiting`]) or started the job of a run-a-job integration
    /// ([`Interpreter::parked`]), so the walk stops there and the execution waits —
    /// for the worker that polls for the task, or for the job to finish.
    Suspended,
}

/// What one walk of a state list produced.
struct Branch {
    outcome: Result<Value, Failure>,
    /// The offset into the run at which the branch ended.
    end: Duration,
    /// Whether the branch SUSPENDED rather than ending: one of its states started the
    /// job of a run-a-job integration ([`Interpreter::parked`]), so the branch has no
    /// result yet and the state that encloses it has none either.
    suspended: bool,
}

/// One unit of a state's CONCURRENT work — a `Parallel` branch, or one iteration of a
/// `Map` state — as the walk ran it, for [`Interpreter::abort_units`].
struct Unit {
    /// Where this unit's own history events begin in the walk's list.
    from: usize,
    /// Where the Map Runs this unit's work records begin in the walk's list of them,
    /// so a failure that stops these units judges only the Map Runs THEY started and
    /// leaves the ones recorded before they began alone.
    map_runs_from: usize,
    /// The event this unit records when the state's failure stopped it, for a kind of
    /// unit the language records one for: a `Map` iteration is aborted in its own
    /// right, while a `Parallel` branch is reported through the states it was in.
    aborted: Option<Step>,
}

/// The suffix of the event type every state records when it is ENTERED, and the one it
/// records when it is LEFT: the prefix is the state's own type, which is what says
/// which aborted event an in-flight state records
/// ([`aborted_event`]).
const STATE_ENTERED: &str = "StateEntered";
const STATE_EXITED: &str = "StateExited";

/// The event a state of a given type records when the work around it was aborted,
/// where the service models one.
///
/// Only a state that can still be RUNNING when the abort reaches it has one:
/// `HistoryEventType` models `WaitStateAborted`, `TaskStateAborted`, `MapStateAborted`
/// and `ParallelStateAborted` and no others
/// (`smithy_query(query="shape",shape="HistoryEventType")`), because a `Pass`, a
/// `Choice`, a `Succeed` and a `Fail` leave in the same instant they are entered.
fn aborted_event(state_type: &str) -> Option<&'static str> {
    match state_type {
        "Wait" => Some("WaitStateAborted"),
        "Task" => Some("TaskStateAborted"),
        "Map" => Some("MapStateAborted"),
        "Parallel" => Some("ParallelStateAborted"),
        _ => None,
    }
}

/// The `$$.Map.Item` of the enclosing `Map` iteration, if the walk is inside one.
#[derive(Debug, Clone, Default)]
struct Scope {
    map_item: Option<Value>,
}

/// A `Map` state's plan: everything resolved from its members before its items run.
struct MapPlan<'a> {
    items: &'a [Value],
    /// The state's own name: "the name of the iteration's parent Map state", which
    /// every one of its iterations' events reports.
    name: &'a str,
    /// The state's `Label`, else its own name: the field the Map Run's ARN and its
    /// children's own ARNs carry.
    label: &'a str,
    /// The `ItemProcessor`'s (or legacy `Iterator`'s) own state list and entry point.
    states: &'a Map<String, Value>,
    start_at: &'a str,
    /// `ItemSelector`, or the legacy `Parameters` form of it. `None` for a state whose
    /// items are already the documents its iterations are handed — which a BATCHED
    /// state's are, because the selector was applied to each item before it was placed
    /// in a batch.
    selector: Option<&'a Value>,
    /// The query language the `Map` state itself is written in, which its own members
    /// — its `ItemSelector` included — are evaluated in. A state of the processor
    /// declares its own.
    language: &'static str,
    /// Whether the processor declares `"Mode": "DISTRIBUTED"`.
    distributed: bool,
    /// Whether the state declares an `ItemBatcher`, so each of `items` is a BATCH of
    /// items rather than one item: what an iteration reports as the count it processed
    /// is read off the batch then ([`super::item_batcher::item_count`]).
    batched: bool,
    /// How many items run at once; `0` is `MaxConcurrency`'s documented "no limit".
    concurrency: usize,
    tolerated_count: u64,
    tolerated_percentage: f64,
    /// Whether the state declares any tolerated-failure member at all.
    tolerance_declared: bool,
    /// What the attempt being CONTINUED ran each item as, by item index, for a walk
    /// that continues one ([`super::integration::ResumedMapRun::items`]): the items
    /// whose child workflow execution already succeeded are not run again. `None` for
    /// a walk that BEGINS an attempt, every item of which runs.
    resumed: Option<&'a [Option<super::integration::FinishedChild>]>,
}

/// A `Parallel` state's branches: each one's state list and entry point.
struct ParallelPlan<'a> {
    branches: Vec<(&'a Map<String, Value>, &'a str)>,
}

/// The name the ONE state `TestState` tests is declared under while it is judged.
///
/// A `definition` that carries the state alone gives it no name, and every diagnostic
/// the walk words names the state it was found in, so the state is judged under this
/// one.
const SINGLE_STATE_NAME: &str = "TheStateUnderTest";

/// Why `definition` is not the definition of a SINGLE state this service accepts, or
/// `Ok` when it is — what `TestState` takes when it names no `stateName`, since it
/// "accepts the definition of a single state".
///
/// The state is judged by the SAME walk every state of a state machine is, by
/// declaring it as the one state of a machine: each rule of the Amazon States
/// Language then applies to it unchanged, down to the machines nested inside it and
/// the query language it is written in. Two things about a LONE state are declared
/// around it rather than checked against it, because no definition encloses it:
///
/// * its transition targets name states no `States` map declares, so each one is
///   declared beside it as a `Succeed` state — `TestState` REPORTS the target it
///   names as `nextState` rather than resolving it;
/// * it need not say where it goes at all ("if you haven't defined a next state in
///   your definition [...] this field doesn't contain a value"), where a state
///   DECLARED in a machine must carry `Next` or `"End": true`, so a state naming no
///   transition is declared as the machine's terminal state.
///
/// Nothing else about the state is altered, and the `Err` is what the service reports
/// about THAT state: a required member the supplied object does not carry is named on
/// its own ([`missing_field_message`]), and every other diagnostic is stated without
/// the name and the JSON pointer the enclosing machine was declared under, neither of
/// which the request carries ([`about_the_single_state`]).
pub fn validate_single_state(definition: &str) -> Result<(), String> {
    let Ok(state) = serde_json::from_str::<Value>(definition) else {
        // A definition that is not JSON at all is refused in the same words whichever
        // form it was sent in, by the walk that words every other definition refusal.
        return match validate(definition) {
            Err(why) => Err(why),
            Ok(()) => Err(error(
                INVALID_JSON_DESCRIPTION,
                None,
                "the definition is not valid JSON",
            )
            .stated()),
        };
    };
    // The definition IS the state, so a member the language requires of a state is
    // missing from the object the REQUEST sent: it is reported as that member, and
    // nothing is said about a state machine the request does not carry.
    if let Some(member) = missing_required_member(&state) {
        return Err(missing_field_message(&member));
    }
    let mut judged = state.clone();
    if let Some(object) = judged.as_object_mut() {
        let continuing = object
            .get("Type")
            .and_then(Value::as_str)
            .is_some_and(|state_type| CONTINUING_STATE_TYPES.contains(&state_type));
        if continuing && !object.contains_key("Next") && !object.contains_key("End") {
            object.insert("End".to_string(), Value::Bool(true));
        }
    }
    let mut states = Map::new();
    states.insert(SINGLE_STATE_NAME.to_string(), judged);
    for target in transition_targets(&state) {
        states
            .entry(target)
            .or_insert_with(|| json!({ "Type": "Succeed" }));
    }
    let machine = json!({ "StartAt": SINGLE_STATE_NAME, "States": Value::Object(states) });
    let mut found = Diagnosis::until_first_error();
    diagnose_machine(
        &machine,
        "",
        JSONPATH,
        1,
        false,
        Outer::Nothing,
        &mut found,
    );
    match found.diagnostics.iter().find(|found| found.is_error()) {
        Some(found) => Err(about_the_single_state(found)),
        None => Ok(()),
    }
}

/// The members the Amazon States Language requires of a state, as alternatives: a
/// state carries the first member of each entry, or one of the members beside it.
///
/// Every state names what it does with `Type`, and a type whose work ONE member
/// declares requires that member too: a `Task` "requires a `Resource` field", a
/// `Choice` carries the `Choices` array its rules live in, a `Parallel` the `Branches`
/// it runs, and a `Map` the `ItemProcessor` its items go through (the language's
/// earlier `Iterator` names the same work). A `Pass`, `Succeed`, `Fail` or `Wait`
/// state declares its work through members none of which it must carry alone.
fn required_state_members(state_type: &str) -> &'static [&'static [&'static str]] {
    match state_type {
        "Task" => &[&["Resource"]],
        "Choice" => &[&["Choices"]],
        "Parallel" => &[&["Branches"]],
        "Map" => &[&["ItemProcessor", "Iterator"]],
        _ => &[],
    }
}

/// The first member `state` must carry and does not: `Type`, and then the member the
/// type it names requires ([`required_state_members`]).
///
/// `None` for a state that carries each of them, and for a `definition` that is not a
/// state object at all — which is not a member that is missing, and is reported as the
/// object it is instead.
fn missing_required_member(state: &Value) -> Option<String> {
    let state = state.as_object()?;
    if !state.contains_key("Type") {
        return Some("Type".to_string());
    }
    // A `Type` that is present but is not a string names no state type, so which
    // members the state owes is not decided here: that is a defect in the member it
    // carries rather than a member it is missing.
    let state_type = state.get("Type").and_then(Value::as_str)?;
    required_state_members(state_type)
        .iter()
        .find(|alternatives| {
            !alternatives
                .iter()
                .any(|member| state.contains_key(*member))
        })
        .and_then(|alternatives| alternatives.first().map(|member| (*member).to_string()))
}

/// What the service reports for a required member the state under test does not
/// carry: the member, named on its own.
///
/// The request sent ONE state object, so there is no state name and no path into an
/// enclosing definition to place the member in.
fn missing_field_message(member: &str) -> String {
    format!("Missing required field: '{}'", echoed(member))
}

/// What a diagnostic of the WRAPPED walk says about the state the request supplied.
///
/// The lone state is judged by declaring it as the one state of a state machine, so
/// each diagnostic names the state under that declared name and locates it inside that
/// machine. Neither is in the request, so the name is reported as the state under test
/// and the location is read back to the state's own member — `None` where it was the
/// state itself.
fn about_the_single_state(found: &Diagnostic) -> String {
    let declared = format!("state '{SINGLE_STATE_NAME}'");
    let message = found.message.replace(&declared, SINGLE_STATE_LABEL);
    let inside = format!("/States/{SINGLE_STATE_NAME}");
    let member = found
        .location
        .as_deref()
        .and_then(|location| location.strip_prefix(inside.as_str()))
        .filter(|member| !member.is_empty());
    match member {
        Some(member) => format!("{}: {message} at {member}", found.code),
        None => format!("{}: {message}", found.code),
    }
}

// ---------------------------------------------------------------------------
// What a state REFERENCES
// ---------------------------------------------------------------------------

/// `$` terms that are not variables.
///
/// `$states` is the reserved context object Step Functions exposes to an
/// expression (`$states.input`, `$states.context`, `$states.result`), not a
/// variable a state assigned.
const RESERVED_TERMS: &[&str] = &["states"];

/// The variables each state of `definition` references, keyed by state name.
///
/// `DescribeStateMachine`'s `variableReferences` is "a map of state name to a
/// list of variables referenced by that state. States that do not use variable
/// references will not be shown in the response" — so a state that references
/// none is absent, and a definition whose states reference none answers an empty
/// map, which the caller sees as no member at all.
///
/// A variable reference is a `$name` term of one of the state's own EXPRESSIONS,
/// which is where both of the service's query languages read one:
///
/// * a JSONata expression, which the language delimits with `{%` and `%}` (the
///   `Assign`, `Output`, `Arguments`, `Condition`, … members of a state whose
///   `QueryLanguage` is JSONata);
/// * a JSONPath reference field, which the Amazon States Language spells as a
///   member whose name ends in `.$` and whose value is a path or a reference.
///
/// Nothing else in the state is scanned, so a literal `Result` that happens to
/// carry a `$` is not mistaken for a reference. A nested `States` map — a
/// `Parallel` branch or a `Map` processor — is walked as the states it declares,
/// so each state's references are reported under ITS own name and never folded
/// into its parent's.
///
/// The definition is parsed with `serde_json`, whose own recursion limit bounds
/// how deep the walk below can go; the extraction of the `$name` terms from one
/// expression is lexical, which is why no expression parser is needed for it —
/// evaluating one is [`super::jsonata`]'s business, not this report's.
pub fn variable_references(definition: &str) -> BTreeMap<String, Vec<String>> {
    let mut references = BTreeMap::new();
    let Ok(document) = serde_json::from_str::<Value>(definition) else {
        return references;
    };
    if let Some(states) = document.get("States").and_then(Value::as_object) {
        collect_state_references(states, &mut references);
    }
    references
}

/// Report the references of every state `states` declares, and of every state
/// nested inside one of them.
///
/// The Amazon States Language scopes a state name to the machine it is declared
/// in, so a `Parallel` branch may declare a name a top-level state also uses.
/// The map is flat, as the wire shape is, so two states of one name are MERGED
/// rather than one silently replacing the other.
fn collect_state_references(
    states: &Map<String, Value>,
    into: &mut BTreeMap<String, Vec<String>>,
) {
    for (name, state) in states {
        let mut own = BTreeSet::new();
        scan_state(state, false, &mut own, into);
        if own.is_empty() {
            continue;
        }
        let entry = into.entry(name.clone()).or_default();
        own.extend(entry.drain(..));
        entry.extend(own);
    }
}

/// Walk one state, collecting ITS references into `own` and the references of
/// any state nested inside it into `nested`.
///
/// `reference_field` is whether the value being walked is the value of a `.$`
/// member, which is what makes a JSONPath string a reference rather than a
/// literal.
fn scan_state(
    value: &Value,
    reference_field: bool,
    own: &mut BTreeSet<String>,
    nested: &mut BTreeMap<String, Vec<String>>,
) {
    match value {
        Value::String(text) => {
            for expression in jsonata_expressions(text) {
                scan_expression(expression, own);
            }
            if reference_field {
                scan_expression(text, own);
            }
        }
        Value::Array(items) => {
            for item in items {
                scan_state(item, reference_field, own, nested);
            }
        }
        Value::Object(members) => {
            // A nested state machine — a `Parallel` branch, a `Map`'s
            // `ItemProcessor`/`Iterator` — is an object carrying BOTH a string
            // `StartAt` and an object `States`, exactly as the top-level
            // definition does. Requiring the pair is what keeps a literal
            // payload member that happens to be called `States` from being
            // walked as states and injecting names of its own.
            let sub_machine = members
                .get("StartAt")
                .and_then(Value::as_str)
                .is_some_and(|start_at| !start_at.is_empty());
            for (member, child) in members {
                match child.as_object() {
                    Some(states) if member == "States" && sub_machine => {
                        collect_state_references(states, nested);
                    }
                    _ => scan_state(child, member.ends_with(".$"), own, nested),
                }
            }
        }
        _ => {}
    }
}

/// Every JSONata expression `text` carries: the language embeds one between
/// `{%` and `%}`.
fn jsonata_expressions(text: &str) -> Vec<&str> {
    let mut expressions = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{%") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("%}") else {
            break;
        };
        expressions.push(&after[..close]);
        rest = &after[close + 2..];
    }
    expressions
}

/// Every variable one expression references.
///
/// A term is `$` followed by an identifier. `$$` names the context, `$` alone
/// the state's input, `$name(` is a function call and [`RESERVED_TERMS`] names
/// the context object — none of them is a variable.
fn scan_expression(expression: &str, into: &mut BTreeSet<String>) {
    let bytes = expression.as_bytes();
    let mut index = 0;
    while let Some(offset) = expression[index..].find('$') {
        let start = index + offset;
        let mut end = start + 1;
        while bytes.get(end).is_some_and(|byte| is_member_byte(*byte)) {
            end += 1;
        }
        index = end.max(start + 1);
        let name = &expression[start + 1..end];
        let identifier = name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_');
        if !identifier
            || (start > 0 && bytes[start - 1] == b'$')
            || bytes.get(end) == Some(&b'(')
            || RESERVED_TERMS.contains(&name)
        {
            continue;
        }
        into.insert(name.to_string());
    }
}

/// Walk `definition` from its `StartAt` state with `input` as the first state's
/// input, and report what happened.
///
/// `tasks` is what a `Task` state's service integration is performed by: the walk
/// asks for the call and the operation running the walk makes it
/// ([`super::integration::interpret`]).
///
/// `first_event_id` is the id the first history event this walk records will carry,
/// which a cause naming the event a state was entered at reports.
///
/// `settle` is how long after this walk's outcomes the clock makes them due, which the
/// caller decides ([`super::EXECUTION_SETTLE_DELAY`], and `Duration::ZERO` for a
/// synchronous start): the walk never sleeps for it, and what it needs it for is the
/// instant a child workflow execution of a Distributed Map STOPS at, which the results
/// that run writes report ([`Interpreter::settle`]).
pub fn run(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    first_event_id: i64,
    settle: Duration,
    tasks: &dyn Tasks,
) -> Run {
    interpret(
        definition,
        input,
        start,
        context,
        Entry::Start,
        Duration::ZERO,
        first_event_id,
        settle,
        tasks,
    )
    .0
}

/// Walk `definition` again from the step a redriven execution did not get past, and
/// report what happened.
///
/// "When you redrive an execution, it continues the failed execution from the
/// unsuccessful step and uses the same input. Step Functions preserves the results and
/// execution history of the successful steps, and doesn't rerun these steps when you
/// redrive an execution": the walk begins AT the state `from` names, with the raw
/// input that state was entered with, so no state the attempt already got past is run
/// a second time and the values those states produced — which `from`'s own input
/// carries — stand for the redriven attempt.
///
/// The state RUNS: it is the unsuccessful step, so a `Task` state there makes its
/// service-integration call again, or schedules its activity task again, rather than
/// being handed a result ([`resume`] is the walk that is handed one). `tasks` is the
/// same seam [`run`] takes, so that call is made exactly as the first attempt's was.
#[allow(clippy::too_many_arguments)]
pub fn redrive(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    from: &RedriveStep,
    first_event_id: i64,
    settle: Duration,
    tasks: &dyn Tasks,
) -> Run {
    let state_input = serde_json::from_str(&from.state_input).unwrap_or(Value::Null);
    let mut run = interpret(
        definition,
        input,
        start,
        context,
        Entry::At(from.state_name.clone(), state_input),
        Duration::ZERO,
        first_event_id,
        settle,
        tasks,
    )
    .0;
    // "Step Functions preserves the results and execution history of the successful
    // steps, and doesn't rerun these steps": the step this attempt continues FROM was
    // already ENTERED in the attempt whose history the redrive preserves, so its
    // entered event is not recorded a second time. What this attempt records is the
    // work that step does from there on — the outcome it reaches, and every state
    // after it.
    if run.steps.first().is_some_and(|step| {
        step.state_name.as_deref() == Some(from.state_name.as_str())
            && step.event_type.ends_with(STATE_ENTERED)
    }) {
        run.steps.remove(0);
    }
    run
}

/// Walk `definition` on from the activity task `reply` answers, and report what
/// happened.
///
/// A worker's reply is the result of the state that scheduled its task, so the walk
/// begins AT that state — with the raw input the state was entered with the first
/// time, which `reply` carries and which its `ResultPath` combines the reply with —
/// applies the reply there instead of scheduling a task, and carries on from its
/// `Next`. The states before it already ran, under the call that scheduled the task,
/// so neither their work nor their events are repeated: what this reports begins at
/// the reply.
///
/// `input` is the EXECUTION's own input, which the context object (`$$`) reports; it
/// is not the document the walk starts from.
///
/// `elapsed` is how long the execution has ALREADY been running when the reply
/// arrives, which is what a definition-level `TimeoutSeconds` bounds: the span a task
/// spent outstanding is part of the execution's own run.
///
/// `tasks` is the same seam [`run`] takes: the states after the reply may themselves
/// be service integrations, and they are called exactly as the first attempt's were.
///
/// `None` when the definition declares no such state any more, so there is nothing to
/// continue from.
#[allow(clippy::too_many_arguments)]
pub fn resume(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    reply: &ActivityReply,
    elapsed: Duration,
    first_event_id: i64,
    settle: Duration,
    tasks: &dyn Tasks,
) -> Option<Run> {
    let state_input: Value = serde_json::from_str(&reply.state_input).ok()?;
    let (run, applied) = interpret(
        definition,
        input,
        start,
        context,
        Entry::Reply(reply.clone(), state_input),
        elapsed,
        first_event_id,
        settle,
        tasks,
    );
    applied.then_some(run)
}

/// Walk `definition` on from the run-a-job task whose job has ENDED, and report what
/// happened.
///
/// The job the state's integration started is its unit of work, so what that job
/// ended as is the state's own outcome: the walk begins AT that state — with the raw
/// input it was entered with the first time, which `reply` carries and which its
/// `ResultPath` combines the job's report with — reports the outcome there instead of
/// starting the job again, and carries on from its `Next` (or into its `Catch`, for a
/// job that ended without doing its work). The states before it already ran, under
/// the call that started the job, so neither their work nor their events are
/// repeated.
///
/// `input` is the EXECUTION's own input, which the context object (`$$`) reports; it
/// is not the document the walk starts from. `elapsed` is how long the execution had
/// already been running when the job ended, which a definition-level `TimeoutSeconds`
/// bounds: the span a job stayed unfinished is part of the execution's own run.
///
/// `None` when the definition declares no such state any more, so there is nothing to
/// continue from.
#[allow(clippy::too_many_arguments)]
pub fn resume_job(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    reply: &JobReply,
    elapsed: Duration,
    first_event_id: i64,
    settle: Duration,
    tasks: &dyn Tasks,
) -> Option<Run> {
    let state_input: Value = serde_json::from_str(&reply.state_input).ok()?;
    let (run, applied) = interpret(
        definition,
        input,
        start,
        context,
        Entry::Job(reply.clone(), state_input),
        elapsed,
        first_event_id,
        settle,
        tasks,
    );
    applied.then_some(run)
}

/// Walk `definition` on from the callback (`.waitForTaskToken`) task a `SendTask*`
/// has answered, and report what happened.
///
/// The answer is the outcome of the state whose integration call carried the token,
/// so the walk begins AT that state — with the raw input it was entered with the
/// first time, which `reply` carries — reports that outcome there instead of making
/// the call again, and carries on from its `Next` (or into its `Retry` and `Catch`,
/// for an answer that reported a failure). The states before it already ran, under
/// the call that suspended the execution, so neither their work nor their events are
/// repeated.
///
/// `input` is the EXECUTION's own input, which the context object (`$$`) reports;
/// `elapsed` is how long the execution had already been running when the answer
/// arrived, which a definition-level `TimeoutSeconds` bounds.
///
/// `None` when the definition declares no such state any more, so there is nothing to
/// continue from.
#[allow(clippy::too_many_arguments)]
pub fn resume_callback(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    reply: &CallbackReply,
    elapsed: Duration,
    first_event_id: i64,
    settle: Duration,
    tasks: &dyn Tasks,
) -> Option<Run> {
    let state_input: Value = serde_json::from_str(&reply.state_input).ok()?;
    let (run, applied) = interpret(
        definition,
        input,
        start,
        context,
        Entry::Callback(reply.clone(), state_input),
        elapsed,
        first_event_id,
        settle,
        tasks,
    );
    applied.then_some(run)
}

/// Where a walk of a definition begins, and what it does when it gets there.
enum Entry {    /// At the definition's `StartAt`, with the execution's own input: an attempt of
    /// its own.
    Start,
    /// At the state whose activity task a worker's reply answers, with the raw input
    /// that state was entered with: the reply is that state's RESULT, so the state
    /// does not run again.
    Reply(ActivityReply, Value),
    /// At the state whose run-a-job integration started the job that has now ended,
    /// with the raw input that state was entered with: what the job ended as is that
    /// state's outcome, so the job is not started again.
    Job(JobReply, Value),
    /// At the state whose callback (`.waitForTaskToken`) call carried the token a
    /// `SendTask*` has now quoted, with the raw input that state was entered with:
    /// what the answer reported is that state's outcome, so the call is not made
    /// again.
    Callback(CallbackReply, Value),
    /// At the state a redriven execution did not get past, with the raw input that
    /// state was entered with: the state RUNS, because it is the step the attempt
    /// stopped at.
    At(String, Value),
}

/// The definition-level `TimeoutSeconds` `document` declares, as a span.
///
/// "TimeoutSeconds (Optional) … The maximum number of seconds an execution of the state
/// machine can run. If it runs longer than the specified time, the execution fails with
/// a States.Timeout error name." It bounds the whole execution, so it is read once —
/// before the walk, which STOPS at it ([`Interpreter::past_timeout`]), and after it, to
/// report a run that got past it ([`interpret`]) — and a value that is not a whole
/// number of seconds declares no bound to measure.
fn definition_timeout(document: &Value) -> Option<Duration> {
    document
        .get("TimeoutSeconds")
        .and_then(Value::as_u64)
        .map(Duration::from_secs)
}

/// One walk of `definition`, and whether the reply [`Entry::Reply`] carries was
/// applied in it.
///
/// `entry` is where the walk begins: at `StartAt`, at the state a reply answers, or at
/// the step a redrive continues from.
///
/// `elapsed` is the span the execution had already spent before this walk's first
/// state, which a definition-level `TimeoutSeconds` counts.
///
/// `first_event_id` is the id the first event this walk records carries
/// ([`Interpreter::first_event_id`]).
#[allow(clippy::too_many_arguments)]
fn interpret(
    definition: &str,
    input: &str,
    start: SystemTime,
    context: &Context,
    entry: Entry,
    elapsed: Duration,
    first_event_id: i64,
    settle: Duration,
    tasks: &dyn Tasks,
) -> (Run, bool) {
    // The work this walk may do, installed for the thread it runs on and restored when
    // it ends: every primitive the walk reaches charges what it spends to this, and
    // what runs on this thread afterwards is metered by its own allowance
    // ([`work_meter`]).
    let _allowance = WorkAllowance::of(MAX_INTERPRET_WORK);
    let document: Value = match serde_json::from_str(definition) {
        Ok(document) => document,
        Err(why) => {
            return (
                failed(
                    Vec::new(),
                    Duration::ZERO,
                    runtime(format!(
                        "the state machine definition is not valid JSON: {}",
                        crate::state::json_parse_diagnostic(definition, &why)
                    )),
                ),
                false,
            );
        }
    };
    let (Some(start_at), Some(states)) = (
        document.get("StartAt").and_then(Value::as_str),
        document.get("States").and_then(Value::as_object),
    ) else {
        return (
            failed(
                Vec::new(),
                Duration::ZERO,
                runtime(
                    "the state machine definition carries no 'StartAt' and 'States' pair"
                        .to_string(),
                ),
            ),
            false,
        );
    };
    // The query language the definition is written in, which every state inherits
    // unless it declares one of its own: both of the languages `QueryLanguage`
    // selects between are evaluated ([`super::jsonata`] for JSONata), so a definition
    // declaring either is RUN. A value that is neither is no query language the
    // language defines, which `validate` refuses at create time and the walk reports
    // here for a definition stored before it did.
    let language = match declared_language(document.get("QueryLanguage")) {
        Ok(Some(declared)) => declared,
        Ok(None) => JSONPATH,
        Err(why) => {
            return (
                failed(Vec::new(), Duration::ZERO, runtime(why)),
                false,
            );
        }
    };
    let input_value: Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(why) => {
            return (
                failed(
                    Vec::new(),
                    Duration::ZERO,
                    runtime(format!(
                        "the execution input is not valid JSON: {}",
                        crate::state::json_parse_diagnostic(input, &why)
                    )),
                ),
                false,
            );
        }
    };

    // A walk that CONTINUES an execution begins at the state whose task the reply
    // answers, or at the step a redrive continues from, with the raw input that state
    // was entered with; one that is an attempt of its own begins at `StartAt` with the
    // execution's input. A state the definition no longer declares — the state machine
    // an execution started through an unqualified ARN has been updated since — is no
    // step to continue from, so such a walk is an attempt of its own.
    let (reply, job_reply, callback_reply, resume_frames, start_at, first_input) = match entry {
        Entry::Reply(reply, state_input) => {
            let state_name = reply.state_name.clone();
            (Some(reply), None, None, Vec::new(), state_name, state_input)
        }
        // A job the task of a `Parallel` BRANCH started is answered one level in:
        // the walk re-enters the concurrent states the parked record names, outermost
        // first, and the frames are what it picks up at each of them
        // ([`ParkedFrame`]).
        Entry::Job(reply, state_input) => {
            let frames = reply.frames.clone();
            match frames.first() {
                Some(frame) => {
                    let state_name = frame.state_name.clone();
                    let state_input = frame.state_input.clone();
                    (None, Some(reply), None, frames, state_name, state_input)
                }
                None => {
                    let state_name = reply.state_name.clone();
                    (None, Some(reply), None, Vec::new(), state_name, state_input)
                }
            }
        }
        Entry::Callback(reply, state_input) => {
            let state_name = reply.state_name.clone();
            (None, None, Some(reply), Vec::new(), state_name, state_input)
        }
        Entry::At(state_name, state_input) if states.contains_key(&state_name) => {
            (None, None, None, Vec::new(), state_name, state_input)
        }
        Entry::At(..) | Entry::Start => (
            None,
            None,
            None,
            Vec::new(),
            start_at.to_string(),
            input_value.clone(),
        ),
    };

    // The documents this walk was GIVEN as text, each with the bytes it is reported
    // in: the caller's own input, and every document the definition itself writes
    // ([`Supplied`]). The input is remembered LAST, so a document the caller sent and
    // the definition also writes is reported in the caller's own bytes.
    let mut supplied = Supplied::default();
    supplied.definition(definition, 0);
    supplied.supplied(input);
    let mut interpreter = Interpreter {
        budget: MAX_STATE_TRANSITIONS,
        depth: 0,
        call_wait_left: MAX_CALL_WAIT_DURATION,
        // What is left of a definition-level `TimeoutSeconds` for this walk: the span
        // the execution had already spent counts towards it, so what remains is what
        // this walk's own offsets are judged against — both as it goes and once it is
        // done (below).
        timeout_left: definition_timeout(&document).map(|timeout| timeout.saturating_sub(elapsed)),
        timed_out: false,
        retry_count: 0,
        steps: Vec::new(),
        first_event_id,
        start,
        settle,
        base_context: context.value(&input_value, start),
        tasks,
        waiting: None,
        parked: None,
        abandoned: Vec::new(),
        // Nothing has failed beside this walk: it is the execution's own, not a branch
        // of a `Parallel` state.
        cancel_after: None,
        // The concurrent states this walk has to re-enter to reach the state a job's
        // outcome answers; empty for every other walk.
        resume_frames,
        map_depth: 0,
        map_runs: Vec::new(),
        map_runs_entered: 0,
        map_runs_entered_by_label: BTreeMap::new(),
        reply,
        job_reply,
        callback_reply,
        resumed_attempts: None,
        attempts_spent: Vec::new(),
        resumed: None,
        // A child execution's ARN is minted in the state machine's OWN region, from
        // the state machine's own ARN, so it agrees with the parent execution's
        // however the request that started it was signed.
        machine_name: context.state_machine_name.clone(),
        region: super::arn_region(&context.state_machine_arn),
        // The execution this walk runs, which the `taskToken` a callback task's call
        // carries names ([`Interpreter::fresh_task_token`]).
        execution_arn: context.execution_arn.clone(),
        child_counter: 0,
        express: context.express,
        // Set by the walk, if a state of the definition fails.
        failed_at: None,
        // The query language the definition declares, which a state inherits unless
        // it declares one of its own.
        language,
        // A run has assigned nothing where it starts.
        variables: BTreeMap::new(),
        assigned: BTreeMap::new(),
        // The execution's own input, as the caller wrote it: a state's data that IS
        // that document is echoed in the caller's bytes rather than re-encoded.
        supplied: supplied.clone(),
    };
    // And the same documents for the diagnoses this walk's own expressions word, which
    // quote the document they could not be applied to ([`GivenDocuments`]).
    let _given = GivenDocuments::of(&supplied);
    let branch = interpreter.walk(
        states,
        &start_at,
        first_input,
        Duration::ZERO,
        &Scope::default(),
    );
    let mut waiting = interpreter.waiting.take();
    let mut parked = interpreter.parked.take();
    let abandoned = std::mem::take(&mut interpreter.abandoned);
    // Whether the walk STOPPED at the definition's own timeout rather than reaching an
    // outcome of its own, which the reporting below answers for.
    let stopped_at_timeout = interpreter.timed_out;
    let mut steps = interpreter.steps;
    // One execution's Map Runs are reported "in the order their states were entered",
    // which is the position each one carries. A nested Distributed Map inside another's
    // items finishes first, so the list is built in finish order and put in entry order
    // HERE, once nothing can prune it any more.
    let mut entered = interpreter.map_runs;
    entered.sort_by_key(|(entered, _)| *entered);
    let mut map_runs: Vec<StartedMapRun> =
        entered.into_iter().map(|(_, map_run)| map_run).collect();
    let mut end = branch.end;
    // A walk that applied a reply reports only what FOLLOWS it: the steps the replying
    // state writes before the reply is placed are the ones the call that scheduled the
    // task already recorded.
    let resumed = interpreter.resumed;
    if let Some(resumed) = resumed {
        steps.drain(..resumed.index.min(steps.len()));
        for step in &mut steps {
            step.after = step.after.saturating_sub(resumed.at);
        }
        map_runs.retain(|map_run| map_run.started_at >= resumed.at);
        for map_run in &mut map_runs {
            map_run.started_at = map_run.started_at.saturating_sub(resumed.at);
            map_run.stopped_at = map_run.stopped_at.saturating_sub(resumed.at);
            for item in &mut map_run.items {
                item.finished_at = item.finished_at.saturating_sub(resumed.at);
            }
            for child in &mut map_run.children {
                child.started_at = child.started_at.saturating_sub(resumed.at);
                child.finished_at = child.finished_at.saturating_sub(resumed.at);
            }
        }
        if let Some(waiting) = waiting.as_mut() {
            waiting.at = waiting.at.saturating_sub(resumed.at);
        }
        if let Some(parked) = parked.as_mut() {
            parked.at = parked.at.saturating_sub(resumed.at);
        }
        end = end.saturating_sub(resumed.at);
    }
    let applied = resumed.is_some();

    // A definition-level `TimeoutSeconds` bounds the whole execution: a run that got
    // past it ended TIMED_OUT at the timeout, whatever it went on to compute. The span
    // an execution had already spent before this walk — the time a task it was
    // suspended at stayed outstanding, say — counts towards it, so what is left of the
    // timeout for THIS walk is what its own offsets are judged against.
    if let Some(timeout) = definition_timeout(&document) {
        let remaining = timeout.saturating_sub(elapsed);
        if end > remaining || stopped_at_timeout {
            let kept = steps
                .into_iter()
                .filter(|step| step.after <= remaining)
                .collect();
            // A Map Run the walk only reached past the timeout never started, exactly
            // as the events past it never happened; one that had started stopped WHEN
            // THE EXECUTION DID, so its own stop cannot be later than the timeout.
            let kept_map_runs = map_runs
                .into_iter()
                .filter(|map_run| map_run.started_at <= remaining)
                .map(|mut map_run| {
                    map_run.stopped_at = map_run.stopped_at.min(remaining);
                    // An item whose iteration would only have finished past the
                    // timeout never finished: the execution ended first, so the Map
                    // Run never counted it.
                    map_run.items.retain(|item| item.finished_at <= remaining);
                    // And the child execution that item ran as never finished
                    // either, for the same reason.
                    map_run
                        .children
                        .retain(|child| child.finished_at <= remaining);
                    map_run
                })
                .collect();
            let mut timed_out = terminal(
                kept,
                remaining,
                super::EVENT_EXECUTION_TIMED_OUT,
                super::EXECUTION_STATUS_TIMED_OUT,
                None,
                // RECORDED: a machine-level timeout reports the STATUS and nothing
                // else — no `error` and no `cause` — unlike the task-level
                // `TimeoutSeconds`, which fails its state with [`TIMEOUT_ERROR`]. What
                // ran out is the execution's own allowance rather than anything a state
                // did, so there is no failure to name.
                None,
                None,
            )
            .with_map_runs(kept_map_runs);
            // The execution ended at the timeout, so a job it had started is waited
            // for by nothing: it is the caller's to stop, like a job a failed state
            // abandoned.
            timed_out.abandoned = abandoned;
            timed_out.abandoned.extend(parked);
            return (timed_out, applied);
        }
    }

    // An execution that SUSPENDED reached no outcome: the walk stopped at an activity
    // task, and what happens next is a worker's reply, or it parked at the job a
    // run-a-job integration started, and what happens next is that job finishing. So
    // the run reports what it is waiting for and no terminal event is written. Two
    // checks come FIRST, so a run that cannot reach it is not reported as waiting for
    // it: the definition-level timeout above, which a run's own `Wait` states can
    // already have spent, and the branch's outcome, which is a FAILURE where the
    // document the task would carry outgrew what a state may produce.
    if branch.outcome.is_ok() && (waiting.is_some() || parked.is_some()) {
        return (
            Run {
                status: super::EXECUTION_STATUS_RUNNING,
                output: None,
                error: None,
                cause: None,
                steps,
                elapsed: end,
                waiting,
                parked,
                abandoned,
                map_runs,
                // A run that is WAITING failed at nothing: what happens next is a
                // worker's reply or the job finishing.
                stopped_at: None,
            },
            applied,
        );
    }

    let mut run = match branch.outcome {
        // A state's input and output is a JSON DOCUMENT: the document the run ended
        // with is reported as [`echoed_payload`] reports every document this module
        // answers — the caller's own BYTES when no state transformed what they wrote,
        // and the encoding the walk produced it in when the definition built it.
        Ok(value) => {
            succeeded(steps, end, echoed_payload(&value, &supplied)).with_map_runs(map_runs)
        }
        Err(failure) => failed(steps, end, failure).with_map_runs(map_runs),
    };
    // The unsuccessful step this walk stopped at, for the redrive that continues the
    // execution from it.
    run.stopped_at = interpreter.failed_at;
    // A job the walk started and then abandoned — and the one it had parked at, where
    // the execution went on to fail anyway — is the caller's to STOP: the task that
    // started it will never finish, so the job it started is not left running behind
    // it.
    run.abandoned = abandoned;
    run.abandoned.extend(parked);
    (run, applied)
}

/// The run of a definition that ended at a `Succeed` state or at `"End": true`.
fn succeeded(steps: Vec<Step>, elapsed: Duration, output: String) -> Run {
    // A state's output is bounded by what the service can report it through, so a
    // run whose value outgrew that ends with ASL's own data-limit error rather than
    // storing a payload no response could carry.
    if output.len() > MAX_PAYLOAD_BYTES {
        return failed(
            steps,
            elapsed,
            named(
                DATA_LIMIT_ERROR,
                format!(
                    "the execution output is {} bytes, more than the {MAX_PAYLOAD_BYTES} a state's \
                     output may carry",
                    output.len()
                ),
            ),
        );
    }
    terminal(
        steps,
        elapsed,
        super::EVENT_EXECUTION_SUCCEEDED,
        super::EXECUTION_STATUS_SUCCEEDED,
        Some(output),
        None,
        None,
    )
}

/// The run of a definition that ended at a `Fail` state, or that could not be run.
fn failed(steps: Vec<Step>, elapsed: Duration, failure: Failure) -> Run {
    // `error` and `cause` leave through the modelled `SensitiveError` (0..256) and
    // `SensitiveCause` (0..32768) shapes. A `Fail` state may read its error from the
    // execution's own data, and a cause the interpreter wrote quotes the value it
    // could not handle, so neither is bounded by the definition alone. The RUN keeps
    // both as it computed them — a synchronous response and the log of an execution
    // carry the whole of what the state raised — and each reader holds them to what it
    // reports them through: an over-long error NAME is trimmed where it is reported
    // ([`trimmed_error`]) and an over-long cause is cut to the bytes the cause member
    // carries.
    terminal(
        steps,
        elapsed,
        super::EVENT_EXECUTION_FAILED,
        super::EXECUTION_STATUS_FAILED,
        None,
        failure.error,
        failure.cause.map(clip_cause),
    )
}

/// `error` as a reader bounded by the modelled `SensitiveError` (0..256) reports it: the
/// name itself when it fits, and as much of it as leaves room for [`ERROR_ELLIPSIS`]
/// inside that maximum when it does not.
///
/// A `Fail` state may name its error from the execution's own data (`ErrorPath`), so the
/// name is not bounded by the definition and is not ASCII either: the cut is made in
/// BYTES, which is what the maximum counts, and moved back to a character boundary, so
/// the trimmed name is both inside the bound and still text. A name past the maximum is
/// TRIMMED rather than replaced: what the state raised is what a caller has to see, and
/// the trim marks that the rest of it was cut. The whole name stays on the run itself,
/// which is what a synchronous response and an execution's log carry.
#[must_use]
pub fn trimmed_error(error: &str) -> String {
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_string();
    }
    let mut end = MAX_ERROR_BYTES - ERROR_ELLIPSIS.len();
    while end > 0 && !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ERROR_ELLIPSIS}", &error[..end])
}

/// What marks an error name [`trimmed_error`] cut, and is counted inside the maximum
/// the name is reported through.
const ERROR_ELLIPSIS: &str = "...";

/// `cause` within the bytes the modelled `SensitiveCause` carries, cut at a character
/// boundary.
fn clip_cause(cause: String) -> String {
    if cause.len() <= MAX_CAUSE_BYTES {
        return cause;
    }
    let mut end = MAX_CAUSE_BYTES;
    while end > 0 && !cause.is_char_boundary(end) {
        end -= 1;
    }
    cause[..end].to_string()
}

/// One run, with the terminal history event the execution ends on appended.
fn terminal(
    mut steps: Vec<Step>,
    elapsed: Duration,
    event_type: &'static str,
    status: &'static str,
    output: Option<String>,
    error: Option<String>,
    cause: Option<String>,
) -> Run {
    steps.push(Step {
        event_type,
        state_name: None,
        after: elapsed,
        terminal: true,
        details: super::EventDetails::default(),
    });
    Run {
        status,
        output,
        error,
        cause,
        steps,
        elapsed,
        // A run that reached a terminal event is waiting for nothing.
        waiting: None,
        parked: None,
        // Set by `interpret`, which is where the walk's own frame is in scope — as
        // `stopped_at` and the Map Runs are.
        abandoned: Vec::new(),
        stopped_at: None,
        // Set by `run`, which is where the walk's Map Runs are in scope.
        map_runs: Vec::new(),
    }
}

/// The diagnosis a document past what a state's output may carry fails with: the
/// service's own sentence about a result that outgrew the quota, naming what produced
/// it.
///
/// `produced_by` is the STATE's name for a state that built the document itself, and
/// the invoked resource for a `Task` state, which is what the service names there. A
/// document produced where the state's own name is not in scope — a payload template's
/// member, an intrinsic's result — carries [`UNNAMED_PRODUCER`] and is named by the
/// state being executed ([`diagnosed`]).
fn result_too_large(produced_by: &str) -> String {
    format!(
        "The state/task '{produced_by}' returned a result with a size exceeding the maximum \
         number of bytes service limit."
    )
}

/// What [`result_too_large`] names as the producer of a document measured where the
/// state that produced it is not in scope; the state being executed substitutes its own
/// name for it ([`is_unnamed_producer`], read by [`diagnosed`] and by the single-state
/// trial, which are the two places a failure is reported FOR a state).
const UNNAMED_PRODUCER: &str = "";

/// Whether `cause` is the quota sentence raised with no producer named
/// ([`UNNAMED_PRODUCER`]), which the state being executed names itself in.
fn is_unnamed_producer(cause: &str) -> bool {
    cause == result_too_large(UNNAMED_PRODUCER)
}


/// The bytes `value` serialises to, refusing a document past what a state's output
/// may carry or deeper than [`MAX_PAYLOAD_DEPTH`]. `produced_by` names what produced
/// it, for the cause the refusal carries ([`result_too_large`]).
///
/// The count is the length of the document's COMPACT JSON text, byte for byte — the
/// separators between a container's members, the quotes and escapes of a string, the
/// digits of a number — because the limit is a boundary a caller may sit exactly on: a
/// document of exactly [`MAX_PAYLOAD_BYTES`] bytes is inside the limit and only one
/// past it is refused.
///
/// Walked ITERATIVELY, over an explicit stack, so measuring a document is not itself
/// bounded by the thread's stack; and it stops at the first byte past the limit, so
/// the measurement of an over-large document costs no more than the limit.
fn measure_payload(value: &Value, produced_by: &str) -> Result<usize, Failure> {
    let too_large = || named(DATA_LIMIT_ERROR, result_too_large(produced_by));
    let mut bytes = 0usize;
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_PAYLOAD_DEPTH {
            return Err(named(
                DATA_LIMIT_ERROR,
                format!(
                    "a document nested more than the {MAX_PAYLOAD_DEPTH} levels this service \
                     carries was produced"
                ),
            ));
        }
        bytes = bytes.saturating_add(match node {
            Value::Null => 4,
            Value::Bool(flag) => {
                // "true" is four bytes and "false" five.
                if *flag {
                    4
                } else {
                    5
                }
            }
            Value::Number(number) => number.to_string().len(),
            Value::String(text) => quoted_bytes(text),
            Value::Array(items) => {
                for item in items {
                    pending.push((item, depth + 1));
                }
                // The brackets, and one comma between each pair of items.
                items.len().saturating_sub(1) + 2
            }
            Value::Object(members) => {
                let mut keys = 0usize;
                for (key, member) in members {
                    // The member's name, its quotes and the colon after it.
                    keys = keys.saturating_add(quoted_bytes(key) + 1);
                    pending.push((member, depth + 1));
                }
                // The braces, and one comma between each pair of members.
                keys.saturating_add(members.len().saturating_sub(1) + 2)
            }
        });
        if bytes > MAX_PAYLOAD_BYTES {
            return Err(too_large());
        }
        // The pending list is bounded by the bytes already counted, so it cannot
        // outgrow the limit either.
        if pending.len() > MAX_PAYLOAD_BYTES {
            return Err(too_large());
        }
    }
    // Measuring a document is itself work the definition asked for, and the one
    // document every state produces goes through here: the bytes it carries are what
    // that walk is charged for ([`work_meter`]).
    if !spend_work(u64::try_from(bytes).unwrap_or(u64::MAX)) {
        return Err(out_of_work());
    }
    Ok(bytes)
}

/// The bytes `text` takes as a JSON string: its quotes, and the escape `serde_json`
/// writes for every byte that needs one.
///
/// The count is part of measuring a document EXACTLY ([`measure_payload`]): a string
/// carrying a quote, a backslash or a control byte is longer in the document than it is
/// in memory, and a document a caller sized to the limit must not be refused for it.
fn quoted_bytes(text: &str) -> usize {
    2 + text
        .bytes()
        .map(|byte| match byte {
            // The two-character escapes: `\"`, `\\`, `\n`, `\r`, `\t`, `\b`, `\f`.
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 2,
            // Every other control byte is written as `\u00xx`.
            0x00..=0x1f => 6,
            _ => 1,
        })
        .sum::<usize>()
}

/// One walk of one definition.
struct Interpreter<'tasks> {
    /// The state transitions still left in [`MAX_STATE_TRANSITIONS`]. Every nested
    /// `Map` iteration and `Parallel` branch spends from the same budget, so the
    /// total work one execution does is bounded however deeply it nests.
    budget: usize,
    /// How deeply the walk is nested inside `Map` iterations and `Parallel` branches,
    /// against [`MAX_NESTING_DEPTH`]: each level is an interpreter frame, so the
    /// nesting a definition can ask for is capped rather than left to the stack.
    depth: usize,
    /// What is LEFT of [`MAX_CALL_WAIT_DURATION`]: the span this walk may still spend
    /// waiting for the calls its `Task` states' work is made of. Spent as each call
    /// answers, and what bounds the thread one run can hold
    /// ([`Interpreter::call_within`]).
    call_wait_left: Duration,
    /// What is left of the definition-level `TimeoutSeconds` for THIS walk — the
    /// timeout less the span the execution had already spent — for a definition that
    /// declares one.
    ///
    /// "TimeoutSeconds … the maximum number of seconds an execution of the state
    /// machine can run": an execution still going at that span ENDS there, so the walk
    /// stops at it instead of running on to the emulator's own ceilings, and the run is
    /// reported as the timeout it is ([`Interpreter::past_timeout`]).
    timeout_left: Option<Duration>,
    /// Whether the walk STOPPED because the span above ran out, which is what makes the
    /// run's outcome the definition's own timeout whatever the walk returned.
    timed_out: bool,
    /// Which `Retry` attempt the state being run is on, which it reads through
    /// `$$.State.RetryCount`. A nested `Map` iteration or `Parallel` branch runs its
    /// own states, each with its own count, so [`Interpreter::nested_walk`] starts
    /// them at zero.
    retry_count: u64,
    steps: Vec<Step>,
    /// The id the FIRST history event this walk records carries: the events of one
    /// execution are numbered in order, and a walk that continues an execution
    /// continues that numbering rather than restarting it (the caller reads the last
    /// id the history holds off the record and hands it on). It is what a cause naming
    /// the event a state was entered at reports ([`Interpreter::next_event_id`]).
    first_event_id: i64,
    /// When the execution started, which a `Wait` state's `Timestamp` is measured
    /// against.
    start: SystemTime,
    /// How long after an outcome this walk computes the clock makes it DUE, as the call
    /// that started the walk decides it ([`run`]).
    ///
    /// The walk sleeps for none of it: what it needs the span for is the instant a
    /// child workflow execution of a Distributed Map stops at, because the results that
    /// run writes report the dates its children ran between and the store records those
    /// children as stopping when the clock makes their outcome due — so what is written
    /// to S3 and what `DescribeExecution` answers for the same child cannot disagree.
    settle: Duration,
    base_context: Value,
    /// What a `Task` state's service integration is performed by, for the one walk
    /// this interpreter is: a nested `Map` iteration or `Parallel` branch runs in
    /// this same frame, so its tasks are made through the same seam.
    tasks: &'tasks dyn Tasks,
    /// The activity task a `Task` state scheduled, which SUSPENDED the walk: the
    /// execution waits at that state until a worker answers it. Set at most once,
    /// because the walk stops at the state that set it.
    waiting: Option<ScheduledActivity>,
    /// The job a `Task` state's run-a-job integration started, which PARKED the walk:
    /// the execution waits at that state until the job finishes. Set at most once,
    /// because the walk stops at the state that set it.
    parked: Option<ParkedJob>,
    /// The jobs this walk started and then ABANDONED, because the concurrent state
    /// around the task that started one failed before the job could finish: what
    /// [`Run::abandoned`] reports, for the caller to stop.
    abandoned: Vec<ParkedJob>,
    /// The instant a branch of the `Parallel` state the walk is currently inside
    /// FAILED, for the branches of it that are still to be walked.
    ///
    /// "If any branch fails … the entire Parallel state is considered to have failed",
    /// and the branches that were still running are stopped there: a state of a sibling
    /// branch that would begin at or after that instant is not entered at all, so
    /// whatever that state would have done is not done either. Set by
    /// [`Interpreter::run_branches`] for the branches it has not walked yet and
    /// restored to the enclosing state's own value after them, so it bounds exactly the
    /// branches of the state that failed and the work nested inside them.
    cancel_after: Option<Duration>,
    /// The concurrent states the walk has to re-enter to reach the task a job's
    /// outcome answers, outermost first ([`ParkedJob::frames`]): taken level by level
    /// as the walk descends into them, so each one is applied by the state it names.
    resume_frames: Vec<ParkedFrame>,
    /// How many `Map` ITERATIONS the walk is currently inside, which is what refuses
    /// a run-a-job task there: a `Map` counts its items as they finish and starts a
    /// child execution per item, and an iteration parked at a job has finished
    /// nothing, so the task is not run rather than run as though its job had ended.
    map_depth: usize,
    /// The Map Run every Distributed Map state this walk entered started, each with
    /// the position of its state among the DISTRIBUTED Map states this walk ENTERED
    /// ([`Interpreter::map_runs_entered`]).
    ///
    /// A run is recorded when its items are done, so the list is built in FINISH
    /// order and read in entry order ([`plan_run`]) — a nested Distributed Map inside
    /// another's items finishes first, and two of them may be entered at the same
    /// offset, so the position is carried rather than derived from the instants.
    /// Bounded by [`MAX_STATE_TRANSITIONS`], which every `Map` state spends from.
    map_runs: Vec<(usize, StartedMapRun)>,
    /// How many DISTRIBUTED `Map` states this walk has ENTERED, which is what numbers
    /// the Map Runs above in entry order. An inline `Map` starts no Map Run, so it
    /// takes no number.
    map_runs_entered: usize,
    /// How many times this walk has entered a Distributed Map state of each LABEL,
    /// which is what the Map Run an attempt being CONTINUED already started is matched
    /// by ([`super::integration::Tasks::map_run`]). Bounded by the state count of the
    /// definition, every entry of which spends from [`MAX_STATE_TRANSITIONS`].
    map_runs_entered_by_label: BTreeMap<String, usize>,
    /// The reply a worker made to the activity task this walk is CONTINUING from,
    /// when it is one: the state the reply names produces that result instead of
    /// suspending. Taken when it is applied, so one reply answers one task.
    reply: Option<ActivityReply>,
    /// What the job this walk is CONTINUING from ended as, when it is one: the state
    /// that started that job reports that outcome instead of starting it again.
    /// Taken when it is applied, so one outcome answers one parked job.
    job_reply: Option<JobReply>,
    /// What a `SendTask*` reported for the callback task this walk is CONTINUING
    /// from, when it is one: the state whose call carried the token reports that
    /// outcome instead of making the call again. Taken when it is applied, so one
    /// answer answers one token.
    callback_reply: Option<CallbackReply>,
    /// How many attempts each retrier of the state ABOUT TO BE ENTERED has already
    /// spent, for a walk continuing from an answer to that state's token: the
    /// retrier budget a `Retry` gives a state belongs to the state, so a resumed walk
    /// takes it up where the suspension left it instead of restarting it.
    ///
    /// Set by [`Interpreter::task_state`] immediately before it enters the retry loop
    /// and TAKEN there, so it seeds exactly the one state the answer resumes and no
    /// state the walk reaches afterwards.
    resumed_attempts: Option<Vec<u64>>,
    /// How many attempts each retrier of the state being run has spent so far, which
    /// the retry loop publishes as it goes: a state that SUSPENDS records it on the
    /// task, so the walk that continues from the answer can take the budget up again.
    attempts_spent: Vec<u64>,
    /// Where the reply was applied, once it has been: what the walk did BEFORE that
    /// point already happened under an earlier call, so [`interpret`] reports only
    /// what follows it.
    resumed: Option<Resumed>,
    /// The name of the state machine this walk runs, and the region its ARNs are
    /// minted in: what the ARN of a child execution a Distributed Map state starts is
    /// built from, together with that state's label.
    machine_name: String,
    region: String,
    /// The ARN of the execution this walk runs, which the `taskToken` a callback
    /// task's call carries names ([`Interpreter::fresh_task_token`]).
    execution_arn: String,
    /// How many child executions this walk has already named, mixed into the next
    /// one's drawn name so two mints in the same nanosecond differ — exactly as the
    /// store's own counter does for an execution name it mints.
    child_counter: u64,
    /// Whether this walk runs an EXPRESS workflow, which is what refuses a `Task`
    /// state asking for the run-a-job pattern ([`Context::express`]).
    express: bool,
    /// The TOP-LEVEL state this walk failed at, with the raw input it was entered
    /// with: what [`Run::stopped_at`] reports, so a redrive continues from the
    /// unsuccessful step. A failure inside a `Map` iteration or a `Parallel` branch
    /// is recorded as the state that encloses it, because that is the step of the
    /// definition the execution stopped at.
    failed_at: Option<RedriveStep>,
    /// The query language the DEFINITION declares, which every state is evaluated in
    /// unless it declares one of its own ([`Interpreter::language_of`]).
    language: &'static str,
    /// The variables the run has assigned, which a JSONata expression reads under
    /// each one's own name (`$total`).
    ///
    /// A nested state machine — a `Map` iteration, a `Parallel` branch — sees what
    /// reaching its enclosing state assigned and its own assignments do not outlive
    /// it ([`Interpreter::nested_walk`]), which is how the language scopes them.
    variables: BTreeMap<String, Value>,
    /// What the state the walk is IN has assigned, as the event it is left by reports
    /// them: "map of variable name and value as a serialized JSON representation".
    ///
    /// Only the state's OWN assignments, not the scope around them — which is what
    /// the member reports — so it is written by that state's `Assign` and taken by the
    /// event the state is left by ([`Interpreter::push_exited`]).
    assigned: BTreeMap<String, String>,
    /// The documents this walk was GIVEN as text, with the bytes each is reported in:
    /// what [`Interpreter::echo`] answers instead of re-encoding a parsed copy.
    supplied: Supplied,
}

/// Where a walk applied an [`ActivityReply`]: the step its own events begin at, and
/// the offset into the walk at which the replying state was entered.
#[derive(Debug, Clone, Copy)]
struct Resumed {
    index: usize,
    at: Duration,
}

/// How the items of ONE `Map` attempt turned out, counted as they are run.
///
/// A `Map` attempt can end at any item (a step budget, a work allowance, a document
/// past what a state may carry), so the counts are kept here rather than derived from
/// the attempt's result: however the attempt ended, what its items did is what this
/// holds.
#[derive(Debug, Default)]
struct MapTally {
    /// Each item that finished, with the offset it finished at, so what the Map Run
    /// reports can follow the items one by one rather than all at once.
    items: Vec<MapItemOutcome>,
    /// The child execution each item of a DISTRIBUTED attempt ran as, in the order
    /// the iterations were started; empty for an inline `Map`, which starts none.
    children: Vec<StartedChildExecution>,
}

impl Interpreter<'_> {
    // -----------------------------------------------------------------------
    // Parallel
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn parallel_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        self.push_entered("ParallelStateEntered", name, at, &raw);
        let branches = state
            .get("Branches")
            .and_then(Value::as_array)
            .filter(|branches| !branches.is_empty())
            .ok_or_else(|| {
                runtime(format!(
                    "the 'Parallel' state '{name}' must carry a non-empty 'Branches' array"
                ))
                .at(at)
            })?;
        // Every branch is run on every visit to this state, so the branch count
        // multiplies that visit's work exactly as a `Choice`'s rule count does.
        if branches.len() > MAX_CHOICE_RULES {
            return Err(runtime(format!(
                "the 'Parallel' state '{name}' declares {} branches, more than the \
                 {MAX_CHOICE_RULES} this service runs",
                branches.len()
            ))
            .at(at));
        }
        let mut plan = ParallelPlan {
            branches: Vec::with_capacity(branches.len()),
        };
        for (index, branch) in branches.iter().enumerate() {
            let label = format!("branch {index} of the 'Parallel' state '{name}'");
            plan.branches
                .push(states_of(branch, &label).map_err(|failure| failure.at(at))?);
        }
        // The frame of the parked record this state's own name: a walk CONTINUING an
        // execution whose job has ended re-enters the `Parallel` the task was parked
        // inside, and this is where it picks up what that record kept. It is taken
        // once, by the first attempt: a later RETRY of this state starts its branches
        // again, exactly as the service re-runs every branch of a retried `Parallel`.
        let mut resume = match self.resume_frames.first() {
            Some(frame) if frame.state_name == name => Some(self.resume_frames.remove(0)),
            _ => None,
        };
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // "Parameters ... The effective input each branch receives" —
            // `Arguments` in JSONata. The branches are handed that document, so it is
            // built BEFORE they start: a state whose own members could not be
            // evaluated starts no branch, and records no event for having started
            // any. It is re-evaluated per attempt, because this attempt's own members
            // read `$$.State.RetryCount`.
            let context = with_retry_count(context, me.retry_count);
            let branch_input = match me.task_request(state, language, &effective, &context) {
                Ok(value) => value,
                Err(failure) => {
                    me.push(super::EVENT_PARALLEL_STATE_FAILED, name, at);
                    return (Err(failure), at);
                }
            };
            // Every attempt at this state starts its branches, so every attempt
            // writes its own `ParallelStateStarted` and the event of the outcome
            // those branches reached.
            me.push(super::EVENT_PARALLEL_STATE_STARTED, name, at);
            let (outcome, end) =
                me.run_branches(&plan, name, &raw, &branch_input, at, resume.take());
            // A branch that PARKED leaves the state with no outcome at all, so the
            // state records neither of the events an outcome is reported by: what
            // happens next is the job finishing.
            if me.parked.is_some() {
                return (outcome, end);
            }
            let event = if outcome.is_ok() {
                super::EVENT_PARALLEL_STATE_SUCCEEDED
            } else {
                super::EVENT_PARALLEL_STATE_FAILED
            };
            me.push(event, name, end);
            (outcome, end)
        });
        match outcome {
            // One of the branches is parked at the job its own run-a-job task
            // started: the walk stops here and the execution waits for that job,
            // exactly as it does for a task at the top level of the definition.
            Ok(value) if self.parked.is_some() => Ok(StateStep {
                value,
                at: end,
                next: Next::Suspended,
            }),
            Ok(results) => {
                let value = self
                    .produced(state, language, &raw, &effective, Some(results), context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("ParallelStateExited", name, end, &value);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self.next_of(state).map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "ParallelStateExited"),
                state,
                language,
                &raw,
                &effective,
                failure,
                context,
                end,
            ),
        }
    }

    /// Run every branch of a `Parallel` state with the same input, and collect their
    /// outputs in branch order. The branches run concurrently on the real service, so
    /// the state's span is the longest branch's.
    ///
    /// The array the branches are building is a document this state will put out, so
    /// it is bounded as it GROWS rather than only once it is whole: a definition that
    /// declares many branches cannot hold more than a state's output may carry while
    /// it runs.
    ///
    /// "If any branch fails ... the entire Parallel state is considered to have
    /// failed", and it failed at the instant that branch did: the branches that were
    /// still running are STOPPED there, so no state of a surviving branch is ENTERED
    /// from that instant on ([`Interpreter::stopped_by_a_sibling`]) — a call such a
    /// state would have made is not made, and the events of what a stopped branch had
    /// already run are cut back to that instant ([`Interpreter::abort_units`]). A
    /// branch that had PARKED at a job when that happened is stopped too, which is
    /// what [`Interpreter::abandoned`] carries.
    ///
    /// The branches run CONCURRENTLY, so where the failing one sits in the `Branches`
    /// list does not decide the outcome; this walk runs them one after another, in the
    /// order the definition lists them.
    ///
    /// One branch may SUSPEND, at the job a run-a-job task of it started: the state
    /// has no result then, so the walk reports the suspension by way of
    /// [`Interpreter::parked`] and records the frame that lets the continuation
    /// re-enter this state ([`ParkedFrame`]). `resume` is that frame, for the walk
    /// that is continuing: the branches it names as finished are not walked again.
    #[allow(clippy::too_many_arguments)]
    fn run_branches(
        &mut self,
        plan: &ParallelPlan<'_>,
        state_name: &str,
        state_input: &Value,
        input: &Value,
        at: Duration,
        resume: Option<ParkedFrame>,
    ) -> Attempt {
        let count = plan.branches.len();
        // What each branch produced, in branch order: a continuation starts from the
        // results the parked record carries, so a branch that had already finished is
        // not walked a second time and nothing it did happens twice.
        let mut done: Vec<Option<Value>> = match &resume {
            Some(frame) if frame.done.len() == count => frame.done.clone(),
            _ => vec![None; count],
        };
        // The array the branches are building is bounded as it GROWS, so a
        // continuation starts the count at what the branches it is NOT walking again
        // already produced: the ceiling then covers the whole of this state's result
        // and not only the branch that resumed.
        let mut results_bytes = 0usize;
        for value in done.iter().flatten() {
            match measure_payload(value, UNNAMED_PRODUCER) {
                Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                Err(failure) => return (Err(failure), at),
            }
        }
        let mut end = at;
        // Where each branch's own events begin in this walk's list, so a branch the
        // state's failure aborted can be cut back to the instant it failed.
        let mut units: Vec<Unit> = Vec::with_capacity(count);
        let mut failed: Option<(Failure, Duration)> = None;
        let mut suspended: Option<usize> = None;
        // What stops the branches still to be walked, for the branches of a state
        // ENCLOSING this one that has already failed: this state's own branches are
        // stopped by its own failure below, and the enclosing bound is put back once
        // they have run.
        let enclosing_cancel = self.cancel_after;
        for (index, (states, start_at)) in plan.branches.iter().enumerate() {
            if done.get(index).is_some_and(Option::is_some) {
                continue;
            }
            // The branch that PARKED carries on from the state that parked it, with
            // the data that state was entered with: the states before it already ran,
            // and their events are already in the execution's history.
            let resumed = resume
                .as_ref()
                .filter(|frame| frame.branch == index)
                .and_then(|_| self.resume_entry());
            let (start_state, branch_input) = match resumed {
                Some((state, state_input)) => (state, state_input),
                None => ((*start_at).to_string(), input.clone()),
            };
            let from = self.steps.len();
            let map_runs_from = self.map_runs.len();
            let branch = self.nested_walk(
                states,
                &start_state,
                branch_input,
                at,
                &Scope::default(),
                false,
            );
            units.push(Unit {
                from,
                map_runs_from,
                aborted: None,
            });
            if branch.suspended {
                // The branch is parked at a job: it has produced nothing yet, and the
                // state cannot have a result until it does.
                end = end.max(branch.end);
                suspended = Some(index);
                continue;
            }
            match branch.outcome {
                Ok(value) => {
                    match measure_payload(&value, UNNAMED_PRODUCER) {
                        Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                        Err(failure) => {
                            // Leaving by any path puts back the bound the state was
                            // entered under: what stops a branch belongs to the state
                            // whose branch failed, not to the walk that goes on after
                            // it.
                            self.cancel_after = enclosing_cancel;
                            return (Err(failure), end.max(branch.end));
                        }
                    }
                    if results_bytes > MAX_PAYLOAD_BYTES {
                        self.cancel_after = enclosing_cancel;
                        return (
                            Err(named(DATA_LIMIT_ERROR, result_too_large(UNNAMED_PRODUCER))),
                            end.max(branch.end),
                        );
                    }
                    end = end.max(branch.end);
                    if let Some(slot) = done.get_mut(index) {
                        *slot = Some(value);
                    }
                }
                // The state failed with the error of the branch that failed FIRST,
                // which is the one whose failure stopped the others.
                Err(failure) => {
                    if failed.as_ref().is_none_or(|(_, when)| branch.end < *when) {
                        failed = Some((failure, branch.end));
                    }
                    // "If any branch fails … all the branches are stopped": from this
                    // instant on the state has failed, so the branches still to be
                    // walked do not enter a state that begins at or after it — and no
                    // call such a state would have made is made
                    // ([`Interpreter::stopped_by_a_sibling`]).
                    let stop = failed.as_ref().map_or(branch.end, |(_, when)| *when);
                    self.cancel_after = Some(match enclosing_cancel {
                        Some(enclosing) => enclosing.min(stop),
                        None => stop,
                    });
                }
            }
        }
        self.cancel_after = enclosing_cancel;
        if let Some((failure, when)) = failed {
            // A branch of this state failed, so the state has failed — and a sibling
            // parked at a job is not waited for any longer: the job it started is
            // ABANDONED, which is what stops it.
            if let Some(parked) = self.parked.take() {
                self.abandoned.push(parked);
            }
            self.abort_units(when, &units);
            return (Err(failure), when);
        }
        if let Some(branch) = suspended {
            // The frame this state contributes to the parked record: which branch is
            // parked, and what the others produced, so the continuation re-enters
            // this state without re-running any of them.
            if let Some(parked) = self.parked.as_mut() {
                parked.frames.insert(
                    0,
                    ParkedFrame {
                        state_name: state_name.to_string(),
                        state_input: state_input.clone(),
                        branch,
                        done,
                    },
                );
            }
            // The state has no result: `parallel_state` reads the parked record and
            // suspends there, so this value is never placed.
            return (Ok(Value::Null), end);
        }
        let results: Vec<Value> = done.into_iter().map(Option::unwrap_or_default).collect();
        (Ok(Value::Array(results)), end)
    }
}

/// The members every event of a `Task` state's own work reports: "the service name of
/// the resource in a task state" and "the action of the resource called by a task
/// state", as [`super::integration::named_work`] read them off the `Resource`.
fn task_work_details(resource_type: &str, action: &str) -> super::EventDetails {
    super::EventDetails {
        resource_type: Some(resource_type.to_string()),
        resource: Some(action.to_string()),
        ..super::EventDetails::default()
    }
}

/// The member one `Map` iteration's own events report beside the state's name: "the
/// index of the array belonging to the Map state iteration".
fn iteration_details(index: usize) -> super::EventDetails {
    super::EventDetails {
        index: Some(i32::try_from(index).unwrap_or(i32::MAX)),
        ..super::EventDetails::default()
    }
}

/// The `states` map and `StartAt` of a nested state machine (an `ItemProcessor`, an
/// `Iterator` or a `Parallel` branch).
fn states_of<'a>(node: &'a Value, label: &str) -> Result<(&'a Map<String, Value>, &'a str), Failure> {
    let object = node
        .as_object()
        .ok_or_else(|| runtime(format!("{label} must be a JSON object")))?;
    let states = object
        .get("States")
        .and_then(Value::as_object)
        .ok_or_else(|| runtime(format!("{label} must carry an object 'States' member")))?;
    let start_at = object
        .get("StartAt")
        .and_then(Value::as_str)
        .ok_or_else(|| runtime(format!("{label} must carry a string 'StartAt' member")))?;
    Ok((states, start_at))
}

/// A `Map` state's `ItemProcessor` (or the legacy `Iterator`), and whether it runs in
/// Distributed mode.
fn map_processor<'a>(
    name: &str,
    state: &'a Map<String, Value>,
) -> Result<(&'a Value, bool), Failure> {
    let processor = state
        .get("ItemProcessor")
        .or_else(|| state.get("Iterator"))
        .ok_or_else(|| {
            runtime(format!(
                "the 'Map' state '{name}' carries neither 'ItemProcessor' nor 'Iterator'"
            ))
        })?;
    let distributed = processor
        .get("ProcessorConfig")
        .and_then(|config| config.get("Mode"))
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("DISTRIBUTED"));
    Ok((processor, distributed))
}

/// A retrier's numeric member, when it is a finite, non-negative number.
///
/// A member that is absent, is not a number, or is negative or non-finite leaves the
/// retrier on its documented default rather than reaching the arithmetic below.
fn finite_seconds(retrier: &Map<String, Value>, member: &str) -> Option<f64> {
    retrier
        .get(member)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0)
}

/// How long a retrier waits before the attempt after `taken` ones:
/// "IntervalSeconds" scaled by "BackoffRate" once per attempt already taken, capped
/// by `MaxDelaySeconds` where the retrier declares one.
///
/// The product is of two numbers the definition chose, so it can overflow to infinity
/// (or to NaN, when a zero interval meets an infinite backoff). A wait no longer than
/// the longest one this service waits for is what such a retrier gets: the figure
/// this returns is always finite and in range, which is what makes converting it to a
/// `Duration` total. An attempt count past `i32::MAX` — which `MaxAttempts` and the
/// step budget both make unreachable — scales to that same maximum rather than to the
/// bare interval.
fn retry_delay_seconds(retrier: &Map<String, Value>, taken: u64) -> f64 {
    let interval =
        finite_seconds(retrier, "IntervalSeconds").unwrap_or(DEFAULT_RETRY_INTERVAL_SECONDS);
    let backoff = finite_seconds(retrier, "BackoffRate").unwrap_or(DEFAULT_RETRY_BACKOFF_RATE);
    let mut delay = interval * backoff.powi(i32::try_from(taken).unwrap_or(i32::MAX));
    if let Some(cap) = finite_seconds(retrier, "MaxDelaySeconds") {
        delay = delay.min(cap);
    }
    if delay.is_finite() {
        delay.clamp(0.0, MAX_WAIT_SECONDS as f64)
    } else {
        MAX_WAIT_SECONDS as f64
    }
}

/// Which retrier of the `Task` state `state_name` of `definition` retries an activity
/// task that failed with `error`, and how long after that failure its next attempt is
/// due.
///
/// `taken` is how many attempts each retrier of that state has already spent on the
/// task, in the order the state declares them. The answer is the FIRST retrier whose
/// `ErrorEquals` names the error, exactly as a retried walk's is, so one rule decides
/// every retry this service makes.
///
/// `None` when the definition declares no such state, when the state declares no
/// `Retry`, when no retrier names the error, or when the one that does has spent its
/// `MaxAttempts`: the failure is then the state's own.
pub fn activity_retry(
    definition: &str,
    state_name: &str,
    error: &str,
    taken: &[u64],
) -> Option<(usize, Duration)> {
    let document = serde_json::from_str::<Value>(definition).ok()?;
    let state = document
        .get("States")
        .and_then(Value::as_object)?
        .get(state_name)
        .and_then(Value::as_object)?;
    let retriers = state.get("Retry").and_then(Value::as_array)?;
    for (index, retrier) in retriers.iter().enumerate() {
        let Some(retrier) = retrier.as_object() else {
            continue;
        };
        if !errors_match(retrier, error) {
            continue;
        }
        let max_attempts = retrier
            .get("MaxAttempts")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
        let spent = taken.get(index).copied().unwrap_or(0);
        if spent >= max_attempts {
            return None;
        }
        return Some((
            index,
            Duration::from_secs_f64(retry_delay_seconds(retrier, spent)),
        ));
    }
    None
}

/// Whether a retrier's or catcher's `ErrorEquals` names `error`.
///
/// "States.ALL — A wildcard that matches any known error name", with one documented
/// exception: "A States.Runtime error isn't retriable, and will always cause the
/// execution to fail. A retry or catch on States.ALL won't catch States.Runtime
/// errors." A definition that names `States.Runtime` itself does not catch it
/// either, because it is not an error the execution can recover from.
///
/// [`TASK_FAILED_ERROR`] is the OTHER wildcard the language documents: "States.TaskFailed
/// […] When used in a retry or catch, States.TaskFailed acts as a wildcard that matches
/// any known error name except States.Timeout" — so a retrier naming it takes over the
/// failure a task's own work reported, whatever that work named it, which is how a
/// definition retries a service call or a function that failed without naming the
/// service's error itself.
fn errors_match(entry: &Map<String, Value>, error: &str) -> bool {
    if error == RUNTIME_ERROR {
        return false;
    }
    entry
        .get("ErrorEquals")
        .and_then(Value::as_array)
        .is_some_and(|names| {
            names.iter().filter_map(Value::as_str).any(|name| {
                name == error
                    || name == ALL_ERRORS
                    || (name == TASK_FAILED_ERROR && error != TIMEOUT_ERROR)
            })
        })
}

/// `context` with `$$.State.RetryCount` set to the attempt the state is on.
fn with_retry_count(context: &Value, attempt: u64) -> Value {
    let mut context = context.clone();
    if let Some(state) = context.get_mut("State").and_then(Value::as_object_mut) {
        state.insert("RetryCount".to_string(), json!(attempt));
    }
    context
}

/// `context` with `$$.Task.Token` set to the `taskToken` a callback task's
/// integration call carries.
///
/// "Task tokens are generated by Step Functions [...] in the context object when a
/// workflow enters a task state": the token is how a `.waitForTaskToken` state's own
/// parameters hand the token to the work they call, so it is a member of the context
/// object that state's templates are rendered with and of no other state's.
fn with_task_token(context: &Value, token: &str) -> Value {
    let mut context = context.clone();
    if let Some(object) = context.as_object_mut() {
        object.insert("Task".to_string(), json!({ "Token": token }));
    }
    context
}

/// `context` with `$$.Map.Item` set to the item being iterated.
fn with_map_item(context: &Value, map_item: &Value) -> Value {
    let mut context = context.clone();
    if let Some(object) = context.as_object_mut() {
        object.insert("Map".to_string(), json!({ "Item": map_item }));
    }
    context
}

/// `ResultSelector` applied to a state's raw result, when the state declares one and
/// its type has one (`with_selector`).
fn select_result(
    state: &Map<String, Value>,
    result: Value,
    context: &Value,
    with_selector: bool,
) -> Result<Value, Failure> {
    match state.get("ResultSelector").filter(|_| with_selector) {
        Some(template) => payload_template(template, &result, context),
        None => Ok(result),
    }
}

/// `ResultPath` applied to a state's result: where in the state's own RAW input the
/// result is placed.
fn path_result(
    state: &Map<String, Value>,
    raw: &Value,
    result: Value,
) -> Result<Value, Failure> {
    match state.get("ResultPath") {
        // "ResultPath (Optional) ... The default value is $, which matches the
        // entire input."
        None => Ok(result),
        // "If the ResultPath is null, that means that the state's own raw input is
        // the state's output."
        Some(Value::Null) => Ok(raw.clone()),
        Some(Value::String(path)) => set_at_reference_path(raw, path, result),
        Some(other) => Err(runtime(format!(
            "'ResultPath' must be a reference-path string or null, not {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Payload templates, paths and intrinsic functions
// ---------------------------------------------------------------------------

/// Evaluate a payload template (`Parameters`, `ItemSelector`, `ResultSelector`).
///
/// "If any field within the Payload Template ... has a name ending with the
/// characters '.$', its value is transformed according to rules below and the field
/// is renamed to strip the '.$' suffix": the value is either a path or an intrinsic
/// function invocation.
///
/// A template is a CALLER's, and so is the document each of its members selects
/// from, so the PRODUCT of the two — how many members it declares, how many of them
/// are expressions to evaluate, and how large each member's value is — is what has to
/// be bounded. Every member is measured as it is rendered and spent from one budget
/// of [`MAX_PAYLOAD_BYTES`], and at most [`MAX_TEMPLATE_PATHS`] of them may be
/// expressions, so the template can never build more than a state's output may carry
/// nor evaluate more expressions than one visit to a state is allowed.
fn payload_template(node: &Value, input: &Value, context: &Value) -> Result<Value, Failure> {
    let mut budget = TemplateBudget {
        bytes: MAX_PAYLOAD_BYTES,
        paths: MAX_TEMPLATE_PATHS,
    };
    render_template(node, input, context, 1, &mut budget)
}

/// What one payload template may still build, and still evaluate.
struct TemplateBudget {
    bytes: usize,
    paths: usize,
}

impl TemplateBudget {
    /// Spend `bytes` of what the template may still build.
    fn spend(&mut self, bytes: usize) -> Result<(), Failure> {
        match self.bytes.checked_sub(bytes) {
            Some(left) => {
                self.bytes = left;
                Ok(())
            }
            // What a template builds is the state's own document, so a template past
            // the ceiling is the same failure an over-large state output is
            // ([`result_too_large`]); the state is named by the one being executed.
            None => Err(named(DATA_LIMIT_ERROR, result_too_large(UNNAMED_PRODUCER))),
        }
    }

    /// Charge one expression evaluation.
    fn charge_path(&mut self) -> Result<(), Failure> {
        match self.paths.checked_sub(1) {
            Some(left) => {
                self.paths = left;
                Ok(())
            }
            None => Err(named(
                PARAMETER_PATH_ERROR,
                format!(
                    "the payload template carries more than the {MAX_TEMPLATE_PATHS} paths this \
                     service evaluates"
                ),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Intrinsic functions
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// One state, tested by itself (`TestState`)
// ---------------------------------------------------------------------------

/// How a failure of the ONE state `TestState` tests names it: the state the request
/// carries alone has no name of its own, so what a failure it raised says about it
/// names it as the state under test rather than inventing a name for it.
pub const SINGLE_STATE_LABEL: &str = "the state under test";

/// `TestExecutionStatus`' `SUCCEEDED`: the state's work produced an output
/// (`smithy_query(query="shape",shape="TestExecutionStatus")` -> `CAUGHT_ERROR |
/// FAILED | RETRIABLE | SUCCEEDED`).
pub const TEST_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `TestExecutionStatus`' `FAILED`: the state failed, and neither a `Retry` nor a
/// `Catch` of its own answers for the error it failed with.
pub const TEST_STATUS_FAILED: &str = "FAILED";

/// `TestExecutionStatus`' `RETRIABLE`: the state failed with an error one of its
/// `Retry` retriers matches and still has attempts left for, so the state machine
/// would re-run it.
pub const TEST_STATUS_RETRIABLE: &str = "RETRIABLE";

/// `TestExecutionStatus`' `CAUGHT_ERROR`: the state failed and one of its `Catch`
/// catchers took the failure over, so the execution continues at that catcher's
/// `Next` with the error output.
pub const TEST_STATUS_CAUGHT_ERROR: &str = "CAUGHT_ERROR";

/// The state types whose `Parameters` member is a payload template over the state's
/// effective INPUT (see [`executable_members`]); the other types declare no such
/// member, so no `afterParameters` stage exists for them.
const TEMPLATED_STATE_TYPES: &[&str] = &["Map", "Parallel", "Pass", "Task"];

/// The state types whose `ResultSelector` member is a payload template over the
/// state's raw RESULT. `Pass` is absent: the Amazon States Language gives it a
/// `Result`, not a `ResultSelector`.
const SELECTOR_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// The state types whose own work is an INVOCATION — a service integration, an
/// iteration over items, a set of branches — rather than a function of the state's
/// data alone.
///
/// "TestState only supports the following when a mock is specified: Activity tasks,
/// .sync or .waitForTaskToken service integration patterns, Parallel, or Map states":
/// a state of one of these types is tested through its mocked outcome, and the
/// operation refuses to test a `Map` or a `Parallel` without one.
pub const INVOKING_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// A mocked outcome for the state under test: "A JSON object that contains a mocked
/// `result` or `errorOutput`", which stands in for the invocation the state would
/// otherwise make. Nothing is invoked for a mocked state; what the mock names IS the
/// state's result or failure, and the state's own data flow is applied to it.
#[derive(Debug, Clone)]
pub enum Mock {
    /// `mock.result`: "A JSON string containing the mocked result of the state
    /// invocation."
    Result(Value),
    /// `mock.errorOutput`: "the mocked response is returned as a JSON object that
    /// contains an `error` and `cause` field."
    Error(Failure),
}

/// What `TestState` reports about one state's data processing: the document the state
/// carried after each filter of the Amazon States Language's data flow.
///
/// A stage a state has no member for is left unreported rather than repeated, and the
/// result-side stages exist only for a state that produces a result of its own.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    /// "The raw state input", as the caller wrote it.
    pub input: Option<String>,
    /// "The input after Step Functions applies the InputPath filter."
    pub after_input_path: Option<String>,
    /// "The effective input after Step Functions applies the Parameters filter."
    pub after_parameters: Option<String>,
    /// "The input after Step Functions applies an Arguments filter. This event will
    /// only be present when QueryLanguage for the state machine or individual states
    /// is set to JSONata" — the JSONata-mode member the other language's
    /// `afterParameters` corresponds to.
    pub after_arguments: Option<String>,
    /// "JSON string that contains the set of workflow variables after execution of
    /// the state", for a state that assigned any: the `Assign` of a JSONata state is
    /// what writes them.
    pub variables: Option<String>,
    /// "The state's raw result."
    pub result: Option<String>,
    /// "The effective result after Step Functions applies the ResultSelector filter."
    pub after_result_selector: Option<String>,
    /// "The effective result combined with the raw state input after Step Functions
    /// applies the ResultPath filter."
    pub after_result_path: Option<String>,
}

/// What one state, tested by itself, produced.
#[derive(Debug, Clone)]
pub struct Trial {
    /// One of [`TEST_STATUS_SUCCEEDED`], [`TEST_STATUS_FAILED`],
    /// [`TEST_STATUS_RETRIABLE`] and [`TEST_STATUS_CAUGHT_ERROR`].
    pub status: &'static str,
    pub output: Option<String>,
    /// Where the state transitions to. "If you haven't defined a next state in your
    /// definition or if the execution of the state fails, this field doesn't contain
    /// a value."
    pub next_state: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub inspection: Inspection,
    /// The position in `Retry` of the retrier that matched a [`TEST_STATUS_RETRIABLE`]
    /// failure, and how long that retrier waits before the attempt it owes.
    pub retry_index: Option<i32>,
    pub retry_backoff_seconds: Option<i32>,
    /// The position in `Catch` of the catcher that took a
    /// [`TEST_STATUS_CAUGHT_ERROR`] failure over.
    pub catch_index: Option<i32>,
}

impl Trial {
    /// The trial of a state that produced `output`.
    fn succeeded(output: Option<String>, next_state: Option<String>, inspection: Inspection) -> Self {
        Self {
            status: TEST_STATUS_SUCCEEDED,
            output,
            next_state,
            error: None,
            cause: None,
            inspection,
            retry_index: None,
            retry_backoff_seconds: None,
            catch_index: None,
        }
    }

    /// The trial of a state that failed with nothing of its own to answer for it.
    ///
    /// `error` and `cause` leave through the modelled `SensitiveError` (0..256) and
    /// `SensitiveCause` (0..32768) shapes, so they are bounded exactly as an
    /// execution's are: a name past its maximum is TRIMMED where it is reported
    /// ([`trimmed_error`]), since what the state raised is what the caller has to see,
    /// while an over-long cause keeps the error name and is clipped to what the service
    /// reports.
    ///
    /// This is the reporting boundary of the single-state trial, so a quota sentence
    /// raised with no producer named ([`UNNAMED_PRODUCER`]) names the state under test
    /// here, exactly as an execution's own state names itself in it ([`diagnosed`]).
    fn failed(inspection: Inspection, failure: Failure) -> Self {
        let Failure { error, cause } = produced_by_state(SINGLE_STATE_LABEL, failure);
        Self {
            status: TEST_STATUS_FAILED,
            output: None,
            next_state: None,
            error: error.as_deref().map(trimmed_error),
            cause: cause.map(clip_cause),
            inspection,
            retry_index: None,
            retry_backoff_seconds: None,
            catch_index: None,
        }
    }
}

/// One `TestState` call: the state under test, and everything the request fixed
/// about it.
pub struct StateTest<'a> {
    /// The state's name, which `$$.State.Name` reports. Empty for a `definition`
    /// that is the state alone, which carries no name — only a `stateName` naming a
    /// state of a full definition gives it one.
    pub name: &'a str,
    /// The state's own definition: the object the Amazon States Language declares it
    /// as.
    pub state: &'a Map<String, Value>,
    /// "A string that contains the JSON input data for the state", as the caller
    /// wrote it.
    pub input: &'a str,
    /// When the state is being tested, which `$$.State.EnteredTime` reports.
    pub start: SystemTime,
    /// What the context object reports about this test, for the members the request
    /// fixes.
    pub context: &'a Context,
    /// "A JSON string representing a valid Context object for the state under test":
    /// the whole `$$` document, which replaces the one built from `context`.
    pub context_override: Option<&'a Value>,
    pub mock: Option<&'a Mock>,
    /// `stateConfiguration.retrierRetryCount`: "The number of retry attempts that
    /// have occurred for the state's Retry that applies to the mocked error", which
    /// is what decides whether the state's failure is still RETRIABLE.
    pub retries_taken: u64,
    /// "JSON object literal that sets variables used in the state under test. Object
    /// keys are the variable names and values are the variable values": the scope the
    /// state's own expressions read, before its `Assign` writes to it.
    pub variables: Option<&'a Value>,
}

/// Run the ONE state `test` names over its input and report what it produced.
///
/// This is `TestState`'s walk: a single state, with no state machine around it, so
/// nothing is transitioned to and nothing is persisted. The state's data flow, its
/// `Choice` rules, its `Wait` members and its `Fail` members are evaluated by the
/// same code an execution's walk uses; an invocation this process cannot make is
/// answered by the request's `mock`, and without one the state fails with ASL's own
/// [`RUNTIME_ERROR`] rather than with a fabricated result.
pub fn test_state(test: &StateTest<'_>) -> Trial {
    // The trial is a caller-sized walk too, so it runs under the same work allowance an
    // execution's does ([`work_meter`]).
    let _allowance = WorkAllowance::of(MAX_INTERPRET_WORK);
    let inspection = Inspection {
        input: Some(test.input.to_string()),
        ..Inspection::default()
    };
    let raw: Value = match serde_json::from_str(test.input) {
        Ok(value) => value,
        Err(why) => {
            return Trial::failed(
                inspection,
                runtime(format!(
                    "the state input is not valid JSON: {}",
                    crate::state::json_parse_diagnostic(test.input, &why)
                )),
            );
        }
    };
    // The input is a caller's document too, so it is measured against the ceiling
    // every document a state produces is measured against: an input larger than a
    // state's output may carry is answered with ASL's own data-limit error rather
    // than evaluated, which is also what bounds what one path or one `StringMatches`
    // rule can be asked to walk.
    if let Err(failure) = measure_payload(&raw, UNNAMED_PRODUCER) {
        // The document measured here is the state's INPUT, which the request supplied:
        // the quota sentence is about a result a state RETURNED, so an input past the
        // ceiling says what it is instead.
        return Trial::failed(
            inspection,
            Failure {
                error: failure.error,
                cause: Some(format!(
                    "the state input is larger than the {MAX_PAYLOAD_BYTES} bytes a state's \
                     document may carry"
                )),
            },
        );
    }
    let base_context = match test.context_override {
        Some(context) => context.clone(),
        None => test.context.value(&raw, test.start),
    };
    let mut interpreter = Interpreter {
        budget: MAX_STATE_TRANSITIONS,
        depth: 0,
        call_wait_left: MAX_CALL_WAIT_DURATION,
        // `TestState` tests ONE state, with no execution and so no definition-level
        // `TimeoutSeconds` around it: nothing here bounds the span the state runs in.
        timeout_left: None,
        timed_out: false,
        // The state is tested as the attempt the request says it is on, which is what
        // it reads through `$$.State.RetryCount`.
        retry_count: test.retries_taken,
        steps: Vec::new(),
        // `TestState` keeps no history: the state under test is the first thing this
        // walk would record, so a diagnosis it raises names the id the first event of
        // a history carries.
        first_event_id: FIRST_EVENT_ID,
        start: test.start,
        // `TestState` owes the clock nothing: it reports what one state computes in the
        // call that asked, and the child executions a span like this would date belong
        // to an execution.
        settle: Duration::ZERO,
        base_context,
        // `TestState` tests ONE state, with no execution around it: a state whose
        // work is an invocation is tested through its `mock`, so nothing is called
        // for this walk and there is nowhere for it to wait.
        tasks: &super::integration::Unserved,
        waiting: None,
        parked: None,
        // `TestState` tests one state, so there is no concurrent state around it to
        // park inside, no job of its own to abandon and no sibling branch whose
        // failure could stop it.
        abandoned: Vec::new(),
        cancel_after: None,
        resume_frames: Vec::new(),
        map_depth: 0,
        // `TestState` starts nothing: it reports what one state computes, and the
        // Map Runs a Distributed Map starts — and the child executions its items run
        // as — belong to an execution.
        map_runs: Vec::new(),
        map_runs_entered: 0,
        map_runs_entered_by_label: BTreeMap::new(),
        // `TestState` continues no execution: a state whose work is an invocation is
        // answered by the request's own `mock`, not by a worker's reply.
        reply: None,
        job_reply: None,
        callback_reply: None,
        resumed_attempts: None,
        attempts_spent: Vec::new(),
        resumed: None,
        machine_name: test.context.state_machine_name.clone(),
        region: super::arn_region(&test.context.state_machine_arn),
        execution_arn: test.context.execution_arn.clone(),
        child_counter: 0,
        // The state under test belongs to no state machine, so no workflow type
        // refuses it: the context the request supplies says nothing of one.
        express: test.context.express,
        // `TestState` redrives nothing: the step a redrive would continue from
        // belongs to an execution.
        failed_at: None,
        // The state under test is written in the language it declares; there is no
        // definition around it to inherit one from, so the other language's default
        // stands in for the absent member.
        language: JSONPATH,
        // "The set will include variables assigned in the state and variables set up
        // as test state input": the request's own set is what the state's expressions
        // read, and its `Assign` writes into it.
        variables: test
            .variables
            .and_then(Value::as_object)
            .map(|variables| {
                variables
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        // Nothing has been assigned BY the state under test yet; what the request set
        // up is the scope around it.
        assigned: BTreeMap::new(),
        // The request's own input is the caller's document here, so what the trial
        // reports of it is the bytes the request carried. The state under test
        // arrives PARSED, so the text the definition wrote its own documents in is
        // not one this walk was given.
        supplied: {
            let mut supplied = Supplied::default();
            supplied.supplied(test.input);
            supplied
        },
    };
    // And the same document for the diagnoses the trial's own expressions word, which
    // quote the document they could not be applied to ([`GivenDocuments`]).
    let _given = GivenDocuments::of(&interpreter.supplied);
    interpreter.trial(test, &raw, inspection)
}

impl Interpreter<'_> {
    /// [`test_state`]'s walk of the one state under test.
    fn trial(&mut self, test: &StateTest<'_>, raw: &Value, mut inspection: Inspection) -> Trial {
        let state = test.state;
        let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
        let context = self.context_for(&Scope::default(), test.name, Duration::ZERO);

        if let Some(language) = state.get("QueryLanguage").and_then(Value::as_str) {
            if !QUERY_LANGUAGES.contains(&language) {
                return Trial::failed(
                    inspection,
                    runtime(format!(
                        "{SINGLE_STATE_LABEL} declares the query language '{}', which is not one \
                         of {}, so nothing was run for it",
                        echoed(language),
                        QUERY_LANGUAGES.join(", ")
                    )),
                );
            }
        }
        let language = match self.language_of(state) {
            Ok(language) => language,
            Err(why) => return Trial::failed(inspection, runtime(why)),
        };
        // A member this emulator does not interpret changes what the state does, so
        // the state is not run for it — the rule the walk applies at every state. A
        // MOCKED state is exempt: the members that describe the invocation the mock
        // stands in for are the mock's business, and nothing is invoked for them; a
        // type this emulator does not run at all is reported by `produce` as the
        // invocation it is, not as a member.
        if test.mock.is_none() && EXECUTED_STATE_TYPES.contains(&state_type) {
            if let Some(member) = unexecutable_member(state_type, state, language) {
                return Trial::failed(
                    inspection,
                    runtime(format!(
                        "{SINGLE_STATE_LABEL} carries the member '{member}', which this emulator \
                         does not interpret, so nothing was run for it"
                    )),
                );
            }
        }
        // The input side: `InputPath`, then the `Parameters` payload template — or, in
        // JSONata, the `Arguments` the state hands its work. "Not populated when the
        // QueryLanguage is JSONata" is why each stage is reported only for the
        // language that has it.
        let effective = match self.input_value(state, raw, &context) {
            Ok(value) => value,
            Err(failure) => return self.classify(test, language, raw, &context, failure, inspection),
        };
        if language != JSONATA {
            inspection.after_input_path = Some(effective.to_string());
        }
        let templated = TEMPLATED_STATE_TYPES.contains(&state_type);
        let after_parameters = match templated {
            true => match self.task_request(state, language, &effective, &context) {
                Ok(value) => value,
                Err(failure) => {
                    return self.classify(test, language, raw, &context, failure, inspection)
                }
            },
            false => effective.clone(),
        };
        if templated {
            match language {
                JSONATA => inspection.after_arguments = Some(after_parameters.to_string()),
                _ => inspection.after_parameters = Some(after_parameters.to_string()),
            }
        }

        // A `Choice` state's transition is what the state DOES, and the walk decides
        // it before the state's output is built, so a rule that cannot be evaluated
        // is what such a state fails with.
        let chosen = if state_type == "Choice" {
            match self.choose(state, language, &effective, &context) {
                Ok(chosen) => Some(chosen),
                Err(failure) => {
                    return self.classify(test, language, raw, &context, failure, inspection)
                }
            }
        } else {
            None
        };

        let produced = self.produce(
            state,
            state_type,
            language,
            &after_parameters,
            &effective,
            &context,
            test.mock,
        );
        let placed = match produced {
            Ok(result) => match language {
                // A JSONata state's output is its `Output`, over the variables its
                // own `Assign` has just written; there are no result-side filters to
                // record stages for. A `Choice` state's own `Assign` and the SELECTED
                // rule's decide it together, exactly as they do in the walk.
                JSONATA => {
                    if let Some(result) = result.as_ref() {
                        inspection.result = Some(result.to_string());
                    }
                    match chosen.as_ref() {
                        Some(chosen) => {
                            self.choice_output(state, chosen.rule.as_ref(), &effective, &context)
                        }
                        None => {
                            self.jsonata_output(state, &effective, result.as_ref(), None, &context)
                        }
                    }
                }
                _ => match result {
                    Some(result) => self.place_recorded(
                        state,
                        state_type,
                        raw,
                        result,
                        &context,
                        &mut inspection,
                    ),
                    None => self.output_value(state, effective.clone(), &context),
                },
            },
            Err(failure) => Err(failure),
        };
        let output = match placed {
            Ok(output) => output,
            Err(failure) => return self.classify(test, language, raw, &context, failure, inspection),
        };
        // The document the state produced is measured here, at the state that
        // produced it, against what a state's output may carry — as the walk measures
        // every state's.
        if let Err(failure) = measure_payload(&output, SINGLE_STATE_LABEL) {
            return self.classify(test, language, raw, &context, failure, inspection);
        }
        inspection.variables = self.assigned_variables();

        let next_state = match chosen {
            // The rule that matched names the next state; a JSONata rule may also
            // declare the state's own output.
            Some(chosen) => Some(chosen.next),
            None if TERMINAL_STATE_TYPES.contains(&state_type) => None,
            None if state.get("End").and_then(Value::as_bool) == Some(true) => None,
            // A single state's `Next` names a state no definition around it declares,
            // so it is REPORTED rather than resolved; a state that names none has no
            // next state at all.
            None => state.get("Next").and_then(Value::as_str).map(str::to_string),
        };
        Trial::succeeded(Some(self.echo(&output)), next_state, inspection)
    }

    /// The variables this walk has assigned, as the one JSON document `TestState`
    /// reports them through — `None` where nothing assigned any, so the member is
    /// absent rather than an empty object.
    fn assigned_variables(&self) -> Option<String> {
        if self.variables.is_empty() {
            return None;
        }
        let variables: Map<String, Value> = self
            .variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        Some(Value::Object(variables).to_string())
    }

    /// The RAW result the state's own work produced: the document the result-side
    /// filters place, `None` for a state that produces no result of its own (its
    /// effective input is what `OutputPath` reads), or the failure it ended in.
    #[allow(clippy::too_many_arguments)]
    fn produce(
        &mut self,
        state: &Map<String, Value>,
        state_type: &str,
        language: &'static str,
        after_parameters: &Value,
        effective: &Value,
        context: &Value,
        mock: Option<&Mock>,
    ) -> Result<Option<Value>, Failure> {
        // The mock IS the outcome of the invocation it stands in for, so nothing is
        // invoked for it and the state's result is the one the request named.
        match mock {
            Some(Mock::Result(result)) => return Ok(Some(result.clone())),
            Some(Mock::Error(failure)) => return Err(failure.clone()),
            None => {}
        }
        match state_type {
            // "Parameters ... Used to create a collection of key-value pairs that
            // will be passed as input": it transforms the state's effective INPUT,
            // while `Result` is the state's RESULT — the rule the walk applies. A
            // JSONata `Pass` has no `Result`: what it puts out is its `Output`, so it
            // produces no result of its own.
            "Pass" => match language {
                JSONATA => Ok(None),
                _ => Ok(Some(
                    state
                        .get("Result")
                        .cloned()
                        .unwrap_or_else(|| after_parameters.clone()),
                )),
            },
            // A `Wait` state produces no result of its own. Its members are still
            // evaluated, so a malformed one is reported as the failure it is, but
            // nothing sleeps for the span it declares.
            "Wait" => {
                self.wait_duration(state, language, effective, context, Duration::ZERO)
                    .map_err(|failure| Failure {
                        error: failure.error,
                        cause: failure
                            .cause
                            .map(|why| format!("{SINGLE_STATE_LABEL}: {why}")),
                    })?;
                Ok(None)
            }
            "Choice" | "Succeed" => Ok(None),
            // A `Fail` state ends with the error and cause it declares, either of
            // which ASL may instead read from the state's input through a `*Path`.
            "Fail" => {
                let error =
                    self.fail_member(state, language, "Error", "ErrorPath", effective, context)?;
                let cause =
                    self.fail_member(state, language, "Cause", "CausePath", effective, context)?;
                Err(Failure { error, cause })
            }
            // Every remaining type's work is an invocation this process does not
            // make: a service integration, an iteration over items, a set of
            // branches. Without a mock to stand in for it, nothing ran — which is
            // what the state fails with, rather than a synthesised result.
            other => Err(runtime(format!(
                "a '{other}' state's own work is an invocation this emulator does not make, and \
                 the request names no mock to stand in for it, so nothing was run for it"
            ))),
        }
    }

    /// The result-side filters over a state's raw result — `ResultSelector`,
    /// `ResultPath` against the state's own raw input, then `OutputPath` — recording
    /// what the state carried after each one.
    fn place_recorded(
        &self,
        state: &Map<String, Value>,
        state_type: &str,
        raw: &Value,
        result: Value,
        context: &Value,
        inspection: &mut Inspection,
    ) -> Result<Value, Failure> {
        inspection.result = Some(result.to_string());
        let with_selector = SELECTOR_STATE_TYPES.contains(&state_type);
        let selected = select_result(state, result, context, with_selector)?;
        if with_selector {
            inspection.after_result_selector = Some(selected.to_string());
        }
        let placed = path_result(state, raw, selected)?;
        inspection.after_result_path = Some(placed.to_string());
        self.output_value(state, placed, context)
    }

    /// What `TestState` reports for a state whose work failed: the attempt one of its
    /// `Retry` retriers still owes (RETRIABLE), the catcher that takes the failure
    /// over (CAUGHT_ERROR), or the failure itself (FAILED).
    ///
    /// The two are the state's OWN error handling, read exactly as the walk reads it:
    /// the first retrier whose `ErrorEquals` names the error decides, and `Catch` is
    /// consulted only once no retrier has an attempt left.
    fn classify(
        &mut self,
        test: &StateTest<'_>,
        language: &'static str,
        raw: &Value,
        context: &Value,
        failure: Failure,
        inspection: Inspection,
    ) -> Trial {
        let state = test.state;
        let name = failure.name().to_string();
        for (index, retrier) in retry_entries(state, "Retry").iter().enumerate() {
            let Some(retrier) = retrier.as_object() else {
                continue;
            };
            if !errors_match(retrier, &name) {
                continue;
            }
            let max_attempts = retrier
                .get("MaxAttempts")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
            if test.retries_taken >= max_attempts {
                break;
            }
            let mut trial = Trial::failed(inspection, failure);
            trial.status = TEST_STATUS_RETRIABLE;
            trial.retry_index = Some(i32::try_from(index).unwrap_or(i32::MAX));
            trial.retry_backoff_seconds = Some(
                retry_delay_seconds(retrier, test.retries_taken).round() as i32,
            );
            return trial;
        }
        for (index, catcher) in retry_entries(state, "Catch").iter().enumerate() {
            let Some(catcher) = catcher.as_object() else {
                continue;
            };
            if !errors_match(catcher, &name) {
                continue;
            }
            let Some(next) = catcher.get("Next").and_then(Value::as_str) else {
                return Trial::failed(
                    inspection,
                    runtime("a 'Catch' catcher must carry a string 'Next'".to_string()),
                );
            };
            // "When a state reports an error and it matches a catcher, the state's
            // output is the error output", placed through the catcher's own
            // `ResultPath` — or, in JSONata, through its own `Output`, which is one
            // of the two places `$states.errorOutput` is in scope.
            let error_output = failure.payload();
            let effective = match self.input_value(state, raw, context) {
                Ok(effective) => effective,
                Err(failure) => return Trial::failed(inspection, failure),
            };
            let output = match language {
                JSONATA => {
                    self.jsonata_output(catcher, &effective, None, Some(&error_output), context)
                }
                _ => self.place_result(catcher, raw, error_output, context, true),
            };
            let output = match output {
                Ok(output) => output,
                Err(failure) => return Trial::failed(inspection, failure),
            };
            let mut trial = Trial::failed(inspection, failure);
            trial.status = TEST_STATUS_CAUGHT_ERROR;
            trial.output = Some(output.to_string());
            trial.next_state = Some(next.to_string());
            trial.catch_index = Some(i32::try_from(index).unwrap_or(i32::MAX));
            return trial;
        }
        Trial::failed(inspection, failure)
    }
}

/// A state's `Retry` or `Catch` entries, or none when it declares no such array.
fn retry_entries<'a>(state: &'a Map<String, Value>, member: &str) -> &'a [Value] {
    state
        .get(member)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

/// `time` as the ISO 8601 instant the context object reports.
fn iso(time: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from_millis(super::epoch_millis(time))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// An ISO 8601 instant in whole milliseconds since the Unix epoch.
///
/// The Amazon States Language's timestamps are ISO 8601 instants, and a timestamp
/// comparator "compares" them as the instants they DENOTE — so the whole instant
/// grammar is read here, a numeric UTC offset (`+01:00`) included, and the offset is
/// applied rather than refused: `2024-03-01T13:00:00+01:00` and
/// `2024-03-01T12:00:00Z` are one instant and compare equal. The maintained date-time
/// implementation parses it, NOT the wire-timestamp parser this service's modelled
/// members are deserialised by, whose RFC 3339 profile accepts no offset at all.
fn iso_millis(text: &str) -> Result<i64, String> {
    ::chrono::DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.timestamp_millis())
        .map_err(|_| format!("'{text}' is not an ISO 8601 instant"))
}
