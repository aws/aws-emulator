Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-patient-insights-job-completes-with-summary
Issue-Step: teardown#17
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-patient-insights-job-completes-with-summary
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: s3|DeleteBucket|2xx->4xx|status
Issue-Commit: 4fd3a81d2ce3a4bb67313136a30f2ab9d4620c6d

# s3 DeleteBucket is refused 409 BucketNotEmpty where AWS answered 204 at teardown

> _Note: this issue was identified against the code base at commit `4fd3a81d2ce3a4bb67313136a30f2ab9d4620c6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBucket

## Description
Cause 1 of 1: a DELETE of a general-purpose bucket whose recorded teardown has already removed every object it holds is answered 409 BucketNotEmpty instead of 204 No Content, so a bucket that was emptied by the same recorded sequence can no longer be deleted.

AWS: DeleteBucket is modeled with NO errors and an http binding of DELETE /{Bucket} with code 204 (Smithy: s3#DeleteBucket http {method: DELETE, uri: /{Bucket}, code: 204}; output Unit, errors []). Its documentation states the only precondition: "All objects (including all object versions and delete markers) in the bucket must be deleted before the bucket itself can be deleted." A bucket that holds nothing is therefore deleted and answered 204 with an empty body. S3's 409 for this operation is BucketNotEmpty, whose message is the fixed sentence "The bucket you tried to delete is not empty." (for a bucket that carries a versioning state, the fixed sentence "The bucket you tried to delete is not empty. You must delete all versions in the bucket."); the only variable part of that refusal is the bucket name carried in the error envelope's BucketName/Resource, never the sentence. In the recording the fixture's teardown deletes the objects it put there plus the two objects the patient-insights job wrote under output/<jobId>/<patientId>/ (summary.json and processing-metadata.json), and the subsequent DeleteBucket is answered 204.

Emulator: the same DeleteBucket, replayed after the identical teardown deletes (which all replayed as passing), answers 409 — the handler's BucketNotEmpty branch, which fires whenever the bucket record still holds any object version. The bucket is therefore left in place and undeletable by the caller that created it.

Hypothesis: the residue is an object this emulator itself wrote under a key the recorded teardown cannot name. In the same fixture, GetPatientInsightsJob reports insightsOutput.uri as "s3://<bucket>/output/<jobId>/" where AWS reports "s3://<bucket>/output/<jobId>/<patientId>/summary.json", and the job delivers its output object at exactly the key that uri names, so the emulator leaves an object at the prefix key "output/<jobId>/" that the teardown's recorded deletes (addressed at AWS's two real output keys) never remove. The divergent output uri itself is already owned by the open issue 20261109T171442Z-connecthealth-patient-insights-job-responses-diverge-epoch-number.md; that issue carries no replay key for this teardown step, so the DeleteBucket status divergence is filed here. An alternative reading of the same evidence is that the emulator's emptiness accounting counts something the recorded deletes did remove (a superseded version or delete marker retained for a bucket with no versioning state).

Why: a client relying on AWS's behavior tears a bucket down by deleting its contents and then the bucket, and treats the 204 as the end of the resource's life. Against this emulator the final delete fails, so every teardown of a bucket a service wrote into leaks a bucket and a cleanup script that checks the status aborts; a retry loop on BucketNotEmpty never clears, because the object blocking it is at a key the client never asked for and does not know to delete.

Fix: s3's DeleteBucket emptiness gate and the object-delivery key derivation that feeds it — i.e. the DeleteBucket handler's not-empty branch in the s3 crate, plus the service-written object's key derivation that leaves a prefix-only key behind. Either the service-written object must land at the key AWS writes (so the recorded deletes reach it), or an output location naming no object address must deliver no object at all rather than an object keyed on the bare prefix.

Traces: connecthealth-patient-insights-job-completes-with-summary#teardown#17 (DeleteBucket, expected 204, got 409). Related failing steps in the same fixture, both already owned: task#12 (GetPatientInsightsJob insightsOutput.uri) and task#13 (s3 GetObject 404 for output/<jobId>/<patientId>/summary.json). Code: services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket.rs:314-337 (the holds_no_versions gate and its 409 BUCKET_NOT_EMPTY refusal); services/service-com-amazonaws-connecthealth-connecthealth/src/ops/start_patient_insights_job.rs:396-407 (output_delivery derived from the job's reported output uri via s3_uri::object_address).

## Evidence
Replay of fixture connecthealth-patient-insights-job-completes-with-summary, step teardown#17, operation DeleteBucket, service s3:
  expected_status: 204 (recorded AWS response, empty body)
  actual_status:   409
  detail: "status: expected 204, got 409"
  verdict: mismatch, divergence_key service=s3|DeleteBucket|status=204->409|keys=status

AWS requirement (Smithy s3#DeleteBucket): errors [] and http code 204 — the delete of an empty bucket has no refusal to answer with. The only documented 409 is BucketNotEmpty, fixed text "The bucket you tried to delete is not empty." (versioned variant: "... You must delete all versions in the bucket."), variable part only the bucket name in the envelope.

Supporting evidence from the same fixture that the bucket is not empty in the emulator only:
  task#12 GetPatientInsightsJob $.insightsOutput.uri — expected 's3://<bucket>/output/<jobId>/<patientId>/summary.json', got 's3://<bucket>/output/<jobId>/' (prefix only)
  task#13 s3 GetObject of output/<jobId>/<patientId>/summary.json — expected 200, got 404
so the object the emulator delivered is keyed differently from the one the teardown deletes, and survives into the DeleteBucket call.

## Steps to Reproduce
1. Replay fixture connecthealth-patient-insights-job-completes-with-summary against the emulator (replay_wire_corpus).
2. The fixture creates a bucket, starts a connecthealth patient-insights job whose OutputDataConfig points at that bucket, waits for the job to complete, then in teardown deletes the objects the recording observed and finally issues DELETE /<bucket>.
3. Observe step teardown#17: DeleteBucket answers 409 (BucketNotEmpty) where the recording has 204 No Content, and the bucket remains.
4. Equivalently by hand: create a bucket, start a patient-insights job with s3OutputPath pointing into it, let it complete, delete the keys the job's AWS-shaped output location names, then DELETE the bucket — the emulator refuses 409 because an object remains at the bare prefix key 'output/<jobId>/'.
