Issue-Source: trace-sweep-divergence
Issue-Trace: automated-reasoning-policy-workflow-start-refusals
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: automated-reasoning-policy-workflow-start-refusals bedrock-cancel-build-workflow-identifier-validation advanced-prompt-optimization-job-stop-refusals bedrock-job-stop-bare-identifier-refusals
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|StartAutomatedReasoningPolicyTestWorkflow|4xx->4xx|$.message
Issue-Key: bedrock|CancelAutomatedReasoningPolicyBuildWorkflow|4xx->4xx|$.message
Issue-Key: bedrock|GetAdvancedPromptOptimizationJob|4xx->4xx|$.message
Issue-Related: 20261127T202739Z-bedrock-job-and-policy-identifier-refusals-answer-404.md
Issue-Commit: e41b3b38bdf5811ca9bd8e7fba3b9fd5eb2e4bc9

# bedrock control-plane refusal messages are emulator-authored for job, policy and URI-label validation failures

> _Note: this issue was identified against the code base at commit `e41b3b38bdf5811ca9bd8e7fba3b9fd5eb2e4bc9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomatedReasoningPolicyBuildWorkflow

## Description
Six independent root causes in the bedrock control plane where the HTTP status and the error code are already correct but the error MESSAGE is emulator-authored instead of AWS's. Two sub-families: (a) not-found sentences for automated-reasoning policies and advanced-prompt-optimization jobs are invented prose, (b) the generated "1 validation error detected: …" sentence for URI-label (@httpLabel) members echoes the offending value, which AWS omits for these members, and the enum value set it prints is incomplete. Open issue 20261127T133904Z-bedrock-4-net-new-causes-build-workflow-clientrequesttoken.md covers the model-customization-job / batch-inference-job and build-workflow-in-progress message families from the behavioral corpus and does NOT own any step below (the replay marks none of them owned, and none of these operations/messages appear in it). Number of causes to close: 6.

Cause 1 of 6 (representative): the StartAutomatedReasoningPolicyBuildWorkflow not-found message for an unknown policy is reworded.
AWS: Smithy (com.amazonaws.bedrock) StartAutomatedReasoningPolicyBuildWorkflow models ResourceNotFoundException (httpError 404), whose doc is "The specified resource Amazon Resource Name (ARN) was not found…"; the exception's only member is message (NonBlankString). Recorded behavior: a well-formed, unversioned policy ARN of the caller's own account and Region that names no policy is refused 404 ResourceNotFoundException with the message "Policy was not found" — entirely FIXED text (no trailing period, nothing taken from the request or from state).
Emulator: 404 ResourceNotFoundException (status and code correct) with the message "Policy does not exist".
Why: AWS's not-found sentence for this resource is a constant that clients, test assertions and operator log scrapers match on; a reworded constant makes the refusal unrecognizable even though the code is right.
Fix: the not-found error construction on the StartAutomatedReasoningPolicyBuildWorkflow handler's policy-resolution path (message text only — the detection is correct).
Traces: automated-reasoning-policy-workflow-start-refusals#task#3

Cause 2 of 6: StartAutomatedReasoningPolicyTestWorkflow has the same reworded not-found sentence.
AWS: Smithy StartAutomatedReasoningPolicyTestWorkflow takes policyArn (@httpLabel, required) and models ResourceNotFoundException (404). Recorded behavior: an unknown policy ARN is refused 404 ResourceNotFoundException "Policy was not found" — fixed text.
Emulator: 404 ResourceNotFoundException with "Policy does not exist".
Why: as cause 1; it is listed separately because this is a distinct handler and will not be closed by fixing the build-workflow path alone.
Fix: the not-found error construction on the StartAutomatedReasoningPolicyTestWorkflow handler (ideally a single shared policy-not-found error used by both).
Traces: automated-reasoning-policy-workflow-start-refusals#task#6

Cause 3 of 6: the buildWorkflowType enum ValidationException message echoes the offending value and prints an incomplete enum value set.
AWS: Smithy StartAutomatedReasoningPolicyBuildWorkflow.buildWorkflowType is a required @httpLabel member targeting the enum AutomatedReasoningPolicyBuildWorkflowType; ValidationException is modeled with httpError 400. Recorded behavior: an unmodeled workflow-type label is refused 400 with the message "1 validation error detected: Value at 'buildWorkflowType' failed to satisfy constraint: Member must satisfy enum value set: [GENERATE_POLICY_SCENARIOS, GENERATE_FIDELITY_REPORT, REFINE_POLICY, IMPORT_POLICY, RESOLVE_POLICY_AMBIGUITIES, BUILD_FIXED_SCHEMA_POLICY, INGEST_CONTENT, ITERATIVELY_REFINE_POLICY]". Two requirements follow: the sentence names the member but NOT the rejected value (fixed text "1 validation error detected: Value at '" + member name + "' failed to satisfy constraint: Member must satisfy enum value set: [" + the enum's values + "]"), and the value set includes BUILD_FIXED_SCHEMA_POLICY, which the on-disk model's enum (INGEST_CONTENT, REFINE_POLICY, IMPORT_POLICY, GENERATE_FIDELITY_REPORT, GENERATE_POLICY_SCENARIOS, RESOLVE_POLICY_AMBIGUITIES, ITERATIVELY_REFINE_POLICY) omits. Hypothesis: AWS omits the offending value for members bound to the URI path, and the service's enum is a superset of the checked-in model's.
Emulator: 400 (status and code correct) with "1 validation error detected: Value '<the rejected label>' at 'buildWorkflowType' failed to satisfy constraint: Member must satisfy enum value set: [INGEST_CONTENT, REFINE_POLICY, IMPORT_POLICY, GENERATE_FIDELITY_REPORT, GENERATE_POLICY_SCENARIOS, RESOLVE_POLICY_AMBIGUITIES, ITERATIVELY_REFINE_POLICY]" — it interpolates the value AWS withholds and advertises seven of the eight accepted values.
Why: the value set in this message is how a client discovers what the service accepts; a caller reading it concludes BUILD_FIXED_SCHEMA_POLICY is invalid and will not attempt a workflow type AWS supports. The spurious value echo also makes the sentence unmatchable for assertions written against AWS.
Fix: the request-validation message builder for @httpLabel members in this service (suppress the value for URI-bound members) and the buildWorkflowType enum value set it renders.
Traces: automated-reasoning-policy-workflow-start-refusals#task#4

Cause 4 of 6: the buildWorkflowId @pattern ValidationException message likewise echoes the offending value.
AWS: Smithy CancelAutomatedReasoningPolicyBuildWorkflow.buildWorkflowId is a required @httpLabel member targeting AutomatedReasoningPolicyBuildWorkflowId (@length max 36, @pattern ^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$); ValidationException is modeled with httpError 400. Recorded behavior: a non-conforming id is refused 400 with "1 validation error detected: Value at 'buildWorkflowId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}" — fixed text plus the member name and the modeled pattern (unanchored in the message), and NO rejected value.
Emulator: 400 (code correct) with "1 validation error detected: Value '<the rejected id>' at 'buildWorkflowId' failed to satisfy constraint: Member must satisfy regular expression pattern: …" — the same spurious value echo.
Why: assertions and log matchers built on AWS's constant sentence fail, and echoing a caller-supplied URI segment back in an error body is a behavior AWS deliberately avoids here.
Fix: the same @httpLabel validation-message builder as cause 3, on the CancelAutomatedReasoningPolicyBuildWorkflow path (pattern failures as well as enum failures).
Traces: bedrock-cancel-build-workflow-identifier-validation#task#2

Cause 5 of 6: the StopAdvancedPromptOptimizationJob not-found message is emulator-authored.
AWS: Smithy StopAdvancedPromptOptimizationJob.jobIdentifier (@httpLabel, required, AdvancedPromptOptimizationJobIdentifier: optional ARN prefix plus a 12-char [a-z0-9] id) with ResourceNotFoundException modeled at httpError 404. Recorded behavior: a bare, syntactically valid 12-char job id that names no job is refused 404 ResourceNotFoundException with "Cannot find advanced prompt optimization job <jobIdentifier>." — fixed text "Cannot find advanced prompt optimization job " + the jobIdentifier from the request + ".".
Emulator: 404 ResourceNotFoundException (code correct) with "no advanced prompt optimization job answers to <jobIdentifier> in this account in <region>" — invented prose that additionally interpolates the serving Region, which is not part of AWS's message.
Why: the sentence is part of the error contract; clients that branch on it, and operators grepping it, do not recognize the emulator's wording, and the Region in it implies a contract element that does not exist.
Fix: the not-found error construction in the StopAdvancedPromptOptimizationJob handler.
Traces: advanced-prompt-optimization-job-stop-refusals#task#4

Cause 6 of 6: GetAdvancedPromptOptimizationJob has the same emulator-authored not-found message on the read path.
AWS: Smithy GetAdvancedPromptOptimizationJob.jobIdentifier is the same identifier member, with ResourceNotFoundException modeled at 404. Recorded behavior: an unknown bare 12-char job id is refused 404 ResourceNotFoundException "Cannot find advanced prompt optimization job <jobIdentifier>." (same template as cause 5).
Emulator: 404 ResourceNotFoundException with "no advanced prompt optimization job answers to <jobIdentifier> in this account in <region>".
Why: as cause 5; a distinct handler, so it must be closed separately.
Fix: the not-found error construction in the GetAdvancedPromptOptimizationJob handler (ideally one shared advanced-prompt-optimization-job not-found error used by both read and stop).
Traces: bedrock-job-stop-bare-identifier-refusals#task#2

## Evidence
All six causes are wire-replay mismatches from simparity-wire-aws-v1 in which the status and the error code matched and only $.message diverged. The job ids, policy ARNs, account id and Region below are this run's values filling a template; the requirement is the fixed text plus the named variable parts.

Cause 1 — automated-reasoning-policy-workflow-start-refusals#task#3 (StartAutomatedReasoningPolicyBuildWorkflow, unknown policy):
  Expected 404 message: 'Policy was not found'
  Actual   404 message: 'Policy does not exist'

Cause 2 — automated-reasoning-policy-workflow-start-refusals#task#6 (StartAutomatedReasoningPolicyTestWorkflow, unknown policy):
  Expected 404 message: 'Policy was not found'
  Actual   404 message: 'Policy does not exist'

Cause 3 — automated-reasoning-policy-workflow-start-refusals#task#4 (StartAutomatedReasoningPolicyBuildWorkflow, unmodeled buildWorkflowType label):
  Expected 400 message: "1 validation error detected: Value at 'buildWorkflowType' failed to satisfy constraint: Member must satisfy enum value set: [GENERATE_POLICY_SCENARIOS, GENERATE_FIDELITY_REPORT, REFINE_POLICY, IMPORT_POLICY, RESOLVE_POLICY_AMBIGUITIES, BUILD_FIXED_SCHEMA_POLICY, INGEST_CONTENT, ITERATIVELY_REFINE_POLICY]"
  Actual   400 message: "1 validation error detected: Value 'SUMMARIZE_CONTENT' at 'buildWorkflowType' failed to satisfy constraint: Member must satisfy constraint: Member must satisfy enum value set: [INGEST_CONTENT, REFINE_POLICY, IMPORT_POLICY, GENERATE_FIDELITY_REPORT, GENERATE_POLICY_SCENARIOS, RESOLVE_POLICY_AMBIGUITIES, ITERATIVELY_REFINE_POLICY]" (as recorded: "Value 'SUMMARIZE_CONTENT' at 'buildWorkflowType' failed to satisfy constraint: Member must satisfy enum value set: [INGEST_CONTENT, REFINE_POLICY, IMPORT_POLICY, GENERATE_FIDELITY_REPORT, GENERATE_POLICY_SCENARIOS, RESOLVE_POLICY_AMBIGUITIES, ITERATIVELY_REFINE_POLICY]")
  Two diffs: the rejected value ('SUMMARIZE_CONTENT' here — a request value, not part of the required template) must NOT appear, and BUILD_FIXED_SCHEMA_POLICY is missing from the advertised value set (the on-disk model's enum also omits it).

Cause 4 — bedrock-cancel-build-workflow-identifier-validation#task#2 (CancelAutomatedReasoningPolicyBuildWorkflow, malformed buildWorkflowId); same cluster also covers bedrock-cancel-build-workflow-identifier-validation#task#3:
  Expected 400 message: "1 validation error detected: Value at 'buildWorkflowId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}"
  Actual   400 message: "1 validation error detected: Value 'not-a-build-workflow' at 'buildWorkflowId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}" ('not-a-build-workflow' is the request's value and must not be echoed)

Cause 5 — advanced-prompt-optimization-job-stop-refusals#task#4 (StopAdvancedPromptOptimizationJob, unknown bare job id); same cluster also covers bedrock-job-stop-bare-identifier-refusals#task#1:
  Expected 404 message: 'Cannot find advanced prompt optimization job qqqq4444qqqq.' (fixed text + the request's jobIdentifier + '.')
  Actual   404 message: 'no advanced prompt optimization job answers to qqqq4444qqqq in this account in us-west-2'

Cause 6 — bedrock-job-stop-bare-identifier-refusals#task#2 (GetAdvancedPromptOptimizationJob, unknown bare job id):
  Expected 404 message: 'Cannot find advanced prompt optimization job 5f55fde6ca96.'
  Actual   404 message: 'no advanced prompt optimization job answers to 5f55fde6ca96 in this account in us-west-2'

## Steps to Reproduce
Replay these wire fixture steps of simparity-wire-aws-v1 against the emulator's bedrock control plane (Region us-west-2, account 000000000000) and compare the error message body only (status and error code already match):

1. automated-reasoning-policy-workflow-start-refusals#task#3 — StartAutomatedReasoningPolicyBuildWorkflow with a well-formed, unversioned automated-reasoning-policy ARN of the caller's account/Region that names no policy. Expect 404 ResourceNotFoundException 'Policy was not found'.
2. automated-reasoning-policy-workflow-start-refusals#task#6 — StartAutomatedReasoningPolicyTestWorkflow with the same unknown policy ARN. Expect 404 ResourceNotFoundException 'Policy was not found'.
3. automated-reasoning-policy-workflow-start-refusals#task#4 — StartAutomatedReasoningPolicyBuildWorkflow with a buildWorkflowType URI label that is not a modeled enum value. Expect 400 ValidationException "1 validation error detected: Value at 'buildWorkflowType' failed to satisfy constraint: Member must satisfy enum value set: [GENERATE_POLICY_SCENARIOS, GENERATE_FIDELITY_REPORT, REFINE_POLICY, IMPORT_POLICY, RESOLVE_POLICY_AMBIGUITIES, BUILD_FIXED_SCHEMA_POLICY, INGEST_CONTENT, ITERATIVELY_REFINE_POLICY]" — no rejected value in the sentence.
4. bedrock-cancel-build-workflow-identifier-validation#task#2 — CancelAutomatedReasoningPolicyBuildWorkflow with a buildWorkflowId that does not match the modeled UUIDv4 pattern. Expect 400 ValidationException "1 validation error detected: Value at 'buildWorkflowId' failed to satisfy constraint: Member must satisfy regular expression pattern: …" — no rejected value in the sentence.
5. advanced-prompt-optimization-job-stop-refusals#task#4 — StopAdvancedPromptOptimizationJob with a bare 12-char [a-z0-9] job id naming no job. Expect 404 ResourceNotFoundException 'Cannot find advanced prompt optimization job <that id>.'
6. bedrock-job-stop-bare-identifier-refusals#task#2 — GetAdvancedPromptOptimizationJob with a bare 12-char job id naming no job. Expect 404 ResourceNotFoundException 'Cannot find advanced prompt optimization job <that id>.'
