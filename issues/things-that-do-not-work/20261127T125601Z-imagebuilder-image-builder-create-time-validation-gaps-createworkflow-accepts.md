Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.create-workflow-rejects-an-unknown-on-failure-mode imagebuilder.create-workflow-rejects-a-wait-step-timeout-above-seven-days imagebuilder.create-infrastructure-configuration-invalid-instance-profile-rejected
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.create-workflow-rejects-an-unknown-on-failure-mode|_refusal#9|AssertionError|raise AssertionError(f"the call was expected to be refused but answered {result!r}")
Issue-Key: imagebuilder.create-workflow-rejects-a-wait-step-timeout-above-seven-days|_refusal#8|AssertionError|raise AssertionError(f"expected a refusal, the call returned {got!r}")
Issue-Key: imagebuilder.create-infrastructure-configuration-invalid-instance-profile-rejected|run#7|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# Image Builder create-time validation gaps: CreateWorkflow accepts invalid step documents (bad onFailure, over-7-day wait timeout) and CreateInfrastructureConfiguration's missing-instance-profile message uses the wrong template

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Three net-new root causes in the imagebuilder emulator, all on create-time validation/refusal wording. THREE causes must be closed before submitting.

Cause 1 of 3: CreateWorkflow does not validate a step's `onFailure` against the allowed modes, so a document AWS refuses is stored and a build version is minted.
  AWS: CreateWorkflow validates the YAML workflow document's steps (Smithy: CreateWorkflow.data = InlineWorkflowData, @length 1..16000, pattern ^[^\x00]+$ — the model constrains only the document's SIZE and bytes, so the step-level rule is documented prose/recorded behavior only; `type` is documented as "Image Builder validates the workflow document steps against the stage you specify"). A step whose `onFailure` is neither `Abort` nor `Continue` is refused with HTTP 400 and error code `InvalidParameterValueException` (the service models the shape; this operation widens it in, see create_workflow.rs extra_errors). Message, as AWS words it for this class: fixed text `The value supplied for parameter '<member>' is not valid. YAML parse error: data.steps[<index>].onFailure must be one of ['Abort', 'Continue']` — the fixed parts are "The value supplied for parameter '…' is not valid. YAML parse error: ", "must be one of ['Abort', 'Continue']" and the `data.steps[<i>].onFailure` path shape; the variable parts are the member that carried the document (`data` or `uri`) and the zero-based index of the offending step. Nothing is created: GetWorkflow on the would-be build-version ARN answers ResourceNotFoundException and the version is not listed.
  Emulator: the create SUCCEEDS. A BUILD workflow whose single step carries `onFailure: Shrug` is answered HTTP 200 with a `workflowBuildVersionArn` of the form `arn:aws:imagebuilder:<region>:<account>:workflow/build/<name>/<version>/1` plus latestVersionReferences, so an unparseable mode is persisted as a real workflow build version.
  Why: a client that relies on AWS rejecting a malformed workflow document gets a workflow that cannot ever run; the typo surfaces only much later (or never) instead of at create, and negative-path code that expects InvalidParameterValueException never runs.
  Fix: the workflow-document reader behind CreateWorkflow — `crate::state::parse_workflow_document` must reject a step whose `onFailure` is outside the allowed set, so the existing `invalid_value(...)` refusal path in the CreateWorkflow handler fires.
  Traces: imagebuilder.create-workflow-rejects-an-unknown-on-failure-mode.

Cause 2 of 3: CreateWorkflow does not enforce the documented maximum wait of a `WaitForAction` step, so a timeout above seven days is stored.
  AWS: a `WaitForAction` step may wait at most seven days = 604800 seconds (AWS docs, workflow step actions: https://docs.aws.amazon.com/imagebuilder/latest/userguide/wfdoc-step-actions.html; the Smithy model states nothing about step contents, so this is documented/recorded behavior only). 604800 is accepted and stored verbatim in the workflow's `data`; 604801 is refused with HTTP 400, code `InvalidParameterValueException`, message with fixed text `The value supplied for parameter '<member>' is not valid. YAML parse error: data.steps[<index>].timeoutSeconds must be smaller than or equal to 604800` — fixed: the "The value supplied for parameter '…' is not valid. YAML parse error: " prefix, the "must be smaller than or equal to" comparison and the 604800 limit itself; variable: the carrying member (`data`/`uri`) and the step index. The refused create leaves no build version behind.
  Emulator: the create SUCCEEDS — a TEST workflow with `action: WaitForAction` and `timeoutSeconds: 604801` is answered HTTP 200 and mints a workflow build version ARN (…/workflow/test/<name>/1.0.0/1).
  Why: a step can be created that outlives the documented maximum, so a client's guard against an unbounded manual-approval pause is silently ineffective and the emulator diverges from AWS on the boundary (604800 accepted / 604801 refused) that the rule is defined by.
  Fix: same component as Cause 1 — the step validation in `crate::state::parse_workflow_document` must carry the per-action bound for `WaitForAction.timeoutSeconds` and report it through the CreateWorkflow handler's invalid-value refusal.
  Traces: imagebuilder.create-workflow-rejects-a-wait-step-timeout-above-seven-days.

Cause 3 of 3: CreateInfrastructureConfiguration refuses a nonexistent instanceProfileName with the generic parameter template instead of AWS's instance-profile wording.
  AWS: `instanceProfileName` is required and "The instance profile must exist in your account" (Smithy: CreateInfrastructureConfiguration.instanceProfileName, InstanceProfileNameType, required, @length 1..256, pattern ^[\w+=,.@-]+$; InvalidParameterValueException is the service-modelled client error, HTTP 400, widened onto this operation). A name the account holds no instance profile under is refused HTTP 400 / `InvalidParameterValueException` with the message `Invalid parameter value for instanceProfileName: The provided instance profile does not exist. Please specify a different instance profile and try again.` — fixed text: "Invalid parameter value for ", ": The provided instance profile does not exist. Please specify a different instance profile and try again."; the only variable part is the member name it names (`instanceProfileName`). Nothing is created.
  Emulator: the status and error code are right, but the message is `The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again.` — it reuses the generic "The value supplied for parameter '<member>' is not valid." prefix (the correct prefix for the CreateWorkflow `data` class above) where AWS uses the "Invalid parameter value for <member>: " prefix for this refusal.
  Why: a client or test that matches on the error text — the documented wording for a missing instance profile — does not match, so its retry/remediation branch is skipped even though the code is correct. Related but distinct from the already-open message-template issues 20261125T024626Z-…-getdistributionconfiguration-not-found-message-now-uses… and 20261125T084656Z-imagebuilder-image-builder-distribution-configuration-not-found-refusal-uses--r2.md, which cover not-found ARN wording, not this parameter-value prefix; neither covers this check.
  Fix: the message builder this refusal uses — `crate::state::missing_instance_profile_message`, reached from the instance-profile resolution branch of the CreateInfrastructureConfiguration handler — must emit AWS's "Invalid parameter value for <member>: …" template rather than the generic parameter-value prefix.
  Traces: imagebuilder.create-infrastructure-configuration-invalid-instance-profile-rejected.

## Evidence
Cause 1 — check imagebuilder.create-workflow-rejects-an-unknown-on-failure-mode:
  Expected: ('InvalidParameterValueException', "The value supplied for parameter 'data' is not valid. YAML parse error: data.steps[0].onFailure must be one of ['Abort', 'Continue']")
  Actual:   AssertionError: the call was expected to be refused but answered {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'clientToken': '…', 'workflowBuildVersionArn': 'arn:aws:imagebuilder:us-west-2:000000000000:workflow/build/<name>/2.0.0/1', 'latestVersionReferences': {...}}

Cause 2 — check imagebuilder.create-workflow-rejects-a-wait-step-timeout-above-seven-days:
  Document: step action WaitForAction, onFailure Abort, timeoutSeconds 604801 (604800 accepted and stored, as the check's control asserts).
  Expected: ('InvalidParameterValueException', "The value supplied for parameter 'data' is not valid. YAML parse error: data.steps[0].timeoutSeconds must be smaller than or equal to 604800")
  Actual:   AssertionError: expected a refusal, the call returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'workflowBuildVersionArn': 'arn:aws:imagebuilder:us-west-2:000000000000:workflow/test/<name>/1.0.0/1', ...}

Cause 3 — check imagebuilder.create-infrastructure-configuration-invalid-instance-profile-rejected:
  Code matched (InvalidParameterValueException); message diverged.
  Expected: 'Invalid parameter value for instanceProfileName: The provided instance profile does not exist. Please specify a different instance profile and try again.'
  Actual:   "The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again."

Handler/model grounding: services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/create_workflow.rs (document read at the `parse_workflow_document` call, refusal via `invalid_value`); services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/create_infrastructure_configuration.rs (instance-profile branch calling `crate::state::missing_instance_profile_message`); smithy-model/com-amazonaws-imagebuilder-imagebuilder.json.

## Steps to Reproduce
Cause 1: ib.create_workflow(name=<n>, semanticVersion='1.0.0', type='BUILD', data="name: <n>\ndescription: one step workflow\nschemaVersion: 1.0\nsteps:\n  - name: OnlyStep\n    action: WaitForAction\n    onFailure: Shrug\n", clientToken=<uuid>). Expect HTTP 400 InvalidParameterValueException with the onFailure-enum YAML parse message; the emulator answers 200 and mints …/workflow/build/<n>/1.0.0/1. Then GetWorkflow on that ARN must be ResourceNotFoundException.

Cause 2: ib.create_workflow(name=<n>, semanticVersion='1.0.0', type='TEST', data="name: longpause\ndescription: waits for a reviewer\nschemaVersion: 1.0\nsteps:\n  - name: ReviewerGate\n    action: WaitForAction\n    onFailure: Abort\n    timeoutSeconds: 604801\n", clientToken=<uuid>). Expect HTTP 400 InvalidParameterValueException with the timeoutSeconds<=604800 YAML parse message and no build version; the same document with 604800 must be accepted and stored. The emulator accepts 604801 with HTTP 200.

Cause 3: create an IAM role+instance profile is NOT needed — call ib.create_infrastructure_configuration(name=<n>, instanceProfileName=<a name no instance profile exists under>, clientToken=<uuid>). Expect error code InvalidParameterValueException with message 'Invalid parameter value for instanceProfileName: The provided instance profile does not exist. Please specify a different instance profile and try again.'; the emulator returns the generic "The value supplied for parameter 'instanceProfileName' is not valid. …" prefix instead. Then list_infrastructure_configurations(filters=[{'name':'name','values':[<n>]}]) must be empty (it is).
