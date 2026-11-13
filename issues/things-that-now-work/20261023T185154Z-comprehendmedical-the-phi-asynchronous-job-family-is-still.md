Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehendmedical.async-phi-job-completes-and-writes-output-to-s3 comprehendmedical.start-job-idempotency-token-reuse
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical.async-phi-job-completes-and-writes-output-to-s3|_start_job_with_role_retry#5|Timeout|time.sleep(interval_s)
Issue-Key: comprehendmedical.start-job-idempotency-token-reuse|_start_job_with_role_retry#6|Timeout|time.sleep(interval_s)
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# ComprehendMedical: the PHI asynchronous job family is still a 501 stub, so no PHI job can be started, tracked or completed

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPHIDetectionJob

## Description
Cause 1 of 1: StartPHIDetectionJob (and its PHI-family siblings) answer HTTP 501 NotImplemented, so every PHI batch workflow stalls at the very first call.

AWS: Smithy: StartPHIDetectionJob input DataAccessRoleArn (IamRoleArn, required), InputDataConfig (required), OutputDataConfig (required), LanguageCode (required, enum: en), optional JobName (@length 1..256) and ClientRequestToken (@idempotencyToken, @pattern ^[a-zA-Z0-9-]+$); output JobId (@length 1..32); modeled errors InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429 — 501/NotImplemented is not among them. A valid request returns HTTP 200 with a JobId; DescribePHIDetectionJob then reports ComprehendMedicalAsyncJobProperties whose JobStatus advances SUBMITTED -> IN_PROGRESS -> COMPLETED, and the job writes a .out file under the OutputDataConfig prefix containing the PHI Entities the synchronous detector would find for the same text. Recorded behavior for the token: reusing a ClientRequestToken with identical parameters returns the SAME JobId, while reusing it with different parameters is NOT rejected — a second, independent job is created with its own JobId and the first job's configuration is untouched. Recorded behavior also has each job's representation agree field-for-field between ListPHIDetectionJobs and DescribePHIDetectionJob.
Emulator: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_phi_detection_job.rs still carries the "//! SCAFFOLDED, NOT IMPLEMENTED" marker and its handle() returns Err(crate::scaffolded::Scaffolded::not_implemented("StartPHIDetectionJob")) — HTTP 501 NotImplemented. Both replays surface as "Check timed out after 60.0s" rather than as a NotImplemented error only because the checks wrap the Start call in a retry-until-deadline helper (a freshly created IAM role can legitimately need a few seconds), so the permanent 501 is retried until the harness kills the check. Nothing downstream is exercised: no JobId exists, so describe/list/stop of a PHI job, the SUBMITTED->COMPLETED transition, the S3 .out output and the idempotency-token semantics are all unreachable.
Why: a client doing PHI batch analysis — the documented way to process more text than the synchronous limit allows — cannot start a job at all, and gets an unmodeled 501 that SDK retry/error handling does not classify as either a client or a server fault. Workflows that poll to completion hang instead of failing fast.
Fix: implement the PHI asynchronous job family over the existing persisted job state (crate::state::AsyncJob) the way the entities-detection family already is, and delete the SCAFFOLDED marker lines: src/ops/start_phi_detection_job.rs, plus the sibling describe/list/stop PHI handlers in the same src/ops directory. Start must honour ClientRequestToken as an idempotency key (identical parameters -> same JobId; different parameters -> a new, independent job), the stored job must advance to a terminal status on the clock, the Describe and List projections must be built from the one stored record so they cannot drift, and a completed job must write its .out output under the OutputDataConfig prefix.
Traces: comprehendmedical.async-phi-job-completes-and-writes-output-to-s3, comprehendmedical.start-job-idempotency-token-reuse. The second phase of comprehendmedical.start-job-invalid-role-arn-pattern-rejected (a well-formed but absent role ARN must yield InvalidRequestException, not 501) is also blocked here; that check's own first-phase message defect is filed separately.

Not a duplicate: the open issue "ComprehendMedical's text-inference operations are still stubs" (20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md) covers only the synchronous inference operations and explicitly excludes this work in its own "Not in scope" section — "The remaining asynchronous job families (ICD10CM / RxNorm / SNOMEDCT / PHI start, describe, list and stop) are a separate gap: they can reuse crate::state::AsyncJob and the entities-detection handlers as written, and need no inference component." This issue is that separate gap, and unlike the inference operations it needs no ML model: PHI job orchestration is pure state management.

## Evidence
Suite: comprehendmedical 0/21 (0%). Two checks [mismatch]:

comprehendmedical.async-phi-job-completes-and-writes-output-to-s3:
  Error: Check timed out after 60.0s
  The check creates a bucket (input/doc.txt), an IAM role with s3 get/list/put, then calls start_phi_detection_job through _start_job_with_role_retry (retries ANY exception every 8s for up to 180s). It then expects describe_phi_detection_job to reach JobStatus COMPLETED and a .out object under output/ containing Entities with a NAME entity "Jane Doe".

comprehendmedical.start-job-idempotency-token-reuse:
  Error: Check timed out after 60.0s
  Same setup, then: List/Describe field-for-field agreement on JobId/JobName/DataAccessRoleArn/LanguageCode/InputDataConfig/OutputDataConfig/JobStatus; repeat Start with the same ClientRequestToken and identical params -> same JobId; repeat with a different JobName -> a DISTINCT JobId, with the first job's JobName unchanged.

Both stall on the first start_phi_detection_job call. Contrast: comprehendmedical.async-job-stop-transition-and-duplicate-stop-rejected used the SAME bucket/role plumbing against start_entities_detection_v2_job and did start a job and drive it to STOPPED — so the S3/IAM prerequisites and the harness are fine, and the failure is specific to the PHI family.

Source proof of the 501:
  services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_phi_detection_job.rs:4 — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented"
  same file:181 — Err(crate::scaffolded::Scaffolded::not_implemented("StartPHIDetectionJob"))
The sibling PHI check start-job-invalid-role-arn-pattern-rejected confirms requests do reach this service and are refused by the pre-handler validation layer, i.e. only the handler body is missing.

Model grounding (smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): StartPHIDetectionJob -> output JobId; errors InternalServerException/500, InvalidRequestException/400, ResourceNotFoundException/404, TooManyRequestsException/429 (no 501). DescribePHIDetectionJob -> ComprehendMedicalAsyncJobProperties. ListPHIDetectionJobs -> ComprehendMedicalAsyncJobPropertiesList + NextToken.

## Steps to Reproduce
1. Point an AWS CLI/botocore comprehendmedical client at the emulator.
2. Create an S3 bucket, put an object at key input/doc.txt, and create an IAM role trusting comprehendmedical.amazonaws.com with s3:GetObject/ListBucket/PutObject on that bucket.
3. aws comprehendmedical start-phi-detection-job --input-data-config S3Bucket=<bucket>,S3Key=input/ --output-data-config S3Bucket=<bucket>,S3Key=output/ --data-access-role-arn <roleArn> --language-code en
   Expected: HTTP 200 with {"JobId": "..."}.
   Actual: HTTP 501 NotImplemented — "operation StartPHIDetectionJob is scaffolded but not implemented in this emulator". A client that retries transient start failures (as a freshly created IAM role warrants) retries this permanent 501 forever; the suite checks time out at 60s.
4. Because no JobId is ever returned, the rest of the contract is unreachable: describe-phi-detection-job polling to COMPLETED, the .out file with Entities under output/, ListPHIDetectionJobs/DescribePHIDetectionJob agreement, stop-phi-detection-job, and ClientRequestToken reuse returning the same JobId for identical parameters (and a new JobId for different parameters).
