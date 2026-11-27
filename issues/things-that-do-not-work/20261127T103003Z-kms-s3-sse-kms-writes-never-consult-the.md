Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.sse-kms-put-object-under-an-asymmetric-or-hmac-key-rejected kms__s3.upload-part-under-a-disabled-sse-kms-key-rejected
Issue-Key: kms__s3.sse-kms-put-object-under-an-asymmetric-or-hmac-key-rejected|run#7|AssertionError|raise AssertionError(
Issue-Key: kms__s3.upload-part-under-a-disabled-sse-kms-key-rejected|_await_terminal_refusal#16|Timeout|time.sleep(interval_s)
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__s3: SSE-KMS writes never consult the key — an asymmetric key is accepted on PutObject and UploadPart keeps succeeding after the key is disabled

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Two distinct root causes, each grounded in a failing behavioral check. The open issue 20261111T024602Z-kms-s3-sse-kms-writes-under-a-disabled.md reports a disabled-key write refused with the wrong code (KMS.KMSInvalidStateException); neither of the causes below is covered by it — here the write is not refused at all, and the key spec is never checked.

Cause 1 of 2: PutObject with SSE-KMS under an asymmetric key succeeds.
  AWS: recorded behavior only (S3 supports only symmetric encryption keys for SSE-KMS). A PutObject whose SSEKMSKeyId names an RSA_2048 ENCRYPT_DECRYPT key is refused with a 400 KMS-class error relayed by S3 (the key cannot generate a data key), and no object is written.
  Emulator: the write succeeds and the response reports SSEKMSKeyId '<rsa key arn>'.
  Why: a client is told its object is encrypted under a key that cannot possibly have wrapped a data key; the refusal AWS gives at write time never happens, so the invalid configuration is only discovered — if ever — on read.
  Fix: the s3 PutObject SSE-KMS path — resolve SSEKMSKeyId through KMS and require a symmetric encryption key, relaying KMS's refusal.
  Traces: kms__s3.sse-kms-put-object-under-an-asymmetric-or-hmac-key-rejected.

Cause 2 of 2: UploadPart on an SSE-KMS multipart upload keeps succeeding indefinitely after the key is disabled.
  AWS: recorded behavior only. S3 may serve a few parts off a cached data key right after DisableKey, but the state is terminal: UploadPart is then refused ('KMS.DisabledException', '<key arn> is disabled.') — fixed text with the key ARN — and the refused part does not land (ListParts shows the part uploaded before the disable and not the refused one).
  Emulator: UploadPart kept succeeding for the whole 60 s window the check polls (the suite cut the check off at its 60 s cap), so the terminal refusal never arrives.
  Why: a client relying on disabling a CMK to stop writes to data encrypted under it keeps accepting parts; the kill switch does not work, and no part is ever refused.
  Fix: the s3 UploadPart (and CompleteMultipartUpload) SSE-KMS path — resolve the upload's key per part through KMS and surface KMS's key-state refusal as KMS.DisabledException.
  Traces: kms__s3.upload-part-under-a-disabled-sse-kms-key-rejected.

## Evidence
kms__s3.sse-kms-put-object-under-an-asymmetric-or-hmac-key-rejected —
AssertionError: PutObject with SSE-KMS under a RSA_2048 (ENCRYPT_DECRYPT) key succeeded (SSEKMSKeyId 'arn:aws:kms:us-west-2:<account>:key/8466a458-...'); S3 supports only symmetric encryption keys.

kms__s3.upload-part-under-a-disabled-sse-kms-key-rejected —
Error: Check timed out after 60.0s. The check polls UploadPart every 3 s until it raises exactly ('KMS.DisabledException', '<key arn> is disabled.') and otherwise reports 'the call kept succeeding ... after the key was disabled'.

## Steps to Reproduce
1. Cause 1: create an RSA_2048 ENCRYPT_DECRYPT key; s3.put_object(Bucket=..., Key=..., ServerSideEncryption='aws:kms', SSEKMSKeyId=<rsa key arn>) -> expect a 400 KMS-class refusal and no object; observe 200 echoing SSEKMSKeyId.
2. Cause 2: create a symmetric key; s3.create_multipart_upload(..., ServerSideEncryption='aws:kms', SSEKMSKeyId=<key arn>); upload part 1; kms.disable_key; retry UploadPart every 3 s -> expect ('KMS.DisabledException', '<key arn> is disabled.') and ListParts without the refused part; observe UploadPart succeeding indefinitely.
