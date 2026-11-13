Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected
Issue-Key: kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected|_poll#11|Timeout|time.sleep(interval_s)
Issue-Commit: e6eec4415380d0861b3cd74514fe77af8743d21c

# kms__s3: S3 GetObject ignores the SSE-KMS CMK's key state — an object whose key is disabled is still served instead of being refused KMS.DisabledException

> _Note: this issue was identified against the code base at commit `e6eec4415380d0861b3cd74514fe77af8743d21c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause (Cause 1 of 1), grounded in a failing check that no open issue carries.

Cause 1 of 1: an SSE-KMS object stays readable after its CMK is disabled, because S3's read path never resolves the object's SSEKMSKeyId against KMS key state.
AWS: Smithy kms#DisableKey moves a key to KeyState=Disabled (kms#KeyState enum: Creating, Enabled, Disabled, PendingDeletion, ...), and kms#DisabledException is a client error documented as 'The request was rejected because the specified KMS key is not enabled.'. An object written with s3#PutObject ServerSideEncryption=aws:kms and SSEKMSKeyId=<key-arn> can only be read by calling kms:Decrypt for that key, so the read must fail while the key is disabled. Recorded against real AWS (the check's blessed 2026-09-22 measurement): GetObject of that object is refused HTTP 400 with error code 'KMS.DisabledException'; HeadObject does NOT call KMS and keeps succeeding, still reporting SSEKMSKeyId=<key-arn> and the object's ContentLength; the object stays listable in ListObjectsV2; an object in the same bucket under a different, still-enabled key keeps returning its exact bytes; and the refusal lifts after kms#EnableKey, when the same GetObject returns the exact original bytes again. Message: the refusal carries KMS's DisabledException text, whose fixed part is the 'is disabled' class text and whose variable part is the disabled key's ARN taken from the object's stored SSEKMSKeyId. Hypothesis: the literal wording is '<key-arn> is disabled.' — the status (400) and the code (KMS.DisabledException) are from the recording, the exact literal is not.
Emulator: no refusal ever appears. After kms.disable_key(KeyId=<key A>), GetObject of key A's object keeps succeeding, so the check's bounded 90 s poll for 'KMS.DisabledException' never converges and the check is cut off at the harness cap ('Error: Check timed out after 60.0s'). This is the behavior of an emulator that stores SSE-KMS objects in plaintext and merely records the key id: nothing in the read path consults KMS.
Why: key-state revocation is the single control SSE-KMS exists for — disabling a CMK is how an operator makes the data it protects immediately unreadable. Here the data keeps being served, so that control cannot be verified and reads AWS would refuse succeed; a client written to handle KMS.DisabledException/400 (and to keep trusting HeadObject, which must still work) never sees it.
Fix: the SSE-KMS branch of S3's GetObject read path — resolve the object's stored SSEKMSKeyId against KMS and fail the read while the key's state is not Enabled, leaving HeadObject, ListObjectsV2 and objects under other keys unaffected (services/service-com-amazonaws-s3-amazons3/src/ops/get_object.rs plus the KMS key-state lookup). Related open issues: Cause 2 of 20261025T032458Z-s3-remaining-divergences-8-root-causes-selectobjectcontent-routing.md describes the s3.* twin of this symptom, and 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md carries other kms__s3 checks; neither carries this check id, and this check additionally pins the per-key scoping (sibling key unaffected) and the HeadObject-still-succeeds half of the contract.
Traces: kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected

## Evidence
Replay verdict for kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected: `Error: Check timed out after 60.0s`. The check creates two CMKs, writes one SSE-KMS object under each, confirms both read back under their own key, then calls kms.disable_key(KeyId=<A>) and polls GetObject of A's object for up to 90 s for the outcome 'KMS.DisabledException' (`_poll(... lambda o: o == "KMS.DisabledException" ...)`); the poll never converges because the emulator keeps returning 200 with the object's bytes, so the check is cut off at the 60 s cap before its later assertions (ListObjectsV2 still lists both keys, HeadObject still reports SSEKMSKeyId and ContentLength, the sibling key's object still reads, and the bytes return after EnableKey). Required instead: HTTP 400 with error code KMS.DisabledException on the GetObject of the disabled key's object only.

## Steps to Reproduce
1) kms.create_key() twice -> key A, key B. 2) CreateBucket. 3) PutObject a.bin with ServerSideEncryption='aws:kms', SSEKMSKeyId=<A>, and b.bin with SSEKMSKeyId=<B>. 4) GetObject both and confirm the bytes and SSEKMSKeyId. 5) kms.disable_key(KeyId=<A>). 6) GetObject a.bin — the emulator returns 200 with the bytes; AWS refuses HTTP 400 code KMS.DisabledException. 7) HeadObject a.bin (must still succeed, reporting SSEKMSKeyId=<A>) and GetObject b.bin (must still return its bytes). 8) kms.enable_key(KeyId=<A>) and GetObject a.bin — the bytes must come back. Replay check kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected to confirm the fix.
