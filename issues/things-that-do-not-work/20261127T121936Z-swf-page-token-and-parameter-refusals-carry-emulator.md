Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.list-domains-corrupt-page-token-rejected swf.list-domains-page-size-and-invalid-token-rejected swf.list-activity-types-page-bounds-and-invalid-token-rejected swf.list-open-workflow-executions-unissued-page-token-rejected swf.get-workflow-execution-history-unissued-page-token-rejected swf.start-workflow-execution-without-task-list-default-undefined-rejected swf.list-open-executions-unknown-domain-answers-empty swf.poll-for-activity-task-unregistered-domain-returns-empty-task
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.list-domains-corrupt-page-token-rejected|run#4|AssertionError|assert refusal == _TOKEN_REFUSAL, (
Issue-Key: swf.list-domains-page-size-and-invalid-token-rejected|run#20|AssertionError|assert (code, message) == _BAD_TOKEN, f"expected {_BAD_TOKEN!r}, got {(code, message)!r}"
Issue-Key: swf|ListActivityTypes|ValidationException
Issue-Key: swf.list-open-workflow-executions-unissued-page-token-rejected|run#15|AssertionError|assert got == _EXPECTED, f"an unissued page token gave {got!r}, expected {_EXPECTED!r}"
Issue-Key: swf.get-workflow-execution-history-unissued-page-token-rejected|run#19|AssertionError|assert got == _REFUSAL, f"a forged page token gave {got!r}, expected {_REFUSAL!r}"
Issue-Key: swf.start-workflow-execution-without-task-list-default-undefined-rejected|run#9|AssertionError|assert got == ("DefaultUndefinedFault", "taskList"), f"refused start answered {got!r}"
Issue-Key: swf|ListOpenWorkflowExecutions|UnknownResourceFault
Issue-Key: swf.poll-for-activity-task-unregistered-domain-returns-empty-task|run#23|AssertionError|assert (idle.get("activityId"), idle.get("startedEventId")) == (None, 0), (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# swf: page-token and parameter refusals carry emulator-authored text instead of AWS's wording, ListOpenWorkflowExecutions refuses an unknown domain, and an empty PollForActivityTask answer serializes activityId as "" (4 causes)

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDomains

## Description
Four net-new causes in SWF's pagination/validation and visibility paths.

Cause 1 of 4: every listing's unissued/forged `nextPageToken` refusal replaces AWS's message with an emulator-authored sentence. Failing checks: swf.list-domains-corrupt-page-token-rejected, swf.list-domains-page-size-and-invalid-token-rejected, swf.list-activity-types-page-bounds-and-invalid-token-rejected, swf.list-open-workflow-executions-unissued-page-token-rejected, swf.get-workflow-execution-history-unissued-page-token-rejected.
AWS: recorded behavior only — the SWF Smithy model declares no ValidationException at all, and the `nextPageToken` member documents only the 24-hour-expiry message. The recording shows a token the operation never issued refused with HTTP 400, error code `ValidationException`, message exactly `Invalid token`. That message is entirely fixed text: it interpolates no token, operation, domain or ordering, and it is the same for ListDomains, ListActivityTypes, ListOpenWorkflowExecutions and GetWorkflowExecutionHistory.
Emulator: the code and status are right but the message is a generated sentence that echoes the rejected token and describes the cursor's binding, e.g. `nextPageToken "forged-continuation-marker" was not issued for a reverseOrder=false listing of this account's domains; pass the token back unchanged, with every other argument unchanged`, `nextPageToken "not-a-real-token" was not issued for a reverseOrder=false listing of this domain's registered types; ...`, `nextPageToken "b3Blbi1saXN0aW5nLWZvcmdlZC10b2tlbg" was not issued for a reverseOrder=false listing of this domain's workflow executions ordered by start time; ...` and `nextPageToken "c2ltMnJlYWwtbm90LWlzc3VlZA==" was not issued for a reverseOrder=false read of this execution's history; pass the token back with the same reverseOrder`.
Why: the error message is part of the contract clients match on; a caller that distinguishes an invalid token from other validation failures by message gets an unrecognized string, and the emulator additionally leaks its own cursor design (and the raw token) into an AWS-facing message.
Fix: the page-token decoders in services/service-com-amazonaws-swf-simpleworkflowservice/src/state.rs (`decode_domain_page_token`, reached from ops/list_domains.rs line 213 via `Scaffolded::validation`, and its siblings for registered types, execution listings and history reads) must all produce the single ValidationException message `Invalid token`.
Traces: swf.list-domains-corrupt-page-token-rejected, swf.list-domains-page-size-and-invalid-token-rejected, swf.list-activity-types-page-bounds-and-invalid-token-rejected, swf.list-open-workflow-executions-unissued-page-token-rejected, swf.get-workflow-execution-history-unissued-page-token-rejected.

Cause 2 of 4: StartWorkflowExecution's DefaultUndefinedFault message is a sentence instead of the missing parameter's name. Failing check: swf.start-workflow-execution-without-task-list-default-undefined-rejected.
AWS: starting an execution with no `taskList` against a workflow type that defines no `defaultTaskList` is refused with HTTP 400, code `DefaultUndefinedFault`, and the message is just the name of the parameter that has no value — `taskList`. Smithy: StartWorkflowExecution models `DefaultUndefinedFault` (HTTP 400) with the doc "The StartWorkflowExecution API action was called without the required parameters set ... If these parameters aren't set and no default parameters were defined in the workflow type, this error is displayed"; its only member is `message`. The message's variable part is the member name that could not be defaulted (`taskList` here, the same shape for the other defaultable members); there is no surrounding prose.
Emulator: code `DefaultUndefinedFault` with message `taskList was not set on the request and the workflow type defines no defaultTaskList`.
Why: callers key off this message to learn WHICH parameter is undefined; an explanatory sentence breaks that parse while carrying no extra information.
Fix: the StartWorkflowExecution handler's defaulting step (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/start_workflow_execution.rs) must set DefaultUndefinedFault.message to the bare member name.
Traces: swf.start-workflow-execution-without-task-list-default-undefined-rejected.

Cause 3 of 4: ListOpenWorkflowExecutions refuses a never-registered domain instead of answering an empty list. Failing check: swf.list-open-executions-unknown-domain-answers-empty.
AWS: recorded behavior — a visibility listing for a domain name that was never registered answers HTTP 200 with `executionInfos: []` and no `nextPageToken`, while DescribeDomain on the same name is refused with `UnknownResourceFault` / `Unknown domain: <name>`. (The model does declare UnknownResourceFault for this operation, but the recording shows the eventually-consistent listing answering empty; the same divergence for PollForDecisionTask was already fixed under 20261109T235349Z-swf-pollfordecisiontask-raises-unknownresourcefault-for-a-never-registered.md, which covers only that operation.)
Emulator: `botocore.errorfactory.UnknownResourceFault: An error occurred (UnknownResourceFault) when calling the ListOpenWorkflowExecutions operation: Unknown domain: <name>` (HTTP 400).
Why: a client that sweeps candidate domain names, or that polls a listing while a domain is being created, gets an exception where AWS gives an empty page, so its loop aborts instead of continuing.
Fix: the ListOpenWorkflowExecutions handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/list_open_workflow_executions.rs) must treat an unknown domain as having no matching executions and return the empty list rather than raising UnknownResourceFault.
Traces: swf.list-open-executions-unknown-domain-answers-empty.

Cause 4 of 4: the empty-task answer of PollForActivityTask serializes `activityId` as an empty string instead of omitting it. Failing check: swf.poll-for-activity-task-unregistered-domain-returns-empty-task.
AWS: an idle poll (here on a domain that was never registered) answers HTTP 200 with a task that carries no work: `startedEventId` is 0 and no `activityId` member is present at all. Smithy: `ActivityTask.activityId` is `@required` with `@length(min:1, max:256)` and documented as "The unique ID of the task", so an empty string is not a value this member can legally carry — the empty answer omits it; `startedEventId` is `@default(0)`.
Emulator: the response contains `activityId` with the value `''` (the check read activityId `''`, startedEventId 0), i.e. the default-valued member is emitted instead of left out.
Why: a client that tests for the presence of `activityId` (or passes it to length-validated code) treats the idle poll as a real task, or fails validation on a zero-length id AWS never sends.
Fix: the PollForActivityTask handler/serializer (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/poll_for_activity_task.rs) must build the empty-task response with `activityId` (and the other work-describing members) absent rather than default-initialized.
Traces: swf.poll-for-activity-task-unregistered-domain-returns-empty-task.

## Evidence
Cause 1 (all five checks, same shape):
- swf.list-domains-corrupt-page-token-rejected — AssertionError: ListDomains with a token it never issued: expected ('ValidationException', 'Invalid token'), got ('ValidationException', 'nextPageToken "forged-continuation-marker" was not issued for a reverseOrder=false listing of this account\'s domains; pass the token back unchanged, with every other argument unchanged').
- swf.list-domains-page-size-and-invalid-token-rejected — expected ('ValidationException', 'Invalid token'), got ('ValidationException', 'nextPageToken "sim2real-behav-not-a-genuine-token" was not issued for a reverseOrder=false listing of this account\'s domains; ...').
- swf.list-activity-types-page-bounds-and-invalid-token-rejected — want ('ValidationException', 'Invalid token'), got ('ValidationException', 'nextPageToken "not-a-real-token" was not issued for a reverseOrder=false listing of this domain\'s registered types; ...').
- swf.list-open-workflow-executions-unissued-page-token-rejected — got ('ValidationException', 'nextPageToken "b3Blbi1saXN0aW5nLWZvcmdlZC10b2tlbg" was not issued for a reverseOrder=false listing of this domain\'s workflow executions ordered by start time; ...'), expected ('ValidationException', 'Invalid token').
- swf.get-workflow-execution-history-unissued-page-token-rejected — got ('ValidationException', 'nextPageToken "c2ltMnJlYWwtbm90LWlzc3VlZA==" was not issued for a reverseOrder=false read of this execution\'s history; pass the token back with the same reverseOrder'), expected ('ValidationException', 'Invalid token').
(The oversized-maximumPageSize refusal in the same two checks already matches AWS and is not part of this cause.)

Cause 2:
- swf.start-workflow-execution-without-task-list-default-undefined-rejected — AssertionError: refused start answered ('DefaultUndefinedFault', 'taskList was not set on the request and the workflow type defines no defaultTaskList'); expected ('DefaultUndefinedFault', 'taskList').

Cause 3:
- swf.list-open-executions-unknown-domain-answers-empty — botocore.errorfactory.UnknownResourceFault: An error occurred (UnknownResourceFault) when calling the ListOpenWorkflowExecutions operation: Unknown domain: sim2real-swf-never-registered-domain. Expected: HTTP 200, executionInfos == [], no nextPageToken (while DescribeDomain on that name is still refused with UnknownResourceFault / 'Unknown domain: <name>').

Cause 4:
- swf.poll-for-activity-task-unregistered-domain-returns-empty-task — AssertionError: the unregistered domain's poll returned activity '' startedEventId 0, expected an empty task (activityId absent, startedEventId 0, no taskToken).

## Steps to Reproduce
1. Cause 1: call ListDomains(registrationStatus=REGISTERED, nextPageToken='forged-continuation-marker'); AWS answers 400 ValidationException 'Invalid token', the emulator answers 400 ValidationException with the long 'nextPageToken "..." was not issued ...' sentence. Repeat with ListActivityTypes(nextPageToken='not-a-real-token'), ListOpenWorkflowExecutions(startTimeFilter, nextPageToken=<forged>) and GetWorkflowExecutionHistory(execution, nextPageToken=<forged>) — same divergence, different sentence each time.
2. Cause 2: RegisterWorkflowType with NO defaultTaskList (defaultExecutionStartToCloseTimeout/defaultTaskStartToCloseTimeout set), then StartWorkflowExecution for that type with no taskList: AWS 400 DefaultUndefinedFault message 'taskList'; the emulator's message is the full sentence.
3. Cause 3: ListOpenWorkflowExecutions(domain='sim2real-swf-never-registered-domain', startTimeFilter={oldestDate:0}, executionFilter={workflowId:<any>}): AWS 200 with executionInfos [] and no nextPageToken; the emulator raises UnknownResourceFault. DescribeDomain on the same name must still be refused with UnknownResourceFault.
4. Cause 4: PollForActivityTask(domain=<never-registered name>, taskList={name:<any>}) (client read timeout > 60 s): AWS 200 with startedEventId 0, no taskToken and no activityId member; the emulator's body carries activityId ''.
