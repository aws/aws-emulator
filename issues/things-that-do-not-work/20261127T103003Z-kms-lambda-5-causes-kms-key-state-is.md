Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda.create-function-with-a-kms-key-pending-deletion-rejected-until-deletion-is-cancelled kms__lambda.durable-config-update-naming-a-disabled-kms-key-rejected kms__lambda.durable-config-update-omitting-kms-key-arn-keeps-the-key-and-an-empty-one-clears-it kms__lambda.durable-execution-data-decrypts-for-a-caller-granted-kms-decrypt kms__lambda.durable-execution-data-refused-to-a-caller-without-kms-decrypt
Issue-Key: kms__lambda.create-function-with-a-kms-key-pending-deletion-rejected-until-deletion-is-cancelled|_refusal#26|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: kms__lambda.durable-config-update-naming-a-disabled-kms-key-rejected|_refusal#33|AssertionError|raise AssertionError(f"expected the update to be refused, but it answered members {sorted(got)!r}")
Issue-Key: kms__lambda.durable-config-update-omitting-kms-key-arn-keeps-the-key-and-an-empty-one-clears-it|run#35|AssertionError|assert "KMSKeyArn" not in cleared["DurableConfig"], (
Issue-Key: kms__lambda.durable-execution-data-decrypts-for-a-caller-granted-kms-decrypt|_untyped#25|KeyError|tag, value = node["t"], node["v"]
Issue-Key: kms__lambda.durable-execution-data-refused-to-a-caller-without-kms-decrypt|_refusal#24|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {sorted(got)!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__lambda: 5 causes — KMS key state is not checked on CreateFunction/DurableConfig updates, an empty KMSKeyArn is stored, step results are not typed envelopes, and durable execution data is served without kms:Decrypt

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionConfiguration

## Description
Five distinct root causes, each grounded in a failing behavioral check.

Cause 1 of 5: CreateFunction accepts a KMSKeyArn whose key is pending deletion.
  AWS: recorded behavior only (the Lambda model types KMSKeyArn as an ARN string). CreateFunction resolves the key and refuses a key in PendingDeletion with a 400 client error naming it, creating no function; once ScheduleKeyDeletion is cancelled the identical create succeeds.
  Emulator: the create returns 201 with the full function configuration carrying 'KMSKeyArn': '<key pending deletion>'.
  Why: a function is created against a key that is being destroyed; the environment-variable encryption AWS refuses up front fails later instead.
  Fix: the lambda CreateFunction handler's KMSKeyArn validation (resolve key state in KMS before storing).
  Traces: kms__lambda.create-function-with-a-kms-key-pending-deletion-rejected-until-deletion-is-cancelled.

Cause 2 of 5: UpdateFunctionConfiguration accepts a DurableConfig.KMSKeyArn whose key is disabled.
  AWS: recorded behavior only. The update is refused 400 InvalidParameterValueException with the disabled-key message naming the key, and the function's DurableConfig keeps no KMSKeyArn.
  Emulator: the update returns 200 with a full configuration response (members Architectures ... Version including DurableConfig), storing the disabled key.
  Why: a durable function is configured against a key that cannot wrap a data key; the refusal AWS gives at configuration time never happens.
  Fix: the UpdateFunctionConfiguration handler's DurableConfig validation (key state check in KMS).
  Traces: kms__lambda.durable-config-update-naming-a-disabled-kms-key-rejected.

Cause 3 of 5: an empty DurableConfig.KMSKeyArn is stored as an empty string instead of clearing the key.
  AWS: recorded behavior only. An update that omits KMSKeyArn keeps the key the function already has; an update that passes KMSKeyArn='' clears it, and the function's DurableConfig afterwards reports no KMSKeyArn member at all.
  Emulator: after the empty update the response's DurableConfig is {'KMSKeyArn': '', 'RetentionPeriodInDays': 1, 'ExecutionTimeout': 300} — the empty string is persisted and projected.
  Why: a client clearing a customer key reads a configuration claiming a key whose ARN is the empty string; it cannot tell that the function is back on the AWS owned key, and cannot safely retire the old key.
  Fix: the UpdateFunctionConfiguration handler's DurableConfig merge — treat an empty KMSKeyArn as a clear, and omit the member when no key is set.
  Traces: kms__lambda.durable-config-update-omitting-kms-key-arn-keeps-the-key-and-an-empty-one-clears-it.

Cause 4 of 5: a durable execution's StepSucceeded result is not the typed-value envelope AWS records.
  AWS: recorded behavior only. In the execution history, StepSucceededDetails.Result.Payload is JSON in the durable SDK's typed-value form — a node {'t': <tag>, 'v': <value>} where tag 'm' is a map of typed values and 'i'/'s' are scalars — which decodes to the value the handler returned.
  Emulator: the payload has no 't' member (KeyError: 't'), i.e. the plain handler result is written instead of the typed envelope.
  Why: a client (or the durable SDK itself) decoding step checkpoints from the history crashes on the first node; execution history is unreadable for any consumer that follows AWS's format.
  Fix: the durable-execution history serializer in the lambda durable-execution handlers (StepSucceeded result payload encoding).
  Traces: kms__lambda.durable-execution-data-decrypts-for-a-caller-granted-kms-decrypt.

Cause 5 of 5: durable execution data is served to a caller with no kms:Decrypt on the function's key.
  AWS: recorded behavior only. Reading a durable execution's data requires decrypting it under the function's customer key, so a caller that does not hold kms:Decrypt on that key is refused (an access-denied class refusal) and no payloads are returned; the companion check shows the same read succeeding once the caller is granted kms:Decrypt.
  Emulator: the read returns the full execution — members ['DurableConfig', 'DurableExecutionArn', 'DurableExecutionName', 'EndTimestamp', 'ExecutionDataIncluded', 'FunctionArn', 'InputPayload', 'Result', 'StartTimestamp', 'Status', 'Version'] — including the decrypted input and result.
  Why: encrypted execution data (inputs and results, often sensitive) is readable by principals AWS denies; the customer key provides no isolation.
  Fix: the durable-execution read handler's decryption path — authorize the caller's kms:Decrypt on the function's key before returning execution data.
  Traces: kms__lambda.durable-execution-data-refused-to-a-caller-without-kms-decrypt.

## Evidence
kms__lambda.create-function-with-a-kms-key-pending-deletion-rejected-until-deletion-is-cancelled — AssertionError: expected a refusal, the call succeeded: {'HTTPStatusCode': 201, ..., 'KMSKeyArn': 'arn:aws:kms:us-west-2:<account>:key/1990a323-...', 'State': 'Pending', ...}.

kms__lambda.durable-config-update-naming-a-disabled-kms-key-rejected — AssertionError: expected the update to be refused, but it answered members ['Architectures', ..., 'DurableConfig', ..., 'Version']. Expected ('InvalidParameterValueException', <the disabled-key message naming the key>) and no KMSKeyArn left in DurableConfig.

kms__lambda.durable-config-update-omitting-kms-key-arn-keeps-the-key-and-an-empty-one-clears-it — AssertionError: an update with an empty KMSKeyArn answered DurableConfig members {'KMSKeyArn': '', 'RetentionPeriodInDays': 1, 'ExecutionTimeout': 300}.

kms__lambda.durable-execution-data-decrypts-for-a-caller-granted-kms-decrypt — File "check_...py", line 89, in _untyped -> tag, value = node['t'], node['v'] -> KeyError: 't' (decoding StepSucceededDetails.Result.Payload).

kms__lambda.durable-execution-data-refused-to-a-caller-without-kms-decrypt — AssertionError: expected a refusal, but the call succeeded: ['DurableConfig', 'DurableExecutionArn', 'DurableExecutionName', 'EndTimestamp', 'ExecutionDataIncluded', 'FunctionArn', 'InputPayload', 'ResponseMetadata', 'Result', 'StartTimestamp', 'Status', 'Version'].

## Steps to Reproduce
1. Cause 1: kms.schedule_key_deletion on a symmetric key, then lambda.create_function(..., KMSKeyArn=<that key>) -> expect a 400 refusal; observe 201. Cancel the deletion and repeat -> accepted.
2. Cause 2: kms.disable_key, then lambda.update_function_configuration(FunctionName=..., DurableConfig={..., 'KMSKeyArn': <disabled key>}) -> expect InvalidParameterValueException; observe 200.
3. Cause 3: set a key in DurableConfig, then update with DurableConfig={..., 'KMSKeyArn': ''} -> expect no KMSKeyArn member afterwards; observe 'KMSKeyArn': ''.
4. Cause 4: run a durable execution with one step and read its history -> StepSucceededDetails.Result.Payload must be a {'t','v'} typed envelope; observe plain JSON.
5. Cause 5: as a role without kms:Decrypt on the function's key, read the durable execution's data -> expect an access-denied refusal; observe the decrypted execution.
