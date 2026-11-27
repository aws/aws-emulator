Issue-Source: trace-sweep-divergence
Issue-Trace: kms-encrypt-dry-run-refused-and-still-resolves-the-key
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-encrypt-dry-run-refused-and-still-resolves-the-key kms-data-key-pair-dry-run-checks-key-state kms-generate-data-key-without-plaintext-dry-run-rejected kms-unrecognised-grant-token-ignored-for-authorized-caller kms-generate-mac-dry-run-refused-after-algorithm-check kms-sign-dry-run-refused-then-signs kms-signing-key-dry-run-verify-refused-and-grant-tokens-ignored kms-verify-mac-dry-run-rejected kms-decrypt-dry-run-ignore-ciphertext kms-decrypt-names-missing-key-or-alias-not-found kms-symmetric-data-plane-refusals
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|Encrypt|4xx->4xx|$.message
Issue-Key: kms|GenerateDataKeyPairWithoutPlaintext|4xx->4xx|$.message
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|4xx->4xx|$.message
Issue-Key: kms|GenerateMac|4xx->4xx|$.message
Issue-Key: kms|Sign|4xx->4xx|$.message
Issue-Key: kms|Verify|4xx->4xx|$.message
Issue-Key: kms|Decrypt|4xx->4xx|$.__type
Issue-Key: kms|Decrypt|4xx->4xx|$.message
Issue-Related: 20261127T133034Z-kms-grant-tokens-narrow-authorization-instead-of-adding.md
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# kms: DryRun refusals carry the wrong message, and Decrypt judges the ciphertext before the dry run

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Encrypt

## Description
Two root causes in the KMS DryRun path, 8 divergence clusters. Both must be closed, and every fixture#step binding under Traces must replay clean.

Cause 1 of 2: the DryRunOperationException the data plane answers carries the Smithy shape's DOCUMENTATION sentence instead of the message AWS puts on the wire.
  AWS: Smithy models DryRunOperationException on Encrypt/Decrypt/GenerateDataKey*/GenerateMac/Sign/Verify/VerifyMac and documents the SHAPE as 'The request was rejected because the DryRun parameter was specified.' — that is API-reference prose, not the wire message. The recordings show the wire message for a DryRun request that would otherwise have succeeded: 'The request would have succeeded, but the DryRun option is set.' — entirely fixed text, no variable parts (no key id, no operation name, no value from the request). The status recorded for the refusal is 400 and the emulator already answers 400, so the status is NOT part of this finding; only the message diverges.
  Emulator: every handler constructs the refusal with the doc string — e.g. services/service-com-amazonaws-kms-trentservice/src/ops/encrypt.rs:431-441 and ops/decrypt.rs:610-620 both hard-code 'The request was rejected because the DryRun parameter was specified.' — so the same wrong sentence is answered on Encrypt, GenerateDataKeyWithoutPlaintext, GenerateDataKeyPairWithoutPlaintext, GenerateMac, Sign, Verify and VerifyMac.
  Why: a client that asserts on, logs or branches by KMS's dry-run message (the documented way to test permissions without performing the operation) reads text AWS never sends for this error class; permission-probing tooling that matches the sentence treats the probe as an unexpected failure.
  Fix: the shared DryRun refusal construction used by the KMS data-plane handlers (the message only, not the code) — one constant in crate::state reused by every ops/<op>.rs dry-run branch, so no two handlers can word it differently.
  Traces: kms-encrypt-dry-run-refused-and-still-resolves-the-key#task#3 (Encrypt, representative), kms-data-key-pair-dry-run-checks-key-state#task#3 (GenerateDataKeyPairWithoutPlaintext), kms-generate-data-key-without-plaintext-dry-run-rejected#task#2 (GenerateDataKeyWithoutPlaintext; cluster also covers kms-unrecognised-grant-token-ignored-for-authorized-caller#task#7 and #task#8), kms-generate-mac-dry-run-refused-after-algorithm-check#task#3 (GenerateMac), kms-sign-dry-run-refused-then-signs#task#2 (Sign), kms-signing-key-dry-run-verify-refused-and-grant-tokens-ignored#task#4 (Verify), kms-verify-mac-dry-run-rejected#task#3 (VerifyMac).

Cause 2 of 2: Decrypt decodes and resolves the ciphertext BEFORE it answers a DryRun request, so a dry run whose CiphertextBlob is not an envelope this service can open is refused InvalidCiphertextException instead of DryRunOperationException.
  AWS: Smithy models both DryRunOperationException and InvalidCiphertextException on Decrypt. Recorded behavior: a Decrypt carrying DryRun=true is answered with DryRunOperationException and the dry-run message above regardless of whether the ciphertext could be opened — the dry run is decided from the request and the key, and the ciphertext is never examined.
  Emulator: ops/decrypt.rs parses the envelope and resolves the key it names at lines 471-486 (and returns invalid_ciphertext() at 472, before the DryRun branch at 610), so a dry run with an opaque/foreign ciphertext answers 400 InvalidCiphertextException with NO message instead of 400 DryRunOperationException 'The request would have succeeded, but the DryRun option is set.'
  Why: the documented way to test kms:Decrypt permission without decrypting is to send DryRun=true; a client that does so with a placeholder or not-yet-readable ciphertext is told its ciphertext is invalid, which is a different error class and a different remediation (fix the ciphertext vs fix the policy), and the permission probe can never succeed.
  Fix: the Decrypt handler's ordering (services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs) — the DryRun short-circuit must be answered after the request/key-state/authorization checks but BEFORE the ciphertext is parsed, resolved or unsealed.
  Traces: kms-decrypt-dry-run-ignore-ciphertext#task#2 (cluster also covers kms-decrypt-names-missing-key-or-alias-not-found#task#4, #task#7, #task#8 and kms-symmetric-data-plane-refusals#task#7).

Related but does NOT cover these steps: the open behavioral issue 20261127T103003Z-kms-4-causes-unrecognized-grant-tokens-are-refused.md records the same dry-run wording as its Cause 2 from the behavioral suite; it carries no wire replay key, so none of the fixture steps above is exercised by it.

## Evidence
From the wire replay (trace set simparity-wire-aws-v1):

Cause 1 — status matched (400 expected, 400 got) and $.message diverged on every one of these: expected 'The request would have succeeded, but the DryRun option is set.', got 'The request was rejected because the DryRun parameter was specified.'
  kms-encrypt-dry-run-refused-and-still-resolves-the-key#task#3 (Encrypt)
  kms-data-key-pair-dry-run-checks-key-state#task#3 (GenerateDataKeyPairWithoutPlaintext)
  kms-generate-data-key-without-plaintext-dry-run-rejected#task#2 (GenerateDataKeyWithoutPlaintext; + kms-unrecognised-grant-token-ignored-for-authorized-caller#task#7, #task#8)
  kms-generate-mac-dry-run-refused-after-algorithm-check#task#3 (GenerateMac)
  kms-sign-dry-run-refused-then-signs#task#2 (Sign)
  kms-signing-key-dry-run-verify-refused-and-grant-tokens-ignored#task#4 (Verify)
  kms-verify-mac-dry-run-rejected#task#3 (VerifyMac)
Source: ops/encrypt.rs:431-441, ops/decrypt.rs:610-620 (literal doc string).

Cause 2 — kms-decrypt-dry-run-ignore-ciphertext#task#2 (Decrypt), 400/400: $.__type value_mismatch expected 'DryRunOperationException', got 'InvalidCiphertextException'; $.message missing — expected 'The request would have succeeded, but the DryRun option is set.', got None (ops/decrypt.rs:711-716 builds InvalidCiphertextException with message None). Same cluster: kms-decrypt-names-missing-key-or-alias-not-found#task#4/#task#7/#task#8, kms-symmetric-data-plane-refusals#task#7.

## Steps to Reproduce
1. Cause 1: create a symmetric ENCRYPT_DECRYPT key; call kms Encrypt with KeyId=<key arn>, Plaintext=<bytes>, DryRun=true -> expect DryRunOperationException with message 'The request would have succeeded, but the DryRun option is set.'; observe 'The request was rejected because the DryRun parameter was specified.' Repeat on GenerateDataKeyWithoutPlaintext, GenerateDataKeyPairWithoutPlaintext, GenerateMac (HMAC key), Sign/Verify (ECC SIGN_VERIFY key) and VerifyMac — same wrong sentence.
2. Cause 2: call kms Decrypt with DryRun=true and a CiphertextBlob that is not an envelope this endpoint can open (e.g. random bytes) -> expect 400 DryRunOperationException with the dry-run message; observe 400 InvalidCiphertextException with no message.
3. Replay the 8 fixture#step bindings listed under Traces.
