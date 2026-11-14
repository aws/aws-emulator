Issue-Source: trace-sweep-divergence
Issue-Trace: iot-job-continuous-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-job-continuous-lifecycle iot-thing-and-group-lifecycle iot-fleet-indexing-error-paths iot-fleet-metric-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.description
Issue-Key: iot|CancelJob|2xx->2xx|$.description
Issue-Key: iot|ListThingsInThingGroup|2xx->2xx|$.things
Issue-Key: iot|ListThingsInThingGroup|2xx->2xx|$.things[*]
Issue-Key: iot|DescribeFleetMetric|4xx->4xx|$.message
Issue-Related: 20261107T022027Z-iot-jobs-and-commands-plane-8-causes-continuous.md
Issue-Commit: a6ac5f2812657951a989dc8ceb784d022fa28a4a

# iot job updates and thing-group membership are not reflected by subsequent reads, and the Job projection drops modeled members (4 causes)

> _Note: this issue was identified against the code base at commit `a6ac5f2812657951a989dc8ceb784d022fa28a4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJob

## Description
Four net-new wire divergences where a write is accepted but the following read does not reflect it, plus one projection gap and one message gap. Four causes must be closed.

Cause 1 of 4: UpdateJob's description is not persisted, and the DescribeJob Job projection omits isConcurrent, the jobProcessDetails counters and the configuration envelopes.
  AWS: Smithy: UpdateJob -> PATCH /jobs/{jobId} with description (JobDescription), presignedUrlConfig, jobExecutionsRolloutConfig, abortConfig, timeoutConfig; DescribeJob -> GET /jobs/{jobId} with output member job (Job). Recorded response after an UpdateJob that set a new description: job.description is the UPDATED text, and the Job carries job.isConcurrent, job.jobProcessDetails with numberOfCanceledThings / numberOfFailedThings / numberOfInProgressThings / numberOfQueuedThings / numberOfRejectedThings / numberOfRemovedThings / numberOfSucceededThings / numberOfTimedOutThings all present (0 for a job with no executions yet), and the structures job.jobExecutionsRolloutConfig, job.presignedUrlConfig, job.schedulingConfig and job.timeoutConfig present (their own members null when unset).
  Emulator: job.description is still the text the job was CREATED with, and isConcurrent, every jobProcessDetails counter and all four configuration structures are absent.
  Why: a client that edits a job and verifies, or that reads job progress counters to drive a rollout dashboard or a completion decision, sees the pre-update description and null where AWS always reports a number or a structure.
  Fix: the UpdateJob handler's write into the job state, and the DescribeJob serializer / Job projection (emit the counters zero-valued and the configuration envelopes with null members when unset).
  Traces: iot-job-continuous-lifecycle#task#9, iot-job-continuous-lifecycle#task#13.

Cause 2 of 4: CancelJob's response echoes the pre-update job description.
  AWS: Smithy: CancelJob -> PUT /jobs/{jobId}/cancel with output member description. Recorded response: the description is the job's CURRENT text — the one UpdateJob last set.
  Emulator: the description the job was created with.
  Why: the cancel response is the caller's confirmation of which job it stopped; it reports a stale description, so a client that logs or matches on it mis-identifies the job.
  Fix: the CancelJob response serializer must read the live job record (which cause 1's write must also fix).
  Traces: iot-job-continuous-lifecycle#task#12.

Cause 3 of 4: a thing removed from a thing group is still listed as a member.
  AWS: Smithy: ListThingsInThingGroup -> GET /thing-groups/{thingGroupName}/things, output things (ThingNameList), paginated. Recorded response after RemoveThingFromThingGroup: things is the empty list.
  Emulator: things still contains the removed thing name, and the fixture's poll re-reads it 30 times without the membership ever clearing.
  Why: group membership drives dynamic policies, job targets and fleet queries; a client that removes a thing and confirms is told it is still in the group, so a thing stays targeted by jobs and policies the operator believes it has left. This is eventual by nature in AWS, but the recording shows it settles and here it never does.
  Fix: the thing-group membership state — RemoveThingFromThingGroup's deletion must be visible to the ListThingsInThingGroup index.
  Traces: iot-thing-and-group-lifecycle#task#16.

Cause 4 of 4: DescribeFleetMetric's not-found Message carries only the tail of the recorded text.
  AWS: Smithy: DescribeFleetMetric -> GET /fleet-metric/{metricName}, errors include ResourceNotFoundException (httpError 404). Recorded response: 404 ResourceNotFoundException whose message is 'An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric ' + <account>:<metricName> + ' not found.'
  Emulator: 404 ResourceNotFoundException (status and code correct) with 'FleetMetric <account>:<metricName> not found.' — exactly the tail of the recorded text, missing the leading 'An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: ' segment.
  Why: a client comparing the message against the recorded text does not match. Hypothesis: that leading segment is how botocore renders str() of a ClientError, so the recording may encode the rendered exception rather than the wire Message member — in which case the emulator's tail is already AWS's wording and nothing diverges here. A fixer must confirm against the recording BEFORE prepending the prefix: emitting it inside the Message member would itself be a divergence from AWS, which never repeats the operation name inside an error message. This cause is filed to carry the wire replay key and to get that determination made, not to assert the prefix as a requirement.
  Fix: the DescribeFleetMetric handler's not-found message — only if the recording shows the prefix belongs to the Message member. Open issue 20261114T082236Z-iot-refusal-message-fidelity-7-causes-addthingtothinggroup-createstream.md raises the same question from the behavioral corpus (its cause 7) and carries no wire key for it.
  Traces: iot-fleet-indexing-error-paths#task#1, iot-fleet-indexing-error-paths#task#4, iot-fleet-metric-lifecycle#task#4, iot-fleet-metric-lifecycle#task#7.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. All four clusters are verdict 'mismatch' with empty owned_by and empty stub_path.

Cause 1 — iot-job-continuous-lifecycle#task#9 (DescribeJob), status 200:
  $.job.description: expected the updated text ('updated probe job'), got the created text ('probe job')
  $.job.isConcurrent: missing (expected True)
  $.job.jobProcessDetails.numberOf{Canceled,Failed,InProgress,Queued,Rejected,Removed,Succeeded,TimedOut}Things: all missing (expected 0)
  $.job.jobExecutionsRolloutConfig: missing (expected {'exponentialRate': None, 'maximumPerMinute': None})
  $.job.presignedUrlConfig: missing (expected {'expiresInSec': None, 'roleArn': None})
  $.job.schedulingConfig: missing (expected {'endBehavior': None, 'endTime': None, 'maintenanceWindows': None, 'startTime': None})
  $.job.timeoutConfig: missing (expected {'inProgressTimeoutInMinutes': None})
  also affected: iot-job-continuous-lifecycle#task#13

Cause 2 — iot-job-continuous-lifecycle#task#12 (CancelJob), status 200:
  $.description: expected the updated text, got the created text

Cause 3 — iot-thing-and-group-lifecycle#task#16 (ListThingsInThingGroup), status 200:
  $.things[0]: extra (expected None, got the removed thing name)
  $.things: poll: 1 item(s), not 0 after 30 attempts (expected [])

Cause 4 — iot-fleet-indexing-error-paths#task#1 (DescribeFleetMetric), status 404 as expected:
  $.message: expected 'An error occurred (ResourceNotFoundException) when calling the DescribeFleetMetric operation: FleetMetric <account>:<metricName> not found.', got 'FleetMetric <account>:<metricName> not found.'
  also affected: iot-fleet-indexing-error-paths#task#4, iot-fleet-metric-lifecycle#task#4, iot-fleet-metric-lifecycle#task#7

The job descriptions, thing names, metric names and account ids above are per-run probe values; the requirements are that the update is visible to the following read, the presence of the modeled members (counters zero-valued, envelopes with null members), the membership clearing, and the message's fixed text with its interpolated values.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — UpdateJob (PATCH /jobs/{jobId}; description, presignedUrlConfig, jobExecutionsRolloutConfig, abortConfig, timeoutConfig), DescribeJob (GET /jobs/{jobId}; output job -> Job), ListThingsInThingGroup (GET /thing-groups/{thingGroupName}/things; things -> ThingNameList, paginated), DescribeFleetMetric (GET /fleet-metric/{metricName}; ResourceNotFoundException 404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. Cause 1: CreateJob(jobId=J, targets=[<a thing ARN>], document=..., targetSelection='CONTINUOUS', description='probe job'), UpdateJob(J, description='updated probe job'), then DescribeJob(J) — expect job.description updated and job.isConcurrent, job.jobProcessDetails counters and the four configuration envelopes present.
3. Cause 2: CancelJob(J) and read the response's description — expect the updated text.
4. Cause 3: CreateThingGroup(G), CreateThing(T), AddThingToThingGroup(G, T), RemoveThingFromThingGroup(G, T), then poll ListThingsInThingGroup(G) — expect things == [].
5. Cause 4: DescribeFleetMetric(metricName=<a metric never created>) and compare Error.Message with the recorded text, first determining from the recording whether the 'An error occurred (...) when calling the ... operation: ' segment belongs to the Message member or is botocore's rendering.
6. Re-replay the representative step iot-job-continuous-lifecycle#task#9 and every fixture#step in trace_ids.
