Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-export-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-export-lifecycle
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeExport|2xx->2xx|$.ExportDescription.ItemCount
Issue-Related: 20261109T055139Z-dynamodb-export-surface-8-net-new-causes-export.md
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# DescribeExport on a completed export reports ItemCount 0 and omits DestinationType and S3SseAlgorithm

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExport

## Description
DescribeExport on a COMPLETED table export returns an ExportDescription that misreports how much data the export carried and drops two of the fields that describe where and how it landed. Two net-new root causes in one response; both must be closed before submitting.

Cause 1 of 2: ExportDescription.ItemCount is reported as 0 for a completed export of a table that holds items.
AWS: Smithy: ExportDescription.ItemCount targets ItemCount (long, @range min 0), doc "The number of items exported" (model services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json, shape com.amazonaws.dynamodb#ExportDescription). AWS reports the count of items actually written by the export job, so for a table whose point-in-time snapshot contains N items a COMPLETED export reports ItemCount = N (the recording exercises a single-item table and AWS reports 1). Status is 200 and no error is involved.
Emulator: the same 200 response carries ExportDescription.ItemCount = 0 — a completed export always reports zero items regardless of how many items the source table held at ExportTime.
Why: ItemCount is the only in-API signal a client has that an export actually moved data. A client that polls DescribeExport until ExportStatus COMPLETED and then asserts ItemCount > 0 (or uses it to size the follow-up S3 read, to reconcile against the table's item count, or to decide whether to re-run the export) concludes every export produced an empty snapshot and either fails the reconciliation or retries the export forever.
Fix: the export job's state — the item-count tally must be accumulated while the export materializes the snapshot and persisted on the export record — and the DescribeExport serializer that projects that record into ExportDescription. Hypothesis: the counter is initialized on the export record but never incremented as items are written.
Traces: dynamodb-export-lifecycle#task#9.

Cause 2 of 2: the same DescribeExport response omits DestinationType and S3SseAlgorithm.
AWS: S3SseAlgorithm is modeled on ExportDescription (enum S3SseAlgorithm, doc "Type of encryption used on the bucket where export data is stored. Valid values ... AES256 - server-side encryption with Amazon S3 managed keys; KMS"), and AWS resolves and reports the effective S3-managed default AES256 for an export that named neither an S3SseAlgorithm nor a KMS key. DestinationType is recorded behavior only — it is not a member of ExportDescription in the on-disk model — but the recording shows AWS returning 'S3' for an export whose destination is an S3 bucket.
Emulator: both members are absent from the ExportDescription (null/not serialized).
Why: a client polling DescribeExport to learn where and how an export landed cannot tell the destination kind or the encryption in effect, so it cannot assert the export's compliance settings nor copy them onto a follow-up export.
Fix: the persisted export record (resolve and store the effective destination kind and SSE algorithm at export creation) plus the DescribeExport ExportDescription serializer that must emit them.
Traces: dynamodb-export-lifecycle#task#9.

Related open issue, which does NOT carry this replay key: 20261109T055139Z-dynamodb-export-surface-8-net-new-causes-export.md describes the DestinationType/S3SseAlgorithm omission for other DescribeExport steps (its Issue-Key set covers $.ExportDescription.DestinationType and $.ExportDescription.S3SseAlgorithm but not $.ExportDescription.ItemCount), and 20261110T232401Z-dynamodb-response-state-and-shape-diverge-on-replica.md and 20261030T060227Z-dynamodb-pitr-and-export-state-no-export-summaries.md touch neighbouring export state. None of them carries dynamodb-export-lifecycle#task#9, so this replay step is only re-run if this issue is closed.

## Evidence
Replay of trace set simparity-wire-aws-v1 (415 fixtures, 4399 steps, 3906 passed, score 88.8%). 94 divergence clusters, all on service dynamodb; 93 are owned by existing open issues, this is the one net-new cluster.

Cluster divergence_key service=dynamodb|DescribeExport|status=200->200|keys=$.ExportDescription.DestinationType,$.ExportDescription.ItemCount,$.ExportDescription.S3SseAlgorithm — verdict mismatch, expected_status 200, actual_status 200, 1 affected step (dynamodb-export-lifecycle#task#9).

Reported detail, verbatim:
  $.ExportDescription.DestinationType: missing (expected 'S3', got None)
  $.ExportDescription.ItemCount: value_mismatch (expected 1, got 0)
  $.ExportDescription.S3SseAlgorithm: missing (expected 'AES256', got None)

The expected ItemCount value 1 is this recording's table size, not a requirement: the requirement is that ItemCount equals the number of items the export wrote. Status (200) and every other ExportDescription member in the step agree.

Model evidence: smithy_query query=shape shape=ExportDescription on services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json — ItemCount {target: ItemCount, type: long, range: {min: 0}, doc: "The number of items exported."}; S3SseAlgorithm {type: enum, doc lists AES256 and KMS}; no DestinationType member (recorded behavior only).

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1 and 2: fixture dynamodb-export-lifecycle, step task#9 — create a table, put at least one item, enable point-in-time recovery, run ExportTableToPointInTime to an S3 bucket naming no S3SseAlgorithm and no KMS key, poll DescribeExport until ExportStatus is COMPLETED, then inspect the ExportDescription.
4. Cause 1: ExportDescription.ItemCount comes back 0; AWS reports the number of items the export wrote (1 for this single-item table).
5. Cause 2: ExportDescription.DestinationType ('S3') and ExportDescription.S3SseAlgorithm ('AES256', the resolved S3-managed default) are both absent from the response.
