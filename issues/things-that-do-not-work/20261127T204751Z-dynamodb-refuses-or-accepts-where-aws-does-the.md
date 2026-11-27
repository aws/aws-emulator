Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-execute-statement-order-by-gsi-sort-key-and-non-index-attribute-rejected
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-execute-statement-order-by-gsi-sort-key-and-non-index-attribute-rejected dynamodb-get-item-attributes-to-get-name-length-limit dynamodb-unsigned-and-interior-tab-header-requests-to-a-table describe-backup-source-table-feature-details-carries-sse-description dynamodb-contributor-insights-index-name-length-limit
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.__type
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.message
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|status
Issue-Key: dynamodb|GetItem|4xx->2xx|$.__type
Issue-Key: dynamodb|GetItem|4xx->2xx|$.message
Issue-Key: dynamodb|GetItem|4xx->2xx|status
Issue-Key: dynamodb|DescribeTimeToLive|2xx->4xx|$.TimeToLiveDescription
Issue-Key: dynamodb|DescribeTimeToLive|2xx->4xx|status
Issue-Key: dynamodb|DescribeBackup|2xx->2xx|$.BackupDescription.SourceTableFeatureDetails.SSEDescription
Issue-Key: dynamodb|DescribeContributorInsights|2xx->2xx|$.ContributorInsightsRuleList[*]
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# DynamoDB refuses or accepts where AWS does the opposite, and two describe payloads omit or over-long a member

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Five divergences in which the emulator's answer to a request is wrong in kind, not in wording: two requests AWS refuses are accepted with 200, one request AWS serves is refused with 400, and two describe responses carry the wrong body - one omitting a member AWS returns, one returning a value that violates the member's own modeled pattern.

THIS BUNDLE CONTAINS 5 CAUSES.

Cause 1 of 5: a PartiQL SELECT whose ORDER BY names an attribute outside the primary key is accepted.
AWS: Smithy: ExecuteStatement.Statement -> PartiQLStatement @length 1..8192, required; the service-common ValidationException applies. AWS only orders a PartiQL result by the key of the table or index being read, and refuses any other ORDER BY attribute with HTTP 400 ValidationException `Attribute <name> in ORDER BY clause must be part of the primary key` - the fixed text is everything but `<name>`, which is the attribute name taken from the statement.
Emulator: HTTP 200 with no $.__type and no $.message - the statement is executed.
Why: a statement AWS rejects runs, so results come back in an order AWS would never produce and the developer only discovers the restriction in production.
Fix: the PartiQL statement validation in the ExecuteStatement handler - resolve the ORDER BY attribute against the target's key schema and refuse when it is not a key attribute.
Traces: dynamodb-execute-statement-order-by-gsi-sort-key-and-non-index-attribute-rejected#task#10.

Cause 2 of 5: GetItem with an empty attribute name in AttributesToGet is accepted.
AWS: Smithy: GetItem.AttributesToGet -> AttributeNameList @length min 1 (list), members are AttributeName. An empty string as an attribute name is refused 400 ValidationException `Empty attribute name` - fixed text, no variable parts.
Emulator: HTTP 200 with no $.__type and no $.message - the read is served.
Why: the emulator silently drops or ignores a nonsense projection member instead of refusing it, so a client that builds AttributesToGet from user input gets a successful read against the emulator and a 400 against AWS.
Fix: the GetItem input validation in the DynamoDB request-validation component - reject an empty attribute name in AttributesToGet (and the same check belongs on the other read paths that accept AttributesToGet).
Traces: dynamodb-get-item-attributes-to-get-name-length-limit#task#6.

Cause 3 of 5: DescribeTimeToLive is refused 400 for a request AWS serves.
AWS: Smithy: DescribeTimeToLive.TableName -> TableArn @length 1..1024, required; output TimeToLiveDescription; modeled errors InternalServerError, InvalidEndpointException, ResourceNotFoundException. Recorded behavior: the request as transmitted in this fixture - the header-handling probe that also sends unsigned and interior-tab-bearing headers - is ACCEPTED and answered 200 with `TimeToLiveDescription: {TimeToLiveStatus: DISABLED}` for a table that has TTL off.
Emulator: 400, with no TimeToLiveDescription in the body.
Why: the emulator's HTTP front door rejects a request AWS admits, so a client whose signer or proxy emits that header form cannot read TTL state at all - the operation is unusable rather than merely differently worded.
Fix: the DynamoDB HTTP edge's header parsing/auth admission (not the DescribeTimeToLive handler, which never runs) - the request must reach the handler and be answered with the TTL description.
Traces: dynamodb-unsigned-and-interior-tab-header-requests-to-a-table#task#8.

Cause 4 of 5: DescribeBackup omits SourceTableFeatureDetails.SSEDescription for an encrypted source table.
AWS: Smithy: DescribeBackup output BackupDescription -> SourceTableFeatureDetails, whose SSEDescription member is documented as "The description of the server-side encryption status on the table when the backup was created"; SSEDescription carries Status (SSEStatus), SSEType (SSEType, KMS) and KMSMasterKeyArn. For a backup of a KMS-encrypted table AWS returns that member populated with Status ENABLED, SSEType KMS and the key ARN.
Emulator: 200, but $.BackupDescription.SourceTableFeatureDetails.SSEDescription is absent entirely.
Why: a caller auditing whether a backup's source table was encrypted, or reading the key it was encrypted under before a restore, gets nothing - the encryption posture of every backup is invisible. (The KMSMasterKeyArn value itself is environment-specific and not asserted; the requirement is that the member is present with Status ENABLED and SSEType KMS.)
Fix: the DescribeBackup serializer/state - copy the source table's SSE description into SourceTableFeatureDetails when the backup is created and emit it.
Traces: describe-backup-source-table-feature-details-carries-sse-description#task#5.

Cause 5 of 5: DescribeContributorInsights returns rule names built from the untruncated index name, violating the rule-name pattern.
AWS: Smithy: DescribeContributorInsights output ContributorInsightsRuleList -> list of ContributorInsightsRule @pattern ^[A-Za-z0-9][A-Za-z0-9\\-\\_\\.]{0,126}[A-Za-z0-9]$ - i.e. a rule name is at most 128 characters. AWS composes each rule name as `DynamoDBContributorInsights-PKC-<table>-<indexNameSegment>-<epochMillis>` (and `-PKT-` for the second rule) and keeps it inside that bound; the recording shows the index-name segment shortened to its first 40 characters for a 255-character IndexName. Hypothesis: the index name is truncated to 40 characters specifically; what the model requires either way is a name of at most 128 characters matching the pattern.
Emulator: it interpolates the full 255-character IndexName, producing a ~333-character rule name that cannot match ContributorInsightsRule's pattern. (The trailing epoch-millis segment also differs between the recording and the replay; that part is a per-run value and is NOT part of the requirement.)
Why: the response violates its own modeled constraint, so a strict SDK or a client that round-trips the rule name to CloudWatch Contributor Insights fails on a value DynamoDB would never emit.
Fix: the DescribeContributorInsights serializer - truncate the index-name segment when composing the rule name so the result satisfies ContributorInsightsRule.
Traces: dynamodb-contributor-insights-index-name-length-limit#task#4.

Related open issues: 20261122T170041Z-dynamodb-9-net-new-causes-partiql-index-qualified.md (PartiQL ORDER BY family) and 20261124T091830Z-dynamodb-updatecontributorinsights-answers-the-transitional-disabling-for-a.md (contributor insights) are adjacent, but the replay report binds none of these five clusters to any open issue (owned_by is empty for all five), so they are not gated by those issues.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All five clusters are verdict=mismatch with empty stub_path and empty owned_by.

Cause 1 - dynamodb-execute-statement-order-by-gsi-sort-key-and-non-index-attribute-rejected#task#10 (affected_count 1):
  status: expected 400, got 200
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got None
  $.message: expected 'Attribute sk in ORDER BY clause must be part of the primary key', got None

Cause 2 - dynamodb-get-item-attributes-to-get-name-length-limit#task#6 (affected_count 1):
  status: expected 400, got 200
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got None
  $.message: expected 'Empty attribute name', got None

Cause 3 - dynamodb-unsigned-and-interior-tab-header-requests-to-a-table#task#8 (affected_count 1):
  status: expected 200, got 400
  $.TimeToLiveDescription: missing (expected {'TimeToLiveStatus': 'DISABLED'}, got None)

Cause 4 - describe-backup-source-table-feature-details-carries-sse-description#task#5 (affected_count 1), status 200 -> 200:
  $.BackupDescription.SourceTableFeatureDetails.SSEDescription: missing (expected {'KMSMasterKeyArn': '<shaped-not-asserted>', 'SSEType': 'KMS', 'Status': 'ENABLED'}, got None)

Cause 5 - dynamodb-contributor-insights-index-name-length-limit#task#4 (affected_count 1), status 200 -> 200:
  $.ContributorInsightsRuleList[0]: expected 'DynamoDBContributorInsights-PKC-<table>-<40 characters of the index name>-<epochMillis>' (118 characters), got the same prefix with all 255 index-name characters interpolated (~333 characters)
  $.ContributorInsightsRuleList[1]: the same divergence for the '-PKT-' rule
  (the epochMillis suffix differs per run and is not part of the requirement)

Model citations (dynamodb-20120810): ExecuteStatement.Statement -> PartiQLStatement @length 1..8192 required; GetItem.AttributesToGet -> AttributeNameList @length min 1; DescribeTimeToLive.TableName -> TableArn @length 1..1024 required, output TimeToLiveDescription, errors ResourceNotFoundException/InvalidEndpointException/InternalServerError; SourceTableFeatureDetails.SSEDescription -> SSEDescription {Status: SSEStatus, SSEType: SSEType, KMSMasterKeyArn}; ContributorInsightsRuleList member ContributorInsightsRule @pattern ^[A-Za-z0-9][A-Za-z0-9\\-\\_\\.]{0,126}[A-Za-z0-9]$.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: ExecuteStatement with a SELECT whose ORDER BY names a non-primary-key attribute (fixture dynamodb-execute-statement-order-by-gsi-sort-key-and-non-index-attribute-rejected, step task#10); assert 400 ValidationException 'Attribute sk in ORDER BY clause must be part of the primary key'.
3. Cause 2: GetItem with an empty string in AttributesToGet (fixture dynamodb-get-item-attributes-to-get-name-length-limit, step task#6); assert 400 ValidationException 'Empty attribute name'.
4. Cause 3: replay the header-handling fixture's DescribeTimeToLive step (fixture dynamodb-unsigned-and-interior-tab-header-requests-to-a-table, step task#8); assert 200 with TimeToLiveDescription.TimeToLiveStatus DISABLED.
5. Cause 4: create a KMS-encrypted table, back it up, DescribeBackup (fixture describe-backup-source-table-feature-details-carries-sse-description, step task#5); assert BackupDescription.SourceTableFeatureDetails.SSEDescription is present with Status ENABLED and SSEType KMS.
6. Cause 5: enable contributor insights on an index with a 255-character name and DescribeContributorInsights (fixture dynamodb-contributor-insights-index-name-length-limit, step task#4); assert every ContributorInsightsRuleList entry is at most 128 characters and matches ContributorInsightsRule's pattern.
7. All five causes must pass before closing.
