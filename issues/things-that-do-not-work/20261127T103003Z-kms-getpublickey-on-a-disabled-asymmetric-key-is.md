Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.sign-on-disabled-key-rejected-until-reenabled
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GetPublicKey|DisabledException
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms: GetPublicKey on a disabled asymmetric key is refused DisabledException instead of returning the public key

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicKey

## Description
REOPENS 20261117T122437Z-kms-4-net-new-causes-ml-kem-encapsulate.md: its fix left the call refused. That issue's Cause 3 reworked GetPublicKey's refusal path (the symmetric-key UnsupportedOperationException message); GetPublicKey on a DISABLED asymmetric key is still refused, where AWS serves it.

Cause 1 of 1: GetPublicKey refuses a disabled asymmetric key.
  AWS: Smithy models DisabledException on GetPublicKey (client error, httpError 409, 'The request was rejected because the specified KMS key is not enabled'), but the recorded behavior is that a key's public key stays readable while the key is Disabled: after DisableKey, Sign is refused ('DisabledException', '<key arn> is disabled.') while GetPublicKey still returns the identical DER public key, and DescribeKey reads KeyState 'Disabled'/Enabled false. Only the private-key operations are blocked by the Disabled state.
  Emulator: GetPublicKey on the disabled key is refused 409 DisabledException '<key arn> is disabled.', so the public key cannot be read at all once the key is disabled.
  Why: a client that verifies signatures locally, or re-reads a key's public material while the key is parked in Disabled (a routine operational state), is hard-refused where AWS answers; the whole disable-and-verify flow is impossible.
  Fix: the GetPublicKey handler's key-state gate — Disabled must not block GetPublicKey (keep the state check on Sign/Decrypt/DeriveSharedSecret and on keys pending deletion).
  Traces: kms.sign-on-disabled-key-rejected-until-reenabled.

## Evidence
kms.sign-on-disabled-key-rejected-until-reenabled —
botocore.errorfactory.DisabledException: An error occurred (DisabledException) when calling the GetPublicKey operation: arn:aws:kms:us-west-2:<account>:key/34c0a2b5-... is disabled.
The check had already observed the correct Sign refusal ('DisabledException', '<key arn> is disabled.') and then asserts `still_public == public_der` — AWS returns the same public key for the disabled key.

## Steps to Reproduce
1. kms.create_key(KeyUsage='SIGN_VERIFY', KeySpec='ECC_NIST_P256'); read its public key with kms.get_public_key.
2. kms.disable_key(KeyId=<arn>).
3. kms.sign(...) -> expect ('DisabledException', '<key arn> is disabled.') (matches today).
4. kms.get_public_key(KeyId=<arn>) -> expect 200 with the same DER public key; observe 409 DisabledException.
