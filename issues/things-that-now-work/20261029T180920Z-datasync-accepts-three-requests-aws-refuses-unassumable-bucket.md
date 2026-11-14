Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-location-iam-role-validation
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-location-iam-role-validation datasync-s3-location-lifecycle datasync-task-and-filter-validation-errors
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|CreateLocationS3|4xx->2xx|$.__type
Issue-Key: datasync|CreateLocationS3|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|CreateLocationS3|4xx->2xx|$.errorCode
Issue-Key: datasync|CreateLocationS3|4xx->2xx|$.message
Issue-Key: datasync|CreateLocationS3|4xx->2xx|status
Issue-Key: datasync|DescribeLocationS3|4xx->2xx|$.__type
Issue-Key: datasync|DescribeLocationS3|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|DescribeLocationS3|4xx->2xx|$.errorCode
Issue-Key: datasync|DescribeLocationS3|4xx->2xx|$.message
Issue-Key: datasync|DescribeLocationS3|4xx->2xx|status
Issue-Key: datasync|CreateTask|4xx->2xx|$.__type
Issue-Key: datasync|CreateTask|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|CreateTask|4xx->2xx|$.errorCode
Issue-Key: datasync|CreateTask|4xx->2xx|$.message
Issue-Key: datasync|CreateTask|4xx->2xx|status
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# DataSync accepts three requests AWS refuses: unassumable bucket-access role, absent location, self-referential task (3 causes)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLocationS3

## Description
Three over-permissive root causes: requests that real DataSync refuses with HTTP 400 InvalidRequestException are answered 200 by the emulator, which then mints or reports state that cannot exist in AWS. Smithy model consulted: services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json — CreateLocationS3 (required S3BucketArn, required S3Config{BucketAccessRoleArn @required, pattern ^arn:(aws|...):iam::[0-9]{12}:role/.*$}), DescribeLocationS3, CreateTask (SourceLocationArn and DestinationLocationArn both @required), and InvalidRequestException {message, errorCode, datasyncErrorCode} (client, 400). Each operation's ONLY modeled client error is InvalidRequestException, so the refusal class is carried by its errorCode/datasyncErrorCode members.

=== Cause 1 of 3: CreateLocationS3 does not prove DataSync can assume the bucket-access role ===
AWS: beyond the modeled ARN pattern, DataSync actually calls sts:AssumeRole on the S3Config.BucketAccessRoleArn before creating the location. When the role cannot be assumed by the DataSync service principal — it does not exist, or its trust policy does not trust datasync.amazonaws.com — the call is refused with HTTP 400, __type InvalidRequestException, errorCode = datasyncErrorCode = "StsAssumeRoleInvalidRequest", and no location is created. Message template: fixed text "Invalid IAM role, " + <the BucketAccessRoleArn exactly as supplied> + ". DataSync service was unable to perform sts:AssumeRole." The only variable slot is the role ARN from the request.
Emulator: the request is answered 200 and a LocationArn is minted — status, __type, both error codes and the message are all absent from the response (detail: "status: expected 400, got 200"; $.__type/$.datasyncErrorCode/$.errorCode/$.message all missing). The emulator validates the role ARN's shape only and never consults IAM trust state. The cluster shows this for three different unassumable-role shapes in the same fixture.
Why: CreateLocationS3 is the documented pre-flight check for a mis-configured bucket-access role — it is where an operator learns the trust policy is wrong. Against the emulator every broken role passes, a location is persisted and listed, and the failure that AWS surfaces synchronously at create time cannot be reproduced at all, so trust-policy negative tests are unwritable.
Fix: the CreateLocationS3 handler's validation — resolve S3Config.BucketAccessRoleArn in emulator IAM state, verify the DataSync service principal can assume it, and on failure raise InvalidRequestException(400) with the StsAssumeRoleInvalidRequest discriminator and no location persisted.
Note: this is NOT the cause already filed as Cause 1 of the open behavioral issue 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md — that one is about a role that exists and is assumable but whose S3 policy does not grant the bucket (a bucket-read test). This cause is the sts:AssumeRole/trust-policy gate, which produces a different discriminator (StsAssumeRoleInvalidRequest) and a different message template.

=== Cause 2 of 3: DescribeLocationS3 answers 200 for a location the account does not hold ===
AWS: a well-formed location ARN that names no existing location is refused with HTTP 400, __type InvalidRequestException, errorCode = datasyncErrorCode = "LocationNotFound", and the message template fixed text "Location " + <the location ARN as supplied> + " is not found." DataSync holds a location only if one was created; nothing about a location is derivable from its ARN.
Emulator: the call returns 200 with a location body ("status: expected 400, got 200", and the whole error envelope missing). The emulator synthesizes a response from the requested ARN rather than looking the location up in its state, so every syntactically valid location ARN appears to exist.
Why: existence checks are the backbone of idempotent provisioning — a reconciler that does DescribeLocationS3 and creates the location only on not-found will never create anything against the emulator, and a client deleting a location then confirming the delete sees it still present. It also makes the (correctly refused) not-found path in the same service inconsistent, so the behavior depends on which ARN happens to be asked for.
Fix: the DescribeLocationS3 handler must look the location up in the service's state and refuse an absent one with InvalidRequestException(400)/LocationNotFound instead of deriving the response from the request.

=== Cause 3 of 3: CreateTask accepts DestinationLocationArn equal to SourceLocationArn ===
AWS: SourceLocationArn and DestinationLocationArn are two independently @required LocationArn members with identical patterns, so their equality is a cross-member business rule rather than a modeled constraint. Real DataSync refuses the degenerate pair with HTTP 400, __type InvalidRequestException, errorCode = datasyncErrorCode = "DestLocSameAsSrcLoc" and the wholly fixed message "Destination location cannot be same as source location." (no variable slot), and no task is created.
Emulator: 200 with a TaskArn ("status: expected 400, got 200"; __type, both codes and message missing), so a task that would copy a location onto itself exists and is listed.
Why: a client relying on AWS to reject the self-copy at create time gets it accepted and startable, turning a configuration bug AWS catches synchronously into a silently running self-copy.
Fix: cross-member validation in the CreateTask handler, before any task state is persisted, emitting the DestLocSameAsSrcLoc refusal.
Note: Cause 2 of the open BEHAVIORAL issue 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md describes the same missing rule from the behavioral suite; it does not carry this fixture step's replay key, so the wire divergence is filed here and that issue is named for the fixer's benefit.

Traces: Cause 1 — datasync-s3-location-iam-role-validation#task#7 (cluster also covers task#8 and task#9); Cause 2 — datasync-s3-location-lifecycle#task#13; Cause 3 — datasync-task-and-filter-validation-errors#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true). Three verdict=mismatch clusters, all expected_status 400 / actual_status 200 — the emulator accepted a request AWS refuses.

1) Cause 1 — datasync-s3-location-iam-role-validation#task#7, CreateLocationS3 (affected_count 3: also task#8, task#9):
   status: expected 400, got 200
   $.__type: missing (expected 'InvalidRequestException')
   $.datasyncErrorCode: missing (expected 'StsAssumeRoleInvalidRequest')
   $.errorCode: missing (expected 'StsAssumeRoleInvalidRequest')
   $.message: missing (expected 'Invalid IAM role, arn:aws:iam::000000000000:role/sim2real/sim2real-probe-datasync-notrust-<id>. DataSync service was unable to perform sts:AssumeRole.' — the role ARN is the request's value; the surrounding sentence is fixed)

2) Cause 2 — datasync-s3-location-lifecycle#task#13, DescribeLocationS3:
   status: expected 400, got 200
   $.__type: missing (expected 'InvalidRequestException')
   $.datasyncErrorCode / $.errorCode: missing (expected 'LocationNotFound')
   $.message: missing (expected 'Location arn:aws:datasync:us-west-2:000000000000:location/loc-<id> is not found.' — the ARN slot comes from the request)

3) Cause 3 — datasync-task-and-filter-validation-errors#task#7, CreateTask:
   status: expected 400, got 200
   $.__type: missing (expected 'InvalidRequestException')
   $.datasyncErrorCode / $.errorCode: missing (expected 'DestLocSameAsSrcLoc')
   $.message: missing (expected 'Destination location cannot be same as source location.' — entirely fixed text)

Model references: CreateLocationS3 input {S3BucketArn @required, S3Config @required}; S3Config.BucketAccessRoleArn @required, pattern ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):iam::[0-9]{12}:role/.*$; DescribeLocationS3.LocationArn @required; CreateTask.SourceLocationArn and .DestinationLocationArn both @required LocationArn; errors for all three ops = [InternalException(500, server), InvalidRequestException(400, client)].

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 datasync fixtures.

Cause 1: create an S3 bucket and an IAM role whose trust policy does NOT trust the DataSync service principal (and/or pass a role ARN for a role that does not exist), then CreateLocationS3 with that ARN in S3Config.BucketAccessRoleArn. Observe 200 and a minted LocationArn instead of 400/InvalidRequestException with StsAssumeRoleInvalidRequest. Replay: datasync-s3-location-iam-role-validation#task#7

Cause 2: issue DescribeLocationS3 with a well-formed location ARN that was never created (or create a location, DeleteLocation it, then describe it). Observe a 200 location body instead of 400/LocationNotFound. Replay: datasync-s3-location-lifecycle#task#13

Cause 3: create one S3 location, then CreateTask with SourceLocationArn == DestinationLocationArn. Observe a TaskArn is returned instead of 400/InvalidRequestException with DestLocSameAsSrcLoc, and ListTasks shows the task. Replay: datasync-task-and-filter-validation-errors#task#7
