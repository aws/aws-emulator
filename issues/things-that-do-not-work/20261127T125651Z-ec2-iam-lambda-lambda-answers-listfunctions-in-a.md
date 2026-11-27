Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__lambda.lambda-list-functions-in-a-not-opted-in-region-rejected
Issue-Key: ec2__iam__lambda.lambda-list-functions-in-a-not-opted-in-region-rejected|run#14|AssertionError|raise AssertionError(f"ListFunctions in the not-opted-in Region {blocked[0]!r} answered {reply!r}")
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__iam__lambda: Lambda answers ListFunctions in a Region the account has not opted in to, and serves the home Region's functions there, instead of refusing with UnrecognizedClientException

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFunctions

## Description
Cause 1 of 1: a Lambda request addressed to a Region the account has not opted into is answered, and answered with the home Region's state.

AWS: recorded behavior only (the opt-in rule is an account/credential rule, not a modelled Lambda error). A request signed for a Region whose opt-in-status is `not-opted-in` - exactly the Regions EC2's DescribeRegions(AllRegions=true, Filter opt-in-status=not-opted-in) lists - is refused with HTTP 403, error code `UnrecognizedClientException` and the fixed message `The security token included in the request is invalid.` Nothing from any other Region is returned.

Emulator: ListFunctions against `af-south-1` answers HTTP 200 and, worse, lists the function created in the home Region - `{'Functions': [{'FunctionName': 'sim2real-lam-optin-...', 'FunctionArn': 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-lam-optin-...', ...}]}`. (The region names and function name here fill the template; the requirement is the 403 refusal and that no cross-Region state is served.)

Why: two things break. A client probing which Regions it may use reads every not-opted-in Region as enabled, so an inventory sweep reports resources in Regions the account cannot even call. And because the reply carries the home Region's functions under a foreign Region's endpoint, a per-Region inventory double-counts the same function in every Region it sweeps and ARNs do not match the endpoint they were fetched from.

Fix: the Lambda request path's Region admission - refuse a request whose signed Region is not opted in with 403 UnrecognizedClientException before dispatch - and the Lambda function store's Region scoping, so a Region's listing only ever contains that Region's functions.

Note: the same opt-in admission defect is already filed for ECS (open: 20261127T105620Z-ec2-ecs-ecs-answers-requests-sent-to-a.md); that issue does not cover Lambda.

Traces: ec2__iam__lambda.lambda-list-functions-in-a-not-opted-in-region-rejected.

## Evidence
Expected (AWS): ClientError with Error.Code 'UnrecognizedClientException' and Error.Message 'The security token included in the request is invalid.' (HTTP 403).

Actual (emulator): HTTP 200 - "ListFunctions in the not-opted-in Region 'af-south-1' answered {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Functions': [{'FunctionName': 'sim2real-lam-optin-5b98ca8eac71', 'FunctionArn': 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-lam-optin-5b98ca8eac71', ...}]}" - note the us-west-2 ARN served from the af-south-1 endpoint.

## Steps to Reproduce
1. DescribeRegions(AllRegions=True, Filters=[{'Name':'opt-in-status','Values':['not-opted-in']}]) and take one Region from the result.
2. CreateFunction in the home Region and confirm ListFunctions there lists it.
3. Build a lambda client for the not-opted-in Region and call ListFunctions.
4. Observe HTTP 200 listing the home Region's function instead of a 403 UnrecognizedClientException.
