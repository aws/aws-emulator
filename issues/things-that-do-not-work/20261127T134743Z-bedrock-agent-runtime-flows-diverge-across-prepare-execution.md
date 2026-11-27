Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.invoke-flow-condition-input-type-mismatch-rejected bedrock-agent-runtime.invoke-flow-condition-node-routes-to-first-satisfied-condition bedrock-agent-runtime.invoke-flow-iterator-collector-reassembles-array-in-order bedrock-agent-runtime.invoke-flow-version-alias-serves-snapshot-after-draft-change bedrock-agent-runtime.flow-execution-inline-code-error-reports-flow-failure-event bedrock-agent-runtime.flow-execution-input-type-mismatch-fails-before-any-node bedrock-agent-runtime.invoke-flow-condition-node-routes-to-first-matching-condition bedrock-agent-runtime.list-flow-executions-reports-stopped-execution-as-aborted bedrock-agent-runtime.flow-execution-lambda-node-error-lists-failed bedrock-agent-runtime.stop-flow-execution-on-succeeded-execution-rejected bedrock-agent-runtime.stopped-flow-execution-stays-aborted-after-its-lambda-node-returns bedrock-agent-runtime.get-flow-execution-through-another-alias-rejected bedrock-agent-runtime.list-flow-execution-events-through-another-alias-rejected bedrock-agent-runtime.get-flow-execution-through-sibling-alias-not-found bedrock-agent-runtime.invoke-flow-input-type-mismatch-streams-validation-exception bedrock-agent-runtime.invoke-flow-mistyped-input-document-rejected-in-stream bedrock-agent-runtime.invoke-flow-unknown-execution-id-streams-validation-exception bedrock-agent-runtime.invoke-flow-completed-execution-id-not-resumable-rejected bedrock-agent-runtime.invoke-flow-draft-alias-after-update-without-prepare-rejected bedrock-agent-runtime.start-flow-execution-reused-name-rejected bedrock-agent-runtime.start-flow-execution-unknown-input-node-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime.invoke-flow-condition-input-type-mismatch-rejected|_wait_for_prepared#6|AssertionError|raise AssertionError(
Issue-Key: bedrock-agent-runtime.invoke-flow-condition-node-routes-to-first-satisfied-condition|_prepare#6|AssertionError|raise AssertionError(
Issue-Key: bedrock-agent-runtime.invoke-flow-iterator-collector-reassembles-array-in-order|_prepared#6|AssertionError|raise AssertionError(
Issue-Key: bedrock-agent-runtime.invoke-flow-version-alias-serves-snapshot-after-draft-change|_until_prepared#7|AssertionError|raise AssertionError(
Issue-Key: bedrock-agent-runtime.flow-execution-inline-code-error-reports-flow-failure-event|_prepare#6|AssertionError|assert status == "Prepared", f"flow preparation ended in {status!r}, expected 'Prepared'"
Issue-Key: bedrock-agent-runtime.flow-execution-input-type-mismatch-fails-before-any-node|_prepare#6|AssertionError|assert status == "Prepared", f"flow preparation ended in {status!r}, expected 'Prepared'"
Issue-Key: bedrock-agent-runtime|InvokeFlow|validationException
Issue-Key: bedrock-agent-runtime.flow-execution-lambda-node-error-lists-failed|run#20|AssertionError|assert ok["status"] == "Succeeded", f"control execution should succeed, got {ok['status']!r}"
Issue-Key: bedrock-agent-runtime.stop-flow-execution-on-succeeded-execution-rejected|run#29|AssertionError|assert done["status"] == "Succeeded", (
Issue-Key: bedrock-agent-runtime.stopped-flow-execution-stays-aborted-after-its-lambda-node-returns|run#29|AssertionError|assert control["status"] == "Succeeded", (
Issue-Key: bedrock-agent-runtime.get-flow-execution-through-another-alias-rejected|run#19|AssertionError|assert got == (
Issue-Key: bedrock-agent-runtime.list-flow-execution-events-through-another-alias-rejected|run#22|AssertionError|assert got == (
Issue-Key: bedrock-agent-runtime.get-flow-execution-through-sibling-alias-not-found|run#11|AssertionError|assert _RESOURCE_ID.fullmatch(flow_id), (
Issue-Key: bedrock-agent-runtime.invoke-flow-input-type-mismatch-streams-validation-exception|_refusal#8|AssertionError|raise AssertionError(f"document {document!r} was accepted; the stream completed with {prior!r}")
Issue-Key: bedrock-agent-runtime.invoke-flow-mistyped-input-document-rejected-in-stream|_assert_type_refused#4|AssertionError|assert err is not None, f"a {actual} document {document!r} was accepted: {events!r}"
Issue-Key: bedrock-agent-runtime.invoke-flow-unknown-execution-id-streams-validation-exception|_failed_with#21|AssertionError|raise AssertionError(f"executionId {execution_id!r} was accepted; the stream completed with {head!r}")
Issue-Key: bedrock-agent-runtime.invoke-flow-completed-execution-id-not-resumable-rejected|run#12|AssertionError|assert resumed_err is not None, "continuing the finished execution was not refused"
Issue-Key: bedrock-agent-runtime.invoke-flow-draft-alias-after-update-without-prepare-rejected|run#15|AssertionError|assert refused == ("error", _NOT_PREPARED), (
Issue-Key: bedrock-agent-runtime.start-flow-execution-reused-name-rejected|run#17|AssertionError|assert (code == "ConflictException" and message.startswith(same_head)
Issue-Key: bedrock-agent-runtime.start-flow-execution-unknown-input-node-rejected|run#20|AssertionError|assert got == expected, (
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock-agent-runtime flows diverge across prepare, execution and refusal paths: 10 net-new causes — over-strict connection type validation, prepare never settles, only Input/Output nodes execute, alias-mismatch 404s, and five reworded/absent refusals

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeFlow

## Description
Ten distinct root causes in the bedrock-agent-runtime flow surface (InvokeFlow, StartFlowExecution, GetFlowExecution, ListFlowExecutionEvents, StopFlowExecution) and the PrepareFlow/CreateFlow state they depend on. All ten are in the bedrock-agent-runtime service; each must be closed before this issue is submitted.

Cause 1 of 10: PrepareFlow over-validates connection data types, reporting IncompatibleConnectionDataType for a connection whose target input carries an extraction expression.
AWS: recorded behavior only — a flow whose Input node declares an output of type Object may be connected to a target input of type Number or String when that input declares an `expression` (e.g. "$.data.amount", "$.data.tag"); AWS projects the Object through the expression and type-checks the PROJECTION, not the source output's declared type. PrepareFlow brings such a flow to status Prepared with no validations.
Emulator: PrepareFlow compares the source output's declared type to the target input's declared type directly, leaves the flow in status Failed and emits one validation per connection, type IncompatibleConnectionDataType, severity Error, message "Connection <connectionName> carries output <sourceOutput> of type <sourceType> into input <targetInput> of type <targetType>. The data types must be the same." with details.incompatibleConnectionDataType.connection = <connectionName>. Every connection out of an Object-typed Input node is condemned, so the flow can never be invoked.
Why: a client cannot author any flow that routes on, or extracts from, a structured input — the single most common flow shape — because PrepareFlow refuses it; the flow is permanently Failed and InvokeFlow/StartFlowExecution are unreachable.
Fix: the connection type-compatibility validation in the PrepareFlow validator — when the target input declares an expression, type-check the projected value, not the source output's declared type.
Traces: invoke-flow-condition-input-type-mismatch-rejected, invoke-flow-condition-node-routes-to-first-satisfied-condition, invoke-flow-iterator-collector-reassembles-array-in-order, invoke-flow-version-alias-serves-snapshot-after-draft-change.

Cause 2 of 10: PrepareFlow never leaves status Preparing.
AWS: recorded behavior only — after PrepareFlow, GetFlow reaches a terminal status (Prepared, or Failed with validations) within the operation's normal settling time; Preparing is transient.
Emulator: GetFlow keeps answering status 'Preparing' until the caller's wait expires; no terminal status is ever reached.
Why: a client polling GetFlow for Prepared blocks forever and can never create a flow version or invoke the flow.
Fix: the PrepareFlow handler's flow-status state transition — Preparing must advance to Prepared or Failed.
Traces: flow-execution-inline-code-error-reports-flow-failure-event, flow-execution-input-type-mismatch-fails-before-any-node.

Cause 3 of 10: flow execution performs no node type other than Input and Output — Condition, Prompt, LambdaFunction and InlineCode nodes abort the run.
AWS: recorded behavior only — a prepared flow containing a Condition node routes the input down the first satisfied condition in declared order; a LambdaFunction node invokes its function; an InlineCode node runs its code; a Prompt node calls its model. The execution reaches status Succeeded and InvokeFlow streams flowOutputEvent then flowCompletionEvent {completionReason: SUCCESS}.
Emulator: the run is aborted and the caller receives, as an InvokeFlow stream error event with code `validationException` (or, via StartFlowExecution, as flow failure events NODE_EXECUTION_FAILED and DEPENDENCY_FAILED with execution status Failed), the emulator-authored message "the node <nodeName> is of type <NodeType>, whose work — a model, a function or another service answering — nothing in this emulator performs, so no value can be computed for it; a flow this service runs sends its input node straight to its output nodes". The fixed text is that whole sentence, which AWS never emits; the variable parts are the node name and node type from the flow definition. Observed for node types Condition, Prompt and LambdaFunction.
Why: every non-trivial flow fails. A client sees a validationException naming its own (valid) node definition, or an execution that reports Failed, so flow routing, Lambda integration and inline code are all unusable; checks that merely need a working control execution to grade something else (stop/abort semantics, execution listing) cannot get one.
Fix: the flow-execution engine behind InvokeFlow/StartFlowExecution — node execution for Condition (evaluate the expression and route), LambdaFunction (invoke the target), InlineCode (run the code) and Prompt; and the failure path must not emit this emulator-authored sentence.
Traces: invoke-flow-condition-node-routes-to-first-matching-condition, list-flow-executions-reports-stopped-execution-as-aborted, flow-execution-lambda-node-error-lists-failed, stop-flow-execution-on-succeeded-execution-rejected, stopped-flow-execution-stays-aborted-after-its-lambda-node-returns.

Cause 4 of 10: GetFlowExecution and ListFlowExecutionEvents do not check the alias embedded in an execution ARN against flowAliasIdentifier, answering 404 not-found instead of the 400 alias-mismatch refusal.
AWS: recorded behavior only — when executionIdentifier is a full execution ARN whose /alias/<id> segment names a different alias than flowAliasIdentifier, both operations refuse with ValidationException (modeled httpError 400) and the message "The Flow Alias Id in the FlowAliasIdentifier does not match the Flow Alias Id in the FlowExecutionIdentifier. Please confirm the Flow Alias Ids match when using an Arn for the FlowExecutionIdentifier." — that text is entirely fixed, with no request values interpolated.
Emulator: both operations answer ResourceNotFoundException with the message "Resource not found, type: FlowExecution".
Why: a client that passes a mismatched alias is told the execution does not exist rather than that its two identifiers disagree, so it retries or gives up instead of correcting the alias; and a genuinely missing execution becomes indistinguishable from a mismatched one.
Fix: the identifier validation of the GetFlowExecution and ListFlowExecutionEvents handlers — compare the ARN's alias segment to flowAliasIdentifier before resolving the execution, and raise the 400 ValidationException above.
Traces: get-flow-execution-through-another-alias-rejected, list-flow-execution-events-through-another-alias-rejected.

Cause 5 of 10: CreateFlow mints a flow id outside the alphabet AWS uses.
AWS: recorded behavior only — a flow id is 10 characters drawn from [0-9A-Z] (uppercase alphanumerics). Smithy's FlowIdentifier pattern ^...([0-9a-zA-Z]{10})$ is the looser accept-pattern for input; the ids AWS actually mints are uppercase.
Emulator: CreateFlow returns a 10-character id that is not 10 of [0-9A-Z] (it contains characters outside that set), so a client that round-trips the id through anything keyed on AWS's minted form rejects it.
Why: a client that validates or canonicalises a flow id against the form AWS mints (and anything that case-normalises or embeds the id in an ARN compared literally) rejects a flow the service just created.
Fix: the flow-id minting in the CreateFlow handler's state — draw from [0-9A-Z].
Traces: get-flow-execution-through-sibling-alias-not-found.

Cause 6 of 10: InvokeFlow does not type-check the input document against the input node's declared output type.
AWS: recorded behavior only — when inputs[0].content.document does not match the declared type of the named nodeOutputName on the Input node, InvokeFlow emits a stream error event with code `validationException` and message "Expected type for Node Output document of Node <nodeName> to be <DECLARED_TYPE>. Actual type is <ACTUAL_TYPE>." and NO flowOutputEvent precedes it. The fixed text is "Expected type for Node Output " / " of Node " / " to be " / ". Actual type is " / "."; the variable parts are the output name, node name, the declared type and the actual type of the submitted document.
Emulator: the mistyped document is ACCEPTED. The stream completes with flowOutputEvent carrying the document through unchanged and flowCompletionEvent {completionReason: SUCCESS}.
Why: a client's type contract on its own flow is not enforced, so a wrongly typed value flows through the whole graph and the client gets a plausible-looking SUCCESS with corrupt data instead of a 400-class refusal it can surface.
Fix: the input validation of the InvokeFlow handler — validate inputs[].content.document against the Input node's declared output type and emit the validationException stream event before any output.
Traces: invoke-flow-input-type-mismatch-streams-validation-exception, invoke-flow-mistyped-input-document-rejected-in-stream.

Cause 7 of 10: InvokeFlow accepts an executionId with no pending conversation (never issued, or belonging to a finished execution) and runs the flow anyway.
AWS: recorded behavior only — Smithy models InvokeFlow.executionId (FlowExecutionId, length 2..100, pattern ^[0-9a-zA-Z._:-]+$). When the id names no resumable conversation — because it was never issued, or because its execution already completed — the stream ends on a `validationException` event with message "Error retrieving session context: <executionId>"; the fixed text is "Error retrieving session context: " and the variable part is the executionId copied from the request.
Emulator: the request succeeds. The stream completes with flowOutputEvent and flowCompletionEvent {completionReason: SUCCESS}, as if a fresh execution had been started; no refusal is emitted for a never-issued id and none for a completed execution's id.
Why: a client resuming a conversation silently starts a brand-new one, losing the session context it asked for, and a client reusing a completed execution's id gets a bogus success instead of learning the execution is finished.
Fix: the executionId resolution/validation of the InvokeFlow handler — resolve the id against pending conversations and emit the validationException stream event above when there is none.
Traces: invoke-flow-unknown-execution-id-streams-validation-exception, invoke-flow-completed-execution-id-not-resumable-rejected.

Cause 8 of 10: the InvokeFlow unprepared-alias refusal message is emulator-authored.
AWS: recorded behavior only — invoking an alias that routes to a flow version which is not prepared ends the stream on a `validationException` event with the message "The flow alias is not in prepared state. Retry the request when the state is prepared." — entirely fixed text, with no version, alias or status value interpolated.
Emulator: the code is right (`validationException`) but the message is "the flow version DRAFT this alias routes to is in status NotPrepared: prepare the flow before invoking it", which interpolates the version and status and uses wording AWS never emits.
Why: a client matching on AWS's refusal sentence (log assertions, user-facing error mapping, support tooling) does not recognise the condition.
Fix: the not-prepared refusal message in the InvokeFlow handler's validation.
Traces: invoke-flow-draft-alias-after-update-without-prepare-rejected.

Cause 9 of 10: the StartFlowExecution duplicate-execution-name ConflictException message is emulator-authored, and does not distinguish a matching from a differing input.
AWS: recorded behavior only — reusing a flowExecutionName the alias already holds is refused with ConflictException, worded by whether the submitted input matches the existing execution's input. Same input: the message begins "Couldn't perform StartFlowExecution operation because there already exists an execution with id " and ends with ".", and must NOT contain "doesn't match the input". Different input: it begins "Couldn't perform StartFlowExecution operation because there already exists execution with id " and ends " and the existing execution's input doesn't match the input provided in the current request.". The fixed text is those two templates; the only variable part is the execution id taken from the request/state.
Emulator: ConflictException is raised (correct code) with the single emulator-authored message "Flow execution <name> already exists and cannot be started again." — one wording for both cases, so the input-matches/input-differs distinction is lost entirely.
Why: a client cannot tell an idempotent re-submission (same input) from a genuine name collision with different data, which is exactly the decision AWS's two wordings exist to support.
Fix: the duplicate-name conflict path in the StartFlowExecution handler's validation — compare the submitted input to the existing execution's input and emit the matching template.
Traces: start-flow-execution-reused-name-rejected.

Cause 10 of 10: StartFlowExecution with an input addressed to a node the flow does not have is refused 424 DependencyFailedException instead of 400 ValidationException naming the real input node.
AWS: recorded behavior only — Smithy models both ValidationException (httpError 400) and DependencyFailedException (httpError 424) on StartFlowExecution. An inputs[].nodeName that is not the flow version's input node is a request-validation error: ValidationException with the message "Node '<submittedNodeName>' is not the flow input node of the flow version being executed. Provide '<actualInputNodeName>'." The fixed text is "Node '" / "' is not the flow input node of the flow version being executed. Provide '" / "'."; the variable parts are the submitted node name from the request and the flow's actual input node name from state. No execution is created.
Emulator: the request is refused with DependencyFailedException and the generic message "There was an issue with a dependency. Check the resource configurations and retry the request." — a 424 that names neither the bad node nor the right one.
Why: the caller's own typo is reported as a downstream dependency failure, which clients treat as retryable and which carries no information about what to fix.
Fix: the inputs[].nodeName validation of the StartFlowExecution handler — validate the node name against the flow version's input node and raise the 400 ValidationException above before touching any dependency.
Traces: start-flow-execution-unknown-input-node-rejected.

## Evidence
Cause 1 — Expected: GetFlow status 'Prepared' with no validations. Actual: AssertionError: PrepareFlow must bring the flow to Prepared; status 'Failed', validations [{'message': "Connection EntryToRouter carries output document of type Object into input amount of type Number. The data types must be the same.", 'severity': 'Error', 'details': {'incompatibleConnectionDataType': {'connection': 'EntryToRouter'}}, 'type': 'IncompatibleConnectionDataType'}, ...] — for a definition whose Input node declares outputs [{'name':'document','type':'Object'}] and whose Condition input declares {'name':'amount','type':'Number','expression':'$.data.amount'}.

Cause 2 — Expected: a terminal GetFlow status. Actual: AssertionError: flow preparation ended in 'Preparing', expected 'Prepared'.

Cause 3 — Expected: flowOutputEvent + flowCompletionEvent {'completionReason':'SUCCESS'}, execution status Succeeded. Actual: botocore.exceptions.EventStreamError: An error occurred (validationException) when calling the InvokeFlow operation: the node Threshold is of type Condition, whose work — a model, a function or another service answering — nothing in this emulator performs, so no value can be computed for it; a flow this service runs sends its input node straight to its output nodes (same sentence with 'the node Ask is of type Prompt'); and via StartFlowExecution: AssertionError: the unheld execution must Succeed; got 'Failed' with failure events [('NODE_EXECUTION_FAILED', 'the node Hook is of type LambdaFunction, whose work ... straight to its output nodes'), ('DEPENDENCY_FAILED', '<same>')]; and AssertionError: control execution should succeed, got 'Failed'.

Cause 4 — Expected: ('ValidationException', 'The Flow Alias Id in the FlowAliasIdentifier does not match the Flow Alias Id in the FlowExecutionIdentifier. Please confirm the Flow Alias Ids match when using an Arn for the FlowExecutionIdentifier.'). Actual: ('ResourceNotFoundException', 'Resource not found, type: FlowExecution') from both GetFlowExecution and ListFlowExecutionEvents.

Cause 5 — Expected: a flow id matching [0-9A-Z]{10}. Actual: AssertionError: CreateFlow returned a 10-character id, not 10 of [0-9A-Z].

Cause 6 — Expected: stream error ('validationException', "Expected type for Node Output document of Node <Input node> to be NUMBER. Actual type is STRING.") with no preceding output. Actual: AssertionError: document 'thirty-seven' was accepted; the stream completed with [{'flowOutputEvent': {'content': {'document': 'thirty-seven'}, 'nodeName': 'Reported'}}, {'flowCompletionEvent': {'completionReason': 'SUCCESS'}}]; likewise AssertionError: a STRING document 'forty-two' was accepted: [{'flowOutputEvent': ...}, {'flowCompletionEvent': {'completionReason': 'SUCCESS'}}].

Cause 7 — Expected: stream error ('validationException', 'Error retrieving session context: <executionId>'). Actual: AssertionError: executionId 'sim2real-exec-<id>' was accepted; the stream completed with [{'flowOutputEvent': {...}}, {'flowCompletionEvent': {'completionReason': 'SUCCESS'}}]; and AssertionError: continuing the finished execution was not refused.

Cause 8 — Expected: ('validationException', 'The flow alias is not in prepared state. Retry the request when the state is prepared.'). Actual: ('validationException', 'the flow version DRAFT this alias routes to is in status NotPrepared: prepare the flow before invoking it').

Cause 9 — Expected: ConflictException whose message starts "Couldn't perform StartFlowExecution operation because there already exists an execution with id " and ends "." for a matching input, and starts "Couldn't perform StartFlowExecution operation because there already exists execution with id " and ends " and the existing execution's input doesn't match the input provided in the current request." for a differing one. Actual: ConflictException: 'Flow execution <name> already exists and cannot be started again.'

Cause 10 — Expected: ('ValidationException', "Node '<submitted>' is not the flow input node of the flow version being executed. Provide '<actual>'."). Actual: ('DependencyFailedException', 'There was an issue with a dependency. Check the resource configurations and retry the request.').

## Steps to Reproduce
1. Boot the emulator and replay the bedrock-agent-runtime behavioral suite; all ten causes reproduce from the listed checks.
2. Cause 1: bedrock-agent.create_flow with an Input node declaring outputs=[{'name':'document','type':'Object'}] connected to a Condition input {'name':'amount','type':'Number','expression':'$.data.amount'}; prepare_flow; poll get_flow. Observe status Failed with IncompatibleConnectionDataType validations.
3. Cause 2: create any Input->Output flow, prepare_flow, poll get_flow. Observe status stuck at 'Preparing'.
4. Cause 3: prepare a flow containing a Condition, Prompt, LambdaFunction or InlineCode node, then invoke_flow (or start_flow_execution) over it. Observe the validationException stream event / Failed execution with the 'nothing in this emulator performs' message.
5. Cause 4: start an execution under alias A, create a second alias B over the same flow version, then get_flow_execution(flowAliasIdentifier=B, executionIdentifier=<A's execution ARN>) and list_flow_execution_events(same). Observe 404 ResourceNotFoundException 'Resource not found, type: FlowExecution'.
6. Cause 5: bedrock-agent.create_flow and inspect the returned id against [0-9A-Z]{10}.
7. Cause 6: prepare a flow whose Input node output 'document' is type Number, then invoke_flow with content={'document':'thirty-seven'}. Observe a SUCCESS stream instead of the validationException event.
8. Cause 7: invoke_flow with executionId=<a never-issued id>, then with the executionId of an execution that already completed. Both stream to SUCCESS.
9. Cause 8: create a flow version + alias, prepare, update the DRAFT without re-preparing, then invoke_flow against TSTALIASID. Compare the validationException message.
10. Cause 9: start_flow_execution with flowExecutionName=N and input X; wait for Succeeded; start_flow_execution again with name N and input X, then with name N and input Y. Compare both ConflictException messages.
11. Cause 10: prepare an Input('Entry')->Output flow, then start_flow_execution(inputs=[{'nodeName':'Elsewhere',...}]). Observe 424 DependencyFailedException.
