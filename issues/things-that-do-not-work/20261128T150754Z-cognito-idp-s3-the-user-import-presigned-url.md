Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__s3.import-csv-put-without-kms-encryption-header-rejected cognito-idp__s3.user-auth-events-stream-sign-ins-into-s3-bucket
Issue-Key: cognito-idp__s3.import-csv-put-without-kms-encryption-header-rejected|run#19|AssertionError|assert status == 403, f"a PUT without the encryption header answered {status}: {raw[:300]!r}"
Issue-Key: cognito-idp__s3.user-auth-events-stream-sign-ins-into-s3-bucket|run#24|AssertionError|assert counts == {"Pass": 1, "Fail": 1}, (
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp__s3: the user-import presigned URL ignores its signed SSE header and user auth events are never delivered to the bucket

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserImportJob

## Description
Two defects where cognito-idp's S3-facing surface diverges.

Cause 1 of 2: the import job's presigned upload URL does not bind the headers it signs.
AWS: CreateUserImportJob returns a PreSignedUrl that signs x-amz-server-side-encryption: aws:kms. A PUT that omits that header is refused by S3 with HTTP 403 and the error code SignatureDoesNotMatch, message 'The request signature we calculated does not match the signature you provided. Check your key and signing method.' — fixed text, no request values — the response carries no x-amz-server-side-encryption header, nothing is stored, and StartUserImportJob then refuses PreconditionNotMetException 'No csv file was uploaded for <job id>' with the job still 'Created'. The same PUT WITH the header answers 200 and S3 reports x-amz-server-side-encryption: aws:kms plus x-amz-server-side-encryption-aws-kms-key-id.
Emulator: the header-less PUT answers HTTP 200 with an empty body, so the CSV is stored unencrypted and the signature's signed-header set is never verified.
Why: a client that forgets the documented header gets a silent success where AWS fails it, and the import CSV is stored without the KMS encryption the URL demands.
Fix: the presigned-URL verification for the import upload (signed headers must be part of the signature check) in the user-import upload endpoint.
Traces: cognito-idp__s3.import-csv-put-without-kms-encryption-header-rejected.

Cause 2 of 2: user auth events are not streamed to the configured S3 bucket.
AWS: with the pool's log-delivery/threat-protection stream configured to an S3 bucket, a successful and a failed sign-in each land as a record in the bucket, keyed by outcome (one Pass and one Fail).
Emulator: after 30 seconds the bucket holds 0 records.
Why: a client that consumes sign-in telemetry from S3 sees nothing, so no downstream pipeline can be exercised.
Fix: the auth-event delivery path that writes to the configured S3 destination.
Traces: cognito-idp__s3.user-auth-events-stream-sign-ins-into-s3-bucket.

## Evidence
Cause 1 — "AssertionError: a PUT without the encryption header answered 200: b''" (the check requires status 403 and the S3 error ('SignatureDoesNotMatch', 'The request signature we calculated does not match the signature you provided. Check your key and signing method.'), then PreconditionNotMetException 'No csv file was uploaded for <job id>' and job Status 'Created').
Cause 2 — "after 30s the bucket holds the user's sign-ins by outcome {} among 0 record(s), expected one Pass and one Fail".

## Steps to Reproduce
1. Replay cognito-idp__s3.import-csv-put-without-kms-encryption-header-rejected: CreateUserPool, GetCSVHeader, CreateUserImportJob, then PUT the CSV to PreSignedUrl with only Content-Type (no x-amz-server-side-encryption) — AWS answers 403 SignatureDoesNotMatch; repeat with the header and expect 200 plus the SSE response headers.
2. Replay cognito-idp__s3.user-auth-events-stream-sign-ins-into-s3-bucket: configure the pool's auth-event stream to an S3 bucket, perform one successful and one failed sign-in, then list the bucket.
