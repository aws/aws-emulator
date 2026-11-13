Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.describe-import-not-found-rejected dynamodb.import-not-found dynamodb.import-table-malformed-item-reports-exact-failure dynamodb.restore-to-earliest-restorable-time-excludes-later-writes dynamodb.transact-write-expression-parse-errors-rejected-top-level
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.describe-import-not-found-rejected|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.import-not-found|run#21|AssertionError|assert err.get('Message') == expected, (
Issue-Key: dynamodb.import-table-malformed-item-reports-exact-failure|run#16|AssertionError|assert described["ImportStatus"] == "FAILED", (
Issue-Key: dynamodb.restore-to-earliest-restorable-time-excludes-later-writes|run#30|AssertionError|assert item_b is None, (
Issue-Key: dynamodb.transact-write-expression-parse-errors-rejected-top-level|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# dynamodb: 4 net-new causes — ImportNotFoundException message, ImportTable item validation, RestoreTableToPointInTime ignores RestoreDateTime, TransactWriteItems accepts malformed expressions

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImport

## Description
Four net-new root causes in the DynamoDB emulator, surfaced by four behavioral checks that are not covered by any open issue. Each cause must be closed before this issue is submitted.

Cause 1 of 4 — ImportNotFoundException carries the wrong message text.
Failing checks: dynamodb.describe-import-not-found-rejected, dynamodb.import-not-found
AWS: Smithy (dynamodb-2012-08-10): DescribeImport.ImportArn is required, @length 37..1024; the only modeled error is ImportNotFoundException (client, HTTP 400, doc "The specified import was not found") whose single member is `message`. The message AWS serves for a well-formed-but-unknown import ARN has the fixed text `Import not found: Import with arn: ` + the requested ImportArn + ` doesn't exist`. The only variable part of that template is the ImportArn echoed from the request; the rest is fixed text.
Emulator: returns the right code (ImportNotFoundException) but the message `Import not found: <ImportArn>` — the fixed segments `Import with arn: ` and ` doesn't exist` are missing entirely.
Why: clients and test suites that match on the documented message (or that parse the "Import with arn: ... doesn't exist" shape to extract the ARN) do not recognise the refusal; log/alarm patterns built against AWS text never fire.
Fix: the DescribeImport handler's not-found refusal path in the dynamodb service crate — the message template used when the ImportArn lookup misses.
Traces: dynamodb.describe-import-not-found-rejected (asserted `Import not found: Import with arn: <arn> doesn't exist`, got `Import not found: <arn>`); dynamodb.import-not-found (same divergence with a never-started import id under a real TableArn).

Cause 2 of 4 — ImportTable never validates item attribute values, so a source file with a malformed item still ends COMPLETED.
Failing check: dynamodb.import-table-malformed-item-reports-exact-failure
AWS: Smithy: ImportStatus enum is IN_PROGRESS | COMPLETED | CANCELLING | CANCELLED | FAILED; ImportTableDescription carries ErrorCount (long, default 0), FailureCode and FailureMessage ("The error code corresponding to the failure that the import job ran into during execution"). For a DYNAMODB_JSON source holding an item whose attribute is typed N but whose value is not a number, the import terminates FAILED with ErrorCount 1, FailureCode `ItemValidationError` and the fixed FailureMessage `Some of the items failed validation checks and were not imported. Please check CloudWatch error logs for more details.` (fixed text, no variable parts), while items preceding the malformed one are imported and readable.
Emulator: the import reaches terminal status COMPLETED. ErrorCount/FailureCode/FailureMessage are therefore never populated, and the malformed item is treated as importable.
Why: a client that imports data and gates on ImportStatus == COMPLETED concludes a corrupt source file loaded cleanly; the per-item validation contract (and the FailureCode a caller branches on) is unobservable, so bad data silently enters the table.
Fix: the ImportTable ingestion/validation path and the import-job state machine in the dynamodb service crate — attribute-value validation per imported item, the error counter, and the terminal FAILED projection into ImportTableDescription.
Traces: dynamodb.import-table-malformed-item-reports-exact-failure (`expected an import with one malformed item to end FAILED, got 'COMPLETED'`).

Cause 3 of 4 — RestoreTableToPointInTime ignores RestoreDateTime and restores current table contents.
Failing check: dynamodb.restore-to-earliest-restorable-time-excludes-later-writes
AWS: Smithy: RestoreTableToPointInTime takes RestoreDateTime (timestamp) or UseLatestRestorableTime, and refuses an out-of-window time with InvalidRestoreTimeException (client, 400). AWS restores the table as it stood at RestoreDateTime: an item written before that instant is present in the target table, and an item written after it is absent. The accepted request also echoes the requested instant back as RestoreSummary.RestoreDateTime.
Emulator: the restore is accepted and the target table becomes ACTIVE, but a consistent GetItem on the target returns the item that was written AFTER the requested RestoreDateTime — i.e. the restore copies the live table rather than reconstructing the point-in-time snapshot.
Why: point-in-time recovery is the operation a client uses to undo a bad write; an emulator that returns post-restore-point data makes every recovery drill pass while proving nothing, and masks application logic that depends on a restored table NOT containing later writes.
Fix: RestoreTableToPointInTime's restore path and the continuous-backup/PITR state in the dynamodb service crate — the target table's contents must be materialised from the requested RestoreDateTime, not copied from the source's current state.
Traces: dynamodb.restore-to-earliest-restorable-time-excludes-later-writes (`item written after the restore time must be excluded from the restored table, got {'pk': {'S': 'item-b'}, 'val': {'S': 'after-restore-time'}}`; the item written before the restore time was correctly present).

Cause 4 of 4 — TransactWriteItems accepts expression parse defects instead of refusing the whole call with a top-level ValidationException.
Failing check: dynamodb.transact-write-expression-parse-errors-rejected-top-level
AWS: recorded behavior plus the DynamoDB reserved-words contract (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/ReservedWords.html). A parse-level defect in ANY TransactItems action is a request-level failure: HTTP 400, error code ValidationException, no CancellationReasons member, and nothing in the transaction is written. The messages are the single-item expression messages WITHOUT any per-item index prefix; their fixed text with the offending token taken from the request is: `Invalid UpdateExpression: Attribute name is a reserved keyword; reserved keyword: <name>`, `Invalid ConditionExpression: Attribute name is a reserved keyword; reserved keyword: <name>`, `Invalid UpdateExpression: Syntax error; token: "<token>", near: "<context>"`, and `Invalid ConditionExpression: The expression has redundant parentheses;`. Smithy: TransactWriteItems models TransactionCanceledException for per-item cancellation, which is NOT the class used for a parse defect.
Emulator: the call SUCCEEDS — HTTP 200 with an empty body (`{}`) — for a TransactItems list pairing a healthy Put with an action whose UpdateExpression/ConditionExpression contains a reserved keyword (`SET status = :v` / `status = :v`), a truncated expression (`SET foo =`) or redundant parentheses (`((pk = :k))`). No ValidationException is raised at all.
Why: a client gets a 200 for a transaction AWS refuses, so the healthy sibling Put lands when AWS would have written nothing, and the expression that was supposed to be applied is silently mis-evaluated or dropped. Code that relies on a transaction being all-or-nothing diverges on real AWS.
Fix: the TransactWriteItems handler's request validation in the dynamodb service crate — every action's UpdateExpression/ConditionExpression must go through the same expression parser and reserved-keyword check as the single-item operations, and a parse defect must become a top-level ValidationException (no CancellationReasons) before any action is applied. Note: the open issue 20261103T065250Z-dynamodb-expression-engine-10-root-causes-in-expression.md covers expression-engine causes for single-item operations; this check is not owned by it, and the defect here is that TransactWriteItems bypasses that validation entirely.
Traces: dynamodb.transact-write-expression-parse-errors-rejected-top-level (`expected the call to be refused, but it succeeded: {... 'HTTPStatusCode': 200 ... 'content-length': '2' ...}`).

## Evidence
Cause 1 — dynamodb.describe-import-not-found-rejected:
  expected message "Import not found: Import with arn: arn:aws:dynamodb:<region>:<account>:table/<table>/import/<import-id> doesn't exist"
  got            'Import not found: arn:aws:dynamodb:<region>:<account>:table/<table>/import/<import-id>'
  (error code ImportNotFoundException was correct; only the message diverges)
dynamodb.import-not-found: identical divergence for a never-started import ARN built from the real TableArn.

Cause 2 — dynamodb.import-table-malformed-item-reports-exact-failure:
  expected DescribeImport(...).ImportTableDescription.ImportStatus == 'FAILED', ErrorCount == 1,
           FailureCode == 'ItemValidationError',
           FailureMessage == 'Some of the items failed validation checks and were not imported. Please check CloudWatch error logs for more details.'
  got      ImportStatus == 'COMPLETED'  (source line 2 was {"Item": {"pk": {"S": "row-2"}, "value": {"N": "not-a-number"}}})

Cause 3 — dynamodb.restore-to-earliest-restorable-time-excludes-later-writes:
  expected GetItem(target, pk=item-b, ConsistentRead) -> no Item (written after RestoreDateTime)
  got      {'pk': {'S': 'item-b'}, 'val': {'S': 'after-restore-time'}}
  (the pre-restore-point item-a restored correctly, so the restore ran but ignored the requested instant)

Cause 4 — dynamodb.transact-write-expression-parse-errors-rejected-top-level:
  expected ValidationException / HTTP 400, CancellationReasons absent, nothing written, message e.g.
           'Invalid UpdateExpression: Attribute name is a reserved keyword; reserved keyword: status'
  got      HTTP 200 with body '{}' for TransactItems=[healthy Put, Update with UpdateExpression 'SET status = :v']

## Steps to Reproduce
Cause 1: ImportTable a DYNAMODB_JSON object from S3 into a new table; take ImportArn from the response; call DescribeImport with a well-formed ARN that was never created (perturb the import id's last character). Inspect Error.Code and Error.Message.

Cause 2: PutObject a DYNAMODB_JSON file with two lines, the second holding {"value": {"N": "not-a-number"}}; ImportTable it with InputFormat=DYNAMODB_JSON, InputCompressionType=NONE; poll DescribeImport until the status is terminal and read ImportStatus/ErrorCount/FailureCode/FailureMessage.

Cause 3: CreateTable; PutItem item-a; UpdateContinuousBackups PointInTimeRecoveryEnabled=true; read EarliestRestorableDateTime from DescribeContinuousBackups; wait past it; PutItem item-b; RestoreTableToPointInTime(SourceTableName, TargetTableName, RestoreDateTime=EarliestRestorableDateTime); wait for the target ACTIVE; GetItem(target, pk=item-b, ConsistentRead=true) — it must be absent.

Cause 4: CreateTable keyed pk(S); PutItem {pk:'seed', status:'active', foo:'orig'}; call TransactWriteItems with TransactItems=[{Put: healthy side item}, {Update: {Key: pk=seed, UpdateExpression: 'SET status = :v', ExpressionAttributeValues: {':v': {'S':'active'}}}}]. Repeat with a ConditionCheck/Put/Delete using ConditionExpression 'status = :v', with UpdateExpression 'SET foo =', and with ConditionExpression '((pk = :k))'. Each must be refused with a top-level ValidationException and must leave the side Put unwritten.
