Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__stepfunctions.sfn-execution-role-trust-honors-source-account-and-source-arn kms__stepfunctions.sfn-execution-role-trust-source-account-mismatch-blocks-cmk-execution kms__stepfunctions.start-execution-cmk-key-denies-or-disabled-create-nothing
Issue-Key: kms__stepfunctions.sfn-execution-role-trust-honors-source-account-and-source-arn|_start_until_permanent_failure#21|AssertionError|raise AssertionError(f"StartExecution unexpectedly started {arn!r} against the mismatched trust")
Issue-Key: kms__stepfunctions.sfn-execution-role-trust-source-account-mismatch-blocks-cmk-execution|_start_until_permanent_failure#7|AssertionError|raise AssertionError("StartExecution unexpectedly succeeded against the mismatched trust")
Issue-Key: kms__stepfunctions.start-execution-cmk-key-denies-or-disabled-create-nothing|run#16|AssertionError|assert _stable_session(_stable(message), role_name) == expected, (
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# kms__stepfunctions: 2 net-new causes — an execution role's aws:SourceAccount/aws:SourceArn trust conditions are not enforced on StartExecution, and the relayed KMS denial names the role ARN instead of the assumed-role session ARN

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two causes about how Step Functions assumes and reports its execution role on CMK-encrypted state machines.

Cause 1 of 2: the execution role's trust policy conditions are ignored, so StartExecution starts an execution with a role Step Functions may not assume.
AWS: recorded behavior only (the Smithy model constrains the role only as an Arn). A state machine whose execution role trusts states.amazonaws.com with a Condition on aws:SourceAccount/aws:SourceArn that does not match the starting state machine cannot be assumed, so the execution is refused/permanently fails rather than running; the same role with matching conditions works. On a CMK machine the refusal is the KMS-relayed denial for the assume, i.e. the attempt never produces a RUNNING execution.
Emulator: StartExecution returns 200 and an executionArn against the deliberately mismatched trust (both for a mismatched aws:SourceArn and a mismatched aws:SourceAccount), so the mismatch is never observable — the trust document's Condition block is not evaluated when the role is assumed.
Why: aws:SourceAccount/aws:SourceArn on a service trust policy is the standard confused-deputy guard; a client that hardens a role this way, or tests that the guard bites, is told the unsafe configuration works.
Fix: the role-assumption step of the StartExecution handler / execution runtime — evaluate the trust policy's Condition block (aws:SourceAccount, aws:SourceArn) before assuming the execution role.
Traces: kms__stepfunctions.sfn-execution-role-trust-honors-source-account-and-source-arn, kms__stepfunctions.sfn-execution-role-trust-source-account-mismatch-blocks-cmk-execution.

Cause 2 of 2: the KMS authorization failure relayed by StartExecution names the IAM role ARN instead of the assumed-role session ARN of the principal that actually made the call.
AWS: recorded behavior only for the wording's principal field. The refusal is correct in status and code (HTTP 400, KmsAccessDeniedException) and its message's fixed text is "User: <principal> is not authorized to perform: kms:GenerateDataKey on resource: <key ARN> with an explicit deny in a resource-based policy (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)"; <principal> is the caller's STS assumed-role ARN, arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>, and <key ARN>, <id>, <n> likewise come from the request/state and the inner call.
Emulator: the same message renders <principal> as the IAM role ARN with its path, arn:aws:iam::<account>:role/<path>/<role-name> — e.g. 'arn:aws:iam::000000000000:role/sim2real/sim2real-behav-sfnkms-<id>' where 'arn:aws:sts::000000000000:assumed-role/sim2real-behav-sfnkms-<id>/<session>' was required.
Why: a client that parses the denied principal out of a KMS denial (to attribute the failure to a session, or to generate the matching policy statement) builds the wrong principal; the related issue 20261106T134622Z-lambda-sqs-stepfunctions-an-accessdenied-names-the-role.md reports the same rendering under a different service and does not cover this check.
Fix: the principal-rendering helper used when the KMS authorization failure is serialized into the Step Functions error message (sts assumed-role session identity, not the IAM role identity).
Traces: kms__stepfunctions.start-execution-cmk-key-denies-or-disabled-create-nothing.

## Evidence
Cause 1 — expected: StartExecution never starts against the mismatched trust. Actual: "StartExecution unexpectedly started 'arn:aws:states:<region>:<account>:execution:<machine>:<uuid>' against the mismatched trust" (mismatched aws:SourceArn) and "StartExecution unexpectedly succeeded against the mismatched trust" (mismatched aws:SourceAccount) — 200 in both cases.

Cause 2 — expected message 'User: arn:aws:sts::<account>:assumed-role/<role-name>/<session> is not authorized to perform: kms:GenerateDataKey on resource: <key ARN> with an explicit deny in a resource-based policy (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)'; got 'User: arn:aws:iam::<account>:role/sim2real/<role-name> is not authorized to perform: kms:GenerateDataKey on resource: <key ARN> with an explicit deny in a resource-based policy (Service: Kms, Status Code: 400, Request ID: ..., SDK Attempt Count: 1)'. Status (400) and code (KmsAccessDeniedException) match; only the principal rendering diverges.

## Steps to Reproduce
1. Cause 1: create an execution role trusting states.amazonaws.com with Condition StringEquals aws:SourceArn = some OTHER state machine ARN (and, in the second leg, aws:SourceAccount = another account). Create a CMK-encrypted state machine using that role and call StartExecution — AWS never produces a running execution; the emulator returns 200 with an executionArn.
2. Cause 2: create a CMK whose key policy explicitly denies kms:GenerateDataKey to the execution role, start an execution on a state machine using that role, and read Error.Message of the KmsAccessDeniedException — the principal must be the arn:aws:sts::<account>:assumed-role/<role-name>/<session> form.
