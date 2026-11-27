Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors cloudfront-field-level-encryption-lifecycle cloudfront-query-arg-profile-config-lifecycle cloudfront-fle-config-not-found-and-invalid-ifmatch-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary[Id="CVBB9JKPE6KWE0"].QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|DeleteFieldLevelEncryptionProfile|2xx->4xx|status
Issue-Commit: 9035f96a24b7bc6854e584ea99175d352b190dfa

# CloudFront field-level encryption config loses its declared forwarding flag and profile deletes are refused 412

> _Note: this issue was identified against the code base at commit `9035f96a24b7bc6854e584ea99175d352b190dfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFieldLevelEncryptionConfig

## Description
Four net-new wire divergences on CloudFront's field-level-encryption (FLE) resources. The shared fault is that the FLE resource record the emulator keeps is not the one the caller created: the QueryArgProfileConfig forwarding flag submitted on create is not persisted (so every read path reports the opposite boolean), and the profile's version identifier is not the one the emulator itself handed out (so a delete carrying a freshly-read If-Match is refused). Related open issues 20261125T151601Z-cloudfront-field-level-encryption-reads-and-deletes-answer.md (unreadable-identifier wording on FLE reads/deletes), 20261105T231732Z-cloudfront-readback-omits-defaulted-members-and-reports-stale.md (dropped QueryArgProfiles.Items) and 20261126T131820Z-cloudfront-distribution-tenant-etag-and-public-key-refusal.md (tenant ETag drift) carry their own replay keys and do NOT cover the steps below.

Cause 1 of 4: CreateFieldLevelEncryptionConfig echoes QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown as true when the request declared false.
AWS: Smithy: FieldLevelEncryptionConfig.QueryArgProfileConfig -> QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is a @required boolean, documented "Flag to set if you want a request to be forwarded to the origin even if the profile specified by the field-level encryption query argument, fle-profile, is unknown." It is a caller-supplied value with no service default: AWS stores the submitted boolean and returns it verbatim in the 201 FieldLevelEncryption body (CreateFieldLevelEncryptionConfig http POST /2020-05-31/field-level-encryption, code 201). The recording submits false and reads false back.
Emulator: the 201 body reports $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown as 'true'.
Why: this flag decides whether a viewer request carrying an unrecognised fle-profile query argument is forwarded to the origin anyway. Flipping a declared false to true converts a fail-closed FLE configuration into a fail-open one, and a client that round-trips the config (read, change one profile, write) silently re-submits the wrong posture.
Fix: CreateFieldLevelEncryptionConfig's handler/state write — persist the submitted ForwardWhenQueryArgProfileIsUnknown rather than a hardcoded or defaulted true — plus the FieldLevelEncryption serializer that reads it.
Traces: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4 (representative), cloudfront-field-level-encryption-lifecycle#task#7, cloudfront-query-arg-profile-config-lifecycle#task#4.

Cause 2 of 4: GetFieldLevelEncryptionConfig reports the same flag as true for a config created with false.
AWS: as Cause 1 — the stored value is returned unchanged on the read path ($.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown), status 200.
Emulator: returns 'true' where the stored value is false.
Why: the read path is how a client confirms its FLE posture before associating the config with a cache behavior; it is told the configuration forwards unknown-profile requests when it does not (and vice versa), so no read can recover the真 value. Both write and read paths are wrong, so neither create-echo nor re-read exposes the caller's setting.
Fix: same state/serializer path as Cause 1, shared by GetFieldLevelEncryptionConfig.
Traces: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6, cloudfront-field-level-encryption-lifecycle#task#8, cloudfront-query-arg-profile-config-lifecycle#task#5.

Cause 3 of 4: ListFieldLevelEncryptionConfigs carries the same wrong flag in each summary.
AWS: FieldLevelEncryptionSummary.QueryArgProfileConfig repeats the stored QueryArgProfileConfig, so the listed summary agrees with the configuration's stored value (recorded: false), status 200.
Emulator: the summary for the listed configuration reports ForwardWhenQueryArgProfileIsUnknown 'true'.
Why: a client that enumerates FLE configurations to audit which ones fail open gets the wrong answer for every configuration, including ones it never read individually.
Fix: the FieldLevelEncryptionSummary serializer must project the stored QueryArgProfileConfig (same state fix as Cause 1).
Traces: cloudfront-field-level-encryption-lifecycle#task#9, cloudfront-query-arg-profile-config-lifecycle#task#6.

Cause 4 of 4: DeleteFieldLevelEncryptionProfile refuses a delete whose If-Match is the profile's current ETag, answering 412 PreconditionFailed instead of deleting it.
AWS: Smithy: DeleteFieldLevelEncryptionProfile http DELETE /2020-05-31/field-level-encryption-profile/{Id}, success code 204 with an empty body; IfMatch is bound to the If-Match header; PreconditionFailed (httpError 412, "The precondition in one or more of the request fields evaluated to false") is returned ONLY when the supplied If-Match does not equal the profile's current version. In the recording the caller deletes with the ETag the service issued for that profile and receives 204.
Emulator: answers 412 PreconditionFailed for that same delete, so the profile is never removed.
Why: an FLE profile can never be torn down: the caller supplies the only ETag the service ever gave it, is told the precondition failed, and re-reading yields no acceptable value. Profiles, and the public keys and configurations that reference them, leak for the lifetime of the emulator, and any scripted create/use/delete lifecycle fails at teardown.
Hypothesis: the emulator does not keep a stored per-resource version for FLE profiles and recomputes the ETag from mutable response content (or compares against the FLE *config*'s version), so the value a client just read is already stale by the time it is echoed back.
Fix: the FLE profile's state record and the If-Match comparison in DeleteFieldLevelEncryptionProfile — the ETag must be a stored version identifier mutated only by writes, and a matching If-Match must yield 204 with no body.
Traces: cloudfront-fle-config-not-found-and-invalid-ifmatch-errors#teardown#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 94.6%, 32 divergence clusters, all cloudfront; 1442 steps, 76 mismatched).

Cause 1 — cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4 CreateFieldLevelEncryptionConfig (201/201): $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'. Cluster also covers cloudfront-field-level-encryption-lifecycle#task#7 and cloudfront-query-arg-profile-config-lifecycle#task#4 (3 steps).
Cause 2 — cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6 GetFieldLevelEncryptionConfig (200/200): $.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'. Cluster also covers cloudfront-field-level-encryption-lifecycle#task#8 and cloudfront-query-arg-profile-config-lifecycle#task#5 (3 steps).
Cause 3 — cloudfront-field-level-encryption-lifecycle#task#9 ListFieldLevelEncryptionConfigs (200/200): $.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary[Id="..."].QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'. Cluster also covers cloudfront-query-arg-profile-config-lifecycle#task#6 (2 steps).
Cause 4 — cloudfront-fle-config-not-found-and-invalid-ifmatch-errors#teardown#7 DeleteFieldLevelEncryptionProfile: status expected 204, got 412 (PreconditionFailed). The DeletePublicKey 412 in the same teardown (#teardown#8) is already owned by 20261118T213617Z-cloudfront-public-key-lifecycle-refuses-creates-and-deletes.md and is NOT part of this issue.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-field-level-encryption-duplicate-and-ifmatch-errors and inspect task#4 (create echo) and task#6 (readback): submit a FieldLevelEncryptionConfig whose QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is false and diff the returned flag.
4. Replay cloudfront-field-level-encryption-lifecycle#task#9 (and cloudfront-query-arg-profile-config-lifecycle#task#6) and check the flag in each FieldLevelEncryptionSummary.
5. Replay cloudfront-fle-config-not-found-and-invalid-ifmatch-errors#teardown#7: read the FLE profile's ETag, DELETE the profile with that value in If-Match, and confirm 204 with an empty body rather than 412.
6. All four causes must match the recording before closing.
