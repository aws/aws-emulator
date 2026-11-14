Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-events.put-action-interactions-tracker-not-found-rejected personalize-events.put-items-success personalize-events.put-users-success personalize-events.put-users-max-batch-ten-users-success personalize-events.put-items-max-batch-ten-items-success personalize-events.put-items-property-update-reflected-in-export
Issue-Shape-Id: com.amazonaws.personalizeevents#AmazonPersonalizeEvents
Issue-Key: personalize-events.put-action-interactions-tracker-not-found-rejected|run#6|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code!r}"
Issue-Key: personalize-events|CreateDatasetExportJob|NotImplemented
Issue-Key: personalize-events.put-items-max-batch-ten-items-success|_create_export_job_with_role_retry#11|Timeout|time.sleep(interval_s)
Issue-Key: personalize-events.put-items-property-update-reflected-in-export|_create_export_job_when_data_visible#11|Timeout|time.sleep(interval_s)
Issue-Commit: 785304a74e11d2699ce58e3844995c4a689cee66

# personalize-events: PutActionInteractions misclassifies an unknown trackingId as 404, and the PUT-mode export read-back path is a 501 stub (2 causes)

> _Note: this issue was identified against the code base at commit `785304a74e11d2699ce58e3844995c4a689cee66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutActionInteractions, CreateDatasetExportJob

## Description
TWO root causes, both surfaced by the personalize-events behavioral suite (7/19 passed). Cause 1 is a refusal-classification defect on the ingestion data plane; Cause 2 is the unimplemented export read-back path that every "the data actually landed" check depends on. A fixer must close BOTH before submitting.

=== Cause 1 of 2: PutActionInteractions answers 404 ResourceNotFoundException where AWS answers 400 InvalidInputException for a never-created trackingId ===
Failing check: personalize-events.put-action-interactions-tracker-not-found-rejected

AWS: Smithy (com-amazonaws-personalizeevents-amazonpersonalizeevents.json): PutActionInteractions is POST /action-interactions, success 200, input trackingId (StringType, required, @length 1..256) and actionInteractions (required, @length 1..10); modeled errors are InvalidInputException (httpError 400, documented "Provide a valid value for the field or parameter"), ResourceInUseException (409) and ResourceNotFoundException (404). Recorded behavior: AWS refuses a syntactically valid trackingId that was never created as an INPUT fault — HTTP 400 in the InvalidInputException class — and not as a missing resource. The recorded wire fixture personalize-events-put-action-interactions-invalid-input (step task#4) shows AWS answering HTTP 400 with the message "ActionInteractions is not supported.": that text is entirely fixed, with no variable part interpolated from the request or from state (no tracking id, name or account). The sibling ingestion operation PutEvents models InvalidInputException ONLY, which is why neither operation's documented Errors list presents an unresolvable tracker as a not-found condition. Hypothesis: AWS decides whether the named tracker can accept action interactions at all before it reports a missing resource, and reports that as an input fault.
Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and the invented message "No event tracker with the tracking ID <trackingId> exists." — the fixed text is "No event tracker with the tracking ID " + " exists." and <trackingId> is the value the caller submitted (the suite submits a fresh uuid4; that value is a per-run input, not part of any template). The replayed check asserted the error code and got 'ResourceNotFoundException' where AWS gives 'InvalidInputException'.
Why: the fault class flips from 4xx-input to 4xx-not-found. A client that branches on ResourceNotFoundException (create-then-retry loops, resource-provisioning waits) takes a provisioning path for a request AWS calls simply invalid and will spin or retry forever, while an `except personalize_events.exceptions.InvalidInputException` handler — the one the API models — never fires. The surfaced message is also wholly different text from AWS's, so any caller matching or displaying it misreports the fault.
Fix: the PutActionInteractions handler's target-resolution and error selection in services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/ops/put_action_interactions.rs, where the event_trackers lookup miss is mapped straight to ResourceNotFoundException; it must select the InvalidInputException class with AWS's fixed wording instead.
Already-open but NOT covering: issue 20261029T211346Z-personalize-events-refuses-an-unresolvable-trackingid-with-invented.md ("personalize-events refuses an unresolvable trackingId with invented wording and the wrong 404 status") describes the same refusal from the WIRE corpus and carries only wire replay keys; the behavioral check above is not bound to it, so the behavioral key is recorded here. Also related but not covering: 20261022T000000Z-personalize-control-plane-does-not-register-event-trackers-or-datasets-for-ingestion.md — even with a populated registry the refusal path still answers the wrong status and error class.
Traces: personalize-events.put-action-interactions-tracker-not-found-rejected (the check's positive control, an identical call against the real ACTIVE tracker expecting 200, is never reached because the assertion on the error code fails first).

=== Cause 2 of 2: the personalize dataset-export path is unimplemented, so no personalize-events write can be read back ===
Failing checks: personalize-events.put-items-success, personalize-events.put-users-success, personalize-events.put-users-max-batch-ten-users-success, personalize-events.put-items-max-batch-ten-items-success, personalize-events.put-items-property-update-reflected-in-export

AWS: Smithy (com-amazonaws-personalize-amazonpersonalize.json): CreateDatasetExportJob takes required jobName (Name, @length 1..63, pattern ^[a-zA-Z0-9][a-zA-Z0-9\-_]*$), datasetArn (Arn, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$), roleArn (RoleArn, pattern ^arn:([a-z\d-]+):iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) and jobOutput (DatasetExportJobOutput), plus optional ingestionMode (enum ALL|BULK|PUT, default PUT) and tags (@length 0..200); it returns datasetExportJobArn and is @idempotent. Its modeled errors are InvalidInputException (400), LimitExceededException (409), ResourceAlreadyExistsException (403), ResourceInUseException (409), ResourceNotFoundException (404) and TooManyTagsException (400) — NotImplemented is not among them. The model documents the lifecycle CREATE PENDING > CREATE IN_PROGRESS > ACTIVE, or CREATE FAILED with a failureReason key, polled through DescribeDatasetExportJob (errors InvalidInputException 400, ResourceNotFoundException 404). Recorded behavior: with ingestionMode=PUT, AWS may transiently refuse the create with InvalidInputException whose message carries the fixed text "No data found in datasets for ingestionMode: PUT" (… wait up to 15 minutes before trying to Export) or "Insufficient privileges for writing data in S3" until the IAM role propagates and the incrementally-PUT data becomes export-visible; the variable parts are the dataset/role ARNs from the request. On success it returns a datasetExportJobArn and the job eventually reports ACTIVE, with the exported rows written under the requested s3DataDestination path.
Emulator: HTTP 501 with error code NotImplemented and the message "operation CreateDatasetExportJob is scaffolded but not implemented in this emulator" — the handler's only body is Scaffolded::not_implemented("CreateDatasetExportJob"). DescribeDatasetExportJob is a 501 scaffold stub too, so even a minted job ARN could never be polled to ACTIVE. Three checks (put-items-success, put-users-success, put-users-max-batch-ten-users-success) abort with botocore ClientError (NotImplemented); the other two (put-items-max-batch-ten-items-success, put-items-property-update-reflected-in-export) catch the refusal inside the documented bounded retry loop for AWS's transient InvalidInputException and burn their whole budget, reported as "Check timed out after 60.0s" — same single cause, different symptom.
Why: 501/NotImplemented is not a modeled error for this operation, so an SDK caller raises an unmodeled ClientError that no `except personalize.exceptions.InvalidInputException` / ResourceNotFoundException handler can catch, and code that legitimately retries AWS's transient export refusal loops to its own deadline against a permanent 501. More broadly, there is no wire path to read back anything PutItems/PutUsers/PutEvents ingested: a client can only ever observe the 200 on the mutating call, never that the data landed with the right itemIds and property values.
Fix: implement the export-job path in the personalize crate — services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_export_job.rs (mint a datasetExportJobArn, record the job in this service's state, and emit the exported rows for the requested ingestionMode to the jobOutput s3DataDestination) and its polling counterpart services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/describe_dataset_export_job.rs (report the documented status progression and failureReason) — plus the dataset-export state they share. Refusals must come from the operation's modeled error set, never 501.
Traces: personalize-events.put-items-success, personalize-events.put-users-success and personalize-events.put-users-max-batch-ten-users-success (direct NotImplemented ClientError on CreateDatasetExportJob); personalize-events.put-items-max-batch-ten-items-success and personalize-events.put-items-property-update-reflected-in-export (same refusal consumed by the bounded create-retry loop, surfacing as a 60s check timeout).

Note on what this issue does NOT bundle: the suite's other six failures are marked ALREADY FILED against the unimplemented personalize ListEventTrackers (services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/list_event_trackers.rs) and are deliberately excluded here.

## Evidence
Suite: simparity-behavioral-aws-v1 — Total 7/19 passed (36.8%); personalize-events 7/19 (37%), 12 failed. Six of those twelve are marked ALREADY FILED (ListEventTrackers) and are excluded.

Cause 1 — [mismatch] personalize-events.put-action-interactions-tracker-not-found-rejected:
  check_personalize_events_put_action_interactions_tracker_not_found_rejected.py line 46:
  assert code == "InvalidInputException", f"expected InvalidInputException, got {code!r}"
  AssertionError: expected InvalidInputException, got 'ResourceNotFoundException'
  Expected (AWS): HTTP 400, error code InvalidInputException, message "ActionInteractions is not supported." (wholly fixed text).
  Actual (emulator): HTTP 404, x-amzn-errortype ResourceNotFoundException, message "No event tracker with the tracking ID <submitted-uuid> exists."
  Emulator source: services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/ops/put_action_interactions.rs:192-201 maps the event_trackers miss directly to ResourceNotFoundException with that invented sentence.
  Model cited: smithy_query(operation="PutActionInteractions") -> errors = [InvalidInputException httpError 400, ResourceInUseException 409, ResourceNotFoundException 404]; http = POST /action-interactions, code 200.

Cause 2 — [error] personalize-events.put-items-success, [error] personalize-events.put-users-success, [error] personalize-events.put-users-max-batch-ten-users-success:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDatasetExportJob operation: operation CreateDatasetExportJob is scaffolded but not implemented in this emulator
  Expected (AWS): HTTP 200 with datasetExportJobArn, or a MODELED refusal (e.g. 400 InvalidInputException "No data found in datasets for ingestionMode: PUT …").
  Actual (emulator): HTTP 501, code NotImplemented.

Cause 2 (same root cause, timeout symptom) — [mismatch] personalize-events.put-items-max-batch-ten-items-success and [mismatch] personalize-events.put-items-property-update-reflected-in-export:
  Error: Check timed out after 60.0s
  Both call _create_export_job_with_role_retry(...), whose loop is `except ClientError: time.sleep(interval_s)` bounded by timeout_s=1200 — the permanent 501 is retried until the 60s check budget expires. PutItems itself returned 200; the checks then fail to read the data back.
  Emulator source: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_export_job.rs:187 — handler body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDatasetExportJob"))`, with the "SCAFFOLDED, NOT IMPLEMENTED" marker at line 4. services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/describe_dataset_export_job.rs:4 carries the same marker.
  Model cited: smithy_query(operation="CreateDatasetExportJob,DescribeDatasetExportJob", model_path="services/service-com-amazonaws-personalize-amazonpersonalize/smithy-model/com-amazonaws-personalize-amazonpersonalize.json").

## Steps to Reproduce
1. Clone this checkout, boot the emulator and replay suite simparity-behavioral-aws-v1.

Cause 1:
2. Create a dataset group and an ACTIVE Action Interactions event tracker through the personalize control plane.
3. Call personalize-events PutActionInteractions with trackingId set to a syntactically valid but never-created id (e.g. a fresh uuid4) and a one-element actionInteractions list ({actionId, sessionId, timestamp, eventType}).
4. Observe HTTP 404 / ResourceNotFoundException / "No event tracker with the tracking ID <id> exists.". AWS answers HTTP 400 in the InvalidInputException class with the fixed message "ActionInteractions is not supported.".
5. Re-replay personalize-events.put-action-interactions-tracker-not-found-rejected to confirm, including its positive control (the same call against the real ACTIVE tracker must return 200).

Cause 2:
6. Create a dataset group plus an ACTIVE Items (or Users) dataset, then call personalize-events PutItems (or PutUsers) with a batch of items/users — it returns 200.
7. Create an S3 destination bucket and an IAM role trusting personalize.amazonaws.com, then call personalize CreateDatasetExportJob with jobName, that datasetArn, roleArn, jobOutput.s3DataDestination.path and ingestionMode="PUT".
8. Observe HTTP 501 with code NotImplemented and the message "operation CreateDatasetExportJob is scaffolded but not implemented in this emulator" instead of a datasetExportJobArn (or a modeled 4xx). Callers that retry AWS's documented transient InvalidInputException around this call hang until their own deadline.
9. Call personalize DescribeDatasetExportJob with any datasetExportJobArn — also 501 NotImplemented, so the documented CREATE PENDING > CREATE IN_PROGRESS > ACTIVE progression can never be polled.
10. Re-replay personalize-events.put-items-success, personalize-events.put-users-success, personalize-events.put-users-max-batch-ten-users-success, personalize-events.put-items-max-batch-ten-items-success and personalize-events.put-items-property-update-reflected-in-export to confirm.
