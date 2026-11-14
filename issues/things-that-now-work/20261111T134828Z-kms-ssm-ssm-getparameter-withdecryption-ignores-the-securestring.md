Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__ssm.securestring-under-a-disabled-cmk-is-neither-decrypted-nor-written
Issue-Key: kms__ssm.securestring-under-a-disabled-cmk-is-neither-decrypted-nor-written|_poll#15|AssertionError|raise AssertionError(f"{description} not within {timeout_s:.0f}s; last {last!r}")
Issue-Commit: 53bfbc08a5ec2983a62510ba6bf3546a998019d9

# kms__ssm: SSM GetParameter WithDecryption ignores the SecureString key's KMS key state — a parameter under a disabled CMK still decrypts instead of being refused with InvalidKeyId

> _Note: this issue was identified against the code base at commit `53bfbc08a5ec2983a62510ba6bf3546a998019d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1: SSM's SecureString decrypt path does not consult the KMS key state of the parameter's customer managed key, so a read that real AWS refuses while the key is disabled succeeds and hands back the plaintext. Failing check: kms__ssm.securestring-under-a-disabled-cmk-is-neither-decrypted-nor-written.

AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models InvalidKeyId on both GetParameter and PutParameter as a client error, HTTP 400, documented "The query key ID isn't valid." Per https://docs.aws.amazon.com/kms/latest/developerguide/enabling-keys.html a disabled CMK cannot be used for cryptographic operations: KMS Decrypt/GenerateDataKey raise DisabledException, and SSM surfaces that to the caller as the modeled InvalidKeyId. The message is KMS's sentence wrapped by SSM; its fixed text is " is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: " / "; Proxy: null)", i.e. the template
  "<key-arn> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <kms-request-id>; Proxy: null)"
where <key-arn> is the ARN of the key the parameter is encrypted under (taken from the request/state) and <kms-request-id> is the per-call KMS request id (a per-run value, not part of the requirement). The same refusal applies to PutParameter of a SecureString naming that key, and a refused write must not create a new parameter version.

Emulator: with the key's KeyState confirmed 'Disabled' by DescribeKey, repeated GetParameter(Name=<name>, WithDecryption=True) kept answering 200 with the decrypted plaintext for the full 40 s the check polls — the replay records the outcome as ('OK', '') at every probe and fails with "AssertionError: GetParameter WithDecryption is refused under the disabled key not within 40s; last ('OK', '')". No error code, no 400. Because the read never refused, the check never reached its PutParameter assertion, so the write path's behaviour under a disabled key is unverified but shares the same unchecked key-state lookup.

Why: a client that disables a CMK as a kill switch (incident response, key-compromise containment, compliance-driven suspension) observes the secret as still readable against the emulator. Code that treats disabling the key as revoking access to every SecureString under it, and error handling that expects a 400 InvalidKeyId with the DisabledException sentence, is never exercised; a secret the operator believes sealed is served in plaintext.

Fix: GetParameter's WithDecryption branch (services/service-com-amazonaws-ssm-amazonssm/src/ops/get_parameter.rs and the SecureString decrypt helper it calls in crate::state::secure_string) must route the unseal through the same KMS key-state gate a direct kms:Decrypt takes, and map KMS's DisabledException to the modeled InvalidKeyId (HTTP 400) with the wrapped KMS sentence above; PutParameter's seal path for a SecureString naming a disabled key must refuse identically and leave the parameter's version unchanged.

Traces: kms__ssm.securestring-under-a-disabled-cmk-is-neither-decrypted-nor-written (mismatch).

## Evidence
Check kms__ssm.securestring-under-a-disabled-cmk-is-neither-decrypted-nor-written, verdict mismatch.

Sequence replayed: kms.CreateKey (SYMMETRIC_DEFAULT, ENCRYPT_DECRYPT) -> ssm.PutParameter(Type=SecureString, KeyId=<key-arn>) -> ssm.GetParameter(WithDecryption=True) returns version 1 with the plaintext (positive control passes) -> kms.DisableKey(<key-arn>) -> kms.DescribeKey reports KeyState 'Disabled' (assertion passes) -> ssm.GetParameter(WithDecryption=True) polled every 2 s for 40 s.

Expected (real AWS): the read is refused — error code "InvalidKeyId", HTTP 400, message "<key-arn> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <kms-request-id>; Proxy: null)" (fixed text as quoted; <key-arn> from state, <kms-request-id> per call). Then PutParameter of a new value naming the same key is refused with the same code and message, and GetParameter(WithDecryption=False) still reports Version 1.

Actual (emulator): every probe returned success — the harness records the outcome tuple ('OK', '') and aborts with
  AssertionError: GetParameter WithDecryption is refused under the disabled key not within 40s; last ('OK', '')
so the 200 response carried the decrypted plaintext while the key was Disabled.

## Steps to Reproduce
1. kms.CreateKey(KeyUsage="ENCRYPT_DECRYPT", KeySpec="SYMMETRIC_DEFAULT") and note KeyMetadata.Arn.
2. ssm.PutParameter(Name="/<unique>", Value="<secret>", Type="SecureString", KeyId=<key-arn>).
3. ssm.GetParameter(Name="/<unique>", WithDecryption=True) — returns Version 1 with the plaintext (expected).
4. kms.DisableKey(KeyId=<key-arn>); confirm kms.DescribeKey reports KeyState "Disabled".
5. ssm.GetParameter(Name="/<unique>", WithDecryption=True) — emulator answers 200 with the plaintext; AWS refuses with 400 InvalidKeyId and the wrapped KMS DisabledException sentence.
6. ssm.PutParameter(Name="/<unique>", Value="<new>", Type="SecureString", KeyId=<key-arn>, Overwrite=True) then ssm.GetParameter(WithDecryption=False) — AWS refuses the write with the same 400 InvalidKeyId and leaves Version at 1.
