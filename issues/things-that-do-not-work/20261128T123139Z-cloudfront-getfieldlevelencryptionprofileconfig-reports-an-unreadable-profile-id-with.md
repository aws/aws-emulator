Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-field-level-encryption-profile-config-get-and-not-found
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-field-level-encryption-profile-config-get-and-not-found
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetFieldLevelEncryptionProfileConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: b880698c4f2cb4e70545429d30696ac71b7bccc0

# CloudFront GetFieldLevelEncryptionProfileConfig reports an unreadable profile id with the absent-profile message

> _Note: this issue was identified against the code base at commit `b880698c4f2cb4e70545429d30696ac71b7bccc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFieldLevelEncryptionProfileConfig

## Description
One cause: the field-level-encryption PROFILE CONFIG read (`GET /2020-05-31/field-level-encryption-profile/{Id}/config`) no longer distinguishes an `Id` value CloudFront refuses as UNREADABLE for the field-level-encryption profile kind from a well-formed identifier that names no profile. The submitted value is carried straight through to the state lookup, misses, and is answered with the profile's absent-record sentence. The HTTP status (404) and the error `Code` match AWS exactly; only `$.ErrorResponse.Error.Message` diverges, so the fault is in the id-admission decision this read path makes (or fails to make) before the store lookup, not in the error envelope or the status mapping.

Cause 1 of 1: GetFieldLevelEncryptionProfileConfig words an unreadable profile id as a missing profile.
AWS: Smithy (cloudfront 2020-05-31) GetFieldLevelEncryptionProfileConfig -- input member `Id` is @required and @httpLabel on `GET /2020-05-31/field-level-encryption-profile/{Id}/config`, success 200; output `FieldLevelEncryptionProfileConfig` (@httpPayload) plus `ETag` (@httpHeader ETag); modeled errors `AccessDenied` (httpError 403) and `NoSuchFieldLevelEncryptionProfile` (httpError 404). The model does not word the messages that one not-found code carries, so for the wording this is RECORDED BEHAVIOR ONLY: the recording shows a value that cannot be an identifier CloudFront issued for the field-level-encryption profile kind answered HTTP 404 in the REST-XML `ErrorResponse` envelope with `Message` = 'Id format is invalid'. That sentence is ENTIRELY FIXED TEXT for this class -- AWS interpolates nothing from the request or from state into it, not even the offending value -- so the requirement is the sentence itself and no recorded id, name or timestamp is part of it. The envelope's `RequestId` is per-run and is not asserted.
Emulator: answers HTTP 404 with `Message` = 'The specified field level encryption profile does not exist.' -- the profile's absent-record sentence (also fixed text, and the correct wording only for a readable identifier that names nothing), for a value that could never have named a profile.
Why: a client cannot tell a request-construction bug (a truncated id, an id of another CloudFront resource kind, an id pasted from another service) from a profile that was deleted or never created. Create-if-absent and idempotent-teardown logic keyed on the absent-record sentence fires on input AWS requires it to reject and surface to the developer, and the genuine absent-id path becomes untestable because both paths emit an identical body with an identical status and code.
Fix: the input validation of GetFieldLevelEncryptionProfileConfig in the cloudfront operation handler, together with the shared field-level-encryption id-admission helper the profile read paths call: the unreadable-identifier branch must be decided BEFORE the state lookup and must keep its own message, while a resolution miss keeps the kind's modeled absent-record sentence. The two branches must not collapse into one in either direction.
Traces: cloudfront-field-level-encryption-profile-config-get-and-not-found#task#4 (cluster size 1).

Related issues, none of which covers this cluster (the replay reports it with no owning open issue and no blocking stub): the OPEN bundle 20261125T151601Z-cloudfront-field-level-encryption-reads-and-deletes-answer.md reports exactly this substitution on four sibling operations -- GetFieldLevelEncryptionConfig, GetFieldLevelEncryptionProfile, DeleteFieldLevelEncryptionConfig, DeleteFieldLevelEncryptionProfile -- but not on the profile's `/config` variant, and it does not carry this replay key. The issue 20261125T014453Z-cloudfront-reads-report-a-malformed-identifier-with-the.md did name this operation, but it is RESOLVED (its resolution added a width-and-alphabet admission rule under state::id_format_refused_for), and a resolved issue covers no failure: the probe recorded here is a value that rule now carries through to the store, so the profile-config read regressed to the absent-record sentence.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 205 fixtures replayed, 1442 steps, score 94.6%, 33 divergence clusters). This is the ONLY cluster the replay reports with an empty `owned_by` and an empty `stub_path`; all 32 others are owned by an open issue or blocked by a filed stub.

Cause 1 -- fixture cloudfront-field-level-encryption-profile-config-get-and-not-found, step task#4, GetFieldLevelEncryptionProfileConfig:
  expected: HTTP 404, $.ErrorResponse.Error.Message = 'Id format is invalid'
  actual:   HTTP 404, $.ErrorResponse.Error.Message = 'The specified field level encryption profile does not exist.'
  verdict: mismatch; affected_count 1
  divergence_key: service=cloudfront|GetFieldLevelEncryptionProfileConfig|status=404->404|keys=$.ErrorResponse.Error.Message

Model evidence (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json):
  GetFieldLevelEncryptionProfileConfig -- GET /2020-05-31/field-level-encryption-profile/{Id}/config, success 200; Id @required @httpLabel; output FieldLevelEncryptionProfileConfig @httpPayload, ETag @httpHeader('ETag'); errors AccessDenied(403), NoSuchFieldLevelEncryptionProfile(404).

Fixed vs variable message text: 'Id format is invalid' is fixed text in full -- no id, name, value, limit or member name from the request or from emulator state is interpolated into it, so no recorded value is part of the requirement. The emulator's 'The specified field level encryption profile does not exist.' is likewise fixed text and is the correct wording only for a readable identifier that resolves to no stored profile. Status (404) and error Code already match AWS and must not change.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Replay fixture cloudfront-field-level-encryption-profile-config-get-and-not-found and observe step task#4 -- GET /2020-05-31/field-level-encryption-profile/{Id}/config with an Id value CloudFront cannot read as an identifier of the field-level-encryption profile kind (the alphabet/width/foreign-namespace-letter class).
3. Expect HTTP 404 with $.ErrorResponse.Error.Message = 'Id format is invalid'; the emulator answers HTTP 404 with 'The specified field level encryption profile does not exist.'
4. Compare ONLY $.ErrorResponse.Error.Message -- the status (404) and the error Code already match, so the fix must change the message emitted on the unreadable-identifier branch WITHOUT regressing the readable-but-absent branch.
5. To confirm both branches hold after the fix, re-replay this fixture and the sibling field-level-encryption fixtures (cloudfront-field-level-encryption-not-found-errors, cloudfront-fle-config-not-found-and-invalid-ifmatch-errors, cloudfront-field-level-encryption-lifecycle) so that a well-formed-but-absent profile id still yields the modeled absent-record sentence.
