Issue-Source: trace-sweep-divergence
Issue-Trace: kms-data-key-generation-ignores-grant-tokens-for-a-key-policy-caller
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-data-key-generation-ignores-grant-tokens-for-a-key-policy-caller kms-decrypt-unusable-grant-token-ignored-under-key-policy kms-generate-mac-accepts-unissued-grant-token-under-key-policy kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller kms-signing-key-dry-run-verify-refused-and-grant-tokens-ignored kms-unrecognised-grant-token-ignored-for-authorized-caller kms-verify-accepts-unissued-grant-token-under-key-policy
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GenerateDataKey|2xx->4xx|$.CiphertextBlob
Issue-Key: kms|GenerateDataKey|2xx->4xx|$.KeyId
Issue-Key: kms|GenerateDataKey|2xx->4xx|$.KeyMaterialId
Issue-Key: kms|GenerateDataKey|2xx->4xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKey|2xx->4xx|$.Plaintext
Issue-Key: kms|GenerateDataKey|2xx->4xx|status
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.KeyId
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.KeyMaterialId
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.KeyPairSpec
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.PrivateKeyCiphertextBlob
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.PrivateKeyPlaintext
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|$.PublicKey
Issue-Key: kms|GenerateDataKeyPair|2xx->4xx|status
Issue-Key: kms|Decrypt|2xx->4xx|$.EncryptionAlgorithm
Issue-Key: kms|Decrypt|2xx->4xx|$.KeyId
Issue-Key: kms|Decrypt|2xx->4xx|$.KeyMaterialId
Issue-Key: kms|Decrypt|2xx->4xx|$.KeyOrigin
Issue-Key: kms|Decrypt|2xx->4xx|$.Plaintext
Issue-Key: kms|Decrypt|2xx->4xx|status
Issue-Key: kms|GenerateMac|2xx->4xx|$.KeyId
Issue-Key: kms|GenerateMac|2xx->4xx|$.KeyOrigin
Issue-Key: kms|GenerateMac|2xx->4xx|$.Mac
Issue-Key: kms|GenerateMac|2xx->4xx|$.MacAlgorithm
Issue-Key: kms|GenerateMac|2xx->4xx|status
Issue-Key: kms|GenerateMac|2xx->2xx|$.Mac
Issue-Key: kms|VerifyMac|2xx->4xx|$.KeyId
Issue-Key: kms|VerifyMac|2xx->4xx|$.KeyOrigin
Issue-Key: kms|VerifyMac|2xx->4xx|$.MacAlgorithm
Issue-Key: kms|VerifyMac|2xx->4xx|$.MacValid
Issue-Key: kms|VerifyMac|2xx->4xx|status
Issue-Key: kms|VerifyMac|4xx->4xx|$.__type
Issue-Key: kms|VerifyMac|4xx->4xx|$.message
Issue-Key: kms|Sign|2xx->4xx|$.KeyId
Issue-Key: kms|Sign|2xx->4xx|$.KeyOrigin
Issue-Key: kms|Sign|2xx->4xx|$.Signature
Issue-Key: kms|Sign|2xx->4xx|$.SigningAlgorithm
Issue-Key: kms|Sign|2xx->4xx|status
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|2xx->4xx|$.CiphertextBlob
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|2xx->4xx|$.KeyId
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|2xx->4xx|$.KeyMaterialId
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|2xx->4xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|2xx->4xx|status
Issue-Key: kms|ReEncrypt|2xx->4xx|$.CiphertextBlob
Issue-Key: kms|ReEncrypt|2xx->4xx|$.DestinationEncryptionAlgorithm
Issue-Key: kms|ReEncrypt|2xx->4xx|$.DestinationKeyMaterialId
Issue-Key: kms|ReEncrypt|2xx->4xx|$.DestinationKeyOrigin
Issue-Key: kms|ReEncrypt|2xx->4xx|$.KeyId
Issue-Key: kms|ReEncrypt|2xx->4xx|$.SourceEncryptionAlgorithm
Issue-Key: kms|ReEncrypt|2xx->4xx|$.SourceKeyId
Issue-Key: kms|ReEncrypt|2xx->4xx|$.SourceKeyMaterialId
Issue-Key: kms|ReEncrypt|2xx->4xx|$.SourceKeyOrigin
Issue-Key: kms|ReEncrypt|2xx->4xx|status
Issue-Key: kms|Verify|2xx->4xx|$.KeyId
Issue-Key: kms|Verify|2xx->4xx|$.KeyOrigin
Issue-Key: kms|Verify|2xx->4xx|$.SignatureValid
Issue-Key: kms|Verify|2xx->4xx|$.SigningAlgorithm
Issue-Key: kms|Verify|2xx->4xx|status
Issue-Related: 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# kms: grant tokens narrow authorization instead of adding to it — authorized data-plane calls are refused 400

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
ONE root cause, 11 divergence clusters (replayed from the wire corpus). All 11 fixture#step bindings listed under Traces must replay clean before this is closed — the representative binding alone does not exercise the cause, because each cluster is a different operation reaching the same shared authorization decision.

Cause 1 of 1: a request that presents GrantTokens has its permission taken FROM those grants instead of IN ADDITION TO everything else that authorizes it, so a caller the key policy already permits is refused as soon as it carries a token the service cannot resolve to a relevant grant.

  AWS: Smithy: every affected operation models GrantTokens (GrantTokenList, @length(0..10)), documented as 'A list of grant tokens. Use a grant token when your permission to call this operation comes from a new grant that has not yet achieved eventual consistency' — a grant token is an ADDITIONAL source of permission, never a restriction of the ones already in force. InvalidGrantTokenException ('The request was rejected because the specified grant token is not valid.', httpError 400) is for a token that is not a usable token at all, not for a well-formed token that happens to name no grant on this key. The recordings show the authorizing behavior directly: with the key policy already allowing the caller, a call carrying an unissued / stale / unparseable GrantTokens entry succeeds exactly as the tokenless call does and answers the full cryptographic output — 200 with CiphertextBlob / Plaintext / Mac / Signature / SignatureValid / MacValid, KeyId as the key ARN, KeyMaterialId, and the operation's algorithm member (SYMMETRIC_DEFAULT, HMAC_SHA_256, ECC_NIST_P256, ECDSA_SHA_256 per fixture).
  Emulator: the request is refused 400. The crate states the divergence in its own words — services/service-com-amazonaws-kms-trentservice/src/ops/encrypt.rs lines 22-29: 'a request that DOES present a grant token is treated as drawing its permission from the grants those tokens name ... the effect here is narrower than AWS: an otherwise-permitted call that carries an unrelated or stale grant token is refused'. In the handlers (encrypt.rs:339-364, decrypt.rs:556-581 and the same block in every other data-plane op) state.grants_for_tokens() answers GrantRefusal::MalformedToken -> 400 InvalidGrantTokenException with the fixed text 'The specified grant token is not valid.', and a resolvable-but-irrelevant token makes `presented` the only permission authorize() will consider, so the call ends in a 400 access denial. The replay observed the code directly on one step: kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller#task#4 answered $.__type 'InvalidGrantTokenException' with $.message 'The specified grant token is not valid.' Affected operations in this corpus: Decrypt, GenerateDataKey, GenerateDataKeyPair, GenerateDataKeyWithoutPlaintext, GenerateMac, ReEncrypt, Sign, Verify, VerifyMac.
  Why: a client that caches grant tokens, or passes the token it got from CreateGrant alongside a request its key policy already allows (the normal, documented usage), is hard-refused where AWS serves it. Nothing the caller can do to the request body makes the emulator behave like AWS short of dropping the token — so KMS-fronted SDK code that always forwards tokens cannot run at all, and callers that distinguish 'bad token' from 'not allowed' read the wrong error class.
  Fix: the shared authorization path — crate::state::authorization::authorize plus state.grants_for_tokens in services/service-com-amazonaws-kms-trentservice/src/state/authorization.rs and state/grants.rs, and the identical token block each ops/<op>.rs copies. Presented grants must be unioned with the key policy, the caller's identity policies and the grants the caller already holds; an unresolvable or irrelevant token must not fail the request, and only a token that cannot be parsed as a token at all may answer InvalidGrantTokenException.
  Two follow-on assertions the same fix must satisfy, because the token refusal currently pre-empts them: (a) kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller#task#4 must then refuse the algorithm/key-spec mismatch with $.__type 'InvalidKeyUsageException' and $.message 'Algorithm HMAC_SHA_384 is incompatible with key spec HMAC_256.' — fixed text 'Algorithm ' / ' is incompatible with key spec ' / '.' with the MacAlgorithm from the request and the key spec from state; (b) kms-generate-mac-accepts-unissued-grant-token-under-key-policy#task#4 must return the SAME Mac the fixture's step 2 produced (it diverges today only because step 2 was refused, so the recorded {{mac}} capture never happened).
  Traces: kms-data-key-generation-ignores-grant-tokens-for-a-key-policy-caller#task#3 (GenerateDataKey, representative; cluster also covers task#4), kms-data-key-generation-ignores-grant-tokens-for-a-key-policy-caller#task#7 (GenerateDataKeyPair), kms-decrypt-unusable-grant-token-ignored-under-key-policy#task#3 (Decrypt), kms-generate-mac-accepts-unissued-grant-token-under-key-policy#task#2 (GenerateMac, cluster also covers task#3), kms-generate-mac-accepts-unissued-grant-token-under-key-policy#task#4 (GenerateMac, the cascaded Mac comparison), kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller#task#3 (VerifyMac), kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller#task#4 (VerifyMac, masked InvalidKeyUsageException), kms-signing-key-dry-run-verify-refused-and-grant-tokens-ignored#task#3 (Sign), kms-unrecognised-grant-token-ignored-for-authorized-caller#task#5 (GenerateDataKeyWithoutPlaintext, cluster also covers task#6), kms-unrecognised-grant-token-ignored-for-authorized-caller#task#10 (ReEncrypt), kms-verify-accepts-unissued-grant-token-under-key-policy#task#3 (Verify, cluster also covers task#4).

Related but does NOT cover these steps: the open behavioral issue 20261127T103003Z-kms-4-causes-unrecognized-grant-tokens-are-refused.md describes the same fault from the behavioral suite (Issue-Corpus simparity-behavioral-aws-v1) and carries no wire replay key, so none of the fixture steps above is exercised by it.

## Evidence
All statuses/bodies below are from the wire replay (trace set simparity-wire-aws-v1).

kms-data-key-generation-ignores-grant-tokens-for-a-key-policy-caller#task#3 (GenerateDataKey) — expected 200, got 400; $.CiphertextBlob, $.Plaintext, $.KeyId ('arn:aws:kms:<region>:<account>:key/<key-id>'), $.KeyMaterialId, $.KeyOrigin ('AWS_KMS') all absent from the 400 body. Same cluster: task#4.
kms-data-key-generation-ignores-grant-tokens-for-a-key-policy-caller#task#7 (GenerateDataKeyPair) — expected 200, got 400; $.PrivateKeyCiphertextBlob, $.PrivateKeyPlaintext, $.PublicKey, $.KeyId, $.KeyMaterialId, $.KeyPairSpec ('ECC_NIST_P256'), $.KeyOrigin absent.
kms-decrypt-unusable-grant-token-ignored-under-key-policy#task#3 (Decrypt) — expected 200, got 400; $.Plaintext, $.KeyId, $.KeyMaterialId, $.EncryptionAlgorithm ('SYMMETRIC_DEFAULT'), $.KeyOrigin absent.
kms-generate-mac-accepts-unissued-grant-token-under-key-policy#task#2 (GenerateMac) — expected 200, got 400; $.Mac, $.MacAlgorithm ('HMAC_SHA_256'), $.KeyId, $.KeyOrigin absent. Same cluster: task#3.
kms-generate-mac-accepts-unissued-grant-token-under-key-policy#task#4 (GenerateMac) — 200/200 but $.Mac value_mismatch: expected the Mac captured at task#2 ({{mac}}), got a freshly computed one, because task#2 never returned a Mac.
kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller#task#3 (VerifyMac) — expected 200, got 400; $.MacValid (true), $.MacAlgorithm, $.KeyId, $.KeyOrigin absent.
kms-hmac-key-verify-mac-ignores-grant-tokens-for-a-key-policy-caller#task#4 (VerifyMac) — 400/400 but $.__type value_mismatch: expected 'InvalidKeyUsageException', got 'InvalidGrantTokenException'; $.message expected 'Algorithm HMAC_SHA_384 is incompatible with key spec HMAC_256.', got 'The specified grant token is not valid.'
kms-signing-key-dry-run-verify-refused-and-grant-tokens-ignored#task#3 (Sign) — expected 200, got 400; $.Signature, $.SigningAlgorithm ('ECDSA_SHA_256'), $.KeyId, $.KeyOrigin absent.
kms-unrecognised-grant-token-ignored-for-authorized-caller#task#5 (GenerateDataKeyWithoutPlaintext) — expected 200, got 400; $.CiphertextBlob, $.KeyId, $.KeyMaterialId, $.KeyOrigin absent. Same cluster: task#6.
kms-unrecognised-grant-token-ignored-for-authorized-caller#task#10 (ReEncrypt) — expected 200, got 400; $.CiphertextBlob, $.KeyId, $.SourceKeyId, $.SourceKeyMaterialId, $.DestinationKeyMaterialId, $.SourceEncryptionAlgorithm and $.DestinationEncryptionAlgorithm ('SYMMETRIC_DEFAULT'), $.SourceKeyOrigin/$.DestinationKeyOrigin absent.
kms-verify-accepts-unissued-grant-token-under-key-policy#task#3 (Verify) — expected 200, got 400; $.SignatureValid (true), $.SigningAlgorithm, $.KeyId, $.KeyOrigin absent. Same cluster: task#4.

Source confirming the cause: services/service-com-amazonaws-kms-trentservice/src/ops/encrypt.rs:22-29 (documented divergence), encrypt.rs:339-364 and decrypt.rs:556-581 (grants_for_tokens -> InvalidGrantTokenException, then `presented` passed to authorize as the only permission).

## Steps to Reproduce
1. Create a symmetric ENCRYPT_DECRYPT key whose key policy authorizes the calling principal (no grant needed).
2. Call kms GenerateDataKey with KeyId=<key arn>, KeySpec='AES_256' and GrantTokens=['<a token this key never issued / an unparseable string>'].
   Expect: 200 with CiphertextBlob, Plaintext, KeyId=<key arn>, KeyMaterialId. Observe: 400 (InvalidGrantTokenException 'The specified grant token is not valid.' / a 400 access denial).
3. Repeat with the same shape of request on Decrypt, GenerateDataKeyPair, GenerateDataKeyWithoutPlaintext, GenerateMac (HMAC_256 key), ReEncrypt, Sign and Verify (ECC_NIST_P256 SIGN_VERIFY key), VerifyMac — each is refused 400 where AWS answers 200.
4. On an HMAC_256 key, call VerifyMac with MacAlgorithm='HMAC_SHA_384' and an unissued grant token: expect 400 InvalidKeyUsageException 'Algorithm HMAC_SHA_384 is incompatible with key spec HMAC_256.'; observe 400 InvalidGrantTokenException.
5. Replay the 11 fixture#step bindings listed in Traces.
