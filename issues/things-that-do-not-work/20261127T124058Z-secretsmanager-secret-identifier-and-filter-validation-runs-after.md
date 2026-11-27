Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-get-secret-value-malformed-secret-id-forms
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-get-secret-value-malformed-secret-id-forms secretsmanager-put-secret-value-malformed-secret-id-mints-no-version secretsmanager-batch-get-secret-value-malformed-id-beside-a-real-secret secretsmanager-delete-resource-policy-secret-id-length-edges secretsmanager-batch-get-secret-value-filter-key-outside-enum-rejected secretsmanager-batch-get-secret-value-filter-value-at-512-serves-513-rejected secretsmanager-below-minimum-lengths-refused
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|GetSecretValue|4xx->4xx|$.__type
Issue-Key: secretsmanager|GetSecretValue|4xx->4xx|$.message
Issue-Key: secretsmanager|PutSecretValue|4xx->4xx|$.__type
Issue-Key: secretsmanager|BatchGetSecretValue|2xx->2xx|$.Errors[*].ErrorCode
Issue-Key: secretsmanager|BatchGetSecretValue|2xx->2xx|$.Errors[*].Message
Issue-Key: secretsmanager|BatchGetSecretValue|2xx->2xx|$.Errors[*]
Issue-Key: secretsmanager|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: secretsmanager|DeleteResourcePolicy|4xx->4xx|$.Message
Issue-Key: secretsmanager|DeleteResourcePolicy|4xx->4xx|$.__type
Issue-Key: secretsmanager|BatchGetSecretValue|4xx->4xx|$.Message
Issue-Key: secretsmanager|BatchGetSecretValue|4xx->4xx|$.__type
Issue-Related: 20261127T124058Z-secretsmanager-the-schema-validation-message-renderer-drops-the.md
Issue-Commit: 34735640b265050797646a7031263995ba2fdc83

# secretsmanager: secret-identifier and filter validation runs after lookup, so malformed input answers ResourceNotFoundException instead of AWS's ValidationException/InvalidParameterException

> _Note: this issue was identified against the code base at commit `34735640b265050797646a7031263995ba2fdc83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
7 net-new causes sharing ONE fault: the emulator does not validate the SYNTAX of a SecretId (or the enum domain of a Filter.Key) before it looks the secret up. Every malformed identifier therefore falls through to the store, misses, and comes back as ResourceNotFoundException — where AWS refuses it on syntax, with a different error code and a different message. The error code is the thing clients branch on (a syntax error is permanent and must not be retried or treated as "create it"; a not-found is a different remediation entirely), so this is a behavioural divergence, not a wording one.

Cause 1 of 7: GetSecretValue with a malformed SecretId.
AWS: Smithy GetSecretValue.SecretId -> SecretIdType, @required @length {min:1,max:2048}; modeled errors include ValidationException-class schema refusals and ResourceNotFoundException. A SecretId that is neither a well-formed secret name nor a secret ARN is refused on grammar before any lookup: HTTP 400, $.__type "ValidationException", $.message "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!". That message is entirely FIXED text — it has no variable part and must be reproduced verbatim.
Emulator: HTTP 400 but $.__type "ResourceNotFoundException" and $.message "Secrets Manager can't find the specified secret."
Why: a client that sent a syntactically illegal id is told the secret does not exist, so it retries, or creates a secret under that illegal name, instead of correcting its input. Error-code branching (ValidationException vs ResourceNotFoundException) is inverted.
Fix: GetSecretValue's input validation — add a SecretId grammar check that runs BEFORE the state lookup.
Traces: secretsmanager-get-secret-value-malformed-secret-id-forms#task#3 (cluster also covers task#4 and task#6 of that fixture).

Cause 2 of 7: PutSecretValue with a malformed SecretId.
AWS: Smithy PutSecretValue.SecretId -> SecretIdType @required @length{1,2048}. Same grammar refusal as cause 1: 400, $.__type "ValidationException", $.message "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!". Critically, AWS mints NO new secret version for the refused call.
Emulator: 400 with $.__type "ResourceNotFoundException" and $.message "Secrets Manager can't find the specified secret."
Why: as cause 1, plus the error code no longer distinguishes "your id is illegal" from "the secret is gone", which drives different client recovery paths.
Fix: PutSecretValue's input validation, same pre-lookup SecretId grammar check.
Traces: secretsmanager-put-secret-value-malformed-secret-id-mints-no-version#task#2 (cluster also covers task#3 of that fixture).

Cause 3 of 7: BatchGetSecretValue per-entry ErrorCode/Message for a malformed id.
AWS: Smithy BatchGetSecretValue output Errors -> APIErrorListType of APIErrorType {SecretId, ErrorCode, Message}; the member doc names ResourceNotFoundException, InvalidParameterException, InvalidRequestException, DecryptionFailure and AccessDeniedException as per-entry codes. A malformed id in SecretIdList does NOT fail the whole call: the batch answers 200 and the bad entry appears in Errors with ErrorCode "ValidationException" and Message "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!" (fixed text; SecretId is echoed from the request).
Emulator: 200, but Errors[0].ErrorCode "ResourceNotFoundException" and Errors[0].Message "Secrets Manager can't find the specified secret."
Why: the batch caller cannot tell a malformed id (fix the request) from a missing secret (provision it), and the two have opposite remediations.
Fix: BatchGetSecretValue's per-entry resolution — classify an id that fails the name/ARN grammar as ValidationException before the store lookup.
Traces: secretsmanager-batch-get-secret-value-malformed-id-beside-a-real-secret#task#2.

Cause 4 of 7: the same BatchGetSecretValue Errors entry diverges wholesale on a later step of the same scenario.
AWS: the single Errors entry is {SecretId: <the ARN-shaped id echoed from the request>, ErrorCode: "ValidationException", ErrorMessage text "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!"}. SecretId echoes the request; the other two are fixed for this error class.
Emulator: the entry carries the same SecretId but ErrorCode "ResourceNotFoundException" and Message "Secrets Manager can't find the specified secret."
Why: as cause 3 — and because this step asserts the whole Errors[0] object, a consumer deserialising APIErrorType gets a wholly different classification for the entry.
Fix: same component as cause 3.
Traces: secretsmanager-batch-get-secret-value-malformed-id-beside-a-real-secret#task#3.

Cause 5 of 7: DeleteResourcePolicy with an EMPTY SecretId reports the schema length violation instead of the name-grammar refusal.
AWS: Smithy DeleteResourcePolicy.SecretId -> SecretIdType @required @length{min:1,max:2048}. For the empty string AWS does NOT emit the "1 validation error detected" length message; it emits the name-grammar refusal, 400 with $.message "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!" (fixed text). AWS's name grammar check precedes the @length{min:1} schema check for this member.
Emulator: 400 with $.message "1 validation error detected: Value '' at 'secretId' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: the message a client surfaces/matches is the wrong one of two AWS messages, and the ordering difference means an empty id is reported as a schema problem rather than a name problem.
Fix: DeleteResourcePolicy's input validation — order the SecretId name-grammar check ahead of the generic @length check.
Traces: secretsmanager-delete-resource-policy-secret-id-length-edges#task#3.

Cause 6 of 7: DeleteResourcePolicy with an OVER-LENGTH SecretId answers the wrong error type and message.
AWS: SecretIdType @length{max:2048}; DeleteResourcePolicy models InvalidParameterException (400). A SecretId longer than the limit is refused 400 with $.__type "InvalidParameterException" and $.Message "SecretId exceeds size limit." — fixed text, no variable part, and note the capitalised "Message" key for this error shape.
Emulator: 400 with $.__type "ResourceNotFoundException" and $.Message "Secrets Manager can't find the specified secret."
Why: an over-long identifier is a client bug AWS reports as an invalid parameter; the emulator reports it as a missing resource, so a client retries or provisions rather than truncating/fixing the id.
Fix: DeleteResourcePolicy's input validation — enforce the SecretId size limit pre-lookup and raise InvalidParameterException.
Traces: secretsmanager-delete-resource-policy-secret-id-length-edges#task#4 (cluster also covers task#5 of that fixture).

Cause 7 of 7: BatchGetSecretValue with a Filter.Key outside the enum answers the wrong error type and omits the message member.
AWS: Smithy Filter.Key -> FilterNameStringType, an enum whose values are exactly description, name, tag-key, tag-value, primary-region, owning-service, all. A key outside that set (e.g. "tag_value") is refused 400 with $.__type "InvalidParameterException" and $.Message "Invalid filter Key: <key>" — fixed text "Invalid filter Key: " plus the rejected key copied from the request.
Emulator: 400 with $.__type "ValidationException" and NO $.Message member at all (it serialises the lowercase $.message key for this error instead).
Why: both the error code and the error-message JSON key differ, so an SDK that reads Message gets null and a client branching on InvalidParameterException never matches.
Fix: BatchGetSecretValue's filter validation plus the error serialiser for InvalidParameterException (member name "Message", not "message").
Traces: secretsmanager-batch-get-secret-value-filter-key-outside-enum-rejected#task#3 (cluster also covers secretsmanager-batch-get-secret-value-filter-value-at-512-serves-513-rejected#task#3 and secretsmanager-below-minimum-lengths-refused#task#7).

No open issue carries these replay keys. The closest open items — 20261123T140313Z-secretsmanager-listsecrets-validationexception-for-an-unknown-sortby-lists (ListSecrets SortBy enum ordering) and 20261127T103003Z-kms-secretsmanager-3-causes-kms-failures-are-reported — touch neighbouring error paths but neither covers SecretId grammar/size classification or the BatchGetSecretValue filter-key refusal.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 45/484 steps mismatched). Expected vs actual, verbatim from the divergence clusters:

Cause 1 — GetSecretValue, 400 -> 400:
  $.__type: expected 'ValidationException', got 'ResourceNotFoundException'
  $.message: expected 'Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!', got "Secrets Manager can't find the specified secret."

Cause 2 — PutSecretValue, 400 -> 400: identical pair of mismatches on $.__type and $.message.

Cause 3 — BatchGetSecretValue, 200 -> 200:
  $.Errors[0].ErrorCode: expected 'ValidationException', got 'ResourceNotFoundException'
  $.Errors[0].Message: expected 'Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!', got "Secrets Manager can't find the specified secret."

Cause 4 — BatchGetSecretValue, 200 -> 200:
  $.Errors[0]: expected {'ErrorCode': 'ValidationException', 'Message': 'Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!', 'SecretId': <the request's id>}, got {'SecretId': <same id>, 'ErrorCode': 'ResourceNotFoundException', 'Message': "Secrets Manager can't find the specified secret."}

Cause 5 — DeleteResourcePolicy, 400 -> 400:
  $.message: expected 'Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!', got "1 validation error detected: Value '' at 'secretId' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 6 — DeleteResourcePolicy, 400 -> 400:
  $.__type: expected 'InvalidParameterException', got 'ResourceNotFoundException'
  $.Message: expected 'SecretId exceeds size limit.', got "Secrets Manager can't find the specified secret."

Cause 7 — BatchGetSecretValue, 400 -> 400:
  $.__type: expected 'InvalidParameterException', got 'ValidationException'
  $.Message: expected 'Invalid filter Key: tag_value', got None (member absent)

Model citations (smithy_query on services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): SecretIdType = string @length{min:1,max:2048}; GetSecretValue/PutSecretValue/DeleteResourcePolicy all take SecretId @required and model both InvalidParameterException and ResourceNotFoundException at httpError 400; BatchGetSecretValue.Errors -> APIErrorListType of APIErrorType{SecretId, ErrorCode, Message}; Filter.Key -> FilterNameStringType enum values [description, name, tag-key, tag-value, primary-region, owning-service, all].

## Steps to Reproduce
1. git clone the repo; boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 corpus.
3. Cause 1: secretsmanager-get-secret-value-malformed-secret-id-forms#task#3 — GetSecretValue with a SecretId that is neither a legal secret name nor an ARN; expect 400 / ValidationException / "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!".
4. Cause 2: secretsmanager-put-secret-value-malformed-secret-id-mints-no-version#task#2 — same id form on PutSecretValue; expect the same refusal and no new version minted.
5. Causes 3 and 4: secretsmanager-batch-get-secret-value-malformed-id-beside-a-real-secret#task#2 and #task#3 — BatchGetSecretValue with one malformed id alongside one real secret; expect 200 and Errors[0].ErrorCode == ValidationException.
6. Cause 5: secretsmanager-delete-resource-policy-secret-id-length-edges#task#3 — DeleteResourcePolicy with SecretId=""; expect the "Invalid name..." message, not the "1 validation error detected" length message.
7. Cause 6: secretsmanager-delete-resource-policy-secret-id-length-edges#task#4 — DeleteResourcePolicy with a SecretId longer than 2048 chars; expect 400 / InvalidParameterException / "SecretId exceeds size limit.".
8. Cause 7: secretsmanager-batch-get-secret-value-filter-key-outside-enum-rejected#task#3 — BatchGetSecretValue with Filters[0].Key="tag_value"; expect 400 / InvalidParameterException with Message "Invalid filter Key: tag_value".
