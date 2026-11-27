Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-delete-item-expected-map-key-length
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-delete-item-expected-map-key-length dynamodb-export-filter-expression-attribute-name-value-length dynamodb-restore-from-backup-backup-arn-length-bounds dynamodb-start-bulk-put-items-output-kms-key-id-length-limit dynamodb-update-table-replica-autoscaling-gsi-maximum-units-floor
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.__type
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->4xx|$.__type
Issue-Key: dynamodb|RestoreTableFromBackup|4xx->4xx|$.__type
Issue-Key: dynamodb|StartBulkPutItems|4xx->4xx|$.__type
Issue-Key: dynamodb|UpdateTableReplicaAutoScaling|4xx->4xx|$.__type
Issue-Key: dynamodb|UpdateTableReplicaAutoScaling|4xx->4xx|$.message
Issue-Related: 20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md
Issue-Related: 20261127T204751Z-dynamodb-arn-nexttoken-and-indexname-refusals-emit-the.md
Issue-Related: 20261127T204751Z-dynamodb-length-constraint-validationexception-messages-abbreviate-the-rejected.md
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# DynamoDB evaluates resource lookup and condition evaluation before input validation, so refusals take the wrong error class

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteItem

## Description
DynamoDB validates a request's shape and values BEFORE it looks the target resource up and before it evaluates any condition, so a malformed request is always refused as ValidationException (`com.amazon.coral.validate#ValidationException`). The emulator runs those later stages first for the operations below, so a request AWS refuses as invalid comes back as a condition failure or a not-found error. The HTTP status is 400 on both sides; the error CODE and the message are wrong, and those are requirements.

THIS BUNDLE CONTAINS 5 CAUSES.

Cause 1 of 5: DeleteItem with an over-long Expected attribute name is answered as a condition failure.
AWS: Smithy: DeleteItem.Expected -> ExpectedAttributeMap, keyed by AttributeName; expression/attribute names are bounded at 65535 bytes (ExpressionAttributeNameMap value -> AttributeName @length 0..65535); modeled errors include ConditionalCheckFailedException and the service-common ValidationException. An attribute name past that bound is refused 400 `com.amazon.coral.validate#ValidationException` with `1 validation error detected: Attribute name is too large, must be less than 65536 bytes` - fixed text, limit from the model.
Emulator: 400 `com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException` with `The conditional request failed` - it evaluated the Expected clause against the stored item instead of rejecting the name.
Why: the caller is told its condition did not hold, which is false and retryable-looking, instead of that its request is malformed; SDK error-handling branches on the exception class, so the bug is invisible until the same code runs against AWS.
Fix: the DeleteItem handler's ordering - run the Expected/ExpressionAttributeNames validation in the request-validation stage, before the condition evaluator is reached.
Traces: dynamodb-delete-item-expected-map-key-length#task#4.

Cause 2 of 5: ExportTableToPointInTime with an over-long FilterSpecification expression-attribute key is answered as a missing table.
AWS: Smithy: ExportTableToPointInTime.FilterSpecification; expression attribute map keys are bounded (AttributeName @length 0..65535); modeled errors include TableNotFoundException. An over-long key is refused 400 `com.amazon.coral.validate#ValidationException` with `Invalid Request: The expression attribute map contains a key that is too long` - fixed text, no variable parts.
Emulator: 400 `com.amazonaws.dynamodb.v20120810#TableNotFoundException` with `Table not found: <tableArn>` (the ARN echoed from the request).
Why: the refusal blames the caller's table rather than its filter, so an export that is simply malformed looks like a provisioning problem.
Fix: the ExportTableToPointInTime handler's ordering - validate FilterSpecification before resolving TableArn.
Traces: dynamodb-export-filter-expression-attribute-name-value-length#task#1.

Cause 3 of 5: RestoreTableFromBackup with a malformed in-range BackupArn is answered as a missing backup.
AWS: Smithy: RestoreTableFromBackup.BackupArn -> BackupArn @length 37..1024, required; modeled error BackupNotFoundException. A value inside the length bounds that is not a backup ARN is still refused 400 `com.amazon.coral.validate#ValidationException` with `Invalid Backup ARN` - BackupNotFoundException is for a well-formed ARN that names nothing.
Emulator: 400 `com.amazonaws.dynamodb.v20120810#BackupNotFoundException` with `Backup not found: <the supplied arn>`.
Why: the caller cannot distinguish "this string is not a backup ARN" from "this backup is gone", and retry/recreate logic keyed on BackupNotFoundException fires for a malformed request.
Fix: the RestoreTableFromBackup handler - run the BackupArn well-formedness check in validation, before the backup-store lookup (see also the `Invalid Backup ARN` wording cause filed under the ARN-message bundle for this same fixture's task#6).
Traces: dynamodb-restore-from-backup-backup-arn-length-bounds#task#10.

Cause 4 of 5: StartBulkPutItems with an over-long output KMS key id is answered as a missing resource.
AWS: Smithy: StartBulkPutItems.OutputS3Configuration (required) carries the SSE KMS key id, bounded by its @length trait; modeled errors include ResourceNotFoundException. A key id past the maximum is refused 400 `com.amazon.coral.validate#ValidationException` with `1 validation error detected: Value '<the supplied key id>' at '<memberPath>' failed to satisfy constraint: Member must have length less than or equal to <max>`.
Emulator: 400 `com.amazonaws.dynamodb.v20120810#ResourceNotFoundException`.
Why: as Cause 3 - a malformed field is reported as a missing resource, so the caller looks for the wrong problem.
Fix: the StartBulkPutItems handler - validate OutputS3Configuration before any table/key resolution.
Traces: dynamodb-start-bulk-put-items-output-kms-key-id-length-limit#task#2 (cluster also covers task#3).

Cause 5 of 5: UpdateTableReplicaAutoScaling with an incomplete auto-scaling update is answered as a missing global table.
AWS: Smithy: UpdateTableReplicaAutoScaling.GlobalSecondaryIndexUpdates @length min 1, ReplicaUpdates @length min 1, TableName required; modeled errors ResourceNotFoundException, ResourceInUseException, LimitExceededException. An update that omits ScalingPolicyUpdate while not disabling auto scaling is refused 400 `com.amazon.coral.validate#ValidationException` with `Failed to update settings for global table with name: \u2018<tableName>\u2019: Parameters 'ScalingPolicyUpdate' are required unless auto scaling is being disabled.` - the fixed text is everything but the table name, which is taken from the request and quoted with the single typographic quotes AWS uses here.
Emulator: 400 `com.amazonaws.dynamodb.v20120810#ResourceNotFoundException` with `Global table with name: '<tableName>' does not exist.` - the existing table is reported as absent because the existence lookup runs first (and treats the table as not a global table).
Why: a caller is told its table does not exist when it does, so it recreates or aborts instead of correcting the omitted member.
Fix: the UpdateTableReplicaAutoScaling handler - validate the auto-scaling update members (ScalingPolicyUpdate presence) in the validation stage, before the global-table lookup.
Traces: dynamodb-update-table-replica-autoscaling-gsi-maximum-units-floor#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All five clusters are verdict=mismatch, status 400 -> 400, stub_path empty, owned_by empty; in four of them BOTH $.__type and $.message diverge.

Cause 1 - dynamodb-delete-item-expected-map-key-length#task#4 (affected_count 1):
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException'
  $.message: expected '1 validation error detected: Attribute name is too large, must be less than 65536 bytes', got 'The conditional request failed'

Cause 2 - dynamodb-export-filter-expression-attribute-name-value-length#task#1 (affected_count 1):
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#TableNotFoundException'
  $.message: expected 'Invalid Request: The expression attribute map contains a key that is too long', got 'Table not found: arn:aws:dynamodb:<region>:<account>:table/<table>'

Cause 3 - dynamodb-restore-from-backup-backup-arn-length-bounds#task#10 (affected_count 1):
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#BackupNotFoundException'
  $.message: expected 'Invalid Backup ARN', got 'Backup not found: arn:aws:dynamodb:<region>:<account>:table/<table>/backup/<digits>'

Cause 4 - dynamodb-start-bulk-put-items-output-kms-key-id-length-limit#task#2 (affected_count 2):
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException'
  $.message: expected the "1 validation error detected: Value '<key id>' ... must have length less than or equal to <max>" constraint sentence

Cause 5 - dynamodb-update-table-replica-autoscaling-gsi-maximum-units-floor#task#5 (affected_count 1):
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException'
  $.message: expected "Failed to update settings for global table with name: \u2018<table>\u2019: Parameters 'ScalingPolicyUpdate' are required unless auto scaling is being disabled.", got "Global table with name: '<table>' does not exist."

Model citations (dynamodb-20120810): DeleteItem.Expected -> ExpectedAttributeMap with ConditionalCheckFailedException modeled; ExpressionAttributeNameMap value -> AttributeName @length 0..65535; ExportTableToPointInTime errors include TableNotFoundException; RestoreTableFromBackup.BackupArn @length 37..1024 with BackupNotFoundException modeled; StartBulkPutItems.OutputS3Configuration required with ResourceNotFoundException modeled; UpdateTableReplicaAutoScaling.GlobalSecondaryIndexUpdates/@length min 1 with ResourceNotFoundException modeled.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: DeleteItem on an existing item with an Expected map key of 65536+ bytes (fixture dynamodb-delete-item-expected-map-key-length, step task#4); assert __type ValidationException and the 'Attribute name is too large, must be less than 65536 bytes' message, not ConditionalCheckFailedException.
3. Cause 2: ExportTableToPointInTime with a FilterSpecification expression-attribute key over 65535 bytes (fixture dynamodb-export-filter-expression-attribute-name-value-length, step task#1); assert ValidationException / 'Invalid Request: The expression attribute map contains a key that is too long'.
4. Cause 3: RestoreTableFromBackup with a BackupArn inside [37,1024] that is not a backup ARN (fixture dynamodb-restore-from-backup-backup-arn-length-bounds, step task#10); assert ValidationException / 'Invalid Backup ARN'.
5. Cause 4: StartBulkPutItems with an over-long OutputS3Configuration KMS key id (fixture dynamodb-start-bulk-put-items-output-kms-key-id-length-limit, steps task#2, task#3); assert ValidationException.
6. Cause 5: UpdateTableReplicaAutoScaling on an existing table with a GSI auto-scaling update that omits ScalingPolicyUpdate and does not disable auto scaling (fixture dynamodb-update-table-replica-autoscaling-gsi-maximum-units-floor, step task#5); assert ValidationException and the 'Parameters ScalingPolicyUpdate are required unless auto scaling is being disabled.' sentence.
7. All five causes must pass before closing.
