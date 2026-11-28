// SPDX-License-Identifier: CC0-1.0

//! The WALK: how one execution runs its definition, state by state.
//!
//! One concern, split out of [`super`] so that what a definition IS (its types, its
//! bounds, the members each state type carries) is read apart from what running one
//! DOES. The type is [`super::Interpreter`], declared beside those types in the module
//! root; this file carries the inherent `impl` that walks a state list — every state
//! type's own step, the data flow around it, its `Retry` and `Catch`, and the two
//! places a walk SUSPENDS (an activity task a worker claims, a token a callback
//! integration handed out).
//!
//! Every method here is `pub(super)`: the module root enters the walk
//! ([`super::interpret`]) and `TestState`'s single-state trial drives the same steps,
//! so both reach them, and nothing outside this module's own tree can.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{Map, Value};

use super::*;

/// What an `Assign` block would leave behind: the scope it would install, and what the
/// state carrying it reports as having assigned.
///
/// The two are different documents: the SCOPE is every variable in force after the block,
/// which is what the states after it read, while what is reported is "a map of variable
/// name and value as a serialized JSON representation" of the block's OWN members, which
/// is what the event the state is left by carries.
struct Assigned {
    variables: BTreeMap<String, Value>,
    reported: BTreeMap<String, String>,
}

impl Interpreter<'_> {
    /// `value` as this service reports it: [`echoed_payload`] against the document
    /// the caller supplied to THIS walk.
    pub(super) fn echo(&self, value: &Value) -> String {
        echoed_payload(value, &self.supplied)
    }

    /// Whether this interpretation has run out of the WORK it may do.
    ///
    /// The ceilings on steps, payloads, paths and nesting bound each factor of the
    /// work a definition can ask for; this answers for their PRODUCT
    /// ([`MAX_INTERPRET_WORK`]), and it is what a run that asks for more work than one
    /// definition is interpreted for ends on. It is the work itself that is counted
    /// and never the clock, so what an execution ends as is decided by its definition
    /// and its input alone.
    pub(super) fn out_of_work(&self) -> Option<Failure> {
        work_spent().then(out_of_work)
    }

    /// Record that a service-integration call this walk waited for blocked it for
    /// `waited`.
    ///
    /// The walk is BLOCKED while a call it asked for is in flight — in another
    /// service's handler, which for a Lambda invocation runs a caller's own code — so
    /// that span is not interpretation and is charged to nothing the interpretation is
    /// judged by. What it IS charged to is the allowance that bounds the waiting one
    /// run may do ([`MAX_CALL_WAIT_DURATION`]), which the next call's own wait is
    /// clamped to what is left of ([`Interpreter::call_within`]): the thread one run
    /// holds is bounded, and a call that does not answer inside the span it was given
    /// is the task's own timeout rather than the execution's end.
    ///
    /// What is charged is the span the walk really WAITED, which is not the span the
    /// work took: the calls of a `Map` state's items are in flight together, so a run
    /// holds its thread once over spans that overlap and the allowance is debited once
    /// for them ([`Interpreter::perform_call`], which charges every call of the walk).
    pub(super) fn spent_calling(&mut self, waited: Duration) {
        self.call_wait_left = self.call_wait_left.saturating_sub(waited);
    }

    /// The span the next service-integration call of this walk may wait for: the
    /// state's own `TimeoutSeconds` where it declares one, clamped to what is left of
    /// the waiting this run may do.
    ///
    /// "TimeoutSeconds […] If the task runs longer than the specified seconds, this
    /// state fails with a States.Timeout error name": a state that declares one asks
    /// for no more than it, and a state that declares none is held to the allowance
    /// alone — so the answer is always a span, and a call that outlasts it is that
    /// task's timeout, which the state's `Retry` and `Catch` may answer for. A walk
    /// with NO waiting left hands no call over at all, so this is never asked for a
    /// span of zero.
    pub(super) fn call_within(&self, declared: Option<Duration>) -> Option<Duration> {
        Some(match declared {
            Some(declared) => declared.min(self.call_wait_left),
            None => self.call_wait_left,
        })
    }

    /// Do the work of `call` with `request` as its request, and report WHEN that work
    /// began — which for a call this turn had already started is the instant it was
    /// started at, so the span the task reports is the span its work really took.
    ///
    /// The waiting is charged here, to the allowance that bounds it
    /// ([`Interpreter::spent_calling`]): what it costs is the span the walk was really
    /// BLOCKED, which for a call that answered while a sibling item's was in flight is
    /// less than the span the work took — the allowance bounds the thread one run holds,
    /// and a run holds it once however many calls are in flight.
    ///
    /// `None` says the call has been STARTED and left in flight, and this turn is
    /// UNWINDING: the walk is inside one item of a `Map` state whose items run together,
    /// so the item yields here and its siblings' own calls are started beside this one.
    /// The item is walked again from where this turn began once the call answers, and
    /// takes the answer then ([`super::map_items`]); nothing of the turn that unwound is
    /// kept, so from this point on the walk runs nothing further.
    ///
    /// Every call this answers for is made exactly once. The turn that starts one is
    /// walked again only to the state that started it, where the answer of that very
    /// call — the same call with the same request — is what it takes; and a turn that
    /// somehow reached ANOTHER call fails the task there rather than making it, because
    /// the work of the call it already started has been performed and making this one
    /// would perform the task's work a second time.
    pub(super) fn perform_call(
        &mut self,
        call: &super::super::integration::Call,
        request: &Value,
        within: Option<Duration>,
        alive: Option<&super::super::integration::Alive>,
    ) -> Option<(super::super::integration::Answer, ::std::time::SystemTime)> {
        if let Some(started) = self.started.take() {
            if !started.is_for(call, request) {
                return Some((Err(started_another_call(call)), started.began()));
            }
            let began = started.began();
            // The turn has taken the call it was waiting for, so what it does from here
            // is done with it waiting: it cannot unwind past an answer it has taken.
            self.probing = None;
            let waited_from = super::super::now();
            let answer = started.settle();
            self.spent_calling(
                super::super::now()
                    .duration_since(waited_from)
                    .unwrap_or_default(),
            );
            return Some((answer, began));
        }
        // Only a call that IS the whole task may be left in flight: a run-a-job call
        // parks the walk at the job it started and a callback call hands a token out,
        // and both of those are records of the turn that made the call, which a turn
        // that unwinds does not keep. And only a turn that has SAMPLED nothing may,
        // because it is walked again from where it began: what it reaches the second
        // time is the same states over the same data, so the call it hands over is the
        // same call — unless something it ran sampled a value, which would make the
        // second turn ask for a different request ([`super::super::samples_drawn`]).
        let repeatable = self
            .probing
            .is_some_and(|drawn| drawn == super::super::samples_drawn());
        if repeatable && call.job.is_none() && !call.callback {
            if let Some(started) = self.tasks.begin(call, request, within) {
                self.started = Some(started);
                self.unwinding = true;
                return None;
            }
        }
        // Every other call is made with this turn waiting for it, so no later call of
        // the same turn can unwind what this one has already done.
        self.probing = None;
        let began = super::super::now();
        // A call carrying a CALLBACK token is held to the heartbeat window of that token
        // as well as to its own span: the work the token went to is what reports the
        // progress that window is measured from, so the window is re-read while the call
        // is in flight rather than fixed at the instant it was handed over.
        let answer = match alive {
            Some(alive) => self.tasks.perform_while_alive(call, request, within, alive),
            None => self.tasks.perform(call, request, within),
        };
        self.spent_calling(
            super::super::now()
                .duration_since(began)
                .unwrap_or_default(),
        );
        Some((answer, began))
    }

    /// Whether the definition's own `TimeoutSeconds` has run out by `at`, the offset
    /// into the run the next state would begin at.
    ///
    /// "The maximum number of seconds an execution of the state machine can run. If it
    /// runs longer than the specified time, the execution fails with a States.Timeout
    /// error name": the execution ENDS at that span, so a run still going there makes
    /// no further transition — and asks for no further work of another service — rather
    /// than running on to the emulator's own ceilings. What such a run is REPORTED as
    /// is the definition's timeout, which [`super::interpret`] answers for it.
    pub(super) fn past_timeout(&mut self, at: Duration) -> bool {
        if self.timeout_left.is_some_and(|left| at > left) {
            self.timed_out = true;
        }
        self.timed_out
    }

    /// The failure a nested state list this walk will not run reports: one that would
    /// nest `Map` iterations and `Parallel` branches more than [`MAX_NESTING_DEPTH`]
    /// levels deep, so the nesting a definition can ask for is capped rather than left
    /// to the thread's stack.
    ///
    /// Read by every path that walks a nested list: the one that walks it to its end in
    /// one go ([`Interpreter::nested_walk`]) and the one that walks a `Map` iteration a
    /// slice at a time ([`super::map_items`]), so both refuse the same definition with
    /// the same sentence.
    pub(super) fn too_deeply_nested(&self) -> Option<Failure> {
        (self.depth >= MAX_NESTING_DEPTH).then(|| {
            runtime(format!(
                "the definition nests 'Map' iterations and 'Parallel' branches more than the \
                 {MAX_NESTING_DEPTH} levels deep this emulator runs, so nothing was run for it"
            ))
        })
    }

    /// Walk a NESTED state list — a `Map` iteration or a `Parallel` branch — one
    /// level deeper, refusing a definition that nests past [`MAX_NESTING_DEPTH`].
    ///
    /// `iteration` is whether the nested list is one `Map` ITEM's, which is what a
    /// run-a-job task inside it is refused by ([`Interpreter::map_depth`]).
    pub(super) fn nested_walk(
        &mut self,
        states: &Map<String, Value>,
        start_at: &str,
        input: Value,
        at: Duration,
        scope: &Scope,
        iteration: bool,
    ) -> Branch {
        if let Some(failure) = self.too_deeply_nested() {
            return Branch {
                outcome: Err(failure),
                end: at,
                suspended: false,
            };
        }
        self.depth += 1;
        if iteration {
            self.map_depth += 1;
        }
        // A nested state list is walked to its OWN END inside the enclosing state's
        // turn, so once one has run the turn may no longer hand a call over and be put
        // down: what the nested list already did would be undone with it
        // ([`Interpreter::perform_call`]). It is not put back after the walk for the same
        // reason.
        self.probing = None;
        // A nested machine's states carry their OWN retry counts; the enclosing
        // state's is not theirs.
        let enclosing = std::mem::take(&mut self.retry_count);
        // And their own VARIABLES: a nested machine's states read what reaching the
        // state that declares it assigned, and what they assign does not outlive the
        // machine, which is how the language scopes an `Assign`.
        let enclosing_variables = self.variables.clone();
        let branch = self.walk(states, start_at, input, at, scope);
        self.variables = enclosing_variables;
        self.retry_count = enclosing;
        if iteration {
            self.map_depth -= 1;
        }
        self.depth -= 1;
        branch
    }

    /// Whether this branch is one a SIBLING branch's failure has stopped, for a state
    /// about to be ENTERED at `at`, and the failure that says so.
    ///
    /// "If any branch fails, the entire Parallel state is considered to have failed and
    /// all the branches are stopped": the state's own outcome is the failure of the
    /// branch that failed ([`Interpreter::run_branches`] keeps the earliest), so this
    /// failure is never what a caller reads — what it decides is that the states of a
    /// stopped branch are not entered.
    ///
    /// The branches of a `Parallel` all BEGIN together, so a state due at the very
    /// instant a sibling failed had already begun: it is entered and then aborted, as
    /// every other state the failure caught in flight is. What a stopped branch does
    /// NOT do is the WORK of such a state ([`Interpreter::stopped_before_working`]).
    pub(super) fn stopped_before_entering(&self, at: Duration) -> Option<Failure> {
        self.stopped_by_a_sibling(at, false)
    }

    /// Whether this branch is one a sibling branch's failure has stopped, for the WORK
    /// of a state entered at `at`: the call a `Task` state would make.
    ///
    /// Work the failure caught IN FLIGHT does not finish — "a send that had not
    /// completed does not occur" — so the call of a state entered at or after that
    /// instant is not made at all, and the task is stopped instead.
    pub(super) fn stopped_before_working(&self, at: Duration) -> Option<Failure> {
        self.stopped_by_a_sibling(at, true)
    }

    /// The failure a stopped branch's state carries, when `at` is past the instant a
    /// sibling failed — `inclusive` for the work of a state, which the failure catches
    /// in flight, and exclusive for entering one.
    fn stopped_by_a_sibling(&self, at: Duration, inclusive: bool) -> Option<Failure> {
        let when = self.cancel_after?;
        (at > when || (inclusive && at == when))
            .then(|| runtime(super::SIBLING_STOP_CAUSE.to_string()))
    }

    /// Walk `states` from `start_at`, with `input` as the first state's input and
    /// `at` as the offset into the run at which this branch begins.
    pub(super) fn walk(
        &mut self,
        states: &Map<String, Value>,
        start_at: &str,
        input: Value,
        at: Duration,
        scope: &Scope,
    ) -> Branch {
        let mut cursor = Cursor {
            name: start_at.to_string(),
            value: input,
            at,
        };
        loop {
            if let Some(branch) = self.walk_one(states, &mut cursor, scope) {
                return branch;
            }
        }
    }

    /// Run the ONE state a walk of `states` has reached, and say where that leaves it:
    /// the branch it ended as, and `None` for a walk that carries on — whose `cursor`
    /// then names the next state, the document it is entered with and the offset it
    /// begins at.
    ///
    /// One state at a time is what lets the iterations of a `Map` state be interleaved
    /// by the clock, each taken up where it left off ([`super::map_items`]); a walk that
    /// runs a state list to its end drives this in a loop.
    pub(super) fn walk_one(
        &mut self,
        states: &Map<String, Value>,
        cursor: &mut Cursor,
        scope: &Scope,
    ) -> Option<Branch> {
        let at = cursor.at;
        // The definition's own `TimeoutSeconds` comes FIRST: an execution still
        // going when it falls due has ENDED there, so it makes no further
        // transition — and the emulator's own ceilings below never decide the
        // outcome of a run the definition already bounded.
        if self.past_timeout(at) {
            return Some(Branch {
                outcome: Err(super::timed_out()),
                end: at,
                suspended: false,
            });
        }
        // A branch of the enclosing `Parallel` state has already FAILED, so the
        // state is failing and the work that had not begun does not begin: a state
        // due after that instant is not entered at all.
        if let Some(stopped) = self.stopped_before_entering(at) {
            return Some(Branch {
                outcome: Err(stopped),
                end: at,
                suspended: false,
            });
        }
        if let Some(failure) = self.out_of_work() {
            return Some(Branch {
                outcome: Err(failure),
                end: at,
                suspended: false,
            });
        }
        // Every transition costs the interpretation's work allowance at least this
        // much, whatever its state looked at, so [`MAX_INTERPRET_WORK`] bounds how many
        // transitions ONE interpretation makes however cheap each of them is. The
        // per-execution event quotas do not give that bound on their own: a Distributed
        // Map's children each have an allowance of their own, so the transitions one
        // request can drive are the product of the items a state fans out over and what
        // each child does, and this is what bounds that product. A charge past the
        // allowance is reported at the next transition, by the check above.
        spend_work(WORK_PER_TRANSITION);
        if self.events_left == 0 {
            return Some(Branch {
                outcome: Err(history_limit_exceeded()),
                end: at,
                suspended: false,
            });
        }
        let Some(state) = states.get(&cursor.name).and_then(Value::as_object) else {
            return Some(Branch {
                outcome: Err(runtime(format!(
                    "state '{}' is not declared by the state machine",
                    cursor.name
                ))),
                end: at,
                suspended: false,
            });
        };
        // The raw input of the state about to run, kept for the one use a
        // FAILURE has for it: the step a redrive continues from is that state
        // with the data it saw ([`Run::stopped_at`]).
        let raw = cursor.value.clone();
        let current = std::mem::take(&mut cursor.value);
        // What the execution's history held before this state's turn, so the turn can be
        // charged for whatever it records ([`Interpreter::events_recorded`]).
        let before = self.events_recorded();
        let step = match self.step(&cursor.name, state, current, at, scope) {
            Ok(step) => step,
            Err(fault) => {
                // "It continues the failed execution from the unsuccessful step
                // and uses the same input": the step is the TOP-LEVEL state of
                // the definition the execution stopped at, so a failure a nested
                // walk reports is recorded as the state that encloses it — the
                // frame that is at depth zero.
                if self.depth == 0 {
                    self.failed_at = Some(RedriveStep {
                        state_name: cursor.name.clone(),
                        state_input: raw.to_string(),
                    });
                }
                return Some(Branch {
                    outcome: Err(fault.failure),
                    end: fault.at,
                    suspended: false,
                });
            }
        };
        cursor.at = step.at;
        cursor.value = step.value;
        // Every transition costs the execution's history. A state records its own
        // events, which spend the allowance where they are recorded; one that recorded
        // NONE — a state whose entry the preserved history already holds — is charged
        // here, so the allowance strictly falls at every transition and a `Next` chain
        // that forms a cycle is stopped by it rather than spinning on what a state
        // happens to record.
        if self.events_recorded() == before {
            self.events_left = self.events_left.saturating_sub(1);
        }
        // The document this state produced is measured HERE, at the state that
        // produced it: a definition that grows what it carries is stopped at the
        // first state past what a state's output may hold, rather than after the
        // whole run.
        if let Err(failure) = measure_payload(&cursor.value, &cursor.name) {
            return Some(Branch {
                outcome: Err(failure),
                end: cursor.at,
                suspended: false,
            });
        }
        match step.next {
            Next::EndBranch => Some(Branch {
                outcome: Ok(std::mem::take(&mut cursor.value)),
                end: cursor.at,
                suspended: false,
            }),
            // The state scheduled an activity task, or started the job of a
            // run-a-job integration: the walk stops here, and `run` reports what
            // it is waiting for rather than an outcome. A nested list reports the
            // suspension to the state that encloses it, which is what parks a
            // `Parallel` branch at the job one of its states started.
            Next::Suspended => Some(Branch {
                outcome: Ok(std::mem::take(&mut cursor.value)),
                end: cursor.at,
                suspended: true,
            }),
            Next::Named(next) => {
                cursor.name = next;
                None
            }
        }
    }

    /// Run one state: `raw` is its input, `at` the offset it is entered at.
    ///
    /// A diagnosis the interpreter raises about this state's own expression is
    /// reported under the state's name and the id of the event it was ENTERED at
    /// ([`diagnosed`]), which is read here — before the state records anything — so
    /// the state that failed can be followed back to its place in the history. For the
    /// state a walk CONTINUES an execution at, that event is one the preserved history
    /// already holds, so its own id is what the diagnosis names ([`ContinuedEntry`]).
    pub(super) fn step(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        at: Duration,
        scope: &Scope,
    ) -> Result<StateStep, Fault> {
        let entered = match self.continued_entry.take() {
            Some(continued) => {
                self.skipping_entry = !continued.recorded;
                continued.event_id
            }
            None => self.next_event_id(),
        };
        // Where this state's own events begin, so a failure of its EXPRESSIONS is
        // recorded at the point it happened rather than after the event that records
        // the state failing ([`Interpreter::push_evaluation_failure`]).
        let from = self.steps.len();
        // And nothing another state's expressions left behind is read as this state's:
        // the field an expression failed in is remembered for the state being executed
        // ([`super::super::jsonata::take_failed_field`]), so this state starts with
        // none.
        super::super::jsonata::clear_failed_field();
        let ran = self.run_state(name, state, raw, at, scope, entered);
        // Whatever this state's turn reached, the entry it was not to record is behind
        // it: the next state the walk enters records its own.
        self.skipping_entry = false;
        match ran {
            Ok(step) => Ok(step),
            Err(fault) => {
                let fault = diagnosed(name, entered, fault);
                self.push_evaluation_failure(name, state, &fault, from);
                Err(fault)
            }
        }
    }

    /// Record the event a state writes for an EXPRESSION of its own that could not be
    /// evaluated, for a fault that is such a failure.
    ///
    /// "This event will only be present in state machines that have QueryLanguage set
    /// to JSONata, or individual states set to JSONata": the event is written for a
    /// failure of this state's own EXPRESSIONS and for no other fault, and it carries
    /// the error and the cause the execution itself reports — the same bytes, so a
    /// caller reading the history and a caller reading the execution are told the same
    /// thing — beside the field of the definition the diagnosis was worded for
    /// ([`super::super::jsonata::take_failed_field`]).
    ///
    /// Which faults those are: every failure under the error name an expression that
    /// could not be evaluated carries ([`QUERY_EVALUATION_ERROR`]), and the document a
    /// JSONATA state's own expressions BUILT outgrowing what a state may carry, which
    /// is that state's template failing under the language's data-limit name
    /// ([`DATA_LIMIT_ERROR`]) rather than under the other one.
    ///
    /// ONE event per failure, written for the state the expression is written IN, and
    /// written WHERE the evaluation failed: an expression is evaluated before the work
    /// it feeds, so the event naming it precedes the event that records the state
    /// failing (`from` is where this state's own events begin). A
    /// `Map` iteration's or a `Parallel` branch's failure is carried to the state that
    /// encloses it unchanged, and that state's own turn ends on the same fault, so a
    /// failure this walk has already recorded is not recorded again: the cause names
    /// the state and the event it was entered at, so an identical one is that same
    /// failure travelling outwards rather than a second expression failing.
    fn push_evaluation_failure(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        fault: &Fault,
        from: usize,
    ) {
        // The events of a Distributed Map's item are the child workflow execution's and
        // are not kept ([`Interpreter::unheld`]), so there is no list of its own to
        // record this on and the enclosing execution's is not it; the diagnosis travels
        // with the failure itself, which is what the child's record reports.
        if self.unheld.is_some() {
            super::super::jsonata::clear_failed_field();
            return;
        }
        let expression_failure = match fault.failure.name() {
            QUERY_EVALUATION_ERROR => true,
            DATA_LIMIT_ERROR => self.language_of(state).ok() == Some(JSONATA),
            _ => false,
        };
        if !expression_failure {
            // Nothing of this state's own expressions failed, so the field one of them
            // would have been written in is not this state's to report.
            super::super::jsonata::clear_failed_field();
            return;
        }
        // Held to the bytes the modelled `SensitiveCause` carries exactly as the
        // execution's own cause is, so the two really are the same bytes.
        let cause = fault.failure.cause.clone().map(clip_cause);
        let recorded = self.steps.iter().any(|step| {
            step.event_type == super::super::EVENT_EVALUATION_FAILED
                && step.details.cause == cause
        });
        if recorded {
            return;
        }
        let location = super::super::jsonata::take_failed_field().map(clip_location);
        let evaluation_failed = Step {
            event_type: super::super::EVENT_EVALUATION_FAILED,
            state_name: Some(name.to_string()),
            after: fault.at,
            terminal: false,
            details: super::super::EventDetails {
                error: fault.failure.error.clone(),
                cause,
                location,
                ..super::super::EventDetails::default()
            },
        };
        // The event belongs where the evaluation happened: before the event this state
        // records for having FAILED, which the failure of the expression is what
        // caused.
        let at = self
            .steps
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, step)| records_a_failure(step.event_type))
            .map_or(self.steps.len(), |(index, _)| index);
        if self.events_left == 0 {
            return;
        }
        self.events_left -= 1;
        self.steps.insert(at, evaluation_failed);
    }

    /// The id the next history event this walk records will carry.
    ///
    /// The walk's events are numbered in order from [`Interpreter::first_event_id`],
    /// so the id of the one about to be recorded is that first id plus the ones
    /// recorded before it. A walk that CONTINUES an execution from a reply reports
    /// only the steps that FOLLOW the point the reply was applied at
    /// ([`Resumed::index`]) — the ones before it were recorded by the call that
    /// suspended the execution — so those are the steps counted here. For the walk of a
    /// Distributed Map's item the count is the child execution's own
    /// ([`Interpreter::unheld`]), because its events are counted rather than kept.
    pub(super) fn next_event_id(&self) -> i64 {
        let recorded = match self.unheld {
            Some(counted) => counted,
            None => self
                .steps
                .len()
                .saturating_sub(self.resumed.map_or(0, |resumed| resumed.index)),
        };
        self.first_event_id
            .saturating_add(i64::try_from(recorded).unwrap_or(i64::MAX))
    }

    /// How many history events this walk has recorded so far.
    ///
    /// The events it HOLDS, for the walk of an execution's own history, and the count a
    /// child workflow execution's are kept as, for the walk of a Distributed Map's item
    /// ([`Interpreter::unheld`]). It is what tells a transition that recorded something
    /// from one that recorded nothing.
    fn events_recorded(&self) -> usize {
        match self.unheld {
            Some(counted) => counted,
            None => self.steps.len(),
        }
    }

    /// [`Interpreter::step`]'s own work: `entered` is the id of the event this state
    /// records on being entered.
    #[allow(clippy::too_many_arguments)]
    fn run_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        at: Duration,
        scope: &Scope,
        entered: i64,
    ) -> Result<StateStep, Fault> {
        let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
        // A state's name is a caller-chosen object key that lands in every history
        // event of that state. `validate` refuses a longer one at create time; this
        // is the same bound made structural, so no name the history retains can be
        // longer than the Amazon States Language allows.
        if name.chars().count() > MAX_STATE_NAME_CHARS {
            return Err(runtime(format!(
                "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                 definition declares one of {}",
                name.chars().count()
            ))
            .at(at));
        }
        if !EXECUTED_STATE_TYPES.contains(&state_type) {
            return Err(runtime(format!(
                "state '{name}' is of type '{state_type}', which this emulator does not execute, \
                 so nothing was run for it"
            ))
            .at(at));
        }
        let language = self.language_of(state).map_err(|why| runtime(why).at(at))?;
        if let Some(member) = unexecutable_member(state_type, state, language) {
            return Err(runtime(format!(
                "state '{name}' carries the member '{member}', which this emulator does not \
                 interpret, so nothing was run for it"
            ))
            .at(at));
        }

        let context = self.context_for(scope, name, state_type, at);
        // Where the execution's own clock stands for this state's turn: the instant every
        // expression of it that reads the current time reads ([`Interpreter::clock_offset`]),
        // which is the same offset the context object reports this state as entered at —
        // on top of the span the execution had already been charged before this walk
        // ([`Interpreter::clock_base`]).
        self.clock_offset = self.clock_base.saturating_add(at);
        // The variables in scope for THIS state, installed on the thread for its turn:
        // a JSONPath reference reads one as the first term of a path, and the paths of
        // this state are evaluated by plain functions the walk reaches through several
        // layers ([`variables`]). A state nested inside this one installs its own and
        // puts this one back, which is how the language scopes an `Assign`.
        let _scope = VariableScope::of(&self.variables);
        let effective = self
            .input_value(state, &raw, &context)
            .map_err(|failure| failure.clone().at(at))?;

        match state_type {
            "Pass" => {
                self.push_entered("PassStateEntered", name, at, &raw);
                // "Parameters ... Used to create a collection of key-value pairs
                // that will be passed as input": it transforms the state's effective
                // INPUT. `Result` is the state's RESULT, so a state that declares
                // both reports its `Result`, computed over the input `Parameters`
                // built. With neither, the result is the effective input unchanged.
                // Both members are JSONPath's: a JSONata `Pass` produces no result of
                // its own, so what it puts out is its `Output` over its own input.
                let result = match language {
                    JSONATA => None,
                    _ => {
                        let payload = match state.get("Parameters") {
                            Some(template) => payload_template(template, &effective, &context)
                                .map_err(|failure| failure.at(at))?,
                            None => effective.clone(),
                        };
                        Some(state.get("Result").cloned().unwrap_or(payload))
                    }
                };
                let value = self
                    .produced(state, language, &raw, &effective, result, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("PassStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(state).map_err(|failure| failure.at(at))?,
                })
            }
            "Wait" => {
                self.push_entered("WaitStateEntered", name, at, &raw);
                let declared = self
                    .wait_duration(state, language, &effective, &context, at)
                    .map_err(|failure| wait_failure(name, failure).at(at))?;
                // What this state still DELAYS the execution by: the span it declares,
                // and none at all for the one this walk is continuing from, whose span
                // has already run out ([`Interpreter::elapsed_wait`]).
                let waited = self.span_still_ahead(name, declared);
                // "A Wait state delays the state machine from continuing for a specified
                // time": a span still AHEAD of the execution suspends the walk here, and
                // the states after this one run when it has run out
                // ([`super::ParkedWait`]) — so what they do is done at that instant
                // rather than at the one the run was planned at, and a state after a
                // `Wait` acts on the world the wait left. A span of NO time delays
                // nothing, so such a state is left where it is reached.
                if let Some(parked) = self.park_at_wait(name, &raw, at, waited) {
                    self.parked_wait = Some(parked);
                    return Ok(StateStep {
                        value: raw,
                        at,
                        next: Next::Suspended,
                    });
                }
                // The span this state waits is the caller's definition's, which the model
                // bounds only in BYTES, so a chain of them is accumulated WITHOUT the
                // panic `Duration`'s own `Add` carries: an offset no duration can
                // represent holds at the furthest one, which is an instant the run never
                // reaches — exactly what an unreachable offset means.
                let at = at.saturating_add(waited);
                let value = self
                    .produced(state, language, &raw, &effective, None, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("WaitStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: self.next_of(state).map_err(|failure| failure.at(at))?,
                })
            }
            "Choice" => {
                self.push_entered("ChoiceStateEntered", name, at, &raw);
                let chosen = self
                    .choose(state, language, &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                // "Output ... you can also specify Output in a Choice rule", and the
                // same for `Assign`: a `Choice` puts out — and assigns — what the
                // SELECTED rule's own members decided, and what the state's own
                // decided only where no rule matched
                // ([`Interpreter::choice_output`]). A JSONPath rule carries no
                // `Output`, so that language's state puts out what every other state
                // written in it does.
                let value = match language {
                    JSONATA => self
                        .choice_output(state, chosen.rule.as_ref(), &effective, &context)
                        .map_err(|failure| failure.at(at))?,
                    _ => {
                        let owner = chosen
                            .rule
                            .as_ref()
                            .map_or(state, |(_, rule)| rule);
                        // What the node that decided the transition assigns COMMITS for
                        // the states after it, exactly as every other node's `Assign`
                        // does ([`Interpreter::jsonpath_assign`]): this state's own
                        // `OutputPath` is evaluated against the scope the state was
                        // reached with, which the block put back.
                        self.jsonpath_assign(owner, &effective, &context)
                            .map_err(|failure| failure.at(at))?;
                        self.output_value(state, effective.clone(), &context)
                            .map_err(|failure| failure.at(at))?
                    }
                };
                self.push_exited("ChoiceStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::Named(chosen.next),
                })
            }
            "Succeed" => {
                self.push_entered("SucceedStateEntered", name, at, &raw);
                let value = self
                    .produced(state, language, &raw, &effective, None, &context, false)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("SucceedStateExited", name, at, &value);
                Ok(StateStep {
                    value,
                    at,
                    next: Next::EndBranch,
                })
            }
            "Fail" => {
                self.push_entered(super::super::EVENT_FAIL_STATE_ENTERED, name, at, &raw);
                // A `Fail` state ends the execution with the error and cause it
                // declares, both of which ASL leaves optional and either of which
                // may instead be read from the state's input — through a `*Path` in
                // JSONPath, through an expression in the member itself in JSONata.
                let error = self
                    .fail_member(state, language, "Error", "ErrorPath", &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                let cause = self
                    .fail_member(state, language, "Cause", "CausePath", &effective, &context)
                    .map_err(|failure| failure.at(at))?;
                Err(Failure {
                    error,
                    cause,
                    ..Failure::default()
                }
                .at(at))
            }
            "Map" => self.map_state(name, state, language, raw, effective, &context, at),
            "Task" => self.task_state(name, state, language, raw, effective, &context, at, entered),
            // `Parallel` is the only type left: the list above is exhaustive.
            _ => self.parallel_state(name, state, language, raw, effective, &context, at),
        }
    }

    /// The query language one state is evaluated in: the one it declares, else the
    /// one the definition does.
    ///
    /// The `Err` is a `QueryLanguage` that names no query language the Amazon States
    /// Language defines, which `validate` refuses at create time.
    pub(super) fn language_of(&self, state: &Map<String, Value>) -> Result<&'static str, String> {
        Ok(declared_language(state.get("QueryLanguage"))?.unwrap_or(self.language))
    }

    /// The bindings a JSONata expression of this state is evaluated with.
    pub(super) fn bindings<'b>(
        &'b self,
        input: &'b Value,
        context: &'b Value,
        result: Option<&'b Value>,
        error_output: Option<&'b Value>,
    ) -> super::super::jsonata::Bindings<'b> {
        super::super::jsonata::Bindings {
            input,
            context,
            result,
            error_output,
            variables: &self.variables,
            // The instant the execution's own clock stands at, which is what the
            // language's current-instant functions answer from: the walk sleeps for none
            // of the spans a definition asks for, so this is ahead of this process's own
            // clock by every span the run has been charged
            // ([`Interpreter::clock_offset`]).
            clock: super::super::after(self.start, self.clock_offset),
        }
    }

    /// What the expressions of ONE visit to a state may build and evaluate: the
    /// allowance is per visit, and what each evaluation COSTS is charged to this
    /// interpretation's own work allowance, so neither the member count of a template
    /// nor the work its expressions do is left to the definition.
    pub(super) fn jsonata_budget(&self) -> super::super::jsonata::Budget {
        super::super::jsonata::Budget::new()
    }

    /// What a state PUTS OUT, in the query language it is written in.
    ///
    /// JSONPath places the state's result through `ResultSelector`, `Assign`,
    /// `ResultPath` and `OutputPath`; JSONata evaluates the state's `Assign` and then
    /// its `Output`. One seam for both, so every state type's output follows the rule
    /// of the language that state is written in.
    ///
    /// `result` is what the state's own work produced, and `None` for a state that
    /// produces none and carries its input through (a `Pass`, a `Wait`, a `Choice`, a
    /// `Succeed`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn produced(
        &mut self,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        result: Option<Value>,
        context: &Value,
        with_selector: bool,
    ) -> Result<Value, Failure> {
        if language == JSONATA {
            return self.jsonata_output(state, effective, result.as_ref(), None, context);
        }
        match result {
            Some(result) => {
                let selected = select_result(state, result, context, with_selector)?;
                // The state's own `Assign` is evaluated over what its work PRODUCED,
                // which is also the document a `ResultSelector` has just filtered, and
                // before the result is placed: `ResultPath` and `OutputPath` select
                // from the state's data, while the assignments read the state's result
                // ([`Interpreter::jsonpath_assign`]).
                self.jsonpath_assign(state, &selected, context)?;
                let placed = path_result(state, raw, selected)?;
                self.output_value(state, placed, context)
            }
            None => {
                // A state that produces no result of its own carries its input
                // through, so that input is what its assignments read.
                self.jsonpath_assign(state, effective, context)?;
                self.output_value(state, effective.clone(), context)
            }
        }
    }

    /// Apply the `Assign` of a state, a `Choice` rule or a `Catch` catcher written in
    /// JSONPath, over the document `read` that node's expressions select from.
    ///
    /// "Assign ... store values in variables": the block is a PAYLOAD TEMPLATE in this
    /// language — "if any field within the Payload Template ... has a name ending with
    /// the characters '.$', its value is transformed according to rules below and the
    /// field is renamed to strip the '.$' suffix" — so a member named `"total.$"`
    /// assigns `total` the value its path or intrinsic selected, and every other member
    /// assigns the literal the definition wrote. The variables outlive the state, which
    /// is what a later state's paths read them through ([`variables`]).
    ///
    /// Each member is one variable and they are evaluated IN ORDER, so an assignment
    /// reads the ones written before it — the rule the other language's `Assign` follows
    /// too ([`Interpreter::jsonata_output`]). All of them spend from the ONE budget a
    /// payload template is given, so a state cannot build more than one document however
    /// many variables it declares.
    ///
    /// What the block assigns COMMITS for the nodes AFTER this one: the scope the node's
    /// own remaining fields are evaluated against is put back to the one it was reached
    /// with, so a node that both reads and writes one variable reads the value the run
    /// already held and the next state is the first to read what this block wrote. The
    /// committed scope is the one the interpreter carries, which is what the next state's
    /// turn installs ([`VariableScope::of`]).
    ///
    /// A block that cannot be built assigns NOTHING: the values are rendered into a copy
    /// of the scope and installed once, so no later state reads half of one state's
    /// assignments.
    pub(super) fn jsonpath_assign(
        &mut self,
        node: &Map<String, Value>,
        read: &Value,
        context: &Value,
    ) -> Result<(), Failure> {
        let Some(assignments) = node.get(ASSIGN_MEMBER) else {
            return Ok(());
        };
        let assignments = assignments.as_object().ok_or_else(|| {
            runtime(format!("'{ASSIGN_MEMBER}' must be a JSON object of variables"))
        })?;
        let built = self.build_assignments(assignments, read, context);
        let reached_with = ::std::mem::take(&mut self.variables);
        let Assigned {
            variables,
            reported,
        } = match built {
            Ok(built) => built,
            Err(failure) => {
                // A block that cannot be built assigns NOTHING: the members before the
                // one that failed were installed for the members after it to read, so
                // the scope the REST of this state's expressions read is put back as
                // the state found it — it is the scope `self.variables` still holds.
                VariableScope::reinstall(&reached_with);
                self.variables = reached_with;
                return Err(failure);
            }
        };
        self.variables = variables;
        // And the scope this node's own remaining fields read is that same one: what
        // the block assigned commits for the nodes AFTER this one.
        VariableScope::reinstall(&reached_with);
        // What THIS state assigned, for the event it is left by: a state that assigns
        // nothing reports nothing, whatever the scope around it holds.
        self.assigned.extend(reported);
        Ok(())
    }

    /// [`Interpreter::jsonpath_assign`]'s own work: the scope the block would leave and
    /// what it assigned, or the failure it could not be built past.
    ///
    /// The members are installed on the thread AS THEY ARE BUILT, because an assignment
    /// reads the ones written before it; the caller is what puts the scope back where a
    /// member failed.
    fn build_assignments(
        &self,
        assignments: &Map<String, Value>,
        read: &Value,
        context: &Value,
    ) -> Result<Assigned, Failure> {
        let mut budget = TemplateBudget::of_one_payload();
        let mut variables = self.variables.clone();
        let mut reported = BTreeMap::new();
        for (member, template) in assignments {
            budget.spend(member.len() + 3)?;
            let (variable, value) = match member.strip_suffix(".$") {
                Some(variable) => {
                    let text = template.as_str().ok_or_else(|| {
                        named(
                            PARAMETER_PATH_ERROR,
                            format!(
                                "'{member}' must be a JSONPath or intrinsic-function string, not \
                                 {template}"
                            ),
                        )
                    })?;
                    budget.charge_path()?;
                    (variable, template_expression(member, text, read, context)?)
                }
                None => (
                    member.as_str(),
                    render_template(template, read, context, 1, &mut budget)?,
                ),
            };
            // A variable is a document the execution retains and a later state reads,
            // so it is measured against the same ceiling every document this walk
            // produces is measured against.
            budget.spend(measure_payload(&value, UNNAMED_PRODUCER)?)?;
            // The variable is in scope for the assignments after it, which is what the
            // paths and the intrinsics of those members read it through. ONE variable
            // is bound rather than the scope re-installed, so a block of many members
            // costs the sum of its members and not their count times the scope's size.
            VariableScope::bind(variable, &value);
            reported.insert(variable.to_string(), value.to_string());
            variables.insert(variable.to_string(), value);
        }
        // And the variables the execution would then hold TOGETHER are held to the
        // space an execution's variables have.
        Self::measured_variable_space(&variables)?;
        Ok(Assigned {
            variables,
            reported,
        })
    }

    /// Hold what an `Assign` would leave in scope to the bytes an execution's variables
    /// may carry together ([`MAX_VARIABLE_BYTES`]).
    ///
    /// Measured over the whole SCOPE rather than over what one state wrote, because the
    /// quota is the space the execution's variables occupy: a variable assigned a second
    /// value takes the place of its first, and the variables of a state machine the walk
    /// has left are not in scope at all, so both free the space they held.
    pub(super) fn measured_variable_space(
        variables: &BTreeMap<String, Value>,
    ) -> Result<(), Failure> {
        let mut bytes = 0usize;
        for value in variables.values() {
            // Each variable is measured against what ONE document may carry as well,
            // which is the per-variable half of the same quota.
            bytes = bytes.saturating_add(measure_payload(value, UNNAMED_PRODUCER)?);
            if bytes > MAX_VARIABLE_BYTES {
                return Err(named(DATA_LIMIT_ERROR, variable_space_exceeded()));
            }
        }
        Ok(())
    }

    /// A JSONata state's `Output`, and the `Assign` that is evaluated after it.
    ///
    /// The fields of a node are evaluated in a FIXED order with `Assign` LAST: the
    /// node's `Output` is built first, against the variables the run held when the node
    /// was reached, and only then are the assignments made — so the first of the two to
    /// fail is the one the failure names, and a node that both reads and writes one
    /// variable puts out the value it was reached with.
    ///
    /// "Assign ... store values in variables", and what a node assigns is in scope for
    /// the nodes AFTER it: the assignments COMMIT when the node is done with its own
    /// fields, so the next state is the first to read what this `Assign` wrote — the
    /// rule the other query language's `Assign` follows too
    /// ([`Interpreter::jsonpath_assign`]).
    ///
    /// A node that declares no `Output` puts out what the state's own data flow owes
    /// at that point: "when a state reports an error and it matches a catcher, the
    /// state's output is the error output", so a CATCHER that declares none puts out
    /// the error output it was handed; a state whose work produced a result puts out
    /// that result; and a state whose work produced none carries its own input through.
    ///
    /// A state that FAILS assigns nothing: where either field cannot be evaluated, the
    /// scope is put back as the state found it, so the states after a caught failure
    /// read what reaching the failed state had assigned and no more — and the event the
    /// state is left by reports no assignment of its own.
    pub(super) fn jsonata_output(
        &mut self,
        state: &Map<String, Value>,
        input: &Value,
        result: Option<&Value>,
        error_output: Option<&Value>,
        context: &Value,
    ) -> Result<Value, Failure> {
        // ONE allowance for the `Output` and the `Assign` after it: they are the
        // expressions of one template, so they spend from the same budget.
        let mut budget = self.jsonata_budget();
        let scope = self.variables.clone();
        let assigned = self.assigned.clone();
        let built = match state.get(OUTPUT_MEMBER) {
            None => Ok(error_output
                .or(result)
                .cloned()
                .unwrap_or_else(|| input.clone())),
            Some(template) => {
                let rendered = {
                    let bindings = self.bindings(input, context, result, error_output);
                    super::super::jsonata::render(
                        template,
                        OUTPUT_MEMBER,
                        &bindings,
                        &mut budget,
                    )
                };
                rendered.map_err(jsonata_failure)
            }
        };
        let produced = built.and_then(|value| {
            self.jsonata_assign(
                state,
                ASSIGN_MEMBER,
                input,
                result,
                error_output,
                context,
                &mut budget,
            )
            .map(|()| value)
        });
        if produced.is_err() {
            self.variables = scope;
            self.assigned = assigned;
        }
        produced
    }

    /// Apply the `Assign` of a state, a `Choice` rule or a `Catch` catcher.
    ///
    /// Each member is one variable, and they are evaluated in order, so an assignment
    /// reads the ones written before it. All of them spend from the ONE budget the
    /// visit was given, so a state cannot assign more than what a state's document
    /// may carry however many variables it declares.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn jsonata_assign(
        &mut self,
        node: &Map<String, Value>,
        at: &str,
        input: &Value,
        result: Option<&Value>,
        error_output: Option<&Value>,
        context: &Value,
        budget: &mut super::super::jsonata::Budget,
    ) -> Result<(), Failure> {
        let Some(assignments) = node.get(ASSIGN_MEMBER) else {
            return Ok(());
        };
        let assignments = assignments.as_object().ok_or_else(|| {
            runtime(format!("'{ASSIGN_MEMBER}' must be a JSON object of variables"))
        })?;
        // The variables are built on a copy and installed once: an assignment that
        // fails leaves the scope as the state found it, so no later state reads half
        // of one state's assignments.
        let mut variables = self.variables.clone();
        let mut assigned = BTreeMap::new();
        for (variable, template) in assignments {
            let value = {
                let bindings = super::super::jsonata::Bindings {
                    input,
                    context,
                    result,
                    error_output,
                    variables: &variables,
                    clock: super::super::after(self.start, self.clock_offset),
                };
                super::super::jsonata::render(
                    template,
                    &super::super::jsonata::member_field(at, variable),
                    &bindings,
                    budget,
                )
                .map_err(jsonata_failure)?
            };
            // A variable is a document the execution retains and a later state reads,
            // so it is measured against the same ceiling every document this walk
            // produces is measured against.
            measure_payload(&value, UNNAMED_PRODUCER)?;
            assigned.insert(variable.clone(), value.to_string());
            variables.insert(variable.clone(), value);
        }
        // And the variables the execution would then hold TOGETHER are held to the
        // space an execution's variables have ([`MAX_VARIABLE_BYTES`]).
        Self::measured_variable_space(&variables)?;
        self.variables = variables;
        // What THIS state assigned, for the event it is left by: a state that assigns
        // nothing reports nothing, whatever the scope around it holds.
        self.assigned.extend(assigned);
        Ok(())
    }

    /// A field that is either one JSONata expression or the literal it is written as,
    /// for a state written in that language; the literal itself for a JSONPath state.
    pub(super) fn literal(
        &self,
        language: &'static str,
        node: &Value,
        field: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<Value>, Failure> {
        if language != JSONATA {
            return Ok(Some(node.clone()));
        }
        let bindings = self.bindings(input, context, None, None);
        super::super::jsonata::field(node, field, &bindings, &mut self.jsonata_budget())
            .map_err(jsonata_failure)
    }

    /// A FIELD of a state whose value the language requires to be of ONE JSON type:
    /// the field's own expression for a state written in JSONata, held to that type;
    /// the literal the definition wrote otherwise.
    ///
    /// `field` is that field's own place in the definition, which the diagnosis of an
    /// expression that misevaluated names ([`super::super::jsonata::typed`]) — so a
    /// state declaring many such fields says WHICH one the execution failed on. A
    /// literal is carried through unchecked here and answered for by the caller's own
    /// reading of the member, because a literal of the wrong type is something the
    /// definition's own validation refuses rather than an expression that
    /// misevaluated.
    pub(super) fn typed_field(
        &self,
        language: &'static str,
        node: &Value,
        field: &str,
        expected: super::super::jsonata::Kind,
        input: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        if language != JSONATA {
            return Ok(node.clone());
        }
        let bindings = self.bindings(input, context, None, None);
        super::super::jsonata::typed(node, field, expected, &bindings, &mut self.jsonata_budget())
            .map_err(jsonata_failure)
    }

    /// The document a `Task` state hands its work.
    ///
    /// "Parameters ... Used to create a collection of key-value pairs that will be
    /// passed as input" — the member is `Arguments` in JSONata and `Parameters` in
    /// JSONPath, and a state that declares neither hands its work its own effective
    /// input.
    pub(super) fn task_request(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match language {
            JSONATA => match state.get(ARGUMENTS_MEMBER) {
                Some(template) => {
                    let bindings = self.bindings(effective, context, None, None);
                    let built = super::super::jsonata::render(
                        template,
                        ARGUMENTS_MEMBER,
                        &bindings,
                        &mut self.jsonata_budget(),
                    )
                    .map_err(jsonata_failure)?;
                    // `Arguments` is the DOCUMENT the work is handed — "a collection of
                    // key-value pairs" — so a whole-field expression that produced a
                    // value of any other kind names no request for the call to make.
                    // That is this state's own refusal rather than the expression
                    // misevaluating: the expression produced a value, and the member
                    // cannot carry it, which is what the service's own sentence for
                    // such a request says ([`request_document_unusable`]).
                    if !built.is_object() {
                        return Err(runtime(request_document_unusable(
                            ARGUMENTS_MEMBER,
                            &built,
                        )));
                    }
                    Ok(built)
                }
                None => Ok(effective.clone()),
            },
            _ => match state.get("Parameters") {
                Some(template) => payload_template(template, effective, context),
                None => Ok(effective.clone()),
            },
        }
    }

    /// The ROLE a `Task` state asks its work to be made under, from its `Credentials`
    /// member; `None` for a state that declares none, whose work is made as the state
    /// machine's own execution role.
    ///
    /// "Credentials … Specify a target role that the state machine's execution role can
    /// assume" — the member carries one field, `RoleArn`, and that field is written in the
    /// state's own query language: a literal ARN, a `RoleArn.$` reference path or
    /// intrinsic in JSONPath, and a JSONata expression in the other. So the member is
    /// resolved exactly as the document the work is handed is ([`Interpreter::task_request`]):
    /// the whole object goes through that language's own template, and what comes out has
    /// to be an object carrying a string `RoleArn` — anything else names no role the work
    /// could be made under.
    ///
    /// Resolving it is the INTERPRETER's own step, taken before anything is handed over:
    /// what the role then DECIDES — whether the execution role may assume it, and whether
    /// that role may make the call — belongs to the task seam
    /// ([`super::super::task_authorization::TaskRoles`]).
    pub(super) fn task_role_arn(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Option<String>, Failure> {
        let Some(declared) = state.get(CREDENTIALS_MEMBER) else {
            return Ok(None);
        };
        let resolved = match language {
            JSONATA => {
                let bindings = self.bindings(effective, context, None, None);
                super::super::jsonata::render(
                    declared,
                    CREDENTIALS_MEMBER,
                    &bindings,
                    &mut self.jsonata_budget(),
                )
                .map_err(jsonata_failure)?
            }
            _ => payload_template(declared, effective, context)?,
        };
        let Some(role) = resolved.get(ROLE_ARN_FIELD) else {
            return Err(runtime(format!(
                "'{CREDENTIALS_MEMBER}' carries no '{ROLE_ARN_FIELD}', so it names no role the \
                 work of this state could be made under"
            )));
        };
        match role {
            Value::String(arn) if !arn.is_empty() => Ok(Some(arn.clone())),
            other => Err(self.invalid_field_value(
                language,
                declared.get(ROLE_ARN_FIELD),
                &format!("{CREDENTIALS_MEMBER}.{ROLE_ARN_FIELD}"),
                "the ARN of a role",
                other,
            )),
        }
    }

    /// A `Task` state whose `Resource` names an ACTIVITY: the state SCHEDULES a task
    /// for that activity and the walk suspends, because what happens next is a
    /// worker's reply.
    ///
    /// "An activity is a worker — a program — that you host", which "polls Step
    /// Functions using the GetActivityTask API action and responds using SendTask*
    /// API actions": the task carries the state's effective input, and the state's
    /// own `TimeoutSeconds`/`HeartbeatSeconds` bound how long it may stay
    /// outstanding. A `Resource` naming any OTHER work is the service-integration
    /// call [`Interpreter::task_state`] makes, which is what hands this one the
    /// states it is for.
    ///
    /// A walk CONTINUING from a worker's reply to this state's task applies that reply
    /// here instead of suspending: the reply is the state's result, so the state's own
    /// result-side data flow places it and the walk goes on to its `Next`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn activity_task_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let resource = task_resource(state);
        if !super::super::names_an_activity(resource) {
            return Err(runtime(format!(
                "state '{name}' invokes '{}', a service integration this emulator does not make, \
                 so nothing was run for it",
                echoed(resource)
            ))
            .at(at));
        }
        // A `Map` ITERATION is walked to its own end inside the enclosing state's turn,
        // and what the Map Run around it reports — the items it has finished, the child
        // execution each item ran as — is counted as those iterations finish, which an
        // iteration waiting for a worker has not done. So a task scheduled inside one is
        // not run, rather than run as though a worker had already answered; a `Parallel`
        // BRANCH suspends, because a branch reports nothing until it ends, and the frames
        // the suspension records are what the continuation re-enters it through
        // ([`ParkedFrame`]).
        if self.map_depth > 0 {
            return Err(runtime(format!(
                "state '{name}' schedules an activity task inside a 'Map' iteration, which this \
                 emulator does not wait at, so nothing was run for it"
            ))
            .at(at));
        }
        // One execution waits at ONE unit of work whose answer comes from outside it:
        // the record that holds it names the state it belongs to, and the answer that
        // resolves it resumes that one state. So a second concurrent suspension — a
        // sibling branch of the same `Parallel` reaching a task or a job of its own — is
        // not scheduled either. The same question the run-a-job path and the callback
        // path ask ([`Interpreter::suspended_in_a_branch`]).
        if self.suspended_in_a_branch() {
            return Err(runtime(format!(
                "state '{name}' schedules a second activity task while this execution is \
                 already waiting at one, which this emulator does not wait at, so nothing was \
                 run for it"
            ))
            .at(at));
        }
        // Both bounds are read from the state's effective INPUT, which is what their
        // `*Path` forms are reference paths into — not from the `Parameters` document
        // built below.
        let timeout_seconds = match self
            .task_seconds(state, language, "TimeoutSeconds", &effective, context)
        {
            Ok(seconds) => seconds,
            Err(failure) => {
                return self.caught_field_failure(
                    name, state, language, raw, &effective, failure, context, at,
                )
            }
        };
        let heartbeat_seconds = match self
            .task_seconds(state, language, "HeartbeatSeconds", &effective, context)
        {
            Ok(seconds) => seconds,
            Err(failure) => {
                return self.caught_field_failure(
                    name, state, language, raw, &effective, failure, context, at,
                )
            }
        };
        // "Parameters ... Used to create a collection of key-value pairs that will be
        // passed as input" — `Arguments` in JSONata: the document the template builds
        // IS what the worker is handed, and without one the state's effective input
        // is.
        let input = match self.task_request(state, language, &effective, context) {
            Ok(input) => input,
            Err(failure) => {
                return self.caught_field_failure(
                    name, state, language, raw, &effective, failure, context, at,
                )
            }
        };
        // "GetActivityTask ... used by workers to retrieve a task (with the specified
        // activity ARN) which has been scheduled for execution by a running state
        // machine": the task is queued UNDER the activity, so a `Resource` naming one
        // this emulator does not hold names no queue a worker could ever poll. Nothing
        // is scheduled for it and the state fails, rather than the execution waiting
        // for a worker that can never be handed the task. Asked once the state's own
        // members have been read, so a definition whose task could not be BUILT reports
        // that instead.
        if !self.tasks.activity(resource) {
            return Err(runtime(super::activity_does_not_exist(resource)).at(at));
        }
        self.push_entered("TaskStateEntered", name, at, raw);
        self.push("ActivityScheduled", name, at);
        // Something has already CLOSED this state's task, so the walk applies that
        // outcome here instead of suspending again: a worker's `SendTaskSuccess` is the
        // task's result — "the JSON output of the task" — and a `SendTaskFailure`, or the
        // closure of a bound the state declared, is the task's failure. The steps up to
        // here are what the call that suspended the walk already recorded, which is what
        // [`Resumed::index`] marks.
        let succeeded = match &self.reply {
            Some(reply) if reply.state_name == name => self.reply.take(),
            _ => None,
        };
        let closed = match &self.callback_reply {
            Some(reply) if reply.state_name == name => self.callback_reply.take(),
            _ => None,
        };
        let answered = succeeded
            .map(|reply| (Ok(reply.result), false))
            .or_else(|| {
                closed.map(|reply| {
                    // The names the closure this reply reports answers to travel with the
                    // failure itself, so the state's retriers and its catchers are matched
                    // against the same set ([`Failure::also_answering`]).
                    let outcome = reply
                        .outcome
                        .map_err(|failure| failure.also_answering(reply.also_answers));
                    (outcome, reply.by_the_clock)
                })
            });
        if let Some((outcome, by_the_clock)) = answered {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            let result = match outcome {
                Ok(result) => result,
                // The failure is the `Task` state's own, so the state answers for it
                // exactly as it answers for a failed integration call: the FIRST catcher
                // of its `Catch` that names the error takes the execution on at its
                // `Next` with the error output, and with none the failure is the
                // execution's ([`Interpreter::catch_or_fail`]).
                Err(failure) => {
                    self.push_detailed(
                        activity_closure(by_the_clock),
                        name,
                        at,
                        super::super::EventDetails::of_failure(
                            failure.error.clone(),
                            failure.cause.clone(),
                        ),
                    );
                    return self.catch_or_fail(
                        (name, "TaskStateExited"),
                        state,
                        language,
                        raw,
                        &effective,
                        failure,
                        context,
                        at,
                    );
                }
            };
            self.push("ActivitySucceeded", name, at);
            let value = self
                .produced(state, language, raw, &effective, Some(result), context, true)
                .map_err(|failure| failure.at(at))?;
            self.push_exited("TaskStateExited", name, at, &value);
            let next = self.next_of(state).map_err(|failure| failure.at(at))?;
            return Ok(StateStep { value, at, next });
        }
        self.waiting = Some(ScheduledActivity {
            activity_arn: Some(resource.to_string()),
            // An activity's task is queued for a worker to claim, and the token that
            // names it is minted when the store queues it.
            token: None,
            state_name: name.to_string(),
            input: self.echo(&input),
            raw_input: raw.clone(),
            at,
            timeout_seconds,
            heartbeat_seconds,
            // An activity's task is answered by a worker rather than by a walk, so
            // its retriers are spent by the store ([`ActivityTask::retries_taken`])
            // and this record starts the first attempt with none spent.
            retries_taken: Vec::new(),
            // The concurrent states this task is nested inside are contributed by the
            // states that enclose it, as the walk leaves each of them
            // ([`Interpreter::run_branches`]).
            frames: Vec::new(),
        });
        Ok(StateStep {
            value: input,
            at,
            next: Next::Suspended,
        })
    }

    /// A `Task` state's `TimeoutSeconds` or `HeartbeatSeconds`, from the literal
    /// member or from the `<member>Path` reference path into the state's input.
    ///
    /// Both are "a positive integer" of seconds, so a value that is not one is a
    /// defect in the definition rather than a bound to be guessed at. It is reported
    /// under the member the DEFINITION wrote — `HeartbeatSecondsPath` where the
    /// definition carries that — because the two forms are alternatives and naming the
    /// one it does not carry points a reader at the wrong field.
    ///
    /// A value a `*Path` member selected that is not a whole number at all cannot be
    /// read as the integer the member holds, which is a different failure from a whole
    /// number the member does not allow: it is reported as the parse failure it is,
    /// naming the reference path the definition wrote and the value that path selected
    /// ([`parameter_not_a_long`]).
    pub(super) fn task_seconds(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<u64>, Failure> {
        let path_member = format!("{member}Path");
        let (wrote, reference, declared) = match (state.get(member), state.get(path_member.as_str()))
        {
            (Some(declared), _) => (
                member.to_string(),
                None,
                self.literal(language, declared, member, input, context)?,
            ),
            (None, Some(Value::String(path))) => (
                path_member.clone(),
                Some(path.clone()),
                Some(member_path_value(&path_member, path, input, context).map_err(runtime)?),
            ),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )))
            }
            (None, None) => return Ok(None),
        };
        match declared.as_ref().and_then(Value::as_u64) {
            Some(seconds) if seconds > 0 => Ok(Some(seconds)),
            _ => {
                let value = declared.unwrap_or(Value::Null);
                // A whole number the member cannot take — zero, a negative one — WAS
                // read as the integer the member holds, so what is reported is the
                // value the member does not allow; every other value could not be read
                // as that integer at all.
                match reference.filter(|_| !is_whole_number(&value)) {
                    Some(path) => Err(runtime(parameter_not_a_long(&wrote, &path, &value))),
                    None => Err(self.invalid_field_value(
                        language,
                        state.get(wrote.as_str()),
                        &wrote,
                        POSITIVE_SECONDS,
                        &value,
                    )),
                }
            }
        }
    }

    /// What a `Task` state answers a FIELD of its own it could not read with: the
    /// state's own `Catch`, exactly as it answers a failure of its work with.
    ///
    /// A field is read while the state is being EXECUTED, so a field that could not be
    /// read is that STATE's error and the state's own error handling answers for it: an
    /// expression that misevaluated in one ([`QUERY_EVALUATION_ERROR`]) is as ordinary
    /// an error of the state as the failure of the work the field would have bounded,
    /// and only an error no catcher names ends the execution. [`RUNTIME_ERROR`] is
    /// carried through unanswered whatever the state declares, because no retrier or
    /// catcher matches it ([`errors_match`]).
    ///
    /// The state is ENTERED first, because it was: the event a catcher leaves it by
    /// pairs with the one it was entered at, which is also the event the diagnosis names.
    ///
    /// It is the `Catch` half alone that answers here, because this is the path of a task
    /// an activity's WORKER answers: such a task's retriers are spent by the store as
    /// each attempt's task closes ([`ActivityTask::retries_taken`]), so the
    /// walk runs no retry loop around it to re-run the field read in. The task whose work
    /// this emulator performs itself reads its fields INSIDE that loop
    /// ([`Interpreter::task_state`]), where a `Retry` answers for them as well.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn caught_field_failure(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        failure: Failure,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        self.push_entered("TaskStateEntered", name, at, raw);
        self.catch_or_fail(
            (name, "TaskStateExited"),
            state,
            language,
            raw,
            effective,
            failure,
            context,
            at,
        )
    }

    /// What a FIELD of a state whose value is of a kind the language does not accept
    /// there reports: the field, what it must carry, and the value found.
    ///
    /// A JSONata field's value is what the expression written IN it produced, so a
    /// value the field cannot take is that expression misevaluating — the language's own
    /// query-evaluation failure, diagnosed under the field it is written in, which a
    /// definition's `Retry` and `Catch` may answer for by that name
    /// ([`super::super::jsonata::invalid_value`]). A field carrying a literal, and one
    /// a JSONPath `*Path` read out of the state's data, is reported under the member the
    /// definition wrote ([`referenced_value_must_be`]).
    pub(super) fn invalid_field_value(
        &self,
        language: &'static str,
        node: Option<&Value>,
        field: &str,
        must_be: &str,
        value: &Value,
    ) -> Failure {
        if language == JSONATA {
            if let Some(refusal) = node
                .and_then(|node| super::super::jsonata::invalid_value(node, field, must_be, value))
            {
                return jsonata_failure(refusal);
            }
        }
        runtime(referenced_value_must_be(field, must_be, value))
    }

    /// What a FIELD of a state whose value is of the kind the language accepts there
    /// but which the field could not READ reports: `diagnosis`, the service's own
    /// sentence about that field.
    ///
    /// It is the same two classes [`Interpreter::invalid_field_value`] answers in — a
    /// JSONata field's failure is the language's own query-evaluation failure, located
    /// at the field the expression is written in, and every other field's is the
    /// service's run-time failure — and the DIAGNOSIS is the one sentence either way,
    /// because what could not be read is the same thing whichever language the state is
    /// written in.
    pub(super) fn unreadable_field_value(
        &self,
        language: &'static str,
        node: Option<&Value>,
        field: &str,
        diagnosis: String,
    ) -> Failure {
        if language == JSONATA {
            if let Some(refusal) = node.and_then(|node| {
                super::super::jsonata::unreadable_value(node, field, diagnosis.clone())
            }) {
                return jsonata_failure(refusal);
            }
        }
        runtime(diagnosis)
    }

    /// The context object (`$$`) a state reads: the run's own members, plus the state
    /// it is being read in and the `Map` item being iterated, when there is one.
    ///
    /// `$$.State` carries the members the service supplies for the state being run: its
    /// name, the instant it was entered, and — for a state kind that CAN be retried —
    /// the attempt it is on. RECORDED shape: `RetryCount` is a member of the context
    /// object of a state whose type may declare a `Retry` ([`executable_members`] is
    /// which those are: the types the Amazon States Language gives that field), present
    /// on every attempt of one and zero on the first, which has been retried no times;
    /// a state that cannot be retried at all carries no such member, so the object a
    /// state of that kind resolves — and the one a diagnosis about an unresolved context
    /// path quotes — is its name and its entry instant alone. [`with_retry_count`]
    /// writes the attempt a `Retry` re-ran.
    pub(super) fn context_for(
        &self,
        scope: &Scope,
        name: &str,
        state_type: &str,
        at: Duration,
    ) -> Value {
        let mut context = self.base_context.clone();
        if let Some(object) = context.as_object_mut() {
            let mut state = Map::new();
            state.insert("Name".to_string(), serde_json::json!(name));
            state.insert(
                "EnteredTime".to_string(),
                // Through the checked helper every other instant of this walk is built
                // with ([`super::super::after`]): the offset is accumulated from a
                // caller's own `Wait` states, so one no instant can represent holds at
                // the furthest instant this clock carries instead of panicking.
                serde_json::json!(iso(super::super::after(self.start, at))),
            );
            if can_retry(state_type) {
                state.insert("RetryCount".to_string(), serde_json::json!(self.retry_count));
            }
            object.insert("State".to_string(), Value::Object(state));
            if let Some(item) = &scope.map_item {
                object.insert("Map".to_string(), serde_json::json!({ "Item": item }));
            }
        }
        context
    }

    /// The context object (`$$`) the states of ONE child workflow execution read:
    /// this walk's own, with the `Execution` members of the CHILD.
    ///
    /// A Distributed Map's item runs as an execution in its own right, so the states
    /// inside it read that execution's identity — its ARN, its name, the input it was
    /// started with and the instant it started — rather than the parent's. The state
    /// machine stays the parent's: a child "runs the same state machine" the Map Run
    /// reports, which is what `ListMapRuns` names in `stateMachineArn`. It has been
    /// redriven no times, because it has just been started.
    pub(super) fn child_context(
        &self,
        arn: &str,
        name: &str,
        input: &Value,
        at: Duration,
    ) -> Value {
        let mut context = self.base_context.clone();
        if let Some(execution) = context.get_mut("Execution").and_then(Value::as_object_mut) {
            execution.insert("Id".to_string(), serde_json::json!(arn));
            execution.insert("Name".to_string(), serde_json::json!(name));
            execution.insert("Input".to_string(), input.clone());
            execution.insert(
                "StartTime".to_string(),
                serde_json::json!(iso(super::super::after(self.start, at))),
            );
            execution.insert("RedriveCount".to_string(), serde_json::json!(0));
        }
        context
    }

    /// A fresh name for one child workflow execution.
    ///
    /// Step Functions names a child workflow execution it starts for an item, so the
    /// name is drawn here, as a version-4 UUID — the same form, from the same
    /// generator, as the name the store mints for a `StartExecution` that gave none.
    /// The counter is what makes two draws in the same nanosecond differ.
    pub(super) fn fresh_child_name(&mut self) -> String {
        self.child_counter = self.child_counter.wrapping_add(1);
        let mut seed = super::super::now_nanos() ^ self.child_counter.rotate_left(29);
        let high = super::super::splitmix64(&mut seed);
        let low = super::super::splitmix64(&mut seed);
        super::super::uuid_v4(high, low)
    }

    /// A fresh `taskToken` for one callback (`.waitForTaskToken`) task of this walk.
    ///
    /// "Task tokens are generated by Step Functions when tasks are assigned to a
    /// worker, or in the context object when a workflow enters a task state": the
    /// token of a callback task is read out of the context object by the state's own
    /// parameters and leaves with the integration call, so it has to exist BEFORE
    /// that call is made and is drawn here rather than when the store records the
    /// task. It is built exactly as the token of an activity's task is
    /// ([`crate::state::task_token`]), over the execution this walk runs — which is
    /// what the token names for a task that belongs to no activity — and the id is
    /// drawn from the same generator a child execution's name is.
    pub(super) fn fresh_task_token(&mut self) -> String {
        self.child_counter = self.child_counter.wrapping_add(1);
        let mut seed = super::super::now_nanos() ^ self.child_counter.rotate_left(17);
        let high = super::super::splitmix64(&mut seed);
        let low = super::super::splitmix64(&mut seed);
        super::super::task_token(&super::super::uuid_v4(high, low), &self.execution_arn)
    }

    pub(super) fn push(&mut self, event_type: &'static str, state_name: &str, after: Duration) {
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::super::EventDetails::default(),
        );
    }

    /// The event a state records when it is ENTERED, carrying the data that state was
    /// entered WITH: "the string that contains the JSON input data for the state",
    /// which is the raw input the state received, before its own `InputPath` selects
    /// from it.
    ///
    /// A state this walk RE-RUNS from its entry records none: "Step Functions preserves
    /// the results and execution history of the successful steps", so the entry the
    /// attempt this walk continues recorded is the one event of it the history holds
    /// ([`ContinuedEntry`]).
    pub(super) fn push_entered(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        input: &Value,
    ) {
        if std::mem::take(&mut self.skipping_entry) {
            return;
        }
        let input = self.echo(input);
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::super::EventDetails {
                input: Some(input),
                ..super::super::EventDetails::default()
            },
        );
    }

    /// The event a state records when it is LEFT, carrying the data that state EMITTED:
    /// "the JSON output data of the state", which is the document the state's own
    /// result-side data flow produced — `ResultSelector`, then `ResultPath`, then
    /// `OutputPath`, or a JSONata state's `Output` — and is the next state's input,
    /// beside the variables that are in scope as it leaves.
    pub(super) fn push_exited(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        output: &Value,
    ) {
        let output = self.echo(output);
        // "Map of variable name and value as a serialized JSON representation": what
        // the state being left ASSIGNED, which is what a caller reads a JSONata
        // state's `Assign` back through. A state that assigned none reports no map at
        // all, whatever the scope around it holds.
        let assigned_variables = std::mem::take(&mut self.assigned);
        self.push_detailed(
            event_type,
            state_name,
            after,
            super::super::EventDetails {
                output: Some(output),
                assigned_variables,
                ..super::super::EventDetails::default()
            },
        );
    }

    /// One history event carrying what the walk knew about it.
    ///
    /// Every event spends one of the execution's own allowance
    /// ([`Interpreter::events_left`]): the history one execution keeps is bounded, so a
    /// run that has filled it records no further event and makes no further transition
    /// — the next one ends it with [`history_limit_exceeded`], which is the event the
    /// history ends on.
    ///
    /// An event the full history drops is therefore always followed by that failure and
    /// never by an outcome of the definition's own: the walk makes no further
    /// transition, so the last state's turn can be half-recorded (an entry whose exit
    /// was dropped) but the run it ends is the quota's failure and not a `SUCCEEDED`
    /// one.
    pub(super) fn push_detailed(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        details: super::super::EventDetails,
    ) {
        if self.events_left == 0 {
            return;
        }
        self.events_left -= 1;
        // What a child workflow execution records is counted and not held: the count is
        // what numbers its next event, and the parent's list is not where its steps go
        // ([`Interpreter::unheld`]).
        if let Some(counted) = &mut self.unheld {
            *counted = counted.saturating_add(1);
            return;
        }
        self.steps.push(Step {
            event_type,
            state_name: Some(state_name.to_string()),
            after,
            terminal: false,
            details,
        });
    }

    /// Record on the event that CLOSED a `Task` state's attempt how long the call that
    /// attempt made really took, for a step that is such an event.
    ///
    /// The span is the work's own duration — the walk was blocked in the call for it —
    /// and it is the span the series that measure a task's run report
    /// ([`super::super::observability`]). It is kept on the EVENT because that is where
    /// the walk's knowledge of the call ends: the walk's offsets are a logical clock no
    /// call advances, and an attempt that was not closed by a call (one whose work was
    /// never handed over, or one that handed it over and is waiting) carries no span at
    /// all.
    pub(super) fn stamp_work_span(&mut self, spent: Duration) {
        // A child workflow execution's events are not kept, so the walk's last event is
        // the enclosing execution's and not this work's ([`Interpreter::unheld`]).
        if self.unheld.is_some() {
            return;
        }
        let Some(step) = self.steps.last_mut() else {
            return;
        };
        if !super::super::observability::closes_task_work(step.event_type) {
            return;
        }
        step.details.work_millis = Some(u64::try_from(spent.as_millis()).unwrap_or(u64::MAX));
    }

    /// The event one attempt of a `Task` state's work failed with, in the FAMILY that
    /// state's `Resource` records its work under ([`WorkEvents`]): the family's own
    /// timeout event where the work ran out of the time the state allowed it, and its
    /// own failure event otherwise, both carrying the work that failed and the error and
    /// cause it failed with.
    pub(super) fn push_task_failure(
        &mut self,
        state_name: &str,
        work: &WorkEvents,
        failure: &Failure,
        after: Duration,
    ) {
        let mut details = work.naming();
        details.error = failure.error.clone();
        details.cause = failure.cause.clone();
        self.push_detailed(work.failed(failure), state_name, after, details);
    }

    /// The outcome of a callback (`.waitForTaskToken`) task that its token's ANSWER
    /// decided, with the event that closes the task.
    ///
    /// One statement of it, because the answer reaches the walk two ways and decides the
    /// state the same way either way: a `SendTask*` that arrived while the call carrying
    /// the token was still in flight is read when that call comes back
    /// ([`super::super::integration::Tasks::settle_handover`]), and one that arrived
    /// after the walk suspended begins a walk of its own, which consumes the reply the
    /// continuation was planned from. A success is the state's result, measured
    /// against the ceiling every state's document is measured against; a failure is the
    /// task's own, which the state's `Retry` and `Catch` answer for.
    pub(super) fn answered_callback(
        &mut self,
        state_name: &str,
        work: &WorkEvents,
        at: Duration,
        outcome: Result<Value, Failure>,
    ) -> Result<Value, Failure> {
        match outcome {
            Ok(result) => {
                let mut details = work.naming();
                details.output = Some(self.echo(&result));
                self.push_detailed(work.succeeded(), state_name, at, details);
                measure_payload(&result, work.produced_by()).map(|_bytes| result)
            }
            Err(failure) => {
                self.push_task_failure(state_name, work, &failure, at);
                Err(failure)
            }
        }
    }

    /// What the job a run-a-job task started ENDED as, placed as that state's own
    /// outcome.
    ///
    /// The job is the task's unit of work, so a job that did its work makes the
    /// task succeed with the report the invoked service gave of it — placed by the
    /// state's own result-side data flow, exactly as a request-response call's
    /// response is — and a job that ended without doing it makes the task fail, which
    /// the state's `Catch` then answers for.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finished_job(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        work: &WorkEvents,
        outcome: Result<Value, Failure>,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let outcome = outcome
            .and_then(|result| measure_payload(&result, work.produced_by()).map(|_bytes| result));
        match outcome {
            Ok(result) => {
                let mut details = work.naming();
                details.output = Some(result.to_string());
                self.push_detailed(work.succeeded(), name, at, details);
                let value = self
                    .produced(state, language, raw, effective, Some(result), context, true)
                    .map_err(|failure| failure.at(at))?;
                self.push_exited("TaskStateExited", name, at, &value);
                let next = self.next_of(state).map_err(|failure| failure.at(at))?;
                Ok(StateStep { value, at, next })
            }
            Err(failure) => {
                self.push_task_failure(name, work, &failure, at);
                self.catch_or_fail(
                    (name, "TaskStateExited"),
                    state,
                    language,
                    raw,
                    effective,
                    failure,
                    context,
                    at,
                )
            }
        }
    }

    /// A `Task` state's failure, recorded on the state that produced it and then
    /// reported to the walk.
    /// The failure of a task is the TASK's, so the history attributes it to the work
    /// that failed — the error name and the cause, on the state that named it —
    /// whatever the walk then does with it: a caller reads what went wrong off the
    /// task rather than only off the execution the task ended.
    pub(super) fn task_fault(
        &mut self,
        state_name: &str,
        work: &WorkEvents,
        failure: Failure,
        after: Duration,
    ) -> Fault {
        self.push_task_failure(state_name, work, &failure, after);
        failure.at(after)
    }

    /// Where the walk goes after a state that carries `Next` or `"End": true`.
    ///
    /// A state carrying neither decided no next state, which is the same failure to
    /// transition an unmatched `Choice` without a `Default` is ([`NO_NEXT_STATE`]), so
    /// it is reported with the same diagnosis — the state names itself around it.
    pub(super) fn next_of(&self, state: &Map<String, Value>) -> Result<Next, Failure> {
        if state.get("End").and_then(Value::as_bool) == Some(true) {
            return Ok(Next::EndBranch);
        }
        match state.get("Next").and_then(Value::as_str) {
            Some(next) => Ok(Next::Named(next.to_string())),
            None => Err(runtime(NO_NEXT_STATE.to_string())),
        }
    }

    // -----------------------------------------------------------------------
    // Data flow: InputPath -> Parameters -> work -> ResultSelector -> ResultPath
    //            -> OutputPath
    // -----------------------------------------------------------------------

    /// The state's EFFECTIVE input: `InputPath` applied to the raw input.
    pub(super) fn input_value(
        &self,
        state: &Map<String, Value>,
        raw: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match state.get("InputPath") {
            None => Ok(raw.clone()),
            // "If the value of InputPath is null, that means that the raw input is
            // discarded, and the effective input for the state is an empty JSON
            // object, {}."
            Some(Value::Null) => Ok(serde_json::json!({})),
            Some(Value::String(path)) => select(path, raw, context),
            Some(other) => Err(runtime(format!(
                "'InputPath' must be a JSONPath string or null, not {other}"
            ))),
        }
    }

    /// `OutputPath` applied to a state's result.
    pub(super) fn output_value(
        &self,
        state: &Map<String, Value>,
        value: Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match state.get("OutputPath") {
            None => Ok(value),
            // "If the value of OutputPath is null, that means the input and result
            // are discarded, and the effective output from the state is an empty
            // JSON object, {}."
            Some(Value::Null) => Ok(serde_json::json!({})),
            Some(Value::String(path)) => select(path, &value, context),
            Some(other) => Err(runtime(format!(
                "'OutputPath' must be a JSONPath string or null, not {other}"
            ))),
        }
    }

    /// `ResultSelector`, then `ResultPath` against the state's RAW input, then
    /// `OutputPath`: the three steps that turn a state's result into its output.
    pub(super) fn place_result(
        &self,
        state: &Map<String, Value>,
        raw: &Value,
        result: Value,
        context: &Value,
        with_selector: bool,
    ) -> Result<Value, Failure> {
        let result = select_result(state, result, context, with_selector)?;
        let placed = path_result(state, raw, result)?;
        self.output_value(state, placed, context)
    }

    /// A `Fail` state's `Error`/`Cause`, from the literal member or from its `*Path`.
    ///
    /// In JSONata the member itself carries the expression — "you can use JSONata
    /// expressions in Error and Cause" — so there is no `*Path` form to read.
    ///
    /// A `*Path` member carries "a reference path or an intrinsic function", and it is a
    /// NAMED FIELD of the state, so a path it writes that selects no value is reported
    /// the way every other named field's unresolvable path is: the field the path was
    /// written for beside the WHOLE document it was looked for in
    /// ([`template_expression`]), which is what points a reader at the member to
    /// correct. The state names itself around the diagnosis ([`names_its_state`]).
    pub(super) fn fail_member(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        path_member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<String>, Failure> {
        if let Some(literal) = state.get(member) {
            // `Error` and `Cause` are OPTIONAL members, so one written as JSON null
            // declares no value for the state to fail with: null is the absence of a
            // value rather than a value of the wrong type, and the execution surfaces
            // the same absent error an omitted member does.
            if literal.is_null() {
                return Ok(None);
            }
            // The member is a string in both languages, so a JSONata expression
            // written in it is held to that type and diagnosed where it produced
            // something else or nothing at all.
            let evaluated = self.typed_field(
                language,
                literal,
                member,
                super::super::jsonata::Kind::String,
                input,
                context,
            )?;
            let text = evaluated.as_str().map(str::to_string).ok_or_else(|| {
                runtime(referenced_value_must_be_a_string(member, &evaluated))
            })?;
            return Ok(Some(text));
        }
        match state.get(path_member) {
            None => Ok(None),
            Some(Value::String(path)) => {
                let value = template_expression(path_member, path, input, context)?;
                let text = value.as_str().map(str::to_string).ok_or_else(|| {
                    runtime(referenced_value_must_be_a_string(path_member, &value))
                })?;
                Ok(Some(text))
            }
            Some(other) => Err(runtime(format!(
                "'{path_member}' must be a JSONPath string, not {other}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Wait
    // -----------------------------------------------------------------------

    /// What the `Wait` state `name` still delays the execution by, out of the `declared`
    /// span it asks for: NONE for the one this walk is continuing from, whose span has
    /// already run out, and the whole of it for every other.
    ///
    /// The continued state is taken as it is reached, so a definition that comes back
    /// round to the same `Wait` delays the execution by it again. What makes the span zero
    /// for that one state is what keeps the run's clock where the wait left it: the state
    /// is left at the instant it ran out, and the states after it — and the outcome they
    /// reach — are stamped from there.
    fn span_still_ahead(&mut self, name: &str, declared: Duration) -> Duration {
        if self.elapsed_wait.as_deref() == Some(name) {
            self.elapsed_wait = None;
            return Duration::ZERO;
        }
        declared
    }

    /// The suspension a `Wait` state of `waited` at the offset `at` makes of this walk,
    /// and `None` for a wait this walk runs through.
    ///
    /// A walk suspends at a wait that has any span left to run, so the states after it
    /// are run by the continuation the clock starts ([`super::ParkedWait`]) rather than
    /// here. Four things keep a wait from suspending one:
    ///
    /// * a span of NO time, which delays nothing — the state this walk CONTINUES from
    ///   has one ([`Interpreter::span_still_ahead`]), which is what it was started for;
    /// * a SYNCHRONOUS run, which answers the whole outcome in its own call and waits
    ///   the spans out there ([`Interpreter::parks_at_waits`]);
    /// * this walk being a CONCURRENT unit's — one `Map` iteration's or one `Parallel`
    ///   branch's, or a Distributed Map child's own run — whose states share the
    ///   enclosing run's clock, so a span inside one is accumulated as before;
    /// * a definition-level `TimeoutSeconds` that runs out before the wait does, which
    ///   ENDS the execution at that instant: the run is the timeout's and there is
    ///   nothing after the wait to run.
    fn park_at_wait(
        &mut self,
        name: &str,
        raw: &Value,
        at: Duration,
        waited: Duration,
    ) -> Option<super::ParkedWait> {
        let ends_at = at.saturating_add(waited);
        let parks = waited > Duration::ZERO
            && self.parks_at_waits
            && self.depth == 0
            && self.unheld.is_none()
            && self.parked_wait.is_none()
            && self.timeout_left.is_none_or(|left| ends_at <= left);
        parks.then(|| super::ParkedWait {
            state_name: name.to_string(),
            raw_input: raw.clone(),
            at: ends_at,
        })
    }

    /// How long a `Wait` state waits.
    ///
    /// `Seconds` is the caller's own figure and keeps its value; `Timestamp` is an
    /// instant, so the wait is however much of it is still ahead of the run. Each has
    /// a `*Path` form that reads the figure from the state's input instead.
    ///
    /// A member that could not be read is reported under THE MEMBER THE DEFINITION
    /// WROTE — `SecondsPath` where the definition carries a `SecondsPath` — because the
    /// member name is what points a reader at the field to correct, and a definition
    /// carrying one of the pair does not carry the other.
    ///
    /// Each member is a FIELD of one type ([`Interpreter::typed_field`]): a JSONata
    /// expression written in it is held to that type and diagnosed under the
    /// language's own query-evaluation failure, while the figure the member carries
    /// being out of range is this state's own refusal.
    pub(super) fn wait_duration(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        input: &Value,
        context: &Value,
        at: Duration,
    ) -> Result<Duration, Failure> {
        let (member, seconds) = match (state.get("Seconds"), state.get("SecondsPath")) {
            (Some(seconds), _) => (
                "Seconds",
                Some(self.typed_field(
                    language,
                    seconds,
                    "Seconds",
                    super::super::jsonata::Kind::Number,
                    input,
                    context,
                )?),
            ),
            (None, Some(Value::String(path))) => (
                "SecondsPath",
                Some(
                    member_path_value("SecondsPath", path, input, context)
                        .map_err(runtime)?,
                ),
            ),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'SecondsPath' must be a JSONPath string, not {other}"
                )));
            }
            (None, None) => ("Seconds", None),
        };
        if let Some(seconds) = seconds {
            let Some(whole) = seconds.as_u64() else {
                return Err(self.invalid_field_value(
                    language,
                    state.get(member),
                    member,
                    WHOLE_SECONDS,
                    &seconds,
                ));
            };
            if whole > MAX_WAIT_SECONDS {
                return Err(runtime(format!(
                    "'{member}' is {whole}, more than the {MAX_WAIT_SECONDS} seconds a 'Wait' \
                     state may wait for"
                )));
            }
            return Ok(Duration::from_secs(whole));
        }

        let (member, timestamp) = match (state.get("Timestamp"), state.get("TimestampPath")) {
            (Some(timestamp), _) => (
                "Timestamp",
                Some(self.typed_field(
                    language,
                    timestamp,
                    "Timestamp",
                    super::super::jsonata::Kind::String,
                    input,
                    context,
                )?),
            ),
            (None, Some(Value::String(path))) => (
                "TimestampPath",
                Some(
                    member_path_value("TimestampPath", path, input, context)
                        .map_err(runtime)?,
                ),
            ),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'TimestampPath' must be a JSONPath string, not {other}"
                )));
            }
            (None, None) => ("Timestamp", None),
        };
        if let Some(timestamp) = timestamp {
            let instant = timestamp.as_str().and_then(|text| {
                iso_millis(text).ok().map(|millis| (text.to_string(), millis))
            });
            let Some((text, due_millis)) = instant else {
                // A value that IS a string and merely denotes no instant could not be
                // READ, which is a different failure from a value of another type and
                // is reported as the parse failure it is
                // ([`field_not_an_instant`]); every other type never named an instant
                // at all.
                return Err(match timestamp.as_str() {
                    Some(written) => self.unreadable_field_value(
                        language,
                        state.get(member),
                        member,
                        field_not_an_instant(member, written),
                    ),
                    None => self.invalid_field_value(
                        language,
                        state.get(member),
                        member,
                        ISO_INSTANT,
                        &timestamp,
                    ),
                });
            };
            let reached = super::super::epoch_millis(self.start)
                .saturating_add(i64::try_from(at.as_millis()).unwrap_or(i64::MAX));
            // A `Timestamp` already behind the run is no wait at all; one further
            // ahead than a `Wait` may wait for is out of range, like an over-maximum
            // `Seconds`, and is NOT silently shortened to the maximum.
            let remaining = u64::try_from(due_millis.saturating_sub(reached)).unwrap_or(0);
            if remaining > MAX_WAIT_SECONDS.saturating_mul(1_000) {
                return Err(runtime(format!(
                    "'{member}' is {text}, further ahead than the {MAX_WAIT_SECONDS} seconds a \
                     'Wait' state may wait for"
                )));
            }
            return Ok(Duration::from_millis(remaining));
        }
        Err(runtime(
            "a 'Wait' state must carry 'Seconds' or 'Timestamp'".to_string(),
        ))
    }

    // -----------------------------------------------------------------------
    // Choice
    // -----------------------------------------------------------------------

    /// The state the first matching choice rule names, or the `Default`.
    ///
    /// Each rule is tested in the language the state is written in: a `Variable` and
    /// one of the comparators in JSONPath, a boolean `Condition` expression in
    /// JSONata. The FIRST rule whose test holds wins, even where a later one's holds
    /// too, and the rule that won is answered beside its `Next` — a JSONata rule's
    /// own `Assign` and `Output` are the state's, and are applied where the state's
    /// own are ([`Interpreter::choice_output`]) rather than before them.
    pub(super) fn choose(
        &mut self,
        state: &Map<String, Value>,
        language: &'static str,
        input: &Value,
        context: &Value,
    ) -> Result<Chosen, Failure> {
        let choices = state
            .get("Choices")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                runtime("a 'Choice' state must carry a non-empty 'Choices' array".to_string())
            })?;
        if choices.is_empty() {
            return Err(runtime(
                "a 'Choice' state must carry a non-empty 'Choices' array".to_string(),
            ));
        }
        // Every rule is evaluated until one matches, so the rule count multiplies the
        // paths one visit to this state evaluates.
        if choices.len() > MAX_CHOICE_RULES {
            return Err(runtime(format!(
                "a 'Choice' state carries {} rules, more than the {MAX_CHOICE_RULES} this service \
                 evaluates",
                choices.len()
            )));
        }
        for (index, rule) in choices.iter().enumerate() {
            let rule = rule.as_object().ok_or_else(|| {
                runtime("every member of 'Choices' must be a JSON object".to_string())
            })?;
            let matched = match language {
                JSONATA => {
                    let condition = rule.get("Condition").ok_or_else(|| {
                        runtime(format!(
                            "a 'Choice' rule written in '{JSONATA}' must carry a 'Condition'"
                        ))
                    })?;
                    let bindings = self.bindings(input, context, None, None);
                    super::super::jsonata::condition(
                        condition,
                        &choice_condition_field(index),
                        &bindings,
                        &mut self.jsonata_budget(),
                    )
                    .map_err(jsonata_failure)?
                }
                _ => condition_matches(rule, input, context)?,
            };
            if !matched {
                continue;
            }
            let next = rule.get("Next").and_then(Value::as_str).ok_or_else(|| {
                runtime("a top-level choice rule must carry a string 'Next'".to_string())
            })?;
            return Ok(Chosen {
                next: next.to_string(),
                // The rule that matched is the one whose own `Assign` applies — and,
                // in JSONata, its `Output` too. Its own place in the definition is
                // where those members are written ([`choice_rule_field`]), which is
                // what a diagnosis about one names.
                rule: Some((index, rule.clone())),
            });
        }
        match state.get("Default").and_then(Value::as_str) {
            Some(default) => Ok(Chosen {
                next: default.to_string(),
                rule: None,
            }),
            // A `Choice` that no rule matched and that declares no `Default` decided
            // no next state, so the execution ends AT it — under the name the
            // interpreter raises every failure of its own under ([`RUNTIME_ERROR`]),
            // which is the name RECORDED for this outcome, with the one diagnosis
            // every state that cannot be left reports ([`NO_NEXT_STATE`]): the state
            // names itself around it, so the cause carries the stuck state and the
            // event it was entered at.
            None => Err(runtime(NO_NEXT_STATE.to_string())),
        }
    }

    /// What a JSONata `Choice` state PUTS OUT, given the rule that was selected.
    ///
    /// "You can use Assign in a Choice rule": the assignments a `Choice` makes are the
    /// SELECTED rule's. RECORDED: a rule matching is what decides it, so the
    /// state-level `Assign` is applied only where NO rule matched and the `Default`
    /// was taken — a matched rule that declares none assigns nothing, rather than
    /// falling back on the state's.
    ///
    /// The `Output` that decides what the state puts out is the selected rule's where it
    /// declares one and the state's otherwise, and a state and rule that declare neither
    /// put the state's own input through.
    ///
    /// The fields are evaluated in the order the language fixes, with `Assign` LAST —
    /// the condition that selected the rule, then the `Output` that applies, then the
    /// `Assign` that applies — so the first of them to fail is the one the failure
    /// names, exactly as for every other state ([`Interpreter::jsonata_output`]).
    ///
    /// As everywhere else, a state whose output cannot be BUILT assigns nothing: the
    /// scope is put back as the state found it.
    pub(super) fn choice_output(
        &mut self,
        state: &Map<String, Value>,
        rule: Option<&(usize, Map<String, Value>)>,
        input: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        let scope = self.variables.clone();
        let assigned = self.assigned.clone();
        let produced = self.choice_output_and_assign(state, rule, input, context);
        if produced.is_err() {
            self.variables = scope;
            self.assigned = assigned;
        }
        produced
    }

    /// [`Interpreter::choice_output`] without the roll-back, which that one adds.
    fn choice_output_and_assign(
        &mut self,
        state: &Map<String, Value>,
        rule: Option<&(usize, Map<String, Value>)>,
        input: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        // ONE allowance for the `Output` that applies and the `Assign` after it: they
        // are the expressions of one visit to this state.
        let mut budget = self.jsonata_budget();
        // The `Output` of whichever decided the transition: the rule that matched where
        // it declares one, and the state itself otherwise. A member of a RULE is written
        // inside that rule, which is where a diagnosis about it locates it.
        let (template, at) = match rule.and_then(|(index, rule)| {
            rule.get(OUTPUT_MEMBER)
                .map(|template| (template.clone(), choice_rule_field(*index, OUTPUT_MEMBER)))
        }) {
            Some(found) => (Some(found.0), found.1),
            None => (
                state.get(OUTPUT_MEMBER).cloned(),
                OUTPUT_MEMBER.to_string(),
            ),
        };
        // Built BEFORE the `Assign` that applies, because `Assign` is the last field a
        // node evaluates: what it writes is in scope for the states after the
        // transition rather than for the field that builds its output.
        let produced = match template {
            None => input.clone(),
            Some(template) => {
                let rendered = {
                    let bindings = self.bindings(input, context, None, None);
                    super::super::jsonata::render(&template, &at, &bindings, &mut budget)
                };
                rendered.map_err(jsonata_failure)?
            }
        };
        // The `Assign` of whichever decided the transition: the rule that matched, or
        // the state itself where none did.
        let (owner, at) = match rule {
            Some((index, rule)) => (rule, choice_rule_field(*index, ASSIGN_MEMBER)),
            None => (state, ASSIGN_MEMBER.to_string()),
        };
        self.jsonata_assign(owner, &at, input, None, None, context, &mut budget)?;
        Ok(produced)
    }

    // -----------------------------------------------------------------------
    // Task
    // -----------------------------------------------------------------------


    /// Run a `Task` state: do the work its `Resource` names, and place what came of
    /// it.
    ///
    /// A `Task` state "represents a single unit of work performed by a state
    /// machine", and the `Resource` says which work. An ACTIVITY belongs to a worker,
    /// so that one is scheduled and the walk suspends at it
    /// ([`Interpreter::activity_task_state`]). Every other `Resource` names a service
    /// INTEGRATION, whose unit of work is one API call: its request is the document
    /// the state's `Parameters` payload template builds over the state's effective
    /// input (the effective input itself when it declares none), and the operation's
    /// response document is the state's RESULT, which the result-side filters then
    /// place. A call that failed is the state's own failure, so the state's `Retry`
    /// and `Catch` apply to it exactly as they apply to a `Map`'s or a `Parallel`'s
    /// work — and what no call is made for is not faked, but reported as the failure
    /// it is.
    ///
    /// `entered` is the id of the event this state records on being entered, which the
    /// refusals that name this state report ([`executing_state`]).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn task_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
        entered: i64,
    ) -> Result<StateStep, Fault> {
        // The activity task is decided FIRST, before this state's own history event:
        // that path records its own, and the work there is a worker's rather than a
        // call this emulator makes.
        if super::super::names_an_activity(task_resource(state)) {
            return self
                .activity_task_state(name, state, language, &raw, effective, context, at);
        }
        self.push_entered("TaskStateEntered", name, at, &raw);
        let resource = state.get("Resource").and_then(Value::as_str).ok_or_else(|| {
            runtime(format!(
                "the 'Task' state '{name}' carries no string 'Resource', so it names no work"
            ))
            .at(at)
        })?;
        // The FAMILY of events this state's work is recorded under, and what those
        // events name the work by, read once from the `Resource` they all describe
        // ([`WorkEvents`]) — chosen here so every event below is asked of it rather than
        // named at each site.
        let work = WorkEvents::of(resource);
        let call = super::super::integration::read(resource).map_err(|why| {
            self.task_fault(name, &work, runtime(format!("state '{name}': {why}")), at)
        })?;
        // The job a run-a-job integration started has ENDED: what it ended as is this
        // state's own outcome, so the state reports it instead of starting the job a
        // second time. The events up to here are the ones the call that parked the
        // walk already recorded, which is what [`Resumed::index`] marks.
        let ended = match &self.job_reply {
            Some(reply) if reply.state_name == name => self.job_reply.take(),
            _ => None,
        };
        if let Some(ended) = ended {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            return self.finished_job(
                name,
                state,
                language,
                &raw,
                &effective,
                &work,
                ended.outcome,
                context,
                at,
            );
        }
        // A `Map` ITERATION is walked to its own end inside the enclosing state's
        // turn, and what the Map Run around it reports — the items it has finished,
        // the child execution each item ran as — is counted as those iterations
        // finish, which an iteration parked at a job has not done. So a run-a-job
        // task inside one is not run, rather than run as though the job had already
        // finished; a `Parallel` BRANCH parks, because a branch reports nothing until
        // it ends ([`ParkedFrame`]).
        if call.job.is_some() && self.map_depth > 0 {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' starts a run-a-job integration inside a 'Map' iteration, \
                     which this emulator does not park at, so nothing was run for it"
                )),
                at,
            ));
        }
        // One execution waits at ONE unit of work whose answer comes from outside it, so
        // a second concurrent suspension is not started: the record that holds it names
        // the state it belongs to, and the frame a `Parallel` contributes names ONE of
        // its branches ([`Interpreter::suspended_in_a_branch`], asked the same way by the
        // activity and callback paths).
        if call.job.is_some() && self.suspended_in_a_branch() {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' starts a run-a-job integration while this execution is \
                     already waiting at a task or a job, which this emulator does not park at, \
                     so nothing was run for it"
                )),
                at,
            ));
        }
        // A walk CONTINUING from a job that has ENDED has not reached the state that
        // job belongs to yet, so a task that would start a job here is one the attempt
        // this continues already started — the state machine has been updated under
        // the execution since. It is not started a second time.
        if call.job.is_some() && self.job_reply.is_some() {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' would start the job of a run-a-job integration again while \
                     this execution is being continued from one that has ended, so nothing was \
                     run for it"
                )),
                at,
            ));
        }
        // The run-a-job pattern is served only for the STANDARD workflow type, so an
        // EXPRESS execution fails AT this state and the integration's call is NOT
        // made: no job is started for a task the workflow type cannot wait at.
        if call.job.is_some() && self.express {
            return Err(self.task_fault(
                name,
                &work,
                express_run_a_job_refusal(name, entered),
                at,
            ));
        }
        // A CALLBACK task is answered from outside the execution, which the walk can
        // only wait for where it is the walk itself that suspends. A `Map` ITERATION
        // cannot: the Map Run around it counts an item as its iteration finishes, and an
        // iteration waiting for a token has not finished, so such a state is not run
        // rather than run as though the token had been answered. A `Parallel` BRANCH can,
        // because a branch reports nothing until it ends: the frames the suspension
        // records are what the continuation re-enters it through ([`ParkedFrame`]). It is
        // the same bound an activity task is held to, for the same reason.
        if call.callback && self.map_depth > 0 {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' waits for a task token inside a 'Map' iteration, which this \
                     emulator does not wait at, so nothing was run for it"
                )),
                at,
            ));
        }
        // And one execution waits at ONE such unit of work, so a second concurrent
        // suspension — a sibling branch of the same `Parallel` reaching a task or a job of
        // its own — hands out no token either.
        if call.callback && self.suspended_in_a_branch() {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' waits for a second task token while this execution is \
                     already waiting at one, which this emulator does not wait at, so nothing \
                     was run for it"
                )),
                at,
            ));
        }
        // And, like the run-a-job pattern, the callback pattern is served only for
        // the STANDARD workflow type: an EXPRESS execution fails AT this state and
        // the call is NOT made, so no token is handed out for an answer the workflow
        // type cannot wait for.
        if call.callback && self.express {
            return Err(self.task_fault(
                name,
                &work,
                express_callback_refusal(name, entered),
                at,
            ));
        }
        // A walk CONTINUING from an answer to this state's token takes up the retrier
        // budget the suspended task carried, so the state fails once its `Retry` is
        // spent instead of making the call again for every answer. Read BEFORE the
        // retry loop, because that loop's own first turn is what consumes the answer.
        self.resumed_attempts = self
            .callback_reply
            .as_ref()
            .filter(|reply| reply.state_name == name)
            .map(|reply| reply.retries_taken.clone());
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // A `SendTask*` has ANSWERED the callback this state's call carried: that
            // answer is this attempt's outcome, so the call is not made a second time
            // and the events up to here are the ones that call already recorded
            // ([`Resumed::index`]). A `Retry` that answers for a reported failure
            // runs the next attempt below, which makes the call — and hands out a
            // token — again.
            let answered = match &me.callback_reply {
                Some(reply) if reply.state_name == name => me.callback_reply.take(),
                _ => None,
            };
            if let Some(reply) = answered {
                me.resumed = Some(Resumed {
                    index: me.steps.len(),
                    at,
                });
                // The names the closure this reply reports answers to beside the one it
                // is reported under travel with the failure, so this state's `Retry` and
                // its `Catch` are matched against the same set — which is what makes a
                // heartbeat window that lapsed reach a retrier or a catcher naming
                // `States.HeartbeatTimeout` as well as one naming `States.Timeout`
                // ([`super::task_closure_errors`]).
                let outcome = reply
                    .outcome
                    .map_err(|failure| failure.also_answering(reply.also_answers));
                return (me.answered_callback(name, &work, at, outcome), at);
            }
            // A branch of the enclosing `Parallel` state has already failed, so this
            // state's work was stopped in flight: the call is NOT made, nothing is
            // scheduled for it, and nothing it would have done is done.
            if let Some(stopped) = me.stopped_before_working(at) {
                return (Err(stopped), at);
            }
            // This attempt's own request reads `$$.State.RetryCount`, so the payload
            // template is rendered with the context of the attempt it is on. A
            // callback attempt's request also reads `$$.Task.Token`, which is the
            // token THIS attempt hands the work it calls: each attempt is a separate
            // hand-over, so each gets a token of its own and only the last one
            // outstanding answers.
            let attempt_context = with_retry_count(context, me.retry_count);
            let token = call.callback.then(|| me.fresh_task_token());
            let attempt_context = match &token {
                Some(token) => with_task_token(&attempt_context, token),
                None => attempt_context,
            };
            // "TimeoutSeconds […] If the task runs longer than the specified seconds,
            // this state fails with a States.Timeout error name": the span it bounds is
            // the call's own, so the member is what the WAIT for the answer is bounded
            // by (`integration::Tasks::perform`), not merely what the span is compared
            // with afterwards. A state that declares none leaves the emulator's own
            // ceiling as the only bound.
            //
            // It is read through the SAME reader an activity task's bounds are read
            // with, which holds the member to the "positive integer" of seconds the
            // language defines: a figure that is not one is a defect in the definition,
            // and reading it as a whole number of seconds is what makes converting it to
            // a `Duration` total for every value a caller can write.
            //
            // Read HERE, inside the ATTEMPT and against that attempt's own context, for
            // the same reason the request below is: the two bounds are fields of this
            // state, so a field this attempt could not read is the attempt's own failure
            // and the state answers for it through its `Retry` and `Catch` like any
            // other error of its work — which is what makes an expression that
            // misevaluated in one of them ([`QUERY_EVALUATION_ERROR`]) an ordinary
            // state error rather than a fault that ends the execution.
            let within = match me.task_seconds(
                state,
                language,
                "TimeoutSeconds",
                &effective,
                &attempt_context,
            ) {
                Ok(seconds) => seconds.map(Duration::from_secs),
                Err(failure) => return (Err(failure), at),
            };
            let heartbeat = match me.task_seconds(
                state,
                language,
                "HeartbeatSeconds",
                &effective,
                &attempt_context,
            ) {
                Ok(seconds) => seconds,
                Err(failure) => return (Err(failure), at),
            };
            // "HeartbeatSeconds […] If more time than the specified seconds elapses
            // between heartbeats from the task, this state fails with a States.Timeout
            // error name": the heartbeats of a task are sent by whoever is DOING its
            // work. The work of a service integration this emulator performs itself
            // sends none, so the window the member declares closes at its own length and
            // what bounds such a task is whichever of the two spans the state declared
            // runs out first. A CALLBACK task's work is somebody ELSE's — whoever the
            // token went to — so the window of one is bounded below instead, from the
            // last heartbeat quoting its token
            // ([`super::super::integration::Alive`]). Each member is still REPORTED as
            // the definition wrote it either way.
            let bounded = match (within, heartbeat.map(Duration::from_secs)) {
                (Some(timeout), Some(heartbeat)) => Some(timeout.min(heartbeat)),
                (timeout, heartbeat) => timeout.or(heartbeat),
            };
            // Resolving what the work is handed is the INTERPRETER's own step, taken
            // before anything is handed over: a template or a path that cannot be
            // resolved against this state's input means the work was never submitted,
            // so the state records no lifecycle event for work that did not happen and
            // the failure is the execution's ([`RUNTIME_ERROR`], which is neither
            // retried nor caught).
            let request = match me.task_request(state, language, &effective, &attempt_context) {
                Ok(request) => request,
                Err(failure) => return (Err(failure), at),
            };
            // WHO this attempt's work is made as: the role the state's `Credentials`
            // names, resolved against THIS attempt's own context exactly as the request
            // above is, so a role the member computes from a variable or an intrinsic is
            // the role this attempt's call is made under. A state declaring none makes
            // its work as the state machine's execution role, as every state does.
            let acting_as = match me.task_role_arn(state, language, &effective, &attempt_context) {
                Ok(role) => role,
                Err(failure) => return (Err(failure), at),
            };
            // WHICH call this attempt makes, for a task that starts a nested execution:
            // an EXPRESS child is RUN by the call that starts it, because that workflow
            // type has no execution-scoped read for the pattern's own reporting call to
            // poll ([`super::super::integration::run_child_synchronously`]). Decided
            // from the child THIS attempt's request names, before anything is handed
            // over, so the events below name the work this attempt really did.
            let mut call = match super::super::integration::nested_child(&call, &request)
                .filter(|child| me.tasks.express_child(child))
            {
                Some(_) => super::super::integration::run_child_synchronously(&call),
                None => call.clone(),
            };
            call.acting_as = acting_as;
            // Each attempt hands the work over and starts it again, so each writes
            // its own pair of events, carrying the document THAT attempt built and
            // the bounds the state declared for it.
            me.push_detailed(
                work.scheduled(),
                name,
                at,
                super::super::EventDetails {
                    input: Some(me.echo(&request)),
                    timeout_seconds: within.map(|within| within.as_secs()),
                    heartbeat_seconds: heartbeat,
                    // The role this attempt's work is made under, which the event that
                    // reports the task as scheduled states.
                    task_role_arn: call.acting_as.clone(),
                    ..work.naming()
                },
            );
            // The attempt BEGINS here, which is what the start event records: the work
            // the state handed over is now this service's to perform, and nothing about
            // how the call turns out is known yet. So the start is written before any of
            // it — before the role's own decision below, before the allowances that may
            // stop the walk handing the call over, and before the call itself.
            me.push_detailed(work.started(), name, at, work.naming());
            // The call is made AS the state machine's execution role, so what that
            // role may do decides whether it is made AT ALL: a call the policies in
            // force on the role do not allow is the invoked service's own
            // access-denied refusal and this attempt's own failure, reported before the
            // request goes anywhere — so the attempt that started closes as a failure
            // and no submission is claimed.
            if let Some(refused) = me.tasks.authorize(&call, &request) {
                let failure = super::reported(refused);
                me.push_task_failure(name, &work, &failure, at);
                return (Err(failure), at);
            }
            // The interpretation's own work allowance is consulted HERE as well as at
            // each transition: a run that has asked for more work than it may do makes
            // no further call (and a `Retry` cannot keep making them).
            if let Some(failure) = me.out_of_work() {
                me.push_task_failure(name, &work, &failure, at);
                return (Err(failure), at);
            }
            // And so is the waiting this run may still do: a run that has spent it does
            // not hand this call over, because there is no span left to wait for its
            // answer in. NOTHING is run for the task, which is the failure this service
            // reports for work it did not perform ([`RUNTIME_ERROR`], as every other
            // call it declines to make carries) rather than a timeout: no bound the
            // state declared for its work expired, because the work never began.
            if me.call_wait_left.is_zero() {
                let failure = Failure {
                    error: Some(RUNTIME_ERROR.to_string()),
                    cause: Some(format!(
                        "the work of state '{name}' was not handed over: this execution has \
                         spent the {} seconds this emulator waits for the calls its tasks are \
                         made of, so nothing was run for it",
                        MAX_CALL_WAIT_DURATION.as_secs()
                    )),
                    ..Failure::default()
                };
                me.push_task_failure(name, &work, &failure, at);
                return (Err(failure), at);
            }
            // The PUBLICATION a `Task` state makes is this service's own call: the entries
            // are the state's document, and what Step Functions adds to each of them is
            // the attribution of the publication to the run that made it — the execution's
            // ARN and its state machine's
            // ([`super::super::integration::publication_request`]). Every other call is
            // made with the document the state built, unchanged.
            let attributed = super::super::integration::publication_request(
                &call,
                &request,
                &me.execution_arn,
                me.state_machine_arn(),
            );
            let called = attributed.as_ref().unwrap_or(&request);
            // A CALLBACK task's token goes out WITH this call, and it answers the task
            // from that instant: the work it reaches may report the task's outcome from
            // inside this very call. So it is handed over BEFORE the call is made, which
            // is what lets a `SendTask*` quoting it be answered while the call is still
            // in flight ([`super::super::integration::Tasks::hand_over`]).
            if let Some(token) = &token {
                me.tasks.hand_over(token, &me.execution_arn, name);
            }
            // The work is done here — and for one item of a `Map` state whose items run
            // together, this is where the item's own call is STARTED and the item yields
            // so that a sibling's is started beside it: the turn unwinds, and the walk
            // takes it up again at this very state once the call answers
            // ([`Interpreter::perform_call`]).
            //
            // A CALLBACK task's `HeartbeatSeconds` bounds the span between two reports of
            // progress from the work its token went to, so it is handed over WITH the
            // token rather than folded into the span the call itself is held to: the
            // window is re-read while the call is in flight, and each heartbeat quoting
            // the token starts it again. What bounds such a call is therefore the state's
            // own `TimeoutSeconds`, "its maximum allowed duration, regardless of the
            // number of SendTaskHeartbeat requests received".
            let alive = token.as_ref().zip(heartbeat).map(|(token, seconds)| {
                super::super::integration::Alive {
                    token: token.clone(),
                    window: Duration::from_secs(seconds),
                }
            });
            let held_to = match alive {
                Some(_) => within,
                None => bounded,
            };
            let Some((answered, began)) =
                me.perform_call(&call, called, me.call_within(held_to), alive.as_ref())
            else {
                return (Err(unwound()), at);
            };
            // A publication the service SERVED but which rejected an entry did not do
            // this task's work, so it is read as the failure it is before the answer
            // becomes a result
            // ([`super::super::integration::rejected_entries`]); every other
            // answer is carried through as it came back.
            let answered = answered.and_then(|answer| {
                match super::super::integration::rejected_entries(&call, &answer) {
                    Some(failed) => Err(failed),
                    None => Ok(answer),
                }
            });
            // The call has come back, so the handover is over: what it carries, if
            // anything answered it, is this task's own outcome.
            let replied = token
                .as_ref()
                .and_then(|token| me.tasks.settle_handover(token));
            // What the WORK really took, which is the span the event that closes this
            // attempt reports: measured from the instant the call was started, so a call
            // started while a sibling item's was in flight reports its own span. What the
            // walk spent BLOCKED in it is charged to the allowance that bounds the
            // waiting one run may do, by the call itself
            // ([`Interpreter::perform_call`]).
            let spent = super::super::now()
                .duration_since(began)
                .unwrap_or_default();
            // The identifier of the job a run-a-job call STARTED is read from the
            // answer as the invoked model spells it, before that answer becomes the
            // task's result in the convention the `Resource` reports it in
            // ([`super::super::integration::reported_result`]).
            let started = answered.as_ref().ok().and_then(|answer| {
                let job = call.job?;
                answer
                    .get(job.job_id)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            // Whether the call this attempt made has ALREADY run its job to
            // completion, which is what decides whether the task parks at one
            // ([`super::super::integration::ran_to_completion`]).
            let ran_to_completion = super::super::integration::ran_to_completion(&call);
            let attempt = match answered {
                // The result is a document a state carries, so it is measured
                // against the same ceiling every state's document is measured
                // against.
                Ok(answer) => {
                    // A call that ran its job to completion answers the JOB's own
                    // report, so what the task ended as is read out of it exactly as
                    // the poll of a parked job reads it ([`RunAJob::outcome_of`]): a
                    // job that did its work makes the task succeed with that report,
                    // and one that ended without doing it makes the task fail with the
                    // same cause a polled job's failure carries.
                    let read = match call.job.filter(|_| ran_to_completion) {
                        Some(job) => match job.outcome_of(&answer, call.parsed_output) {
                            Some(Ok(report)) => {
                                Ok(super::super::integration::task_result(&report))
                            }
                            Some(Err(cause)) => Err(Failure {
                                error: Some(TASK_FAILED_ERROR.to_string()),
                                cause: Some(cause),
                                ..Failure::default()
                            }),
                            // The answer carries no status of the run at all, so
                            // nothing is known about what the child did: the task
                            // reports that rather than a result nothing stated.
                            None => Err(runtime(format!(
                                "state '{name}': the '{}' call of {} answered no status for the \
                                 child execution it ran, so nothing was reported for it",
                                call.operation, call.api.sdk_id
                            ))),
                        },
                        None => Ok(super::super::integration::reported_result(&call, &answer)),
                    };
                    match read {
                        Ok(result) => match measure_payload(&result, work.produced_by()) {
                            Ok(_bytes) => Ok(result),
                            Err(failure) => Err(failure),
                        },
                        Err(failure) => Err(failure),
                    }
                }
                Err(failed) => Err(super::reported(failed)),
            };
            // What the ANSWER to this attempt's token decided, for a callback task whose
            // token was answered while its call was in flight: it is the attempt's
            // outcome, read below once the submission has been recorded.
            let mut finished: Option<Result<Value, Failure>> = None;
            match (&attempt, call.job) {
                // The call SUBMITTED the job and ran it to its end: the task is
                // finished by that one call, so what it answered is recorded as the
                // task's submission ("the response from a resource when a task has
                // started") and the outcome event says what the job ended as — rather
                // than the walk parking at a job nothing can report.
                (outcome, Some(_)) if ran_to_completion => {
                    me.push_detailed(
                        super::super::EVENT_TASK_SUBMITTED,
                        name,
                        at,
                        work.naming(),
                    );
                    match outcome {
                        Ok(result) => {
                            let mut details = work.naming();
                            details.output = Some(result.to_string());
                            me.push_detailed(work.succeeded(), name, at, details);
                        }
                        Err(failure) => me.push_task_failure(name, &work, failure, at),
                    }
                }
                // A run-a-job integration's call STARTED a job: the task is not
                // finished, so what the call answered is recorded as the task's
                // SUBMISSION ("the response from a resource when a task has
                // started") and the walk parks at the job until it ends.
                (Ok(result), Some(job)) => {
                    let Some(job_id) = started else {
                        let failure = runtime(format!(
                            "state '{name}': the '{}' call of {} answered no '{}', so the job it \
                             started cannot be reported and nothing was run for it",
                            call.operation, call.api.sdk_id, job.job_id
                        ));
                        me.push_task_failure(name, &work, &failure, at);
                        return (Err(failure), at);
                    };
                    let mut details = work.naming();
                    details.output = Some(result.to_string());
                    me.push_detailed(super::super::EVENT_TASK_SUBMITTED, name, at, details);
                    me.parked = Some(ParkedJob {
                        state_name: name.to_string(),
                        raw_input: raw.clone(),
                        job,
                        api: call.api,
                        family: call.family,
                        parsed_output: call.parsed_output,
                        job_id: job_id.to_string(),
                        // The job is reported and stopped through calls of its own
                        // service, which for some integrations name it by a member of
                        // the request that STARTED it beside the identifier that call
                        // answered.
                        naming: job.naming(&request),
                        // And the resource the job BELONGS to, as that request
                        // addressed it: the calls that report and stop the job are
                        // authorized over it, and carry the job's identifier rather
                        // than naming it themselves.
                        acts_on: job.acts_on_named(&request, &job_id),
                        submitted: result.clone(),
                        at,
                        // The bound the job is held to is the shorter of the two spans
                        // the state declared: nothing heartbeats a job this service
                        // polls, so a `HeartbeatSeconds` closes its window at its own
                        // length.
                        timeout_seconds: bounded.map(|bounded| bounded.as_secs()),
                        // Filled in as the walk unwinds: each concurrent state the
                        // suspension passes through records its own frame
                        // ([`Interpreter::run_branches`]).
                        frames: Vec::new(),
                    });
                }
                (Ok(result), None) => {
                    let mut details = work.naming();
                    details.output = Some(result.to_string());
                    // A CALLBACK integration's call HANDED the work over together
                    // with the token that answers it: the task is not finished by
                    // the call, so what the call answered is recorded as the task's
                    // SUBMISSION ("the response from a resource when a task has
                    // started") and the token is what finishes it.
                    match &token {
                        Some(token) => {
                            me.push_detailed(
                                super::super::EVENT_TASK_SUBMITTED,
                                name,
                                at,
                                details,
                            );
                            match replied {
                                // The work ANSWERED the token while the call was
                                // still in flight, which is how the callback pattern
                                // is written for a function handed `$$.Task.Token`:
                                // that answer is the task's outcome, so the state is
                                // finished here instead of suspending at a token
                                // nothing will quote again.
                                Some(outcome) => {
                                    finished =
                                        Some(me.answered_callback(name, &work, at, outcome));
                                }
                                // Nothing has answered it: the walk suspends at the
                                // token, and the task the store records for it is
                                // what a later `SendTask*` answers.
                                None => {
                                    me.waiting = Some(ScheduledActivity {
                                        // A callback task belongs to no activity: no
                                        // poll claims it, and what answers it is the
                                        // token the call just carried.
                                        activity_arn: None,
                                        token: Some(token.clone()),
                                        state_name: name.to_string(),
                                        input: me.echo(&request),
                                        raw_input: raw.clone(),
                                        at,
                                        timeout_seconds: within
                                            .map(|within| within.as_secs()),
                                        heartbeat_seconds: heartbeat,
                                        // The budget this state's `Retry` has spent
                                        // so far, which the walk that continues from
                                        // the answer takes up: the budget is the
                                        // state's, not this attempt's.
                                        retries_taken: me.attempts_spent.clone(),
                                        // Contributed by the concurrent states that
                                        // enclose this one, as the walk leaves each of
                                        // them ([`Interpreter::run_branches`]).
                                        frames: Vec::new(),
                                    });
                                }
                            }
                        }
                        None => {
                            me.push_detailed(work.succeeded(), name, at, details);
                        }
                    }
                }
                // The call itself FAILED, so the work was never submitted: that failure
                // is the task's own — RECORDED: a starting call the invoked service
                // REFUSED closes the attempt under the family's own failure event, the
                // one every other failure of this state's work is recorded under
                // ([`WorkEvents::failed`]), because nothing was submitted and the
                // refusal IS what the task ended as. An answer the token happened to
                // collect before it (the handover above ended either way) reports an
                // outcome of work this state never handed over.
                (Err(failure), _) => me.push_task_failure(name, &work, failure, at),
            }
            // The span that call really took belongs to the event that CLOSED this
            // attempt: the work ran for exactly that span, and a walk OFFSET cannot
            // carry it — the offsets are the logical clock the definition's own `Wait`
            // states and `Retry` intervals advance, which the branches of a `Parallel`
            // share because they all begin together, so no call may move it.
            me.stamp_work_span(spent);
            // A token the work answered before its call came back finished the task:
            // that answer is the attempt's outcome rather than the submission the call
            // itself answered.
            (finished.unwrap_or(attempt), at)
        });
        match outcome {
            // The walk SUSPENDED at the token this state's callback call carried, or
            // PARKED at the job its run-a-job call started: the state has no result
            // yet, so the walk stops here and the execution waits for the answer. The
            // document the record carries is the one that call answered, which is what
            // the submission event reports.
            Ok(submitted) if self.parked.is_some() || self.waiting.is_some() => Ok(StateStep {
                value: submitted,
                at: end,
                next: Next::Suspended,
            }),
            Ok(result) => {
                let value = self
                    .produced(state, language, &raw, &effective, Some(result), context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("TaskStateExited", name, end, &value);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "TaskStateExited"),
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

    // -----------------------------------------------------------------------
    // Map
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(super) fn map_state(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        raw: Value,
        effective: Value,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        self.push_entered("MapStateEntered", name, at, &raw);
        // A `Map` state inside the turn of another `Map`'s item records a Map Run of its
        // own, reads the dataset its `ItemReader` names and runs items of its own, so
        // from here that turn may no longer hand a call over and be put down: what this
        // state has already done would be undone with it
        // ([`Interpreter::perform_call`]). The ITEMS of this state are a turn each, and
        // each of those sets its own ([`super::map_items`]).
        self.probing = None;
        let (processor, distributed) = map_processor(name, state).map_err(|failure| failure.at(at))?;
        let (processor_states, processor_start) =
            states_of(processor, &format!("the 'ItemProcessor' of state '{name}'"))
                .map_err(|failure| failure.at(at))?;
        // "A user-defined or an auto-generated string that identifies a Map state":
        // the field the Map Run's own ARN carries, the field the ARN of the CHILD state
        // machine the Map Run runs carries, and the field the ARN of every child
        // execution it starts carries too — so one reading
        // ([`super::super::map_run_identity::map_state_label`]) serves them all.
        let label = super::super::map_run_identity::map_state_label(
            &self.machine_name,
            name,
            state.get("Label").and_then(Value::as_str),
        );
        // Which DISTRIBUTED Map state of this walk this one is, in ENTRY order: the Map
        // Run it starts is recorded once its items are done, and the list it goes in is
        // read in this order rather than that one. An inline `Map` starts no Map Run, so
        // it takes no place in that list and is not counted here.
        let entered = self.map_runs_entered;
        // And WHICH entry into a state of this label it is, which is what the Map Run
        // the attempt being continued already started is matched by: a Distributed Map
        // nested inside another is entered once per outer item, and each of those
        // entries is a Map Run of its own.
        let occurrence = self
            .map_runs_entered_by_label
            .get(&label)
            .copied()
            .unwrap_or(0);
        if distributed {
            self.map_runs_entered = self.map_runs_entered.saturating_add(1);
            self.map_runs_entered_by_label
                .insert(label.clone(), occurrence.saturating_add(1));
        }
        // "ItemBatcher — specifies how to batch the items": a state that declares one
        // hands each iteration a BATCH of its items rather than one item, and the
        // `ItemSelector` is applied to each item on the way into its batch.
        let batcher = state.get("ItemBatcher");
        let concurrency = self
            .map_concurrency(state, language, &effective, context)
            .map_err(|failure| failure.at(at))?;
        let tolerated_count = self
            .number_member(
                state,
                language,
                "ToleratedFailureCount",
                &effective,
                context,
                0.0,
            )
            .map_err(|failure| failure.at(at))? as u64;
        let tolerated_percentage = self
            .number_member(
                state,
                language,
                "ToleratedFailurePercentage",
                &effective,
                context,
                0.0,
            )
            .map_err(|failure| failure.at(at))?;
        let tolerance_declared = declares_tolerance(state);

        // A Distributed Map "starts a Map Run", a resource of its own, so the Map Run
        // this state runs as is resolved AS THE STATE IS ENTERED: its items' own
        // results are written under its id ([`super::super::result_writer`]) and the
        // state's output names its ARN, so neither can wait for the store to mint one.
        // A walk CONTINUING an attempt is handed the Map Run that attempt already
        // started ([`Tasks::map_run`]), which keeps the ARN a client is holding.
        let resumed = match distributed {
            true => self.tasks.map_run(occurrence, &label),
            false => None,
        };
        let map_run_arn = match &resumed {
            Some(resumed) => resumed.arn.clone(),
            None => self.fresh_map_run_arn(&label),
        };
        // "ResultWriter — specifies the Amazon S3 location where Step Functions writes
        // all child workflow execution results": the results it writes are a MAP RUN's,
        // and an inline `Map` starts none — so a state that declares one without a
        // DISTRIBUTED processor describes a dataset there is no run to write, and the
        // state is not run for it rather than run as though the member were not there.
        if !distributed && state.contains_key("ResultWriter") {
            return Err(runtime(format!(
                "the 'Map' state '{name}' carries a 'ResultWriter', which writes the results of a \
                 Map Run, while its processor declares no 'DISTRIBUTED' mode and so starts no Map \
                 Run, so nothing was run for it"
            ))
            .at(at));
        }
        // "Step Functions starts a child workflow execution for each item": those
        // executions are started BY the service, as the state machine's own execution
        // role, so a role that may not start them is a Map Run that fails having
        // started NONE of them. Asked before the state's items are read, because
        // nothing of the run happens without that grant.
        if distributed {
            if let Some(failure) = self.child_start_denial() {
                self.map_runs.push((
                    entered,
                    StartedMapRun {
                        arn: map_run_arn,
                        label: label.clone(),
                        status: super::super::MAP_RUN_STATUS_FAILED,
                        started_at: at,
                        stopped_at: at,
                        max_concurrency: super::super::reported_max_concurrency(
                            i64::try_from(concurrency).unwrap_or(i64::MAX),
                        ),
                        tolerated_failure_count: i64::try_from(tolerated_count)
                            .unwrap_or(i64::MAX),
                        tolerated_failure_percentage: tolerated_percentage as f32,
                        // Nothing was iterated and nothing was started: the run failed
                        // before its first child, which is what makes a missing grant
                        // tell apart from a granted one.
                        total_items: 0,
                        items: Vec::new(),
                        children: Vec::new(),
                    },
                ));
                self.push(super::super::EVENT_MAP_STATE_FAILED, name, at);
                return self.catch_or_fail(
                    (name, "MapStateExited"),
                    state,
                    language,
                    &raw,
                    &effective,
                    failure,
                    context,
                    at,
                );
            }
        }

        // The tally belongs to the LAST attempt: a retried `Map` state resets it, so
        // what is reported is what the attempt the state ended on did.
        let mut tally = MapTally::default();
        // Where this state's results are written, for a Distributed Map that declares a
        // `ResultWriter`: the destination is read, and the results written, by the
        // attempt that produced them.
        let writer = match distributed {
            true => state.get("ResultWriter"),
            false => None,
        };
        // What the attempt the state ended on WROTE: the document the state puts out
        // for having written its results, which is not the results themselves.
        let mut written: Option<Value> = None;
        // How many items the attempt the state ended on iterated, which is what the Map
        // Run reports: every attempt reads the state's item source again, so the figure
        // belongs to the attempt rather than to the state.
        let mut total_items = 0usize;
        let (outcome, end) = self.with_retries(state, at, &mut |me, at| {
            // Every attempt at this state reads its item source again — a `Retry`
            // re-runs the state, and reading the dataset an `ItemReader` names is part
            // of what the state DOES — so a read that failed is this attempt's failure
            // and the next attempt reads it afresh.
            let read = me
                .map_items(name, state, language, &effective, context)
                .and_then(|items| match batcher {
                    None => Ok(items),
                    Some(batcher) => me.batched_items(
                        name, state, language, batcher, items, &effective, context,
                    ),
                });
            let items = match read {
                Ok(items) => items,
                Err(failure) => return (Err(failure), at),
            };
            total_items = items.len();
            // Every attempt iterates those items, so every attempt writes its own
            // `MapStateStarted` — carrying "the size of the array for Map state
            // iterations" — and the event of the outcome those iterations reached.
            me.push_detailed(
                super::super::EVENT_MAP_STATE_STARTED,
                name,
                at,
                super::super::EventDetails {
                    length: Some(i32::try_from(items.len()).unwrap_or(i32::MAX)),
                    ..super::super::EventDetails::default()
                },
            );
            let plan = MapPlan {
                items: &items,
                name,
                label: &label,
                states: processor_states,
                start_at: processor_start,
                selector: match batcher {
                    // A batched state's selector was spent on the items BEFORE they
                    // were placed in their batches, so the batch an iteration is handed
                    // is already the document it runs on.
                    Some(_) => None,
                    None => state.get("ItemSelector").or_else(|| state.get("Parameters")),
                },
                language,
                distributed,
                batched: batcher.is_some(),
                concurrency,
                tolerated_count,
                tolerated_percentage,
                tolerance_declared,
                // What the attempt being CONTINUED already ran each item as, for a
                // redrive: the items whose child execution succeeded are not run again.
                resumed: resumed.as_ref().map(|resumed| resumed.items.as_slice()),
            };
            // This attempt's own members read `$$.State.RetryCount`, so they are
            // evaluated with the context of the attempt they are on.
            let context = with_retry_count(context, me.retry_count);
            let (mut outcome, end) = me.run_map_items(&plan, &effective, &context, at, &mut tally);
            // An iteration's offset into the run is this walk's LOGICAL clock, which the
            // calls its states made do not advance — so an item whose work really took
            // time finished at an instant later than its offset says. Each child
            // execution's own offset is therefore raised to the span this walk has REALLY
            // spent, read ONCE here, before anything reports a child: the records the
            // store keeps and the results a `ResultWriter` writes are both derived from
            // this offset ([`Interpreter::settle`]), so they cannot disagree, and neither
            // reports a child as having stopped at an instant inside the work it did.
            let spent = super::super::now()
                .duration_since(me.start)
                .unwrap_or_default();
            for child in &mut tally.children {
                child.finished_at = child.finished_at.max(spent);
            }
            // "ResultWriter — specifies the Amazon S3 location where Step Functions
            // writes all child workflow execution results": writing them is part of
            // what this ATTEMPT does, so it happens here — whichever status the items
            // reached, because the results of a failed run are written too — and a
            // write that could not be made is the attempt's own failure, which the
            // state's `Retry` answers for exactly as it answers for its items'. A run
            // that had already failed keeps its own failure, which happened first.
            written = None;
            if let Some(writer) = writer {
                match me.write_results(
                    name, language, writer, &map_run_arn, &tally, &effective, &context,
                ) {
                    Ok(document) => written = Some(document),
                    Err(failure) => {
                        if outcome.is_ok() {
                            outcome = Err(failure);
                        }
                    }
                }
            }
            // Every attempt records the event of the outcome it reached, which is the
            // outcome of its items AND of the results it wrote for them.
            let event = if outcome.is_ok() {
                super::super::EVENT_MAP_STATE_SUCCEEDED
            } else {
                super::super::EVENT_MAP_STATE_FAILED
            };
            me.push(event, name, end);
            (outcome, end)
        });
        // A Distributed Map "starts a Map Run", a resource of its own; an INLINE Map
        // starts none, so only the distributed mode records one.
        if distributed {
            // Held in the order the iterations FINISHED, which is the order the Map
            // Run counts them in: items of one batch run at once, so the one that
            // finished first is not always the one that started first.
            let mut finished_items = std::mem::take(&mut tally.items);
            finished_items.sort_by_key(|item| item.finished_at);
            // Where this state's Map Run belongs in the walk's list is decided by the
            // position its state holds among the Map states ENTERED, never by a
            // position in the list captured before the items ran: the same list is
            // pruned WHILE they run, by the failure of a nested `Parallel` or `Map`
            // ([`Interpreter::abort_units`]), so a position taken at entry no longer
            // names the same place.
            self.map_runs.push((
                entered,
                StartedMapRun {
                    arn: map_run_arn,
                    label: label.clone(),
                    status: if outcome.is_ok() {
                        super::super::MAP_RUN_STATUS_SUCCEEDED
                    } else {
                        super::super::MAP_RUN_STATUS_FAILED
                    },
                    started_at: at,
                    stopped_at: end,
                    max_concurrency: super::super::reported_max_concurrency(
                        i64::try_from(concurrency).unwrap_or(i64::MAX),
                    ),
                    tolerated_failure_count: i64::try_from(tolerated_count).unwrap_or(i64::MAX),
                    tolerated_failure_percentage: tolerated_percentage as f32,
                    total_items: i64::try_from(total_items).unwrap_or(i64::MAX),
                    items: finished_items,
                    // One child execution per item of the attempt the state ended
                    // on, which is what `ListExecutions` reports for this Map Run.
                    children: std::mem::take(&mut tally.children),
                },
            ));
        }
        match outcome {
            Ok(results) => {
                // What the state produced: where its results were WRITTEN for a run
                // that declared a `ResultWriter`, and the results themselves for every
                // other one. The state's own result-side data flow is applied to
                // whichever document that is.
                let produced = written.unwrap_or(results);
                let value = self
                    .produced(state, language, &raw, &effective, Some(produced), context, true)
                    .map_err(|failure| failure.at(end))?;
                self.push_exited("MapStateExited", name, end, &value);
                Ok(StateStep {
                    value,
                    at: end,
                    next: self
                        .next_of(state)
                        .map_err(|failure| failure.at(end))?,
                })
            }
            Err(failure) => self.catch_or_fail(
                (name, "MapStateExited"),
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

    /// One `Map` state's `ItemSelector`, in the query language that state is written
    /// in: a payload template in JSONPath, a template of JSONata expressions in
    /// JSONata.
    pub(super) fn render_selector(
        &self,
        language: &'static str,
        template: &Value,
        effective: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        match language {
            JSONATA => {
                let bindings = self.bindings(effective, context, None, None);
                super::super::jsonata::render(
                    template,
                    ITEM_SELECTOR_MEMBER,
                    &bindings,
                    &mut self.jsonata_budget(),
                )
                .map_err(jsonata_failure)
            }
            _ => payload_template(template, effective, context),
        }
    }

    /// The items a `Map` state iterates: what its `ItemReader`'s dataset holds when it
    /// declares one, else `Items` when it declares that, else what `ItemsPath`
    /// (default `$`) selects from the state's effective input.
    pub(super) fn map_items(
        &self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        // "ItemReader — specifies a dataset and its location": a state that declares
        // one iterates THAT dataset, so nothing of its own input is read for its items.
        if let Some(reader) = state.get("ItemReader") {
            return self.dataset_items(name, reader, language, effective, context);
        }
        let selected = match state.get("Items") {
            // "Items — an array, or a JSONata expression that evaluates to an array":
            // the member is the array itself in either language, and in JSONata it may
            // be the expression that produces one — held to that type, so an
            // expression that produced something else says so.
            Some(items) => match language {
                JSONATA => self.typed_field(
                    language,
                    items,
                    "Items",
                    super::super::jsonata::Kind::Array,
                    effective,
                    context,
                )?,
                _ => payload_template(items, effective, context)?,
            },
            None => match state.get("ItemsPath") {
                None => effective.clone(),
                Some(Value::String(path)) => select(path, effective, context)?,
                Some(other) => {
                    return Err(runtime(format!(
                        "'ItemsPath' must be a reference-path string, not {other}"
                    )));
                }
            },
        };
        if let Value::Array(items) = selected {
            return Ok(items);
        }
        // The items a `Map` state iterates come from a REFERENCE PATH — its `ItemsPath`,
        // or the default `$` the language gives a state that declares none — so a value
        // that is not an array is reported as that path not pointing at one. A state
        // whose items are its `Items` member names no path, so the member itself is
        // what the failure names.
        match (state.get("Items"), state.get("ItemsPath")) {
            (Some(_), _) => Err(runtime(format!(
                "the 'Items' field of the 'Map' state '{name}' is not an array of items to \
                 iterate"
            ))),
            (None, items_path) => Err(runtime(reference_path_must_point_to_array(
                items_path
                    .and_then(Value::as_str)
                    .unwrap_or(WHOLE_INPUT_PATH),
            ))),
        }
    }

    /// The items the DATASET a `Map` state's `ItemReader` names holds.
    ///
    /// The reader's own `Parameters` (or JSONata `Arguments`) is a payload template of
    /// the state's language, so it is rendered here — against the state's effective
    /// input, exactly as its `ItemSelector` is — and `MaxItems` is resolved from the
    /// member or from its `MaxItemsPath` form in that same language. The dataset is
    /// then read through the same seam a `Task` state's integration call is made over
    /// ([`super::super::item_reader`]).
    pub(super) fn dataset_items(
        &self,
        name: &str,
        reader: &Value,
        language: &'static str,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        let parameters = match reader.get("Arguments").or_else(|| reader.get("Parameters")) {
            Some(template) => self.render_selector(language, template, effective, context)?,
            None => Value::Object(Map::new()),
        };
        // "MaxItems — limits the number of data items passed", and `MaxItemsPath` reads
        // that figure from the state's input instead: one member of the reader's
        // `ReaderConfig`, resolved the same way every other numeric member of a state
        // is.
        let empty = Map::new();
        let config = reader
            .get("ReaderConfig")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let max_items =
            self.number_member(config, language, "MaxItems", effective, context, 0.0)? as usize;
        let dataset = super::super::item_reader::read(name, reader, &parameters, max_items)
            .map_err(reported)?;
        dataset.items(name, self.tasks).map_err(reported)
    }

    /// Write the results of the Map Run `map_run_arn` where the state's `ResultWriter`
    /// says, and answer the document the state puts out for having written them.
    ///
    /// The writer's own `Parameters` (or JSONata `Arguments`) is a payload template of
    /// the state's language, so it is rendered here — against the state's effective
    /// input, exactly as its `ItemSelector` is — and the results are then written
    /// through the same seam the dataset an `ItemReader` names is read over
    /// ([`super::super::result_writer`]).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn write_results(
        &self,
        name: &str,
        language: &'static str,
        writer: &Value,
        map_run_arn: &str,
        tally: &MapTally,
        effective: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        let parameters = match writer.get("Arguments").or_else(|| writer.get("Parameters")) {
            Some(template) => self.render_selector(language, template, effective, context)?,
            None => Value::Object(Map::new()),
        };
        let destination =
            super::super::result_writer::read(name, writer, &parameters).map_err(reported)?;
        // One record per child workflow execution the run's items ran as, in the order
        // they were started, each reporting what that execution itself reports.
        let results: Vec<super::super::result_writer::ItemResult> = tally
            .children
            .iter()
            .map(|child| super::super::result_writer::ItemResult {
                status: child.status.to_string(),
                execution_arn: child.arn.clone(),
                name: child.name.clone(),
                input: child.input.clone(),
                output: child.output.clone(),
                error: child.error.clone(),
                cause: child.cause.clone(),
                // The instants that execution ran between: for a child this attempt
                // started, the offsets its own iteration began and ended at — its stop
                // owed to the clock one `settle` later, exactly as the store records it
                // ([`Interpreter::settle`]) — and for one it KEPT, the dates the record
                // already carries, which is what `DescribeExecution` goes on reporting
                // for it. So the results and the execution never disagree.
                start_date: iso(match child.kept {
                    Some((started, _)) => started,
                    None => super::super::after(self.start, child.started_at),
                }),
                stop_date: iso(match child.kept {
                    Some((_, stopped)) => stopped,
                    None => super::super::after(
                        self.start,
                        self.settle.saturating_add(child.finished_at),
                    ),
                }),
                // A child workflow execution is never redriven on its own — its Map
                // Run redrives it, and a run that redrives one starts it afresh — so
                // every record reports the count its own execution carries, which is
                // the zero it was started with.
                redrive_count: 0,
            })
            .collect();
        let written = super::super::result_writer::write(
            name,
            &destination,
            map_run_arn,
            self.state_machine_arn(),
            &results,
            self.tasks,
        )
        .map_err(reported)?;
        Ok(written.reported())
    }

    /// The state machine this walk runs, as the context object (`$$`) reports it:
    /// "StateMachine.Id", which is the ARN every child workflow execution of a Map Run
    /// of it ran under.
    pub(super) fn state_machine_arn(&self) -> &str {
        self.base_context
            .pointer("/StateMachine/Id")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// A fresh ARN for the Map Run a Distributed Map state labelled `label` starts.
    ///
    /// The id is drawn from the same generator a child workflow execution's name is
    /// ([`Interpreter::fresh_child_name`]), because it is the field that tells two Map
    /// Runs of one `Map` state apart and nothing outside this walk has minted it yet:
    /// the run's results are written under it before the store records the run.
    pub(super) fn fresh_map_run_arn(&mut self, label: &str) -> String {
        let id = self.fresh_child_name();
        super::super::map_run_arn(&self.region, &self.machine_name, label, &id)
    }

    /// Why the execution role this walk runs as may not start the CHILD workflow
    /// executions of a Distributed Map state, or `None` when nothing of the kind can be
    /// shown.
    ///
    /// "Step Functions starts a child workflow execution for each item": the start is
    /// made by the service, as the state machine's own execution role, on the state
    /// machine the children run — so it is subject to `states:StartExecution` on that
    /// state machine, which is the same access request the optimized
    /// `states:startExecution` integration's own call is subject to
    /// ([`super::super::integration::access_request`]) and is decided by the same gate
    /// ([`Tasks::authorize`]).
    ///
    /// RECORDED: the failure is the service's own [`RUNTIME_ERROR`] — the run could not
    /// be made, which is neither retried nor caught — and it carries the refusal the
    /// start came back with, so an operator reads which principal was denied which
    /// action on which state machine.
    ///
    /// The start is Step Functions' OWN, so no invoked service answers it: it is decided
    /// on what the account HOLDS about the role ([`Tasks::authorize_child_start`])
    /// rather than on the reading an invoked service's front door takes of a call made
    /// as that role.
    pub(super) fn child_start_denial(&self) -> Option<Failure> {
        let call = match super::super::integration::read(CHILD_START_RESOURCE) {
            Ok(call) => call,
            // The `Resource` is this crate's own constant, so it resolves. A build in
            // which it did not could form no access request for the start, and an
            // authorization this emulator cannot make is not a grant: the run fails
            // rather than starting children nothing judged.
            Err(why) => {
                debug_assert!(false, "the child-start integration must resolve: {why}");
                return Some(runtime(format!(
                    "the child workflow executions of this 'Map' state could not be authorized: \
                     {why}"
                )));
            }
        };
        let request = serde_json::json!({ "StateMachineArn": self.state_machine_arn() });
        let denied = self.tasks.authorize_child_start(&call, &request)?;
        Some(runtime(denied.cause))
    }

    /// `items` grouped as the state's `ItemBatcher` says, for a state that declares
    /// one: each of the documents the state's iterations are then handed is one BATCH.
    ///
    /// The `ItemSelector` is applied FIRST, item by item, because it transforms each
    /// item and the batches are built out of what it produced — so the state's own
    /// selector is spent here and the iterations are handed the batches unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn batched_items(
        &self,
        name: &str,
        state: &Map<String, Value>,
        language: &'static str,
        batcher: &Value,
        items: Vec<Value>,
        effective: &Value,
        context: &Value,
    ) -> Result<Vec<Value>, Failure> {
        let Some(members) = batcher.as_object() else {
            return Err(runtime(format!(
                "'ItemBatcher' must be an object, not {batcher}"
            )));
        };
        if let Some(member) = members
            .keys()
            .find(|member| !super::super::item_batcher::BATCHER_MEMBERS.contains(&member.as_str()))
        {
            return Err(runtime(format!(
                "the 'ItemBatcher' carries the member '{member}', which this emulator does not \
                 interpret, so nothing was run for it"
            )));
        }
        let selected = match state.get("ItemSelector").or_else(|| state.get("Parameters")) {
            None => items,
            Some(template) => {
                let mut selected = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    let map_item = serde_json::json!({ "Index": index, "Value": item });
                    let item_context = with_map_item(context, &map_item);
                    selected.push(
                        self.render_selector(language, template, effective, &item_context)?,
                    );
                }
                selected
            }
        };
        let batching = super::super::item_batcher::Batching {
            max_items: self.number_member(
                members,
                language,
                "MaxItemsPerBatch",
                effective,
                context,
                0.0,
            )? as usize,
            max_bytes: self.number_member(
                members,
                language,
                "MaxInputBytesPerBatch",
                effective,
                context,
                0.0,
            )? as usize,
            batch_input: match members.get("BatchInput") {
                None => None,
                Some(template) => {
                    Some(self.render_selector(language, template, effective, context)?)
                }
            },
        };
        super::super::item_batcher::batches(name, selected, &batching).map_err(reported)
    }

    /// Run every item of a `Map` state and collect what the iterations produced.
    ///
    /// Items run concurrently on the real service, in batches of `MaxConcurrency`, so
    /// a batch's span is its longest item's rather than the sum: that is what the
    /// returned offset reports. The items of one batch really do make progress
    /// CONCURRENTLY — each is walked a state at a time and yields to the sibling due
    /// before it ([`super::map_items`]) — so an iteration whose own control flow waits
    /// on an effect a sibling commits observes that effect; and their WORK is really
    /// performed at the same time, up to that many calls in flight together.
    ///
    /// `tally` is counted up as the items are run, so it holds what this attempt's
    /// items did however the attempt ended.
    pub(super) fn run_map_items(
        &mut self,
        plan: &MapPlan<'_>,
        effective: &Value,
        context: &Value,
        at: Duration,
        tally: &mut MapTally,
    ) -> Attempt {
        // As for a `Parallel`'s branches: what stopped the walk AROUND this state goes
        // on stopping it, and what one of this state's own iterations stops does not
        // outlive the state ([`Interpreter::cancel_after`]).
        let enclosing = self.cancel_after;
        let attempt = self.walk_map_items(plan, effective, context, at, tally);
        self.cancel_after = enclosing;
        attempt
    }

    /// The child workflow execution the attempt being CONTINUED ran the item whose own
    /// document is `document` as, with its position among that attempt's children; and
    /// `None` for an item that attempt did not get a success for.
    ///
    /// An item is identified by WHAT IT IS — the document the iteration is handed, which
    /// is that child's own `input` — rather than only by where it sits: the item source
    /// is read again on the continued attempt, so a dataset whose objects changed
    /// between them must not attribute one item's result to another. The item's own
    /// position is tried first, because that is where an unchanged dataset's match is,
    /// and a child another item has already claimed is never matched twice.
    pub(super) fn kept_child<'plan>(
        &self,
        plan: &MapPlan<'plan>,
        index: usize,
        document: &str,
        claimed: &[bool],
    ) -> Option<(usize, &'plan super::super::integration::FinishedChild)> {
        let items = plan.resumed?;
        let at = |position: usize| -> Option<&'plan super::super::integration::FinishedChild> {
            match claimed.get(position).copied().unwrap_or(true) {
                true => None,
                false => items
                    .get(position)?
                    .as_ref()
                    .filter(|kept| kept.input == document),
            }
        };
        if let Some(kept) = at(index) {
            return Some((index, kept));
        }
        (0..items.len()).find_map(|position| Some((position, at(position)?)))
    }

    /// Where the concurrent unit that SUSPENDED carries on from, for a walk continuing
    /// an execution whose job has ended or whose task has been answered: the next
    /// concurrent state down, else the state the work belonged to itself.
    pub(super) fn resume_entry(&self) -> Option<(String, Value)> {
        if let Some(frame) = self.resume_frames.first() {
            return Some((frame.state_name.clone(), frame.state_input.clone()));
        }
        // Exactly one of the three is set for a walk that continues an execution: the
        // job that ended, the worker's reply, or the answer to a token.
        let (state_name, state_input) = match (&self.job_reply, &self.reply, &self.callback_reply) {
            (Some(reply), _, _) => (&reply.state_name, &reply.state_input),
            (_, Some(reply), _) => (&reply.state_name, &reply.state_input),
            (_, _, Some(reply)) => (&reply.state_name, &reply.state_input),
            _ => return None,
        };
        Some((
            state_name.clone(),
            serde_json::from_str(state_input).unwrap_or(Value::Null),
        ))
    }

    /// Stop the branches or iterations of a state that has FAILED, at the instant
    /// `when` it failed.
    ///
    /// A state whose concurrent work has already failed does not go on running the
    /// rest of it: each unit is cut back to the events that were due at or before that
    /// instant, the state it was in when the failure reached it records that type's own
    /// aborted event, and the unit itself records the one its kind has. `units` holds
    /// WHERE each unit's own events sit in this walk's list, in the order that unit
    /// recorded them — one run of the list for a `Parallel` branch, which is walked to
    /// its end in one go, and several for a `Map` iteration, whose events are
    /// interleaved with its siblings' by the clock ([`super::map_items`]).
    ///
    /// Every unit is judged against the list as it STANDS and the one cut is applied in
    /// a single pass, so no unit's positions move while another's are still being read.
    pub(super) fn abort_units(&mut self, when: Duration, units: &[Unit]) {
        // Which events of the list no unit keeps, and where the events of a unit's own
        // stop go: at the first event of its own it does not keep, which is where the
        // work it was still doing was cut off.
        let mut dropped: BTreeSet<usize> = BTreeSet::new();
        let mut stopped_at: BTreeMap<usize, Vec<Step>> = BTreeMap::new();
        for unit in units {
            let kept_until = unit
                .steps
                .iter()
                .position(|index| {
                    self.steps
                        .get(*index)
                        .is_some_and(|step| step.after > when)
                });
            let Some(kept_until) = kept_until else {
                continue;
            };
            // The states this unit was still inside when the failure reached it: each
            // one entered and not yet left, innermost last.
            let mut open: Vec<(&'static str, Option<String>)> = Vec::new();
            for step in unit
                .steps
                .get(..kept_until)
                .unwrap_or_default()
                .iter()
                .filter_map(|index| self.steps.get(*index))
            {
                if let Some(entered) = step.event_type.strip_suffix(STATE_ENTERED) {
                    open.push((entered, step.state_name.clone()));
                } else if let Some(left) = step.event_type.strip_suffix(STATE_EXITED) {
                    if let Some(index) = open.iter().rposition(|(name, _)| *name == left) {
                        open.remove(index);
                    }
                }
            }
            let mut stopped: Vec<Step> = Vec::new();
            for (entered, state_name) in open.iter().rev() {
                if let Some(event_type) = aborted_event(entered) {
                    stopped.push(Step {
                        event_type,
                        state_name: state_name.clone(),
                        after: when,
                        terminal: false,
                        details: super::super::EventDetails::default(),
                    });
                }
            }
            if let Some(aborted) = &unit.aborted {
                let mut aborted = aborted.clone();
                aborted.after = when;
                stopped.push(aborted);
            }
            let cut = unit.steps.get(kept_until..).unwrap_or_default();
            if let Some(first) = cut.first() {
                stopped_at.insert(*first, stopped);
            }
            dropped.extend(cut.iter().copied());
        }
        if !dropped.is_empty() {
            self.steps = std::mem::take(&mut self.steps)
                .into_iter()
                .enumerate()
                .flat_map(|(index, step)| {
                    let mut here = stopped_at.remove(&index).unwrap_or_default();
                    if !dropped.contains(&index) {
                        here.push(step);
                    }
                    here
                })
                .collect();
        }
        // A Map Run one of THESE units only started after the state had already failed
        // never started: the state it belongs to was not entered. The list also holds
        // the Map Runs of work that finished before these units began — an earlier
        // `Parallel` branch's, the walk's own — whose own instants this failure says
        // nothing about, so only what these units recorded is judged.
        let recorded_from = units
            .first()
            .map_or(self.map_runs.len(), |unit| unit.map_runs_from)
            .min(self.map_runs.len());
        let mut recorded = self.map_runs.split_off(recorded_from);
        recorded.retain(|(_, map_run)| map_run.started_at <= when);
        self.map_runs.append(&mut recorded);
    }

    // -----------------------------------------------------------------------
    // Retry and Catch
    // -----------------------------------------------------------------------

    /// Run `work`, re-running it while one of the state's `Retry` retriers matches
    /// what it failed with and has attempts left.
    ///
    /// The retriers are scanned in the order the array declares them and the first one
    /// that names the failure decides, by the name the failure is reported under or by any
    /// other name it answers to ([`Failure::also_answers`]) — the one matching every
    /// `Catch` catcher is asked ([`Interpreter::catch_or_fail`]), so neither half of a
    /// state's error handling answers for a failure the other would not.
    ///
    /// The work is really RE-RUN, and the interval each retrier declares really
    /// lengthens the run, so a definition that retries reports an outcome that came
    /// due later than one that does not. The attempt the work is on is what the
    /// retried state reads through `$$.State.RetryCount`.
    pub(super) fn with_retries(
        &mut self,
        state: &Map<String, Value>,
        at: Duration,
        work: &mut dyn FnMut(&mut Self, Duration) -> Attempt,
    ) -> Attempt {
        let enclosing = self.retry_count;
        let answer = self.retry_loop(state, at, work);
        self.retry_count = enclosing;
        answer
    }

    /// [`Interpreter::with_retries`]' loop, which leaves the attempt count for its
    /// caller to restore.
    fn retry_loop(
        &mut self,
        state: &Map<String, Value>,
        at: Duration,
        work: &mut dyn FnMut(&mut Self, Duration) -> Attempt,
    ) -> Attempt {
        let mut at = at;
        let mut attempts: Vec<u64> = state
            .get("Retry")
            .and_then(Value::as_array)
            .map(|retriers| vec![0; retriers.len()])
            .unwrap_or_default();
        // A walk CONTINUING from an answer to a task this state SUSPENDED at takes up
        // the retrier budget that task carried: the budget a `Retry` gives is the
        // STATE's, so `MaxAttempts` counts every attempt the execution has made at it
        // rather than restarting at each answer — which is what makes such a state
        // fail once its retriers are spent instead of retrying for ever. Set by
        // [`Interpreter::task_state`] for exactly the state the answer resumes.
        if let Some(spent) = self.resumed_attempts.take() {
            for (slot, taken) in attempts.iter_mut().zip(spent) {
                *slot = taken;
            }
        }
        // `$$.State.RetryCount` is how many attempts this state has already made,
        // which for a resumed one is what those retriers have spent between them.
        self.retry_count = attempts.iter().copied().fold(0, u64::saturating_add);
        loop {
            // What the state has spent so far, for a task this attempt may SUSPEND at:
            // the record it leaves carries it, so the answer's own walk can seed this
            // loop from it.
            self.attempts_spent = attempts.clone();
            let (outcome, end) = work(self, at);
            at = end;
            let Err(failure) = outcome else {
                return (outcome, at);
            };
            let Some(retriers) = state.get("Retry").and_then(Value::as_array) else {
                return (Err(failure), at);
            };
            let mut retried = false;
            for (index, retrier) in retriers.iter().enumerate() {
                let Some(retrier) = retrier.as_object() else {
                    continue;
                };
                if !failure.matched_by(retrier) {
                    continue;
                }
                let max_attempts = retrier
                    .get("MaxAttempts")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
                let taken = attempts.get(index).copied().unwrap_or(0);
                if taken >= max_attempts {
                    break;
                }
                // The delay is already clamped finite and inside
                // [`MAX_WAIT_SECONDS`] by [`retry_delay_seconds`], whatever a
                // definition writes in `IntervalSeconds`, `BackoffRate`,
                // `MaxDelaySeconds` and `JitterStrategy`. The conversion and the
                // accumulation are
                // nevertheless written so NEITHER can panic: a caller-sized figure
                // must not be able to take the process down, so a delay that somehow
                // left that range waits no time at all and the offset saturates
                // rather than overflowing.
                let delay = Duration::try_from_secs_f64(retry_delay_seconds(retrier, taken))
                    .unwrap_or(Duration::ZERO);
                at = at.saturating_add(delay);
                if let Some(slot) = attempts.get_mut(index) {
                    *slot = taken + 1;
                }
                retried = true;
                break;
            }
            if !retried {
                return (Err(failure), at);
            }
            // A retry re-runs the work, and the attempt it re-runs records the events
            // of that attempt: a definition cannot ask for more attempts than the
            // execution's history carries, and a state whose retriers reach that quota
            // ends on the quota's own failure — the same one the next transition would
            // report — rather than on the attempt's. The attempt is charged HERE, and
            // not only where it records something, because an attempt may fail before
            // it reaches an event of its own — a `Map` state whose item source could not
            // be read reaches none — and this loop makes progress towards its end on
            // every iteration whatever the attempt did, however many attempts the
            // retrier declares.
            if self.events_left == 0 {
                return (Err(history_limit_exceeded()), at);
            }
            self.events_left -= 1;
            self.retry_count += 1;
        }
    }

    /// The state the first matching `Catch` catcher names, or the failure itself when
    /// none does.
    ///
    /// `left` is the event the state records when a catcher takes over, so a caught
    /// state's history pairs the event it was entered with.
    ///
    /// The catchers are scanned in the order the array declares them and the FIRST one
    /// that names the failure takes it over — by the name the failure is reported under
    /// or by any other name it answers to ([`Failure::also_answers`], which is how a
    /// closure the language gives two names reaches a catcher naming either), never by
    /// how specifically a catcher names it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn catch_or_fail(
        &mut self,
        caught: (&str, &'static str),
        state: &Map<String, Value>,
        language: &'static str,
        raw: &Value,
        effective: &Value,
        failure: Failure,
        context: &Value,
        at: Duration,
    ) -> Result<StateStep, Fault> {
        let (name, left) = caught;
        // The failure as THIS state reports it: the quota sentence raised with no
        // producer named is the document this state produced, so the error output a
        // catcher is handed carries the same sentence the execution's own cause would
        // ([`produced_by_state`]).
        let failure = produced_by_state(name, failure);
        let catchers = state
            .get("Catch")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for catcher in catchers {
            let Some(catcher) = catcher.as_object() else {
                continue;
            };
            if !failure.matched_by(catcher) {
                continue;
            }
            let Some(next) = catcher.get("Next").and_then(Value::as_str) else {
                return Err(
                    runtime("a 'Catch' catcher must carry a string 'Next'".to_string()).at(at),
                );
            };
            // "When a state reports an error and it matches a catcher, the state's
            // output is the error output", placed through the catcher's own
            // `ResultPath` — or, in JSONata, through the catcher's own `Output` and
            // `Assign`, which are the two places `$states.errorOutput` is in scope.
            // The catcher's `Assign` is evaluated the same way in the other language:
            // over the ERROR OUTPUT the catcher was handed, since that is the document
            // its own members read.
            let error_output = failure.payload();
            let value = match language {
                JSONATA => self
                    .jsonata_output(catcher, effective, None, Some(&error_output), context)
                    .map_err(|failure| failure.at(at))?,
                _ => {
                    self.jsonpath_assign(catcher, &error_output, context)
                        .map_err(|failure| failure.at(at))?;
                    self.place_result(catcher, raw, error_output, context, true)
                        .map_err(|failure| failure.at(at))?
                }
            };
            self.push_exited(left, name, at, &value);
            return Ok(StateStep {
                value,
                at,
                next: Next::Named(next.to_string()),
            });
        }
        Err(failure.at(at))
    }

    /// How many of a `Map` state's items run AT ONCE: the bound the state's
    /// `MaxConcurrency` states, or the one its `MaxConcurrencyPath` resolves to, as the
    /// whole number the member carries. `0` is the member's documented default, "which
    /// places no limit".
    ///
    /// "MaxConcurrency ... an upper bound on how many invocations of the Iterator may run
    /// in parallel": the bound is READ as a whole number, and an execution whose bound is
    /// no value such a reading exists for, or is a negative one, fails at that state for
    /// it — a value that is no iteration count is not a bound to be guessed at, defaulted
    /// or clamped.
    ///
    /// WHICH whole numbers can be read, and what is wrong when none can, follow from
    /// WHERE the figure came from:
    ///
    /// * a figure the DEFINITION STATES is read as the 64-bit integer such a stated bound
    ///   is held in ([`STATED_BOUND_CEILING`]): one inside that integer is a bound the
    ///   definition was accepted under and bounds the iterations in flight, by itself or
    ///   — past the largest concurrency a run can have ([`MAX_MAP_CONCURRENCY`], the
    ///   range a Map Run reports its own through) — no more tightly than that ceiling
    ///   does, so the state runs under the ceiling;
    /// * a figure the state RESOLVED as it ran — a path into the execution's data, or an
    ///   expression computing one — is that run's own, which no definition could be held
    ///   to, so it is read as the integer the member itself carries
    ///   ([`MAX_MAP_CONCURRENCY`]);
    /// * a value no such reading exists for is reported as the parse failure of the
    ///   member the definition wrote ([`field_not_an_integer`]), and one a REFERENCE PATH
    ///   referred to names that path and the value beside it
    ///   ([`path_value_not_an_integer`]), since what a reader follows back there is the
    ///   execution's own data;
    /// * a figure that WAS read and is negative is reported under the bound's own member
    ///   whichever form named it ([`concurrency_cannot_be_negative`]), because what is
    ///   wrong with it is the figure rather than where it was written.
    fn map_concurrency(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        input: &Value,
        context: &Value,
    ) -> Result<usize, Failure> {
        let path_member = format!("{CONCURRENCY_MEMBER}Path");
        let (declared, path) = match (
            state.get(CONCURRENCY_MEMBER),
            state.get(path_member.as_str()),
        ) {
            (Some(declared), _) => (
                // The member is a figure in both languages, so a JSONata expression
                // written in it is held to that type.
                self.typed_field(
                    language,
                    declared,
                    CONCURRENCY_MEMBER,
                    super::super::jsonata::Kind::Number,
                    input,
                    context,
                )?,
                None,
            ),
            (None, Some(Value::String(reference))) => (
                member_path_value(&path_member, reference, input, context).map_err(runtime)?,
                Some(reference.as_str()),
            ),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )));
            }
            // "Its default value is 0, which places no limit on concurrency": every item
            // of a state that declares neither member runs at once.
            (None, None) => return Ok(0),
        };
        // A value of another JSON type the DEFINITION wrote is no figure the member can
        // carry at all, which is the definition's own type error and is reported as one.
        // (A figure a path referred to is the run's own data, and is read below.)
        if path.is_none() && !declared.is_number() {
            return Err(self.invalid_field_value(
                language,
                state.get(CONCURRENCY_MEMBER),
                CONCURRENCY_MEMBER,
                NON_NEGATIVE_NUMBER,
                &declared,
            ));
        }
        // Which integer the figure is read as: the one a STATED bound is held in, or the
        // member's own where the state resolved the figure as it ran.
        let stated = path.is_none() && state.get(CONCURRENCY_MEMBER).is_some_and(Value::is_number);
        let read = match stated {
            true => stated_bound(&declared),
            false => whole_number_below(&declared, RESOLVED_BOUND_CEILING),
        };
        let Some(bound) = read else {
            return Err(match path {
                Some(reference) => {
                    runtime(path_value_not_an_integer(&path_member, &declared, reference))
                }
                None => runtime(field_not_an_integer(CONCURRENCY_MEMBER, &declared)),
            });
        };
        // A bound past the largest concurrency a run can have bounds it no more tightly
        // than that ceiling does, which only a STATED figure can be: the reading above
        // admits no resolved figure past it.
        Self::non_negative_bound(bound.min(MAX_MAP_CONCURRENCY as f64), &declared)
    }

    /// The concurrency `bound`, read off `declared`, as the count of items that run at
    /// once — or the failure a NEGATIVE bound reports, under the bound's own member.
    ///
    /// "An upper bound on how many invocations of the Iterator may run in parallel":
    /// there is no state that runs fewer than none of its items at a time, so a figure
    /// that was read and is negative is no bound, whichever member the definition named
    /// it in ([`concurrency_cannot_be_negative`]).
    fn non_negative_bound(bound: f64, declared: &Value) -> Result<usize, Failure> {
        if bound < 0.0 {
            return Err(runtime(concurrency_cannot_be_negative(declared)));
        }
        Ok(bound as usize)
    }

    /// A numeric member of a state, from the literal member or from its `*Path` form.
    ///
    /// A value that is not a figure the member accepts is reported under the member the
    /// DEFINITION wrote, as a `Wait` state's members are
    /// ([`Interpreter::wait_duration`]): the two forms are alternatives, so naming the
    /// one the definition does not carry points a reader at the wrong field.
    pub(super) fn number_member(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        input: &Value,
        context: &Value,
        default: f64,
    ) -> Result<f64, Failure> {
        let path_member = format!("{member}Path");
        let (member, value) = match (state.get(member), state.get(&path_member)) {
            (Some(value), _) => (
                member.to_string(),
                // The member is a figure in both languages, so a JSONata expression
                // written in it is held to that type.
                self.typed_field(
                    language,
                    value,
                    member,
                    super::super::jsonata::Kind::Number,
                    input,
                    context,
                )?,
            ),
            (None, Some(Value::String(path))) => (
                path_member.clone(),
                expression(path, input, context).map_err(|failure| {
                    let Failure {
                        error,
                        cause,
                        also_answers,
                    } = failure;
                    Failure {
                        error,
                        cause: cause.map(|why| member_path_diagnosis(&path_member, path, why)),
                        also_answers,
                    }
                })?,
            ),
            (None, Some(other)) => {
                return Err(runtime(format!(
                    "'{path_member}' must be a JSONPath string, not {other}"
                )));
            }
            (None, None) => return Ok(default),
        };
        value
            .as_f64()
            .filter(|number| *number >= 0.0)
            .ok_or_else(|| {
                self.invalid_field_value(
                    language,
                    state.get(member.as_str()),
                    &member,
                    NON_NEGATIVE_NUMBER,
                    &value,
                )
            })
    }
}

/// The largest whole number ONE PAST the 64-bit integer a `MaxConcurrency` the
/// DEFINITION states is read as, which is what bounds that reading ([`stated_bound`]).
///
/// It is stated as the figure one past the integer's own largest value, because that
/// integer's largest value is not a figure a double holds exactly: a reading is bounded
/// by being BELOW this one rather than by being at most the largest.
const STATED_BOUND_CEILING: f64 = 9_223_372_036_854_775_808.0;

/// The same bound for a concurrency the state RESOLVED as it ran, which is read as the
/// integer the member itself carries ([`MAX_MAP_CONCURRENCY`]).
const RESOLVED_BOUND_CEILING: f64 = MAX_MAP_CONCURRENCY as f64 + 1.0;

/// The whole number a `MaxConcurrency` the DEFINITION STATES can be READ as, which is
/// the 64-bit integer such a bound is held in; `None` for a figure outside it, and for
/// one carrying a fraction.
///
/// A JSON number inside that integer is read as it was written; every other figure is
/// read through the double it was parsed as ([`whole_number_below`]), which is exact for
/// every whole number the integer holds.
fn stated_bound(value: &Value) -> Option<f64> {
    match value.as_i64() {
        Some(bound) => Some(bound as f64),
        None => whole_number_below(value, STATED_BOUND_CEILING),
    }
}

/// The whole number `value` can be READ as, where the integer doing the reading holds
/// every whole number below `ceiling` in magnitude; `None` for a value no such reading
/// exists for — one of another JSON type, one carrying a fraction, and one whose
/// magnitude that integer does not hold.
///
/// The bound is stated as the figure ONE PAST the largest the reading integer holds,
/// because the largest value of such an integer is not always a figure a double holds
/// exactly ([`STATED_BOUND_CEILING`]). The figure comes back as the double it was read
/// from rather than as an integer type, for the same reason: the reading is only ever
/// compared, clamped and counted with.
fn whole_number_below(value: &Value, ceiling: f64) -> Option<f64> {
    let number = value.as_f64()?;
    let readable =
        number.is_finite() && number.fract() == 0.0 && number < ceiling && number >= -ceiling;
    readable.then_some(number)
}

/// The value the state member `member` selects through `path`, with a path that
/// selected nothing reported under that member ([`member_path_diagnosis`]).
fn member_path_value(
    member: &str,
    path: &str,
    input: &Value,
    context: &Value,
) -> Result<Value, String> {
    expression(path, input, context).map_err(|failure| {
        member_path_diagnosis(member, path, failure.cause.unwrap_or_default())
    })
}

/// Whether `value` is a JSON number a member holding a whole number of seconds can be
/// READ as one from — an integer, of either sign.
///
/// A value that is not is one such a member cannot be parsed from at all, which is the
/// failure [`parameter_not_a_long`] words; a whole number outside the range the member
/// allows was read and is reported as the value it is.
fn is_whole_number(value: &Value) -> bool {
    value.is_i64() || value.is_u64()
}

/// What a task fails with when the turn that had already STARTED a call reached another
/// one instead ([`Interpreter::perform_call`]).
///
/// It is unreachable by construction: a turn is put down only where walking it again
/// cannot ask for anything else, so the call it reaches the second time is the call it
/// started. If it ever were not, the work of the started call has already been performed
/// — so this one is NOT made, because making it would perform the task's work a second
/// time, and the task fails with the name every other call this emulator declines to make
/// carries ([`RUNTIME_ERROR`], which is neither retried nor caught).
fn started_another_call(call: &super::super::integration::Call) -> super::super::integration::Failed {
    super::super::integration::Failed {
        error: RUNTIME_ERROR.to_string(),
        cause: format!(
            "{} is not the call this item had already started, whose work has been performed, so \
             nothing further was run for it",
            call.described()
        ),
    }
}

/// The event an activity's task records for having been CLOSED without a result: the
/// language's own timeout event where a bound the state declared ran out, and the
/// activity's failure event for a failure a worker reported.
///
/// It is decided by WHICH of the two closed the task
/// ([`super::CallbackReply::by_the_clock`]) and never by the error name, which a worker
/// reporting a failure chooses.
fn activity_closure(by_the_clock: bool) -> &'static str {
    match by_the_clock {
        true => super::super::EVENT_ACTIVITY_TIMED_OUT,
        false => super::super::EVENT_ACTIVITY_FAILED,
    }
}

/// Whether `event_type` is the event a STATE records for having failed.
///
/// The event a state's own expression failing writes belongs before it
/// ([`Interpreter::push_evaluation_failure`]): the expression is evaluated first, and
/// the state failing is what came of it. The list is the one event per state TYPE that
/// reports its failure, plus the one a `Map` iteration reports.
fn records_a_failure(event_type: &str) -> bool {
    [
        super::super::EVENT_PARALLEL_STATE_FAILED,
        super::super::EVENT_MAP_STATE_FAILED,
        super::super::EVENT_MAP_ITERATION_FAILED,
        super::super::EVENT_TASK_FAILED,
        super::super::EVENT_TASK_SUBMIT_FAILED,
        super::super::EVENT_ACTIVITY_FAILED,
        super::super::EVENT_LAMBDA_FUNCTION_FAILED,
    ]
    .contains(&event_type)
}

/// Where in a definition the `Condition` of the `Choice` rule at `index` is written.
///
/// RECORDED: the field a diagnosis names is the path to it through the state's own
/// members, and a rule's position in the `Choices` array is what tells one rule from
/// the next — so a state of many rules says which of them the execution failed on.
fn choice_condition_field(index: usize) -> String {
    choice_rule_field(index, "Condition")
}

/// Where in a definition the member `member` of the `Choice` rule at `index` is
/// written: the rule's own place in the `Choices` array, and the member inside it.
///
/// A rule carries members of its own — its `Condition`, and the `Assign` and `Output` a
/// JSONata rule may declare — so a diagnosis about one locates it in the RULE rather
/// than in the state, which is what tells it from the state's own member of that name.
fn choice_rule_field(index: usize, member: &str) -> String {
    super::super::jsonata::member_field(&format!("Choices[{index}]"), member)
}

