Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__kms__sns__sqs.test-repository-triggers-to-a-topic-whose-key-excludes-codecommit-rejected
Issue-Key: codecommit__kms__sns__sqs.test-repository-triggers-to-a-topic-whose-key-excludes-codecommit-rejected|run#17|AssertionError|assert tested["successfulExecutions"] == [granted["name"]], (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# codecommit__kms__sns__sqs: TestRepositoryTriggers reports success for a trigger whose SNS topic key excludes CodeCommit

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestRepositoryTriggers

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: TestRepositoryTriggers does not attempt (or does not authorize) the publish to the trigger's SNS target, so a topic encrypted under a CMK whose key policy does not allow codecommit.amazonaws.com is reported as a successful trigger.
  AWS: recorded behavior only. TestRepositoryTriggers actually sends a test notification per trigger and reports the outcome per trigger: successfulExecutions names only the trigger whose topic CodeCommit can publish to, and failedExecutions names the trigger whose topic is encrypted with a key that does not grant CodeCommit kms:GenerateDataKey/kms:Decrypt, with the failure message for that trigger.
  Emulator: both triggers are reported successful and failedExecutions is empty — successfulExecutions = ['<granted trigger>', '<excluded trigger>'], failed = [].
  Why: a client using TestRepositoryTriggers to validate notification wiring before relying on it is told a mis-keyed topic works; the real repository events would silently never be delivered.
  Fix: the TestRepositoryTriggers handler — perform the per-trigger publish through the SNS/KMS authorization path and project the per-trigger outcome into successfulExecutions/failedExecutions.
  Traces: codecommit__kms__sns__sqs.test-repository-triggers-to-a-topic-whose-key-excludes-codecommit-rejected.

## Evidence
codecommit__kms__sns__sqs.test-repository-triggers-to-a-topic-whose-key-excludes-codecommit-rejected —
AssertionError: expected only the trigger whose key grants CodeCommit to succeed, got ['sim2real-xsvc-gra-700249ee53aa', 'sim2real-xsvc-exc-a0f82bbf2e24'] (failed: []).

## Steps to Reproduce
1. Create two symmetric CMKs: one whose key policy grants codecommit.amazonaws.com kms:GenerateDataKey*/kms:Decrypt, one that does not.
2. Create two SNS topics, each with KmsMasterKeyId set to one of the keys, and subscribe an SQS queue to each.
3. Create a CodeCommit repository with one trigger per topic and call TestRepositoryTriggers.
4. Observe both triggers in successfulExecutions with an empty failedExecutions; AWS puts the trigger on the excluding key in failedExecutions.
