Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-describe-backup-arn-length-bounds
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-describe-backup-arn-length-bounds dynamodb-describe-export-malformed-arn-rejected dynamodb-export-table-to-point-in-time-invalid-table-arn-and-missing-client-token dynamodb-restore-from-backup-backup-arn-length-bounds dynamodb-restore-from-backup-gsi-override-non-key-attribute-count dynamodb-list-imports-next-token-length-bounds dynamodb-list-imports-next-token-pattern dynamodb-contributor-insights-index-name-length-limit
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeBackup|4xx->4xx|$.message
Issue-Key: dynamodb|DescribeExport|4xx->4xx|$.message
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->4xx|$.message
Issue-Key: dynamodb|RestoreTableFromBackup|4xx->4xx|$.message
Issue-Key: dynamodb|ListImports|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateContributorInsights|4xx->4xx|$.message
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# DynamoDB ARN, NextToken and IndexName refusals emit the generic Smithy constraint sentence instead of DynamoDB's own message

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackup

## Description
For resource identifiers (ARNs), opaque pagination tokens and IndexName, DynamoDB does NOT report a modeled length bound: it answers its own short sentence, with no `N validation error(s) detected:` envelope and no member path. The emulator instead renders the generic Smithy constraint text derived from the member's @length trait, so the refusal conveys a different rule than AWS's and leaks an internal member path. Status (400) and error code (ValidationException) match in every cause below; only the message diverges.

THIS BUNDLE CONTAINS 6 CAUSES.

Cause 1 of 6: DescribeBackup with a malformed BackupArn.
AWS: Smithy: DescribeBackup.BackupArn -> BackupArn @length min 37 max 1024, required; modeled error BackupNotFoundException. A BackupArn that is not a backup ARN is refused 400 ValidationException with the fixed text `Invalid Backup ARN` - a constant with no variable parts, emitted at every length including below the modeled minimum and above the maximum.
Emulator: `1 validation error detected: Value '<the supplied value>' at 'backupArn' failed to satisfy constraint: Member must have length greater than or equal to 37`.
Why: a client that branches on AWS's sentence sees an unrelated one, and the emulator asserts a length rule where AWS asserts that the string is not a backup ARN at all - so a caller "fixes" the length and is refused again.
Fix: the DescribeBackup input validation in the DynamoDB request-validation component - the BackupArn check must produce the constant `Invalid Backup ARN` for any malformed value at any length, ahead of the generic @length check.
Traces: dynamodb-describe-backup-arn-length-bounds#task#6 (cluster also covers task#9).

Cause 2 of 6: DescribeExport with a malformed ExportArn.
AWS: Smithy: DescribeExport.ExportArn -> ExportArn @length min 37 max 1024, required; modeled error ExportNotFoundException. A malformed export ARN is refused 400 ValidationException `Invalid Export ARN` - fixed text, no variable parts.
Emulator: `1 validation error detected: Value '<abbreviated ARN>' at 'exportArn' failed to satisfy constraint: Member must have length less than or equal to 1024`.
Why: as Cause 1.
Fix: the DescribeExport input validation; same component as Cause 1.
Traces: dynamodb-describe-export-malformed-arn-rejected#task#1.

Cause 3 of 6: ExportTableToPointInTime with an invalid TableArn.
AWS: Smithy: ExportTableToPointInTime.TableArn -> TableArn @length min 1 max 1024, required. An invalid table ARN is refused 400 ValidationException `Invalid TableArn: Invalid Table ARN` - fixed text naming the member and then the reason.
Emulator: `1 validation error detected: Value '' at 'tableArn' failed to satisfy constraint: Member must have length greater than or equal to 1`.
Why: as Cause 1 - and because the emulator checks only the modeled minimum, every TableArn that is long enough but not an ARN takes a different path entirely.
Fix: the ExportTableToPointInTime input validation; same component as Cause 1.
Traces: dynamodb-export-table-to-point-in-time-invalid-table-arn-and-missing-client-token#task#3 (cluster also covers task#4).

Cause 4 of 6: RestoreTableFromBackup with a malformed BackupArn.
AWS: Smithy: RestoreTableFromBackup.BackupArn -> BackupArn @length min 37 max 1024, required; modeled errors BackupNotFoundException, BackupInUseException. A malformed ARN is refused 400 ValidationException `Invalid Backup ARN`.
Emulator: `1 validation error detected: Value '<the supplied value>' at 'backupArn' failed to satisfy constraint: Member must have length greater than or equal to 37`.
Why: as Cause 1; the same wrong rule is reported on the restore path as on the describe path.
Fix: share one BackupArn validator between DescribeBackup and RestoreTableFromBackup so both emit `Invalid Backup ARN`.
Traces: dynamodb-restore-from-backup-backup-arn-length-bounds#task#6 (cluster also covers task#9, task#11 and dynamodb-restore-from-backup-gsi-override-non-key-attribute-count#task#1).

Cause 5 of 6: ListImports with an undecodable NextToken.
AWS: Smithy: ListImports.NextToken -> ImportNextToken @length min 112 max 1024, @pattern ^([0-9a-f]{16})+$. A token that satisfies the model but does not decode is refused 400 ValidationException `Invalid Request: Provided nextToken is invalid: <token>` - fixed text `Invalid Request: Provided nextToken is invalid: ` followed by the token taken verbatim from the request.
Emulator: the constant `Invalid NextToken` - the token is dropped and the AWS wording is replaced.
Why: a paginating client cannot tell from the refusal which token was rejected, and the sentence it matches on is absent.
Fix: the ListImports pagination-token validation in its handler - emit AWS's sentence and interpolate the supplied token.
Traces: dynamodb-list-imports-next-token-length-bounds#task#3 (cluster also covers task#4, task#5 and dynamodb-list-imports-next-token-pattern#task#4).

Cause 6 of 6: UpdateContributorInsights with an out-of-bounds IndexName.
AWS: Smithy: UpdateContributorInsights.IndexName -> IndexName @length min 3 max 255, @pattern ^[a-zA-Z0-9_.-]+$. An IndexName outside those bounds is refused 400 ValidationException `IndexName must be at least 3 characters long and at most 255 characters long` - fixed text, limits from the model, value not echoed.
Emulator: `1 validation error detected: Value '<the 256-character name>' at 'indexName' failed to satisfy constraint: Member must have length less than or equal to 255`.
Why: as Cause 1; DynamoDB's single sentence covers both bounds, so a client matching it never matches the emulator's.
Fix: the UpdateContributorInsights input validation; same component as Cause 1.
Traces: dynamodb-contributor-insights-index-name-length-limit#task#5 (cluster also covers task#6, task#7).

Related open issues: 20261127T132819Z-dynamodb-validationexception-texts-are-generated-from-the-smithy.md (behavioral) describes this substitution for DeleteBackup/DescribeImport/DescribeContributorInsights, and 20261109T055139Z-dynamodb-validation-refusal-messages-are-emulator-authored-for.md and 20261111T170613Z-dynamodb-non-constraint-validationexception-messages-are-wrapped-in.md touch the same builder. The replay report binds none of these six clusters to any of them (owned_by is empty for all six), so none of them gates these causes; they are filed here with their own replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All six clusters are verdict=mismatch, status 400 -> 400, stub_path empty, owned_by empty; the diverging key is $.message only ($.__type matches, i.e. both sides are com.amazon.coral.validate#ValidationException).

Cause 1 - dynamodb-describe-backup-arn-length-bounds#task#6 (affected_count 2):
  expected 'Invalid Backup ARN'
  got "1 validation error detected: Value 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' at 'backupArn' failed to satisfy constraint: Member must have length greater than or equal to 37"

Cause 2 - dynamodb-describe-export-malformed-arn-rejected#task#1 (affected_count 1):
  expected 'Invalid Export ARN'
  got "1 validation error detected: Value 'arn:aws:dynamodb:...:table/<table>/export/...' at 'exportArn' failed to satisfy constraint: Member must have length less than or equal to 1024"

Cause 3 - dynamodb-export-table-to-point-in-time-invalid-table-arn-and-missing-client-token#task#3 (affected_count 2):
  expected 'Invalid TableArn: Invalid Table ARN'
  got "1 validation error detected: Value '' at 'tableArn' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 4 - dynamodb-restore-from-backup-backup-arn-length-bounds#task#6 (affected_count 4):
  expected 'Invalid Backup ARN'
  got "1 validation error detected: Value 'qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq' at 'backupArn' failed to satisfy constraint: Member must have length greater than or equal to 37"

Cause 5 - dynamodb-list-imports-next-token-length-bounds#task#3 (affected_count 4):
  expected 'Invalid Request: Provided nextToken is invalid: <the 112-hex-character token from the request>'
  got 'Invalid NextToken'

Cause 6 - dynamodb-contributor-insights-index-name-length-limit#task#5 (affected_count 3):
  expected 'IndexName must be at least 3 characters long and at most 255 characters long'
  got "1 validation error detected: Value '<256 g characters>' at 'indexName' failed to satisfy constraint: Member must have length less than or equal to 255"

Model citations (dynamodb-20120810): BackupArn @length 37..1024; ExportArn @length 37..1024; TableArn @length 1..1024; ImportNextToken @length 112..1024 @pattern ^([0-9a-f]{16})+$; IndexName @length 3..255 @pattern ^[a-zA-Z0-9_.-]+$.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: DescribeBackup with a 36-character non-ARN BackupArn (fixture dynamodb-describe-backup-arn-length-bounds, steps task#6 and task#9); assert $.message == 'Invalid Backup ARN'.
3. Cause 2: DescribeExport with an over-1024-character malformed ExportArn (fixture dynamodb-describe-export-malformed-arn-rejected, step task#1); assert 'Invalid Export ARN'.
4. Cause 3: ExportTableToPointInTime with an empty/invalid TableArn (fixture dynamodb-export-table-to-point-in-time-invalid-table-arn-and-missing-client-token, steps task#3, task#4); assert 'Invalid TableArn: Invalid Table ARN'.
5. Cause 4: RestoreTableFromBackup with a 36-character non-ARN BackupArn (fixture dynamodb-restore-from-backup-backup-arn-length-bounds, steps task#6, task#9, task#11); assert 'Invalid Backup ARN'.
6. Cause 5: ListImports with a 112-hex-character token that does not decode (fixture dynamodb-list-imports-next-token-length-bounds, steps task#3-task#5, and dynamodb-list-imports-next-token-pattern step task#4); assert 'Invalid Request: Provided nextToken is invalid: <token>'.
7. Cause 6: UpdateContributorInsights with a 256-character IndexName (fixture dynamodb-contributor-insights-index-name-length-limit, steps task#5-task#7); assert 'IndexName must be at least 3 characters long and at most 255 characters long'.
8. All six causes must pass before closing.
