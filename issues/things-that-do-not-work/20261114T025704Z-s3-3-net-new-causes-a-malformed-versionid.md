Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.get-object-nonexistent-version-errors s3.s3-analytics-xacct-put-rejected-for-an-empty-bucket-account-id s3.update-object-encryption-error-paths
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.get-object-nonexistent-version-errors|run#8|AssertionError|assert err.get("Code") == "InvalidArgument", f"expected InvalidArgument for a malformed VersionId, got {err!r}"
Issue-Key: s3.s3-analytics-xacct-put-rejected-for-an-empty-bucket-account-id|_error#6|AssertionError|raise AssertionError(f"the call succeeded where real AWS rejects it; it returned {result!r}")
Issue-Key: s3.update-object-encryption-error-paths|run#20|AssertionError|assert msg_malformed == MSG_ACCESS_DENIED, (
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# s3: 3 net-new causes — a malformed VersionId is answered NoSuchVersion instead of InvalidArgument, an empty BucketAccountId analytics export is accepted, and UpdateObjectEncryption's AccessDenied message is emulator-authored

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 3: GetObject with a structurally invalid VersionId is refused as a missing version rather than as a bad argument.
AWS: S3 distinguishes a malformed version id from a well-formed one that no longer exists. A VersionId that is not a valid version-id token is refused with HTTP 400 InvalidArgument and the message "Invalid version id specified"; only a well-formed id whose version has been permanently deleted is refused with 404 NoSuchVersion "The specified version does not exist.". Smithy: s3#GetObject declares NoSuchKey (404) and InvalidObjectState (403); InvalidArgument is one of S3's common client errors and is recorded behavior here. The message's fixed text is "Invalid version id specified"; the request's VersionId is the only variable part of the exchange.
Emulator: the same request is answered 404 NoSuchVersion with {'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.', 'Key': <key>, 'VersionId': <the malformed id>} — the deleted-version refusal, for input AWS rejects as malformed.
Why: a client cannot tell "you sent me a bad version id" (a 400, never worth retrying, a caller bug) from "that version is gone" (a 404, an expected lifecycle outcome); SDK retry/error classification and 404-tolerant code paths both take the wrong branch.
Fix: VersionId syntax validation in the GetObject handler's version resolution, ahead of the version lookup, and the InvalidArgument refusal it must raise.
Traces: s3.get-object-nonexistent-version-errors.

Cause 2 of 3: PutBucketAnalyticsConfiguration accepts an explicitly empty S3BucketDestination BucketAccountId and stores the configuration.
AWS: a present-but-empty BucketAccountId is invalid (unlike omitting the member, which is valid, and unlike a well-formed foreign 12-digit id, which is also valid): the call is refused with HTTP 400 MalformedXML and the message "The XML you provided was not well-formed or did not validate against our published schema" — fixed text with no variable part — and nothing is stored, so ListBucketAnalyticsConfigurations still reports no configuration for that bucket. Smithy: s3#PutBucketAnalyticsConfiguration takes Bucket, Id and the AnalyticsConfiguration payload and declares no operation-specific errors, so the refusal is S3's common MalformedXML; the empty-vs-omitted distinction is recorded behavior.
Emulator: the PUT succeeds — HTTP 204 with no error — and the configuration is stored under the given Id.
Why: a caller that serializes an empty string where it meant "this account" gets a configuration AWS would never have created; the export silently names an empty account, and code that relies on the refusal to catch the mistake (or on the bucket listing being empty afterwards) diverges from the start.
Fix: payload validation in the PutBucketAnalyticsConfiguration handler (reject a present-but-empty S3BucketDestination.BucketAccountId with MalformedXML before storing), and leave bucket analytics state untouched on refusal.
Traces: s3.s3-analytics-xacct-put-rejected-for-an-empty-bucket-account-id.

Cause 3 of 3: UpdateObjectEncryption's AccessDenied for a KMS key that is not the bucket owner's carries an emulator-authored message.
AWS: UpdateObjectEncryption refuses both a malformed KMS key ARN and a foreign-account one with HTTP 403 AccessDenied whose message is "The KMS key must be from the same account as the bucket" — fixed text, no request values interpolated (S3 does not reveal whether the key id was malformed or merely inaccessible). Smithy: s3#UpdateObjectEncryption declares AccessDenied (403), InvalidRequest (400) and NoSuchKey (404); the message text is recorded behavior.
Emulator: the refusal's message is "The Amazon Web Services KMS key in the request must be owned by the same account as the bucket. Modify the request to specify a KMS key from the same account, and then try again." — different wording for the same error class.
Why: the error message is part of the contract a caller matches on (log assertions, user-facing surfacing, test fixtures); a reworded message makes an emulator-based test pass where the AWS message would fail it, and vice versa.
Fix: the AccessDenied refusal text in the UpdateObjectEncryption handler's KMS-key ownership check.
Traces: s3.update-object-encryption-error-paths.

## Evidence
s3.get-object-nonexistent-version-errors — AssertionError: expected InvalidArgument for a malformed VersionId, got {'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.', 'Key': 'k', 'VersionId': '<malformed id>'}. Expected: 400 InvalidArgument, Message "Invalid version id specified"; the well-formed-but-deleted id must still give 404 NoSuchVersion "The specified version does not exist.".
s3.s3-analytics-xacct-put-rejected-for-an-empty-bucket-account-id — AssertionError: the call succeeded where real AWS rejects it; it returned HTTPStatusCode 204. Expected: 400 MalformedXML, Message "The XML you provided was not well-formed or did not validate against our published schema", and ListBucketAnalyticsConfigurations empty afterwards.
s3.update-object-encryption-error-paths — AssertionError: expected the AccessDenied message 'The KMS key must be from the same account as the bucket', got 'The Amazon Web Services KMS key in the request must be owned by the same account as the bucket. Modify the request to specify a KMS key from the same account, and then try again.'

## Steps to Reproduce
Cause 1: create a versioned bucket, PutObject key "k" twice, then GetObject with VersionId equal to the first version id with its last character replaced by an out-of-alphabet character — AWS answers 400 InvalidArgument "Invalid version id specified"; the emulator answers 404 NoSuchVersion. Then DeleteObject that version and GetObject it again — both answer 404 NoSuchVersion.
Cause 2: create a bucket and call PutBucketAnalyticsConfiguration with an export whose S3BucketDestination.BucketAccountId is the empty string — AWS answers 400 MalformedXML and stores nothing; the emulator answers 204 and the configuration is listed. Controls: the same call with a 12-digit foreign account id, and with the member omitted, both succeed on AWS.
Cause 3: PutObject, create a KMS key, UpdateObjectEncryption to SSE-KMS with that key (succeeds), then call UpdateObjectEncryption with an ARN-shaped but malformed KMS key ARN — compare the AccessDenied message against "The KMS key must be from the same account as the bucket".
