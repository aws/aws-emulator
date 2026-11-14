Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found|_poll_until#7|AssertionError|raise AssertionError(
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# Cloud Control GetResource never resolves an AWS::SQS::Queue created through the native SQS API

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: Cloud Control's resource index only contains resources Cloud Control itself created, so a queue created with the native SQS CreateQueue API is invisible to GetResource and its post-delete NotFound semantics can never be graded.

AWS: Cloud Control API is a uniform façade over the resource providers, not a side index. GetResource(TypeName='AWS::SQS::Queue', Identifier=<queue URL/name>) resolves ANY queue that exists in the account/region however it was created, and returns ResourceNotFoundException (HTTP 404, message naming the type and identifier) only once the queue is gone. Smithy: GetResource takes TypeName+Identifier and declares ResourceNotFoundException; nothing in the model scopes it to Cloud-Control-created resources. Recorded behavior: the check's natively created queue resolves on real AWS, then stops resolving after the native DeleteQueue.
Emulator: GetResource never resolves the natively created queue at all — 'waited 30.0s for Cloud Control GetResource to resolve <name>, a queue it never created and it never happened; last observation None'. The delete half of the check is therefore unreachable.
Why: a client that provisions with CloudFormation/SQS and reads or drifts-checks through Cloud Control sees an empty account; a resource that plainly exists reads as absent, and ResourceNotFound stops being a trustworthy signal of deletion.
Fix: the GetResource handler and the Cloud Control resource resolution layer for AWS::SQS::Queue — resolve through the owning service's state instead of a Cloud-Control-only registry.
Traces: cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found. Related but NOT covering this check: open issue 20261107T181315Z-cloudcontrol-sqs-getresource-listresources-resolve-only-cloud-control.md (filed for a sibling check).

## Evidence
[mismatch] cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found
AssertionError: waited 30.0s for Cloud Control GetResource to resolve 'sim2real-behav-ccnat-4b1b4e4bfe44', a queue it never created and it never happened; last observation None

Expected: GetResource resolves the queue while it exists, then answers ResourceNotFoundException after the native DeleteQueue.
Actual: GetResource never resolves it (no observation at all within 30s).

## Steps to Reproduce
1. sqs:CreateQueue a queue natively (no Cloud Control involvement).
2. cloudcontrol:GetResource(TypeName='AWS::SQS::Queue', Identifier=<that queue>) — expect the resource description; the emulator never resolves it.
3. sqs:DeleteQueue, then GetResource again — expect ResourceNotFoundException.
