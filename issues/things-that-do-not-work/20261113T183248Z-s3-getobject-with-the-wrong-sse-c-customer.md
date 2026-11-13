Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-ssec-and-endpoint-enforcement-ssec-multipart-upload-round-trips-only-with-the-customer-key s3.s3-ssec-and-endpoint-enforcement-ssec-upload-part-copy-rekeys-an-ssec-source-into-a-multipart-object
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-ssec-multipart-upload-round-trips-only-with-the-customer-key|run#37|AssertionError|assert (code, status, msg) == ("AccessDenied", 403, _WRONG_SECRET), (
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-ssec-upload-part-copy-rekeys-an-ssec-source-into-a-multipart-object|run#29|AssertionError|assert (code, status, msg) == ("AccessDenied", 403, _WRONG_SECRET), (
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# s3: GetObject with the wrong SSE-C customer key answers the generic 'Access Denied' message instead of AWS's SSE-C wording

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
1 net-new root cause in the SSE-C read path. Related open issues cover neighbouring SSE-C causes but carry neither of these replay keys: 20261110T210333Z-s3-6-net-new-causes-in-the-sse.md, 20261103T040901Z-s3-10-more-net-new-root-causes-sse.md.

Cause 1 of 1: the wrong-customer-key refusal loses AWS's message.
AWS: reading an SSE-C object with a syntactically valid but wrong customer key is refused 403 AccessDenied with the fixed message "Requests specifying Server Side Encryption with Customer provided keys must provide the correct secret key." — no values from the request appear in it. This holds for a plain SSE-C object, for one assembled by multipart upload, and for one re-keyed by UploadPartCopy (reading it with the SOURCE's key is refused the same way). Smithy (s3): GetObject input SSECustomerAlgorithm/SSECustomerKey/SSECustomerKeyMD5; the model states no message text, so the sentence is recorded AWS behavior.
Emulator: the code and status are right (AccessDenied/403) but the message is the generic 'Access Denied'.
Why: 'Access Denied' is the message S3 uses for an authorization failure, so a client (or an operator reading a log) cannot tell a bad SSE-C key from a missing s3:GetObject grant — the two have completely different fixes — and any handling that matches AWS's sentence matches nothing.
Fix: the SSE-C key-match check's error message in the GetObject handler of the s3 service (same template for the multipart-assembled and UploadPartCopy-re-keyed objects).
Traces: s3.s3-ssec-and-endpoint-enforcement-ssec-multipart-upload-round-trips-only-with-the-customer-key, s3.s3-ssec-and-endpoint-enforcement-ssec-upload-part-copy-rekeys-an-ssec-source-into-a-multipart-object.

## Evidence
Cause 1: GetObject with the wrong key returned 'AccessDenied'/403 ('Access Denied'); expected AccessDenied/403 with 'Requests specifying Server Side Encryption with Customer provided keys must provide the correct secret key.' — reported identically for the multipart-assembled SSE-C object and for reading the UploadPartCopy-re-keyed object with the source's key.

## Steps to Reproduce
Cause 1: CreateMultipartUpload/UploadPart/CompleteMultipartUpload an object with SSECustomerAlgorithm='AES256' and key K1; call GetObject with SSECustomerAlgorithm='AES256' and a different valid key K2 and read Error.Message. Repeat with an object re-keyed via UploadPartCopy from an SSE-C source, reading it back with the source's key.
