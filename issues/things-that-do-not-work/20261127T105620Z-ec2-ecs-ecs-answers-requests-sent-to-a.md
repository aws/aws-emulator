Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.ecs-list-clusters-in-a-not-opted-in-region-rejected ec2__ecs.ecs-list-task-definitions-in-a-not-opted-in-region-rejected
Issue-Key: ec2__ecs.ecs-list-clusters-in-a-not-opted-in-region-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call answered instead of being refused: {answer!r}")
Issue-Key: ec2__ecs.ecs-list-task-definitions-in-a-not-opted-in-region-rejected|_refusal#13|AssertionError|raise AssertionError(f"the call answered instead of being refused: {answer!r}")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# ec2__ecs: ECS answers requests sent to a Region the account has not opted in to instead of refusing them with UnrecognizedClientException

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
Cause 1 of 1: ECS serves read calls addressed to a non-opted-in Region.
AWS: a request signed for a Region the account has not opted in to (the Regions DescribeRegions(AllRegions=True, Filters=[opt-in-status=not-opted-in]) reports) never reaches the service's data: it is refused with HTTP 400 and the error code `UnrecognizedClientException` and the message `The security token included in the request is invalid.` — entirely fixed text, no variable part. The same call in an enabled Region answers normally (recorded behavior; the ECS Smithy model carries no per-Region member, so this is an endpoint/credential-scope rule, not an operation parameter).
Emulator: answers HTTP 200 with an empty result in the closed Region — `{'clusterArns': []}` for ListClusters and `{'taskDefinitionArns': []}` for ListTaskDefinitions. Both operations show the same defect, so the Region gate is missing for the service as a whole rather than in one handler.
Why: a client treats “200 with an empty list” as “this Region is enabled and empty”. Region-sweep tooling (inventory, tag crawlers, cost/cleanup jobs) silently records closed Regions as empty instead of skipping them on the refusal, and code that handles UnrecognizedClientException to detect an unavailable Region never takes that branch.
Why it is not a listing bug: the home-Region listing in the same check is correct (the created cluster is returned), so the divergence is purely the missing Region/credential-scope refusal.
Fix: Region/credential-scope admission for the ECS service — the request-routing and signing-scope check ahead of the operation handlers (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/, applied to every operation, not just the two listings).
Traces: checks ec2__ecs.ecs-list-clusters-in-a-not-opted-in-region-rejected, ec2__ecs.ecs-list-task-definitions-in-a-not-opted-in-region-rejected.

## Evidence
Expected: ClientError with Code 'UnrecognizedClientException' and Message 'The security token included in the request is invalid.' (HTTP 400).
Actual (ListClusters in the not-opted-in Region): AssertionError: the call answered instead of being refused: {'clusterArns': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.
Actual (ListTaskDefinitions in the not-opted-in Region): AssertionError: the call answered instead of being refused: {'taskDefinitionArns': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.
Control in the same checks: the home-Region ListClusters does include the cluster created there, so only the closed-Region refusal is missing.

## Steps to Reproduce
1. ec2.DescribeRegions(AllRegions=True, Filters=[{'Name':'opt-in-status','Values':['not-opted-in']}]) and take the first Region reported closed.
2. Create a cluster in the check's own Region and confirm ListClusters there returns its ARN.
3. Call ecs.ListClusters with a client bound to the closed Region — expect UnrecognizedClientException, observe a 200 with clusterArns [].
4. Repeat with ecs.ListTaskDefinitions in the closed Region (check ec2__ecs.ecs-list-task-definitions-in-a-not-opted-in-region-rejected).
