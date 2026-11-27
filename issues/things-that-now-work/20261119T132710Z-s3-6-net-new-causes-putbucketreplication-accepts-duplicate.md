Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.bucket-replication-destination-validation-rejected s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication s3.put-bucket-accelerate-configuration-directory-bucket-rejected s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec s3.create-bucket-reserved-prefix-or-suffix-rejected s3.notification-fifo-destination-rejected s3.s3x-s3express-session-and-listing-expired-token-denied
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.bucket-replication-destination-validation-rejected|_error#31|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication|run#12|AssertionError|assert (code, message) == (
Issue-Key: s3.put-bucket-accelerate-configuration-directory-bucket-rejected|run#4|AssertionError|assert code == "InvalidArgument", f"expected InvalidArgument, got {code!r}: {message!r}"
Issue-Key: s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec|run#6|AssertionError|assert (code, message) == (
Issue-Key: s3.create-bucket-reserved-prefix-or-suffix-rejected|_assert_reserved_prefix#10|AssertionError|assert message == _STARTS_RESERVED, f"{reason}: expected the exact message, got {message!r}"
Issue-Key: s3.notification-fifo-destination-rejected|run#26|AssertionError|assert message2 == (
Issue-Key: s3.s3x-s3express-session-and-listing-expired-token-denied|run#24|AssertionError|assert st["pinned"] == len(st["ops"]), (
Issue-Commit: c439b0c06ad4c0c74cea84caf475018e0421a0f9

# s3: 6 net-new causes — PutBucketReplication accepts duplicate rule priorities, directory-bucket config refusals use the wrong code, and four refusal messages are the emulator's own wording

> _Note: this issue was identified against the code base at commit `c439b0c06ad4c0c74cea84caf475018e0421a0f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketReplication

## Description
Six independent root causes in the S3 emulator, all surviving the replay's own ALREADY-FILED/OWNED marking. Each is numbered with its own failing check ids; all six must be closed before this issue is submitted.

=== Cause 1 of 6: PutBucketReplication accepts two rules that share a Priority ===
Failing check: s3.bucket-replication-destination-validation-rejected
AWS: PutBucketReplication requires every rule's Priority to be unique within the configuration; a configuration containing two enabled rules with the same Priority is refused, nothing is applied, and GetBucketReplication keeps answering ReplicationConfigurationNotFoundError. Smithy: PutBucketReplication (PUT /{Bucket}?replication) models ReplicationConfiguration.Rules[].Priority and declares NO operation-specific errors — S3 carries its refusals as generic REST-XML errors — so the code/status/message below are taken from the recorded response: HTTP 400, Error.Code "InvalidRequest", Error.Message "Found duplicate priority <Priority>." The fixed text is "Found duplicate priority " and the trailing "."; the only variable part is the duplicated Priority integer taken from the request (the recording happens to use 7 — that value is not part of the requirement).
Emulator: the call SUCCEEDS. The check's _error() helper raises "expected the call to be rejected, but it succeeded and returned None", i.e. the emulator answered 200 and stored a configuration AWS never accepts.
Why: a client that relies on S3 to reject colliding priorities gets a silently ambiguous replication configuration. The two sibling validations the same check exercises (Filter without DeleteMarkerReplication -> InvalidRequest "DeleteMarkerReplication must be specified for this version of Cross Region Replication configuration schema. Please refer to S3 Developer Guide for more information.", and the positive control) are never reached, so they are untested behind this acceptance.
Fix: the PutBucketReplication handler's request validation (uniqueness of Rules[].Priority across the submitted configuration, refusing before any state is written).
Traces: s3.bucket-replication-destination-validation-rejected.

=== Cause 2 of 6: a directory bucket's unsupported bucket-configuration PUTs are refused as NotImplemented instead of InvalidArgument ===
Failing checks: s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication, s3.put-bucket-accelerate-configuration-directory-bucket-rejected
AWS: on an S3 Express One Zone directory bucket (name ending --<az-id>--x-s3), the bucket-configuration PUTs that have no directory-bucket equivalent are refused as a REQUEST-SHAPE error, not as an unimplemented feature: HTTP 400, Error.Code "InvalidArgument", Error.Message "<ConfigurationMember> is not valid, expected CreateBucketConfiguration". The fixed text is " is not valid, expected CreateBucketConfiguration"; the variable part is the request's top-level configuration member name — "VersioningConfiguration" for PutBucketVersioning, "NotificationConfiguration" for PutBucketNotificationConfiguration, "ReplicationConfiguration" for PutBucketReplication, "AccelerateConfiguration" for PutBucketAccelerateConfiguration. The matching read is refused differently again: GetBucketAccelerateConfiguration on a directory bucket answers HTTP 405 MethodNotAllowed "The specified method is not allowed against this resource." Smithy: PutBucketVersioning (PUT /{Bucket}?versioning) and PutBucketAccelerateConfiguration (PUT /{Bucket}?accelerate) model the configuration as a required httpPayload member and declare no operation-specific errors, so the code and message are recorded behavior.
Emulator: PutBucketVersioning on a directory bucket returns ('NotImplemented', 'This bucket does not support Versioning.'); PutBucketAccelerateConfiguration returns ('NotImplemented', 'This bucket does not support Transfer Acceleration.'). Both the code and the message diverge, and the status class with them (501-family rather than 400).
Why: a client distinguishing "I sent the wrong thing" from "this service cannot do that" branches on the code. NotImplemented reads as an emulator/服务 gap and is commonly retried or escalated, whereas InvalidArgument is a terminal caller error. Because the versioning leg fails first, the notification and replication legs of the same mapping are never observed and are presumed wrong by the same construction.
Fix: the directory-bucket refusal mapping shared by the bucket-configuration handlers (PutBucketVersioning, PutBucketAccelerateConfiguration, PutBucketNotificationConfiguration, PutBucketReplication) — one InvalidArgument refusal naming the submitted configuration member, plus MethodNotAllowed on the corresponding GET.
Traces: s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication, s3.put-bucket-accelerate-configuration-directory-bucket-rejected.

=== Cause 3 of 6: CopyObject of a TAGGED source into a directory bucket names the wrong restriction ===
Failing check: s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec
AWS: copying an object that carries Object Tags into a directory bucket (default TaggingDirective=COPY) is refused with Error.Code "NotImplemented" and the message "This bucket does not support copying source objects with Object Tags." — entirely fixed text, no values from the request. That message is distinct from the one PutObjectTagging on a directory bucket uses ("This bucket does not support Object Tags."), because the restriction being reported is different: the copy of tags, not the tagging API. The companion refusal in the same check is "This bucket does not support server side encryption with a customer provided key." for a destination requesting SSE-C.
Emulator: returns ('NotImplemented', 'This bucket does not support Object Tags.') — the correct code with the PutObjectTagging message, so the refusal misnames which restriction fired. The SSE-C leg and the no-object-left-behind leg after it are never reached.
Why: the message is the only thing distinguishing the two restrictions; a caller (or an operator reading logs) is told object tagging is unsupported when what was actually refused was copying a tagged source, and will try the wrong remedy (dropping the tag API rather than setting TaggingDirective=REPLACE).
Fix: the CopyObject handler's directory-bucket validation — its own message for a tagged source, separate from the PutObjectTagging refusal text.
Traces: s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec.

=== Cause 4 of 6: CreateBucket's reserved-prefix/suffix refusal carries the generic "Access Denied" message ===
Failing check: s3.create-bucket-reserved-prefix-or-suffix-rejected
AWS: a CreateBucket whose name starts with a reserved prefix (sthree-, amzn-s3-demo-) is refused HTTP 403 with Error.Code "AccessDenied" and Error.Message "The requested bucket name starts with a reserved prefix"; a name ending in a reserved suffix (-s3alias, --ol-s3, .mrap) is refused with the same code and "The requested bucket name ends with a reserved suffix". Both messages are entirely fixed text — no part of them comes from the request. (Documented under the S3 bucket naming rules; Smithy CreateBucket models only BucketAlreadyExists/BucketAlreadyOwnedByYou, so these refusals are generic REST-XML errors and the wording is recorded behavior.) These are posture refusals, deliberately distinct from the shape refusal InvalidBucketName "The specified bucket is not valid." that xn--, a leading hyphen or a trailing hyphen get.
Emulator: the code is right (AccessDenied) but the message is the framework's generic 'Access Denied'. The check fails on the first reserved prefix (sthree-), so the amzn-s3-demo- prefix and all three reserved suffixes are untested behind it.
Why: AccessDenied with "Access Denied" is indistinguishable from a credentials/policy denial, so a caller retries with different credentials or a bucket policy change instead of renaming the bucket. The message is the only signal that the name, not the principal, was refused.
Fix: the CreateBucket handler's bucket-name validation — the reserved-prefix and reserved-suffix refusals each carrying their own message beside the AccessDenied code.
Traces: s3.create-bucket-reserved-prefix-or-suffix-rejected.

=== Cause 5 of 6: PutBucketNotificationConfiguration's FIFO SNS-topic refusal uses the SQS wording instead of the ARN-pattern constraint ===
Failing check: s3.notification-fifo-destination-rejected
AWS: a TopicConfigurations destination whose TopicArn names a FIFO topic (topic name ending .fifo) fails S3's SNS-ARN pattern check, so the refusal is HTTP 400 Error.Code "InvalidArgument", Error.ArgumentName "Topic", Error.ArgumentValue the submitted topic ARN, and Error.Message the fixed constraint text "The ARN failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[\p{Alpha}\-]*:sns:[\p{Alnum}\-]+:[\p{Digit}]{12}:[\p{Alnum}\-_]{1,256}$" (fixed text in full; the only per-request value in the response is ArgumentValue, the ARN from the request). The FIFO SQS case is a different refusal and its message really is "FIFO SQS queues are not supported." with ArgumentName "Queue" — the emulator already serves that one correctly. Smithy: PutBucketNotificationConfiguration (PUT /{Bucket}?notification) declares no operation-specific errors; both refusals are recorded behavior.
Emulator: answers InvalidArgument (correct) with the message 'FIFO SNS topics are not supported.' — the SQS wording generalised to topics, which AWS never emits. The later legs (ArgumentName/ArgumentValue for the topic case, and that the rejected PUT applied nothing) are not reached.
Why: callers and SDK error-mapping layers that match on the message see an unknown string; more importantly the refusal hides WHY the ARN was refused (it failed the SNS ARN pattern), so a caller cannot tell that any ARN shape outside that pattern — not only .fifo — is rejected.
Fix: the PutBucketNotificationConfiguration handler's destination validation — validate TopicArn against S3's SNS ARN pattern and emit the constraint message, keeping the separate FIFO-queue refusal for QueueConfigurations.
Traces: s3.notification-fifo-destination-rejected.

=== Cause 6 of 6: directory-bucket GetObject requests signed with a pinned CreateSession token are answered with a retryable response, so botocore re-signs and retries ===
Failing check: s3.s3x-s3express-session-and-listing-expired-token-denied
AWS: every request a client sends to a directory bucket is answered once; a session token that is still inside its CreateSession Expiration is honoured (HTTP 200) and one past it is refused terminally (HTTP 403, Error.Code "AccessDenied", Error.Message "Access Denied") — neither answer is retryable, so the SDK's attempt count equals its operation count. The check pins one CreateSession identity and counts both: botocore's before-call (one per operation) and before-sign (one per ATTEMPT).
Emulator: the token lifetime, the AccessDenied/403/"Access Denied" refusals for GetObject and PutObject, and the no-object-left-behind leg all matched; the run then ended on "expired: not every call was signed with the pinned session: pinned=37 ops=[32x 'GetObject', 'PutObject']" — 37 signed attempts for 33 operations. Hypothesis: four of those GetObject attempts were answered with a response botocore classifies as retryable (a 5xx or a throttling code) rather than with the 200 or the terminal 403 AWS gives, so the SDK re-signed and retried them. The suite reports only the attempt/operation counts, so which response was served is not recorded here.
Why: a retryable answer on a directory-bucket read makes latency and attempt counts non-deterministic and silently masks terminal refusals behind SDK retries; a client with retries disabled sees a transient failure on a read AWS always answers once.
Fix: the directory-bucket GetObject path and the S3 Express session-auth check in front of it — every outcome must be a terminal 200 or the 403 AccessDenied, never a retryable status.
Traces: s3.s3x-s3express-session-and-listing-expired-token-denied.

## Evidence
Cause 1 — s3.bucket-replication-destination-validation-rejected:
  expected: HTTP 400, InvalidRequest, "Found duplicate priority <Priority>." and GetBucketReplication still ReplicationConfigurationNotFoundError
  actual:   AssertionError: expected the call to be rejected, but it succeeded and returned None (PutBucketReplication with two enabled rules sharing Priority answered 2xx)

Cause 2 — s3.directory-bucket-rejects-object-tagging-versioning-notification-and-replication:
  expected: ('InvalidArgument', 'VersioningConfiguration is not valid, expected CreateBucketConfiguration')
  actual:   AssertionError: PutBucketVersioning on a directory bucket returned ('NotImplemented', 'This bucket does not support Versioning.')
        — s3.put-bucket-accelerate-configuration-directory-bucket-rejected:
  expected: InvalidArgument / 'AccelerateConfiguration is not valid, expected CreateBucketConfiguration'
  actual:   AssertionError: expected InvalidArgument, got 'NotImplemented': 'This bucket does not support Transfer Acceleration.'

Cause 3 — s3.copy-object-into-directory-bucket-rejects-source-tags-and-destination-ssec:
  expected: ('NotImplemented', 'This bucket does not support copying source objects with Object Tags.')
  actual:   AssertionError: CopyObject of a tagged source (TaggingDirective=COPY) into a directory bucket returned ('NotImplemented', 'This bucket does not support Object Tags.')

Cause 4 — s3.create-bucket-reserved-prefix-or-suffix-rejected:
  expected: AccessDenied / 'The requested bucket name starts with a reserved prefix'
  actual:   AssertionError: an sthree- prefix: expected the exact message, got 'Access Denied'

Cause 5 — s3.notification-fifo-destination-rejected:
  expected: InvalidArgument, ArgumentName 'Topic', ArgumentValue <the .fifo topic ARN>, message 'The ARN failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[\p{Alpha}\-]*:sns:[\p{Alnum}\-]+:[\p{Digit}]{12}:[\p{Alnum}\-_]{1,256}$'
  actual:   AssertionError: FIFO SNS topics are not supported.   (that string is the message the emulator returned)

Cause 6 — s3.s3x-s3express-session-and-listing-expired-token-denied:
  expected: one answered attempt per operation (pinned == len(ops))
  actual:   AssertionError: expired: not every call was signed with the pinned session: pinned=37 ops=['GetObject' x32, 'PutObject']  (4 extra signed attempts = 4 retried requests)

## Steps to Reproduce
Cause 1: create two versioned buckets and a replication role; call PutBucketReplication on the source with Rules=[{ID:'dup-a',Status:'Enabled',Priority:7,Filter:{Prefix:'a/'},DeleteMarkerReplication:{Status:'Disabled'},Destination:{Bucket:<dst arn>}}, {ID:'dup-b', ... Priority:7, Filter:{Prefix:'b/'} ...}]. Expect InvalidRequest 'Found duplicate priority 7.'; the emulator returns success. Then GetBucketReplication and observe the stored configuration.

Cause 2: create a directory bucket (CreateBucket with CreateBucketConfiguration.Location.Type=AvailabilityZone and Bucket.Type=Directory, name '<base>--<az-id>--x-s3'); with s3={'disable_s3_express_session_auth': True} call PutBucketVersioning(VersioningConfiguration={'Status':'Enabled'}) and PutBucketAccelerateConfiguration(AccelerateConfiguration={'Status':'Enabled'}) on it. Also call PutBucketNotificationConfiguration({}) and PutBucketReplication on it, and GetBucketAccelerateConfiguration.

Cause 3: PutObject into a general-purpose bucket with Tagging='class=gold', then CopyObject that key into a directory bucket with the default TaggingDirective.

Cause 4: CreateBucket with the names 'sthree-<base>' and 'amzn-s3-demo-<base>', and with '<base>-s3alias', '<base>--ol-s3', '<base>.mrap'.

Cause 5: create a standard SNS topic and confirm PutBucketNotificationConfiguration accepts it; then create a FIFO topic (Name ending '.fifo', Attributes={'FifoTopic':'true'}), grant s3.amazonaws.com SNS:Publish, and call PutBucketNotificationConfiguration with TopicConfigurations=[{TopicArn:<fifo arn>,Events:['s3:ObjectCreated:*']}]. Inspect Error.Code, Error.Message, Error.ArgumentName, Error.ArgumentValue.

Cause 6: create a directory bucket and an object; CreateSession(SessionMode='ReadWrite'); build an S3 client pinned to exactly those session credentials (override botocore's identity_cache on before-sign.s3 so it never re-mints) and poll GetObject every 10 s past the token's Expiration, counting before-sign events against before-call events. Expect them equal; the emulator produces more signed attempts than operations.
