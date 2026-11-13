Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.deploy-ssm-patchbaseline-via-cloudformation-then-lifecycle cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning
Issue-Key: cloudformation__ssm.deploy-ssm-patchbaseline-via-cloudformation-then-lifecycle|run#9|AssertionError|assert _listed(ssm, baseline_name) == [baseline_id], (
Issue-Key: cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning|run#14|AssertionError|assert statuses == ["CREATE_IN_PROGRESS", "CREATE_FAILED", "DELETE_COMPLETE"], (
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudformation__ssm: 2 net-new causes — a CFN-created AWS::SSM::PatchBaseline is invisible to DescribePatchBaselines, and rollback emits DELETE_IN_PROGRESS for a resource the handler never provisioned

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent root causes. They live in different components and must both be closed before this issue is submitted.

Cause 1 of 2: a patch baseline a CREATE_COMPLETE stack provisioned is not visible to SSM's DescribePatchBaselines.
Failing check: cloudformation__ssm.deploy-ssm-patchbaseline-via-cloudformation-then-lifecycle
  AWS:      a CREATE_COMPLETE AWS::SSM::PatchBaseline is a real SSM patch baseline. DescribePatchBaselines, filtered on the baseline's name, returns exactly that baseline, and its BaselineId equals the stack resource's PhysicalResourceId — so the resource CloudFormation says it created is the one SSM's own list path serves. Recorded behavior: the recording reads back exactly `[<baseline_id>]` for the created baseline's name.
  Emulator: DescribePatchBaselines for the created baseline's name returns `[]`. The resource handler reports CREATE_COMPLETE and hands back a BaselineId, but the baseline is absent from the state DescribePatchBaselines reads (never registered, or registered where the list path cannot see it).
  Why:      a client that provisions a patch baseline through CloudFormation cannot then find it, reference it, associate it with patch groups, or clean it up through SSM: the control plane reports a resource that SSM's own reads deny exists. Every later leg of the baseline's lifecycle in the same trajectory is unreachable.
  Fix:      the CloudFormation AWS::SSM::PatchBaseline resource handler and the SSM patch-baseline state it writes — the handler must register the baseline in the same state DescribePatchBaselines serves, keyed by the BaselineId it publishes as the PhysicalResourceId.
  Traces:   cloudformation__ssm.deploy-ssm-patchbaseline-via-cloudformation-then-lifecycle.

Cause 2 of 2: with validation disabled, a create whose resource handler failed before provisioning anything still produces a DELETE_IN_PROGRESS event during rollback.
Failing check: cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning
  AWS:      when a create is submitted with validation disabled and the resource handler itself fails before provisioning anything — here AWS::SSM::Parameter, whose PutParameter is refused because the parameter already exists — the rollback has no physical resource to delete, so that logical resource's event sequence is exactly CREATE_IN_PROGRESS, CREATE_FAILED, DELETE_COMPLETE. No DELETE_IN_PROGRESS is emitted, because no delete is ever started. Recorded behavior: the recording asserts that three-element sequence for the logical resource.
  Emulator: the logical resource's events are CREATE_IN_PROGRESS, CREATE_FAILED, DELETE_IN_PROGRESS, DELETE_COMPLETE — an extra DELETE_IN_PROGRESS for a resource that was never provisioned. (The CREATE_FAILED reason itself, carrying the handler's AlreadyExists refusal, is as AWS reports it; only the event sequence diverges.)
  Why:      a client that reads stack events to decide whether a failed resource ever existed — cleanup logic, drift reconciliation, audit trails, anything that treats DELETE_IN_PROGRESS as proof a physical resource was created and then removed — is told a delete was started against a resource that was never created.
  Fix:      the CloudFormation rollback path's per-resource stack-event emission: skip DELETE_IN_PROGRESS for a logical resource whose create attempt never produced a physical resource, and go straight to DELETE_COMPLETE.
  Traces:   cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning.

## Evidence
Cause 1 — cloudformation__ssm.deploy-ssm-patchbaseline-via-cloudformation-then-lifecycle [mismatch]:
  assert _listed(ssm, baseline_name) == [baseline_id]
  AssertionError: DescribePatchBaselines for '<baseline-name>' found []
  Expected: DescribePatchBaselines filtered on the baseline name returns [<the stack resource's PhysicalResourceId>].
  Actual:   [] — the baseline the CREATE_COMPLETE stack reports does not appear in SSM at all.

Cause 2 — cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning [mismatch]:
  AssertionError: Param event sequence with validation disabled: ['CREATE_IN_PROGRESS', 'CREATE_FAILED', 'DELETE_IN_PROGRESS', 'DELETE_COMPLETE']; the resource handler attempt never provisioned anything for rollback to delete, so there is no DELETE_IN_PROGRESS
  Observed rows: ('CREATE_IN_PROGRESS', None), ('CREATE_FAILED', 'Resource handler returned message: "Error occurred during operation \'PutParameter\': The parameter already exists. To overwrite this value, set the overwrite option in the request to true." (RequestToken: <token>, HandlerErrorCode: AlreadyExists)'), ('DELETE_IN_PROGRESS', None), ('DELETE_COMPLETE', None)
  Expected sequence: ['CREATE_IN_PROGRESS', 'CREATE_FAILED', 'DELETE_COMPLETE'].

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Cause 1: CreateStack with a template declaring an AWS::SSM::PatchBaseline with a unique Name; wait for CREATE_COMPLETE, note the resource's PhysicalResourceId, then call SSM DescribePatchBaselines filtered on that name. AWS returns the baseline; the emulator returns an empty list.
3. Cause 2: PutParameter a parameter name out of band, then CreateStack with validation disabled on a template whose AWS::SSM::Parameter declares that same name. Let the stack roll back and read DescribeStackEvents for that logical resource. AWS emits CREATE_IN_PROGRESS, CREATE_FAILED, DELETE_COMPLETE; the emulator inserts a DELETE_IN_PROGRESS before DELETE_COMPLETE.
