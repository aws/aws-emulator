Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-create-image-builds-an-available-ami
Issue-Step: task#20
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-create-image-builds-an-available-ami
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|GetImage|2xx->2xx|$.image.workflows[*]
Issue-Commit: 4e68013acb17007a4b50a9cc9c0932294de10e0a

# Image Builder GetImage omits the test-stage service-owned workflow from image.workflows

> _Note: this issue was identified against the code base at commit `4e68013acb17007a4b50a9cc9c0932294de10e0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImage

## Description
Cause 1 of 1: a read of an image built WITHOUT caller-supplied `workflows` reports only the build-stage service-owned workflow configuration; the test-stage one is missing from `image.workflows`.

AWS: `Image.workflows` (shape `com.amazonaws.imagebuilder#Image`, target `WorkflowConfigurationList` of `WorkflowConfiguration`, whose only required member is `workflowArn` with pattern `^arn:aws(?:-[a-z]+)*:imagebuilder:...:(?:[0-9]{12}|aws(?:-[a-z-]+)?):workflow/(build|test|distribution)/...$`) is documented as "The build, test, and distribution workflow configurations that are associated with the image." `CreateImage.workflows` documents "If you specify workflows, they replace the default workflows that Image Builder otherwise runs for the build" — so a build that specified none is associated with Image Builder's DEFAULT workflow set. The model does not enumerate which workflows that set contains or at which versions AWS publishes them, so for that part this is recorded behavior only: the recording of a plain AMI build shows `image.workflows` carrying TWO entries — the build-stage workflow owned by `aws`, and a test-stage entry `{"workflowArn": "arn:aws:imagebuilder:<region>:aws:workflow/test/test-image/<version>/<build>"}`. Both are present on the built image regardless of the build's `imageTestsConfiguration`; the recorded test-stage entry is also published at a DIFFERENT semantic version from the build-stage one (the two service-owned workflows are versioned independently by AWS).

Emulator: GetImage returns 200 with `image.workflows` holding a single element — index [1] is absent entirely (`$.image.workflows[1]: missing (expected {'workflowArn': 'arn:aws:imagebuilder:us-west-2:aws:workflow/test/test-image/1.0.3/1'}, got None)`). The build-stage entry at index [0] matches. Two things in the default-workflow derivation produce this: (a) the set is FILTERED on whether image tests are enabled, so the TEST-stage service-owned workflow is dropped from the recorded run rather than being associated with the image, and (b) every service-owned workflow ARN is minted from ONE shared version constant, so even when the test entry is emitted its version node cannot match the version AWS actually publishes that workflow at, while the build-stage one happens to agree.

Why: a client that reads back the workflows associated with an image — to show the build/test/distribution stages a build ran, to re-submit the same workflow set on a later `CreateImage`, or to resolve each `workflowArn` with `GetWorkflow`/`GetWorkflowExecution` — sees only the build stage. It concludes the image was built with no test workflow at all, so test-stage execution history, `ListWorkflowExecutions`-style drill-down and any round-trip of the reported configuration back into a create are all built on an incomplete list. A client that pins the reported workflow ARNs also gets a version node that does not name the workflow build AWS published.

Fix: the image build's workflow observation and the service-owned workflow catalogue it draws on — the default workflow set must contain the build-stage AND test-stage service-owned workflow for the build's output kind, independently of `imageTestsConfiguration.imageTestsEnabled`, and each service-owned workflow must carry its OWN published semantic version rather than one constant shared across all of them. The stored value then flows through the image read unchanged.

Traces: imagebuilder-create-image-builds-an-available-ami#task#20 (GetImage, 200/200, `$.image.workflows[1]` missing).

## Evidence
Fixture `imagebuilder-create-image-builds-an-available-ami`, step `task#20`, operation GetImage, service imagebuilder. verdict=mismatch, expected_status=200, actual_status=200.

detail: `$.image.workflows[1]: missing (expected {'workflowArn': 'arn:aws:imagebuilder:us-west-2:000000000000-style aws-owned arn:aws:imagebuilder:us-west-2:aws:workflow/test/test-image/1.0.3/1'}, got None)` — verbatim from the replay: `$.image.workflows[1]: missing (expected {'workflowArn': 'arn:aws:imagebuilder:us-west-2:aws:workflow/test/test-image/1.0.3/1'}, got None)`.

Expected: `image.workflows` is a two-element list — the build-stage service-owned workflow followed by the test-stage service-owned workflow (`workflow/test/test-image/...`), each named by a fully-assigned build-version ARN owned by `aws`. The region, the workflow version and the build node in the recorded ARN are environment/catalogue values filling the template `arn:<partition>:imagebuilder:<region>:aws:workflow/test/<test-workflow-name>/<published-version>/<build>`; the fixed parts are the `workflow/test/` stage segment, the `aws` owner and the presence of the entry.

Actual: `image.workflows` has exactly one element (the build-stage entry); index 1 does not exist.

Emulator code path: `services/service-com-amazonaws-imagebuilder-imagebuilder/src/state/image_build_run.rs` — `default_build_workflows()` (lines 170-186) filters `SERVICE_OWNED_WORKFLOWS` with `tests_enabled || workflow.workflow_type != TEST_WORKFLOW_TYPE`, and `ServiceOwnedWorkflow::build_version_arn()` (lines 146-153) mints every service-owned ARN from the single constant `SERVICE_OWNED_WORKFLOW_VERSION = "1.0.1"` (line 89). `ImageBuildRun::observed()` (lines 221-242) records that list on the build, `CreateImage` stores it (`src/ops/create_image.rs`, `workflows: Some(run.workflows)`), and `state/image.rs` `image_view()` reports it unchanged (`workflows: record.workflows.clone()`), so the omission originates in the default-set derivation, not in the read.

No open issue covers this: the open imagebuilder issues concern build status progression (20261115T043558Z), distribution output AMIs (20261124T052905Z), the not-found message template (20261118T174922Z) and tagging 204 framing (20261122T204134Z); none of them touch `image.workflows`.

## Steps to Reproduce
1. Replay fixture `imagebuilder-create-image-builds-an-available-ami` against the emulator (`replay_wire_corpus`).
2. By hand: create an image recipe, an infrastructure configuration, then call `CreateImage` with only `imageRecipeArn` + `infrastructureConfigurationArn` (no `workflows`, no `executionRole`).
3. Wait for the build to reach AVAILABLE and call `GetImage` with the returned `imageBuildVersionArn`.
4. Inspect `image.workflows`: the emulator returns a single-element list containing only the build-stage service-owned workflow. AWS returns both the build-stage and the test-stage service-owned workflow configurations (`workflow/build/...` and `workflow/test/...`, owner `aws`), each at its own published version.
