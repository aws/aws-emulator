Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-application-update-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-application-update-lifecycle
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|DescribeApplications|2xx->2xx|$.Applications[*].Metadata
Issue-Key: appstream|DescribeApplications|2xx->2xx|$.Applications[*].DisplayName
Issue-Key: appstream|DescribeApplications|2xx->2xx|$.Applications[*].LaunchPath
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# appstream application update lifecycle diverges: Application.Metadata omitted and UpdateApplication edits never visible

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeApplications

## Description
Two net-new root causes on the AppStream application catalog, both surfacing on DescribeApplications in the wire fixture appstream-application-update-lifecycle. Neither cluster was marked OWNED or stub-blocked by the replay report. Overlap note: the open issue 20261111T040418Z-appstream-updateapplication-stub.md asks for UpdateApplication to be implemented at all (it recorded a 501 scaffold); the write path now answers, so cause 2 below states the read-back behavior the handler must carry and gives the wire replay key that proves it. 20261106T134718Z-appstream-createapplication-stub.md is the neighbouring create-side stub. A fixer should expect causes 1 and 2 to meet in the same application handler/projection files.

Cause 1 of 2: DescribeApplications never projects Application.Metadata, so the member is absent from every application in the response.
AWS: Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): shape com.amazonaws.appstream#Application carries member Metadata -> com.amazonaws.appstream#Metadata (type map), documented "Additional attributes that describe the application."; DescribeApplications returns DescribeApplicationsResult{Applications: Applications (list of Application), NextToken}, errors OperationNotPermittedException (400) and ResourceNotFoundException (404). Recorded behavior for the content: AWS serves Metadata as a populated map on an application created from an app block, keyed by the application's launch attributes (the recording asserts the key WORKING_DIRECTORY is present; its value is not asserted). The key set is derived from the application's own attributes (working directory, launch path/parameters supplied at create/update time) — it is not a fixed literal.
Emulator: HTTP 200 with the application projected, but $.Applications[0].Metadata is MISSING entirely (expected a map containing WORKING_DIRECTORY, got None). The status and every other asserted member of that first application match on step task#8.
Why: a client that reads Application.Metadata to recover an application's additional attributes (the console's application detail view, and any tooling that round-trips an application definition) gets a null/absent map where AWS always serves one, so it either KeyErrors on WORKING_DIRECTORY or silently treats the application as having no attributes. Because the response is otherwise a well-formed 200, nothing signals the omission.
Fix: the DescribeApplications response projection for the Application shape in the appstream service crate (the Application -> wire serializer shared by DescribeApplications/UpdateApplication/CreateApplication), plus whatever application state record feeds it — the Metadata map must be derived and emitted, not dropped.
Traces: appstream-application-update-lifecycle#task#8 (DescribeApplications, 200 vs 200, keys $.Applications[0].Metadata).

Cause 2 of 2: an UpdateApplication edit is not visible to the following DescribeApplications — the read serves the pre-update values.
AWS: Smithy: UpdateApplication{Name (required, pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$), DisplayName (len 0..100), Description (len 0..256), IconS3Location, LaunchPath (len 1..), WorkingDirectory (len 1..), LaunchParameters (len 1..), AppBlockArn, AttributesToDelete (len 0..2)} -> UpdateApplicationResult{Application}, errors ConcurrentModificationException (400), OperationNotPermittedException (400), ResourceNotFoundException (404). An accepted UpdateApplication mutates the stored application, so a subsequent DescribeApplications for the same ARN must return the NEW DisplayName and LaunchPath. Recorded behavior: after a successful UpdateApplication the read-back serves the updated DisplayName and LaunchPath (the recording's own values are per-run examples, not requirements) together with the Metadata map of cause 1.
Emulator: HTTP 200, but the read-back serves the ORIGINAL values: $.Applications[0].DisplayName value_mismatch (expected the updated display name, got the pre-update one) and $.Applications[0].LaunchPath value_mismatch (expected the updated launch path, got the pre-update one), plus the same missing Metadata. The update is accepted upstream and then lost: either UpdateApplication does not write the mutated fields back into service state, or DescribeApplications resolves the application from a create-time snapshot instead of current state.
Why: the write is silently lost. A client that updates an application and then reads it back to confirm (the standard control-plane pattern) sees its edit vanish with no error, so it retries forever or proceeds believing the old launch path/display name is current — a lost update, the most damaging class of state divergence, because every status code says success.
Fix: the UpdateApplication handler's state mutation and the DescribeApplications state lookup in the appstream service crate — UpdateApplication must apply each supplied member (and AttributesToDelete) to the stored application record, and DescribeApplications must read that record rather than a stale copy. The Metadata derivation of cause 1 belongs in the same projection.
Traces: appstream-application-update-lifecycle#task#10 (DescribeApplications, 200 vs 200, keys $.Applications[0].DisplayName, $.Applications[0].LaunchPath, $.Applications[0].Metadata).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator booted from this checkout (emulator_up true, /__admin/health 200): 150 steps, 72 passed, 78 mismatched, score 48%.

Cause 1 — fixture appstream-application-update-lifecycle, step task#8, operation DescribeApplications, verdict mismatch, expected_status 200, actual_status 200, affected_count 1:
  $.Applications[0].Metadata: missing (expected a map containing 'WORKING_DIRECTORY', got None)

Cause 2 — fixture appstream-application-update-lifecycle, step task#10, operation DescribeApplications, verdict mismatch, expected_status 200, actual_status 200, affected_count 1:
  $.Applications[0].DisplayName: value_mismatch (expected the updated display name, got the pre-update display name)
  $.Applications[0].LaunchPath: value_mismatch (expected the updated launch path, got the pre-update launch path)
  $.Applications[0].Metadata: missing (expected a map containing 'WORKING_DIRECTORY', got None)

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): Application.Metadata -> Metadata (map), "Additional attributes that describe the application."; DescribeApplicationsResult{Applications, NextToken}; UpdateApplication input members DisplayName/LaunchPath/WorkingDirectory/LaunchParameters/AppBlockArn/AttributesToDelete with output Application.

The concrete application name, ARN, display-name and launch-path strings in the recording are per-run values and are not requirements; the requirements are that Metadata is present as a map keyed by the application's attributes, and that a DescribeApplications after a successful UpdateApplication reflects the updated members.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay fixture appstream-application-update-lifecycle from trace set simparity-wire-aws-v1.
3. At step task#8, call DescribeApplications for the created application's ARN: observe HTTP 200 whose Applications[0] has NO Metadata member, where AWS serves a Metadata map containing the WORKING_DIRECTORY key.
4. Call UpdateApplication(Name=<the application>, DisplayName=<new value>, LaunchPath=<new value>) and then, at step task#10, DescribeApplications for the same ARN: observe the response still carries the pre-update DisplayName and LaunchPath (and still no Metadata), where AWS serves the updated values.
5. Re-replay appstream-application-update-lifecycle#task#8 and appstream-application-update-lifecycle#task#10 to confirm both causes are closed.
