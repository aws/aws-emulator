// SPDX-License-Identifier: CC0-1.0

//! The single-state trial `TestState` runs: ONE state of a definition, with no
//! execution around it.
//!
//! One concern in a file of its own, reached from [`super`]: the request fixes the
//! state, its input, the attempt it is on and the mock that stands in for its work, and
//! the walk reports what that one state computed.

use super::*;

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
    /// The bytes the request wrote the mocked RESULT in (`mock.result`: "a JSON string
    /// containing the mocked result of the state invocation").
    ///
    /// The mocked result is a document the caller supplied AS TEXT, exactly as the
    /// state's input is, so it is reported in those bytes wherever it is reported or
    /// quoted ([`super::Supplied`]) rather than in a re-encoding of the parsed copy.
    pub mock_result: Option<&'a str>,
    /// `stateConfiguration.mapIterationFailureCount`: "The number of Map state
    /// iterations that failed during the Map state invocation", which is what the
    /// state's tolerated-failure threshold is weighed against.
    pub map_iteration_failures: Option<u64>,
    /// `stateConfiguration.retrierRetryCount`: "The number of retry attempts that
    /// have occurred for the state's Retry that applies to the mocked error", which
    /// is what decides whether the state's failure is still RETRIABLE.
    pub retries_taken: u64,
    /// "JSON object literal that sets variables used in the state under test. Object
    /// keys are the variable names and values are the variable values": the scope the
    /// state's own expressions read, before its `Assign` writes to it.
    pub variables: Option<&'a Value>,
    /// The `QueryLanguage` the submitted DEFINITION declares, as that definition wrote
    /// it; `None` where it declares none.
    ///
    /// "The state machine's query language" is declared at the definition's top level
    /// and INHERITED by every state it holds, which a state overrides for itself by
    /// declaring its own — so a `stateName` naming one state of a full definition is
    /// tested in the language that definition set, exactly as an execution of it would
    /// run that state. A `definition` that IS the state carries the member at its own
    /// top level, where it is the state's own.
    pub definition_language: Option<&'a Value>,
    /// What performs the integration call of a `Task` state under test
    /// ([`super::super::integration::Tasks`]).
    ///
    /// `TestState` "can test ... an Amazon Web Services service integration request and
    /// response", so a `Task` state with no `mock` standing in for its work has that
    /// work DONE: the call is made through the same seam an execution's `Task` state
    /// makes it through, as the role the request named.
    pub tasks: &'a dyn Tasks,
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
                ..Failure::default()
            },
        );
    }
    let base_context = match test.context_override {
        Some(context) => context.clone(),
        None => test.context.value(&raw, test.start),
    };
    // The language the state under test INHERITS: the one the submitted definition
    // declares, which is what a state of it that declares none is written in.
    let inherited = match declared_language(test.definition_language) {
        Ok(declared) => declared.unwrap_or(JSONPATH),
        Err(why) => return Trial::failed(inspection, runtime(why)),
    };
    let mut interpreter = Interpreter {
        // `TestState` keeps no history of its own, so the allowance it runs under is a
        // whole execution's: what bounds the ONE state it tests is the work that state
        // asks for and not a history it writes nothing into.
        events_left: MAX_HISTORY_EVENTS,
        // The one state under test records what it records for the trial to report; no
        // child workflow execution's events are counted here, because `TestState` starts
        // none.
        unheld: None,
        depth: 0,
        call_wait_left: MAX_CALL_WAIT_DURATION,
        // `TestState` tests ONE state, with no execution and so no definition-level
        // `TimeoutSeconds` around it: nothing here bounds the span the state runs in.
        timeout_left: None,
        timed_out: false,
        // The state is tested as the attempt the request says it is on, which is what a
        // state of a kind that can be retried reads through `$$.State.RetryCount`.
        retry_count: test.retries_taken,
        // The state is tested at the instant the request is answered at, so the clock it
        // reads stands where this walk begins.
        clock_offset: Duration::ZERO,
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
        // `TestState` "can test ... an Amazon Web Services service integration request
        // and response": the call a `Task` state under test makes is made through the
        // same seam an execution's is, by whatever the operation handed this walk
        // ([`StateTest::tasks`]), and a mocked state's work is the mock instead.
        tasks: test.tasks,
        waiting: None,
        parked: None,
        // `TestState` tests one state, so there is no concurrent state around it to
        // park inside, no job of its own to abandon and no sibling branch whose
        // failure could stop it.
        abandoned: Vec::new(),
        aborted_task: None,
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
        region: super::super::arn_region(&test.context.state_machine_arn),
        execution_arn: test.context.execution_arn.clone(),
        child_counter: 0,
        // The state under test belongs to no state machine, so no workflow type
        // refuses it: the context the request supplies says nothing of one.
        express: test.context.express,
        // `TestState` redrives nothing: the step a redrive would continue from
        // belongs to an execution.
        failed_at: None,
        // The state under test is written in the language it declares, and in the one
        // the submitted definition declares where it declares none — which is what
        // `QueryLanguage` at a definition's top level sets for every state it holds.
        language: inherited,
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
            // The mocked result is a caller's document too: it reaches this walk as the
            // text the request wrote it in, so that text is what reports it. The
            // request's own INPUT is remembered last, so it decides for a document the
            // two were written as differently.
            if let Some(mocked) = test.mock_result {
                supplied.supplied(mocked);
            }
            supplied.supplied(test.input);
            supplied
        },
        // `TestState` continues no execution either, so the state under test is entered
        // by this walk and names the event this walk numbers for it.
        continued_entry: None,
        skipping_entry: false,
        // The trial of a single state starts no call of its own beside another's
        // ([`super::map_items`]).
        started: None,
        probing: None,
        unwinding: false,
    };
    // And the same document for the diagnoses the trial's own expressions word, which
    // quote the document they could not be applied to ([`GivenDocuments`]).
    let _given = GivenDocuments::of(&interpreter.supplied);
    interpreter.trial(test, &raw, inspection)
}

impl Interpreter<'_> {
    /// [`test_state`]'s walk of the one state under test.
    pub(super) fn trial(&mut self, test: &StateTest<'_>, raw: &Value, mut inspection: Inspection) -> Trial {
        let state = test.state;
        let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
        let context = self.context_for(&Scope::default(), test.name, state_type, Duration::ZERO);
        // "The set will include variables assigned in the state and variables set up as
        // test state input": the request's own set is in scope for the paths of the
        // state under test, installed for this trial exactly as it is for a state of a
        // walk ([`variables`]).
        let _scope = VariableScope::of(&self.variables);

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
            inspection.after_input_path = Some(self.echo(&effective));
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
                JSONATA => inspection.after_arguments = Some(self.echo(&after_parameters)),
                _ => inspection.after_parameters = Some(self.echo(&after_parameters)),
            }
        }

        // The ITERATION side, which only a `Map` state has: the stages its items pass
        // through and the bounds it declared ([`Interpreter::map_inspection`]).
        let bounds = match state_type == MAP_STATE_TYPE {
            true => self.map_inspection(test, language, &effective, &context, &mut inspection),
            false => MapBounds::none(),
        };
        // "The number of Map state iterations that failed during the Map state
        // invocation": a figure counting more failed iterations than the state HAS
        // describes no invocation of this state, so the request is refused rather than
        // an outcome being computed from it.
        if let (Some(failures), Some(iterations)) = (test.map_iteration_failures, bounds.iterations)
        {
            if failures > iterations as u64 {
                return Trial::refusal(
                    inspection,
                    MAP_ITERATION_FAILURES_PAST_ITERATIONS.to_string(),
                );
            }
        }
        // And what that count says about the state: a `Map` state whose failed
        // iterations went past the threshold it declared fails with the BREACH of that
        // threshold, which supersedes whatever the iterations themselves failed with —
        // the same rule the items of an execution's `Map` state are judged by
        // ([`super::map_items`]).
        if let Some(failure) = bounds.breached(test.map_iteration_failures) {
            // The state's own error handling is still REPORTED for that error — the
            // `Retry` entry that answers for it, and the `Catch` entry that would have
            // taken it over — but the breach of the failure budget is what ENDED the
            // state, so this trial's own status is FAILED, and a state that failed
            // puts out no document and transitions nowhere: "if the execution of the
            // state fails, this field doesn't contain a value"
            // (`TestStateOutput$nextState`).
            let mut trial = self.classify(test, language, raw, &context, failure, inspection);
            trial.status = TEST_STATUS_FAILED;
            trial.output = None;
            trial.next_state = None;
            return trial;
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
                // A JSONata state's output is its `Output`, evaluated before the
                // `Assign` that follows it; there are no result-side filters to
                // record stages for. A `Choice` state's own members and the SELECTED
                // rule's decide it together, exactly as they do in the walk.
                JSONATA => {
                    if let Some(result) = result.as_ref() {
                        inspection.result = Some(self.echo(result));
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
                    None => {
                        // The `Assign` of whichever decided the transition, for a
                        // `Choice` state: the rule that matched, or the state itself
                        // where none did — exactly as the walk reads it
                        // ([`Interpreter::jsonpath_assign`]).
                        let owner = chosen
                            .as_ref()
                            .and_then(|chosen| chosen.rule.as_ref())
                            .map_or(state, |(_, rule)| rule);
                        match self.jsonpath_assign(owner, &effective, &context) {
                            Ok(()) => self.output_value(state, effective.clone(), &context),
                            Err(failure) => Err(failure),
                        }
                    }
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

    /// What `TestState` reports about a `Map` state's ITERATION side: the stages its
    /// items pass through, the bounds it declared, and how many iterations its items
    /// make.
    ///
    /// The items are derived from the state's own definition and input — its
    /// `ItemsPath` (or its `Items`) selects them, its `ItemSelector` transforms each
    /// one and its `ItemBatcher` groups them — through the SAME readers an execution's
    /// `Map` state derives them with ([`Interpreter::map_items`],
    /// [`Interpreter::batched_items`]), so what the stages report is the documents this
    /// state's iterations would be handed.
    ///
    /// Nothing is RUN for them and nothing fails for them: the iteration itself is the
    /// invocation the request's `mock` stands in for, so a state whose items are not
    /// its own input to derive — one whose `ItemReader` names a dataset, one whose
    /// `ItemsPath` points at no array, one whose template cannot be rendered — reports
    /// the stages it has and no more, rather than failing for a stage nothing was
    /// iterated at.
    ///
    /// `afterItemsPointer` is reported for none of them: the Amazon States Language
    /// declares no `ItemsPointer` member, so no filter of a state resolves one.
    fn map_inspection(
        &mut self,
        test: &StateTest<'_>,
        language: &'static str,
        effective: &Value,
        context: &Value,
        inspection: &mut Inspection,
    ) -> MapBounds {
        let state = test.state;
        // The bounds the state DECLARES are the ones it is reported under: a bound the
        // definition leaves out names no value for this state, so nothing is reported
        // for it.
        let tolerated_count = self.declared_bound(state, language, "ToleratedFailureCount", effective, context);
        let tolerated_percentage =
            self.declared_bound(state, language, "ToleratedFailurePercentage", effective, context);
        inspection.max_concurrency = self
            .declared_bound(state, language, "MaxConcurrency", effective, context)
            .map(reported_bound);
        inspection.tolerated_failure_count = tolerated_count.map(reported_bound);
        inspection.tolerated_failure_percentage = tolerated_percentage.map(|share| share as f32);
        let mut bounds = MapBounds {
            iterations: None,
            tolerated_count: tolerated_count.unwrap_or(0.0),
            tolerated_percentage: tolerated_percentage.unwrap_or(0.0),
            // A state that declares no threshold tolerates no failed iteration, so
            // there is no threshold for one to exceed; a DISTRIBUTED Map runs under one
            // either way — the rule its items are judged by ([`super::map_items`]).
            tolerant: declares_tolerance(state)
                || map_processor(test.name, state).is_ok_and(|(_, distributed)| distributed),
        };
        // "ItemReader — specifies a dataset and its location": a state that declares
        // one iterates THAT dataset rather than anything its own input carries, and
        // reading it is part of the invocation the mock stands in for, so no stage of
        // its items is derived here.
        if state.contains_key("ItemReader") {
            return bounds;
        }
        let Ok(items) = self.map_items(test.name, state, language, effective, context) else {
            return bounds;
        };
        // "The effective input after the ItemsPath filter is applied. Not populated
        // when the QueryLanguage is JSONata" — and a state whose items are its own
        // `Items` member passes them through no such filter at all.
        if language != JSONATA && !state.contains_key("Items") {
            inspection.after_items_path = self.listed(&items);
        }
        // "An array containing the inputs for each Map iteration, transformed by the
        // ItemSelector specified in a Map state": one document per iteration, which is
        // the item itself for a state that declares no such template. Each is rendered
        // with `$$.Map.Item` set to the item and its position, which is what the
        // template reads them through ([`super::map_items`]) — and the stage is built as
        // the TEXT it is reported in, so neither the text nor what is held to build it
        // grows with how many items a caller's array holds past what the member carries.
        bounds.iterations = Some(items.len());
        let selector = state.get("ItemSelector").or_else(|| state.get("Parameters"));
        let mut iterated = Listed::default();
        for (index, item) in items.iter().enumerate() {
            let selected = match selector {
                None => Ok(item.clone()),
                Some(template) => {
                    let map_item = json!({ "Index": index, "Value": item });
                    let item_context = with_map_item(context, &map_item);
                    self.render_selector(language, template, effective, &item_context)
                }
            };
            let Ok(value) = selected else {
                return bounds;
            };
            if !iterated.push(&self.echo(&value)) {
                return bounds;
            }
        }
        inspection.after_item_selector = iterated.reported();
        // "The effective input after the ItemBatcher filter is applied in a Map state":
        // a state that declares one hands each iteration a BATCH of its items, so the
        // batches are what its iterations are, and the selector was already spent on
        // the items on their way into them.
        if let Some(batcher) = state.get("ItemBatcher") {
            let batched = self.batched_items(
                test.name, state, language, batcher, items, effective, context,
            );
            let Ok(batches) = batched else {
                return bounds;
            };
            bounds.iterations = Some(batches.len());
            inspection.after_item_batcher = self.listed(&batches);
        }
        bounds
    }

    /// A numeric bound a `Map` state DECLARES, resolved from the member itself or from
    /// its reference-path form; `None` where the state declares neither, and where the
    /// value it declares is one no figure can be read out of.
    fn declared_bound(
        &self,
        state: &Map<String, Value>,
        language: &'static str,
        member: &str,
        effective: &Value,
        context: &Value,
    ) -> Option<f64> {
        if !state.contains_key(member) && !state.contains_key(&format!("{member}Path")) {
            return None;
        }
        self.number_member(state, language, member, effective, context, 0.0)
            .ok()
    }

    /// How an inspection stage whose document is a LIST of documents is reported, for a
    /// list already held whole ([`Listed`]).
    fn listed(&self, documents: &[Value]) -> Option<String> {
        let mut listed = Listed::default();
        for document in documents {
            if !listed.push(&self.echo(document)) {
                return None;
            }
        }
        listed.reported()
    }

    /// The variables this walk has assigned, as the one JSON document `TestState`
    /// reports them through — `None` where nothing assigned any, so the member is
    /// absent rather than an empty object.
    pub(super) fn assigned_variables(&self) -> Option<String> {
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
    pub(super) fn produce(
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
                        // A diagnosis this module worded about the state's own
                        // expression already names the field it is written in, so it
                        // is carried exactly as raised — the rule the walk applies
                        // around every such diagnosis ([`diagnosed`]). Every other
                        // reason a `Wait`'s members could not be read names the state
                        // under test, which is the only name this trial has for it.
                        cause: failure.cause.clone().map(|why| {
                            match names_its_state(&why) {
                                true => why,
                                false => format!("{SINGLE_STATE_LABEL}: {why}"),
                            }
                        }),
                        ..failure
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
                Err(Failure {
                    error,
                    cause,
                    ..Failure::default()
                })
            }
            // A `Task` state's own work is the integration call its `Resource` names,
            // and `TestState` "can test ... an Amazon Web Services service integration
            // request and response": the call is MADE, and the answer it comes back
            // with is the state's result ([`Interpreter::tried_task`]).
            "Task" => self.tried_task(state, language, after_parameters, effective, context),
            // Every remaining type's work is an invocation this process does not
            // make: an iteration over items, a set of branches. Without a mock to
            // stand in for it, nothing ran — which is what the state fails with,
            // rather than a synthesised result.
            other => Err(runtime(format!(
                "a '{other}' state's own work is an invocation this emulator does not make, and \
                 the request names no mock to stand in for it, so nothing was run for it"
            ))),
        }
    }

    /// The result the `Task` state under test's own work produced: its integration
    /// call, made and answered.
    ///
    /// "Using this API, you can test the following: ... An Amazon Web Services service
    /// integration request and response; An HTTP Task request and response" — so the
    /// call is really made, through the SAME seam an execution's `Task` state makes it
    /// through ([`super::super::integration::Tasks`]) and under the same bounds: the
    /// state's own `TimeoutSeconds`, the role the request named (which decides whether
    /// the call is made at all), and this trial's own work allowance.
    ///
    /// Three kinds of work are NOT done here, because each is answered from outside
    /// the call that asked and `TestState` "only supports the following when a mock is
    /// specified: Activity tasks, .sync or .waitForTaskToken service integration
    /// patterns": an activity's work belongs to a worker that polls for it, a
    /// run-a-job (`.sync`) task's outcome is the job's own end, and a callback
    /// (`.waitForTaskToken`) task's is whoever holds the token. Without a mock standing
    /// in for one of those outcomes, nothing ran — which is what such a state fails
    /// with, rather than a synthesised result.
    fn tried_task(
        &mut self,
        state: &Map<String, Value>,
        language: &'static str,
        request: &Value,
        effective: &Value,
        context: &Value,
    ) -> Result<Option<Value>, Failure> {
        let resource = task_resource(state);
        if super::super::names_an_activity(resource) {
            return Err(runtime(
                "a 'Task' state whose work is an activity's is done by a worker polling for it, \
                 which is an answer this call cannot wait for, and the request names no mock to \
                 stand in for it, so nothing was run for it"
                    .to_string(),
            ));
        }
        let call = super::super::integration::read(resource)
            .map_err(|why| runtime(format!("{SINGLE_STATE_LABEL}: {why}")))?;
        if call.job.is_some() || call.callback {
            return Err(runtime(format!(
                "{} is answered from outside the call that started it \u{2014} by the job it \
                 starts, or by whoever its task token went to \u{2014} which is an answer this \
                 call cannot wait for, and the request names no mock to stand in for it, so \
                 nothing was run for it",
                call.described()
            )));
        }
        // "TimeoutSeconds […] If the task runs longer than the specified seconds, this
        // state fails with a States.Timeout error name": the member bounds the WAIT for
        // the answer, read through the same reader the walk reads it with.
        let within = self
            .task_seconds(state, language, "TimeoutSeconds", effective, context)?
            .map(Duration::from_secs);
        // The call is made AS the role the request named, so what that role may do
        // decides whether it is made AT ALL — reported before the request goes
        // anywhere, exactly as it is for an execution's `Task` state.
        if let Some(refused) = self.tasks.authorize(&call, request) {
            return Err(super::reported(refused));
        }
        // And the trial's own work allowance: a run that has asked for more work than
        // one interpretation may do makes no call.
        if let Some(failure) = self.out_of_work() {
            return Err(failure);
        }
        // What Step Functions adds to a PUBLICATION's entries is the attribution of
        // the publication to the run that made it; this trial runs no execution, so
        // the attribution it carries is the empty one this walk holds, and every other
        // call is made with the document the state built, unchanged.
        let attributed = super::super::integration::publication_request(
            &call,
            request,
            &self.execution_arn,
            self.state_machine_arn(),
        );
        let called = attributed.as_ref().unwrap_or(request);
        let Some((answered, _began)) =
            self.perform_call(&call, called, self.call_within(within), None)
        else {
            // Unreachable: only a turn that has SAMPLED nothing and may be walked again
            // leaves a call in flight, and this trial walks one state once.
            return Err(unwound());
        };
        // A publication the service SERVED but which rejected an entry did not do this
        // task's work, so it is read as the failure it is before the answer becomes a
        // result.
        let answered = answered.and_then(|answer| {
            match super::super::integration::rejected_entries(&call, &answer) {
                Some(failed) => Err(failed),
                None => Ok(answer),
            }
        });
        match answered {
            Ok(answer) => {
                let result = super::super::integration::reported_result(&call, &answer);
                // The result is a document the state carries, so it is measured
                // against the ceiling every state's document is measured against.
                measure_payload(&result, SINGLE_STATE_LABEL)?;
                Ok(Some(result))
            }
            Err(failed) => Err(super::reported(failed)),
        }
    }

    /// The result-side filters over a state's raw result — `ResultSelector`, the
    /// state's own `Assign`, `ResultPath` against the state's own raw input, then
    /// `OutputPath` — recording what the state carried after each one.
    pub(super) fn place_recorded(
        &mut self,
        state: &Map<String, Value>,
        state_type: &str,
        raw: &Value,
        result: Value,
        context: &Value,
        inspection: &mut Inspection,
    ) -> Result<Value, Failure> {
        inspection.result = Some(self.echo(&result));
        let with_selector = SELECTOR_STATE_TYPES.contains(&state_type);
        let selected = select_result(state, result, context, with_selector)?;
        // "The effective result after Step Functions applies the ResultSelector
        // filter": the stage is the state's result as the result-side data flow
        // RECEIVES it, so a state whose type has no such filter — or that declares
        // none — reports its raw result there rather than nothing at all.
        inspection.after_result_selector = Some(self.echo(&selected));
        // And the state's own `Assign` reads that same document, as it does in a walk
        // ([`Interpreter::produced`]).
        self.jsonpath_assign(state, &selected, context)?;
        let placed = path_result(state, raw, selected)?;
        inspection.after_result_path = Some(self.echo(&placed));
        self.output_value(state, placed, context)
    }

    /// What `TestState` reports for a state whose work failed: the attempt one of its
    /// `Retry` retriers still owes (RETRIABLE), the catcher that takes the failure
    /// over (CAUGHT_ERROR), or the failure itself (FAILED).
    ///
    /// The two are the state's OWN error handling, read exactly as the walk reads it:
    /// the first retrier whose `ErrorEquals` names the error decides, and `Catch` is
    /// consulted only once no retrier has an attempt left.
    pub(super) fn classify(
        &mut self,
        test: &StateTest<'_>,
        language: &'static str,
        raw: &Value,
        context: &Value,
        failure: Failure,
        mut inspection: Inspection,
    ) -> Trial {
        let state = test.state;
        // Which `Retry` entry answers for this error, whether or not it still has an
        // attempt left: "the array index of the Retry which handled the exception" is
        // what the trial reports for a failure the state's own error handling took
        // over, and a state with no retrier for the error has none to name.
        let retrier = retry_entries(state, "Retry")
            .iter()
            .enumerate()
            .filter_map(|(index, retrier)| retrier.as_object().map(|retrier| (index, retrier)))
            .find(|(_, retrier)| failure.matched_by(retrier));
        let retry_index = Some(match retrier {
            Some((index, _)) => i32::try_from(index).unwrap_or(i32::MAX),
            None => NO_RETRIER_INDEX,
        });
        if let Some((_, retrier)) = retrier {
            let max_attempts = retrier
                .get("MaxAttempts")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_RETRY_MAX_ATTEMPTS);
            if test.retries_taken < max_attempts {
                let mut trial = Trial::failed(inspection, failure);
                trial.status = TEST_STATUS_RETRIABLE;
                trial.retry_index = retry_index;
                trial.retry_backoff_seconds =
                    Some(retry_delay_seconds(retrier, test.retries_taken).round() as i32);
                return trial;
            }
        }
        for (index, catcher) in retry_entries(state, "Catch").iter().enumerate() {
            let Some(catcher) = catcher.as_object() else {
                continue;
            };
            if !failure.matched_by(catcher) {
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
                // The catcher's own `Assign` reads the error output it was handed in
                // this language too, as it does in the walk
                // ([`Interpreter::catch_or_fail`]).
                _ => match self.jsonpath_assign(catcher, &error_output, context) {
                    Ok(()) => {
                        self.caught_output(catcher, raw, error_output, context, &mut inspection)
                    }
                    Err(failure) => Err(failure),
                },
            };
            let output = match output {
                Ok(output) => output,
                Err(failure) => return Trial::failed(inspection, failure),
            };
            let mut trial = Trial::failed(inspection, failure);
            trial.status = TEST_STATUS_CAUGHT_ERROR;
            trial.output = Some(self.echo(&output));
            trial.next_state = Some(next.to_string());
            trial.catch_index = Some(i32::try_from(index).unwrap_or(i32::MAX));
            // A caught failure reports both handler positions: the catcher that took it
            // over, and the retrier whose attempts preceded it.
            trial.retry_index = retry_index;
            return trial;
        }
        Trial::failed(inspection, failure)
    }

    /// The output a CATCHER puts out for the failure it took over, recording the
    /// document that catcher's `ResultPath` placed.
    ///
    /// "When a state reports an error and it matches a catcher, the state's output is
    /// the error output": the error output is the result such a state has, so the
    /// catcher's own `ResultPath` is the filter that places it — and the document the
    /// placement produced is "the effective result combined with the raw state input
    /// after Step Functions applies the ResultPath filter", which is the one
    /// result-side stage a caught failure has. The catcher's own `Assign` has already
    /// read the error output by the time this is reached, exactly as it has in the walk.
    fn caught_output(
        &self,
        catcher: &Map<String, Value>,
        raw: &Value,
        error_output: Value,
        context: &Value,
        inspection: &mut Inspection,
    ) -> Result<Value, Failure> {
        let selected = select_result(catcher, error_output, context, true)?;
        let placed = path_result(catcher, raw, selected)?;
        inspection.after_result_path = Some(self.echo(&placed));
        self.output_value(catcher, placed, context)
    }
}

/// The state type whose own work is an ITERATION over items, and so the only one with
/// per-iteration stages and bounds of its own to report.
const MAP_STATE_TYPE: &str = "Map";

/// What this operation answers a `stateConfiguration.mapIterationFailureCount` that
/// counts more failed iterations than the state under test has iterations.
///
/// RECORDED text, and a FIXED sentence: the figure the request sent and the number of
/// iterations the state's items make are both the caller's own, so the refusal states
/// the rule rather than either of them.
pub(super) const MAP_ITERATION_FAILURES_PAST_ITERATIONS: &str =
    "Map iteration failure count must be less than or equal to the number of Map iterations";

/// `number` as the modelled 32-bit integer an inspection bound is reported through.
///
/// `InspectionData$maxConcurrency` and `$toleratedFailureCount` are integers of a
/// minimum of zero, while the members they report are figures a definition may write in
/// any numeric form, so what is reported is the figure held to what that integer
/// carries.
fn reported_bound(number: f64) -> i32 {
    number.clamp(0.0, f64::from(i32::MAX)) as i32
}

/// The text an inspection stage whose document is a LIST of documents is reported in:
/// each document as the walk reports it ([`Interpreter::echo`]), between brackets and
/// separated by `", "`.
///
/// RECORDED encoding: the per-iteration stages of a `Map` state report the LIST of
/// documents its iterations are handed, which is not the encoding one JSON array
/// holding them has.
///
/// It is built document by document, and stops at what the modelled member carries
/// ([`MAX_PAYLOAD_BYTES`], the `SensitiveData` maximum every stage leaves through): a
/// stage longer than that is no value of the member, so neither the text nor the work of
/// building it grows with how many items a caller's array holds past that point.
#[derive(Debug)]
struct Listed {
    reported: String,
    /// Whether the list is still inside what the member carries.
    within: bool,
}

impl Default for Listed {
    fn default() -> Self {
        Self {
            reported: String::from("["),
            within: true,
        }
    }
}

impl Listed {
    /// Add one document, as the text it is reported in; `false` once the list has
    /// reached what the member carries, after which nothing more is held.
    fn push(&mut self, document: &str) -> bool {
        if !self.within {
            return false;
        }
        if self.reported.len() > 1 {
            self.reported.push_str(", ");
        }
        self.reported.push_str(document);
        self.within = self.reported.len() <= MAX_PAYLOAD_BYTES;
        if !self.within {
            self.reported = String::new();
        }
        self.within
    }

    /// The whole list, or `None` for one past what the member carries.
    fn reported(mut self) -> Option<String> {
        if !self.within {
            return None;
        }
        self.reported.push(']');
        Some(self.reported)
    }
}

/// What bounds the `Map` state under test, and how many iterations its items make.
///
/// The two are read together because both are needed to judge the failed-iteration
/// count the request carries: how many iterations there are at all, and the threshold
/// the state tolerates among them.
#[derive(Debug, Clone)]
struct MapBounds {
    /// How many iterations the state's items make — batches for a state that declares
    /// an `ItemBatcher` — or `None` where they are not this state's own input to
    /// derive.
    iterations: Option<usize>,
    /// `ToleratedFailureCount` as the state resolved it.
    tolerated_count: f64,
    /// `ToleratedFailurePercentage` as the state resolved it.
    tolerated_percentage: f64,
    /// Whether the state has a tolerated-failure threshold at all.
    tolerant: bool,
}

impl MapBounds {
    /// The bounds of a state that is not a `Map`: no iterations, and no threshold.
    fn none() -> Self {
        Self {
            iterations: None,
            tolerated_count: 0.0,
            tolerated_percentage: 0.0,
            tolerant: false,
        }
    }

    /// The failure a `Map` state whose `failures` iterations went PAST the threshold it
    /// declared ends with, or `None` where the threshold still tolerates them.
    ///
    /// "ToleratedFailureCount [...] the maximum number of failed items" and
    /// "ToleratedFailurePercentage [...] the maximum percentage of failed items" are
    /// weighed exactly as an execution's Map Run weighs them
    /// ([`super::super::within_tolerated_failures`]), so one `Map` state never reports
    /// its failure budget two ways.
    fn breached(&self, failures: Option<u64>) -> Option<Failure> {
        let failures = failures.filter(|failures| *failures > 0)?;
        if !self.tolerant {
            return None;
        }
        let total = self.iterations.unwrap_or(0);
        let within = super::super::within_tolerated_failures(
            i64::try_from(failures).unwrap_or(i64::MAX),
            i64::try_from(total).unwrap_or(i64::MAX),
            self.tolerated_count as i64,
            self.tolerated_percentage,
        );
        match within {
            true => None,
            false => Some(named(
                TOLERATED_FAILURE_ERROR,
                TOLERATED_FAILURE_CAUSE.to_string(),
            )),
        }
    }
}
