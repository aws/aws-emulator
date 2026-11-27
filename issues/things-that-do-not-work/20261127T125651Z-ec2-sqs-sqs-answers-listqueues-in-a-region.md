Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__sqs.sqs-list-queues-in-a-not-opted-in-region-rejected
Issue-Key: ec2__sqs.sqs-list-queues-in-a-not-opted-in-region-rejected|run#11|AssertionError|raise AssertionError(f"ListQueues in the not-opted-in Region {away!r} answered {answer!r}")
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__sqs: SQS answers ListQueues in a Region the account has not opted in to instead of refusing with UnrecognizedClientException

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListQueues

## Description
Cause 1 of 1: an SQS request addressed to a Region the account has not opted into is answered rather than refused.

AWS: recorded behavior only (the opt-in rule is an account/credential rule, not a modelled SQS error). A request signed for a Region whose opt-in-status is `not-opted-in` - exactly the Regions EC2's DescribeRegions(AllRegions=true, Filter opt-in-status=not-opted-in) lists - is refused with HTTP 403, error code `UnrecognizedClientException` and the fixed message `The security token included in the request is invalid.`

Emulator: ListQueues against that Region answers HTTP 200 with an empty body - `{'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}` (no QueueUrls member, i.e. an empty listing).

Why: a client sweeping Regions to inventory queues, or probing which Regions it may call, reads every not-opted-in Region as an enabled, simply-empty Region, and cannot distinguish "Region disabled for this account" from "Region enabled and empty".

Fix: the SQS request path's Region admission - refuse a request whose signed Region is not opted in with 403 UnrecognizedClientException before dispatch.

Note: the same opt-in admission defect is already filed for ECS (open: 20261127T105620Z-ec2-ecs-ecs-answers-requests-sent-to-a.md); that issue does not cover SQS.

Traces: ec2__sqs.sqs-list-queues-in-a-not-opted-in-region-rejected.

## Evidence
Expected (AWS): ClientError with Error.Code 'UnrecognizedClientException' and Error.Message 'The security token included in the request is invalid.' (HTTP 403).

Actual (emulator): "ListQueues in the not-opted-in Region 'af-south-1' answered {'ResponseMetadata': {'RequestId': 'be3fcd1b-58d1-48c7-8247-021bc54e721e', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}".

## Steps to Reproduce
1. DescribeRegions(AllRegions=True, Filters=[{'Name':'opt-in-status','Values':['not-opted-in']}]) and take one Region from the result.
2. Build an sqs client for that Region and call ListQueues.
3. Observe HTTP 200 with an empty listing instead of a 403 UnrecognizedClientException.
