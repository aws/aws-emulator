Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-target-error-paths
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-target-error-paths
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyActivityStream|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyActivityStream|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyActivityStream|4xx->4xx|status
Issue-Commit: ae04e9c7457c944b401f43b41441b2fef7a78bf2

# rds ModifyActivityStream refuses a cross-Region ResourceArn as 404 DBInstanceNotFound instead of 400 InvalidParameterValue

> _Note: this issue was identified against the code base at commit `ae04e9c7457c944b401f43b41441b2fef7a78bf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyActivityStream

## Description
One cause. ModifyActivityStream does not validate the SHAPE and REGION of its ResourceArn before resolving the resource, so an ARN that names another Region is reported as a missing DB instance.

AWS: Smithy: ModifyActivityStream input ResourceArn (String) with modeled errors DBInstanceNotFoundFault (404, awsQueryError code DBInstanceNotFound), InvalidDBInstanceStateFault (400) and ResourceNotFoundFault (404) — none of which covers an ARN whose Region is not the endpoint's. The recording shows AWS rejecting the ARN at parameter validation, BEFORE any lookup: HTTP 400 with Error.Code 'InvalidParameterValue' and the message template 'The provided ARN (<arn>) is invalid for this parameter (<parameterName>). Expected region = <endpoint region>, actual region = <region parsed from the ARN>'. Fixed text: 'The provided ARN (', ') is invalid for this parameter (', '). Expected region = ', ', actual region = '. Variable parts, taken from the request and the endpoint: the ARN exactly as submitted, the input member name (here ResourceArn), the Region the endpoint serves, and the Region parsed out of the ARN. The recorded instance id and the us-west-2/us-east-1 pair are one run's values, not part of the requirement.
Emulator: it skips ARN validation, extracts the identifier from the ARN and looks it up in the local Region, then answers HTTP 404 with Error.Code 'DBInstanceNotFound' and its own sentence 'DBInstance not found: <identifier>'.
Why: a client that targets the wrong Region gets a retryable-looking not-found for a DB instance that does exist elsewhere, instead of the 400 that tells it the ARN's Region is wrong; error-handling branches keyed on DBInstanceNotFound (create-if-absent, treat-as-deleted) take the destructive path, and the ARN-validation path is untestable.
Fix: ModifyActivityStream's input validation in the rds crate — parse and validate ResourceArn (partition/service/Region/account/resource-type) and raise the 400 InvalidParameterValue with AWS's wording when the ARN's Region differs from the endpoint's, before the DB-instance lookup. The same gate belongs on the sibling ARN-taking activity-stream operations.

Traces: rds-db-instance-target-error-paths#task#14.

## Evidence
Fixture rds-db-instance-target-error-paths, step task#14, operation ModifyActivityStream: status expected 400, got 404. $.ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'DBInstanceNotFound'. $.ErrorResponse.Error.Message: expected 'The provided ARN (arn:aws:rds:us-east-1:000000000000:db:sim2real-probe-missing-inst-xyz) is invalid for this parameter (ResourceArn). Expected region = us-west-2, actual region = us-east-1', got 'DBInstance not found: sim2real-probe-missing-inst-xyz'. (The ARN, the identifier and the two Region names are the request's/endpoint's values filling the template; the quoted fixed text and the parameter name ResourceArn are the requirement.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture rds-db-instance-target-error-paths step task#14: POST Action=ModifyActivityStream to the us-west-2 RDS endpoint with ResourceArn=arn:aws:rds:us-east-1:000000000000:db:<any-identifier> (an ARN whose Region is not the endpoint's).
3. Observe HTTP 404 / Error.Code DBInstanceNotFound / 'DBInstance not found: <identifier>' where the recording has HTTP 400 / InvalidParameterValue / 'The provided ARN (<arn>) is invalid for this parameter (ResourceArn). Expected region = us-west-2, actual region = us-east-1'.
