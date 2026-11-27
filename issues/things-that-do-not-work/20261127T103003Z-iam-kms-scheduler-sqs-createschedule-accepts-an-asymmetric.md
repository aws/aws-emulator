Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms__scheduler__sqs.create-schedule-asymmetric-kms-key-rejected iam__kms__scheduler__sqs.create-schedule-disabled-kms-key-rejected-until-enabled
Issue-Key: iam__kms__scheduler__sqs.create-schedule-asymmetric-kms-key-rejected|_refusal#15|AssertionError|raise AssertionError(f"expected a refusal but the call succeeded: {answer!r}")
Issue-Key: iam__kms__scheduler__sqs.create-schedule-disabled-kms-key-rejected-until-enabled|_refused_with#20|AssertionError|raise AssertionError(f"the call was accepted, a refusal was expected: {reply!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# iam__kms__scheduler__sqs: CreateSchedule accepts an asymmetric KMS key and a disabled KMS key

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Two distinct root causes, each grounded in a failing behavioral check. Both are the KmsKeyArn admission check on CreateSchedule, but they are separate rules (key spec vs key state) with separate refusals.

Cause 1 of 2: an asymmetric key is accepted as a schedule's KmsKeyArn.
  AWS: recorded behavior only (the scheduler model types KmsKeyArn as an ARN string). EventBridge Scheduler requires symmetric keys, so CreateSchedule naming an RSA_2048 key is refused 400 ValidationException whose message begins 'The KMS key ' and ends ' you provided is an asymmetric key. EventBridge Scheduler requires symmetric keys to encrypt and decrypt data.' with the key ARN from the request in between; no schedule is created, so GetSchedule answers ResourceNotFoundException 'Schedule <name> does not exist.'
  Emulator: CreateSchedule returns 200 with a ScheduleArn — 'expected a refusal but the call succeeded: {... 'ScheduleArn': 'arn:aws:scheduler:...:schedule/default/sim2real-xsvc-asym-no-<id>'}'.
  Why: a client gets a schedule bound to a key that can never wrap a data key; the configuration error AWS reports synchronously never surfaces.
  Fix: the scheduler CreateSchedule handler's KmsKeyArn validation — resolve the key in KMS and require a symmetric encryption key spec.
  Traces: iam__kms__scheduler__sqs.create-schedule-asymmetric-kms-key-rejected.

Cause 2 of 2: a disabled key is accepted as a schedule's KmsKeyArn.
  AWS: recorded behavior only. CreateSchedule naming a key whose state is Disabled is refused 400 ValidationException with a message naming that key, and no schedule is created (GetSchedule -> ResourceNotFoundException 'Schedule <name> does not exist.'); once the key is enabled, the identical request succeeds.
  Emulator: CreateSchedule returns 200 with a ScheduleArn while the key is disabled.
  Why: a client is allowed to create a schedule whose key cannot be used, so the first invocation — not the create — fails, and the until-enabled contract is unobservable.
  Fix: the scheduler CreateSchedule handler's KmsKeyArn validation — check key state in KMS before accepting.
  Traces: iam__kms__scheduler__sqs.create-schedule-disabled-kms-key-rejected-until-enabled.

## Evidence
iam__kms__scheduler__sqs.create-schedule-asymmetric-kms-key-rejected — AssertionError: expected a refusal but the call succeeded: {'HTTPStatusCode': 200, 'ScheduleArn': 'arn:aws:scheduler:us-west-2:<account>:schedule/default/sim2real-xsvc-asym-no-<id>'}. Expected ValidationException 'The KMS key <key arn> you provided is an asymmetric key. EventBridge Scheduler requires symmetric keys to encrypt and decrypt data.'

iam__kms__scheduler__sqs.create-schedule-disabled-kms-key-rejected-until-enabled — AssertionError: the call was accepted, a refusal was expected: {'HTTPStatusCode': 200, 'ScheduleArn': 'arn:aws:scheduler:us-west-2:<account>:schedule/default/sim2real-xsvc-dis-sch-<id>'}. Expected a ValidationException naming the disabled key, and GetSchedule answering ResourceNotFoundException.

## Steps to Reproduce
1. Cause 1: create an RSA_2048 key; scheduler.create_schedule(Name=..., KmsKeyArn=<rsa arn>, Target=<sqs queue>, FlexibleTimeWindow=..., ScheduleExpression=...) -> expect 400 ValidationException about the asymmetric key; observe 200.
2. Cause 2: create a symmetric key and kms.disable_key it; issue the same CreateSchedule with that key -> expect 400 ValidationException naming the key and GetSchedule -> ResourceNotFoundException; observe 200. Enable the key and repeat -> accepted.
