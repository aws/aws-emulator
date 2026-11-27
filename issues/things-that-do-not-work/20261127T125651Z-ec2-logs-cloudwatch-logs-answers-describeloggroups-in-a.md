Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__logs.logs-describe-log-groups-in-a-not-opted-in-region-rejected
Issue-Key: ec2__logs.logs-describe-log-groups-in-a-not-opted-in-region-rejected|run#10|AssertionError|raise AssertionError(f"DescribeLogGroups in the not-opted-in Region {elsewhere!r} answered, listing {shown!r}")
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__logs: CloudWatch Logs answers DescribeLogGroups in a Region the account has not opted in to, and serves the home Region's log groups there, instead of refusing with UnrecognizedClientException

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
Cause 1 of 1: a CloudWatch Logs request addressed to a Region the account has not opted into is answered, and answered with the home Region's log groups.

AWS: recorded behavior only (the opt-in rule is an account/credential rule, not a modelled Logs error). A request signed for a Region whose opt-in-status is `not-opted-in` - exactly the Regions EC2's DescribeRegions(AllRegions=true, Filter opt-in-status=not-opted-in) lists - is refused with HTTP 403, error code `UnrecognizedClientException` and the fixed message `The security token included in the request is invalid.` No log group from any other Region is returned.

Emulator: DescribeLogGroups against `mx-central-1` answers and lists the log group created in the home Region - `listing ['/sim2real/logs-optin-364ffb808f41']`. (The Region and log-group name here fill the template; the requirement is the 403 refusal and that no cross-Region state is served.)

Why: two things break. A client sweeping Regions reads every not-opted-in Region as enabled, so it reports log groups in Regions the account cannot call at all. And because the foreign-Region endpoint serves the home Region's groups, a per-Region inventory counts the same log group once per Region swept, and a delete issued against the foreign endpoint would act on another Region's resource.

Fix: the CloudWatch Logs request path's Region admission - refuse a request whose signed Region is not opted in with 403 UnrecognizedClientException before dispatch - and the log-group store's Region scoping, so a Region's listing only ever contains that Region's groups.

Note: the same opt-in admission defect is already filed for ECS (open: 20261127T105620Z-ec2-ecs-ecs-answers-requests-sent-to-a.md); that issue does not cover CloudWatch Logs.

Traces: ec2__logs.logs-describe-log-groups-in-a-not-opted-in-region-rejected.

## Evidence
Expected (AWS): ClientError with Error.Code 'UnrecognizedClientException' and Error.Message 'The security token included in the request is invalid.' (HTTP 403).

Actual (emulator): "DescribeLogGroups in the not-opted-in Region 'mx-central-1' answered, listing ['/sim2real/logs-optin-364ffb808f41']" - the home Region's log group served from the foreign Region's endpoint.

## Steps to Reproduce
1. DescribeRegions(AllRegions=True, Filters=[{'Name':'opt-in-status','Values':['not-opted-in']}]) and take one Region from the result.
2. CreateLogGroup in the home Region.
3. Build a logs client for the not-opted-in Region and call DescribeLogGroups.
4. Observe it answers, listing the home Region's log group, instead of a 403 UnrecognizedClientException.
