Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-data-exports.create-export-writes-access-test-object-at-bucket-root
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports.create-export-writes-access-test-object-at-bucket-root|_until_keys#14|Timeout|time.sleep(2)
Issue-Commit: d1c103039c63f2296b4493ce7c42775d3faa3fcc

# bcm-data-exports CreateExport/UpdateExport never write the 4-byte aws-programmatic-access-test-object write-access probe at the destination bucket root

> _Note: this issue was identified against the code base at commit `d1c103039c63f2296b4493ce7c42775d3faa3fcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExport

## Description
Cause 1 of 1: a CreateExport that is accepted proves its write access to the destination bucket by PUTting a probe object, and the emulator writes no such object.
AWS: recorded behavior only (the Smithy model says nothing about the probe; it models only where the export FILE goes — "the destination Amazon Simple Storage Service (Amazon S3) bucket name and object keys of a data exports file", shape `S3Destination`). The recording shows that when CreateExport accepts a definition, the service writes an object at the bucket ROOT — key `aws-programmatic-access-test-object`, outside the export's `S3Prefix` — whose body is the 4 bytes `test`, and that this is the only object in the bucket at that point. Deleting that object and then calling UpdateExport against the same bucket makes the service write it again, with the same key and the same 4-byte body. Both the key and the body are fixed text; nothing of the request appears in them.
Emulator: the probe object never appears. A bucket created for the export, with a policy granting bcm-data-exports.amazonaws.com s3:PutObject, still lists no `aws-programmatic-access-test-object` after CreateExport returns 200 — the bucket listing never becomes exactly `['aws-programmatic-access-test-object']` within the check's 60 s budget, so the check exhausted its time waiting for a write that is never performed. The crate's only object-store write is the scheduled export-file delivery under the destination prefix (`crate::state::delivery`, `materialize_due_deliveries`); neither `validate_delivery_grant` nor the create/update handlers put anything at the bucket root.
Why: a client or operator that verifies an export's delivery permission by looking for the probe object at the bucket root (the documented way to confirm Data Exports can write to the bucket) sees an empty bucket and concludes delivery is misconfigured; tooling that waits for the probe before proceeding hangs, and a bucket-notification or lifecycle rule keyed on that root object never fires.
Fix: the write-access proof belongs with the destination grant evaluation reached by both definition-accepting handlers — `crate::state::destination::validate_delivery_grant` (called from `src/ops/create_export.rs` and `src/ops/update_export.rs`): on an accepted definition it must PUT the fixed-key, fixed-body probe object at the bucket root through the mounted S3 object store, in addition to scheduling the export-file delivery under the prefix.
Traces: bcm-data-exports.create-export-writes-access-test-object-at-bucket-root.

## Evidence
Check bcm-data-exports.create-export-writes-access-test-object-at-bucket-root: "Error: Check timed out after 60.0s". The check creates a bucket, asserts it is empty, grants bcm-data-exports.amazonaws.com s3:PutObject on arn:aws:s3:::<bucket>/*, calls CreateExport (which returns 200), then polls ListObjectsV2 until the key set equals ['aws-programmatic-access-test-object'] with a 60 s budget — expected body b'test' — and then repeats the wait after UpdateExport. The poll never satisfies, i.e. the probe key is never present. No code path in the service crate writes the key `aws-programmatic-access-test-object`: the only object-store writes are the export-file deliveries in src/state/delivery.rs, which write under the definition's S3Prefix.

## Steps to Reproduce
1. CreateBucket <bucket>; PutBucketPolicy allowing Principal {"Service": "bcm-data-exports.amazonaws.com"} s3:PutObject on arn:aws:s3:::<bucket>/* with aws:SourceAccount = the account and aws:SourceArn like arn:aws:bcm-data-exports:us-east-1:<account>:export/*.
2. Confirm ListObjectsV2(Bucket=<bucket>) is empty.
3. CreateExport with an S3Destination naming <bucket> and S3Prefix 'cur/daily' (TEXT_OR_CSV / GZIP / CREATE_NEW_REPORT, RefreshCadence SYNCHRONOUS) -> returns 200.
4. ListObjectsV2(Bucket=<bucket>): AWS shows exactly the key 'aws-programmatic-access-test-object' (GetObject body = b'test'); the emulator never shows it.
5. DeleteObject that key, then UpdateExport the same ExportArn with the definition GetExport reports and a changed S3Prefix -> AWS writes the probe object again; the emulator does not.
