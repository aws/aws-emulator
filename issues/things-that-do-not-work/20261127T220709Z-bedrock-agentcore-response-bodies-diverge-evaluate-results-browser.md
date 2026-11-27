Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-browser-session-get-malformed-id-validation
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-browser-session-get-malformed-id-validation bedrock-agentcore-code-interpreter-tool-required-argument-errors bedrock-agentcore-evaluate-span-missing-content-refuses-every-targeted-trace bedrock-agentcore-evaluate-two-traces-each-refused
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|GetBrowserSession|2xx->2xx|$.viewPort
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|4xx->4xx|$.message
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|4xx->4xx|$.reason
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|4xx->2xx|$.message
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|4xx->2xx|$headers.content-type
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore|InvokeCodeInterpreter|4xx->2xx|status
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].errorCode
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].explanation
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].label
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].tokenUsage
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].value
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*].errorMessage
Issue-Key: bedrock-agentcore|Evaluate|2xx->2xx|$.evaluationResults[*]
Issue-Commit: d1bf6e73c350197b177d1f9613fd03fd8fa1db6f

# bedrock-agentcore response bodies diverge: Evaluate results, browser-session viewPort and code-interpreter tool arguments

> _Note: this issue was identified against the code base at commit `d1bf6e73c350197b177d1f9613fd03fd8fa1db6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Evaluate

## Description
This bundle carries 6 net-new causes in bedrock-agentcore SUCCESS-path and tool-argument handling: a session read drops a modeled structure, the code-interpreter tool-argument gate refuses with the wrong body and misses one required-member rule entirely, and Evaluate's per-trace results are wrong in three distinct ways. Each cause must be closed before this issue is submitted.

Cause 1 of 6: GetBrowserSession omits the session's viewPort.
AWS: Smithy: GetBrowserSessionResponse has member viewPort targeting ViewPort{width @required (320..3840), height @required (240..2160)}. A session started with a viewport (or with the service default) reads back with viewPort populated; the recording shows both dimensions present.
Emulator: 200 with viewPort absent from the body entirely.
Why: a client that sizes its automation/live-view to session.viewPort gets null and either crashes or silently falls back to its own default, so rendered page geometry no longer matches the session.
Fix: GetBrowserSession's response serializer / the browser-session state record must carry and emit the viewPort (width+height) established at StartBrowserSession.
Traces: bedrock-agentcore-browser-session-get-malformed-id-validation#task#4.

Cause 2 of 6: InvokeCodeInterpreter's missing-argument refusal is re-worded and carries a reason member AWS omits.
AWS: Smithy: InvokeCodeInterpreter POST /code-interpreters/{codeInterpreterIdentifier}/tools/invoke, name targets ToolName (executeCode, executeCommand, readFiles, ...), arguments targets ToolArguments{code, language, command, path, paths, content, ...}; ValidationException is httpError 400. For an executeCode invocation missing code and/or language the recorded refusal is 400 with the fixed message "code and language fields are required in argument." and NO `reason` member in the body (recorded behavior; the model's required `reason` is not serialized for this refusal).
Emulator: 400 whose message is the per-argument sentence "The executeCode tool requires the code argument." and whose body additionally carries "reason": "FieldValidationFailed".
Why: AWS reports one fixed sentence covering both fields; the emulator emits a different, argument-specific sentence plus an extra top-level member, so message- and shape-asserting clients diverge. The same spurious `reason` member was already filed for InvokeBrowser (20261120T124700Z-bedrock-agentcore-invokebrowser-validationexception-body-carries-a-reason-member-aws.md, fixed); that issue does not cover InvokeCodeInterpreter and carries no replay key for this step.
Fix: InvokeCodeInterpreter's tool-argument validation and its ValidationException serializer (single fixed sentence, no `reason`).
Traces: bedrock-agentcore-code-interpreter-tool-required-argument-errors#task#2 (cluster also covers task#3, task#4, task#5 - the other missing-argument permutations).

Cause 3 of 6: InvokeCodeInterpreter accepts a content block with no path and no text/blob.
AWS: Smithy: ToolArguments.content targets InputContentBlockList of InputContentBlock{path @required, text, blob}. For a write-files style invocation whose content entry omits path and carries neither text nor blob, AWS refuses 400 ValidationException with the fixed message "path field and one of (text, blob) field is required in content."
Emulator: 200 with content-type application/vnd.amazon.eventstream, no message and no x-amzn-errortype - the invalid invocation is executed and streamed back as a success.
Why: a malformed write is silently accepted, so a client that depends on the refusal to detect a bad request proceeds as if files were written; nothing in the stream reports the violated rule.
Fix: InvokeCodeInterpreter's tool-argument validation must enforce InputContentBlock's required path and the text/blob one-of rule before the tool runs.
Traces: bedrock-agentcore-code-interpreter-tool-required-argument-errors#task#6.

Cause 4 of 6: Evaluate marks a trace whose span data is complete as LogEventMissingException.
AWS: Smithy: Evaluate POST /evaluations/evaluate/{evaluatorId}, output evaluationResults (@required list of EvaluationResultContent). For a targeted trace whose span carries its log event, the recorded result element carries label, value, explanation and tokenUsage{inputTokens, outputTokens, totalTokens} and NO errorCode/errorMessage (recorded behavior only - the per-member semantics are not further modeled).
Emulator: 200 whose evaluationResults[0] has label, value, explanation and tokenUsage all absent and instead carries errorCode "LogEventMissingException".
Why: the evaluator scores nothing for a trace AWS does score - a caller reading label/value for its quality gate finds them null and sees a refusal code for data it did supply, so the evaluation is unusable.
Fix: Evaluate's handler - the span-to-log-event correlation that decides between a scored result and a LogEventMissingException result.
Traces: bedrock-agentcore-evaluate-span-missing-content-refuses-every-targeted-trace#task#1.

Cause 5 of 6: Evaluate's LogEventMissingException errorMessage names the wrong span.
AWS: recorded behavior only. The message template is fixed text "Session span data is incomplete. Span with ID: " + the id of the span that is missing its log event + " and name: " + that span's name + " is missing a corresponding log event. Please retry the request with the complete span data". Both the span id and the span name are values taken from the request's span data - the id must be the span actually lacking a log event.
Emulator: emits the template with a different span id from the same request than the one that is incomplete (the name part matches).
Why: the message is the only place the caller learns WHICH span to resend; naming an unrelated span sends the client to re-upload the wrong span and the retry fails identically. (This is not a per-run value: the id is request-supplied, and the emulator picks the wrong element of the request.)
Fix: Evaluate's handler must carry the identity of the span that failed correlation into the errorMessage instead of selecting a different span from the submitted set.
Traces: bedrock-agentcore-evaluate-span-missing-content-refuses-every-targeted-trace#task#2 (cluster also covers bedrock-agentcore-evaluate-two-traces-each-refused#task#2).

Cause 6 of 6: Evaluate's second result element diverges for a multi-trace request.
AWS: recorded behavior only. With more than one targeted trace, evaluationResults carries one element per (evaluator, trace) pair; the recording's element at index 1 pairs evaluator Builtin.Helpfulness with its trace's spanContext{sessionId, traceId} and the LogEventMissingException errorMessage naming that trace's incomplete span.
Emulator: the element at index 1 does not match the recorded element (reported as missing+extra at $.evaluationResults[1]): the evaluatorArn/evaluatorId/evaluatorName and spanContext agree, but the errorMessage again names a span id other than the incomplete one, so the per-trace result cannot be matched to the trace that failed.
Why: for a multi-trace Evaluate the caller cannot attribute refusals to traces, so a batch of evaluations reports the wrong trace as incomplete.
Fix: Evaluate's handler - per-trace result assembly (one element per evaluator/trace pair with that trace's own failing span) in the response builder.
Traces: bedrock-agentcore-evaluate-span-missing-content-refuses-every-targeted-trace#task#3 (cluster also covers bedrock-agentcore-evaluate-two-traces-each-refused#task#1).

## Evidence
Cause 1 - bedrock-agentcore-browser-session-get-malformed-id-validation#task#4 (GetBrowserSession): expected $.viewPort = {height, width} present; actual $.viewPort missing (null) on an otherwise 200 response.
Cause 2 - bedrock-agentcore-code-interpreter-tool-required-argument-errors#task#2 (InvokeCodeInterpreter), 400 both sides: expected $.message "code and language fields are required in argument." and no $.reason; actual $.message "The executeCode tool requires the code argument." and $.reason "FieldValidationFailed". Same cluster for task#3/#4/#5.
Cause 3 - bedrock-agentcore-code-interpreter-tool-required-argument-errors#task#6 (InvokeCodeInterpreter): expected 400 with $.message "path field and one of (text, blob) field is required in content.", content-type application/json and an x-amzn-errortype header; actual 200, content-type application/vnd.amazon.eventstream, no message, no x-amzn-errortype.
Cause 4 - bedrock-agentcore-evaluate-span-missing-content-refuses-every-targeted-trace#task#1 (Evaluate), 200 both sides: expected $.evaluationResults[0].label/.value/.explanation/.tokenUsage{inputTokens,outputTokens,totalTokens} present and no errorCode; actual all four missing and errorCode "LogEventMissingException" present.
Cause 5 - ...#task#2 (Evaluate): expected $.evaluationResults[0].errorMessage "Session span data is incomplete. Span with ID: a7b6c5d4e3f20118 and name: invoke_agent OrdersDesk is missing a corresponding log event. Please retry the request with the complete span data"; actual the same fixed text with span id 3c4d5e6f70819a2b - a different span of the same request.
Cause 6 - ...#task#3 (Evaluate): $.evaluationResults[1] reported missing+extra - expected element carries evaluatorId Builtin.Helpfulness, context.spanContext{sessionId orders-desk-session-..., traceId e1d2...}, errorCode LogEventMissingException and the errorMessage naming span a7b6c5d4e3f20118; actual element carries the same evaluator/context but the errorMessage names span 3c4d5e6f70819a2b.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator and inspect these steps:
1. bedrock-agentcore-browser-session-get-malformed-id-validation, step task#4 - GET /browsers/{browserIdentifier}/sessions/get?sessionId=<live session>; check for viewPort in the body.
2. bedrock-agentcore-code-interpreter-tool-required-argument-errors, step task#2 - POST /code-interpreters/{id}/tools/invoke with name=executeCode and arguments missing code/language; compare message and the presence of `reason`.
3. Same fixture, step task#6 - POST the same URI with a content block that has no path and neither text nor blob; expect 400, observe 200 eventstream.
4. bedrock-agentcore-evaluate-span-missing-content-refuses-every-targeted-trace, step task#1 - POST /evaluations/evaluate/{evaluatorId} for a trace with complete span data; compare evaluationResults[0] members.
5. Same fixture, step task#2 - compare the span id inside evaluationResults[0].errorMessage against the span that actually lacks a log event.
6. Same fixture, step task#3 (and bedrock-agentcore-evaluate-two-traces-each-refused#task#1) - a multi-trace Evaluate; compare evaluationResults[1] element-by-element.
