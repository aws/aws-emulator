Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-snapshots
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-snapshots ec2-snapshot-filter-and-pagination-sweep ec2-snapshot-inventory-and-attribute-inspection
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.nextToken
Issue-Key: ec2|DescribeSnapshots|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeSnapshots|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: 57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf

# EC2 DescribeSnapshots diverges on inventory shape, pagination token and malformed-id refusal

> _Note: this issue was identified against the code base at commit `57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
Three net-new wire divergences, all on DescribeSnapshots: the snapshot inventory it serves does not have the recorded shape/content, a truncated page omits nextToken, and a malformed snapshot id is refused with the wrong error class and message. All three must be closed before submitting.

Neighbouring open issues do NOT cover these: 20261030T201123Z-ec2-describesnapshots-cannot-see-a-snapshot-created-through.md is about EBS-direct-API snapshots not being visible to DescribeSnapshots, and 20261029T200817Z-ec2-id-validation-verdict-is-inverted-malformed-ids.md is about malformed-vs-unknown id verdicts on other operations; neither carries the fixture steps named here.

Cause 1 of 3: the DescribeSnapshots inventory projection does not match the recorded snapshot set.
AWS: Smithy models DescribeSnapshots{MaxResults, NextToken, OwnerIds(Owner), RestorableByUserIds, SnapshotIds(SnapshotId), DryRun, Filters} -> {Snapshots(snapshotSet, SnapshotList), NextToken(nextToken)}; no modeled error shapes (ec2Query wire codes). A describe of the account-visible snapshot inventory answers HTTP 200 with snapshotSet carrying one <item> per snapshot, each item carrying snapshotId, volumeId, status, startTime, progress, ownerId, volumeSize, description, ownerAlias, encrypted, storageTier, transferType, completionTime and fullSnapshotSizeInBytes.
Emulator: the top-level snapshotSet does not even have the recorded type - replay: "$.DescribeSnapshotsResponse.snapshotSet: type_mismatch", with the expected value being the item LIST of the recorded inventory (the recorded ids, descriptions, timestamps and sizes are that run's values, not the requirement; the requirement is that snapshotSet is a list of snapshot items projecting the members above).
Why: a client iterating snapshotSet.item cannot read the inventory at all - a wrongly typed snapshotSet breaks SDK deserialization of the whole result, and anything filtering snapshots by owner-alias/description/storage-tier sees nothing comparable.
Fix: the operation's serializer and the snapshot state/catalog it projects (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs) - emit snapshotSet as a list of fully projected snapshot items.
Traces: ec2-catalog-describe-snapshots#task#1 (same cluster also fails at ec2-catalog-describe-snapshots#task#2 and ec2-snapshot-filter-and-pagination-sweep#task#29).

Cause 2 of 3: a truncated DescribeSnapshots page omits nextToken, so the next page is unreachable.
AWS: Smithy marks DescribeSnapshots @paginated with inputToken NextToken, outputToken NextToken, items Snapshots, pageSize MaxResults, and models output member NextToken (xmlName nextToken). When MaxResults truncates the matching set, the response carries a non-empty nextToken; its value is opaque and per-run (the fixture asserts only that it is present and shaped).
Emulator: the member is absent - replay: "$.DescribeSnapshotsResponse.nextToken: missing (expected '<shaped-not-asserted>', got None)".
Why: a paginating client sees the first page and concludes the listing is complete, silently losing every remaining snapshot; SDK paginators terminate after one page.
Fix: the operation's pagination in its handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs) - emit nextToken whenever the result set was truncated by MaxResults.
Traces: ec2-snapshot-filter-and-pagination-sweep#task#31.

Cause 3 of 3: a malformed snapshot id is refused with the wrong error code and a reworded message.
AWS: recorded behavior only for the text (ec2Query, DescribeSnapshots models no error shapes). A syntactically invalid value in SnapshotIds is refused with HTTP 400, error code InvalidParameterValue and the message "Value (<value>) for parameter snapshotId is invalid. Expected: 'snap-...'." - the fixed text is "Value (", ") for parameter snapshotId is invalid. Expected: 'snap-...'.", and the only variable part is the offending value copied from the request (recorded run used snap-bad-id).
Emulator: HTTP 400 but error code InvalidSnapshotID.Malformed with the message "Value (snap-bad-id) for snapshot ID is invalid." - wrong code, and reworded text that drops the parameter name snapshotId and the "Expected: 'snap-...'" hint - replay: "$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'InvalidSnapshotID.Malformed')", "$.Response.Errors.Error.Message: value_mismatch".
Why: AWS distinguishes a parameter-syntax rejection (InvalidParameterValue) from an id-shape rejection; clients and error-mapping layers that branch on InvalidParameterValue - or that parse the parameter name out of the message - do not recognise this refusal and retry or surface it as the wrong failure class.
Fix: the operation's input validation (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_snapshots.rs) - classify a malformed SnapshotIds entry as InvalidParameterValue and emit AWS's message template.
Traces: ec2-snapshot-inventory-and-attribute-inspection#task#10.

## Evidence
Replay output (trace set simparity-wire-aws-v1), verbatim:

- ec2-catalog-describe-snapshots#task#1 - DescribeSnapshots, expected 200 / got 200: "$.DescribeSnapshotsResponse.snapshotSet: type_mismatch (expected {'item': [{'snapshotId': ..., 'volumeId': 'vol-ffffffff', 'status': 'completed', 'startTime': ..., 'progress': '100%', 'ownerId': ..., 'volumeSize': '1', 'description': ..., 'ownerAlias': 'amazon', 'encrypted': 'false', 'storageTier': 'standard', 'transferType': 'standard', 'completionTime': ..., 'fullSnapshotSizeInBytes': ...}, ...]}, got <other type>)". Cluster affects 3 steps: ec2-catalog-describe-snapshots#task#1, ec2-catalog-describe-snapshots#task#2, ec2-snapshot-filter-and-pagination-sweep#task#29.
- ec2-snapshot-filter-and-pagination-sweep#task#31 - DescribeSnapshots, expected 200 / got 200: "$.DescribeSnapshotsResponse.nextToken: missing (expected '<shaped-not-asserted>', got None)".
- ec2-snapshot-inventory-and-attribute-inspection#task#10 - DescribeSnapshots, expected 400 / got 400: "$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'InvalidSnapshotID.Malformed')"; "$.Response.Errors.Error.Message: value_mismatch (expected \"Value (snap-bad-id) for parameter snapshotId is invalid. Expected: 'snap-...'.\", got 'Value (snap-bad-id) for snapshot ID is invalid.')".

Message anatomy for Cause 3: fixed text = "Value (" + ") for parameter snapshotId is invalid. Expected: 'snap-...'."; variable part = the offending value from the request (snap-bad-id in this run). Snapshot ids, descriptions, timestamps, sizes and the nextToken value are per-run values and are not themselves requirements.

Model facts used (smithy_query, services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): DescribeSnapshots input {MaxResults, NextToken, OwnerIds->Owner, RestorableByUserIds, SnapshotIds->SnapshotId, DryRun, Filters}; output {NextToken->nextToken, Snapshots->snapshotSet (SnapshotList)}; errors []; paginated {inputToken: NextToken, outputToken: NextToken, items: Snapshots, pageSize: MaxResults}.

## Steps to Reproduce
1. Replay fixtures ec2-catalog-describe-snapshots, ec2-snapshot-filter-and-pagination-sweep and ec2-snapshot-inventory-and-attribute-inspection from trace set simparity-wire-aws-v1; steps ec2-catalog-describe-snapshots#task#1, ec2-snapshot-filter-and-pagination-sweep#task#31 and ec2-snapshot-inventory-and-attribute-inspection#task#10 diverge.
2. Cause 1: ec2.describe_snapshots(OwnerIds=['amazon']) (or the recorded catalog filter) -> snapshotSet must be a list of snapshot items each projecting snapshotId, volumeId, status, startTime, progress, ownerId, volumeSize, description, ownerAlias, encrypted, storageTier, transferType, completionTime, fullSnapshotSizeInBytes; observe a wrongly typed snapshotSet.
3. Cause 2: ec2.describe_snapshots(MaxResults=<n smaller than the matching set>) -> response must carry nextToken; observe it absent, so a paginator stops after page one.
4. Cause 3: ec2.describe_snapshots(SnapshotIds=['snap-bad-id']) -> expect HTTP 400 InvalidParameterValue with "Value (snap-bad-id) for parameter snapshotId is invalid. Expected: 'snap-...'."; observe InvalidSnapshotID.Malformed with "Value (snap-bad-id) for snapshot ID is invalid.".
