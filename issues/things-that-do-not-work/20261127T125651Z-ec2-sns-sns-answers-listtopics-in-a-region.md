Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__sns.sns-list-topics-in-a-not-opted-in-region-rejected
Issue-Key: ec2__sns.sns-list-topics-in-a-not-opted-in-region-rejected|run#12|AssertionError|raise AssertionError(f"ListTopics in the not-opted-in Region {target!r} returned {listed!r}")
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__sns: SNS answers ListTopics in a Region the account has not opted in to instead of refusing with UnrecognizedClientException

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTopics

## Description
Cause 1 of 1: an SNS request addressed to a Region the account has not opted into is answered rather than refused.

AWS: recorded behavior only (the opt-in rule is an account/credential rule, not a modelled SNS error). A request signed for a Region whose opt-in-status is `not-opted-in` - exactly the Regions EC2's DescribeRegions(AllRegions=true, Filter opt-in-status=not-opted-in) lists - is refused with HTTP 403, error code `UnrecognizedClientException` and the fixed message `The security token included in the request is invalid.`

Emulator: ListTopics against that Region answers HTTP 200 with an empty Topics list - `{'Topics': [], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', ...}}}`.

Why: a client sweeping Regions to inventory topics, or probing which Regions it may call, reads every not-opted-in Region as an enabled, simply-empty Region, and cannot distinguish "Region disabled for this account" from "Region enabled and empty".

Fix: the SNS request path's Region admission - refuse a request whose signed Region is not opted in with 403 UnrecognizedClientException before dispatch.

Note: the same opt-in admission defect is already filed for ECS (open: 20261127T105620Z-ec2-ecs-ecs-answers-requests-sent-to-a.md); that issue does not cover SNS.

Traces: ec2__sns.sns-list-topics-in-a-not-opted-in-region-rejected.

## Evidence
Expected (AWS): ClientError with Error.Code 'UnrecognizedClientException' and Error.Message 'The security token included in the request is invalid.' (HTTP 403).

Actual (emulator): "ListTopics in the not-opted-in Region 'ap-southeast-7' returned {'Topics': [], 'ResponseMetadata': {'RequestId': '2efb23f9-3c26-456a-8636-ffff1daf1cf4', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '240', ...}}}".

## Steps to Reproduce
1. DescribeRegions(AllRegions=True, Filters=[{'Name':'opt-in-status','Values':['not-opted-in']}]) and take one Region from the result.
2. Build an sns client for that Region and call ListTopics.
3. Observe HTTP 200 with an empty Topics list instead of a 403 UnrecognizedClientException.
