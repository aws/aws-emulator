// SPDX-License-Identifier: CC0-1.0

//! The SUBSEGMENT TREE an execution's own segment carries: the work the run did, as the
//! nested spans a reader of the trace walks.
//!
//! # The rule this module implements
//!
//! A segment reports work that happened, and the work a workflow does is its STATES: a
//! recorded execution's segment therefore carries one subsegment per state the run
//! entered, named after that state and spanning the instants its own history says it was
//! entered and left. The tree MIRRORS the definition, because the work of a state that
//! runs other states is the work of those states:
//!
//! * a state whose work is CONCURRENT reports one subsegment per unit of that work, and
//!   the states of that unit sit inside it — a `Parallel` state one per branch of its
//!   `Branches` list, named by the branch's own position in that list, and a `Map` state
//!   one per iteration, named by "the index of the array belonging to a Map state" its
//!   iteration's events carry;
//! * a state whose work is a CALL to another service reports one subsegment per attempt
//!   it made, named as a call out to that service is named — by the name the AWS SDKs
//!   give the service and the operation the call made
//!   ([`super::super::integration::called_work`]) — so a state a `Retry` made three
//!   attempts at reports three of them, and a state whose first attempt answered reports
//!   one.
//!
//! Each entity carries the MARK its own outcome implies, out of the vocabulary X-Ray
//! documents for a segment document: "`fault` — boolean indicating that a server error
//! occurred" and "`error` — boolean indicating that a client error occurred", with the
//! `cause` block naming what it ended with. WHICH of them an unsuccessful outcome is
//! follows what the outcome was:
//!
//! * a CALL out is marked as X-Ray classifies the answer it got ([`failure_mark`]): a
//!   refusal the service answered with a client status is the error of that request, and
//!   work that failed any other way — a function that raised — is a fault of the work;
//! * a STATE whose failure the DEFINITION answered — a `Catch` took it over and the run
//!   went on — carries the error of the request it made
//!   ([`Tree::mark_answered_failures`]), and one whose failure nothing answered carries
//!   the fault of the work it was doing;
//! * work another unit's failure STOPPED carries the error of the work around it, stating
//!   that it was canceled;
//! * a state whose attempt failed and whose next attempt answered carries no mark at all:
//!   the state's work succeeded.
//!
//! A call out also carries the RESPONSE it answered, which is the status the invoked
//! service came back with, and the FUNCTION ERROR that answer reported where it reported
//! one ([`function_error_signal`]).
//!
//! # What it is built from
//!
//! Nothing here is a second interpreter: every entity, instant and mark is read off the
//! execution's own HISTORY ([`super::super::HistoryEvent`]) — the `<Type>StateEntered`
//! and `<Type>StateExited` events a state writes, the `<Type>StateAborted` one it writes
//! when another unit's failure stopped the work around it, the `MapIteration*` events one
//! iteration writes, and the hand-over and outcome events one attempt at a state's work
//! writes — and the
//! NESTING is read off the definition that execution ran
//! ([`super::super::RanConfiguration::definition`]), which is the one record of which
//! branch or processor a state belongs to. A history that names a state the definition
//! does not (and a definition this emulator cannot read as JSON) reports its states flat,
//! because a segment reports the work that happened as well as it can rather than
//! refusing to report it.
//!
//! Both walks are BOUNDED: the states reported are capped by
//! [`super::MAX_REPORTED_STATES`], the whole tree by [`MAX_ENTITIES`] and its nesting by
//! [`MAX_UNIT_DEPTH`], so a definition that nests without end or a run that loops without
//! end costs a bounded document — and the depth stays well inside the nesting X-Ray's own
//! ingestion accepts.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::serde_json::{json, Value};

use crate::state::{
    integration, EventDetails, Execution, HistoryEvent, EVENT_ACTIVITY_FAILED,
    EVENT_ACTIVITY_SCHEDULED, EVENT_ACTIVITY_SUCCEEDED, EVENT_ACTIVITY_TIMED_OUT,
    EVENT_FAIL_STATE_ENTERED, EVENT_LAMBDA_FUNCTION_FAILED, EVENT_LAMBDA_FUNCTION_SCHEDULED,
    EVENT_LAMBDA_FUNCTION_SUCCEEDED, EVENT_LAMBDA_FUNCTION_TIMED_OUT, EVENT_MAP_ITERATION_ABORTED,
    EVENT_MAP_ITERATION_FAILED, EVENT_MAP_ITERATION_STARTED, EVENT_MAP_ITERATION_SUCCEEDED,
    EVENT_TASK_FAILED, EVENT_TASK_SCHEDULED, EVENT_TASK_SUBMITTED, EVENT_TASK_SUBMIT_FAILED,
    EVENT_TASK_SUCCEEDED, EVENT_TASK_TIMED_OUT, STATE_ABORTED_SUFFIX, STATE_ENTERED_SUFFIX,
    STATE_EXITED_SUFFIX,
};

/// The prefix of the event types an EXECUTION's own outcome is written under, which
/// report the run rather than a state of it.
const EXECUTION_EVENT_PREFIX: &str = "Execution";

/// How the subsegment of one `Parallel` BRANCH is named, beside the branch's own position
/// in the state's `Branches` list.
const BRANCH_UNIT: &str = "Branch";

/// How the subsegment of one `Map` ITERATION is named, beside the index of the item that
/// iteration was handed.
const ITERATION_UNIT: &str = "Iteration";

/// What the `cause` of an entity covering work another unit's failure STOPPED states.
const CANCELED_CAUSE: &str = "Canceled";

/// The `namespace` an entity covering a call OUT of this service carries: `aws` is the
/// value X-Ray documents for "a subsegment that originates from an Amazon Web Services
/// SDK call", which is what makes the call's own downstream service a node of the trace.
const AWS_NAMESPACE: &str = "aws";

/// The status a call that ANSWERED its caller is reported with, where the result it
/// answered keeps none of the answer's own HTTP facts
/// ([`super::super::integration::answered_status`]).
///
/// A call that answered the state's result answered successfully, and a request-response
/// call of the services this one calls answers a success with `200 OK` — which is the
/// status AWS Lambda documents for the invocation such a task makes ("for synchronous
/// invocation [...] the status code is 200").
const ANSWERED_STATUS: u16 = 200;

/// The member a `Parallel` state declares its branches under, which is the one of the
/// crate's nested-machine members ([`crate::state::asl::NESTED_MACHINE_MEMBERS`]) that
/// carries a LIST of machines; every other member of that list names one machine, which
/// for a `Map` state is the processor each of its iterations runs.
const BRANCHES_MEMBER: &str = "Branches";

/// The deepest nesting of concurrent states the tree reports.
///
/// Emulator resource guard, NOT an AWS quota, and a bound of the DOCUMENT rather than of
/// the run: each scope costs two levels of nesting — the unit and the state inside it —
/// and the call a state's attempt made costs one more, so seven scopes keep the deepest
/// entity inside the sixteen levels X-Ray's own ingestion accepts. A document deeper than
/// that is refused WHOLE, which would lose the trace rather than shorten it.
///
/// The interpreter runs a definition nested up to [`crate::state::asl::MAX_NESTING_DEPTH`]
/// scopes deep, which is deeper than a document can carry: the states of a deeper scope
/// are reported inside the deepest unit this tree reports — a scope they really are inside
/// — rather than at a depth the document could not be ingested at.
const MAX_UNIT_DEPTH: usize = 7;

/// The most entities ONE tree holds — states, units and calls together.
///
/// Emulator resource guard, for the same reason [`super::MAX_REPORTED_STATES`] is one: a
/// run's loop can enter states, and a `Retry` make attempts, without end.
const MAX_ENTITIES: usize = 512;

/// The most states the nesting of a definition is read for.
///
/// Emulator resource guard: the definition is a caller's, bounded only at the modelled
/// 1 MiB, so the states read off it are bounded here rather than by its size.
const MAX_NESTED_STATES: usize = 1_024;

/// One subsegment per state the run entered, nested the way the definition nests those
/// states, with one subsegment per attempt each state's work made and the mark each
/// outcome implies.
///
/// This is the whole of what a run's segment reports BESIDE its own identity: see the
/// module docs for the rule and what it is read off.
pub(super) fn state_subsegments(execution: &Execution) -> Vec<Value> {
    let ended = execution.stop_date.unwrap_or(execution.start_date);
    let mut tree = Tree::new(&execution.ran.definition);
    for event in &execution.history {
        tree.record(event);
    }
    // A run that did not reach its end left work unfinished: the state it was in when it
    // ended did not do its work and did not fail at it either, which is the same thing a
    // sibling's failure does to the work it stops.
    if execution.status != crate::state::EXECUTION_STATUS_SUCCEEDED {
        tree.cancel_unfinished_states(ended);
    }
    tree.mark_answered_failures();
    tree.mark_units_from_their_work();
    tree.documents(ended)
}

/// Which MARK an entity's outcome gives it, per the vocabulary in the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// Work that did what it was asked, which carries no mark at all.
    Clean,
    /// Work that FAILED at what it was doing, which is a fault of the work the entity
    /// covers.
    Fault,
    /// Work a service REFUSED for what the request asked of it, which is an error of that
    /// request rather than a fault of the work.
    Error,
    /// Work another unit's failure STOPPED, which is not the stopped work's own doing: an
    /// error of the work around it, stating that it was canceled.
    Canceled,
}

/// Which concurrent UNIT of a container state a nested state belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    /// One branch of a `Parallel` state, by its position in `Branches`.
    Branch(usize),
    /// One iteration of a `Map` state, whose index is a fact of the RUN rather than of
    /// the definition, so it is taken from the iteration the run had started.
    Iteration,
}

/// One container state a state of the definition sits inside, and the unit of it the
/// state belongs to.
#[derive(Debug, Clone)]
struct Nested {
    container: String,
    unit: Unit,
}

/// The call OUT one attempt at a state's work made, as the trace names it.
#[derive(Debug, Clone)]
struct Called {
    /// The name the AWS SDKs give the service the call addressed, which is what the
    /// entity covering the call is named after.
    service: &'static str,
    /// The API operation the call made.
    operation: String,
    /// The function the call invoked, for a call that invoked one: the `aws` block of a
    /// call out names the resource the call addressed, and the one an invocation
    /// addressed is the function it named — as it named it, which is the name, the
    /// qualified name or the ARN that call carried.
    function: Option<String>,
}

/// WHAT one entity of the tree covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Covers {
    /// A state the run entered.
    State,
    /// One unit of a concurrent state's work: a branch, or an iteration.
    Unit,
    /// One attempt's call out to another service.
    Call,
}

/// One entity of the tree: a state the run entered, a unit of a concurrent state's work,
/// or one attempt's call out.
#[derive(Debug)]
struct Entity {
    name: String,
    covers: Covers,
    start: SystemTime,
    /// The instant the entity's own events say its work ended; `None` for work no event
    /// closed, whose span is then taken from the work inside it
    /// ([`Tree::ended_at`]).
    end: Option<SystemTime>,
    mark: Mark,
    /// The error name and cause the outcome reported, which the `cause` block states.
    error: Option<String>,
    cause: Option<String>,
    /// What this entity covers, for one that covers a call out.
    called: Option<Called>,
    /// The HTTP status the call this entity covers ANSWERED, for a call that answered at
    /// all.
    answered_status: Option<u16>,
    /// Whether the state this entity covers was LEFT: a state that failed and was left
    /// is one the definition ANSWERED the failure of ([`Tree::mark_answered_failures`]).
    left: bool,
    /// The FUNCTION ERROR that answer reported, for an invocation whose function raised.
    function_error: Option<&'static str>,
    children: Vec<usize>,
    /// The units of this state's concurrent work, by the name each carries.
    units: BTreeMap<String, usize>,
    /// Those units in the order they STARTED, which is how a state inside an iteration is
    /// attributed to one ([`Tree::open_unit`]).
    started_units: Vec<String>,
    /// The attempt at this state's work whose outcome has not been recorded yet.
    open_call: Option<usize>,
}

impl Entity {
    fn new(name: String, covers: Covers, start: SystemTime, called: Option<Called>) -> Self {
        Self {
            name,
            covers,
            start,
            end: None,
            mark: Mark::Clean,
            error: None,
            cause: None,
            called,
            answered_status: None,
            left: false,
            function_error: None,
            children: Vec::new(),
            units: BTreeMap::new(),
            started_units: Vec::new(),
            open_call: None,
        }
    }
}

/// The tree under construction: the entities, the roots, where each state's occurrences
/// sit, and the nesting the definition declares.
#[derive(Debug)]
struct Tree {
    entities: Vec<Entity>,
    roots: Vec<usize>,
    /// Every occurrence of each state name, in the order the run entered them: a state a
    /// loop re-entered has one entity per pass.
    occurrences: BTreeMap<String, Vec<usize>>,
    nesting: BTreeMap<String, Vec<Nested>>,
    /// How many state occurrences have been reported, against
    /// [`super::MAX_REPORTED_STATES`].
    states_reported: usize,
}

impl Tree {
    fn new(definition: &str) -> Self {
        Self {
            entities: Vec::new(),
            roots: Vec::new(),
            occurrences: BTreeMap::new(),
            nesting: nesting(definition),
            states_reported: 0,
        }
    }

    /// Take `event` into the tree: what it reports about the work is read off its TYPE,
    /// the state it names and the detail it carries.
    fn record(&mut self, event: &HistoryEvent) {
        let Some(name) = event.state_name.as_deref() else {
            return;
        };
        // An execution's own outcome reports the run rather than a state of it, and the
        // run's own segment already carries it.
        if event.event_type.starts_with(EXECUTION_EVENT_PREFIX) {
            return;
        }
        let at = event.timestamp;
        let details = &event.details;
        // A `Map` iteration's events name the state the iteration belongs to — "the name
        // of the iteration's parent Map state" — rather than a state of their own, so
        // they report the UNIT and not an occurrence.
        match event.event_type.as_str() {
            EVENT_MAP_ITERATION_STARTED => return self.unit_started(name, at, details),
            EVENT_MAP_ITERATION_SUCCEEDED => return self.unit_ended(name, at, details, Mark::Clean),
            EVENT_MAP_ITERATION_FAILED => {
                return self.unit_ended(name, at, details, failure_mark(details))
            }
            EVENT_MAP_ITERATION_ABORTED => {
                return self.unit_ended(name, at, details, Mark::Canceled)
            }
            _ => {}
        }
        if event.event_type.ends_with(STATE_ENTERED_SUFFIX) {
            self.entered(name, at);
            // A `Fail` state is the definition itself declaring the run failed, and it
            // is entered and never left, so its entry is also its outcome.
            if event.event_type == EVENT_FAIL_STATE_ENTERED {
                self.faulted(name, details, at);
            }
            return;
        }
        if event.event_type.ends_with(STATE_EXITED_SUFFIX) {
            return self.exited(name, at);
        }
        if event.event_type.ends_with(STATE_ABORTED_SUFFIX) {
            return self.canceled(name, at);
        }
        if let Some(called) = called_work(&event.event_type, details) {
            return self.call_scheduled(name, at, called);
        }
        match event.event_type.as_str() {
            // The outcome of one attempt at a state's work: it closes that attempt's own
            // call, and it is the state's own outcome too — a state whose attempt
            // answered succeeded, and one whose attempt failed failed, until a later
            // attempt of the same state says otherwise.
            EVENT_TASK_SUCCEEDED | EVENT_LAMBDA_FUNCTION_SUCCEEDED | EVENT_ACTIVITY_SUCCEEDED => {
                self.call_ended(name, at, Mark::Clean, details);
                self.succeeded(name);
            }
            EVENT_TASK_FAILED
            | EVENT_TASK_TIMED_OUT
            | EVENT_TASK_SUBMIT_FAILED
            | EVENT_LAMBDA_FUNCTION_FAILED
            | EVENT_LAMBDA_FUNCTION_TIMED_OUT
            | EVENT_ACTIVITY_FAILED
            | EVENT_ACTIVITY_TIMED_OUT => {
                match stopped_by_a_sibling(details) {
                    true => self.canceled(name, at),
                    false => {
                        let mark = failure_mark(details);
                        self.call_ended(name, at, mark, details);
                        self.faulted(name, details, at);
                    }
                }
            }
            // A run-a-job call ANSWERED, having started the job the task is then parked
            // at: the call out is over, and the state's own work is not.
            EVENT_TASK_SUBMITTED => self.call_ended(name, at, Mark::Clean, details),
            // Every other event a state's failure is reported through — a compound
            // state's own failure, an evaluation that could not be made — and every
            // other event its success is.
            other if other.ends_with("Failed") || other.ends_with("TimedOut") => {
                self.faulted(name, details, at);
            }
            other if other.ends_with("Succeeded") => self.succeeded(name),
            _ => {}
        }
    }

    /// Mark every state the run was still inside when it ENDED as work that did not
    /// finish, at `ended`.
    ///
    /// Only a STATE is: a unit of concurrent work records no end of its own at all — its
    /// span is the work inside it ([`Tree::ended_at`]) — and a call out the run ended
    /// before its answer was recorded was closed where the next event said so.
    fn cancel_unfinished_states(&mut self, ended: SystemTime) {
        let unfinished: Vec<usize> = self
            .occurrences
            .values()
            .flatten()
            .copied()
            .filter(|index| {
                let entity = &self.entities[*index];
                entity.end.is_none() && entity.mark == Mark::Clean
            })
            .collect();
        for index in unfinished {
            self.cancel(index, ended);
        }
    }

    /// Mark the failure of every state the DEFINITION ANSWERED as the error of the
    /// request that state made rather than a fault of the run's own work.
    ///
    /// A state that failed and was LEFT is one a `Catch` took over: "when a state reports
    /// an error and it matches a catcher, the state's output is the error output" and the
    /// run goes on from the state that catcher names, so the failure was answered inside
    /// the workflow. A failure nothing answered is never left behind — it is what the
    /// execution itself reports — and stays the fault of the work that failed at it.
    fn mark_answered_failures(&mut self) {
        let answered: Vec<usize> = self
            .occurrences
            .values()
            .flatten()
            .copied()
            .filter(|index| {
                let entity = &self.entities[*index];
                entity.mark == Mark::Fault && entity.left
            })
            .collect();
        for index in answered {
            self.entities[index].mark = Mark::Error;
        }
    }

    /// Give every unit of concurrent work whose own events stated nothing about WHY it
    /// ended the outcome of the work it ENDED AT.
    ///
    /// A `Map` iteration records an event of its own when it ends, and a `Parallel` BRANCH
    /// records none at all: "a `Parallel` branch is reported through the states it was
    /// in". Neither event states what the work ended with — the iteration's own events
    /// carry the index of the item it was handed and nothing else — so what the unit
    /// reports is what the LAST state it entered did: a unit that ended at a state whose
    /// failure nothing answered is a fault, one that ended at work a sibling's failure
    /// stopped is canceled, and one that ran to a state which did what it was asked
    /// carries no mark — including a unit whose earlier state failed and whose own `Catch`
    /// answered that failure, because the unit then went on and finished its work.
    ///
    /// A unit whose own events DID state a cause — an iteration the stop aborted in its
    /// own right — keeps what they said.
    fn mark_units_from_their_work(&mut self) {
        let units: Vec<usize> = (0..self.entities.len())
            .filter(|index| {
                let entity = &self.entities[*index];
                entity.covers == Covers::Unit
                    && entity.error.is_none()
                    && entity.cause.is_none()
            })
            .collect();
        for unit in units {
            let Some(outcome) = self.entities[unit]
                .children
                .last()
                .map(|child| {
                    let child = &self.entities[*child];
                    (child.mark, child.error.clone(), child.cause.clone())
                })
                .filter(|(mark, _, _)| *mark != Mark::Clean)
            else {
                continue;
            };
            let (mark, error, cause) = outcome;
            let entity = &mut self.entities[unit];
            entity.mark = mark;
            entity.error = error;
            entity.cause = cause;
        }
    }

    /// Report one ENTRY into the state `name`, inside the unit of the concurrent work it
    /// belongs to.
    fn entered(&mut self, name: &str, at: SystemTime) {
        if self.states_reported >= super::MAX_REPORTED_STATES {
            return;
        }
        let parent = self.unit_for(name, at);
        if let Some(index) = self.push(parent, name.to_string(), Covers::State, at, None) {
            self.states_reported += 1;
            self.occurrences
                .entry(name.to_string())
                .or_default()
                .push(index);
        }
    }

    /// The state `name` was LEFT at `at`, which closes its span.
    fn exited(&mut self, name: &str, at: SystemTime) {
        if let Some(index) = self.occurrence(name) {
            self.entities[index].end = Some(at);
            self.entities[index].left = true;
        }
    }

    /// The work around the state `name` was STOPPED at `at`, so the state's own work did
    /// not finish.
    fn canceled(&mut self, name: &str, at: SystemTime) {
        let Some(index) = self.occurrence(name) else {
            return;
        };
        self.cancel(index, at);
        if let Some(call) = self.entities[index].open_call.take() {
            self.cancel(call, at);
        }
    }

    /// Mark the entity `index` as work a failure elsewhere stopped, unless its own
    /// outcome has already been recorded.
    fn cancel(&mut self, index: usize, at: SystemTime) {
        let entity = &mut self.entities[index];
        entity.end.get_or_insert(at);
        if entity.mark == Mark::Clean {
            entity.mark = Mark::Canceled;
            entity.cause = Some(CANCELED_CAUSE.to_string());
        }
    }

    /// The state `name` FAILED, with what the event reported about it.
    ///
    /// A failure that says the state's work was STOPPED because a sibling unit's failure
    /// stopped it is not a fault of this state's own work: it is work that did not
    /// happen, which is what the trace reports as canceled — the same thing the
    /// `<Type>StateAborted` event says about a state another unit's failure stopped.
    fn faulted(&mut self, name: &str, details: &EventDetails, at: SystemTime) {
        let Some(index) = self.occurrence(name) else {
            return;
        };
        if stopped_by_a_sibling(details) {
            return self.canceled(name, at);
        }
        let entity = &mut self.entities[index];
        entity.mark = failure_mark(details);
        entity.error = details.error.clone();
        entity.cause = details.cause.clone();
    }

    /// The state `name` SUCCEEDED, which clears what an earlier attempt at its work
    /// reported: the state's work is what the attempt it ended on did.
    fn succeeded(&mut self, name: &str) {
        let Some(index) = self.occurrence(name) else {
            return;
        };
        let entity = &mut self.entities[index];
        entity.mark = Mark::Clean;
        entity.error = None;
        entity.cause = None;
    }

    /// One attempt at the state `name`'s work HANDED that work over, which is one call
    /// out of this service.
    fn call_scheduled(&mut self, name: &str, at: SystemTime, called: Called) {
        let Some(index) = self.occurrence(name) else {
            return;
        };
        // An attempt whose outcome no event reported ended when the next one was handed
        // over: nothing of it happened after that.
        if let Some(open) = self.entities[index].open_call.take() {
            self.entities[open].end.get_or_insert(at);
        }
        let child = self.push(
            Some(index),
            called.service.to_string(),
            Covers::Call,
            at,
            Some(called),
        );
        self.entities[index].open_call = child;
    }

    /// The attempt in flight at the state `name` ENDED at `at`, as `mark` says.
    fn call_ended(&mut self, name: &str, at: SystemTime, mark: Mark, details: &EventDetails) {
        let Some(index) = self.occurrence(name) else {
            return;
        };
        let Some(call) = self.entities[index].open_call.take() else {
            return;
        };
        let entity = &mut self.entities[call];
        entity.end = Some(at);
        entity.mark = mark;
        if mark != Mark::Clean {
            entity.error = details.error.clone();
            entity.cause = details.cause.clone();
            match details.cause.as_deref().and_then(integration::refused_status) {
                // A call a service REFUSED answered that refusal, which the cause it
                // carries states the status of.
                Some(status) => entity.answered_status = Some(status),
                // An INVOCATION whose answer reported a function error answered too: the
                // call was made and the function it ran raised, so it carries the status
                // an answered call does, and the signal that error was reported under.
                None => {
                    let signal = entity
                        .called
                        .as_ref()
                        .and_then(|called| {
                            function_error_signal(called, details.cause.as_deref())
                        });
                    if let Some(signal) = signal {
                        entity.answered_status = Some(ANSWERED_STATUS);
                        entity.function_error = Some(signal);
                    }
                }
            }
            return;
        }
        // A call that ANSWERED the state's result answered an HTTP response, which is
        // what the trace of the call reports: the status the result's own SDK envelope
        // names where the call's family reports one, and [`ANSWERED_STATUS`] where it
        // does not.
        entity.answered_status = Some(
            details
                .output
                .as_deref()
                .and_then(integration::answered_status)
                .unwrap_or(ANSWERED_STATUS),
        );
    }

    /// One ITERATION of the state `name` started at `at`, which opens the unit its own
    /// states sit inside.
    fn unit_started(&mut self, name: &str, at: SystemTime, details: &EventDetails) {
        let Some(container) = self.occurrence(name) else {
            return;
        };
        // An event that carries no index of its own names the unit by the position it
        // opens, which is how many the state has started already.
        let started = self.entities[container].started_units.len();
        let label = iteration_unit(details).unwrap_or_else(|| iteration_at(started));
        self.unit(container, &label, at);
    }

    /// One iteration of the state `name` ENDED at `at`, as `mark` says.
    fn unit_ended(&mut self, name: &str, at: SystemTime, details: &EventDetails, mark: Mark) {
        let Some(container) = self.occurrence(name) else {
            return;
        };
        // The unit the event names, and — for an event that carries no index — the one
        // still open, which is the unit such an event ends: the position the start
        // named is not the position this event would compute, because the unit it
        // opened is counted by now.
        let label = match iteration_unit(details) {
            Some(label) => label,
            None => {
                let Some(open) = self.open_unit(container) else {
                    return;
                };
                open
            }
        };
        let Some(unit) = self.entities[container].units.get(&label).copied() else {
            return;
        };
        match mark {
            Mark::Canceled => self.cancel(unit, at),
            _ => {
                let entity = &mut self.entities[unit];
                entity.end = Some(at);
                entity.mark = mark;
                if mark == Mark::Fault {
                    entity.error = details.error.clone();
                    entity.cause = details.cause.clone();
                }
            }
        }
    }

    /// The entity the state `name` sits inside: the unit of each container the definition
    /// nests it in, outermost first, and `None` for a state of the definition's own top
    /// level.
    ///
    /// A container the history holds no occurrence of — a definition this emulator could
    /// not read the nesting of, a state entered under a name the definition declares
    /// twice — stops the walk WHERE IT IS: the state is reported inside the deepest unit
    /// that was resolved, which is a scope it really is inside, rather than not at all.
    /// So does a tree that has no room for a further unit ([`MAX_ENTITIES`]).
    fn unit_for(&mut self, name: &str, at: SystemTime) -> Option<usize> {
        let nesting = self.nesting.get(name).cloned().unwrap_or_default();
        let mut parent: Option<usize> = None;
        for nested in nesting.iter().take(MAX_UNIT_DEPTH) {
            let Some(container) = self.occurrence(&nested.container) else {
                break;
            };
            let label = match nested.unit {
                Unit::Branch(index) => format!("{BRANCH_UNIT} {index}"),
                Unit::Iteration => match self.open_unit(container) {
                    Some(label) => label,
                    None => break,
                },
            };
            match self.unit(container, &label, at) {
                Some(unit) => parent = Some(unit),
                None => break,
            }
        }
        parent
    }

    /// The unit `label` of the container `container`, created at `at` where the container
    /// has none yet.
    fn unit(&mut self, container: usize, label: &str, at: SystemTime) -> Option<usize> {
        if let Some(found) = self.entities[container].units.get(label).copied() {
            return Some(found);
        }
        let index = self.push(Some(container), label.to_string(), Covers::Unit, at, None)?;
        self.entities[container]
            .units
            .insert(label.to_string(), index);
        self.entities[container].started_units.push(label.to_string());
        Some(index)
    }

    /// The unit of `container` a state entered now belongs to: the one that started
    /// EARLIEST and whose own work has not ended, and the one started last otherwise.
    ///
    /// An iteration's own events carry the index of the item it was handed and the states
    /// inside it do not, so which iteration a state belongs to is read off the iterations
    /// the run had OPEN when that state was entered: the state's own events are written as
    /// the walk reaches it, and the walk takes the open iterations in the order they
    /// started — so the state entered while several are open belongs to the one that has
    /// been open longest, and the iteration that ends takes its own events with it.
    fn open_unit(&self, container: usize) -> Option<String> {
        let started = &self.entities[container].started_units;
        started
            .iter()
            .find(|label| {
                self.entities[container]
                    .units
                    .get(*label)
                    .is_some_and(|unit| self.entities[*unit].end.is_none())
            })
            .or_else(|| started.last())
            .cloned()
    }

    /// The occurrence of the state `name` an event now reports about: the one entered
    /// last whose span is still open, and the one entered last otherwise.
    fn occurrence(&self, name: &str) -> Option<usize> {
        let occurrences = self.occurrences.get(name)?;
        occurrences
            .iter()
            .rev()
            .find(|index| self.entities[**index].end.is_none())
            .or_else(|| occurrences.last())
            .copied()
    }

    /// Add one entity under `parent`, and answer where it sits; `None` once the tree
    /// holds as many entities as it may.
    fn push(
        &mut self,
        parent: Option<usize>,
        name: String,
        covers: Covers,
        at: SystemTime,
        called: Option<Called>,
    ) -> Option<usize> {
        if self.entities.len() >= MAX_ENTITIES {
            return None;
        }
        let index = self.entities.len();
        self.entities.push(Entity::new(name, covers, at, called));
        match parent {
            Some(parent) => self.entities[parent].children.push(index),
            None => self.roots.push(index),
        }
        Some(index)
    }

    /// The subsegments the segment carries: the entities of the definition's own top
    /// level, each with the tree inside it.
    fn documents(&self, ended: SystemTime) -> Vec<Value> {
        self.roots
            .iter()
            .map(|root| self.document(*root, ended))
            .collect()
    }

    /// One entity as the subsegment document it is reported as.
    fn document(&self, index: usize, ended: SystemTime) -> Value {
        let entity = &self.entities[index];
        let mut object = ::serde_json::Map::new();
        object.insert(
            "id".to_string(),
            json!(::service_common::xray_trace::mint_span_id()),
        );
        object.insert("name".to_string(), json!(entity.name));
        object.insert(
            "start_time".to_string(),
            json!(::service_common::xray_segments::epoch_seconds(entity.start)),
        );
        object.insert(
            "end_time".to_string(),
            json!(::service_common::xray_segments::epoch_seconds(
                self.ended_at(index, ended)
            )),
        );
        match entity.mark {
            Mark::Clean => {}
            Mark::Fault => {
                object.insert("fault".to_string(), json!(true));
            }
            Mark::Error | Mark::Canceled => {
                object.insert("error".to_string(), json!(true));
            }
        }
        if entity.mark != Mark::Clean {
            if let Some(cause) = cause_document(entity) {
                object.insert("cause".to_string(), cause);
            }
        }
        if let Some(called) = &entity.called {
            object.insert("namespace".to_string(), json!(AWS_NAMESPACE));
            let mut aws = ::serde_json::Map::new();
            aws.insert("operation".to_string(), json!(called.operation));
            if let Some(function) = &called.function {
                aws.insert("function_name".to_string(), json!(function));
            }
            if let Some(signal) = entity.function_error {
                aws.insert("function_error".to_string(), json!(signal));
            }
            object.insert("aws".to_string(), Value::Object(aws));
        }
        // "http — information about the original HTTP request": the response the call
        // this entity covers answered, for a call that answered one at all.
        if let Some(status) = entity.answered_status {
            object.insert(
                "http".to_string(),
                json!({ "response": { "status": status } }),
            );
        }
        if !entity.children.is_empty() {
            let children: Vec<Value> = entity
                .children
                .iter()
                .map(|child| self.document(*child, ended))
                .collect();
            object.insert("subsegments".to_string(), Value::Array(children));
        }
        Value::Object(object)
    }

    /// The instant the work of the entity `index` ended: the one its own events recorded,
    /// else the last instant the work inside it ran to, else `ended` — the instant the run
    /// itself ended, which is as far as any of its work can have run.
    fn ended_at(&self, index: usize, ended: SystemTime) -> SystemTime {
        let entity = &self.entities[index];
        if let Some(end) = entity.end {
            return end;
        }
        entity
            .children
            .iter()
            .map(|child| self.ended_at(*child, ended))
            .max()
            .unwrap_or(ended)
    }
}

/// The `cause` block an unsuccessful entity carries, or `None` where its outcome stated
/// nothing to report.
///
/// What it STATES depends on what the entity covers, because the two are different facts:
///
/// * a CALL out states what the service on the far side ANSWERED, which is that service's
///   own exception as the SDK the trace is written through renders it
///   ([`super::super::integration::current_sdk_rendering`]);
/// * a STATE, and a unit of its concurrent work, states the ERROR the work was classified
///   under — the name a `Retry` and a `Catch` match on, which is what a reader of the
///   trace reads the kind of failure off.
///
/// Beside it goes the exception a reader attributes a root cause by: "the exception(s)
/// that caused the error", each with "the exception message", which is what the failure
/// said happened, under the type it was classified as. A failure with no classification of
/// its own states what it said happened, because an entity reports its outcome as well as
/// it can rather than reporting nothing.
fn cause_document(entity: &Entity) -> Option<Value> {
    let classified = entity.error.as_deref().filter(|error| !error.is_empty());
    let said = entity.cause.as_deref().filter(|cause| !cause.is_empty());
    let answered = said.map(|said| {
        integration::current_sdk_rendering(said).unwrap_or_else(|| said.to_string())
    });
    let message = match entity.covers {
        Covers::Call => answered.clone().or_else(|| classified.map(str::to_string))?,
        _ => classified
            .map(str::to_string)
            .or_else(|| answered.clone())?,
    };
    let mut exception = ::serde_json::Map::new();
    exception.insert(
        "id".to_string(),
        json!(::service_common::xray_trace::mint_span_id()),
    );
    exception.insert(
        "message".to_string(),
        json!(answered.unwrap_or_else(|| message.clone())),
    );
    if let Some(error) = classified {
        exception.insert("type".to_string(), json!(error));
    }
    Some(json!({
        "message": message,
        "exceptions": [Value::Object(exception)],
    }))
}

/// The FUNCTION ERROR the answer to the call `called` reported, read off the failure's own
/// `cause`, or `None` for a call that addressed no function or whose answer reported none.
///
/// "`FunctionError` — if present, indicates that an error occurred during function
/// execution": an invocation whose function RAISED answers that signal beside the error
/// DOCUMENT the run produced — the `errorMessage` and `errorType` its runtime wrote —
/// which is what the task reports as the failure's cause. So a failed INVOCATION whose
/// cause is such a document is one the function errored in, and the signal it is reported
/// under is the one AWS Lambda reports for an error a handler raised
/// ([`::service_com_amazonaws_lambda_awsgirapiservice::state::execution::FUNCTION_ERROR_UNHANDLED`],
/// stated by the service that answers it rather than a second time here).
///
/// Only an invocation has one: the signal is a member of Lambda's own answer, so a call to
/// any other service reports none however its failure was worded. Nor does a refusal the
/// service answered, which states its own status instead, or a failure no answer came back
/// for.
fn function_error_signal(called: &Called, cause: Option<&str>) -> Option<&'static str> {
    let (lambda, invoke) = integration::invoked_function_work()?;
    if called.service != lambda || called.operation != invoke {
        return None;
    }
    let document: Value = ::serde_json::from_str(cause?).ok()?;
    document.get(FUNCTION_ERROR_DOCUMENT_TYPE)?.as_str()?;
    Some(
        ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::FUNCTION_ERROR_UNHANDLED,
    )
}

/// The member the error document an invocation's run produces names the class of the
/// error by, which is what makes a failure's cause such a document.
const FUNCTION_ERROR_DOCUMENT_TYPE: &str = "errorType";

/// Which MARK the failure `details` report gives the work that failed.
///
/// X-Ray's two outcome flags say WHOSE the unsuccessful outcome is: "`error` — boolean
/// indicating that a client error occurred" and "`fault` — boolean indicating that a
/// server error occurred". A failure a service REFUSED the task's call with carries the
/// status of that refusal ([`crate::state::integration::refused_status`]), and a client
/// status (4xx) is a refusal of what the request asked for — the error of that request.
/// Everything else is work that failed at what it was doing: a function that raised, a
/// bound that ran out, a state the definition failed — a fault of the work.
fn failure_mark(details: &EventDetails) -> Mark {
    match details
        .cause
        .as_deref()
        .and_then(integration::refused_status)
    {
        Some(status) if (400..500).contains(&status) => Mark::Error,
        _ => Mark::Fault,
    }
}

/// Whether the failure `details` report is one that says the state's work was STOPPED
/// because a sibling unit of the concurrent work around it failed
/// ([`crate::state::asl::SIBLING_STOP_CAUSE`]).
///
/// Such a state did not do its work and did not fail at it either: "the entire Parallel
/// state is considered to have failed and all the branches are stopped", so the work this
/// state would have done is work the stop prevented — which the trace reports as canceled,
/// exactly as it reports a state the stop caught in flight.
fn stopped_by_a_sibling(details: &EventDetails) -> bool {
    details.cause.as_deref() == Some(crate::state::asl::SIBLING_STOP_CAUSE)
}

/// The unit one `Map` iteration ran in, named by "the index of the array belonging to a
/// Map state" its own events carry, or `None` for an event that carries no index.
fn iteration_unit(details: &EventDetails) -> Option<String> {
    details.index.map(iteration_at)
}

/// The unit named by `position`, for an iteration no index names.
fn iteration_at<Index: ::std::fmt::Display>(position: Index) -> String {
    format!("{ITERATION_UNIT} {position}")
}

/// The call OUT the event `event_type` hands over, read off what that event names the
/// work by, and `None` for an event that hands no call over.
///
/// Which family of events a state's work is recorded under says what the work IS: a
/// `LambdaFunctionScheduled` names the function's own ARN and so hands over one
/// invocation of it, while a `TaskScheduled` names "the service name of the resource in a
/// task state" and "the action of the resource called by a task state" and so hands over
/// that action's call. An `ActivityScheduled` hands over no call at all: an activity's
/// work is done by a worker that polls for it, so there is nothing this service called.
fn called_work(event_type: &str, details: &EventDetails) -> Option<Called> {
    match event_type {
        EVENT_LAMBDA_FUNCTION_SCHEDULED => {
            let (service, operation) = integration::invoked_function_work()?;
            Some(Called {
                service,
                operation,
                // The function the invocation addressed, as the `Resource` addressed it:
                // that family's events name the work by the function's own ARN.
                function: details.resource.clone(),
            })
        }
        EVENT_TASK_SCHEDULED => {
            let resource_type = details.resource_type.as_deref()?;
            let action = details.resource.as_deref().unwrap_or_default();
            let (service, operation) = integration::called_work(resource_type, action)?;
            Some(Called {
                service,
                operation,
                function: invoked_function(details),
            })
        }
        EVENT_ACTIVITY_SCHEDULED => None,
        _ => None,
    }
}

/// The function an attempt's call invoked, read off the document that attempt handed the
/// call: an invocation names the function in the `FunctionName` the call takes, and that
/// is what the call ADDRESSED — a name, a qualified name or a function's ARN, as Lambda
/// documents the member, reported as the request wrote it.
///
/// `None` for a call that addressed no function, which every call to another service is.
fn invoked_function(details: &EventDetails) -> Option<String> {
    let input = details.input.as_deref()?;
    let document: Value = ::serde_json::from_str(input).ok()?;
    let named = document.get("FunctionName")?.as_str()?;
    if named.is_empty() {
        return None;
    }
    Some(named.to_string())
}

/// Where each state of `definition` sits: the concurrent states it is nested inside,
/// outermost first, and no entry at all for a state of the definition's own top level.
///
/// A definition this emulator cannot read as JSON nests nothing, so its run's states are
/// reported flat; a state NAME the definition declares in two scopes keeps the outermost
/// of them, because a history event names a state by that name alone.
fn nesting(definition: &str) -> BTreeMap<String, Vec<Nested>> {
    let Ok(document) = ::serde_json::from_str::<Value>(definition) else {
        return BTreeMap::new();
    };
    let mut found: BTreeMap<String, Vec<Nested>> = BTreeMap::new();
    read_nesting(&document, &[], &mut found, 0);
    found
}

/// Read the states of the state machine `machine` — which is a definition, a `Parallel`
/// branch or a `Map` processor — as sitting `inside` the containers already walked.
///
/// The members a state nests a machine under are the crate's own
/// ([`crate::state::asl::NESTED_MACHINE_MEMBERS`]), so a further spelling of a processor
/// is read here as soon as that one statement carries it: the member naming a LIST of
/// machines is the concurrent state's branches, each a unit of its own, and every other
/// one names the single machine a `Map` state's iterations each run.
///
/// The walk stops at [`MAX_UNIT_DEPTH`] scopes, which is the depth the tree can attribute
/// a state to, so no state is recorded inside a scope the tree would then truncate.
fn read_nesting(
    machine: &Value,
    inside: &[Nested],
    found: &mut BTreeMap<String, Vec<Nested>>,
    depth: usize,
) {
    if depth >= MAX_UNIT_DEPTH {
        return;
    }
    let Some(states) = machine.get("States").and_then(Value::as_object) else {
        return;
    };
    for (name, state) in states {
        if found.len() >= MAX_NESTED_STATES {
            return;
        }
        found.entry(name.clone()).or_insert_with(|| inside.to_vec());
        for member in crate::state::asl::NESTED_MACHINE_MEMBERS {
            let Some(nested) = state.get(*member) else {
                continue;
            };
            if *member == BRANCHES_MEMBER {
                for (index, branch) in nested
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let mut inside = inside.to_vec();
                    inside.push(Nested {
                        container: name.clone(),
                        unit: Unit::Branch(index),
                    });
                    read_nesting(branch, &inside, found, depth + 1);
                }
                continue;
            }
            let mut inside = inside.to_vec();
            inside.push(Nested {
                container: name.clone(),
                unit: Unit::Iteration,
            });
            read_nesting(nested, &inside, found, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{nesting, Unit};

    /// The nesting read off a definition: a state of the top level sits inside nothing, a
    /// state of a `Parallel` branch inside that branch, and a state of a `Map` processor
    /// inside an iteration of it — however deeply the two are combined, and in either
    /// spelling of the processor member.
    #[test]
    fn the_nesting_of_a_state_is_read_off_the_definition() {
        let definition = r#"{
            "StartAt": "Fan",
            "States": {
                "Fan": {
                    "Type": "Parallel",
                    "Branches": [
                        {"StartAt": "Left", "States": {"Left": {"Type": "Pass", "End": true}}},
                        {"StartAt": "Each", "States": {"Each": {
                            "Type": "Map",
                            "ItemProcessor": {"StartAt": "Item", "States": {
                                "Item": {"Type": "Pass", "End": true}
                            }},
                            "End": true
                        }}}
                    ],
                    "Next": "Legacy"
                },
                "Legacy": {
                    "Type": "Map",
                    "Iterator": {"StartAt": "Old", "States": {"Old": {"Type": "Pass", "End": true}}},
                    "End": true
                }
            }
        }"#;
        let read = nesting(definition);
        for (case, state, expected) in [
            ("a state of the top level", "Fan", vec![]),
            (
                "a state of the first branch",
                "Left",
                vec![("Fan", Unit::Branch(0))],
            ),
            (
                "a Map state of the second branch",
                "Each",
                vec![("Fan", Unit::Branch(1))],
            ),
            (
                "a state of that Map's processor",
                "Item",
                vec![("Fan", Unit::Branch(1)), ("Each", Unit::Iteration)],
            ),
            (
                "a state of a processor in the legacy spelling",
                "Old",
                vec![("Legacy", Unit::Iteration)],
            ),
        ] {
            let found: Vec<(&str, Unit)> = read
                .get(state)
                .map(|nesting| {
                    nesting
                        .iter()
                        .map(|nested| (nested.container.as_str(), nested.unit))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                found, expected,
                "case {case:?}: expected {state:?} nested in {expected:?}, got {found:?}"
            );
        }
    }

    /// Text that is no definition nests nothing, so the states of a run of it are
    /// reported flat rather than not at all.
    #[test]
    fn text_that_is_no_definition_nests_nothing() {
        let read = nesting("{\"StartAt\": ");
        assert!(
            read.is_empty(),
            "expected no nesting read off unparseable text, got {read:?}"
        );
    }
}
