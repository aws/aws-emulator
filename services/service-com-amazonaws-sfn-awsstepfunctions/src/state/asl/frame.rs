// SPDX-License-Identifier: CC0-1.0

//! The `Parallel` state's own walk: how one state runs the branches it declares, and what
//! their outcomes make of the state.
//!
//! One concern of the interpreter, held in one file. A `Parallel` state is the one state
//! whose work is SEVERAL walks — "the branches run concurrently" — so what it owes is its
//! own: the input every branch is handed, the instant a failing branch stops its siblings
//! at, the frame a suspended branch records for the continuation that re-enters the state,
//! and the task a sibling's failure aborted after a worker had already been handed it.
//! Every other state's turn is walked beside this ([`super::walk`]), and the frame all of
//! them share is declared with the interpretation itself ([`super`]).

use super::*;

impl Interpreter<'_> {
    /// Whether this walk has SUSPENDED at work one of its states handed over: the job a
    /// run-a-job task started, or the task a worker's reply or an answered token
    /// finishes.
    ///
    /// The ONE statement of it, because a `Parallel` state answers for either the same
    /// way: a branch that suspended leaves the state with no result, and the frame the
    /// suspension records is what the continuation re-enters the state through.
    pub(super) fn suspended_in_a_branch(&self) -> bool {
        self.parked.is_some() || self.waiting.is_some()
    }

    // -----------------------------------------------------------------------
    // Parallel
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(super) fn parallel_state(
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
                    me.push(super::super::EVENT_PARALLEL_STATE_FAILED, name, at);
                    return (Err(failure), at);
                }
            };
            // Every attempt at this state starts its branches, so every attempt
            // writes its own `ParallelStateStarted` and the event of the outcome
            // those branches reached.
            me.push(super::super::EVENT_PARALLEL_STATE_STARTED, name, at);
            let (outcome, end) =
                me.run_branches(&plan, name, &raw, &branch_input, at, resume.take());
            // A branch that SUSPENDED leaves the state with no outcome at all, so the
            // state records neither of the events an outcome is reported by: what
            // happens next is the job finishing, or the task being answered.
            if me.suspended_in_a_branch() {
                return (outcome, end);
            }
            let event = if outcome.is_ok() {
                super::super::EVENT_PARALLEL_STATE_SUCCEEDED
            } else {
                super::super::EVENT_PARALLEL_STATE_FAILED
            };
            me.push(event, name, end);
            (outcome, end)
        });
        match outcome {
            // One of the branches is suspended at the work one of its own states handed
            // over — the job a run-a-job task started, or the task a worker or a token
            // answers: the walk stops here and the execution waits for it, exactly as it
            // does for a task at the top level of the definition.
            Ok(value) if self.suspended_in_a_branch() => Ok(StateStep {
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
    pub(super) fn run_branches(
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
                steps: (from..self.steps.len()).collect(),
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
            // suspended at work of its own is not waited for any longer: the job it
            // started is ABANDONED, which is what stops it.
            if let Some(parked) = self.parked.take() {
                self.abandoned.push(parked);
            }
            self.abort_units(when, &units);
            // A task a sibling branch had already SCHEDULED is work this execution
            // really handed over: it is queued under its activity from the instant the
            // state was entered, so a worker polling that activity is handed it. The
            // state is therefore ABORTED rather than never scheduled, and stopping work
            // somebody else is holding takes a span ([`super::HANDOVER_STOP_DELAY`]) —
            // so the state's failure is dated at the instant that work was stopped, and
            // the task stays claimable until then.
            let Some(aborted) = self.waiting.take() else {
                return (Err(failure), when);
            };
            let stopped = when.max(
                aborted
                    .at
                    .saturating_add(super::super::HANDOVER_STOP_DELAY),
            );
            if let Some(event_type) = aborted_event(TASK_STATE_TYPE) {
                self.push(event_type, &aborted.state_name, stopped);
            }
            self.aborted_task = Some(aborted);
            return (Err(failure), stopped);
        }
        if let Some(branch) = suspended {
            // The frame this state contributes to the suspended record: which branch is
            // waiting, and what the others produced, so the continuation re-enters
            // this state without re-running any of them. One of the two records is set
            // — a job the branch parked at, or a task it is waiting at — and the frame
            // is the same either way.
            let frame = ParkedFrame {
                state_name: state_name.to_string(),
                state_input: state_input.clone(),
                branch,
                done,
            };
            if let Some(parked) = self.parked.as_mut() {
                parked.frames.insert(0, frame.clone());
            }
            if let Some(waiting) = self.waiting.as_mut() {
                waiting.frames.insert(0, frame);
            }
            // The state has no result: `parallel_state` reads the suspended record and
            // suspends there, so this value is never placed.
            return (Ok(Value::Null), end);
        }
        let results: Vec<Value> = done.into_iter().map(Option::unwrap_or_default).collect();
        (Ok(Value::Array(results)), end)
    }
}
