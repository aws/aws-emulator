Issue-Source: trace-sweep-divergence
Issue-Trace: s3-sse-c-copy-object-rekeys-an-object-to-a-new-customer-key
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-sse-c-copy-object-rekeys-an-object-to-a-new-customer-key s3-update-object-encryption-kms-key-arn-length-bounds
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-server-side-encryption-customer-algorithm
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-server-side-encryption-customer-key-md5
Issue-Key: s3|UpdateObjectEncryption|4xx->4xx|$.Error.Message
Issue-Key: s3|UpdateObjectEncryption|4xx->4xx|$.Error.Code
Issue-Key: s3|UpdateObjectEncryption|4xx->4xx|status
Issue-Related: 20261128T022809Z-s3-rejects-every-signed-request-that-carries-an.md
Issue-Related: 20261128T022809Z-s3-rejects-signed-requests-whose-header-values-carry.md
Issue-Related: 20261128T022809Z-s3-xml-well-formedness-refusals-are-skipped-or.md
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 server-side encryption key checks diverge: SSE-C reads accept and reject the wrong customer key and UpdateObjectEncryption reports the wrong KMS refusal

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
4 net-new causes sharing ONE fault: the emulator does not track which encryption key an object is sealed with, and does not validate a KMS key reference the way AWS does. After a CopyObject re-keys an SSE-C object to a new customer key, the read verdicts for the old and the new key are SWAPPED — the key that should work is refused and the key that should be refused succeeds — and on the SSE-KMS path the ARN-format and key-lookup refusals carry the wrong code, status and message.

Cause 1 of 4 — GetObject refuses the SSE-C customer key the object is now sealed with. AWS: once CopyObject has re-keyed the object to a new customer key, a GetObject presenting THAT key succeeds — 200 serving the object bytes with accept-ranges, last-modified, ETag, the checksum headers (x-amz-checksum-crc32 plus x-amz-checksum-type FULL_OBJECT), the object's content-type, and the SSE-C echo headers x-amz-server-side-encryption-customer-algorithm 'AES256' and x-amz-server-side-encryption-customer-key-md5 = the base64 MD5 of the key presented. Recorded behavior only (Smithy models no error for an SSE-C key mismatch). Emulator: 403 with Code 'AccessDenied' and Message 'Requests specifying Server Side Encryption with Customer provided keys must provide the correct secret key.' — which is the right envelope for the WRONG key, applied to the right one. Why: an SSE-C re-key is unusable; the caller holds the current key and can never read its own object again. Fix: the SSE-C key state must be updated by the CopyObject re-key path so the object read compares against the current key, and the read must echo the SSE-C algorithm and key-MD5 headers.

Cause 2 of 4 — GetObject serves the object to the SUPERSEDED SSE-C key. AWS: after the re-key, presenting the old customer key is refused 403 with Code 'AccessDenied' and the fixed message 'Requests specifying Server Side Encryption with Customer provided keys must provide the correct secret key.', served as content-type 'application/xml'. Emulator: 200 with the object bytes and content-type 'text/plain'. Why: this is the serious half — a key the owner rotated AWAY from still decrypts the object, so key rotation provides no revocation and a test asserting the old key is dead passes wrongly. Fix: as Cause 1 — one state fix corrects both verdicts, but both must be asserted. Traces: s3-sse-c-copy-object-rekeys-an-object-to-a-new-customer-key#task#6.

Cause 3 of 4 — UpdateObjectEncryption reports a generic ARN-parse sentence for a malformed KMS key ARN. AWS: the operation requires the FULL KMS key ARN; anything shorter or otherwise malformed is refused 400 with Message 'Invalid KMS Key ARN format. You must provide the full KMS Key ARN to make an UpdateObjectEncryption request' — fixed text, no variable part (Smithy UpdateObjectEncryption models InvalidRequest/400, so this is the wording of that class; recorded behavior only). Emulator: the same 400 but with its own sentence 'Requests modifying object encryption configuration to SSE-KMS require a valid target kms key arn: Invalid ARN - the ARN does not name the resource this member requires', which names an internal member requirement instead of telling the caller to supply the full ARN. Why: the message is the only thing that tells a caller a key id or alias is not enough here; the emulator's wording sends it to inspect its ARN parser instead. Fix: the UpdateObjectEncryption ARN validation's message. Traces: s3-update-object-encryption-kms-key-arn-length-bounds#task#4 (and #task#5, #task#7).

Cause 4 of 4 — UpdateObjectEncryption answers 403 AccessDenied where AWS answers 400 with a KMS lookup failure. AWS: a syntactically full ARN that does not resolve to a key is refused 400 with Code 'KMS.NotFoundException' and Message 'Invalid arn <region>' — fixed text 'Invalid arn ' followed by the region taken from the submitted ARN. Recorded behavior only. Emulator: 403 with Code 'AccessDenied' and 'The KMS key must be from the same account as the bucket'. Why: the status class and the code both differ, so a client cannot distinguish 'this key does not exist' (fix the reference) from 'you are not allowed to use it' (fix the policy), and the account-mismatch diagnosis is simply not what happened. Fix: the UpdateObjectEncryption KMS-key resolution path must surface the lookup failure as the modeled 400 rather than collapsing every key problem into a 403. Traces: s3-update-object-encryption-kms-key-arn-length-bounds#task#6.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.
Representative s3-sse-c-copy-object-rekeys-an-object-to-a-new-customer-key#task#5 (GetObject with the CURRENT customer key): status expected 200, got 403; body expected the object bytes, got {'Error': {'Code': 'AccessDenied', 'Message': 'Requests specifying Server Side Encryption with Customer provided keys must provide the correct secret key.', ...}}; accept-ranges, last-modified, etag, x-amz-checksum-crc32, x-amz-checksum-type, x-amz-server-side-encryption-customer-algorithm ('AES256') and x-amz-server-side-encryption-customer-key-md5 all missing; content-type expected 'text/plain' got 'application/xml'.
- #task#6 (GetObject with the SUPERSEDED key): status expected 403, got 200 — expected the AccessDenied envelope above, got the object bytes; content-type expected 'application/xml' got 'text/plain'.
- s3-update-object-encryption-kms-key-arn-length-bounds#task#4 (UpdateObjectEncryption, 400/400): $.Error.Message expected 'Invalid KMS Key ARN format. You must provide the full KMS Key ARN to make an UpdateObjectEncryption request', got 'Requests modifying object encryption configuration to SSE-KMS require a valid target kms key arn: Invalid ARN - the ARN does not name the resource this member requires' (same divergence at #task#5 and #task#7).
- #task#6 (UpdateObjectEncryption): status expected 400, got 403; $.Error.Code expected 'KMS.NotFoundException' got 'AccessDenied'; Message expected 'Invalid arn <region>' (fixed prefix plus the region from the submitted ARN), got 'The KMS key must be from the same account as the bucket'.

## Steps to Reproduce
1. Replay fixture s3-sse-c-copy-object-rekeys-an-object-to-a-new-customer-key. After the CopyObject re-key, task#5's GetObject presenting the NEW customer key must be 200 with the object bytes and must echo x-amz-server-side-encryption-customer-algorithm and x-amz-server-side-encryption-customer-key-md5; task#6's GetObject presenting the OLD key must be refused 403 AccessDenied with the fixed SSE-C secret-key sentence, served as application/xml.
2. Replay s3-update-object-encryption-kms-key-arn-length-bounds. task#4/#5/#7 must answer the 400 whose message is the full-KMS-Key-ARN sentence; task#6's unresolvable but well-formed ARN must be refused 400 with Code KMS.NotFoundException and the 'Invalid arn <region>' message, not 403 AccessDenied.
