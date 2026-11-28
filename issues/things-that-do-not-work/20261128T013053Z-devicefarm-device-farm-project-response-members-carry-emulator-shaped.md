Issue-Source: trace-sweep-divergence
Issue-Trace: devicefarm-run-descendant-reads-on-absent-run
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devicefarm-run-descendant-reads-on-absent-run devicefarm-stop-run-job-session-refusals devicefarm-stop-job-refuses-non-job-arn devicefarm-stop-remote-access-session-refuses-non-session-arn
Issue-Shape-Id: com.amazonaws.devicefarm#DeviceFarm_20150623
Issue-Key: devicefarm|CreateProject|2xx->2xx|$.project.arn
Issue-Key: devicefarm|GetProject|2xx->2xx|$.project.created
Issue-Commit: f92d3cc1add58c930fe9ef1ae6c5aa7e04b710c1

# Device Farm Project response members carry emulator-shaped identity and timestamp values

> _Note: this issue was identified against the code base at commit `f92d3cc1add58c930fe9ef1ae6c5aa7e04b710c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProject

## Description
This bundle has 2 causes. Both are shape divergences in the `Project` structure the emulator returns under HTTP 200: the fixture asserts the SHAPE of the member (expected '<shaped-not-asserted>'), and the emulator returns a value whose shape is not the one AWS Device Farm produces. Neither cause is about the particular id or clock value in one recording — both are about the FORM the emulator mints. Both are reached on the happy path, so every downstream Device Farm read in the corpus carries them.

Cause 1 of 2: CreateProject mints a project ARN whose resource id is not an AWS-shaped project id (fixture devicefarm-run-descendant-reads-on-absent-run, step setup#1, operation CreateProject, 200 -> 200, key $.project.arn).
AWS: Smithy: CreateProject -> CreateProjectOutput.project (Project); Project$arn targets com.amazonaws.devicefarm#AmazonResourceName with @length min 32 max 1011 and @pattern ^arn:aws:devicefarm:.+$. The model's pattern is deliberately loose, so the ARN's internal shape is recorded behavior only: AWS Device Farm mints a project ARN of the form arn:aws:devicefarm:<region>:<account-id>:project:<guid>, where <guid> is a lowercase GUID in the 8-4-4-4-12 hex form (the model's own doc example for a Device Farm ARN is written as EXAMPLE-GUID-123-456). The region and 12-digit account segments are environment values and are NOT part of the requirement; the GUID FORM of the resource-id segment is.
Emulator: the resource-id segment is a 16-hex-digit zero-padded monotonic counter followed by a literal marker word, e.g. `arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated` — one segment, no GUID grouping, and an emulator-internal marker token embedded in a customer-visible identifier. The counter value and the region/account in that example are per-environment; the `<16-hex>-emulated` shape is what diverges.
Why: a client that round-trips, indexes, logs or validates Device Farm project ids as GUIDs (the shape every real project ARN has) rejects or mis-parses every ARN the emulator mints, and the ARN is the handle for every subsequent GetProject/ListRuns/CreateUpload call, so the failure is not cosmetic. The embedded marker also leaks emulator identity into data a client may persist.
Fix: the project-ARN minting in the devicefarm service's state helper (the resource-id generator shared by the Project handlers, not the CreateProject handler body) should emit a GUID-shaped, never-reused resource id; the child-resource minting that embeds the project id inherits the fix. See Traces for citations.
Traces: devicefarm-run-descendant-reads-on-absent-run#setup#1 (representative), devicefarm-stop-run-job-session-refusals#setup#1. Code: services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/state.rs lines 428-438 (mint_project_arn formats `{PROJECT_RESOURCE_TYPE}:{:016x}-{RESOURCE_ID_SUFFIX}`), line 531 (RESOURCE_ID_SUFFIX = "emulated"), lines 520-527 and 480-491 (the same shape reused for devicepool/networkprofile/upload ids); services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/ops/create_project.rs line 204.

Cause 2 of 2: Project$created is serialized with sub-millisecond precision AWS never emits (fixture devicefarm-stop-job-refuses-non-job-arn, step task#2, operation GetProject, 200 -> 200, key $.project.created).
AWS: Smithy: Project$created targets com.amazonaws.devicefarm#DateTime (type timestamp); Device Farm is awsJson1_1, so a timestamp with no @timestampFormat override goes on the wire as epoch-seconds, a JSON number. AWS's epoch-seconds serialization carries at most millisecond resolution — three fractional digits, and no fractional part at all when the value is whole (recorded behavior only for the digit count; the model fixes only the epoch-seconds encoding).
Emulator: the member is emitted as an epoch-seconds number with seven fractional digits, e.g. `1795829319.4706488`, because the stored instant keeps full nanosecond sub-second precision and is serialized unrounded. The particular instant in that example is a per-run clock value and is NOT the finding; the seven-fractional-digit form is.
Why: a client that parses Device Farm timestamps into a millisecond-resolution type, or that compares/serializes them back and asserts byte-stability, sees a value it cannot represent or round-trip, and any consumer asserting on AWS's timestamp precision diverges. The same helper backs every `created`-style member in this service, so the whole service's timestamps are affected, not just Project.
Fix: the shared timestamp helper in the devicefarm service's state module (the one every handler mints `created` from) should build the modelled timestamp at millisecond resolution rather than carrying raw nanoseconds into the serializer.
Traces: devicefarm-stop-job-refuses-non-job-arn#task#2 (this cause's representative), devicefarm-stop-remote-access-session-refuses-non-session-arn#task#2. Code: services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/state.rs lines 841-849 (`now()` passes `since_epoch.subsec_nanos()` straight to `DateTime::from_secs_and_nanos`); services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/ops/create_project.rs line 211.

No open issue covers either cause. The two open devicefarm-related issues are unrelated: 20261127T091234Z-devicefarm-listuploads-stub.md (ListUploads scaffold stub) and 20261109T130344Z-cloudformation-devicefarm-no-aws-devicefarm-resource-type-is.md (CloudFormation resource-type registration).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 81 steps, 34 passed, 36 mismatched, 11 blocked, score 42%).

Cause 1 — divergence_key `service=devicefarm|CreateProject|status=200->200|keys=$.project.arn`, verdict mismatch, affected_count 2:
  expected $.project.arn: AWS-shaped project ARN, resource id a GUID — arn:aws:devicefarm:<region>:<account>:project:<8-4-4-4-12 hex>
  actual   $.project.arn: 'arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated'
  detail: "$.project.arn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:devicefarm:us-west-2:000000000000:project:0000000000000001-emulated')"
  Status matches (200); only the member's shape diverges. Fixed part of the requirement: the `arn:aws:devicefarm:` frame and the `project:` resource type. Variable parts taken from the environment/state: region, 12-digit account id, and the per-project id — the ID's GUID form is required, its value is not. Not required: the literal counter `0000000000000001` or the marker `-emulated`, which has no counterpart in AWS output.

Cause 2 — divergence_key `service=devicefarm|GetProject|status=200->200|keys=$.project.created`, verdict mismatch, affected_count 2:
  expected $.project.created: epoch-seconds JSON number at AWS's millisecond resolution (<= 3 fractional digits)
  actual   $.project.created: 1795829319.4706488 (7 fractional digits)
  detail: "$.project.created: value_mismatch (expected '<shaped-not-asserted>', got 1795829319.4706488)"
  Status matches (200). Fixed part of the requirement: epoch-seconds JSON number with at most millisecond resolution. Variable part: the instant itself, which comes from the clock and is explicitly NOT asserted here.

Source in the checkout confirming both are minting/serialization shape choices rather than recording artifacts: state.rs:428-438 and :531 (`{:016x}-emulated`), state.rs:841-849 (`from_secs_and_nanos(secs, subsec_nanos())`).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture `devicefarm-run-descendant-reads-on-absent-run`, step `setup#1` — CreateProject with a project name. The call answers 200 and $.project.arn comes back as arn:aws:devicefarm:<region>:<account>:project:<16-hex>-emulated instead of an ARN whose resource id is a GUID. Same cluster: devicefarm-stop-run-job-session-refusals#setup#1.
4. Cause 2: fixture `devicefarm-stop-job-refuses-non-job-arn`, step `task#2` — GetProject on the project created in setup. The call answers 200 and $.project.created comes back as an epoch-seconds number with 7 fractional digits (e.g. 1795829319.4706488) instead of AWS's millisecond-resolution epoch-seconds. Same cluster: devicefarm-stop-remote-access-session-refuses-non-session-arn#task#2.
5. Both clusters are reported with empty stub_path and empty owned_by, so neither is already filed.
