Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements
Issue-Key: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements|_await_records#32|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: ea226a61fd4fc055bb456ff32cf72ea13892c65d

# CloudTrail does not audit REFUSED EC2 calls: a rejected DescribeInstances/RunInstances is never recorded, so the Client.-prefixed errorCode, errorMessage and the absent responseElements of a failed call are unobservable

> _Note: this issue was identified against the code base at commit `ea226a61fd4fc055bb456ff32cf72ea13892c65d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1 — a REFUSED EC2 management call is never written into the account's management-event log, so CloudTrail event history has no record whose errorCode/errorMessage/absent responseElements a caller can read back, and a poll for one's own rejected call never converges.

AWS: every management call is recorded in the Region's event history whether it succeeded or was refused, and is readable through LookupEvents (Smithy, cloudtrail model: LookupEvents, input LookupAttributes/StartTime/EndTime/MaxResults/NextToken, output Events[].CloudTrailEvent, paginated on NextToken). For a refused EC2 call the recorded behavior the check measured against real AWS is: one record per call, eventSource `ec2.amazonaws.com`, eventName the operation name (`DescribeInstances`, `RunInstances`), `errorCode` equal to the fault class plus the EC2 code — fixed `Client.` prefix + the code the SDK saw, i.e. `Client.InvalidInstanceID.NotFound` where the SDK saw `InvalidInstanceID.NotFound`, and `Client.DryRunOperation` where the SDK saw `DryRunOperation` — `errorMessage` equal, character for character, to the message the SDK received (for the NotFound case the EC2 template `The instance ID '<instance-id>' does not exist` with the id from the request; for the dry run the fixed sentence `Request would have succeeded, but DryRun flag is set.`), NO `responseElements` at all (there was no response to record), `requestID` equal to the ResponseMetadata.RequestId of that very call, and `readOnly` true for a Describe. A SUCCESSFUL call made by the same principal at the same instant carries no `errorCode` at all. These per-record field semantics are recorded behavior only — the model states none of them.

Emulator: no such record is ever produced. The check makes a refused DescribeInstances (unknown id), a refused RunInstances(DryRun=True) and one successful DescribeInstances, then polls LookupEvents for all three request ids under a 900 s budget (20 s interval, paging to exhaustion) and is still polling when the run's 60 s cap ends it: verdict `mismatch`, detail `Check timed out after 60.0s`. This is the refusal half of the same missing seam already filed for the SUCCESS half: 20261028T021552Z-cloudtrail-ec2-cloudtrail-records-no-ec2-ec2-amazonaws-com-management.md shows LookupEvents pages out of the shared service_common::management_events log (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs) while EC2's handlers never write into it. That issue's replay set names only cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id and covers only successful RunInstances/TerminateInstances records; it says nothing about recording a call the service REFUSED, which is a distinct recording path (the error fields are populated and the response fields are omitted) and is not covered by it. Hypothesis: the 60 s cap is consumed by that LookupEvents poll — the EC2 phase before it is a handful of synchronous calls whose SDK-side codes and messages the check asserts first.

Why: a client cannot audit failures. The security-relevant half of an audit trail — who was denied, what was refused, with which error code and message, correlated by request id to the caller's own SDK response — returns an empty Events list, which is indistinguishable from 'the call was never made'. Incident reconstruction, denied-access alerting and compliance evidence built on refused API calls are dead, and a poll for one's own rejection record never terminates instead of failing fast.

Fix: drive the shared management-event recording seam (service_common::management_events / ServedApiCall, the one services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/state.rs::record_management_event already consumes) from EC2's served operations on the REFUSAL path as well as the success path, populating eventSource/eventName/readOnly/awsRegion/requestID/eventID plus errorCode as `Client.` + the EC2 code returned and errorMessage as the exact message returned, and omitting responseElements entirely for a refused call, so CloudTrail's LookupEvents can serve them.

Traces: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements.

## Evidence
Failing check cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements — verdict 'mismatch', detail 'Check timed out after 60.0s' (no audit assertion reached).

Expected (real AWS, as the check asserts after its _await_records poll, budget _INDEX_BUDGET_S = 900.0, _POLL_INTERVAL_S = 20.0):
  - the rejected DescribeInstances record: eventSource == 'ec2.amazonaws.com', eventName == 'DescribeInstances', errorCode == 'Client.InvalidInstanceID.NotFound' (the SDK saw 'InvalidInstanceID.NotFound'), errorMessage == "The instance ID '<id>' does not exist", responseElements is None, requestID == that call's ResponseMetadata.RequestId, readOnly is True;
  - the rejected RunInstances(DryRun=True) record: eventName == 'RunInstances', errorCode == 'Client.DryRunOperation' (SDK saw 'DryRunOperation'), errorMessage == 'Request would have succeeded, but DryRun flag is set.', no responseElements;
  - the successful DescribeInstances record: no errorCode key at all.
  The check's own failure text for the missing-record case: "DATAPLANE_UNIMPLEMENTED: CloudTrail event history produced no record for [...] after <N>s (nf=..., dry=..., ok=...). Real AWS audits a rejected EC2 call on the same timeline as a successful one."

Actual (emulator): no record for any of the three request ids ever appears, so the poll never completes.

Message-template note: the fixed parts are the `Client.` prefix on the audited errorCode, the sentence "Request would have succeeded, but DryRun flag is set." and the frame "The instance ID '…' does not exist"; the only variable part is the instance id taken from the request. Request ids and timestamps in the recording are per-run values and are not part of the requirement — the requirement is that the record's requestID equals the request id the caller's own response carried.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 CreateVpc (10.43.0.0/16) + CreateSubnet (10.43.1.0/24) in an available AZ; resolve an al2023 x86_64 AMI with DescribeImages.
3. ec2 DescribeInstances(InstanceIds=['i-' + 8 hex digits nobody owns]) — keep the error code, message and ResponseMetadata.RequestId (expect InvalidInstanceID.NotFound / "The instance ID '<id>' does not exist").
4. ec2 RunInstances(DryRun=True, ImageId=<ami>, InstanceType=t3.micro, MinCount=1, MaxCount=1, SubnetId=<subnet>) — keep the error code, message and RequestId (expect DryRunOperation / 'Request would have succeeded, but DryRun flag is set.').
5. ec2 DescribeInstances(Filters=[tag:<run marker>]) as a positive control — keep its RequestId.
6. cloudtrail LookupEvents over a window covering steps 3-5 (LookupAttributes EventName='DescribeInstances', then 'RunInstances'), paging NextToken to exhaustion, selecting records by requestID.
7. Expect one record per call with the errorCode/errorMessage/responseElements/readOnly content listed in Evidence, and no errorCode on the step-5 record.
8. Observe Events: [] for every lookup, no matter how long the poll runs.
