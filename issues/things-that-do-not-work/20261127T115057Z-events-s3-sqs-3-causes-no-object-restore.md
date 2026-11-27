Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.object-restore-completed-reaches-eventbridge-with-expiry-and-source-class events__s3__sqs.object-restore-initiated-names-the-source-storage-class events__s3__sqs.put-object-retention-reaches-eventbridge-as-object-retention-updated
Issue-Key: events__s3__sqs.object-restore-completed-reaches-eventbridge-with-expiry-and-source-class|run#20|AssertionError|assert completed, (
Issue-Key: events__s3__sqs.object-restore-initiated-names-the-source-storage-class|run#15|AssertionError|assert source == "GLACIER", f"source-storage-class was {source!r}"
Issue-Key: events__s3__sqs.put-object-retention-reaches-eventbridge-as-object-retention-updated|run#22|AssertionError|assert updated, f"no Object Retention Updated arrived within {budget_s:.0f}s; saw {types!r}"
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__s3__sqs: 3 causes — no Object Restore Completed event, no source storage class on Object Restore Initiated, and no Object Retention Updated event

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreObject

## Description
Cause 1 of 3: the restore of an archived object never publishes `Object Restore Completed`. AWS: RestoreObject on a bucket with EventBridge notifications publishes `Object Restore Initiated` on source `aws.s3` and then, when the temporary copy is available, `Object Restore Completed` whose detail carries the restore expiry time and the source storage class — HeadObject showing `ongoing-request="false"` with an expiry-date means the completion event is due. Recorded behavior only (documented S3 EventBridge event types). Emulator: only `['Object Restore Initiated', 'Object Created']` are seen; the completion event never arrives even though HeadObject already reports `ongoing-request="false", expiry-date=...`. Checks: events__s3__sqs.object-restore-completed-reaches-eventbridge-with-expiry-and-source-class.

Cause 2 of 3: `Object Restore Initiated` omits the source storage class. AWS: the detail names the storage class the object is being restored FROM (e.g. GLACIER/DEEP_ARCHIVE) in its source-storage-class field. Recorded behavior only. Emulator: `source-storage-class was None`. Checks: events__s3__sqs.object-restore-initiated-names-the-source-storage-class.

Cause 3 of 3: PutObjectRetention publishes no `Object Retention Updated`. AWS: on an Object-Lock-enabled bucket with EventBridge notifications, PutObjectRetention publishes `Object Retention Updated` on source `aws.s3` for the object version. Recorded behavior only. Emulator: `no Object Retention Updated arrived within 55s; saw ['Object Created']`.

Why: restore- and compliance-driven pipelines never run — a client waiting for a restore to complete before reading the object waits forever (or must poll HeadObject), cannot tell which archive tier an object came from, and governance tooling that reacts to retention changes sees nothing.
Fix: the S3 EventBridge notification emission for the restore lifecycle (initiated detail fields + the completion event when the restored copy becomes available) and for PutObjectRetention.
Traces: the three check ids listed per cause above.

## Evidence
Cause 1: `AssertionError: no Object Restore Completed arrived within 10s of the restore completing ...; HeadObject Restore is 'ongoing-request="false", expiry-date="..."'; detail-types seen ['Object Restore Initiated', 'Object Created']`
Cause 2: `AssertionError: source-storage-class was None`
Cause 3: `AssertionError: no Object Retention Updated arrived within 55s; saw ['Object Created']`

## Steps to Reproduce
1. s3 CreateBucket with EventBridge notifications enabled (and Object Lock for cause 3); events PutRule on {source:["aws.s3"]} + PutTargets to an SQS queue.
2. PutObject with StorageClass=GLACIER, then RestoreObject; poll HeadObject's Restore header and read the queue.
3. PutObjectRetention on a versioned, lock-enabled object and read the queue.
