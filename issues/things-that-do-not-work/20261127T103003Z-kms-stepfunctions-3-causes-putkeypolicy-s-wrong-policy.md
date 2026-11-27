Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__stepfunctions.cmk-second-key-policy-name-rejected-state-machine-follows-replaced-default kms__stepfunctions.cmk-state-machine-data-keys-requested-as-aes-256-with-arn-context kms__stepfunctions.create-activity-kms-key-id-length-bounds-rejected
Issue-Key: kms__stepfunctions.cmk-second-key-policy-name-rejected-state-machine-follows-replaced-default|run#22|AssertionError|assert got == ("ValidationException", _BAD_NAME_MSG), (
Issue-Key: kms__stepfunctions.cmk-state-machine-data-keys-requested-as-aes-256-with-arn-context|run#28|AssertionError|raise AssertionError(
Issue-Key: kms__stepfunctions.create-activity-kms-key-id-length-bounds-rejected|run#18|AssertionError|assert got[case] == want[case], (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__stepfunctions: 3 causes — PutKeyPolicy's wrong policy name answers NotFoundException, no GenerateDataKey by states.amazonaws.com reaches CloudTrail, and CreateActivity's length refusal elides the value

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateActivity

## Description
Three distinct root causes, each grounded in a failing behavioral check.

Cause 1 of 3: PutKeyPolicy with a policy name other than 'default' is refused NotFoundException with emulator-authored prose.
  AWS: recorded behavior only for the message; KMS supports exactly one key policy name ('default'). AWS refuses a second name with ('ValidationException', 'Invalid policy name') — fixed text, no variable parts.
  Emulator: it answers ('NotFoundException', 'No such policy exists: workflow. The only valid key policy name for <key arn> is "default".') — the wrong error class and a message AWS never sends.
  Why: a client branching on the error class treats a malformed request as a missing resource (and may retry against a different key), and anything matching the text reads emulator prose.
  Fix: the KMS PutKeyPolicy handler's PolicyName validation (error class and message).
  Traces: kms__stepfunctions.cmk-second-key-policy-name-rejected-state-machine-follows-replaced-default.

Cause 2 of 3: the GenerateDataKey Step Functions makes for a CMK-encrypted state machine is never recorded in CloudTrail.
  AWS: recorded behavior only. Running an execution on a state machine encrypted with a customer managed key produces a GenerateDataKey event in CloudTrail for that key, with the states.amazonaws.com service principal as the invoker and the state machine ARN in the encryption context, delivered within the lookup window.
  Emulator: no such record appears — 'no GenerateDataKey record sent by states.amazonaws.com for the state machine's key appeared in CloudTrail within 30s'.
  Why: a client auditing which service used its CMK, and under which encryption context, cannot see Step Functions' key usage at all.
  Fix: the KMS data-plane audit emission for service-principal calls and the CloudTrail LookupEvents store (the GenerateDataKey the stepfunctions execution path performs must be recorded with its invoker and encryption context).
  Traces: kms__stepfunctions.cmk-state-machine-data-keys-requested-as-aes-256-with-arn-context.

Cause 3 of 3: CreateActivity's kmsKeyId length refusal elides the offending value.
  AWS: recorded behavior only for the message shape; the member is bounded at 1..2048. A 2049-character kmsKeyId is refused ('ValidationException', "1 validation error detected: Value '<the whole 2049-character value>' at 'encryptionConfiguration.kmsKeyId' failed to satisfy constraint: Member must have length less than or equal to 2048") — the value appears in full — and an empty one with the 'greater than or equal to 1' message; neither activity is created.
  Emulator: the code and the constraint sentence match, but the echoed value is truncated at roughly 256 characters with an elision marker appended ('…kkkk… [2049 bytes]').
  Why: a client or operator matching the refusal, or recovering the rejected value from it, reads a message AWS never sends.
  Fix: the shared validation-message formatter used by the stepfunctions handlers — echo the offending value verbatim with no length budget.
  Traces: kms__stepfunctions.create-activity-kms-key-id-length-bounds-rejected.

## Evidence
kms__stepfunctions.cmk-second-key-policy-name-rejected-state-machine-follows-replaced-default — AssertionError: PutKeyPolicy under a second name answered ('NotFoundException', 'No such policy exists: workflow. The only valid key policy name for <key arn> is "default".'), expected ('ValidationException', 'Invalid policy name').

kms__stepfunctions.cmk-state-machine-data-keys-requested-as-aes-256-with-arn-context — AssertionError: no GenerateDataKey record sent by states.amazonaws.com for the state machine's key appeared in CloudTrail within 30s.

kms__stepfunctions.create-activity-kms-key-id-length-bounds-rejected — AssertionError: CreateActivity with a 2049-character kmsKeyId was refused with 'ValidationException': "1 validation error detected: Value 'alias/kkkk…[2049 bytes]' at 'encryptionConfiguration.kmsKeyId' failed to satisfy constraint: Member must have length less than or equal to 2048" — the value is elided where AWS echoes it in full.

## Steps to Reproduce
1. Cause 1: kms.put_key_policy(KeyId=<key>, PolicyName='workflow', Policy=<valid policy>) -> expect ('ValidationException', 'Invalid policy name'); observe NotFoundException with the emulator sentence.
2. Cause 2: create a state machine with encryptionConfiguration CUSTOMER_MANAGED_KMS_KEY, start an execution, then cloudtrail.lookup_events for GenerateDataKey on that key -> expect a record whose invoker is states.amazonaws.com and whose encryption context carries the state machine ARN; observe none.
3. Cause 3: sfn.create_activity(name=..., encryptionConfiguration={'type':'CUSTOMER_MANAGED_KMS_KEY','kmsKeyId':'alias/'+'k'*2043}) -> expect the ValidationException message echoing the whole value; observe the elided value.
