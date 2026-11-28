Issue-Source: trace-sweep-divergence
Issue-Trace: polly-speech-synthesis-task-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: polly-speech-synthesis-task-lifecycle
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly|StartSpeechSynthesisTask|2xx->2xx|$.SynthesisTask.OutputUri
Issue-Commit: ddf45e8bed609c152204385892b724152ed64798

# Polly StartSpeechSynthesisTask announces an OutputUri that does not resolve to the task's output object

> _Note: this issue was identified against the code base at commit `ddf45e8bed609c152204385892b724152ed64798`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisTask

## Description
StartSpeechSynthesisTask returns 200 and every other member of the created SynthesisTask matches the recording, but the SynthesisTask.OutputUri it announces cannot be resolved to the output object of the task it just created: the replay classifies the member as `resource_not_identified` rather than a plain value mismatch. 1 cause.

Cause 1 of 1: SynthesisTask.OutputUri does not address the task's output object at an endpoint that is actually served.
AWS: recorded behavior only for the URI's concrete shape — the Smithy model (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) documents SynthesisTask$OutputUri only as "Pathway for the output speech file" (smithy_query(query="doc", shape="SynthesizeTask", member="OutputUri")) and declares no pattern or length on it; StartSpeechSynthesisTask is POST /v1/synthesisTasks with a 200 success status and takes OutputS3BucketName (required) and the optional OutputS3KeyPrefix. What the recording shows is a path-style URL addressing the OUTPUT OBJECT in the bucket the request named: scheme + S3 data-plane authority + `/<OutputS3BucketName>/<OutputS3KeyPrefix><TaskId>.<suffix-of-OutputFormat>`. The fixed part of that template is the path-style `/<bucket>/<key>` form and the derivation of the key from the request's OutputS3KeyPrefix, the task's own TaskId and the requested OutputFormat's suffix; the authority, the bucket name and the TaskId are variable parts taken from the environment, the request and the minted task, so neither the recorded AWS host nor the recorded task-id uuid is itself a requirement. The requirement the member carries is that the URI is FOLLOWABLE: a caller handed it must reach the object the completed task writes, so the authority has to be the data-plane address this emulator is actually reached/served on, not some other address. Status 200 in both recording and emulator, so no error code or message is involved.
Emulator: for the same step the emulator answers 200 but announces `https://127.0.0.1:41175/<bucket>/<task-id>.mp3`, and the replay cannot identify that as the task's output resource — `$.SynthesisTask.OutputUri: resource_not_identified (expected 'https://s3.us-west-2.amazonaws.com/sim2real-probe-polly-synthbucket-d291409a/ffb4619c-db31-4696-9ca7-de10defc06c2.mp3', got 'https://127.0.0.1:41175/sim2real-probe-polly-synthbucket-d291409a/e7279dbc-0af0-45f7-9133-147906da9a94.mp3')`. The bucket segment and the `/<bucket>/<key>` form agree; what does not resolve is the announced AUTHORITY, which in the same replay run is not the data-plane listener the emulator reports itself reachable on (the run's health probe reports `/__admin/health answered 200 and the :46365 data listener accepts`, while the announced port is 41175). The authority is built in services/service-com-amazonaws-polly-parrot-v1/src/state/output_location.rs::announced_base from reached_emulator_endpoint_of_parts(ctx.parts()), falling back to served_endpoint_authority and then to the raw context endpoint; the announced value is whichever of those wins, and the fallback it lands on here is not the served data-plane address, so the location the task reports is not an address the object can be read at.
Why: the asynchronous synthesis API hands the caller nothing but this URI — a client that starts a task, waits for `completed`, then GETs OutputUri to read the audio (or the json speech-mark stream) gets a connection to a port nothing serves instead of the object, so the entire async workflow produces no retrievable output even though the task is accepted and reports success. Record-and-replay harnesses and contract tests that resolve OutputUri to check the delivered object likewise fail to identify the resource for a call that otherwise matched AWS exactly.
Fix: the announced-location component of the operation — the authority resolution in services/service-com-amazonaws-polly-parrot-v1/src/state/output_location.rs (announced_base / announced_output_uri) as consumed by the handler's StoredSynthesisTask construction in services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs. It must announce the emulator's served data-plane authority — the address the request actually reached it on — so the path-style `/<bucket>/<key>` location it already builds resolves to the object the task's completion writes; the key derivation (prefix + TaskId + format suffix) is already correct and should not change.
Traces: polly-speech-synthesis-task-lifecycle#task#2 (StartSpeechSynthesisTask, 200 -> 200, `$.SynthesisTask.OutputUri: resource_not_identified`).

Not covered by any open issue: no open polly issue carries this step's replay key. 20261103T042343Z-polly-startspeechsynthesistask-omits-the-synthesistask-sdkfeaturetoggles-member-from.md names the same fixture/step but for the absent SdkFeatureToggles member only; 20261022T175007Z-polly-startspeechsynthesistask-stub.md is the now-superseded stub issue (the operation answers 200 here); 20261030T071447Z-polly-synthesizespeech-output-stream-diverges-from-the-rendered.md owns the two SynthesizeSpeech clusters in this same replay and nothing about task output locations.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false; 12 fixtures, 49 steps, 45 passed, 4 mismatched, score 91.8%; emulator_detail: "/__admin/health answered 200 and the :46365 data listener accepts").

Divergence cluster (verdict=mismatch, service=polly, operation=StartSpeechSynthesisTask):
  fixture_id: polly-speech-synthesis-task-lifecycle
  step_key:   task#2
  expected_status: 200, actual_status: 200
  detail: $.SynthesisTask.OutputUri: resource_not_identified (expected 'https://s3.us-west-2.amazonaws.com/sim2real-probe-polly-synthbucket-d291409a/ffb4619c-db31-4696-9ca7-de10defc06c2.mp3', got 'https://127.0.0.1:41175/sim2real-probe-polly-synthbucket-d291409a/e7279dbc-0af0-45f7-9133-147906da9a94.mp3')
  divergence_key: service=polly|StartSpeechSynthesisTask|status=200->200|keys=$.SynthesisTask.OutputUri
  affected (1): polly-speech-synthesis-task-lifecycle#task#2
  stub_path: (none), owned_by: (none)

Expected vs actual, read as a template rather than as literals:
  template (both): <scheme>://<data-plane authority>/<OutputS3BucketName>/<OutputS3KeyPrefix><TaskId>.<suffix(OutputFormat)>
  fixed text:      the path-style '/<bucket>/<key>' form and the key derivation prefix+TaskId+'.'+format suffix
  variable parts:  the authority (environment), the bucket (from the request), the TaskId (minted per run) — none of the recorded literals is a requirement
  divergent part:  the announced authority, which the replay cannot identify as this emulator's object-storage location ('resource_not_identified'); in the same run the emulator reports its data listener at :46365 while the announced authority is 127.0.0.1:41175, so the URI handed to the caller is not served.
No other member of the SynthesisTask diverged in this step and the status agreed at 200.

Model facts used: smithy_query(query="operation", operation="StartSpeechSynthesisTask,GetSpeechSynthesisTask") — POST /v1/synthesisTasks, 200, required OutputFormat/OutputS3BucketName/Text/VoiceId, optional OutputS3KeyPrefix (pattern ^[0-9a-zA-Z\/\!\-_\.\*\'\(\):;\$@=+\,\?&]{0,800}$); smithy_query(query="doc", shape="SynthesisTask", member="OutputUri") — "Pathway for the output speech file", no constraints modeled, hence "recorded behavior only" for the URI's concrete shape.
Source corroboration: services/service-com-amazonaws-polly-parrot-v1/src/state/output_location.rs lines 37-70 (announced_output_uri / announced_base authority order) and services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs lines 282-292 (output_key) and 461-509 (output_uri stored on the task alongside the owed delivery).

## Steps to Reproduce
1. Boot the emulator from this checkout and note the data-plane listener it serves on (the address /__admin/health is probed at).
2. Create the output bucket, then call StartSpeechSynthesisTask with OutputFormat=mp3, VoiceId=Joanna, OutputS3BucketName=<bucket> and a short Text.
3. Read SynthesisTask.OutputUri from the 200 response and compare its authority with the emulator's served data-plane address: the emulator announces a different authority (e.g. 127.0.0.1:41175 while the data listener accepts on :46365).
4. Wait for the task to reach `completed` (GetSpeechSynthesisTask) and then GET the announced OutputUri: the request does not reach the output object, whereas on AWS the URI resolves path-style to <bucket>/<OutputS3KeyPrefix><TaskId>.mp3.
5. Re-replay fixture polly-speech-synthesis-task-lifecycle, step task#2, and confirm $.SynthesisTask.OutputUri is identified as the task's output resource instead of reporting `resource_not_identified`.

## Resolution

Cause 1 is DECLINED as a per-environment value, and the requirement behind it is pinned by tests
rather than changed in the handler.

The announced location was already rooted at an address this emulator is reached at: the
emulator's own endpoint the answering request arrived on, else the address this process is served
at (`src/state/output_location.rs`, which calls the one reading `services/common` owns for that
question). The replayed client addresses AWS's own hostnames, so the first branch yields nothing
and the announcement is the SERVED base — checked by replacing the reading with the served base
unconditionally and re-replaying, which gave a byte-identical verdict. The two ports the report
compares (the announced one and the `:46365` of the run's health detail) are not from the same
emulator instance.

The member therefore differs from the recording only in the authority and in the per-run TaskId,
both of which the report itself states are not requirements, and the recorded authority
(`s3.<region>.amazonaws.com`) is one this emulator must not announce: a client sent there leaves
the emulated environment, where the bucket does not exist. The replay key
`polly|StartSpeechSynthesisTask|2xx->2xx|$.SynthesisTask.OutputUri` stays mismatched for that
reason, exactly as the earlier triage of the same member recorded
(`20261118T125815Z-polly-startspeechsynthesistask-omits-the-synthesistask-sdkfeaturetoggles-member-from-6a8d3c.md`:
"Excluded as per-environment … differs only in host:port and in the generated task id inside the
key, with the bucket name and the path-style URL form identical").

What the fix adds is the evidence, as tables: `src/state/output_location.rs` now carries a 12-row
unit table over the addressing itself (including a caller that resolves AWS's own hostnames, which
is announced the served base, and a request a front end re-addressed to this process), and
`tests/all/speech_synthesis_task_creation.rs` drives the whole addressing table against three
bases this emulator may be served at, asserting the start call, the task read and the listing all
report the one location and that following it reaches the object the completed task wrote.
