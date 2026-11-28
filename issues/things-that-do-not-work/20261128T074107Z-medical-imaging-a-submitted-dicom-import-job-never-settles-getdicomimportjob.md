Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medical-imaging.update-image-set-metadata-revert-with-dicom-updates-rejected
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging.update-image-set-metadata-revert-with-dicom-updates-rejected|_until#25|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s:.0f}s; last read {value!r}")
Issue-Commit: 6ee4b13902a5451963e09087bb437008ad9d1a26

# A submitted DICOM import job never settles: GetDICOMImportJob keeps reporting IN_PROGRESS past the completion window, so no imported image set ever becomes readable

> _Note: this issue was identified against the code base at commit `6ee4b13902a5451963e09087bb437008ad9d1a26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDICOMImportJob

## Description
Cause 1 of 1: a DICOM import job submitted against an ACTIVE data store, over an input prefix that holds one readable DICOM P10 object, never reaches a terminal jobStatus — repeated GetDICOMImportJob reads keep answering IN_PROGRESS, so every assertion that depends on the imported image set existing (the whole versioning/revert flow) is unreachable.

Failing check: medical-imaging.update-image-set-metadata-revert-with-dicom-updates-rejected.

AWS: StartDICOMImportJob answers 200 with jobStatus from JobStatus (COMPLETED | FAILED | IN_PROGRESS | SUBMITTED) and DICOMImportJobProperties$jobStatus is @required; the two terminal members of that enum are COMPLETED and FAILED, and DICOMImportJobProperties$endedAt/$message are only populated once the job has ended (message is documented as "The error message thrown if an import job fails"). AWS HealthImaging drives a submitted job SUBMITTED -> IN_PROGRESS -> COMPLETED (or FAILED) on its own; an import of a prefix that holds a valid P10 instance COMPLETES and the image set it generated becomes ACTIVE/IMPORTED, searchable by SearchImageSets and readable by GetImageSetMetadata. Smithy: GetDICOMImportJob GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId}, output jobProperties (DICOMImportJobProperties, required); StartDICOMImportJob POST /startDICOMImportJob/datastore/{datastoreId}, output jobStatus (JobStatus, required). The duration of a real import is not modeled — recorded behavior only for the timing; what IS a requirement is that the job reaches a terminal status without further client action.

Emulator: the job is submitted (200, jobId minted, jobStatus SUBMITTED) and then stalls in IN_PROGRESS. The replayed check polls GetDICOMImportJob with a 2 s interval; two consecutive reads, at least 2 s apart and both already past the point where the job had left SUBMITTED for IN_PROGRESS, still answered jobStatus IN_PROGRESS with no endedAt and no message — i.e. the job was still non-terminal well beyond the emulator's own completion window (crate::state::IMPORT_JOB_COMPLETION_DELAY = 4 s, IMPORT_JOB_IN_PROGRESS_DELAY = 2 s). It neither COMPLETED nor FAILED, so no refusal explains it either. Consequently the imported image set stays LOCKED/IMPORTING with its attributes and image frames undelivered, and SearchImageSets/GetImageSetMetadata can never see the study.

Hypothesis (not stated by the model or by the recording): in this emulator a job that owes output manifests only becomes terminal when that S3 delivery is MADE — ImportJob::advance_to refuses to settle while output_owed.is_some(), and import_job_output::claim_one flips the delivery to DeliveryState::InFlight under the state lock before the cross-service PutObject hops out. A claimant that never reports back (the timer task armed by StartDICOMImportJob, or a put that neither returns nor errors) leaves the delivery claimed, delivery_is_due() then answers false for DELIVERY_RECLAIM_AFTER (30 s) and every GetDICOMImportJob read skips it and reports the stale non-terminal status. Either way, the observable defect is that the terminal status is gated on a side-effect that is not being completed or not being re-driven promptly.

Why: a client that submits an import and polls GetDICOMImportJob — the documented way to learn an import finished — waits forever and never gets COMPLETED or FAILED, so it can never proceed to read the image sets the import generated. Every downstream flow (search by StudyInstanceUID, GetImageSetMetadata, UpdateImageSetMetadata, revert) is blocked, and a client cannot even distinguish "still running" from "wedged": the shape is valid, only the state machine never finishes. Nothing the client can send recovers it.

Fix: the DICOM import job state machine and its output-delivery gate — the GetDICOMImportJob handler's pre-read delivery pass and the job-completion settle path in the service state. A job whose completion has fallen due must reach COMPLETED (or FAILED with a message) on the next read, and a delivery claim that has not reported back must not keep the job non-terminal.

Traces: medical-imaging.update-image-set-metadata-revert-with-dicom-updates-rejected (replay it to confirm: the check asserts the import reaches COMPLETED and then exercises the revert/DICOMUpdates refusal).

## Evidence
Expected (AWS): after StartDICOMImportJob over an input prefix holding one readable DICOM P10 object, GetDICOMImportJob eventually answers 200 with jobProperties.jobStatus == "COMPLETED", endedAt set, no message; the generated image set then reads ACTIVE/IMPORTED and is returned by SearchImageSets for its DICOMStudyInstanceUID.

Actual (emulator): GetDICOMImportJob repeatedly answers 200 with jobProperties.jobStatus == "IN_PROGRESS", no endedAt and no message, on consecutive polls spaced 2 s apart and taken after the job had already left SUBMITTED. The check aborts with:

  AssertionError: the DICOM import did not happen within 1s; last read {'jobId': '<32-hex>', 'jobName': '<job name from the request>', 'jobStatus': 'IN_PROGRESS', 'datastoreId': '<32-hex>', 'dataAccessRoleArn': '<the role ARN from the request>', 'submittedAt': <submit time>, 'inputS3Uri': 's3://<bucket>/dicom/', 'outputS3Uri': 's3://<bucket>/results/<datastoreId>-DicomImport-<jobId>/'}

(The ids, names, bucket and timestamps above are per-run values; the requirement is only that jobStatus becomes COMPLETED/FAILED. The polling budget printed as "1s" is the runner's remaining-time cap, not an AWS-side requirement — the job was already past the emulator's own 4 s completion window on the last read.)

No terminal status and no error code were ever produced, so there is no diverging error code or message to quote: the divergence is the absence of the terminal transition the required jobStatus enum promises.

## Steps to Reproduce
1. CreateDatastore and poll GetDatastore until datastoreStatus == ACTIVE.
2. Create an S3 bucket, put one valid explicit-VR little-endian DICOM P10 file under the key prefix dicom/, and create an IAM role trusted by medical-imaging.amazonaws.com with read on the input prefix and write on results/.
3. StartDICOMImportJob with datastoreId, that dataAccessRoleArn, a fresh clientToken, inputS3Uri=s3://<bucket>/dicom/ and outputS3Uri=s3://<bucket>/results/. It answers 200 with jobStatus SUBMITTED.
4. Poll GetDICOMImportJob(datastoreId, jobId) every 2 s. Observe it move SUBMITTED -> IN_PROGRESS and then stay IN_PROGRESS indefinitely: jobStatus never becomes COMPLETED or FAILED, endedAt is never set.
5. SearchImageSets with filter EQUAL on DICOMStudyInstanceUID of the imported study returns no imageSetsMetadataSummaries, because the generated image set is still LOCKED/IMPORTING with nothing delivered.

Replay: medical-imaging.update-image-set-metadata-revert-with-dicom-updates-rejected
