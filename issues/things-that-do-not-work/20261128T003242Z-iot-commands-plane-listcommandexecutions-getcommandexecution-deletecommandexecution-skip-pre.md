Issue-Source: trace-sweep-divergence
Issue-Trace: iot-command-execution-listing-filter-conflicts
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-command-execution-listing-filter-conflicts iot-command-execution-listing-scope-errors iot-list-command-executions-status-filter-needs-a-target iot-command-duplicate-and-execution-errors iot-get-command-execution-target-arn-validation
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|ListCommandExecutions|4xx->2xx|$.message
Issue-Key: iot|ListCommandExecutions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|ListCommandExecutions|4xx->2xx|status
Issue-Key: iot|ListCommandExecutions|4xx->4xx|$.message
Issue-Key: iot|DeleteCommandExecution|4xx->2xx|$
Issue-Key: iot|DeleteCommandExecution|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteCommandExecution|4xx->2xx|status
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|$.Message
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|status
Issue-Key: iot|GetCommandExecution|4xx->4xx|$.message
Issue-Key: iot|GetCommandExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|GetCommandExecution|4xx->4xx|status
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot Commands plane: ListCommandExecutions/GetCommandExecution/DeleteCommandExecution skip pre-flight refusals and author their own text

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommandExecutions

## Description
Six independent net-new root causes in the iot Commands plane (ListCommandExecutions, DeleteCommandExecution, GetCommandExecution). Each cause must be closed before submitting. Related but NOT covering these: open issue 20261127T130644Z-iot-7-net-new-causes-listcommandexecutions-three-validationexception.md is a BEHAVIORAL issue whose cause 1 names the same both-scopes wording as Cause 2 here and whose causes 2-3 name other ListCommandExecutions messages; it carries no wire replay key, and the wire replay reports all six clusters below as unowned.

Cause 1 of 6: ListCommandExecutions accepts a listing that carries NO time window instead of refusing it.
AWS: Smithy ListCommandExecutions (POST /command-executions) models startedTimeFilter and completedTimeFilter (TimeFilter) as optional members and ValidationException (httpError 400). The model does not state that one is required, so this is recorded behavior only: a request that supplies neither window is refused HTTP 400 ValidationException with the message "No 'startedTimeFilter' or 'completedTimeFilter' parameter value was provided. At least one valid parameter is expected." That sentence is entirely fixed text; the only variable parts are the two member names, and no request value is interpolated.
Emulator: the request is ACCEPTED with HTTP 200 and a commandExecutions page; no error body and no x-amzn-errortype header are emitted.
Why: a client that relies on AWS refusing an unscoped listing gets a page the real service never serves, so pagination/filter code written against AWS is never exercised and an unbounded scan is silently permitted.
Fix: ListCommandExecutions' input validation, before the listing is computed (services/service-com-amazonaws-iot-awsiotservice/src/ops/list_command_executions.rs).
Traces: iot-command-execution-listing-filter-conflicts#task#3 (cluster of 5: also iot-command-execution-listing-scope-errors#task#7, #task#8 and iot-list-command-executions-status-filter-needs-a-target#task#3, #task#4).

Cause 2 of 6: the both-scopes refusal has the right class and status but emulator-authored text.
AWS: ListCommandExecutions takes targetArn (TargetArn @length 0..2048) and commandArn (CommandArn) as optional members and models ValidationException (400). Recorded behavior: a request carrying BOTH is refused 400 ValidationException with "Both 'targetArn' and 'commandArn' parameter values were provided. Only one valid parameter value is accepted." The whole sentence is fixed text; the only variable parts are the two member names the request supplied together, named in that order.
Emulator: status 400 is right, but the message is "only one of commandArn and targetArn may be provided" — different fixed text, lower-cased, member names unquoted and in the opposite order, and the "Only one valid parameter value is accepted." sentence absent.
Why: the message is the only place AWS reports which pair of scopes it rejected; whole-message comparisons and log scrapers keyed to AWS's sentence never match.
Fix: the refusal-message construction in the same handler's scope-conflict branch.
Traces: iot-command-execution-listing-filter-conflicts#task#6 (cluster of 5: also #task#7, #task#8 and iot-command-execution-listing-scope-errors#task#5, #task#6).

Cause 3 of 6: a listing scoped to a commandArn whose command does not exist answers 200 instead of 404.
AWS: ListCommandExecutions models ResourceNotFoundException (httpError 404). Recorded behavior: a commandArn naming a command that is absent from the account is refused 404 with the message "Command <commandId> cannot be found." Fixed text: "Command " and " cannot be found."; the only variable part is the command id taken from the commandArn in the request.
Emulator: HTTP 200 with no error body — the absent command is treated as a command with no executions.
Why: a client cannot distinguish "this command has no executions" from "this command does not exist", which is exactly the distinction AWS's 404 carries; cleanup and polling loops written against AWS never terminate on the not-found path.
Fix: resolve the commandArn against the command store before listing and raise ResourceNotFoundException with the sentence above (same handler).
Traces: iot-command-execution-listing-scope-errors#task#3.

Cause 4 of 6: DeleteCommandExecution answers 200 for a targetArn that is not in the account instead of refusing it.
AWS: Smithy DeleteCommandExecution (DELETE /command-executions/{executionId}) requires executionId (@httpLabel, CommandExecutionId @length 1..64 @pattern ^[a-zA-Z0-9_-]+$) and targetArn (@httpQuery, @length 0..2048), with an empty output. AccessDeniedException is not modeled on this operation, so this is recorded behavior only: a targetArn that does not resolve in the caller's account is refused HTTP 403 with x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/ and a body whose member is capital-M "Message" carrying the fixed sentence "targetArn not found in account" (no request value interpolated).
Emulator: HTTP 200 with an empty body — the delete is reported as successful against a target that does not exist.
Why: a client deleting executions on a mistyped or cross-account target is told the delete succeeded; the authorization boundary AWS enforces here is absent, and error-handling code for the 403 path is unreachable.
Fix: DeleteCommandExecution's pre-flight target resolution and its AccessDeniedException construction (services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_command_execution.rs).
Traces: iot-command-duplicate-and-execution-errors#task#4.

Cause 5 of 6: DeleteCommandExecution's validation order is inverted — an unresolvable targetArn is reported as an executionId pattern violation.
AWS: for the same operation, when the request carries BOTH an executionId that violates CommandExecutionId's @pattern ^[a-zA-Z0-9_-]+$ and a targetArn that is not in the account, the recorded answer is the target refusal: HTTP 403, x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/, Message "targetArn not found in account" — i.e. target resolution precedes executionId pattern validation.
Emulator: HTTP 400 with x-amzn-errortype ValidationException and Message "1 validation error detected: Value at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+" — the pattern check runs first (and, separately, drops the rejected value; that formatter defect is filed as its own issue).
Why: a client that keys on the 403/AccessDeniedException pair to detect a wrong target sees a ValidationException about a different member, so it retries or reports the wrong remediation.
Fix: order the pre-flight checks in DeleteCommandExecution so target resolution runs before the executionId constraint report (same handler).
Traces: iot-command-duplicate-and-execution-errors#task#5.

Cause 6 of 6: GetCommandExecution reports a missing execution as a target authorization failure.
AWS: Smithy GetCommandExecution (GET /command-executions/{executionId}?targetArn=...) models ResourceNotFoundException (404) and ValidationException (400). Recorded behavior: an executionId that satisfies the pattern but names no stored execution for a resolvable target is refused HTTP 404 with the message "Command execution <executionId> cannot be found." Fixed text: "Command execution " and " cannot be found."; the only variable part is the executionId from the request path.
Emulator: HTTP 403 with x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/ and message "targetArn not found in account" — the wrong status, the wrong (unmodeled) error class and a sentence about a different member. Hypothesis: the handler resolves the targetArn before (and instead of) the execution lookup and conflates "no execution stored for this target" with "target not in account".
Why: a poller waiting for an execution to appear sees an authorization failure and aborts, instead of the retryable not-found AWS returns; the message also names the wrong request member, so the client's diagnostics point at the target rather than the execution id.
Fix: GetCommandExecution's lookup order and its not-found refusal (services/service-com-amazonaws-iot-awsiotservice/src/ops/get_command_execution.rs).
Traces: iot-get-command-execution-target-arn-validation#task#3.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up=true, 178 fixtures). Names, ids and 64-hex values below are one run's values filling the templates described per cause; they are not part of the requirement.

Cause 1 — iot-command-execution-listing-filter-conflicts#task#3, ListCommandExecutions: expected status 400 with $.message "No 'startedTimeFilter' or 'completedTimeFilter' parameter value was provided. At least one valid parameter is expected." and an x-amzn-errortype header; actual status 200, $.message missing, x-amzn-errortype missing. Same cluster: iot-command-execution-listing-scope-errors#task#7, #task#8; iot-list-command-executions-status-filter-needs-a-target#task#3, #task#4 (5 steps).

Cause 2 — iot-command-execution-listing-filter-conflicts#task#6, ListCommandExecutions: status 400 on both sides; expected $.message "Both 'targetArn' and 'commandArn' parameter values were provided. Only one valid parameter value is accepted.", actual "only one of commandArn and targetArn may be provided". Same cluster: #task#7, #task#8; iot-command-execution-listing-scope-errors#task#5, #task#6 (5 steps).

Cause 3 — iot-command-execution-listing-scope-errors#task#3, ListCommandExecutions: expected status 404 with $.message "Command sim2real-probe-iot-lce-absent-4f1ae224 cannot be found."; actual status 200 with no message and no x-amzn-errortype.

Cause 4 — iot-command-duplicate-and-execution-errors#task#4, DeleteCommandExecution: expected status 403, body {"Message": "targetArn not found in account"}, header x-amzn-errortype "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/"; actual status 200 with an empty body and no errortype header.

Cause 5 — iot-command-duplicate-and-execution-errors#task#5, DeleteCommandExecution: expected status 403 / x-amzn-errortype AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/ / $.Message "targetArn not found in account"; actual status 400 / x-amzn-errortype ValidationException / $.Message "1 validation error detected: Value at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+".

Cause 6 — iot-get-command-execution-target-arn-validation#task#3, GetCommandExecution: expected status 404 with $.message "Command execution sim2real-probe-exec-50137338b1d1 cannot be found."; actual status 403, $.message "targetArn not found in account", x-amzn-errortype "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/".

Model citations: ListCommandExecutions POST /command-executions, optional targetArn/commandArn/startedTimeFilter/completedTimeFilter/status/namespace/sortOrder, errors ValidationException(400), ResourceNotFoundException(404), ThrottlingException(400), InternalServerException(500), @paginated(nextToken/commandExecutions/maxResults). GetCommandExecution GET /command-executions/{executionId}, required executionId (@length 1..64, @pattern ^[a-zA-Z0-9_-]+$) and targetArn (@httpQuery), errors ValidationException(400), ResourceNotFoundException(404). DeleteCommandExecution DELETE /command-executions/{executionId}, required executionId + targetArn, errors ConflictException(409), ValidationException(400), ThrottlingException(400), InternalServerException(500) — AccessDeniedException is unmodeled, so its 403 is recorded behavior only.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and observe the six clusters named in Traces.
3. Equivalently, by hand against the emulator's iot endpoint:
   a. POST /command-executions with only a commandArn and no startedTimeFilter/completedTimeFilter — expect 400 ValidationException "No 'startedTimeFilter' or 'completedTimeFilter' parameter value was provided. At least one valid parameter is expected."; the emulator answers 200.
   b. POST /command-executions with both targetArn and commandArn (plus a time window) — compare the message against "Both 'targetArn' and 'commandArn' parameter values were provided. Only one valid parameter value is accepted.".
   c. POST /command-executions scoped to a commandArn for a command that was never created — expect 404 "Command <commandId> cannot be found."; the emulator answers 200.
   d. DELETE /command-executions/<valid-pattern-id>?targetArn=<arn of a thing that does not exist> — expect 403 AccessDeniedException with Message "targetArn not found in account"; the emulator answers 200.
   e. DELETE /command-executions/<id containing a '.'>?targetArn=<arn not in account> — expect the 403 target refusal, not a ValidationException about executionId.
   f. GET /command-executions/<pattern-valid id that was never started>?targetArn=<arn of an existing thing> — expect 404 "Command execution <executionId> cannot be found."; the emulator answers 403 "targetArn not found in account".
4. Re-replay the fixtures/steps listed in Traces to confirm the fix.
