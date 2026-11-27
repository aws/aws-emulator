Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__stepfunctions.sfn-list-state-machines-in-a-not-opted-in-region-rejected
Issue-Key: ec2__iam__stepfunctions.sfn-list-state-machines-in-a-not-opted-in-region-rejected|run#16|AssertionError|raise AssertionError(f"ListStateMachines in the not-opted-in Region {remote_region!r} answered {page!r}")
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__iam__stepfunctions: Step Functions answers ListStateMachines in a Region the account has not opted in to instead of refusing with UnrecognizedClientException

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStateMachines

## Description
Cause 1 of 1: a Step Functions request addressed to a Region the account has not opted into is answered rather than refused.

AWS: recorded behavior only (the opt-in rule is an account/credential rule, not a modelled Step Functions error). A request signed for a Region whose opt-in-status is `not-opted-in` - exactly the Regions EC2's DescribeRegions(AllRegions=true, Filter opt-in-status=not-opted-in) lists - is refused with HTTP 403, error code `UnrecognizedClientException` and the fixed message `The security token included in the request is invalid.`

Emulator: ListStateMachines against that Region answers HTTP 200 with an empty listing - `{'stateMachines': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. (`af-south-1` is the Region this run picked; the requirement is the 403 refusal for any not-opted-in Region, not that Region specifically.)

Why: a client sweeping Regions to inventory state machines - or probing which Regions it is entitled to call - reads every not-opted-in Region as an enabled, simply-empty Region. It therefore reports no error where AWS blocks the call outright, and cannot distinguish "Region disabled for this account" from "Region enabled and empty".

Fix: the Step Functions request path's Region admission - refuse a request whose signed Region is not opted in with 403 UnrecognizedClientException before dispatch.

Note: the same opt-in admission defect is already filed for ECS (open: 20261127T105620Z-ec2-ecs-ecs-answers-requests-sent-to-a.md); that issue does not cover Step Functions.

Traces: ec2__iam__stepfunctions.sfn-list-state-machines-in-a-not-opted-in-region-rejected.

## Evidence
Expected (AWS): ClientError with Error.Code 'UnrecognizedClientException' and Error.Message 'The security token included in the request is invalid.' (HTTP 403).

Actual (emulator): "ListStateMachines in the not-opted-in Region 'af-south-1' answered {'stateMachines': [], 'ResponseMetadata': {'RequestId': '22a4b93d-f57a-411a-9798-ba09f1d085ea', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', ...}}}".

## Steps to Reproduce
1. DescribeRegions(AllRegions=True, Filters=[{'Name':'opt-in-status','Values':['not-opted-in']}]) and take one Region from the result.
2. Build a stepfunctions client for that Region and call ListStateMachines.
3. Observe HTTP 200 with an empty list instead of a 403 UnrecognizedClientException.
