Issue-Source: trace-sweep-divergence
Issue-Trace: glue-job-runs-cursor-from-another-listing-refused
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-job-runs-cursor-from-another-listing-refused glue-connection-type-registration-lifecycle glue-stop-workflow-run-refusals glue-workflow-without-a-starting-trigger-refuses-run-start-and-stop
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetJobRuns|4xx->2xx|$.Message
Issue-Key: glue|GetJobRuns|4xx->2xx|$.__type
Issue-Key: glue|GetJobRuns|4xx->2xx|status
Issue-Key: glue|DescribeConnectionType|4xx->2xx|$.Message
Issue-Key: glue|DescribeConnectionType|4xx->2xx|$.__type
Issue-Key: glue|DescribeConnectionType|4xx->2xx|status
Issue-Key: glue|GetWorkflowRuns|2xx->4xx|$.Runs
Issue-Key: glue|GetWorkflowRuns|2xx->4xx|status
Issue-Commit: bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5

# Glue lifecycle and pagination state checks diverge: 200 where AWS refuses, 400 where AWS succeeds

> _Note: this issue was identified against the code base at commit `bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRuns

## Description
Three net-new root causes in glue where the emulator takes the wrong branch outright: two accept a request AWS refuses (HTTP 200 instead of 400) and one refuses a request AWS answers (400 instead of 200).

Cause 1 of 3: GetJobRuns accepts a pagination token that does not belong to its listing.
AWS: GetJobRuns is `@paginated` on NextToken (Smithy: inputToken/outputToken NextToken, items JobRuns) and the token is scoped to the listing that produced it. A NextToken minted by a different listing (another job's GetJobRuns, or another operation) is refused with HTTP 400, `__type` `InvalidInputException` and message `Invalid pagination token.` — fixed text, no request values (Smithy: GetJobRuns models InvalidInputException).
Emulator: HTTP 200 with a normal page body and no error at all, so the foreign cursor is honoured.
Why: a client that crosses cursors between listings (a retry loop reusing a stale token, a harness threading tokens) gets a plausible-looking page of the wrong listing instead of the refusal that tells it the token is invalid — results are silently wrong rather than loudly rejected, and the client's InvalidInputException path is never exercised.
Fix: pagination-token validation in the GetJobRuns handler of the glue service crate — the token must be bound to (and verified against) the listing key it was minted for, and a foreign/corrupt token must raise InvalidInputException.
Traces: glue-job-runs-cursor-from-another-listing-refused#task#8 (and step task#11 in the same fixture).

Cause 2 of 3: DescribeConnectionType still describes a connection type after it has been deregistered.
AWS: DescribeConnectionType.ConnectionType is `@required`; a type that is not registered (including one deregistered after RegisterConnectionType) is refused HTTP 400 with `__type` `InvalidInputException` and message `The connection type does not exist.` — fixed text, no request values (Smithy: DescribeConnectionType models AccessDeniedException, InternalServiceException, InvalidInputException, ValidationException).
Emulator: HTTP 200 with a connection-type document, i.e. the deregistration left the type readable.
Why: the registration lifecycle is unobservable — a client cannot confirm a deregistration took effect, and code that probes DescribeConnectionType to decide whether to register concludes the type is still available.
Fix: the connection-type registry state behind the Register/Deregister/DescribeConnectionType handlers of the glue service crate (the describe path must consult live registration state and refuse an absent type).
Traces: glue-connection-type-registration-lifecycle#task#5.

Cause 3 of 3: GetWorkflowRuns refuses a workflow that simply has no runs yet.
AWS: GetWorkflowRuns.Name is `@required`; for an existing workflow that has not run, the call answers HTTP 200 with an empty `Runs` list and no NextToken (Smithy: GetWorkflowRuns output Runs -> WorkflowRuns; the empty-list-on-200 behavior for a run-less workflow is recorded behavior, and EntityNotFoundException is reserved for a workflow that does not exist).
Emulator: HTTP 400 with no Runs member — an absent-runs condition reported as an error.
Why: this inverts the control flow of every client that lists a workflow's history: the normal 'nothing has run yet' case arrives as an exception, so stop/poll/inspect flows abort where AWS returns an empty page. It also makes 'workflow missing' and 'workflow idle' indistinguishable.
Fix: the empty-result branch of the GetWorkflowRuns handler of the glue service crate (return 200 with Runs: [] when the workflow exists and has no runs; refuse only when the workflow itself is absent).
Traces: glue-stop-workflow-run-refusals#task#6 (and glue-workflow-without-a-starting-trigger-refuses-run-start-and-stop#task#5).

Related but not covering: 20261117T133709Z-glue-accepts-requests-aws-refuses-and-answers-200.md and 20261102T132146Z-glue-answers-200-for-absent-entities-and-an.md are the same defect class on GetPlan/GetMapping/GetDatabase/GetConnection and own none of the steps above; 20261115T110711Z-glue-5-net-new-causes-gettables-pagination-tokens.md owns GetTables' token handling only, not GetJobRuns'.

## Evidence
Cause 1 — fixture glue-job-runs-cursor-from-another-listing-refused step task#8 (GetJobRuns with a token from another listing), expected HTTP 400 / actual HTTP 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidInputException', got None)
  $.Message: missing (expected 'Invalid pagination token.', got None)
  Same cluster at step task#11.

Cause 2 — fixture glue-connection-type-registration-lifecycle step task#5 (DescribeConnectionType after deregistration), expected 400 / actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidInputException', got None)
  $.Message: missing (expected 'The connection type does not exist.', got None)

Cause 3 — fixture glue-stop-workflow-run-refusals step task#6 (GetWorkflowRuns on a workflow with no runs), expected 200 / actual 400:
  status: expected 200, got 400
  $.Runs: missing (expected [], got None)
  Same cluster at glue-workflow-without-a-starting-trigger-refuses-run-start-and-stop#task#5.

## Steps to Reproduce
Cause 1: CreateJob A and CreateJob B, StartJobRun on both, GetJobRuns(A, MaxResults=1) to mint a token, then GetJobRuns(B, NextToken=<A's token>) — expect 400 InvalidInputException 'Invalid pagination token.', observe 200 with a page.
Cause 2: RegisterConnectionType a REST- type, DeleteConnectionType/deregister it, then DescribeConnectionType on it — expect 400 InvalidInputException 'The connection type does not exist.', observe 200 with a document.
Cause 3: CreateWorkflow and do not start it, then GetWorkflowRuns(Name) — expect 200 with Runs: [], observe 400.
