Issue-Source: trace-sweep-divergence
Issue-Trace: batch-scheduling-policy-standalone-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-scheduling-policy-standalone-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeSchedulingPolicies|4xx->4xx|$.message
Issue-Commit: f854ab471aec7cf544889eecd20c7c69231858a1

# batch DescribeSchedulingPolicies malformed-ARN refusal message still diverges from the recorded ClientException text

> _Note: this issue was identified against the code base at commit `f854ab471aec7cf544889eecd20c7c69231858a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchedulingPolicies

## Description
One net-new root cause on the AWS Batch scheduling-policy read plane. It is the ONLY divergence cluster in this replay whose owned_by is EMPTY: every other cluster in the report is owned by an open issue that already carries its replay key (20261102T113649Z-batch-job-api-refusal-contracts-diverge-two-wrong.md, 20261102T113649Z-batch-describejobdefinitions-omits-empty-collections-and-applied-defaults.md, 20261029T234420Z-batch-refusal-contracts-diverge-not-found-arn-wording.md, 20261109T161717Z-batch-3-net-new-causes-tag-untag-204.md, 20261112T140115Z-batch-a-fargate-job-never-reaches-succeeded-and.md, 20261112T140115Z-logs-deletelogstream-refuses-the-batch-job-log-stream.md) and is not re-filed here. Two issues on THIS step exist but are both RESOLVED (20261030T031148Z-batch-describeschedulingpolicies-malformed-arn-refusal-message-is-emulator.md and 20261030T204009Z-batch-describeschedulingpolicies-malformed-arn-refusal-is-not-wrapped.md, both listed [fixed]); a resolved issue does not cover a failure that still replays, and neither carries this step's replay key any more, so this is filed net-new. The earlier fix clearly landed in part - the envelope those issues asked for is now present in the emulator's message - but the message as a whole still fails the recording's assertion, so the refusal contract for this operation is still not the recorded one.

Cause 1 of 1: DescribeSchedulingPolicies' refusal for a malformed scheduling-policy ARN carries a message whose text is not the one AWS sends for that refusal class.

AWS: Smithy (services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json) models DescribeSchedulingPolicies as POST /v1/describeschedulingpolicies, success code 200, input member `arns` -> StringList (@required, clientOptional), output `schedulingPolicies` -> SchedulingPolicyDetailList, and exactly two errors: ClientException (httpError 400) and ServerException (500). The model states no message text, so the wording here is recorded behavior only. The recording refuses an `arns` entry that is not a well-formed scheduling-policy ARN with HTTP 400 / ClientException, and its message is asserted by SHAPE rather than literally (the replay reports expected '<shaped-not-asserted>') because the recorded text terminates in a per-call request id. The recorded shape is Batch's input-validation envelope - fixed text "Error executing request, Exception : ", then the validator's own sentence, then fixed text ", RequestId: ", then the call's request id - whose variable parts are the inner sentence's interpolated value (taken from the request) and the request id (taken from the call). The replay still reports this actual message as a value_mismatch against that shape even though the envelope's fixed text is now present, so the part that still diverges is the inner validator sentence, not the envelope and not the per-run request id (a per-run request-id difference alone would satisfy the shape assertion and is not what is filed here).
Hypothesis: AWS's inner sentence for a malformed ARN in `arns` does not echo the rejected string the way the emulator's does - the recorded refusal names the member/constraint (e.g. Batch's standard "1 validation error detected: Value '<rejected value>' at 'arns' failed to satisfy constraint: ..." form, or the arn-pattern sentence without the trailing ": <value>" interpolation) - which is why the emulator's sentence, wrapped correctly, still fails the shape. Only the recorded shape mismatch is asserted; this explanation is labelled Hypothesis because neither the model nor the replay detail states the recorded sentence.

Emulator: HTTP 400 with x-amzn-errortype ClientException (status and error class both correct) and `$.message` = "Error executing request, Exception : Scheduling policy arn should match a valid Scheduling policy arn pattern: not-a-valid-arn, RequestId: <uuid>". In that string the fixed text is "Error executing request, Exception : " + "Scheduling policy arn should match a valid Scheduling policy arn pattern: " + ", RequestId: "; the variable parts are the rejected `arns` entry taken from the request (the literal 'not-a-valid-arn' is this recording's value filling that slot, not a requirement) and the per-call request id (also not the defect).

Why: Batch answers every client mistake with the single ClientException shape, so the message text is the only machine-readable detail a caller gets. A client that distinguishes "the ARN I passed is syntactically wrong" from "the scheduling policy does not exist" or from an arity/quota refusal by matching AWS's sentence gets no match against the emulator, so a correctable input mistake falls into a generic error path; conversely a suite written against the emulator's sentence passes here and fails against AWS. Because the sibling validators on this same operation render through the same envelope, a caller cannot handle all of this operation's refusals with one matcher the way AWS's uniform text allows.

Fix: the inner refusal text produced by DescribeSchedulingPolicies' input validation in the batch crate - the arn-pattern branch of that operation's validation layer (the envelope/renderer it now goes through is correct and must be kept; only the sentence it wraps has to become the recorded one). Not the status and not the error class, which already match.

Traces: batch-scheduling-policy-standalone-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true (/__admin/health answered 200 and the data listener accepts), bootstrap_signal false, 19 fixtures replayed, 165 steps total, 141 passed, 24 mismatched, 0 errored, 0 blocked, score 85.5%, 16 divergence clusters.

This cluster: divergence_key "service=batch|DescribeSchedulingPolicies|status=400->400|keys=$.message", fixture_id batch-scheduling-policy-standalone-lifecycle, step_key task#9, operation DescribeSchedulingPolicies, verdict mismatch, affected_count 1, stub_path EMPTY, owned_by EMPTY (no open issue carries this replay key).

expected_status 400, actual_status 400 - HTTP status and error class (ClientException) agree.
detail, verbatim:
  $.message: value_mismatch (expected '<shaped-not-asserted>', got 'Error executing request, Exception : Scheduling policy arn should match a valid Scheduling policy arn pattern: not-a-valid-arn, RequestId: e6b7a156-1ccc-4dd0-a614-211a256d0d84')

Reading of that detail: the expected side is a shape assertion (not a literal) because the recorded AWS message ends in a per-call request id. The actual message now DOES carry the envelope's fixed text ("Error executing request, Exception : " ... ", RequestId: ") that the two resolved issues on this step asked for, and it still fails the shape assertion - so the residual divergence is in the sentence the envelope wraps. The uuid e6b7a156-1ccc-4dd0-a614-211a256d0d84 is this run's request id and 'not-a-valid-arn' is this fixture's `arns` entry; neither is part of the requirement, and a difference confined to them would not be filed.

Smithy evidence (services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json): DescribeSchedulingPolicies - http POST /v1/describeschedulingpolicies, code 200; input {arns: StringList, required}; output {schedulingPolicies: SchedulingPolicyDetailList}; errors ClientException (errorKind client, httpError 400) and ServerException (500). No message text is modeled, hence "recorded behavior only" for the wording.

Prior-art status: list_existing_issues(status="all", query="describeschedulingpolicies") returns exactly two issues, both [fixed]: 20261030T031148Z-batch-describeschedulingpolicies-malformed-arn-refusal-message-is-emulator.md and 20261030T204009Z-batch-describeschedulingpolicies-malformed-arn-refusal-is-not-wrapped.md. No open issue mentions this operation.

## Steps to Reproduce
1. Clone this checkout, boot the emulator from it and replay the wire corpus simparity-wire-aws-v1.
2. Fixture batch-scheduling-policy-standalone-lifecycle, step task#9: POST /v1/describeschedulingpolicies with `arns` containing one entry that is not a well-formed scheduling-policy ARN (the recorded probe uses the literal "not-a-valid-arn"). By hand: aws batch describe-scheduling-policies --arns not-a-valid-arn
3. Observed: HTTP 400, x-amzn-errortype ClientException, $.message = "Error executing request, Exception : Scheduling policy arn should match a valid Scheduling policy arn pattern: not-a-valid-arn, RequestId: <uuid>" - which the replay reports as value_mismatch against the recording's shaped message assertion.
4. Expected: HTTP 400 / ClientException whose message is the recorded AWS text for a malformed `arns` entry, rendered through Batch's input-validation envelope ("Error executing request, Exception : " + <AWS's sentence> + ", RequestId: " + this call's request id). The envelope is already correct; the wrapped sentence is not.
5. Cross-check with the sibling validators on the same operation (arns count out of range, and ListQuotaShares maxResults) so that every refusal this operation emits matches on one client-side matcher.
6. Re-replay to confirm: batch-scheduling-policy-standalone-lifecycle#task#9.
