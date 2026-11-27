Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-job-stop-bare-identifier-refusals
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-job-stop-bare-identifier-refusals advanced-prompt-optimization-job-stop-refusals automated-reasoning-policy-workflow-start-refusals bedrock-stop-model-customization-job-unknown-name-invalid
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|StopModelInvocationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopModelInvocationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|StopModelInvocationJob|4xx->4xx|status
Issue-Key: bedrock|StopAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopAdvancedPromptOptimizationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|StopAdvancedPromptOptimizationJob|4xx->4xx|status
Issue-Key: bedrock|StartAutomatedReasoningPolicyBuildWorkflow|4xx->4xx|$.message
Issue-Key: bedrock|StartAutomatedReasoningPolicyBuildWorkflow|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|StartAutomatedReasoningPolicyBuildWorkflow|4xx->4xx|status
Issue-Key: bedrock|StopModelCustomizationJob|4xx->4xx|$.message
Issue-Key: bedrock|StopModelCustomizationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|StopModelCustomizationJob|4xx->4xx|status
Issue-Key: bedrock|GetModelCustomizationJob|4xx->4xx|$.message
Issue-Key: bedrock|GetModelCustomizationJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|GetModelCustomizationJob|4xx->4xx|status
Issue-Commit: e41b3b38bdf5811ca9bd8e7fba3b9fd5eb2e4bc9

# bedrock job and policy identifier refusals answer 404 ResourceNotFoundException where AWS answers 400 ValidationException

> _Note: this issue was identified against the code base at commit `e41b3b38bdf5811ca9bd8e7fba3b9fd5eb2e4bc9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopModelInvocationJob

## Description
Five independent root causes in the bedrock control plane share one fault: an identifier that AWS rejects up front as malformed/unsupported for the operation is instead resolved against state and refused as a missing resource, so the status CLASS (400 vs 404), the error code (ValidationException vs ResourceNotFoundException), the x-amzn-errortype header and the message are all wrong. Open issue 20261127T133904Z-bedrock-4-net-new-causes-build-workflow-clientrequesttoken.md covers the ARN-form not-found MESSAGES for model-customization and batch-inference jobs from the behavioral corpus; it carries no wire replay key and does not own any of the steps below, where the identifier is not an ARN at all (or is a cross-Region/versioned ARN) and the required answer is a 400, not a reworded 404. Number of causes to close: 5.

Cause 1 of 5: StopAdvancedPromptOptimizationJob accepts a job ARN whose Region segment is not the endpoint's Region and treats it as a lookup miss instead of refusing the ARN.
AWS: Smithy (com.amazonaws.bedrock) StopAdvancedPromptOptimizationJob.jobIdentifier targets AdvancedPromptOptimizationJobIdentifier, @httpLabel, required, @length max 1011, @pattern ^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$; modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404). The @pattern alone admits any Region segment, so the refusal is a semantic one: recorded behavior shows a syntactically well-formed advanced-prompt-optimization-job ARN naming a Region other than the one the endpoint serves is refused 400 ValidationException with the message "The provided ARN is invalid." — the message is entirely FIXED text and carries no value from the request. Hypothesis: AWS validates the ARN's Region (and account) against the request context before any job lookup.
Emulator: answers 404 with x-amzn-errortype ResourceNotFoundException and the emulator-authored message "no advanced prompt optimization job answers to <jobArn> in this account in <region>" (template: fixed prose interpolating the request's ARN and the serving Region — neither appears in AWS's sentence).
Why: a client cannot distinguish "you sent me an identifier for another Region" from "this job does not exist". Retry and error-classification logic branches on the 4xx code: a ValidationException is a permanent request defect the caller must fix, while a ResourceNotFoundException is routinely treated as "not created yet / already gone" and polled or swallowed, so a cross-Region ARN bug becomes invisible and the caller loops.
Fix: the StopAdvancedPromptOptimizationJob handler's input validation — the ARN form must be validated (Region/account against the request context) and refused with ValidationException before the job lookup in state runs.
Traces: advanced-prompt-optimization-job-stop-refusals#task#3

Cause 2 of 5: StartAutomatedReasoningPolicyBuildWorkflow accepts a VERSIONED policy ARN and reports the policy as missing instead of refusing the versioned ARN.
AWS: Smithy StartAutomatedReasoningPolicyBuildWorkflow.policyArn targets AutomatedReasoningPolicyArn, @httpLabel, required, @pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$ — the version suffix is syntactically allowed by the shared shape, but this operation does not accept it; ValidationException (400) is modeled. Recorded behavior: a policy ARN carrying a version suffix is refused 400 ValidationException with the message "PolicyArn: +<policyArn> should not contain version." — fixed text "PolicyArn: +" and " should not contain version." with the request's policyArn (version suffix included) as the only variable part.
Emulator: answers 404 with x-amzn-errortype ResourceNotFoundException and the message "Policy does not exist" — it strips/ignores nothing and simply fails to find a policy whose id includes the version, so an unsupported-identifier-shape error is reported as a missing resource.
Why: the caller that passed a version ARN (the natural mistake, since GetAutomatedReasoningPolicy-style reads accept one) is told its policy does not exist, so it re-creates or re-polls the policy instead of dropping the version suffix. The 400/404 split also flips the error's retry classification.
Fix: the StartAutomatedReasoningPolicyBuildWorkflow handler's policyArn validation — reject a version-qualified ARN with ValidationException before resolving the policy in state.
Traces: automated-reasoning-policy-workflow-start-refusals#task#5

Cause 3 of 5 (representative): StopModelInvocationJob accepts a BARE 12-character job identifier and answers not-found, where AWS requires the ARN form and refuses the bare id.
AWS: Smithy StopModelInvocationJob.jobIdentifier targets ModelInvocationJobIdentifier, @httpLabel, required, @length max 1011, @pattern ^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-invocation-job/)?[a-z0-9]{12})$ (the ARN prefix is optional in the shape); ValidationException (400) and ResourceNotFoundException (404) are both modeled. Recorded behavior only: a bare 12-char identifier (no ARN prefix) is refused 400 ValidationException with the message "The provided ARN is invalid." — entirely fixed text, no value taken from the request. So the operation's requirement is the full model-invocation-job ARN; the bare form is a request defect, not a lookup.
Emulator: answers 404 with x-amzn-errortype ResourceNotFoundException and "no batch inference job answers to <jobIdentifier> in this account in <region>" (emulator-authored template interpolating the identifier and Region).
Why: a caller that stored only the job's short id learns "no such job" and concludes the job was reaped, instead of learning that it must pass the ARN. Code that treats 404 as terminal-success for a stop ("already gone") silently skips a stop that never happened.
Fix: the StopModelInvocationJob handler's jobIdentifier validation — require the ARN form and raise ValidationException with AWS's fixed sentence before the state lookup.
Traces: bedrock-job-stop-bare-identifier-refusals#task#3

Cause 4 of 5: StopModelCustomizationJob accepts a bare job NAME and answers not-found, where AWS refuses the identifier.
AWS: Smithy StopModelCustomizationJob.jobIdentifier targets ModelCustomizationJobIdentifier, @httpLabel, required, @length max 1011, @pattern ^(arn:...:model-customization-job/<model>/[a-z0-9]{12})|([a-zA-Z0-9](-*[a-zA-Z0-9\+\-\.])*)$ — the bare-name alternative is syntactically admitted, yet recorded behavior shows a bare, unknown job name is refused 400 ValidationException with the message "The provided job identifier is invalid." — entirely fixed text, nothing from the request. ValidationException (400) and ResourceNotFoundException (404) are both modeled on the operation.
Emulator: answers 404 with x-amzn-errortype ResourceNotFoundException and "no model customization job answers to <jobIdentifier> in this account in <region>".
Why: same inverted classification as above — a permanent request defect is reported as a transient/absent resource, so SDK retry and "already stopped" handling take the wrong branch, and the caller is never told the identifier form is unusable.
Fix: the identifier validation shared by the model-customization-job handlers (StopModelCustomizationJob and GetModelCustomizationJob), which must raise ValidationException for a non-ARN identifier before the job lookup.
Traces: bedrock-job-stop-bare-identifier-refusals#task#4

Cause 5 of 5: GetModelCustomizationJob has the same defect on the read path.
AWS: Smithy GetModelCustomizationJob.jobIdentifier is the same ModelCustomizationJobIdentifier @httpLabel member, with ValidationException (400) and ResourceNotFoundException (404) modeled. Recorded behavior: a bare, unknown job name is refused 400 ValidationException "The provided job identifier is invalid." (fixed text).
Emulator: 404 ResourceNotFoundException with "no model customization job answers to <jobIdentifier> in this account in <region>".
Why: a describe loop that polls until the job is found will poll forever on an identifier AWS would have rejected immediately.
Fix: the GetModelCustomizationJob handler's validation (same shared identifier check as cause 4); it is listed separately because the read path is a distinct handler and must be closed too.
Traces: bedrock-stop-model-customization-job-unknown-name-invalid#task#2

## Evidence
All five causes are wire-replay mismatches from the simparity-wire-aws-v1 corpus. Status, error code and message are requirements; the ARNs, job ids, account id and Region in the recorded strings are this run's values filling a template.

Cause 1 — advanced-prompt-optimization-job-stop-refusals#task#3 (StopAdvancedPromptOptimizationJob, cross-Region job ARN):
  Expected: 400, message 'The provided ARN is invalid.' (fixed text, no variable part)
  Actual:   404, x-amzn-errortype: ResourceNotFoundException, message 'no advanced prompt optimization job answers to arn:aws:bedrock:us-east-1:000000000000:advanced-prompt-optimization-job/qqqq4444qqqq in this account in us-west-2'
  Replay detail: "status: expected 400, got 404 / $.message: value_mismatch / $headers.x-amzn-errortype: shape_mismatch"

Cause 2 — automated-reasoning-policy-workflow-start-refusals#task#5 (StartAutomatedReasoningPolicyBuildWorkflow, version-qualified policy ARN):
  Expected: 400, message 'PolicyArn: +arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/0000011bazjb:9 should not contain version.' (fixed text 'PolicyArn: +' ... ' should not contain version.'; the ARN is the request's policyArn)
  Actual:   404, x-amzn-errortype: ResourceNotFoundException, message 'Policy does not exist'

Cause 3 — bedrock-job-stop-bare-identifier-refusals#task#3 (StopModelInvocationJob, bare 12-char id):
  Expected: 400, message 'The provided ARN is invalid.'
  Actual:   404, x-amzn-errortype: ResourceNotFoundException, message 'no batch inference job answers to 5f55fde6ca96 in this account in us-west-2'

Cause 4 — bedrock-job-stop-bare-identifier-refusals#task#4 (StopModelCustomizationJob, bare job name); same cluster also covers bedrock-stop-model-customization-job-unknown-name-invalid#task#1:
  Expected: 400, message 'The provided job identifier is invalid.'
  Actual:   404, x-amzn-errortype: ResourceNotFoundException, message 'no model customization job answers to sim2real-probe-nojob-4171406a in this account in us-west-2'

Cause 5 — bedrock-stop-model-customization-job-unknown-name-invalid#task#2 (GetModelCustomizationJob, bare job name):
  Expected: 400, message 'The provided job identifier is invalid.'
  Actual:   404, x-amzn-errortype: ResourceNotFoundException, message 'no model customization job answers to sim2real-probe-bedrock-nojob-0f8b6861 in this account in us-west-2'

## Steps to Reproduce
Replay these wire fixture steps of simparity-wire-aws-v1 against the emulator's bedrock control plane (Region us-west-2, account 000000000000):

1. advanced-prompt-optimization-job-stop-refusals#task#3 — POST /advanced-prompt-optimization-jobs/{jobIdentifier}/stop with jobIdentifier = an advanced-prompt-optimization-job ARN whose Region segment is NOT the endpoint's Region (e.g. us-east-1 against a us-west-2 endpoint) and a 12-char id. Expect 400 ValidationException 'The provided ARN is invalid.'
2. automated-reasoning-policy-workflow-start-refusals#task#5 — StartAutomatedReasoningPolicyBuildWorkflow with policyArn = a well-formed automated-reasoning-policy ARN carrying a version suffix (…/<12-char id>:<n>). Expect 400 ValidationException 'PolicyArn: +<that ARN> should not contain version.'
3. bedrock-job-stop-bare-identifier-refusals#task#3 — StopModelInvocationJob with jobIdentifier = a bare 12-char id (no ARN prefix). Expect 400 ValidationException 'The provided ARN is invalid.'
4. bedrock-job-stop-bare-identifier-refusals#task#4 — StopModelCustomizationJob with jobIdentifier = a bare job name that names no job. Expect 400 ValidationException 'The provided job identifier is invalid.'
5. bedrock-stop-model-customization-job-unknown-name-invalid#task#2 — GetModelCustomizationJob with jobIdentifier = a bare job name that names no job. Expect 400 ValidationException 'The provided job identifier is invalid.'

In each case the emulator currently answers 404 with x-amzn-errortype ResourceNotFoundException and an emulator-authored 'no <resource> answers to <identifier> in this account in <region>' / 'Policy does not exist' message.
