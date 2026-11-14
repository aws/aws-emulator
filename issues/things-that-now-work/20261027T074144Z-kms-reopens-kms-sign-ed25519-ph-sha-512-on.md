Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.sign-verify-ed25519-raw-and-prehash
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|Sign|NotImplemented
Issue-Commit: ecf0c4efe25bcf25a3f7aae9bca0eda53779d0ad

# REOPENS kms Sign: ED25519_PH_SHA_512 on an ECC_NIST_EDWARDS25519 key is still refused 501 NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `ecf0c4efe25bcf25a3f7aae9bca0eda53779d0ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Sign

## Description
REOPENS 20261024T102105Z-kms-sign-stub.md: its fix left the call refused.

Cause 1 of 1: Sign refuses the pre-hash Ed25519 signing algorithm with the scaffold's NotImplemented, even though the key it is asked to use is exactly the key spec that algorithm belongs to. Failing check: kms.sign-verify-ed25519-raw-and-prehash.

AWS: Sign(KeyId=<an ECC_NIST_EDWARDS25519 SIGN_VERIFY key>, Message=<the SHA-512 digest of the message>, MessageType=DIGEST, SigningAlgorithm=ED25519_PH_SHA_512) is a supported, fully modeled cryptographic operation and answers HTTP 200 with KeyId (the key ARN), SigningAlgorithm and Signature — a 64-byte Ed25519 signature that differs from the pure-Ed25519 (ED25519_SHA_512 / MessageType=RAW) signature over the same underlying message, and that the same key's Verify accepts. Smithy (com-amazonaws-kms-trentservice): SigningAlgorithmSpec values include both ED25519_SHA_512 and ED25519_PH_SHA_512; KeySpec values include ECC_NIST_EDWARDS25519; Sign.SigningAlgorithm is @required, Sign.Message is @required @length 1..4096, Sign.MessageType is the MessageType enum RAW|DIGEST|EXTERNAL_MU. Sign's modeled error set is exactly DependencyTimeoutException(503), DisabledException(409), DryRunOperationException(412), InvalidGrantTokenException(400), InvalidKeyUsageException(400), KeyUnavailableException(500), KMSInternalException(500), KMSInvalidStateException(409), NotFoundException(404) — there is no NotImplemented error in the model, and this key/algorithm pair is not an InvalidKeyUsageException case either, because the key's own GetPublicKey correctly advertises SigningAlgorithms {ED25519_SHA_512, ED25519_PH_SHA_512}.

Emulator: the request is refused with HTTP 501, error code `NotImplemented`, message `operation Sign with the ED25519_PH_SHA_512 signing algorithm and a KMS key whose KeySpec is ECC_NIST_EDWARDS25519 is scaffolded but not implemented in this emulator`. Of that message, only the fixed scaffold text is template; `ED25519_PH_SHA_512` is the request's SigningAlgorithm and `ECC_NIST_EDWARDS25519` is the resolved key's KeySpec, i.e. values taken from the request and from state. The pure-Ed25519 path is NOT affected — the same check signs successfully with ED25519_SHA_512/MessageType=RAW and only the pre-hash call is refused — so the gap is the Ed25519ph scheme itself, not the operation: Sign's own request walk completes every check (key resolution, key state, grants/authorization, KeyUsage, the key-spec algorithm list, the MessageType cross-check, the DIGEST length check) and then the signing backend reports the scheme as unsupported.

Why: a client that signs with ED25519_PH_SHA_512 — the only way to sign a message larger than 4096 bytes with an Ed25519 KMS key, since the caller must pre-hash and submit the digest — gets a 501 with an unmodeled error code its SDK cannot classify, instead of a signature. The code is not retryable-or-not in any modeled sense, so SDK retry and error handling both misbehave, and no Ed25519 pre-hash signature can be produced or verified anywhere in the emulator. Because the refusal aborts the check at this call, the rest of the Ed25519 contract is also left unserved: Verify over a pre-hash signature, and Verify's KMSInvalidSignatureException (HTTP 400) on a one-bit-tampered digest. Not reached by this replay, and so not claimed here as observed, but visible in the handler for whoever fixes this: the MessageType/algorithm cross-check refusal is worded `<SigningAlgorithm> signing algorithm requires KMS MessageType:<RAW|DIGEST>.` whereas AWS words that class of refusal `Message type DIGEST is incompatible with algorithm ED25519_SHA_512.` / `Message type RAW is incompatible with algorithm ED25519_PH_SHA_512.` (fixed text `Message type `, ` is incompatible with algorithm `, trailing `.`; the message type and the algorithm name come from the request) — worth closing in the same pass, since it is the next assertion after the one that failed.

Fix: the service's signing-scheme composition in state, where the signature schemes live beside the key material — `crate::state::sign_message` returns `SignatureRefusal::Unsupported` for the Ed25519ph scheme, which the Sign handler then launders into the scaffold's not_implemented; the Ed25519ph scheme must be implemented there (and the matching judgement in the Verify path, which shares those schemes). The Sign handler's own request walk needs no change beyond no longer having an Unsupported case to report for this key spec.

Traces: kms.sign-verify-ed25519-raw-and-prehash.

## Evidence
Failing check kms.sign-verify-ed25519-raw-and-prehash (verdict: error).

Expected (AWS): HTTP 200 from Sign with a 64-byte Ed25519ph Signature, differing from the RAW ED25519_SHA_512 signature over the same message, and accepted by Verify(MessageType=DIGEST, SigningAlgorithm=ED25519_PH_SHA_512).

Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Sign operation: operation Sign with the ED25519_PH_SHA_512 signing algorithm and a KMS key whose KeySpec is ECC_NIST_EDWARDS25519 is scaffolded but not implemented in this emulator

The preceding calls in the same check all succeed, which bounds the defect: CreateKey(KeyUsage=SIGN_VERIFY, KeySpec=ECC_NIST_EDWARDS25519) succeeds; GetPublicKey returns KeySpec=ECC_NIST_EDWARDS25519, SigningAlgorithms={ED25519_SHA_512, ED25519_PH_SHA_512} and a DER public key that loads as a real Ed25519 point; Sign(MessageType=RAW, SigningAlgorithm=ED25519_SHA_512) returns a valid 64-byte signature. Only the ED25519_PH_SHA_512 call is refused.

Code: services/service-com-amazonaws-kms-trentservice/src/ops/sign.rs lines 441-460 — `match crate::state::sign_message(key, &input.signing_algorithm, &message_type, &input.message)` with `Err(crate::state::SignatureRefusal::Unsupported) => return Err(crate::scaffolded::Scaffolded::not_implemented(&format!("Sign with the {} signing algorithm and a KMS key whose KeySpec is {}", input.signing_algorithm, key.key_spec)))`. The MessageType/algorithm wording noted in the description is at the same file, lines 390-406. Model: services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json (SigningAlgorithmSpec, KeySpec, Sign, Verify).

Prior issue: 20261024T102105Z-kms-sign-stub.md is marked fixed and claimed to implement Sign; the operation is implemented but still refuses this algorithm/key-spec pair with the stub's NotImplemented.

## Steps to Reproduce
1. Boot the kms emulator.
2. kms.create_key(KeyUsage="SIGN_VERIFY", KeySpec="ECC_NIST_EDWARDS25519") and note the KeyId.
3. kms.get_public_key(KeyId=...) and observe SigningAlgorithms includes ED25519_PH_SHA_512.
4. kms.sign(KeyId=..., Message=b"message to sign", MessageType="RAW", SigningAlgorithm="ED25519_SHA_512") — succeeds, 64-byte signature.
5. kms.sign(KeyId=..., Message=hashlib.sha512(b"message to sign").digest(), MessageType="DIGEST", SigningAlgorithm="ED25519_PH_SHA_512") — observe HTTP 501 NotImplemented instead of a 64-byte Ed25519ph signature.
6. Replay kms.sign-verify-ed25519-raw-and-prehash to confirm; after a fix it must also pass Verify of the pre-hash signature and the KMSInvalidSignatureException on a tampered digest.
