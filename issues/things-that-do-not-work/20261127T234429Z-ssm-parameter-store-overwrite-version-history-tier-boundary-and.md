Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-put-parameter-overwrite-changes-type-per-version
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-put-parameter-overwrite-changes-type-per-version ssm-standard-tier-value-4096-boundary ssm-parameter-policy-invalid-expiration-timestamp-error ssm-parameter-policy-invalid-notification-unit-error ssm-parameter-policies-length-bound ssm-put-parameter-uncompilable-allowed-pattern-rejected ssm-parameter-arn-names-on-batch-reads-and-delete
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|PutParameter|2xx->4xx|$.Tier
Issue-Key: ssm|PutParameter|2xx->4xx|$.Version
Issue-Key: ssm|PutParameter|2xx->4xx|status
Issue-Key: ssm|GetParameter|2xx->2xx|$.Parameter.Type
Issue-Key: ssm|GetParameter|2xx->2xx|$.Parameter.Value
Issue-Key: ssm|GetParameter|2xx->2xx|$.Parameter.Version
Issue-Key: ssm|PutParameter|2xx->2xx|$.Version
Issue-Key: ssm|GetParameterHistory|2xx->2xx|$.Parameters[*]
Issue-Key: ssm|GetParameter|2xx->4xx|$.Parameter
Issue-Key: ssm|GetParameter|2xx->4xx|status
Issue-Key: ssm|PutParameter|4xx->2xx|$.__type
Issue-Key: ssm|PutParameter|4xx->2xx|$.message
Issue-Key: ssm|PutParameter|4xx->2xx|status
Issue-Key: ssm|PutParameter|4xx->4xx|$.message
Issue-Key: ssm|DeleteParameter|4xx->4xx|$.__type
Issue-Key: ssm|DeleteParameter|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# Parameter Store overwrite, version history, tier boundary and policy validation diverge from AWS

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutParameter

## Description
Nine net-new causes in Parameter Store. Causes 1-5 are one broken write path (an overwrite that changes Type is refused, so every later version read is wrong); causes 6-9 are the tier boundary, policy validation and ARN-name refusal on the same parameter plane.

Cause 1 of 9: an Overwrite PutParameter that changes Type is refused.
AWS: PutParameter with Overwrite=true may change a parameter's Type (String -> SecureString); the call returns 200 with the new Version (2) and Tier ('Standard'). Smithy: PutParameter input Overwrite, Type; output Version, Tier.
Emulator: 400.
Why: a client cannot promote a plaintext parameter to SecureString, the documented way to encrypt an existing parameter.
Fix: the PutParameter handler's overwrite path (type change must be permitted and recorded as a new version).
Traces: ssm-put-parameter-overwrite-changes-type-per-version#task#2.

Cause 2 of 9: GetParameter then serves the stale version.
AWS: after the type-changing overwrite, GetParameter returns Type 'SecureString', the new Value and Version 2.
Emulator: Type 'String', the previous Value, Version 1.
Why: the read reflects a write that AWS applied; consumers decrypt nothing and read the old value.
Fix: follows cause 1 (parameter state write), verify on the read path.
Traces: ssm-put-parameter-overwrite-changes-type-per-version#task#3.

Cause 3 of 9: version numbering is one behind after the refused write.
AWS: the next PutParameter returns Version 3.
Emulator: returns Version 2.
Why: version numbers are the parameter's history identity; a client that pinned a version addresses the wrong content.
Fix: parameter version allocation in state.
Traces: ssm-put-parameter-overwrite-changes-type-per-version#task#4.

Cause 4 of 9: GetParameter reports the shifted version.
AWS: Parameter.Version 3.
Emulator: 2.
Fix: as cause 3.
Traces: ssm-put-parameter-overwrite-changes-type-per-version#task#5.

Cause 5 of 9: GetParameterHistory is missing a version and mis-types another.
AWS: history holds three entries; entry 2 is the SecureString version (KeyId 'alias/aws/ssm', Type 'SecureString') and entry 3 the following String version.
Emulator: entry 2 is a String version with the wrong value and entry 3 is absent.
Why: history is the audit record of a parameter; a missing/mis-typed version misreports what was stored and when.
Fix: parameter history state write.
Traces: ssm-put-parameter-overwrite-changes-type-per-version#task#6.

Cause 6 of 9: a Standard-tier value at the 4096-byte boundary is rejected on read.
AWS: a Standard parameter whose value is 4096 bytes (including multi-byte UTF-8 characters) is stored and GetParameter returns 200 with the full value — the limit is counted in bytes, not characters.
Emulator: 400.
Why: any parameter holding non-ASCII text near the limit becomes unreadable.
Fix: the parameter value size check (byte length) in PutParameter/GetParameter.
Traces: ssm-standard-tier-value-4096-boundary#task#6.

Cause 7 of 9: PutParameter does not validate policy attributes.
AWS: 400, __type InvalidPolicyAttributeException, message "Invalid timestamp format: <value>." for a non-timestamp Expiration, and the equivalent refusal for an invalid notification Unit. Smithy: PutParameter error InvalidPolicyAttributeException.
Emulator: 200, policy stored.
Why: an unusable policy is accepted and never acts.
Fix: PutParameter policy validation.
Traces: ssm-parameter-policy-invalid-expiration-timestamp-error#task#1.

Cause 8 of 9: the Policies length refusal message diverges.
AWS: 400 ValidationException whose message is the standard template "1 validation error detected: Value '<policies document>' at 'policies' failed to satisfy constraint: Member must have length less than or equal to 4096" — fixed text is the template, the variable parts are the submitted document and the limit (Smithy: PutParameter$Policies length 1..4096).
Emulator: emits a different message for the same refusal.
Fix: PutParameter validation message rendering.
Traces: ssm-parameter-policies-length-bound#task#3.

Cause 9 of 9: DeleteParameter with an ARN name answers the wrong error.
AWS: 400, __type ValidationException, message "Parameter ARN is not supported for this operation."
Emulator: 400, __type ParameterNotFound, with no message.
Why: the caller is told the parameter does not exist when the real fault is that an ARN is not accepted here, so a retry with the same (valid) parameter keeps failing.
Fix: the DeleteParameter handler's name resolution/validation.
Traces: ssm-parameter-arn-names-on-batch-reads-and-delete#task#6.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-put-parameter-overwrite-changes-type-per-version#task#2: expected 200 with $.Tier 'Standard', $.Version 2; got 400.
Cause 2 #task#3: $.Parameter.Type expected 'SecureString' got 'String'; $.Parameter.Value expected the overwritten value got the previous one; $.Parameter.Version expected 2 got 1.
Cause 3 #task#4: $.Version expected 3 got 2. Cause 4 #task#5: $.Parameter.Version expected 3 got 2.
Cause 5 #task#6: $.Parameters[1] expected the SecureString version (KeyId 'alias/aws/ssm', Type 'SecureString', Version 2), got a String version; $.Parameters[2] expected the Version 3 String entry, got None.
Cause 6 ssm-standard-tier-value-4096-boundary#task#6: expected 200 with $.Parameter (4096-byte value containing a multi-byte character), got 400.
Cause 7 ssm-parameter-policy-invalid-expiration-timestamp-error#task#1: expected 400 'InvalidPolicyAttributeException' / 'Invalid timestamp format: tomorrow.', got 200.
Cause 8 ssm-parameter-policies-length-bound#task#3: $.message value_mismatch against the AWS "1 validation error detected: Value '<policies>' at 'policies' ... less than or equal to 4096" rendering.
Cause 9 ssm-parameter-arn-names-on-batch-reads-and-delete#task#6: $.__type expected 'ValidationException' got 'ParameterNotFound'; $.message expected 'Parameter ARN is not supported for this operation.' got None.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Causes 1-5: replay fixture ssm-put-parameter-overwrite-changes-type-per-version and inspect task#2..task#6 (PutParameter Overwrite with a changed Type, then GetParameter / PutParameter / GetParameterHistory).
3. Cause 6: fixture ssm-standard-tier-value-4096-boundary step task#6.
4. Cause 7: fixture ssm-parameter-policy-invalid-expiration-timestamp-error step task#1 (and its sibling ssm-parameter-policy-invalid-notification-unit-error).
5. Cause 8: fixture ssm-parameter-policies-length-bound step task#3.
6. Cause 9: fixture ssm-parameter-arn-names-on-batch-reads-and-delete step task#6.
