Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.build-workflow-token-reused-with-other-content-rejected bedrock.second-build-workflow-while-one-runs-rejected bedrock.stop-customization-job-unknown-arn-not-found bedrock.stop-unknown-batch-inference-job-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.build-workflow-token-reused-with-other-content-rejected|_refusal#9|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {answer!r}")
Issue-Key: bedrock.second-build-workflow-while-one-runs-rejected|run#9|AssertionError|assert message.startswith("Existing build workflow in progress for policy arn: arn:") and (
Issue-Key: bedrock.stop-customization-job-unknown-arn-not-found|run#11|AssertionError|assert read == ("ResourceNotFoundException", "Model-Customization job not found."), (
Issue-Key: bedrock.stop-unknown-batch-inference-job-rejected|run#12|AssertionError|assert stopped == want, f"stopping a job ARN that names no job gave {stopped!r}, expected {want!r}"
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# bedrock: 4 net-new causes — build-workflow clientRequestToken reuse is not refused, and the ResourceInUseException / ResourceNotFoundException messages for build workflows, model-customization jobs and batch inference jobs are emulator-authored

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomatedReasoningPolicyBuildWorkflow

## Description
Four independent root causes in the bedrock control plane, all grounded in failing behavioral checks. The open issue 20261029T172312Z-bedrock-control-plane-is-over-permissive-enforced-guardrail.md covers idempotency-token reuse for OTHER control-plane resources and does NOT own any of the checks below (the suite marks none of them OWNED); cause 1 is the same family of defect on a different operation. Number of causes to close: 4.

Cause 1 of 4: StartAutomatedReasoningPolicyBuildWorkflow accepts a clientRequestToken replayed with DIFFERENT content instead of refusing it with ConflictException.
AWS: Smithy (com.amazonaws.bedrock, StartAutomatedReasoningPolicyBuildWorkflow): clientRequestToken targets IdempotencyToken, is marked @idempotencyToken and is bound to the x-amz-client-token header; ConflictException is a modeled error of the operation with httpError 400. Recorded behavior: starting a build workflow with a token, then replaying the IDENTICAL request with that token, returns the SAME buildWorkflowId (this part of the check passed); replaying that token with a different sourceContent payload is refused 400 ConflictException whose message begins with the fixed text "A different request was previously submitted with the same ClientRequestToken" (the message has no values taken from the request). The refused start creates nothing: the policy still lists exactly the one workflow the token started.
Emulator: the call with the reused token and the altered document is ACCEPTED — it returns a 200 with a buildWorkflowId (in the run, 'e5d54483-acb9-4ecc-af6f-8bf7fbd8b226') and no error at all, so a second build workflow is recorded for the policy.
Why: a client that retries a build-workflow start after mutating the request (an SDK retry across a code path that rebuilt the payload, or a caller reusing a token key) gets a silent duplicate start instead of the ConflictException that tells it the token is already bound to a different request. Idempotency-token semantics are the contract clients rely on to make retries safe; with it absent, duplicate workflows consume the policy's single in-flight build slot and the caller never learns its token was divergent.
Fix: the StartAutomatedReasoningPolicyBuildWorkflow handler's idempotency bookkeeping — the stored token must be compared against the request it was first bound to (policyArn, buildWorkflowType and sourceContent) and a mismatch must raise ConflictException with AWS's fixed sentence, before any workflow is created in state.
Traces: bedrock.build-workflow-token-reused-with-other-content-rejected

Cause 2 of 4: the ResourceInUseException raised when a second build workflow is started while one is in progress carries an emulator-authored message that does not name the busy policy the way AWS does.
AWS: ResourceInUseException is a modeled error of StartAutomatedReasoningPolicyBuildWorkflow (httpError 400). Recorded behavior: while a policy's build workflow is in progress, a second start is refused 400 ResourceInUseException with the message "Existing build workflow in progress for policy arn: <policyArn>" — the fixed text is "Existing build workflow in progress for policy arn: " and the only variable part is the policyArn from the request. The error code and status the emulator returns are already correct; only the message diverges.
Emulator: the message is "build workflow <buildWorkflowId> of automated reasoning policy <policyArn> is SCHEDULED, so another build workflow cannot be started for it" — invented prose that additionally leaks the in-flight workflow id and its internal status word, and does not end with the policy ARN.
Why: callers and operators match on this sentence (log scraping, error-classification helpers, test assertions). A reworded message makes the refusal unrecognizable, and the exposed internal status token ('SCHEDULED') is not part of AWS's refusal vocabulary for this error.
Fix: the message construction in the StartAutomatedReasoningPolicyBuildWorkflow handler's in-progress pre-condition check (the busy-policy guard), not the state machine — the detection itself is right.
Traces: bedrock.second-build-workflow-while-one-runs-rejected

Cause 3 of 4: the model-customization-job not-found refusal message is emulator-authored instead of AWS's fixed sentence.
AWS: Smithy: GetModelCustomizationJob and StopModelCustomizationJob both take jobIdentifier (an ARN or a bare job name, per the ModelCustomizationJobIdentifier pattern) and both model ResourceNotFoundException with httpError 404. Recorded behavior: for a well-formed model-customization-job ARN of the caller's own account and Region that names no job, BOTH operations are refused 404 ResourceNotFoundException with the message "Model-Customization job not found." — entirely fixed text, with no part taken from the request. The same recording also requires that a bare unknown job NAME passed to StopModelCustomizationJob is refused ValidationException "The provided job identifier is invalid." (also entirely fixed text); the check aborted on the first assertion so that leg was not exercised, but it is the same message surface and belongs to this cause's fix.
Emulator: the Get is refused with ResourceNotFoundException (code correct) but the message is "no model customization job answers to <jobArn> in this account in <region>" — invented prose that interpolates the ARN and the Region, neither of which appears in AWS's message.
Why: AWS's not-found sentence for this resource is a constant; a client branching on it (or an operator grepping it) cannot recognize the emulator's wording, and the emulator's message implies the identifier/Region are part of the error contract when they are not.
Fix: the not-found error construction shared by the GetModelCustomizationJob and StopModelCustomizationJob handlers (and the invalid-identifier ValidationException on the same path).
Traces: bedrock.stop-customization-job-unknown-arn-not-found

Cause 4 of 4: the batch-inference-job (model-invocation-job) not-found refusal messages are emulator-authored, and are the same sentence for stop and read where AWS uses two different fixed sentences.
AWS: Smithy: StopModelInvocationJob and GetModelInvocationJob both take jobIdentifier (ModelInvocationJobIdentifier: an optional model-invocation-job ARN prefix plus a 12-char id) and both model ResourceNotFoundException with httpError 404. Recorded behavior, for a well-formed model-invocation-job ARN of the caller's account/Region that names no job: StopModelInvocationJob is refused 404 ResourceNotFoundException "The provided batch inference job was not found." and GetModelInvocationJob is refused 404 ResourceNotFoundException "InvokeModel-Job not found." Both messages are entirely fixed text — nothing in them comes from the request — and they are deliberately DIFFERENT strings per operation.
Emulator: StopModelInvocationJob answers ResourceNotFoundException (code correct) with "no batch inference job answers to <jobArn> in this account in <region>" — invented prose interpolating the ARN and Region. The check aborted before reaching the Get, which shares the same message surface and is part of this cause.
Why: a client cannot distinguish the stop and read refusals or match either sentence; error-message assertions and operator runbooks written against AWS break.
Fix: the not-found error construction in the StopModelInvocationJob and GetModelInvocationJob handlers — each must emit its own AWS sentence rather than one shared generated string.
Traces: bedrock.stop-unknown-batch-inference-job-rejected

## Evidence
Cause 1 — check bedrock.build-workflow-token-reused-with-other-content-rejected:
  Expected: ('ConflictException', 'A different request was previously submitted with the same ClientRequestToken')
  Actual:   the call succeeded, returning buildWorkflowId 'e5d54483-acb9-4ecc-af6f-8bf7fbd8b226'
  AssertionError: the call was expected to be refused but succeeded: 'e5d54483-acb9-4ecc-af6f-8bf7fbd8b226'

Cause 2 — check bedrock.second-build-workflow-while-one-runs-rejected:
  Expected message: starts with "Existing build workflow in progress for policy arn: arn:" and ends with the policy's own resource part (":automated-reasoning-policy/<id>"); code ResourceInUseException (correct in the emulator)
  Actual message:   'build workflow f6bbe7fc-1682-4943-a62a-b5d72e7e4158 of automated reasoning policy arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/000001dppa3s is SCHEDULED, so another build workflow cannot be started for it'

Cause 3 — check bedrock.stop-customization-job-unknown-arn-not-found:
  Expected: ('ResourceNotFoundException', 'Model-Customization job not found.')
  Actual:   ('ResourceNotFoundException', 'no model customization job answers to arn:aws:bedrock:us-west-2:000000000000:model-customization-job/amazon.nova-micro-v1:0:128k/30839b8b5909 in this account in us-west-2')
  (the ARN, account and Region above are this run's values filling the emulator's template; the requirement is the fixed sentence, which carries none of them)

Cause 4 — check bedrock.stop-unknown-batch-inference-job-rejected:
  Expected: ('ResourceNotFoundException', 'The provided batch inference job was not found.')
  Actual:   ('ResourceNotFoundException', 'no batch inference job answers to arn:aws:bedrock:us-west-2:000000000000:model-invocation-job/82c457db1a5e in this account in us-west-2')
  Not reached, same cause: GetModelInvocationJob on that ARN must answer ('ResourceNotFoundException', 'InvokeModel-Job not found.')

## Steps to Reproduce
Replay these behavioral checks against the emulator (bedrock control plane):

1. bedrock.build-workflow-token-reused-with-other-content-rejected
   CreateAutomatedReasoningPolicy; StartAutomatedReasoningPolicyBuildWorkflow(buildWorkflowType=INGEST_CONTENT, clientRequestToken=T, sourceContent=doc A) -> id1; repeat identically with T -> must return id1; then call it again with T but sourceContent=doc B (different bytes and documentName). Expect 400 ConflictException 'A different request was previously submitted with the same ClientRequestToken' and ListAutomatedReasoningPolicyBuildWorkflows still showing only id1. Emulator returns a success with a buildWorkflowId.

2. bedrock.second-build-workflow-while-one-runs-rejected
   CreateAutomatedReasoningPolicy; StartAutomatedReasoningPolicyBuildWorkflow (no token) -> running workflow; start a second one for the same policyArn. Expect 400 ResourceInUseException with message 'Existing build workflow in progress for policy arn: <policyArn>'. Inspect the message the emulator returns.

3. bedrock.stop-customization-job-unknown-arn-not-found
   Build a well-formed ARN arn:aws:bedrock:<region>:<account>:model-customization-job/amazon.nova-micro-v1:0:128k/<12 hex> naming no job. Call GetModelCustomizationJob(jobIdentifier=that ARN) and StopModelCustomizationJob(jobIdentifier=that ARN): expect 404 ResourceNotFoundException 'Model-Customization job not found.' for both. Then StopModelCustomizationJob with a bare unknown job NAME: expect ValidationException 'The provided job identifier is invalid.'

4. bedrock.stop-unknown-batch-inference-job-rejected
   Build arn:aws:bedrock:<region>:<account>:model-invocation-job/<12 hex> naming no job. StopModelInvocationJob(jobIdentifier=that ARN): expect 404 ResourceNotFoundException 'The provided batch inference job was not found.' GetModelInvocationJob(jobIdentifier=that ARN): expect 404 ResourceNotFoundException 'InvokeModel-Job not found.'
