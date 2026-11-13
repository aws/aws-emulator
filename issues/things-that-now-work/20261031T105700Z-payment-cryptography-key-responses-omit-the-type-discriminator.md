Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-alias-management-v2
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-alias-management-v2 payment-cryptography-asymmetric-key-csr-and-certificate-v2 payment-cryptography-export-import-tr31-roundtrip-v2 payment-cryptography-key-lifecycle-symmetric-v2 payment-cryptography-key-replication-regions-v2 payment-cryptography-resource-policy-workflow-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|CreateKey|2xx->2xx|$.Key.KeyAttributes.KeyModesOfUse.__type
Issue-Key: payment-cryptography|DeleteKey|2xx->2xx|$.Key.KeyAttributes.KeyModesOfUse.__type
Issue-Key: payment-cryptography|GetKey|2xx->2xx|$.Key.KeyAttributes.KeyModesOfUse.__type
Issue-Key: payment-cryptography|ListKeys|2xx->2xx|$.Keys[*]
Issue-Key: payment-cryptography|RestoreKey|2xx->2xx|$.Key.KeyAttributes.KeyModesOfUse.__type
Issue-Commit: 2b1389a4a6c4df71c4118b776bf7e768de987e7c

# payment-cryptography key responses omit the __type discriminator on KeyModesOfUse

> _Note: this issue was identified against the code base at commit `2b1389a4a6c4df71c4118b776bf7e768de987e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKey

## Description
Every control-plane operation that serializes a Key or KeySummary emits the nested KeyModesOfUse structure WITHOUT the `__type` member that Payment Cryptography puts on it. This is one serializer defect observed on five operations; each is listed below as its own cause so each can be re-replayed.

Cause 1 of 5: CreateKey.
AWS: Smithy (PaymentCryptographyControlPlane, awsJson1_0) — CreateKey -> CreateKeyOutput.Key : com.amazonaws.paymentcryptography#Key, whose required member KeyAttributes targets KeyAttributes, whose KeyModesOfUse member targets com.amazonaws.paymentcryptography#KeyModesOfUse (nine PrimitiveBoolean members, each @default false). Recorded behavior: the serialized KeyModesOfUse object carries an additional `__type` member whose value is the fully-qualified shape id of the structure the service resolves it to, i.e. the literal string `com.amazonaws.magnus#KeyModesOfUse` (Magnus is the service's internal name; the string is fixed text, not a per-run value). All nine boolean flags themselves are serialized correctly.
Emulator: returns HTTP 200 with a structurally correct Key, but `$.Key.KeyAttributes.KeyModesOfUse.__type` is absent (expected 'com.amazonaws.magnus#KeyModesOfUse', got None).
Why: a client that keys off the type discriminator on the nested structure — a polymorphic deserializer, a recorded-response differ, or any consumer that round-trips the document and compares it — sees a document AWS never emits and either mis-deserializes the modes-of-use block or reports the response as changed.
Fix: the Key/KeyAttributes serializer in the payment-cryptography control-plane crate, where KeyModesOfUse is written: it must emit the `__type` discriminator on that nested object on every path that serializes it, rather than at each call site.
Traces: payment-cryptography-alias-management-v2#setup#1 (this cluster also covers 7 further CreateKey steps across the asymmetric-key-csr, export-import-tr31, key-lifecycle, key-replication and resource-policy fixtures).

Cause 2 of 5: DeleteKey.
AWS: DeleteKey -> DeleteKeyOutput.Key : Key, same nested KeyModesOfUse; recorded responses carry `__type` = 'com.amazonaws.magnus#KeyModesOfUse'.
Emulator: HTTP 200, `$.Key.KeyAttributes.KeyModesOfUse.__type` missing.
Why/Fix: as cause 1 — the scheduled-deletion response is serialized through the same Key writer.
Traces: payment-cryptography-alias-management-v2#teardown#10 (cluster also covers 7 further DeleteKey steps).

Cause 3 of 5: GetKey.
AWS: GetKey -> GetKeyOutput.Key : Key; recorded responses carry the `__type` discriminator on KeyModesOfUse.
Emulator: HTTP 200, `$.Key.KeyAttributes.KeyModesOfUse.__type` missing.
Why/Fix: as cause 1.
Traces: payment-cryptography-key-lifecycle-symmetric-v2#task#2 (cluster also covers task#8).

Cause 4 of 5: ListKeys.
AWS: ListKeys -> ListKeysOutput.Keys : KeySummaryList of com.amazonaws.paymentcryptography#KeySummary, whose required KeyAttributes member also nests KeyModesOfUse; recorded list entries carry the same `__type` discriminator on that nested object.
Emulator: HTTP 200 and the list entry otherwise matches, but `$.Keys[0].KeyAttributes.KeyModesOfUse.__type` is absent, so the whole entry fails comparison. (The KeyCheckValue difference in the same diff is a per-run generated value and is NOT part of this finding.)
Why/Fix: as cause 1, applied to the KeySummary serializer used by the list path.
Traces: payment-cryptography-key-lifecycle-symmetric-v2#task#3.

Cause 5 of 5: RestoreKey.
AWS: RestoreKey -> RestoreKeyOutput.Key : Key; recorded responses carry the discriminator.
Emulator: HTTP 200, `$.Key.KeyAttributes.KeyModesOfUse.__type` missing.
Why/Fix: as cause 1.
Traces: payment-cryptography-key-lifecycle-symmetric-v2#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). Five divergence clusters, all verdict=mismatch, all expected_status 200 / actual_status 200:
- CreateKey, divergence_key service=payment-cryptography|CreateKey|status=200->200|keys=$.Key.KeyAttributes.KeyModesOfUse.__type, affected_count 8: "$.Key.KeyAttributes.KeyModesOfUse.__type: missing (expected 'com.amazonaws.magnus#KeyModesOfUse', got None)".
- DeleteKey, same key shape, affected_count 8: identical detail.
- GetKey, affected_count 2: identical detail.
- RestoreKey, affected_count 1: identical detail.
- ListKeys, affected_count 1: "$.Keys[0]: missing (expected {... 'KeyModesOfUse': {'__type': 'com.amazonaws.magnus#KeyModesOfUse', 'Decrypt': True, ...}, ...}, got {... 'KeyModesOfUse': {'Encrypt': True, 'Decrypt': True, ...} ...})" — the emulator's entry is identical except for the absent __type (and the per-run KeyCheckValue).
Smithy: Key.KeyAttributes (required) -> KeyAttributes.KeyModesOfUse -> com.amazonaws.paymentcryptography#KeyModesOfUse; KeySummary.KeyAttributes (required) likewise.

## Steps to Reproduce
1. Replay fixture payment-cryptography-alias-management-v2 against the emulator and inspect step setup#1 (CreateKey): the 200 body's $.Key.KeyAttributes.KeyModesOfUse object has no __type member.
2. Same fixture, step teardown#10 (DeleteKey): same omission.
3. Replay payment-cryptography-key-lifecycle-symmetric-v2 and inspect task#2 (GetKey), task#3 (ListKeys, $.Keys[0].KeyAttributes.KeyModesOfUse) and task#9 (RestoreKey): same omission on each.
4. Compare each against the recorded response, which carries "__type": "com.amazonaws.magnus#KeyModesOfUse" inside KeyModesOfUse.
