Issue-Source: trace-sweep-divergence
Issue-Trace: kms-get-public-key-disabled-key
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-get-public-key-disabled-key kms-key-material-import kms-symmetric-data-plane-refusals
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GetPublicKey|2xx->4xx|$.CustomerMasterKeySpec
Issue-Key: kms|GetPublicKey|2xx->4xx|$.KeyId
Issue-Key: kms|GetPublicKey|2xx->4xx|$.KeyOrigin
Issue-Key: kms|GetPublicKey|2xx->4xx|$.KeySpec
Issue-Key: kms|GetPublicKey|2xx->4xx|$.KeyUsage
Issue-Key: kms|GetPublicKey|2xx->4xx|$.PublicKey
Issue-Key: kms|GetPublicKey|2xx->4xx|$.SigningAlgorithms
Issue-Key: kms|GetPublicKey|2xx->4xx|status
Issue-Key: kms|DeleteImportedKeyMaterial|2xx->2xx|$.KeyMaterialId
Issue-Key: kms|Encrypt|2xx->4xx|$.CiphertextBlob
Issue-Key: kms|Encrypt|2xx->4xx|$.EncryptionAlgorithm
Issue-Key: kms|Encrypt|2xx->4xx|$.KeyId
Issue-Key: kms|Encrypt|2xx->4xx|status
Issue-Key: kms|GenerateDataKeyPair|2xx->2xx|$.PrivateKeyPlaintext
Issue-Related: 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# kms: 4 causes — GetPublicKey refuses a disabled key, DeleteImportedKeyMaterial emits an empty KeyMaterialId, an accepted symmetric Encrypt is refused, and a data key pair's private key has the wrong shape

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicKey

## Description
Four independent causes in the KMS key-material and data-plane response path, 4 divergence clusters. Each must be closed, and each fixture#step binding under Traces must replay clean.

Cause 1 of 4: GetPublicKey refuses a DISABLED asymmetric key instead of returning its public key.
  AWS: Smithy models DisabledException on GetPublicKey ('The request was rejected because the specified KMS key is not enabled', httpError 409), but the recording shows the Disabled state does not block GetPublicKey: after DisableKey, the key's public material is still served — 200 with PublicKey (the same DER SubjectPublicKeyInfo as before the disable), KeyId as the key ARN, KeySpec and the deprecated CustomerMasterKeySpec ('RSA_2048'), KeyUsage ('SIGN_VERIFY') and the full SigningAlgorithms list (RSASSA_PKCS1_V1_5_SHA_256/384/512, RSASSA_PSS_SHA_256/384/512). Only the private-key operations (Sign, Decrypt, DeriveSharedSecret) are refused while the key is Disabled.
  Emulator: the call is refused 400 — none of PublicKey/KeyId/KeySpec/CustomerMasterKeySpec/KeyUsage/SigningAlgorithms is answered — because the handler's key-state gate treats Disabled as unusable for GetPublicKey too.
  Why: a client that verifies signatures locally, re-reads key material, or audits a key parked in Disabled (a routine operational state) cannot read the public key at all; the whole disable-and-still-verify flow is impossible.
  Fix: the GetPublicKey handler's key-state gate (services/service-com-amazonaws-kms-trentservice/src/ops/get_public_key.rs) — Disabled must not block GetPublicKey; keep the state gate on the private-key operations and on keys pending deletion. Note: the open behavioral issue 20261127T103003Z-kms-getpublickey-on-a-disabled-asymmetric-key-is.md describes the same fault from the behavioral suite and carries no wire replay key, so it does not exercise this fixture step.
  Traces: kms-get-public-key-disabled-key#task#5.

Cause 2 of 4: DeleteImportedKeyMaterial answers KeyMaterialId as an empty string where AWS omits the member.
  AWS: Smithy: DeleteImportedKeyMaterialResponse$KeyMaterialId targets BackingKeyIdResponseType, @length(0..64) @pattern('^[a-f0-9]+$'), documented 'Identifies the deleted key material.' The recording shows the member ABSENT from the response when the call deleted no identified material; a present member always identifies material (64 lowercase hex chars).
  Emulator: the member is always serialized — services/service-com-amazonaws-kms-trentservice/src/ops/delete_imported_key_material.rs:325 writes `key_material_id: Some(deleted.unwrap_or_default())`, collapsing the 'nothing was deleted' case to '' — so the response carries KeyMaterialId: "".
  Why: a client that branches on presence ('was a specific key-material version deleted?') takes the wrong branch, and one that feeds the value straight back into DeleteImportedKeyMaterial/ImportKeyMaterial sends an identifier that violates the request member's @length(64,64) @pattern constraint.
  Fix: the DeleteImportedKeyMaterial handler's response construction (delete_imported_key_material.rs:322-326) — map the absent deletion to None instead of unwrap_or_default(), so the member is omitted rather than emitted empty.
  Traces: kms-key-material-import#task#4.

Cause 3 of 4: an Encrypt request AWS accepts on an enabled symmetric key is refused 400.
  AWS: Smithy: Encrypt takes KeyId (required), Plaintext (@length 1..4096), EncryptionContext, GrantTokens, EncryptionAlgorithm, DryRun, and answers 200 with CiphertextBlob, KeyId as the key ARN and EncryptionAlgorithm. The recording for these two steps is a 200 with CiphertextBlob, KeyId = the key ARN and EncryptionAlgorithm 'SYMMETRIC_DEFAULT' on an Enabled SYMMETRIC_DEFAULT key.
  Emulator: both steps answer 400 and the response carries none of CiphertextBlob/KeyId/EncryptionAlgorithm. Hypothesis: the shared input validation in crate::state::check_encryption_context (state.rs:1351-1365, called from ops/encrypt.rs:280) refuses the request with a ValidationException carrying 'Invalid EncryptionContext' because an encryption-context entry has an empty name or an empty value, which the recording shows AWS accepting; the other 400-capable branches reachable here are the key-usage/algorithm refusals (encrypt.rs:373-392), which this key and SYMMETRIC_DEFAULT satisfy.
  Why: a client whose encryption context contains an entry AWS accepts cannot encrypt at all against the emulator, and the data it would then decrypt with the same context is never produced.
  Fix: the Encrypt handler's input validation and the shared encryption-context check it calls (ops/encrypt.rs, crate::state::check_encryption_context) — accept the contexts AWS accepts, and keep the refusal only for a context that is genuinely not a context.
  Traces: kms-symmetric-data-plane-refusals#task#3 (cluster also covers #task#4).

Cause 4 of 4: GenerateDataKeyPair's PrivateKeyPlaintext does not have the shape the recorded response has for the requested KeyPairSpec.
  AWS: Smithy: GenerateDataKeyPairResponse$PrivateKeyPlaintext is a PlaintextType blob, 'the plaintext copy of the private key ... When you use the HTTP API or the Amazon Web Services CLI, the value is Base64-encoded' — a DER-encoded private key matching the KeyPairSpec the request asked for and the PublicKey returned alongside it. The replay does not compare the random bytes, it compares the value's SHAPE against the recording, and that shape check failed.
  Emulator: the step answers 200 but PrivateKeyPlaintext is reported as a shape mismatch against the recorded private key (observed value: a base64 PKCS#8 EC private key, 'MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg…'), i.e. the private key is emitted in a different encoding/length class than AWS's DER private key for that spec.
  Why: a client that parses the returned private key with a standard DER/PKCS#8 reader, or pairs it with the returned PublicKey, gets material of the wrong form for the KeyPairSpec it asked for, so local crypto with the data key pair fails.
  Fix: the GenerateDataKeyPair handler's private-key encoding (services/service-com-amazonaws-kms-trentservice/src/ops/generate_data_key_pair.rs and the key-pair generation in crate::state::asymmetric) — emit the private key in the DER form AWS returns for the requested KeyPairSpec.
  Traces: kms-symmetric-data-plane-refusals#task#10.

## Evidence
From the wire replay (trace set simparity-wire-aws-v1):

Cause 1 — kms-get-public-key-disabled-key#task#5 (GetPublicKey): status expected 200, got 400; $.PublicKey, $.KeyId ('arn:aws:kms:<region>:<account>:key/<key-id>'), $.KeySpec ('RSA_2048'), $.CustomerMasterKeySpec ('RSA_2048'), $.KeyUsage ('SIGN_VERIFY'), $.SigningAlgorithms (['RSASSA_PKCS1_V1_5_SHA_256','RSASSA_PKCS1_V1_5_SHA_384','RSASSA_PKCS1_V1_5_SHA_512','RSASSA_PSS_SHA_256','RSASSA_PSS_SHA_384','RSASSA_PSS_SHA_512']) all missing from the 400 body. task#2 of the same fixture (the same key before it was disabled) answered 200.

Cause 2 — kms-key-material-import#task#4 (DeleteImportedKeyMaterial): 200/200, $.KeyMaterialId unexpected_present — expected None, got ''. Source: ops/delete_imported_key_material.rs:325 `key_material_id: Some(deleted.unwrap_or_default())`.

Cause 3 — kms-symmetric-data-plane-refusals#task#3 (Encrypt), same cluster task#4: status expected 200, got 400; $.CiphertextBlob, $.KeyId (the key ARN) and $.EncryptionAlgorithm ('SYMMETRIC_DEFAULT') missing.

Cause 4 — kms-symmetric-data-plane-refusals#task#10 (GenerateDataKeyPair): 200/200, $.PrivateKeyPlaintext value_mismatch against the recording's shaped expectation; got 'MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgclONK+Gqo1foc4LUsidKzdPcp2GpNDGGlFNv2l8zaHmhRANCAATHNVEPzwwxvLECc4BjrfvIsbItT00ySf0GtdlTG+7FtgFked3ywuvuEuMNFUgYaxwLsAKcJo3iZHK1E7SDeN2Y'. (The $.KeyOrigin omission on the same step belongs to the already-filed 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md and is not part of this issue.)

## Steps to Reproduce
1. Cause 1: kms CreateKey(KeyUsage='SIGN_VERIFY', KeySpec='RSA_2048'); GetPublicKey -> 200. kms DisableKey(KeyId=<arn>); GetPublicKey again -> expect 200 with the same PublicKey, KeySpec/CustomerMasterKeySpec 'RSA_2048', KeyUsage 'SIGN_VERIFY' and the six RSA signing algorithms; observe 400.
2. Cause 2: create an EXTERNAL-origin key, ImportKeyMaterial, then DeleteImportedKeyMaterial twice (or once with no material to delete) -> expect the response to OMIT KeyMaterialId; observe KeyMaterialId: "".
3. Cause 3: on an Enabled SYMMETRIC_DEFAULT key, Encrypt with the encryption context the fixture uses (kms-symmetric-data-plane-refusals#task#3/#task#4) -> expect 200 with CiphertextBlob, KeyId=<key arn>, EncryptionAlgorithm='SYMMETRIC_DEFAULT'; observe 400.
4. Cause 4: GenerateDataKeyPair(KeyId=<symmetric key arn>, KeyPairSpec=<the fixture's spec>) -> compare PrivateKeyPlaintext's encoding/length with the recorded DER private key for that spec.
5. Replay the 4 fixture#step bindings listed under Traces.
