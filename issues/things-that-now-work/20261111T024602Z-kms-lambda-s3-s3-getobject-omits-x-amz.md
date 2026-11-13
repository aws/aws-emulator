Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn
Issue-Key: kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn|run#14|AssertionError|assert payload[obj_key]["sse_key"] == key["arn"], (
Issue-Commit: 6a777fbd6800e714176d50f5d7d416617b079449

# kms__lambda__s3: S3 GetObject omits x-amz-server-side-encryption-aws-kms-key-id for an SSE-KMS object (HeadObject reports it)

> _Note: this issue was identified against the code base at commit `6a777fbd6800e714176d50f5d7d416617b079449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: a successful GetObject of an SSE-KMS object returns the encryption algorithm but no key identifier, so the reader cannot learn which CMK protects the object.

AWS: GetObject's response carries both SSE members (Smithy: s3 GetObjectOutput.ServerSideEncryption -> httpHeader x-amz-server-side-encryption and GetObjectOutput.SSEKMSKeyId -> httpHeader x-amz-server-side-encryption-aws-kms-key-id). For an object written with ServerSideEncryption=aws:kms, S3 reports the RESOLVED, canonical key ARN of the CMK — the same value HeadObject reports — regardless of whether the writer passed SSEKMSKeyId as a bare key id or as a full ARN. Recorded behavior: the reader (boto3 inside a Lambda whose role has s3:GetObject + kms:Decrypt on that key) sees resp['ServerSideEncryption'] == 'aws:kms' and resp['SSEKMSKeyId'] == the key's ARN for both objects.

Emulator: the response includes ServerSideEncryption 'aws:kms' but SSEKMSKeyId is absent (None), while HeadObject on the same object DOES report the resolved key ARN (the check's HeadObject assertions pass earlier in the same run). So the key identity is tracked and serialized on HEAD but dropped from the GET response.

Why: a client that reads an object and must act on its encryption key — audit which CMK protected a payload, re-encrypt under the same key, or assert that a cross-account/by-id reference resolved to the expected canonical ARN — gets nothing from GetObject and must issue a second HeadObject, and code that keys on resp['SSEKMSKeyId'] sees None (or raises KeyError) where AWS gives an ARN.

Fix: the GetObject response serializer / its SSE header emission for the object-read path (it must emit x-amz-server-side-encryption-aws-kms-key-id with the stored resolved key ARN, as the HeadObject path already does).

Traces: kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn.

## Evidence
Check kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn. Two objects are written to the same bucket with ServerSideEncryption='aws:kms', one passing SSEKMSKeyId=<key ARN> ('by-arn.txt'), one passing SSEKMSKeyId=<bare key id> ('by-id.txt'). HeadObject on both already asserts ServerSideEncryption 'aws:kms' and SSEKMSKeyId == the resolved key ARN, and those assertions pass. The Lambda reader then does s3.get_object(...) and reports {body, ServerSideEncryption, SSEKMSKeyId}:

Expected (AWS): {'body': '<payload>', 'sse': 'aws:kms', 'sse_key': '<arn:aws:kms:<region>:<account>:key/<key-id>>'}
Actual (emulator): AssertionError: by-arn.txt: GetObject must report the resolved key ARN to the reader too: {'body': 'sim2real-xsvc-sse-kms-by-arn', 'sse': 'aws:kms', 'sse_key': None}

The body and the algorithm are correct; only the key-id header is missing. The expected value is the canonical key ARN of the object's CMK taken from the emulator's state, not a literal from this run.

## Steps to Reproduce
1. CreateKey (symmetric CMK); note its KeyId and Arn. CreateBucket.
2. PutObject(Key='by-arn.txt', ServerSideEncryption='aws:kms', SSEKMSKeyId=<key ARN>) and PutObject(Key='by-id.txt', ServerSideEncryption='aws:kms', SSEKMSKeyId=<bare key id>).
3. HeadObject both: ServerSideEncryption=='aws:kms' and SSEKMSKeyId==<key ARN> (passes today).
4. GetObject both (directly, or from a Lambda whose role has s3:GetObject on the bucket and kms:Decrypt on the key).
   -> AWS: the response carries x-amz-server-side-encryption-aws-kms-key-id == <key ARN> (boto3 resp['SSEKMSKeyId']).
   -> Emulator: the header is absent, resp.get('SSEKMSKeyId') is None.
