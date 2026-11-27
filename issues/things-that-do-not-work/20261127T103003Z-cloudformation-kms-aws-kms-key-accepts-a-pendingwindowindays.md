Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kms.cfn-kms-key-pending-window-out-of-range-rejected-at-create
Issue-Key: cloudformation__kms.cfn-kms-key-pending-window-out-of-range-rejected-at-create|run#8|AssertionError|assert stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# cloudformation__kms: AWS::KMS::Key accepts a PendingWindowInDays outside 7-30, so the stack CREATE_COMPLETEs instead of rolling back

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: a template whose AWS::KMS::Key carries PendingWindowInDays=40 is provisioned successfully.
  AWS: Smithy (KMS): ScheduleKeyDeletion.PendingWindowInDays targets PendingWindowInDaysType with @range min 7 max 30, and the AWS::KMS::Key resource documents the same 7-30 range for PendingWindowInDays. A value of 40 is outside it, so the resource fails to create and the stack rolls back (CREATE_FAILED -> ROLLBACK_COMPLETE) with the resource's failure reason naming the out-of-range property; no key is left behind.
  Emulator: the stack reaches 'CREATE_COMPLETE' — the property is accepted and the key is created.
  Why: a client relying on CloudFormation to reject an out-of-range deletion window at deploy time gets a green stack that real AWS would roll back, and the key's deletion window silently differs from what the template asked for.
  Fix: the AWS::KMS::Key resource provider's property validation in the cloudformation resource handler (PendingWindowInDays range check, surfacing a resource failure that drives the rollback).
  Traces: cloudformation__kms.cfn-kms-key-pending-window-out-of-range-rejected-at-create.

## Evidence
cloudformation__kms.cfn-kms-key-pending-window-out-of-range-rejected-at-create —
AssertionError: a template with PendingWindowInDays=40 ended 'CREATE_COMPLETE'; the property's documented range is 7-30, so the key must fail to create and the stack roll back.

## Steps to Reproduce
1. CreateStack with a template containing a single AWS::KMS::Key whose Properties include PendingWindowInDays: 40 and a minimal KeyPolicy.
2. Wait for the stack to settle.
3. Observe StackStatus 'CREATE_COMPLETE'; AWS leaves the stack in ROLLBACK_COMPLETE with the Key resource CREATE_FAILED for the out-of-range PendingWindowInDays.
