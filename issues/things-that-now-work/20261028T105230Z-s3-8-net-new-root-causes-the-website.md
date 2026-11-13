Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body s3.notification-invalid-destination-rejected s3.op-contract-putBucketNotificationConfiguration s3.notification-filter-non-matching-key-not-delivered s3.deep-s3-immutability-composite-sha256-checksum-reports-parts s3.s3checksum-incompatible-algorithm-and-type-pairs-rejected s3.s3crr-replication-fails-visibly-when-the-role-cannot-write-the-destination s3.deploy-s3-1-rename-onto-owned-bucket-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys|_await#9|Timeout|time.sleep(3.0)
Issue-Key: s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes|_await_http#13|Timeout|time.sleep(3.0)
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body|_error#10|AssertionError|raise AssertionError("the call was expected to be refused and it succeeded")
Issue-Key: s3.notification-invalid-destination-rejected|_err_code#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.op-contract-putBucketNotificationConfiguration|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.notification-filter-non-matching-key-not-delivered|_drain_records#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: s3.deep-s3-immutability-composite-sha256-checksum-reports-parts|run#10|AssertionError|assert r.get("ChecksumSHA256") == _b64sha256(data), (
Issue-Key: s3.s3checksum-incompatible-algorithm-and-type-pairs-rejected|run#10|AssertionError|assert sha_message == (
Issue-Key: s3.s3crr-replication-fails-visibly-when-the-role-cannot-write-the-destination|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: s3.deploy-s3-1-rename-onto-owned-bucket-rejected|run#24|AssertionError|assert squat in reason, f"Bucket failure reason does not name the colliding bucket {squat!r}: {reason!r}"
Issue-Commit: a991e58b63dd0e61a72e74cc1f818b9fb1c8de8d

# s3: 8 net-new root causes — the website endpoint serves nothing, WebsiteRedirectLocation/checksum-pair refusals are missing or misworded, notification destinations are unvalidated and never fire, UploadPart omits ChecksumSHA256, replication never FAILs, CFN bucket-collision reason omits the bucket

> _Note: this issue was identified against the code base at commit `a991e58b63dd0e61a72e74cc1f818b9fb1c8de8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketWebsite

## Description
This issue bundles 8 distinct net-new root causes across the S3 website endpoint, notification configuration, flexible checksums, replication and the CloudFormation S3 bucket provider. All 8 must be closed before submitting; each cause below lists its own failing check ids.

===== Cause 1 of 8: the S3 website endpoint is not served =====
AWS: a bucket with a website configuration is reachable on a SEPARATE HTTP surface from the REST endpoint. On that surface `GET /` and `GET /sub/` serve the IndexDocument suffix with 200; a key the caller is authorized for but which does not exist serves the ErrorDocument body with HTTP 404; content the policy does not grant answers 403; a PUT answers 405. With RoutingRules configured, `HttpErrorCodeReturnedEquals: "404"` redirects a missing key with the rule's HttpRedirectCode (302) and `Location` = the absolute URL on the website host of the `ReplaceKeyWith` target, `KeyPrefixEquals` substitutes `ReplaceKeyPrefixWith` with a default 301, an existing key is untouched (200, no Location), and the REST endpoint ignores routing rules entirely (a missing key is still `NoSuchKey`/404/"The specified key does not exist."). Smithy models PutBucketWebsite/GetBucketWebsite only; the endpoint behaviour is recorded behavior per the S3 WebsiteEndpoints documentation the checks cite.
Emulator: nothing is served on the website endpoint. Both checks hang on their very first website-endpoint GET — the positive control that fetches an existing object through the website host never reaches 200 — and are killed by the harness ("Check timed out after 60.0s"). GetBucketWebsite does store and return the configuration, so the control plane is present and only the data-plane surface is missing.
Why: static-website hosting is unusable and untestable: index/error-document resolution, 403-vs-404 distinction, the 405 on writes and every RoutingRule are invisible. Worse, a client gets a hang rather than any HTTP answer, so it cannot even distinguish "not configured" from "endpoint absent".
Fix: the s3 crate needs a website-endpoint HTTP surface (distinct host/route from the REST endpoint) that resolves IndexDocument/ErrorDocument, applies RoutingRules, maps authorization outcomes to 403/404 and answers 405 to non-GET/HEAD; the REST handlers must keep ignoring routing rules.
Traces: s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes; s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys.

===== Cause 2 of 8: PutObject does not validate WebsiteRedirectLocation =====
AWS: `x-amz-website-redirect-location` (Smithy: PutObject.WebsiteRedirectLocation, `httpHeader: x-amz-website-redirect-location`) must begin with `http://`, `https://` or `/`. A relative value with no leading slash is refused with HTTP 400 and error code `InvalidRedirectLocation`, message `The website redirect location must have a prefix of 'http://' or 'https://' or '/'.` — the message is entirely fixed text. The refusal has no effect: the key is not created, so a following GetObject answers `NoSuchKey`/"The specified key does not exist."
Emulator: `PutObject(Key="bad.html", WebsiteRedirectLocation="new.html")` SUCCEEDS ("the call was expected to be refused and it succeeded"), so the object is created with an unusable redirect value and no refusal is produced.
Why: a malformed redirect target is persisted instead of rejected; the website endpoint would later emit a `Location` AWS would never have accepted, and a client that relies on the 400 to validate user input accepts bad data.
Fix: validation in the s3 crate's PutObject handler (and the same check in CopyObject/CreateMultipartUpload, which carry the same member) must reject a WebsiteRedirectLocation without one of the three prefixes with 400/InvalidRedirectLocation before anything is stored.
Traces: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body.

===== Cause 3 of 8: PutBucketNotificationConfiguration never validates its destinations =====
AWS: PutBucketNotificationConfiguration validates each destination before storing it — Smithy even models the opt-out `SkipDestinationValidation` (`httpHeader: x-amz-skip-destination-validation`), which presupposes that validation is the default. A destination S3 cannot deliver to (an SQS queue whose policy does not allow s3 SendMessage, a Lambda it has no permission to invoke) is refused with HTTP 400, error code `InvalidArgument` and the message `Unable to validate the following destination configurations` (fixed text; AWS appends the offending destination's identity). The refusal must not partially apply: GetBucketNotificationConfiguration afterwards reports no QueueConfigurations. Granting the queue the missing permission makes the IDENTICAL call succeed.
Emulator: both calls return HTTP 200 with an empty body and the unvalidatable configuration is stored ("expected the call to be rejected, but it SUCCEEDED" / "expected a ClientError, but the call SUCCEEDED"). No destination validation exists at all.
Why: the single most common S3 notification misconfiguration (a destination missing its resource policy) is accepted silently; the operator learns nothing at configuration time and events simply never arrive. A related open issue, 20261025T032458Z-s3-sqs-s3-notification-destinations-are-never-validated.md, records this under the `s3__sqs` service and does not carry these two `s3` checks.
Fix: the s3 crate's PutBucketNotificationConfiguration handler must pre-flight every Queue/Topic/Lambda destination (honouring SkipDestinationValidation) and refuse with 400/InvalidArgument/"Unable to validate the following destination configurations" without persisting any part of the configuration.
Traces: s3.notification-invalid-destination-rejected; s3.op-contract-putBucketNotificationConfiguration.

===== Cause 4 of 8: object-event notifications are never published, so filter non-delivery is ungradeable =====
AWS: with a notification configuration whose rule carries a key-name filter, an object whose key MATCHES the filter produces exactly one event at the destination and an object whose key does NOT match produces none — the filter is enforced by S3, so the negative leg is observable only because the positive leg delivers.
Emulator: the check runs out the 60 s harness cap ("Check timed out after 60.0s") waiting for the matching object's event, because S3 publishes nothing on either path. The filter can therefore never be shown to suppress anything.
Why: every object-driven pipeline built on bucket notifications is inert, and a filter regression (too broad or too narrow) cannot be detected at all.
Fix: the s3 crate's object-write paths (PutObject/CompleteMultipartUpload/DeleteObject/Copy) must evaluate the stored notification configuration and publish the matching s3:ObjectCreated:*/ObjectRemoved:* events to SQS/SNS/Lambda/EventBridge destinations, applying prefix/suffix FilterRules.
Traces: s3.notification-filter-non-matching-key-not-delivered.

===== Cause 5 of 8: UploadPart's response omits its flexible-checksum member =====
AWS: when UploadPart is called with `x-amz-sdk-checksum-algorithm: SHA256` (Smithy: UploadPart.ChecksumAlgorithm, `httpChecksum.requestAlgorithmMember`), the response echoes the part's digest in `x-amz-checksum-sha256` (Smithy: UploadPartOutput.ChecksumSHA256, `httpHeader: x-amz-checksum-sha256`) — the base64 SHA-256 of that part's bytes. The caller needs it to build the `MultipartUpload.Parts[].ChecksumSHA256` list that CompleteMultipartUpload requires.
Emulator: the UploadPart response carries no ChecksumSHA256 at all ("part 1 ChecksumSHA256 None != sha256 of its bytes"), so a checksum-aware multipart upload cannot be completed.
Why: any SDK path that uses flexible checksums over multipart (the default for several SDK versions) breaks: the client has no per-part digest to send back, and the COMPOSITE whole-object checksum (`sha256(sha256(p1)||sha256(p2))` with the `-<parts>` suffix) can never be verified. A related open issue, 20261025T160428Z-s3-multipart-analytics-3-net-new-root-causes.md, mentions UploadPart omitting its flexible-checksum response members; it does not carry this check id and the suite still reports this failure as unowned.
Fix: the s3 crate's UploadPart handler must compute the requested algorithm over the part body and serialize it into the corresponding `x-amz-checksum-*` response header; CompleteMultipartUpload/GetObject/GetObjectAttributes must then report the COMPOSITE digest consistently.
Traces: s3.deep-s3-immutability-composite-sha256-checksum-reports-parts.

===== Cause 6 of 8: the incompatible checksum algorithm/type refusal carries a generic message instead of AWS's =====
AWS: CreateMultipartUpload rejects an algorithm/type pair that cannot exist. Smithy models both `ChecksumAlgorithm` (`x-amz-checksum-algorithm`) and `ChecksumType` (`x-amz-checksum-type`) on CreateMultipartUploadRequest. `SHA256` + `FULL_OBJECT` is refused HTTP 400 `InvalidRequest` with the message `The FULL_OBJECT checksum type cannot be used with the sha256 checksum algorithm.` — the fixed text is "The FULL_OBJECT checksum type cannot be used with the ", " checksum algorithm." and the variable part is the requested algorithm rendered lower-case, taken from the request. (The converse pair, CRC64NVME + COMPOSITE, is refused with the matching message for that direction.)
Emulator: the status and the `InvalidRequest` code are right, but the message is the emulator's generic validation prose: `A parameter or header in your request isn't valid. For details, see the description of this API operation.`
Why: the message is a requirement of the error class, and here it is the ONLY thing that tells a caller which of the two headers was wrong and why — a client (or a human) gets no actionable diagnosis and cannot match on AWS's text.
Fix: the s3 crate's CreateMultipartUpload validation must emit the per-pair message templates rather than falling through to the generic invalid-parameter message.
Traces: s3.s3checksum-incompatible-algorithm-and-type-pairs-rejected.

===== Cause 7 of 8: replication never reaches FAILED when the role cannot write the destination =====
AWS: replication is authorized per object. A replication role denied `s3:ReplicateObject` on the destination makes the source object's `ReplicationStatus` reach the terminal value `FAILED` (visible on HeadObject/GetObject) and nothing is delivered to the destination bucket; an identically wired healthy pair reaches `COMPLETED`, so FAILED is not a blanket outcome. Recorded behavior only (ReplicationStatus is a modeled response member; the authorization outcome is not expressed in the model).
Emulator: the status never leaves its non-terminal/COMPLETED value for the denied pair and the check runs out the 60 s harness cap ("Check timed out after 60.0s") polling for FAILED. A related open issue, 20261025T165616Z-cloudformation-dynamodb-s3-s3-cross-bucket-replication-has-no-data-plane.md, records that replication copies nothing; it is filed under a different service token and does not carry this check or the FAILED-status contract.
Why: a broken replication role is reported as healthy. An operator watching ReplicationStatus — the only per-object signal AWS gives — cannot tell a permission failure from success, and a DR/compliance copy that does not exist looks like it does.
Fix: the s3 crate's replication engine must authorize each replication against the configured role (source read actions and destination write actions) and set the source object's ReplicationStatus to FAILED, leaving the destination untouched, while an authorized pair transitions to COMPLETED.
Traces: s3.s3crr-replication-fails-visibly-when-the-role-cannot-write-the-destination.

===== Cause 8 of 8: the CloudFormation AWS::S3::Bucket failure reason does not name the colliding bucket =====
AWS: renaming an `AWS::S3::Bucket` onto a bucket name the account already owns fails the resource and rolls the stack back to UPDATE_ROLLBACK_COMPLETE, and the failing resource's `ResourceStatusReason` names the colliding bucket so the operator can identify it; after the rollback the resource's PhysicalResourceId is the ORIGINAL bucket, the original bucket's objects are intact, and the pre-existing bucket is neither adopted (no stack ownership tag) nor modified. Recorded behavior only (stack-event reasons are not modeled in Smithy).
Emulator: the rollback and the AlreadyExists handler error code are produced, but the reason is `Resource handler returned message: "Your previous request to create the named bucket succeeded and you already own it." (RequestToken: <token>, HandlerErrorCode: AlreadyExists)` — the generic BucketAlreadyOwnedByYou sentence with the bucket name dropped. The fixed wrapper text (`Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: AlreadyExists)`) is right; the variable part that must come from the request — the colliding bucket name — is absent.
Why: on a stack with several buckets the operator cannot tell WHICH name collided, which is the entire purpose of the reason field; automation that parses the reason to report the offending resource gets nothing.
Fix: the CloudFormation AWS::S3::Bucket resource provider's create/rename failure path must include the requested bucket name in the message it reports into ResourceStatusReason.
Traces: s3.deploy-s3-1-rename-onto-owned-bucket-rejected.

## Evidence
Cause 1 — expected: website-endpoint GET of an existing key returns 200 with its bytes, `/` serves index.html, a missing authorized key serves error.html with 404, private content 403, PUT 405; with RoutingRules a missing key is 302 with Location=<website-base>/fallback.html and /docs/guide.html is 301 to <website-base>/documents/guide.html. Actual: both checks hang on the first website-endpoint request and report "Check timed out after 60.0s" (the GetBucketWebsite round-trip of IndexDocument/ErrorDocument/RoutingRules succeeds, so only the serving surface is missing).

Cause 2 — expected: PutObject(WebsiteRedirectLocation="new.html") -> HTTP 400, Code=InvalidRedirectLocation, Message="The website redirect location must have a prefix of 'http://' or 'https://' or '/'.", and the key is not created (GetObject -> NoSuchKey / "The specified key does not exist."). Actual: AssertionError "the call was expected to be refused and it succeeded" — the PutObject returned success.

Cause 3 — expected: HTTP 400, Code=InvalidArgument, Message="Unable to validate the following destination configurations", with no part of the configuration stored. Actual: HTTPStatusCode 200 with an empty body (content-length 0) for both the SQS-queue destination with no S3 SendMessage grant and the unvalidatable Lambda destination — "expected the call to be rejected, but it SUCCEEDED" / "expected a ClientError, but the call SUCCEEDED".

Cause 4 — expected: the key matching the FilterRule produces exactly one event at the destination and the non-matching key produces none. Actual: "Check timed out after 60.0s" — the matching object's event never arrives, so nothing is delivered on either leg.

Cause 5 — expected: UploadPart(ChecksumAlgorithm="SHA256") response carries ChecksumSHA256 = base64(sha256(part bytes)). Actual: AssertionError "part 1 ChecksumSHA256 None != sha256 of its bytes" — the response member is absent.

Cause 6 — expected: CreateMultipartUpload(ChecksumAlgorithm="SHA256", ChecksumType="FULL_OBJECT") -> 400 InvalidRequest with Message="The FULL_OBJECT checksum type cannot be used with the sha256 checksum algorithm.". Actual: the message is "A parameter or header in your request isn't valid. For details, see the description of this API operation."

Cause 7 — expected: HeadObject ReplicationStatus reaches FAILED for the role denied s3:ReplicateObject, with nothing written to the destination, while a healthy pair reaches COMPLETED. Actual: "Check timed out after 60.0s" — FAILED never appears.

Cause 8 — expected: the failing Bucket resource event's ResourceStatusReason names the colliding bucket. Actual: 'Resource handler returned message: "Your previous request to create the named bucket succeeded and you already own it." (RequestToken: <token>, HandlerErrorCode: AlreadyExists)' — AssertionError "Bucket failure reason does not name the colliding bucket".

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check ids in trace_ids.

Cause 1: create a bucket, make it public-read, put index.html/error.html/page.html, call put_bucket_website with IndexDocument/ErrorDocument (and then RoutingRules), and GET the website base URL for `/`, an existing key, a missing key and a PUT — no request is answered.
Cause 2: s3.put_object(Bucket=b, Key="bad.html", Body=b"x", WebsiteRedirectLocation="new.html") — it succeeds instead of returning 400 InvalidRedirectLocation.
Cause 3: create an SQS queue with NO policy allowing s3 SendMessage and call put_bucket_notification_configuration with a QueueConfiguration for it — it returns 200; then grant the policy and observe the identical call also succeed (so the first was never validated). Repeat with a Lambda destination lacking an invoke permission.
Cause 4: configure a notification with a key-name FilterRule, put a MATCHING key, and poll the destination — no event arrives.
Cause 5: create_multipart_upload(ChecksumAlgorithm="SHA256"), then upload_part(..., ChecksumAlgorithm="SHA256") and read ChecksumSHA256 from the response — it is absent.
Cause 6: create_multipart_upload(ChecksumAlgorithm="SHA256", ChecksumType="FULL_OBJECT") and read Error.Message.
Cause 7: set up two versioned buckets, grant the replication role the source read actions but DENY s3:ReplicateObject on the destination, put_bucket_replication, put an object and poll head_object ReplicationStatus for FAILED.
Cause 8: create a stack with an AWS::S3::Bucket whose name is a parameter; create a second bucket out-of-band; update_stack to rename the resource onto that owned name; after UPDATE_ROLLBACK_COMPLETE read the failing Bucket event's ResourceStatusReason.
