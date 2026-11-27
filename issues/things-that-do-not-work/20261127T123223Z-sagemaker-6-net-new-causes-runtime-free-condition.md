Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.pipeline-definition-for-unknown-execution-rejected sagemaker.pipeline-execution-keeps-definition-it-started-with-after-update sagemaker.pipeline-executions-list-newest-execution-in-creation-order sagemaker.pipeline-parameters-for-execution-page-every-parameter-once sagemaker.retry-pipeline-execution-rejects-succeeded-execution sagemaker.stop-finished-pipeline-execution-rejected sagemaker.list-pipeline-executions-unissued-next-token-rejected sagemaker.attach-cluster-node-volume-cluster-not-found sagemaker.attach-cluster-node-volume-missing-node-not-found sagemaker.search-creation-time-filter-malformed-value-rejected sagemaker.search-trial-component-parameter-range-bounds sagemaker.search-unissued-next-token-rejected sagemaker.send-pipeline-execution-step-failure-spent-callback-token-rejected sagemaker.send-pipeline-execution-step-success-spent-callback-token-rejected
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.pipeline-definition-for-unknown-execution-rejected|_poll#17|AssertionError|raise AssertionError(f"{what} not reached within {budget_s:.0f}s; last read {result!r}")
Issue-Key: sagemaker.pipeline-execution-keeps-definition-it-started-with-after-update|run#9|AssertionError|assert early["PipelineExecutionStatus"] == "Succeeded", (
Issue-Key: sagemaker.pipeline-executions-list-newest-execution-in-creation-order|run#12|AssertionError|assert status == "Succeeded", f"the first execution ended {status!r}, expected 'Succeeded'"
Issue-Key: sagemaker.pipeline-parameters-for-execution-page-every-parameter-once|run#6|AssertionError|assert status == "Succeeded", f"a branchless Condition step must succeed; got {status!r}"
Issue-Key: sagemaker.retry-pipeline-execution-rejects-succeeded-execution|run#11|AssertionError|assert ok_run["PipelineExecutionStatus"] == "Succeeded", (
Issue-Key: sagemaker.stop-finished-pipeline-execution-rejected|_wait#12|AssertionError|raise AssertionError(f"{what} not reached within {budget_s:.0f}s; last read {result!r}")
Issue-Key: sagemaker.list-pipeline-executions-unissued-next-token-rejected|run#11|AssertionError|assert _finish(sm, arn, _budget(ctx, 240.0)) == "Succeeded", (
Issue-Key: sagemaker.attach-cluster-node-volume-cluster-not-found|run#12|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: sagemaker.attach-cluster-node-volume-missing-node-not-found|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.search-creation-time-filter-malformed-value-rejected|_refusal#16|AssertionError|raise AssertionError(f"expected the search to be refused, but it returned {result!r}")
Issue-Key: sagemaker.search-trial-component-parameter-range-bounds|run#17|AssertionError|assert got == expected, f"rate {op} 2.5 matched {sorted(got)!r}, expected {sorted(expected)!r}"
Issue-Key: sagemaker.search-unissued-next-token-rejected|run#15|AssertionError|assert refused == ("ValidationException", "Invalid or Expired NextToken"), (
Issue-Key: sagemaker.send-pipeline-execution-step-failure-spent-callback-token-rejected|_receive_token#13|Timeout|got = sqs.receive_message(QueueUrl=queue_url, WaitTimeSeconds=10, MaxNumberOfMessages=1)
Issue-Key: sagemaker.send-pipeline-execution-step-success-spent-callback-token-rejected|_receive_token#13|Timeout|got = sqs.receive_message(QueueUrl=queue_url, WaitTimeSeconds=10, MaxNumberOfMessages=1)
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# sagemaker: 6 net-new causes — runtime-free Condition-step executions end Failed, ClusterNotExists refusals drop their exception prefix, Search ignores malformed CreationTime values and numeric parameter ranges, the Search NextToken refusal is emulator-worded, and the Callback SQS message omits pipelineExecutionArn

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution, DescribePipelineExecution, AttachClusterNodeVolume, DescribeCluster, Search, SendPipelineExecutionStepFailure

## Description
Six independent root causes in the emulated SageMaker control plane, all grounded in failing behavioral checks the suite report lists under `### sagemaker`. They are numbered; each must be closed before this issue is submitted.

=== Cause 1 of 6: a pipeline execution whose steps need no job runtime (Condition steps) still ends Failed with the emulator's "no step of it ran" reason instead of Succeeded ===
AWS: Smithy — `PipelineExecutionStatus` enum {Executing, Stopping, Stopped, Failed, Succeeded, CompletedWithFailures}; `StepStatus` enum {Starting, Executing, Stopping, Stopped, Failed, Succeeded}. A `Condition` step is evaluated by the Pipelines service itself: it runs no training/processing/transform/Lambda work, it evaluates its `Conditions` against the execution's resolved parameters and then schedules the steps in its `IfSteps` or `ElseSteps`. Recorded behavior: an execution of a definition whose only steps are Condition steps — including a Condition whose `IfSteps`/`ElseSteps` are empty, and Condition steps nested inside one another's `IfSteps` — reaches `PipelineExecutionStatus` "Succeeded" with no `FailureReason` member, and each Condition step is listed by ListPipelineExecutionSteps with `StepStatus` "Succeeded".
Emulator: every such execution ends `Failed` and carries the emulator-authored FailureReason "The pipeline execution was accepted but no step of it ran: this emulator has no runtime that performs this step's work (training, processing, transform or Lambda), so the execution ended Failed rather than reporting work that did not happen." The branchless/empty-branch Condition case is the clearest proof the gate is wrong: there is no step whose work needs a runtime at all, yet the execution is failed for lacking one.
Why: the whole Pipelines surface becomes unobservable for clients that use Condition steps for control flow. A client cannot distinguish "my condition evaluated false" from "my pipeline broke": retries, stop semantics, version pinning and every listing/pagination flow that needs one Succeeded execution to exist can never be exercised, and a FailureReason no AWS response ever contains is surfaced to callers' error handling.
Fix: the pipeline-execution runner/state that decides an execution's terminal status — it must evaluate Condition steps itself (resolving `Parameters.*` references and the declared `Conditions`), schedule the selected branch, mark the Condition step Succeeded, and only fall back to a failure when a step genuinely requires an unimplemented job runtime. Related but distinct open issue: 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md covers processing/training/HPO/AutoML steps that need a runtime; it does not state that a Condition step needs none, which is this cause.
Traces: sagemaker.pipeline-definition-for-unknown-execution-rejected, sagemaker.pipeline-execution-keeps-definition-it-started-with-after-update, sagemaker.pipeline-executions-list-newest-execution-in-creation-order, sagemaker.pipeline-parameters-for-execution-page-every-parameter-once, sagemaker.retry-pipeline-execution-rejects-succeeded-execution, sagemaker.stop-finished-pipeline-execution-rejected, sagemaker.list-pipeline-executions-unissued-next-token-rejected.

=== Cause 2 of 6: the ResourceNotFound raised for an absent HyperPod cluster drops the "ClusterNotExistsException: " prefix AWS puts in the message ===
AWS: Smithy — AttachClusterNodeVolume models exactly one error, `ResourceNotFound` (client, HTTP 400, "Resource being access is not found"), whose only member is `Message`; DescribeCluster raises the same shape. Recorded behavior: for a cluster ARN that resolves to no cluster, both operations answer HTTP 400 with Error.Code `ResourceNotFound` and Error.Message "ClusterNotExistsException: Specified cluster not found". The whole message is fixed text for this error class — the internal exception name, a colon and a space, then the sentence; no part of it comes from the request.
Emulator: the code is right but the message is "Specified cluster not found" — the "ClusterNotExistsException: " prefix is missing, on AttachClusterNodeVolume and on DescribeCluster alike.
Why: SageMaker reuses one wire error code (`ResourceNotFound`) for many subjects and discriminates them only by this message prefix, so a client that keys on the message to tell "no such cluster" from "no such node"/"no such endpoint" matches nothing and falls through to generic handling.
Fix: the message construction for the cluster-not-found refusal in the AttachClusterNodeVolume and DescribeCluster handlers (or the shared cluster-lookup helper that builds the ResourceNotFound).
Traces: sagemaker.attach-cluster-node-volume-cluster-not-found, sagemaker.attach-cluster-node-volume-missing-node-not-found.

=== Cause 3 of 6: a Search CreationTime filter whose Value is not a date-time is accepted and answered with an empty result set instead of being refused ===
AWS: Smithy — `Filter.Value` is documented "For timestamp properties, Value must be an ISO 8601 date-time string of the following format: YYYY-mm-dd'T'HH:MM:SS". Recorded behavior: `Search(Resource="Experiment", SearchExpression={Filters:[{Name:"CreationTime", Operator:"GreaterThan", Value:"last-tuesday"}, ...]})` is refused with HTTP 400, Error.Code `ValidationException`, Error.Message "CreationTime cannot be parsed into a Date value." — the message is fixed text except the property name, which is the filter's own `Name`. A well-formed CreationTime filter on the same expression still answers normally afterwards.
Emulator: the call succeeds with HTTP 200 and `Results: []`, so an unparseable bound silently becomes "nothing matches".
Why: a client that mis-formats a timestamp bound gets a confident empty answer instead of an error, so a search-driven decision ("no experiments in this window, so none exist") is made on a filter the service never applied — a silent-wrong-answer class, strictly worse than a refusal.
Fix: the Search handler's filter validation — a timestamp-typed property's `Value` must be parsed before the index is consulted and a parse failure must raise ValidationException with the message above, leaving the rest of the expression unevaluated.
Traces: sagemaker.search-creation-time-filter-malformed-value-rejected.

=== Cause 4 of 6: Search never matches a numeric range comparison on a trial component's Parameters.<name> ===
AWS: Smithy — `Operator` enum {Equals, NotEquals, GreaterThan, GreaterThanOrEqualTo, LessThan, LessThanOrEqualTo, Contains, Exists, NotExists, In}; `Filter.Value` is documented "For numerical properties, Value must be an integer or floating-point decimal", and GreaterThan/LessThan and their OrEqualTo forms are "Not supported for text properties" — i.e. they are numeric comparisons. Recorded behavior: with trial components carrying `Parameters={"rate":{"NumberValue":1.5|2.5|3.5}}` plus one component with no `rate`, a filter {Name:"Parameters.rate", Operator:<op>, Value:"2.5"} matches exactly the components whose numeric value satisfies the comparison: GreaterThan -> the 3.5 component, GreaterThanOrEqualTo -> 2.5 and 3.5, LessThan -> 1.5, LessThanOrEqualTo -> 1.5 and 2.5, Equals -> 2.5; the component without the parameter never matches any of them.
Emulator: `Operator: "Exists"` on the same property works (it returns exactly the three rated components), but every value comparison returns nothing: GreaterThan 2.5 matched [] where it must match the 3.5 component. DescribeTrialComponent reports `{"rate": {"NumberValue": 2.5}}` correctly, so the value is stored — only the comparison is missing.
Why: hyperparameter/metric search is the point of Search on trial components. A client that selects the best run by a numeric bound gets an empty page and concludes no run qualifies, so the comparison silently answers "no match" instead of "not supported".
Fix: the Search handler's filter evaluation for dotted `Parameters.<name>` (and the same path for Metrics/HyperParameters) — it must read the parameter's NumberValue, coerce the filter's `Value` to a number, and apply the relational operator, with inclusive/exclusive boundaries as the OrEqualTo forms require.
Traces: sagemaker.search-trial-component-parameter-range-bounds.

=== Cause 5 of 6: Search refuses a NextToken it never issued with emulator-authored wording instead of AWS's ===
AWS: Smithy — Search is `paginated` with inputToken/outputToken `NextToken` (length 0..8192, pattern .*). Recorded behavior: a well-formed token string that Search never issued is refused with HTTP 400, Error.Code `ValidationException`, Error.Message "Invalid or Expired NextToken" — the whole message is fixed text for this error class, with no value taken from the request.
Emulator: the code is right but the message is "NextToken does not name a page boundary of this result set; restart the listing without it."
Why: a client that branches on the refusal text (to restart a listing rather than abort) never matches, and the emulator-authored sentence leaks into logs and user-facing errors where AWS's wording is expected.
Fix: the Search handler's NextToken validation message (the pagination-token decoder's rejection text).
Traces: sagemaker.search-unissued-next-token-rejected.

=== Cause 6 of 6: the SQS message a Callback step posts carries no pipelineExecutionArn, so a consumer cannot tell which execution it must answer ===
AWS: recorded behavior only (the message body is not in the Smithy model; Smithy models only `CallbackStepMetadata` {CallbackToken length 10 pattern ^[a-zA-Z0-9]+$, SqsQueueUrl, OutputParameters} and the 10-character `CallbackToken` of SendPipelineExecutionStepSuccess/Failure). The JSON body SageMaker Pipelines sends to the step's `SqsQueueUrl` carries lower-camel-case keys including `token` (the 10-character callback token) and `pipelineExecutionArn` (the ARN of the execution the step belongs to), alongside the step's resolved `arguments`.
Emulator: the body has no `pipelineExecutionArn` member at all (`body.get("pipelineExecutionArn")` is None; the observed body is `{'token': <10 chars>, 'arguments': {...}}`, plus a `Status` member on a stop). A consumer that must correlate a queued token with the execution it started — the normal pattern, since one queue serves many executions — can never match, and both spent-token checks burn their whole budget in the receive loop and are killed at 60 s without ever reaching the refusal they test.
Why: the Callback step is unusable for its purpose: a worker draining a shared queue has no way to know which pipeline execution a token belongs to, so it cannot look up the execution, cannot scope its work, and cannot ignore messages for other executions.
Fix: the Callback-step dispatcher that serializes and enqueues the callback message body (the pipeline-execution runner's SQS publish), which must include `pipelineExecutionArn` for the execution that owns the step. The same message body has two further defects filed separately under the `sagemaker__sqs` heading (unresolved `arguments`, and the stop message's `Status` vs `status` key); fixing all of them in the one serializer is expected.
Traces: sagemaker.send-pipeline-execution-step-failure-spent-callback-token-rejected, sagemaker.send-pipeline-execution-step-success-spent-callback-token-rejected (both "Check timed out after 60.0s" inside the receive loop that filters on pipelineExecutionArn).

## Evidence
Replay output, verbatim from the behavioral suite report (all under `### sagemaker`):

Cause 1 — [mismatch] sagemaker.pipeline-execution-keeps-definition-it-started-with-after-update:
  AssertionError: a condition with two empty branches must succeed; got 'Failed'
[mismatch] sagemaker.pipeline-parameters-for-execution-page-every-parameter-once:
  AssertionError: a branchless Condition step must succeed; got 'Failed'
[mismatch] sagemaker.list-pipeline-executions-unissued-next-token-rejected:
  AssertionError: a branchless Condition step execution did not succeed
[mismatch] sagemaker.pipeline-executions-list-newest-execution-in-creation-order:
  AssertionError: the first execution ended 'Failed', expected 'Succeeded'
[mismatch] sagemaker.retry-pipeline-execution-rejects-succeeded-execution:
  AssertionError: with Mode='go' the condition takes its empty branch, so the execution must succeed; got 'Failed', FailureReason="The pipeline execution was accepted but no step of it ran: this emulator has no runtime that performs this step's work (training, processing, transform or Lambda), so the execution ended Failed rather than reporting work that did not happen."
[mismatch] sagemaker.stop-finished-pipeline-execution-rejected:
  AssertionError: PipelineExecutionStatus Succeeded not reached within 39s; last read 'Failed'
[mismatch] sagemaker.pipeline-definition-for-unknown-execution-rejected:
  AssertionError: PipelineExecutionStatus Succeeded not reached within 40s; last read {... 'PipelineExecutionStatus': 'Failed', 'FailureReason': "The pipeline execution was accepted but no step of it ran: this emulator has no runtime that performs this step's work (training, processing, transform or Lambda), so the execution ended Failed rather than reporting work that did not happen." ...}
Expected in every one of these: PipelineExecutionStatus 'Succeeded' and no FailureReason. (The ARNs, execution ids and timestamps in the dump are per-run values, not requirements.)

Cause 2 — [mismatch] sagemaker.attach-cluster-node-volume-cluster-not-found and [mismatch] sagemaker.attach-cluster-node-volume-missing-node-not-found:
  AssertionError: expected message 'ClusterNotExistsException: Specified cluster not found', got 'Specified cluster not found'
Error.Code was 'ResourceNotFound' (correct) on both AttachClusterNodeVolume and the independent DescribeCluster read. Fixed text: the entire string "ClusterNotExistsException: Specified cluster not found".

Cause 3 — [mismatch] sagemaker.search-creation-time-filter-malformed-value-rejected:
  AssertionError: expected the search to be refused, but it returned set()
  (the refused call: Search(Resource='Experiment', SearchExpression={'Filters':[{'Name':'Tags.probe-window','Operator':'Equals','Value':<mark>},{'Name':'CreationTime','Operator':'GreaterThan','Value':'last-tuesday'}]}))
  Expected: 400 ValidationException, "CreationTime cannot be parsed into a Date value."

Cause 4 — [mismatch] sagemaker.search-trial-component-parameter-range-bounds:
  AssertionError: rate GreaterThan 2.5 matched [], expected ['sim2real-behav-rate-afb0fd90c06f']
(the name is a per-run generated name; the requirement is that the component whose Parameters.rate NumberValue is 3.5 — and only it — matches GreaterThan "2.5". The preceding Operator:'Exists' assertion on the same property passed, and DescribeTrialComponent reported {'rate': {'NumberValue': 2.5}}.)

Cause 5 — [mismatch] sagemaker.search-unissued-next-token-rejected:
  AssertionError: a NextToken Search never issued must be refused; got ('ValidationException', 'NextToken does not name a page boundary of this result set; restart the listing without it.')
  Expected: ('ValidationException', 'Invalid or Expired NextToken')

Cause 6 — [mismatch] sagemaker.send-pipeline-execution-step-failure-spent-callback-token-rejected and [mismatch] sagemaker.send-pipeline-execution-step-success-spent-callback-token-rejected:
  Error: Check timed out after 60.0s
The receive loop these two block in accepts a message only when `json.loads(msg['Body']).get('pipelineExecutionArn') == execution_arn`; the emulator's body has no such member (confirmed directly by the sibling check sagemaker__sqs.stop-with-running-callback-step-queues-a-stopping-message-and-waits-for-the-token: "the callback message names another execution: None"). Expected body members for a Callback step: lower-camel-case `token`, `arguments` and `pipelineExecutionArn`.

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it:

Cause 1: CreatePipeline with a definition whose only step needs no runtime — {"Version":"2020-12-01","Parameters":[],"Steps":[{"Name":"Gate","Type":"Condition","Arguments":{"Conditions":[{"Type":"Equals","LeftValue":"a","RightValue":"a"}],"IfSteps":[],"ElseSteps":[]}}]} — then StartPipelineExecution and poll DescribePipelineExecution. Observe PipelineExecutionStatus 'Failed' with the "no step of it ran" FailureReason instead of 'Succeeded'. (Replay: the seven sagemaker.pipeline-*/retry-/stop-/list-pipeline-executions- checks named above.)

Cause 2: AttachClusterNodeVolume(ClusterArn="arn:aws:sagemaker:<region>:<account>:cluster/<12 hex chars that name no cluster>", NodeId="i-<17 hex>", VolumeId=<a real gp3 volume>) and DescribeCluster(ClusterName=<that same ARN>). Observe Error.Code 'ResourceNotFound' with Error.Message 'Specified cluster not found' instead of 'ClusterNotExistsException: Specified cluster not found'.

Cause 3: CreateExperiment(ExperimentName=n, Tags=[{'Key':'probe-window','Value':m}]), then Search(Resource='Experiment', SearchExpression={'Filters':[{'Name':'Tags.probe-window','Operator':'Equals','Value':m},{'Name':'CreationTime','Operator':'GreaterThan','Value':'last-tuesday'}]}). Observe HTTP 200 with Results [] instead of 400 ValidationException 'CreationTime cannot be parsed into a Date value.'

Cause 4: CreateTrialComponent three times with Parameters={'rate':{'NumberValue':1.5}}, {'NumberValue':2.5}, {'NumberValue':3.5} and a common tag, plus a fourth with the tag and no Parameters. Then Search(Resource='ExperimentTrialComponent', SearchExpression={'Filters':[{'Name':'Tags.<key>','Operator':'Equals','Value':<batch>},{'Name':'Parameters.rate','Operator':'GreaterThan','Value':'2.5'}]}). Observe no Results; repeat with GreaterThanOrEqualTo / LessThan / LessThanOrEqualTo / Equals and observe the same, while Operator 'Exists' returns the three rated components.

Cause 5: Search(Resource='Experiment', SearchExpression={...}, NextToken='c2ltMnJlYWwtZm9yZ2VkLXRva2Vu'). Observe ValidationException with 'NextToken does not name a page boundary of this result set; restart the listing without it.' instead of 'Invalid or Expired NextToken'.

Cause 6: create an SQS queue and a role SageMaker can assume that may sqs:SendMessage to it; CreatePipeline with a single step {"Name":"AwaitSignoff","Type":"Callback","SqsQueueUrl":<queue url>,"Arguments":{"ticket":"signoff"}}; StartPipelineExecution, then ReceiveMessage from the queue and json.loads the Body. Observe the body has only 'token' and 'arguments' — no 'pipelineExecutionArn' — so a consumer cannot correlate the token with the execution returned by StartPipelineExecution.
