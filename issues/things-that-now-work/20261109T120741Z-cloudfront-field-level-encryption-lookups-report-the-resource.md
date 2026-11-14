Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-field-level-encryption-not-found-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-field-level-encryption-not-found-errors cloudfront-fle-profile-lifecycle cloudfront-field-level-encryption-profile-config-get-and-not-found
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteFieldLevelEncryptionConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetFieldLevelEncryptionProfileConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# CloudFront field-level-encryption lookups report the resource missing where AWS reports an invalid id format

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFieldLevelEncryptionConfig

## Description
Four causes, one shared fault: when the id in the request path does not have CloudFront's id form, the field-level-encryption handlers go straight to a state lookup and report the resource as absent, instead of reporting that the identifier itself is malformed. The HTTP status (404) and the error code already match in all four clusters; only the message diverges, and the message is the requirement.

The open issue 20261105T231732Z-cloudfront-path-identifiers-are-not-format-validated-before.md describes this same fault for distribution/OAC/function routes; it does NOT carry the replay keys of the four FLE clusters below.

Cause 1 of 4: GetFieldLevelEncryptionConfig.
AWS: 404 whose Error.Message is the fixed text `Id format is invalid` (no variable part) - recorded behavior only; the Smithy model gives the error shape (NoSuchFieldLevelEncryptionConfig, httpError 404) but no message text, and the recording shows AWS judging the path identifier's format before any lookup.
Emulator: 404 with `The specified field level encryption config does not exist.` - the not-found message for a well-formed id.
Why: the two cases are different client contracts. A client that distinguishes 'you asked for something that cannot exist' (a bug in the caller's id handling, not retryable) from 'this id is gone' (retry/recreate) takes the wrong branch, and a test asserting the invalid-id message fails against the emulator.
Fix: GetFieldLevelEncryptionConfig's request validation - check the path id's format before the state lookup and refuse with the invalid-id-format message.
Traces: cloudfront-field-level-encryption-not-found-errors#task#1.

Cause 2 of 4: GetFieldLevelEncryptionProfile.
AWS: 404 with Error.Message `Id format is invalid` (recorded behavior only).
Emulator: `The specified field level encryption profile does not exist.`
Why/Fix: as cause 1, in GetFieldLevelEncryptionProfile's request validation.
Traces: cloudfront-field-level-encryption-not-found-errors#task#2 (2 affected steps, incl. cloudfront-fle-profile-lifecycle#task#3).

Cause 3 of 4: DeleteFieldLevelEncryptionConfig.
AWS: 404 with Error.Message `Id format is invalid` (recorded behavior only).
Emulator: `The specified field level encryption config does not exist.`
Why/Fix: as cause 1, in DeleteFieldLevelEncryptionConfig's request validation.
Traces: cloudfront-field-level-encryption-not-found-errors#task#3.

Cause 4 of 4: GetFieldLevelEncryptionProfileConfig.
AWS: 404 with Error.Message `Id format is invalid` (recorded behavior only).
Emulator: `The specified field level encryption profile does not exist.`
Why/Fix: as cause 1, in GetFieldLevelEncryptionProfileConfig's request validation.
Traces: cloudfront-field-level-encryption-profile-config-get-and-not-found#task#4.

In all four, the expected message is entirely fixed text - nothing in it comes from the request or from state - so reproducing it exactly is the whole requirement.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up). Four unowned mismatch clusters, each expected_status 404 == actual_status 404, failing only $.ErrorResponse.Error.Message:
1. GetFieldLevelEncryptionConfig - cloudfront-field-level-encryption-not-found-errors#task#1: expected 'Id format is invalid', got 'The specified field level encryption config does not exist.'
2. GetFieldLevelEncryptionProfile - cloudfront-field-level-encryption-not-found-errors#task#2: expected 'Id format is invalid', got 'The specified field level encryption profile does not exist.' (2 affected steps).
3. DeleteFieldLevelEncryptionConfig - cloudfront-field-level-encryption-not-found-errors#task#3: expected 'Id format is invalid', got 'The specified field level encryption config does not exist.'
4. GetFieldLevelEncryptionProfileConfig - cloudfront-field-level-encryption-profile-config-get-and-not-found#task#4: expected 'Id format is invalid', got 'The specified field level encryption profile does not exist.'
No stub_path and no owned_by on any of the four.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, replay the wire corpus (simparity-wire-aws-v1).
2. Representative step: fixture cloudfront-field-level-encryption-not-found-errors, step task#1 - GET /2020-05-31/field-level-encryption/<id that is not in CloudFront's id form>; the emulator answers 404 with the not-found message instead of 'Id format is invalid'.
3. Same fixture task#2 (GetFieldLevelEncryptionProfile) and task#3 (DeleteFieldLevelEncryptionConfig), and cloudfront-field-level-encryption-profile-config-get-and-not-found#task#4 (GetFieldLevelEncryptionProfileConfig) - same substitution on each route.
