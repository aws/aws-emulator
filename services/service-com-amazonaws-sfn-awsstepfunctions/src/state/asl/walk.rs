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

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value};

use super::*;

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

    /// Record that a service-integration call this walk waited for took `waited`.
    ///
    /// The walk is BLOCKED while a call it asked for is in flight — in another
    /// service's handler, which for a Lambda invocation runs a caller's own code — so
    /// that span is not interpretation and is charged to nothing the interpretation is
    /// judged by. What it IS charged to is the allowance that bounds the waiting one
    /// run may do ([`MAX_CALL_WAIT_DURATION`]), which the next call's own wait is
    /// clamped to what is left of ([`Interpreter::call_within`]): the thread one run
    /// holds is bounded, and a call that does not answer inside the span it was given
    /// is the task's own timeout rather than the execution's end.
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
        if self.depth >= MAX_NESTING_DEPTH {
            return Branch {
                outcome: Err(runtime(format!(
                    "the definition nests 'Map' iterations and 'Parallel' branches more than the \
                     {MAX_NESTING_DEPTH} levels deep this emulator runs, so nothing was run for it"
                ))),
                end: at,
                suspended: false,
            };
        }
        self.depth += 1;
        if iteration {
            self.map_depth += 1;
        }
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
        (at > when || (inclusive && at == when)).then(|| {
            runtime(
                "a branch of the enclosing 'Parallel' state failed, so this branch was stopped"
                    .to_string(),
            )
        })
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
        let mut name = start_at.to_string();
        let mut current = input;
        let mut at = at;
        loop {
            // The definition's own `TimeoutSeconds` comes FIRST: an execution still
            // going when it falls due has ENDED there, so it makes no further
            // transition — and the emulator's own ceilings below never decide the
            // outcome of a run the definition already bounded.
            if self.past_timeout(at) {
                return Branch {
                    outcome: Err(Failure {
                        error: Some(super::TIMEOUT_ERROR.to_string()),
                        cause: None,
                    }),
                    end: at,
                    suspended: false,
                };
            }
            // A branch of the enclosing `Parallel` state has already FAILED, so the
            // state is failing and the work that had not begun does not begin: a state
            // due after that instant is not entered at all.
            if let Some(stopped) = self.stopped_before_entering(at) {
                return Branch {
                    outcome: Err(stopped),
                    end: at,
                    suspended: false,
                };
            }
            if let Some(failure) = self.out_of_work() {
                return Branch {
                    outcome: Err(failure),
                    end: at,
                    suspended: false,
                };
            }
            if self.budget == 0 {
                return Branch {
                    outcome: Err(runtime(format!(
                        "the execution made more than the {MAX_STATE_TRANSITIONS} state \
                         transitions this emulator runs, so it was stopped"
                    ))),
                    end: at,
                    suspended: false,
                };
            }
            self.budget -= 1;
            let Some(state) = states.get(&name).and_then(Value::as_object) else {
                return Branch {
                    outcome: Err(runtime(format!(
                        "state '{name}' is not declared by the state machine"
                    ))),
                    end: at,
                    suspended: false,
                };
            };
            // The raw input of the state about to run, kept for the one use a
            // FAILURE has for it: the step a redrive continues from is that state
            // with the data it saw ([`Run::stopped_at`]).
            let raw = current.clone();
            let step = match self.step(&name, state, current, at, scope) {
                Ok(step) => step,
                Err(fault) => {
                    // "It continues the failed execution from the unsuccessful step
                    // and uses the same input": the step is the TOP-LEVEL state of
                    // the definition the execution stopped at, so a failure a nested
                    // walk reports is recorded as the state that encloses it — the
                    // frame that is at depth zero.
                    if self.depth == 0 {
                        self.failed_at = Some(RedriveStep {
                            state_name: name.clone(),
                            state_input: raw.to_string(),
                        });
                    }
                    return Branch {
                        outcome: Err(fault.failure),
                        end: fault.at,
                        suspended: false,
                    };
                }
            };
            at = step.at;
            current = step.value;
            // The document this state produced is measured HERE, at the state that
            // produced it: a definition that grows what it carries is stopped at the
            // first state past what a state's output may hold, rather than after the
            // whole run.
            if let Err(failure) = measure_payload(&current, &name) {
                return Branch {
                    outcome: Err(failure),
                    end: at,
                    suspended: false,
                };
            }
            match step.next {
                Next::EndBranch => {
                    return Branch {
                        outcome: Ok(current),
                        end: at,
                        suspended: false,
                    };
                }
                // The state scheduled an activity task, or started the job of a
                // run-a-job integration: the walk stops here, and `run` reports what
                // it is waiting for rather than an outcome. A nested list reports the
                // suspension to the state that encloses it, which is what parks a
                // `Parallel` branch at the job one of its states started.
                Next::Suspended => {
                    return Branch {
                        outcome: Ok(current),
                        end: at,
                        suspended: true,
                    };
                }
                Next::Named(next) => name = next,
            }
        }
    }

    /// Run one state: `raw` is its input, `at` the offset it is entered at.
    ///
    /// A diagnosis the interpreter raises about this state's own expression is
    /// reported under the state's name and the id of the event it was ENTERED at
    /// ([`diagnosed`]), which is read here — before the state records anything — so
    /// the state that failed can be followed back to its place in the history.
    pub(super) fn step(
        &mut self,
        name: &str,
        state: &Map<String, Value>,
        raw: Value,
        at: Duration,
        scope: &Scope,
    ) -> Result<StateStep, Fault> {
        let entered = self.next_event_id();
        // Where this state's own events begin, so a failure of its EXPRESSIONS is
        // recorded at the point it happened rather than after the event that records
        // the state failing ([`Interpreter::push_evaluation_failure`]).
        let from = self.steps.len();
        // And nothing another state's expressions left behind is read as this state's:
        // the field an expression failed in is remembered for the state being executed
        // ([`super::super::jsonata::take_failed_field`]), so this state starts with
        // none.
        super::super::jsonata::clear_failed_field();
        match self.run_state(name, state, raw, at, scope, entered) {
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
        let location = super::super::jsonata::take_failed_field();
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
        self.steps.insert(at, evaluation_failed);
    }

    /// The id the next history event this walk records will carry.
    ///
    /// The walk's events are numbered in order from [`Interpreter::first_event_id`],
    /// so the id of the one about to be recorded is that first id plus the ones
    /// recorded before it. A walk that CONTINUES an execution from a reply reports
    /// only the steps that FOLLOW the point the reply was applied at
    /// ([`Resumed::index`]) — the ones before it were recorded by the call that
    /// suspended the execution — so those are the steps counted here.
    pub(super) fn next_event_id(&self) -> i64 {
        let recorded = self
            .steps
            .len()
            .saturating_sub(self.resumed.map_or(0, |resumed| resumed.index));
        self.first_event_id
            .saturating_add(i64::try_from(recorded).unwrap_or(i64::MAX))
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

        let context = self.context_for(scope, name, at);
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
                let waited = self
                    .wait_duration(state, language, &effective, &context, at)
                    .map_err(|failure| wait_failure(name, failure).at(at))?;
                let at = at + waited;
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
                // same for `Assign`: a JSONata `Choice` puts out what the SELECTED
                // rule's own members decided, and what the state's own decided only
                // where no rule matched ([`Interpreter::choice_output`]). A JSONPath
                // rule carries neither, so that language's state puts out what every
                // other state does.
                let value = match language {
                    JSONATA => self
                        .choice_output(state, chosen.rule.as_ref(), &effective, &context)
                        .map_err(|failure| failure.at(at))?,
                    _ => self
                        .produced(state, language, &raw, &effective, None, &context, false)
                        .map_err(|failure| failure.at(at))?,
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
                Err(Failure { error, cause }.at(at))
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
    /// JSONPath places the state's result through `ResultSelector`, `ResultPath` and
    /// `OutputPath`; JSONata evaluates the state's `Assign` and then its `Output`.
    /// One seam for both, so every state type's output follows the rule of the
    /// language that state is written in.
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
            Some(result) => self.place_result(state, raw, result, context, with_selector),
            None => self.output_value(state, effective.clone(), context),
        }
    }

    /// A JSONata state's `Assign`, and then the `Output` those assignments feed.
    ///
    /// "Assign ... store values in variables": the assignments are evaluated FIRST,
    /// so a variable this state assigns is in scope for its own `Output`, and they
    /// outlive the state — which is what a later state reads them through.
    ///
    /// A node that declares no `Output` puts out what the state's own data flow owes
    /// at that point: "when a state reports an error and it matches a catcher, the
    /// state's output is the error output", so a CATCHER that declares none puts out
    /// the error output it was handed; a state whose work produced a result puts out
    /// that result; and a state whose work produced none carries its own input through.
    ///
    /// A state that FAILS assigns nothing: where the `Output` those assignments feed
    /// cannot be built, the scope is put back as the state found it, so the states
    /// after a caught failure read what reaching the failed state had assigned and no
    /// more — and the event the state is left by reports no assignment of its own.
    pub(super) fn jsonata_output(
        &mut self,
        state: &Map<String, Value>,
        input: &Value,
        result: Option<&Value>,
        error_output: Option<&Value>,
        context: &Value,
    ) -> Result<Value, Failure> {
        // ONE allowance for the `Assign` and the `Output` it feeds: they are the
        // expressions of one template, so they spend from the same budget.
        let mut budget = self.jsonata_budget();
        let scope = self.variables.clone();
        let assigned = self.assigned.clone();
        let produced = self
            .jsonata_assign(state, ASSIGN_MEMBER, input, result, error_output, context, &mut budget)
            .and_then(|()| match state.get("Output") {
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
                    super::super::jsonata::render(
                        template,
                        ARGUMENTS_MEMBER,
                        &bindings,
                        &mut self.jsonata_budget(),
                    )
                    .map_err(jsonata_failure)
                }
                None => Ok(effective.clone()),
            },
            _ => match state.get("Parameters") {
                Some(template) => payload_template(template, effective, context),
                None => Ok(effective.clone()),
            },
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
        // A nested state list is walked to its own end inside the enclosing state's
        // turn, so there is nowhere for one of its states to wait: a `Map` iteration
        // or a `Parallel` branch that schedules a task is not run, rather than run as
        // though the task had already answered.
        if self.depth > 0 {
            return Err(runtime(format!(
                "state '{name}' schedules an activity task inside a 'Map' iteration or a \
                 'Parallel' branch, which this emulator does not wait at, so nothing was run \
                 for it"
            ))
            .at(at));
        }
        // Both bounds are read from the state's effective INPUT, which is what their
        // `*Path` forms are reference paths into — not from the `Parameters` document
        // built below.
        let timeout_seconds = self
            .task_seconds(state, language, "TimeoutSeconds", &effective, context)
            .map_err(|failure| failure.at(at))?;
        let heartbeat_seconds = self
            .task_seconds(state, language, "HeartbeatSeconds", &effective, context)
            .map_err(|failure| failure.at(at))?;
        // "Parameters ... Used to create a collection of key-value pairs that will be
        // passed as input" — `Arguments` in JSONata: the document the template builds
        // IS what the worker is handed, and without one the state's effective input
        // is.
        let input = self
            .task_request(state, language, &effective, context)
            .map_err(|failure| failure.at(at))?;
        self.push_entered("TaskStateEntered", name, at, raw);
        self.push("ActivityScheduled", name, at);
        // A worker has already answered THIS state's task: the reply is that task's
        // result — "the JSON output of the task" — so the state's own result-side data
        // flow (`ResultSelector`, `ResultPath`, `OutputPath`) is applied to it and the
        // walk carries on to the state's `Next`, instead of suspending again. The
        // steps up to here are what the call that suspended the walk already recorded,
        // which is what [`Resumed::index`] marks.
        let replied = match &self.reply {
            Some(reply) if reply.state_name == name => self.reply.take(),
            _ => None,
        };
        if let Some(reply) = replied {
            self.resumed = Some(Resumed {
                index: self.steps.len(),
                at,
            });
            self.push("ActivitySucceeded", name, at);
            let value = self
                .produced(
                    state,
                    language,
                    raw,
                    &effective,
                    Some(reply.result),
                    context,
                    true,
                )
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
    pub(super) fn task_seconds(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        input: &Value,
        context: &Value,
    ) -> Result<Option<u64>, Failure> {
        let path_member = format!("{member}Path");
        let (wrote, declared) = match (state.get(member), state.get(path_member.as_str())) {
            (Some(declared), _) => (
                member.to_string(),
                self.literal(language, declared, member, input, context)?,
            ),
            (None, Some(Value::String(path))) => (
                path_member.clone(),
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
            _ => Err(self.invalid_field_value(
                language,
                state.get(wrote.as_str()),
                &wrote,
                POSITIVE_SECONDS,
                declared.as_ref().unwrap_or(&Value::Null),
            )),
        }
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

    /// The context object (`$$`) a state reads: the run's own members, plus the state
    /// it is being read in and the `Map` item being iterated, when there is one.
    pub(super) fn context_for(&self, scope: &Scope, name: &str, at: Duration) -> Value {
        let mut context = self.base_context.clone();
        if let Some(object) = context.as_object_mut() {
            object.insert(
                "State".to_string(),
                serde_json::json!({
                    "Name": name,
                    "EnteredTime": iso(self.start + at),
                    "RetryCount": self.retry_count,
                }),
            );
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
    pub(super) fn push_entered(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        input: &Value,
    ) {
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
    pub(super) fn push_detailed(
        &mut self,
        event_type: &'static str,
        state_name: &str,
        after: Duration,
        details: super::super::EventDetails,
    ) {
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

    /// The event one attempt of a run-a-job task failed to START its job with: "the
    /// task submission failed", which is a different event from the failure of a task
    /// whose work ran — and the same event a request-response task records when ITS
    /// call failed, because for that pattern the call IS the work.
    pub(super) fn push_submit_failure(
        &mut self,
        state_name: &str,
        work: &WorkEvents,
        failure: &Failure,
        after: Duration,
        call: &super::super::integration::Call,
    ) {
        if call.job.is_none() || failure.name() == TIMEOUT_ERROR {
            self.push_task_failure(state_name, work, failure, after);
            return;
        }
        let mut details = work.naming();
        details.error = failure.error.clone();
        details.cause = failure.cause.clone();
        self.push_detailed(
            super::super::EVENT_TASK_SUBMIT_FAILED,
            state_name,
            after,
            details,
        );
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
                let value = expression(path, input, context)?;
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
                return Err(self.invalid_field_value(
                    language,
                    state.get(member),
                    member,
                    ISO_INSTANT,
                    &timestamp,
                ));
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
                // The rule that matched is the one whose own `Assign` and `Output`
                // apply; a JSONPath rule carries neither. Its own place in the
                // definition is where those members are written
                // ([`choice_rule_field`]), which is what a diagnosis about one names.
                rule: match language {
                    JSONATA => Some((index, rule.clone())),
                    _ => None,
                },
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
    /// The `Output` those assignments feed is the selected rule's where it declares
    /// one and the state's otherwise, and a state and rule that declare neither put
    /// the state's own input through.
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
        let produced = self.choice_assign_and_output(state, rule, input, context);
        if produced.is_err() {
            self.variables = scope;
            self.assigned = assigned;
        }
        produced
    }

    /// [`Interpreter::choice_output`] without the roll-back, which that one adds.
    fn choice_assign_and_output(
        &mut self,
        state: &Map<String, Value>,
        rule: Option<&(usize, Map<String, Value>)>,
        input: &Value,
        context: &Value,
    ) -> Result<Value, Failure> {
        // ONE allowance for the `Assign` that applies and the `Output` it feeds: they
        // are the expressions of one visit to this state.
        let mut budget = self.jsonata_budget();
        // The `Assign` of whichever decided the transition: the rule that matched, or
        // the state itself where none did. A member of a RULE is written inside that
        // rule, which is where a diagnosis about it locates it.
        let (owner, at) = match rule {
            Some((index, rule)) => (rule, choice_rule_field(*index, ASSIGN_MEMBER)),
            None => (state, ASSIGN_MEMBER.to_string()),
        };
        self.jsonata_assign(owner, &at, input, None, None, context, &mut budget)?;
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
        match template {
            None => Ok(input.clone()),
            Some(template) => {
                let rendered = {
                    let bindings = self.bindings(input, context, None, None);
                    super::super::jsonata::render(&template, &at, &bindings, &mut budget)
                };
                rendered.map_err(jsonata_failure)
            }
        }
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
        let within = self
            .task_seconds(state, language, "TimeoutSeconds", &effective, context)
            .map_err(|failure| self.task_fault(name, &work, failure, at))?
            .map(Duration::from_secs);
        let heartbeat = self
            .task_seconds(state, language, "HeartbeatSeconds", &effective, context)
            .map_err(|failure| self.task_fault(name, &work, failure, at))?;
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
        // One execution parks at ONE job: the record that holds it names the state
        // that started it, and the poll that resolves it resumes that one state. A
        // second concurrent job is therefore not started either.
        if call.job.is_some() && self.parked.is_some() {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' starts a second run-a-job integration while this execution \
                     is already parked at one, which this emulator does not park at, so nothing \
                     was run for it"
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
        // only wait for where it is the walk itself that suspends: a nested state
        // list is walked to its own end inside the enclosing state's turn, so a `Map`
        // iteration or a `Parallel` branch whose state would wait for a token is not
        // run rather than run as though the token had already been answered. It is
        // the same bound an activity task is held to, for the same reason.
        if call.callback && self.depth > 0 {
            return Err(self.task_fault(
                name,
                &work,
                runtime(format!(
                    "state '{name}' waits for a task token inside a 'Map' iteration or a \
                     'Parallel' branch, which this emulator does not wait at, so nothing was \
                     run for it"
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
                return (me.answered_callback(name, &work, at, reply.outcome), at);
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
                let failure = Failure {
                    error: Some(refused.error),
                    cause: Some(refused.cause),
                };
                me.push_task_failure(name, &work, &failure, at);
                return (Err(failure), at);
            }
            // The interpretation's own work allowance is consulted HERE as well as at
            // each transition: a run that has asked for more work than it may do makes
            // no further call (and a `Retry` cannot keep making them).
            if let Some(failure) = me.out_of_work() {
                me.push_submit_failure(name, &work, &failure, at, &call);
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
                };
                me.push_submit_failure(name, &work, &failure, at, &call);
                return (Err(failure), at);
            }
            let began = super::super::now();
            // A CALLBACK task's token goes out WITH this call, and it answers the task
            // from that instant: the work it reaches may report the task's outcome from
            // inside this very call. So it is handed over BEFORE the call is made, which
            // is what lets a `SendTask*` quoting it be answered while the call is still
            // in flight ([`super::super::integration::Tasks::hand_over`]).
            if let Some(token) = &token {
                me.tasks.hand_over(token, &me.execution_arn, name);
            }
            let answered = me.tasks.perform(&call, &request, me.call_within(within));
            // The call has come back, so the handover is over: what it carries, if
            // anything answered it, is this task's own outcome.
            let replied = token
                .as_ref()
                .and_then(|token| me.tasks.settle_handover(token));
            let spent = super::super::now()
                .duration_since(began)
                .unwrap_or_default();
            // The span the walk spent BLOCKED in that call is not interpretation, so it
            // is charged to the allowance that bounds the waiting one run may do
            // ([`Interpreter::spent_calling`]) and to nothing else.
            me.spent_calling(spent);
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
            let attempt = match answered {
                // The result is a document a state carries, so it is measured
                // against the same ceiling every state's document is measured
                // against.
                Ok(answer) => {
                    let result = super::super::integration::reported_result(&call, &answer);
                    match measure_payload(&result, work.produced_by()) {
                        Ok(_bytes) => Ok(result),
                        Err(failure) => Err(failure),
                    }
                }
                Err(Failed { error, cause }) => Err(Failure {
                    error: Some(error),
                    cause: Some(cause),
                }),
            };
            // What the ANSWER to this attempt's token decided, for a callback task whose
            // token was answered while its call was in flight: it is the attempt's
            // outcome, read below once the submission has been recorded.
            let mut finished: Option<Result<Value, Failure>> = None;
            match (&attempt, call.job) {
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
                        me.push_submit_failure(name, &work, &failure, at, &call);
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
                        timeout_seconds: within.map(|within| within.as_secs()),
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
                // is the task's, and an answer the token happened to collect before it
                // (the handover above ended either way) reports an outcome of work this
                // state never handed over.
                (Err(failure), _) => {
                    me.push_submit_failure(name, &work, failure, at, &call)
                }
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
        let (processor, distributed) = map_processor(name, state).map_err(|failure| failure.at(at))?;
        let (processor_states, processor_start) =
            states_of(processor, &format!("the 'ItemProcessor' of state '{name}'"))
                .map_err(|failure| failure.at(at))?;
        // "A user-defined or an auto-generated string that identifies a Map state":
        // the field the Map Run's own ARN carries, and the field the ARN of every
        // child execution it starts carries too, so one reading serves both.
        let label = state
            .get("Label")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
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
            .number_member(state, language, "MaxConcurrency", &effective, context, 0.0)
            .map_err(|failure| failure.at(at))? as usize;
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
        let tolerance_declared = [
            "ToleratedFailureCount",
            "ToleratedFailureCountPath",
            "ToleratedFailurePercentage",
            "ToleratedFailurePercentagePath",
        ]
        .iter()
        .any(|member| state.contains_key(*member));

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
        let denied = self.tasks.authorize(&call, &request)?;
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
    /// returned offset reports. The ITEMS themselves are interpreted one after the
    /// other, because the interpreter is a single walk — the result is the same, and
    /// no iteration can observe another's.
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

    /// [`Interpreter::run_map_items`]' own work, inside the stop it restores.
    fn walk_map_items(
        &mut self,
        plan: &MapPlan<'_>,
        effective: &Value,
        context: &Value,
        at: Duration,
        tally: &mut MapTally,
    ) -> Attempt {
        *tally = MapTally::default();
        let batch = if plan.concurrency == 0 {
            plan.items.len().max(1)
        } else {
            plan.concurrency
        };
        let mut results: Vec<Value> = Vec::new();
        let mut results_bytes = 0usize;
        let mut failures: u64 = 0;
        // The iteration that failed FIRST, with the offset it failed at: the failure
        // an intolerant state reports, and the instant its other iterations stopped.
        let mut first_failure: Option<(Failure, Duration)> = None;
        // Where each iteration's own events begin in this walk's list, so an iteration
        // the state's failure aborted can be cut back to the instant it failed.
        let mut units: Vec<Unit> = Vec::new();
        // "ToleratedFailureCount" and "ToleratedFailurePercentage" are what lets a
        // state go on iterating past a failed item; a state that declares neither
        // tolerates none, so the first failure is the state's own and the iterations
        // that had not finished stop there.
        let tolerant = plan.distributed || plan.tolerance_declared;
        // Which of the child executions the attempt being CONTINUED ran have been
        // claimed by an item of this one, so two items never keep the same execution
        // ([`Interpreter::kept_child`]).
        let mut claimed = vec![false; plan.resumed.map_or(0, <[_]>::len)];
        let mut at = at;
        let mut index = 0usize;
        for chunk in plan.items.chunks(batch) {
            let batch_start = at;
            let mut batch_end = at;
            for item in chunk {
                // Every item's turn is charged to the step budget BEFORE the work it
                // needs is done, so the per-item work (the context the iteration
                // reads, the `ItemSelector` it is given) stops with the budget rather
                // than being done for every remaining item of a long array.
                if let Some(failure) = self.out_of_work() {
                    return (Err(failure), batch_end);
                }
                if self.budget == 0 {
                    return (
                        Err(runtime(format!(
                            "the execution made more than the {MAX_STATE_TRANSITIONS} state \
                             transitions this emulator runs, so it was stopped"
                        ))),
                        batch_end,
                    );
                }
                // `$$.Map.Item` is what the iteration reads the item and its
                // position through, both in the `ItemSelector` evaluated here and in
                // every state of the iteration.
                let map_item = serde_json::json!({ "Index": index, "Value": item });
                let item_context = with_map_item(context, &map_item);
                let scope = Scope {
                    map_item: Some(map_item),
                };
                let item_input = match plan.selector {
                    Some(template) => match self.render_selector(plan.language, template, effective, &item_context) {
                        Ok(value) => value,
                        Err(failure) => return (Err(failure), batch_end),
                    },
                    None => item.clone(),
                };
                // "Step Functions preserves the results and execution history of the
                // successful steps, and doesn't rerun these steps when you redrive an
                // execution": an item whose child workflow execution SUCCEEDED in the
                // attempt being continued is not run a second time. Its result stands
                // as this attempt's, the execution it ran as is reported unchanged —
                // and so is left untouched by the store — and the per-item work it
                // already did is therefore not repeated.
                let document = item_input.to_string();
                if let Some((position, kept)) = self.kept_child(plan, index, &document, &claimed) {
                    claimed[position] = true;
                    let value = kept
                        .output
                        .as_deref()
                        .and_then(|output| serde_json::from_str(output).ok())
                        .unwrap_or(Value::Null);
                    match measure_payload(&value, UNNAMED_PRODUCER) {
                        Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                        Err(failure) => return (Err(failure), batch_end),
                    }
                    results.push(value);
                    tally.items.push(MapItemOutcome {
                        finished_at: batch_start,
                        failed: false,
                    });
                    tally.children.push(StartedChildExecution {
                        name: kept.name.clone(),
                        arn: kept.arn.clone(),
                        input: kept.input.clone(),
                        output: kept.output.clone(),
                        error: None,
                        cause: None,
                        status: super::super::EXECUTION_STATUS_SUCCEEDED,
                        started_at: batch_start,
                        finished_at: batch_start,
                        item_count: kept.item_count,
                        // The dates the execution it ran as already reports, which do
                        // not move by being kept.
                        kept: Some((kept.start_date, kept.stop_date)),
                    });
                    index += 1;
                    continue;
                }
                // A DISTRIBUTED Map "starts a child workflow execution for each
                // item", so this item's iteration runs as an execution of its own:
                // its ARN is minted HERE, before the iteration, because the item's
                // states read it through `$$.Execution.Id` — and the context they
                // read is the CHILD's, not the parent's. An inline Map starts no
                // execution, so its iteration keeps the parent's context.
                let child = plan.distributed.then(|| {
                    let name = self.fresh_child_name();
                    let arn = super::super::execution_arn(
                        &self.region,
                        &format!("{}/{}", self.machine_name, plan.label),
                        &name,
                    );
                    (name, arn)
                });
                let enclosing_context = child.as_ref().map(|(name, arn)| {
                    let child_context =
                        self.child_context(arn, name, &item_input, batch_start);
                    std::mem::replace(&mut self.base_context, child_context)
                });
                // The child's own `input` is the document this iteration was handed,
                // kept before the walk takes it: "the string that contains the JSON
                // input data" of that execution.
                let child_input = child.as_ref().map(|_| item_input.to_string());
                // One iteration per item: it is STARTED here, carrying the index of
                // the item it was handed, and records its own outcome below — which is
                // how a caller observes the fan-out, and how many items ran at once.
                let from = self.steps.len();
                self.push_detailed(
                    super::super::EVENT_MAP_ITERATION_STARTED,
                    plan.name,
                    batch_start,
                    iteration_details(index),
                );
                units.push(Unit {
                    from,
                    map_runs_from: self.map_runs.len(),
                    aborted: Some(Step {
                        event_type: super::super::EVENT_MAP_ITERATION_ABORTED,
                        state_name: Some(plan.name.to_string()),
                        after: Duration::ZERO,
                        terminal: false,
                        details: iteration_details(index),
                    }),
                });
                let branch = self.nested_walk(
                    plan.states,
                    plan.start_at,
                    item_input,
                    batch_start,
                    &scope,
                    true,
                );
                if let Some(enclosing) = enclosing_context {
                    self.base_context = enclosing;
                }
                batch_end = batch_end.max(branch.end);
                if let Some((name, arn)) = child {
                    // The child execution this item ran as, as it ended: an item that
                    // produced a result is an execution that SUCCEEDED with that
                    // result as its output, and one whose iteration failed is an
                    // execution that FAILED with that failure's own error and cause.
                    let (status, output, error, cause) = match &branch.outcome {
                        Ok(value) => (
                            super::super::EXECUTION_STATUS_SUCCEEDED,
                            Some(value.to_string()),
                            None,
                            None,
                        ),
                        Err(failure) => (
                            super::super::EXECUTION_STATUS_FAILED,
                            None,
                            failure.error.clone(),
                            failure.cause.clone(),
                        ),
                    };
                    tally.children.push(StartedChildExecution {
                        name,
                        arn,
                        input: child_input.unwrap_or_default(),
                        output,
                        error,
                        cause,
                        status,
                        started_at: batch_start,
                        finished_at: branch.end,
                        // One item per child, or the batch's own count for a state
                        // that declares an `ItemBatcher`: see
                        // [`StartedChildExecution::item_count`].
                        item_count: match plan.batched {
                            true => i32::try_from(super::super::item_batcher::item_count(item))
                                .unwrap_or(i32::MAX),
                            false => 1,
                        },
                        // This item RAN in this attempt, so the execution it ran as is
                        // one this attempt started.
                        kept: None,
                    });
                }
                let outcome_event = match &branch.outcome {
                    Ok(_) => super::super::EVENT_MAP_ITERATION_SUCCEEDED,
                    Err(_) => super::super::EVENT_MAP_ITERATION_FAILED,
                };
                self.push_detailed(
                    outcome_event,
                    plan.name,
                    branch.end,
                    iteration_details(index),
                );
                match branch.outcome {
                    Ok(value) => {
                        // The array the iterations are building is a document this
                        // state will put out, so it is bounded as it grows rather
                        // than only once it is whole.
                        match measure_payload(&value, UNNAMED_PRODUCER) {
                            Ok(bytes) => results_bytes = results_bytes.saturating_add(bytes),
                            Err(failure) => return (Err(failure), batch_end),
                        }
                        // The array the iterations build IS this state's result, so it
                        // outgrowing the limit is the same quota failure a single
                        // iteration past it raises, worded the one way the service
                        // words it: the state being executed is what names itself in
                        // it ([`result_too_large`], named by [`diagnosed`] or by the
                        // catcher that takes it over).
                        if results_bytes > MAX_PAYLOAD_BYTES {
                            return (
                                Err(named(DATA_LIMIT_ERROR, result_too_large(UNNAMED_PRODUCER))),
                                batch_end,
                            );
                        }
                        results.push(value);
                        tally.items.push(MapItemOutcome {
                            finished_at: branch.end,
                            failed: false,
                        });
                    }
                    Err(failure) => {
                        failures += 1;
                        tally.items.push(MapItemOutcome {
                            finished_at: branch.end,
                            failed: true,
                        });
                        if first_failure
                            .as_ref()
                            .is_none_or(|(_, when)| branch.end < *when)
                        {
                            first_failure = Some((failure, branch.end));
                        }
                        // A state that tolerates no failure has already failed, so the
                        // iterations running BESIDE this one in the same batch are
                        // stopped at the instant it failed, exactly as the items it had
                        // not started yet are never started
                        // ([`Interpreter::stopped_by_a_sibling`]).
                        if !tolerant {
                            let stop =
                                first_failure.as_ref().map_or(branch.end, |(_, when)| *when);
                            self.cancel_after = Some(
                                self.cancel_after.map_or(stop, |enclosing| enclosing.min(stop)),
                            );
                        }
                    }
                }
                index += 1;
            }
            at = batch_end;
            // A state that tolerates no failure has already failed, so the items it
            // had not started yet are never started.
            if failures > 0 && !tolerant {
                break;
            }
        }

        if failures == 0 {
            return (Ok(Value::Array(results)), at);
        }
        if super::super::within_tolerated_failures(
            i64::try_from(failures).unwrap_or(i64::MAX),
            i64::try_from(plan.items.len()).unwrap_or(i64::MAX),
            i64::try_from(plan.tolerated_count).unwrap_or(i64::MAX),
            plan.tolerated_percentage,
        ) {
            return (Ok(Value::Array(results)), at);
        }
        // A Distributed Map reports its own tolerated-failure error; an inline Map
        // has no threshold to exceed, so the iteration's own failure is the state's.
        if tolerant {
            return (
                Err(named(
                    TOLERATED_FAILURE_ERROR,
                    format!(
                        "{failures} of {} items failed, more than this 'Map' state tolerates",
                        plan.items.len()
                    ),
                )),
                at,
            );
        }
        let Some((failure, when)) = first_failure else {
            return (Err(runtime("a 'Map' iteration failed".to_string())), at);
        };
        self.abort_units(when, &units);
        (Err(failure), when)
    }

    /// Where the concurrent unit that PARKED carries on from, for a walk continuing
    /// an execution whose job has ended: the next concurrent state down, else the
    /// state that started the job itself.
    pub(super) fn resume_entry(&self) -> Option<(String, Value)> {
        if let Some(frame) = self.resume_frames.first() {
            return Some((frame.state_name.clone(), frame.state_input.clone()));
        }
        let reply = self.job_reply.as_ref()?;
        Some((
            reply.state_name.clone(),
            serde_json::from_str(&reply.state_input).unwrap_or(Value::Null),
        ))
    }

    /// Stop the branches or iterations of a state that has FAILED, at the instant
    /// `when` it failed.
    ///
    /// A state whose concurrent work has already failed does not go on running the
    /// rest of it: each unit is cut back to the events that were due at or before that
    /// instant, the state it was in when the failure reached it records that type's own
    /// aborted event, and the unit itself records the one its kind has. `units` is
    /// where each unit's events begin in this walk's list, in the order they were run,
    /// so they are cut from the last one back and no earlier unit's position moves.
    pub(super) fn abort_units(&mut self, when: Duration, units: &[Unit]) {
        for (position, unit) in units.iter().enumerate().rev() {
            // Every bound is clamped to the list as it stands: a later unit has
            // already been cut back by the time this one is, so the span this reads
            // is the one the list still holds.
            let until = units
                .get(position + 1)
                .map_or(self.steps.len(), |next| next.from)
                .min(self.steps.len());
            let from = unit.from.min(until);
            let Some(cut) = (from..until).find(|index| {
                self.steps
                    .get(*index)
                    .is_some_and(|step| step.after > when)
            }) else {
                continue;
            };
            // The states this unit was still inside when the failure reached it: each
            // one entered and not yet left, innermost last.
            let mut open: Vec<(&'static str, Option<String>)> = Vec::new();
            for step in self.steps.get(from..cut).unwrap_or_default() {
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
            self.steps.splice(cut..until, stopped);
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
                if !errors_match(retrier, failure.name()) {
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
                // definition writes in `IntervalSeconds`, `BackoffRate` and
                // `MaxDelaySeconds`. The conversion and the accumulation are
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
            // A retry re-runs the work, so it spends from the same step budget: a
            // definition cannot ask for an unbounded number of attempts.
            if self.budget == 0 {
                return (Err(failure), at);
            }
            self.budget -= 1;
            self.retry_count += 1;
        }
    }

    /// The state the first matching `Catch` catcher names, or the failure itself when
    /// none does.
    ///
    /// `left` is the event the state records when a catcher takes over, so a caught
    /// state's history pairs the event it was entered with.
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
            if !errors_match(catcher, failure.name()) {
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
            let error_output = failure.payload();
            let value = match language {
                JSONATA => self
                    .jsonata_output(catcher, effective, None, Some(&error_output), context)
                    .map_err(|failure| failure.at(at))?,
                _ => self
                    .place_result(catcher, raw, error_output, context, true)
                    .map_err(|failure| failure.at(at))?,
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
                expression(path, input, context).map_err(|failure| Failure {
                    error: failure.error,
                    cause: failure
                        .cause
                        .map(|why| member_path_diagnosis(&path_member, path, why)),
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

