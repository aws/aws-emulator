Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# DICOM import jobs ignore S3 entirely: no input read, no output manifest, and a job can never FAIL

_Filed: 2026-10-24T00:23:53Z_
_Operation: StartDICOMImportJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartDICOMImportJob` records the job and the clock marches it SUBMITTED → IN_PROGRESS → COMPLETED after fixed spans, without ever touching the emulated S3 service. Consequences a client can observe:

1. `inputS3Uri` is stored but never read. A job naming a bucket/prefix that does not exist, or one holding no DICOM P10 objects, still reports `jobStatus: COMPLETED`. In AWS such a job ends `FAILED` with a `message` explaining the failure (`DICOMImportJobProperties.message` is modeled precisely as "The error message thrown if an import job fails").
2. `outputS3Uri` is stored but nothing is ever written under it. In AWS a finished import job writes its job-output manifests (`job-output-manifest.json`, `SUCCESS`/`FAILURE` summaries) beneath `outputS3Uri/<datastoreId>-DicomImport-<jobId>/`, which is how callers verify an import.
3. `jobStatus` is therefore effectively a two-outcome clock: `FAILED` (a value the `JobStatus` enum declares) is unreachable, and `message` is always absent.

Expected: the job resolves its input prefix against the emulated S3 state, fails with `FAILED` + `message` when the input is absent/empty, and writes the job-output manifest objects under `outputS3Uri` on success.
Actual: every job reports COMPLETED with no S3 interaction in either direction.

Cross-service reads against the emulated S3 crate are an established pattern in this tree, so the wiring seam exists.

## Evidence

Spec: `smithy_query(query="operation", operation="StartDICOMImportJob")` — required `inputS3Uri` and `outputS3Uri` (S3 prefixes), `JobStatus` enum includes `FAILED`; `DICOMImportJobProperties.message` documented as the error message thrown if an import job fails. Well-known HealthImaging behavior: an import reads DICOM P10 objects from the input prefix and writes job-output manifests under the output prefix.
Handler: `src/ops/start_dicom_import_job.rs` lines 225-241 — `input_s3_uri` / `output_s3_uri` are copied into the `ImportJob` record and nothing else; `src/state.rs` `ImportJob::advance_to` (lines 389-401) only moves Submitted → InProgress → Completed on the clock and never sets `JobStatus::Failed` or `message`.
Precedent for the wiring: `service_com_amazonaws_s3_amazons3::state::service_state(engine)` is used cross-crate by e.g. amplifybackend `list_s3_buckets.rs` and lightsail `state.rs`.
Tests: `tests/all/datastore_lifecycle.rs::an_import_job_reads_back_whole_and_runs_to_completion` asserts the monotonic climb to COMPLETED only; no test asserts a FAILED job or any S3 side effect.

## Steps to Reproduce

1. Mount the medical-imaging and S3 emulators on one router
2. CreateDatastore and wait for ACTIVE
3. POST /startDICOMImportJob/datastore/{datastoreId} with inputS3Uri=s3://bucket-that-does-not-exist/in/ and outputS3Uri=s3://bucket-that-does-not-exist/out/
4. Poll GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId} and observe jobStatus reaches COMPLETED with no message, instead of FAILED with a message
5. List the output bucket/prefix through the S3 emulator and observe no job-output manifest objects were written
