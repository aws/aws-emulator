Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-command-execution-lookup-bounds
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-command-execution-lookup-bounds codebuild-retry-build-batch-input-rejections codebuild-retry-build-identifier-rejections codebuild-stop-build-batch-identifier-form-rejections codebuild-stop-build-identifier-form-rejections
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|BatchGetCommandExecutions|4xx->4xx|$.message
Issue-Key: codebuild|RetryBuildBatch|4xx->4xx|$.message
Issue-Key: codebuild|RetryBuildBatch|4xx->4xx|$.__type
Issue-Key: codebuild|RetryBuild|4xx->4xx|$.message
Issue-Key: codebuild|RetryBuild|4xx->4xx|$.__type
Issue-Key: codebuild|StopBuildBatch|4xx->4xx|$.__type
Issue-Key: codebuild|StopBuildBatch|4xx->4xx|$.message
Issue-Key: codebuild|StopBuild|4xx->4xx|$.__type
Issue-Key: codebuild|StopBuild|4xx->4xx|$.message
Issue-Commit: ab3264a14a30dede061951437467343aaef14e5b

# codebuild: 7 net-new causes — identifier and input rejections answer the wrong error type and message

> _Note: this issue was identified against the code base at commit `ab3264a14a30dede061951437467343aaef14e5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCommandExecutions

## Description
The emulator's CodeBuild input-rejection paths diverge from AWS in two linked ways: (a) a build/batch identifier whose FORM is not a valid CodeBuild identifier is run through the state store as a lookup and answered ResourceNotFoundException, where AWS refuses it as InvalidInputException before any lookup; and (b) where the emulator does reject, its message is the generic constraint-validation text (or a differently rendered value set) instead of the service's own wording. All 7 causes answer HTTP 400 like AWS, so only the error code (`__type`) and the `message` diverge — which is exactly what a client switches on. 7 causes, all on service codebuild.

Cause 1 of 7: BatchGetCommandExecutions — validation message for an over-long commandExecutionIds list.
AWS: Smithy: BatchGetCommandExecutions.commandExecutionIds targets CommandExecutionIds, @required, @length(min 1, max 100); the only modeled error is InvalidInputException (HTTP 400). For a list over the maximum AWS answers 400 InvalidInputException whose message is the standard validation text: the fixed text is "1 validation error detected: Value '" + the rejected value + "' at '" + the member name + "' failed to satisfy constraint: " + the constraint clause; the variable parts are the submitted list (rendered inside square brackets, elements comma+space separated, in request order), the member name (commandExecutionIds) and the limit from the model (100). The recorded response is 400 with that message, carrying the full submitted id list.
Emulator: answers 400 with the same error code but a different `message` body — the replay reports `$.message: value_mismatch` against the recorded expected message (the report's detail is clipped mid-value, so the emulator's exact rendering must be re-read from a replay of this step; what is established is that the text is not AWS's). The handler itself (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/batch_get_command_executions.rs) performs no length check, so the text comes from the shared constraint-validation layer rather than from CodeBuild's wording.
Why: a client that parses or asserts on the validation message (SDK integration tests, retry/triage logic keyed on the member name and limit named in the text) cannot tell which member or which bound it violated, and a test suite pinned to AWS's message fails against the emulator.
Fix: the request-validation path for this operation — the rendering of the `@length` violation message for a list member (member name, submitted value rendering and limit clause), not the handler body.
Traces: codebuild-command-execution-lookup-bounds#task#2 (also task#3).

Cause 2 of 7: RetryBuildBatch — enum value set in the validation message is re-ordered.
AWS: Smithy: RetryBuildBatch.retryType targets the enum RetryBuildBatchType; error InvalidInputException (HTTP 400). For an unmodeled enum value AWS answers 400 with the standard validation text whose fixed parts are "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]" and whose variable parts are the rejected value, the member name (retryType) and the enum's value set listed in the order the model declares its members. The recording shows AWS listing it as [RETRY_FAILED_BUILDS, RETRY_ALL_BUILDS].
Emulator: 400 with the set re-sorted: "1 validation error detected: Value 'RETRY_EVERYTHING' at 'retryType' failed to satisfy constraint: Member must satisfy enum value set: [RETRY_ALL_BUILDS, RETRY_FAILED_BUILDS]". The rejected value and member name are right; the value-set ordering is not.
Why: the enum set in the message is the documented list of accepted values; a client (or an assertion) that compares the message to AWS's sees a mismatch, and generated docs/tests that echo the message disagree with AWS.
Fix: the enum-constraint message rendering in the request-validation path — emit the value set in model declaration order instead of sorting it.
Traces: codebuild-retry-build-batch-input-rejections#task#2 (also task#3).

Cause 3 of 7: RetryBuildBatch — a malformed BuildBatch identifier is answered as not-found instead of invalid-input.
AWS: Smithy: RetryBuildBatch.id (NonEmptyString, @length min 1) with modeled errors InvalidInputException and ResourceNotFoundException (both HTTP 400). A CodeBuild batch identifier has the form <projectName>:<uuid> (or the batch ARN); an identifier that is not of that form names no batch, and AWS rejects it on FORM before any lookup: 400 InvalidInputException, message "Invalid BuildBatch ID provided" — fixed text with no variable part (the offending value is not echoed).
Emulator: 400 ResourceNotFoundException, message "BuildBatch cannot be found: <id>" (the recorded run shows the probe's generated id; the id is the variable part, the rest is the emulator's own text). It treats a form violation as a store miss.
Why: the two errors mean different things to a client — InvalidInputException says the request can never succeed, ResourceNotFoundException says this resource is absent and invites create-then-retry or a wait loop. Error-code-keyed handling (SDK retry/classification, CloudFormation and CI tooling) takes the wrong branch, and no form validation exists at all, so garbage identifiers are accepted into the lookup.
Fix: RetryBuildBatch's input validation — classify the identifier by form first and raise InvalidInputException with the service's wording, reserving ResourceNotFoundException for a well-formed id the store does not hold.
Traces: codebuild-retry-build-batch-input-rejections#task#4.

Cause 4 of 7: RetryBuild — a missing build identifier is answered with the generic required-member message.
AWS: Smithy: RetryBuild.id is NOT @required in the model (NonEmptyString, @length min 1); errors InvalidInputException, ResourceNotFoundException, AccountLimitExceededException (all HTTP 400). AWS enforces the identifier at the service level, not as a structural required-member violation: a request with no id gets 400 InvalidInputException with the message "Build ID or ARN is required" — entirely fixed text, no variable part.
Emulator: 400 with the generic structural message "Value at 'id' failed to satisfy constraint: Member must not be null" — a shape-level required-member rejection for a member the model does not mark @required, and not the wording AWS uses.
Why: a client asserting on AWS's message, or a user reading it, is told the failure is a null member rather than that a build ID or ARN must be supplied; the message also implies a model constraint that does not exist.
Fix: RetryBuild's input validation — raise InvalidInputException with the service-level "Build ID or ARN is required" text for an absent/empty id, instead of deferring to the generic required-member check.
Traces: codebuild-retry-build-identifier-rejections#task#2.

Cause 5 of 7: RetryBuild — a malformed build identifier is answered as not-found instead of invalid-input.
AWS: same model as cause 4. A build identifier is <projectName>:<uuid> or the build ARN; one that is not of that form is refused on form with 400 InvalidInputException, message "Invalid build ID provided" (fixed text, value not echoed).
Emulator: 400 ResourceNotFoundException, message "Build cannot be found: <id>" (variable part: the identifier as given, here an ARN the probe generated). The form check is absent, so the value falls through to a store lookup.
Why: as in cause 3 — the client reads "absent resource" where AWS says "invalid request", so retry/wait logic and error-code assertions diverge.
Fix: RetryBuild's input validation — form classification ahead of the lookup, with CodeBuild's wording.
Traces: codebuild-retry-build-identifier-rejections#task#3.

Cause 6 of 7: StopBuildBatch — a malformed BuildBatch identifier is answered as not-found instead of invalid-input.
AWS: Smithy: StopBuildBatch.id (NonEmptyString, @required, @length min 1); errors InvalidInputException and ResourceNotFoundException (HTTP 400). An identifier that is not a batch id (<projectName>:<uuid>) or batch ARN is refused on form: 400 InvalidInputException, message "Invalid BuildBatch ID provided" (fixed text).
Emulator: 400 ResourceNotFoundException, message "BuildBatch <id> does not exist" — the emulator's own template, whose variable part is the identifier as given; note it is also worded differently from the emulator's own RetryBuildBatch text, so the emulator is not even internally consistent.
Why: same client-visible consequence as cause 3; additionally a caller stopping a batch cannot distinguish "you sent nonsense" from "that batch is gone".
Fix: StopBuildBatch's input validation — classify the identifier by form before the store lookup.
Traces: codebuild-stop-build-batch-identifier-form-rejections#task#2 (also task#3).

Cause 7 of 7: StopBuild — a malformed build identifier is answered as not-found instead of invalid-input.
AWS: Smithy: StopBuild.id (NonEmptyString, @required, @length min 1); errors InvalidInputException and ResourceNotFoundException (HTTP 400). A value that is not <projectName>:<uuid> or a build ARN is refused on form: 400 InvalidInputException, message "Invalid build ID provided" (fixed text).
Emulator: 400 ResourceNotFoundException, message "Build <id> does not exist" (variable part: the identifier as given). The handler goes straight to the builds map and reports a miss.
Why: same as cause 5 — a stop request with a malformed id is reported as a missing build, so clients retry or poll for something that can never exist.
Fix: StopBuild's input validation — add form classification before the store lookup; the miss branch that currently owns this response is services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/stop_build.rs:154-163, which should keep ResourceNotFoundException only for a well-formed id the store does not hold.
Traces: codebuild-stop-build-identifier-form-rejections#task#2 (also task#3).

Relation to open issues: the open codebuild issues concern build EXECUTION (builds FAULT at PROVISIONING, sandbox executions never run) — e.g. 20261027T101500Z-codebuild-sandbox-command-executions-never-run-the-command.md and 20261125T142812Z-codebuild-a-build-advances-only-while-an-api.md. None of them covers input rejection/error-classification on these five operations, and none carries these replay keys.

## Evidence
Replay of simparity-wire-aws-v1 (29 fixtures, 199 steps, 184 passed, 15 mismatched; score 92.5%). All 7 clusters are service=codebuild, verdict=mismatch, expected_status=400 and actual_status=400 — only $.__type and/or $.message diverge; none carries a stub_path or owned_by.

1) codebuild-command-execution-lookup-bounds#task#2 (BatchGetCommandExecutions), affected also task#3 — $.message: value_mismatch; expected "1 validation error detected: Value '[<submitted commandExecutionIds>]' at '<member>' failed to satisfy constraint: ..." (report detail clipped inside the value list); emulator message differs.
2) codebuild-retry-build-batch-input-rejections#task#2 (RetryBuildBatch), affected also task#3 — $.message expected "1 validation error detected: Value 'RETRY_EVERYTHING' at 'retryType' failed to satisfy constraint: Member must satisfy enum value set: [RETRY_FAILED_BUILDS, RETRY_ALL_BUILDS]", got "... enum value set: [RETRY_ALL_BUILDS, RETRY_FAILED_BUILDS]".
3) codebuild-retry-build-batch-input-rejections#task#4 (RetryBuildBatch) — $.__type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected 'Invalid BuildBatch ID provided', got 'BuildBatch cannot be found: sim2real-probe-cbretrybatch-8db8a88f'.
4) codebuild-retry-build-identifier-rejections#task#2 (RetryBuild) — $.message expected 'Build ID or ARN is required', got "Value at 'id' failed to satisfy constraint: Member must not be null".
5) codebuild-retry-build-identifier-rejections#task#3 (RetryBuild) — $.__type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected 'Invalid build ID provided', got 'Build cannot be found: arn:aws:codebuild:us-west-2:000000000000:build/sim2real-probe-cbretry-9fe21f0b'.
6) codebuild-stop-build-batch-identifier-form-rejections#task#2 (StopBuildBatch), affected also task#3 — $.__type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected 'Invalid BuildBatch ID provided', got 'BuildBatch sim2real-probe-cbstopbatch-c3897793 does not exist'.
7) codebuild-stop-build-identifier-form-rejections#task#2 (StopBuild), affected also task#3 — $.__type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected 'Invalid build ID provided', got 'Build sim2real-probe-cbstop-392b39b1 does not exist'.

The generated ids/ARNs above are per-run values filling the message template; the requirement is the fixed text and the member/limit/value-set parts named in the description. Model evidence (services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json): BatchGetCommandExecutions.commandExecutionIds @required @length(1,100), only error InvalidInputException; RetryBuild.id NOT @required; RetryBuildBatchType = {RETRY_ALL_BUILDS, RETRY_FAILED_BUILDS}; StopBuild/StopBuildBatch.id @required, errors InvalidInputException + ResourceNotFoundException. Code evidence: src/ops/stop_build.rs:154-163 raises ResourceNotFoundException on a bare map miss with no form check; src/ops/batch_get_command_executions.rs performs no list-length check.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) and look at the service=codebuild clusters.
3. Cause 1: replay codebuild-command-execution-lookup-bounds step task#2 — POST BatchGetCommandExecutions with a valid sandboxId and more than 100 commandExecutionIds; compare the 400 body's message to AWS's "1 validation error detected: Value '[...]' at 'commandExecutionIds' failed to satisfy constraint: ...".
4. Cause 2: replay codebuild-retry-build-batch-input-rejections step task#2 — RetryBuildBatch with retryType='RETRY_EVERYTHING'; the enum value set must be listed [RETRY_FAILED_BUILDS, RETRY_ALL_BUILDS].
5. Cause 3: replay the same fixture step task#4 — RetryBuildBatch with an id that is not <projectName>:<uuid> or a batch ARN; expect 400 InvalidInputException 'Invalid BuildBatch ID provided'.
6. Cause 4: replay codebuild-retry-build-identifier-rejections step task#2 — RetryBuild with no id; expect 400 InvalidInputException 'Build ID or ARN is required'.
7. Cause 5: replay the same fixture step task#3 — RetryBuild with a syntactically wrong build ARN/id; expect 400 InvalidInputException 'Invalid build ID provided'.
8. Cause 6: replay codebuild-stop-build-batch-identifier-form-rejections step task#2 — StopBuildBatch with a malformed batch id; expect 400 InvalidInputException 'Invalid BuildBatch ID provided'.
9. Cause 7: replay codebuild-stop-build-identifier-form-rejections step task#2 — StopBuild with a malformed build id; expect 400 InvalidInputException 'Invalid build ID provided'.
10. All seven steps (plus the task#3 members of clusters 1, 2, 6 and 7) must pass before closing this issue.
