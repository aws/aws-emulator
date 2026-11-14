Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.create-workspace-instance-duplicate-client-token-conflicting-params-rejected workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition workspaces-instances.create-workspace-instance-state-transition-to-allocated
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances.create-workspace-instance-duplicate-client-token-conflicting-params-rejected|run#12|AssertionError|assert conflicting_message == "The client token provided does not match the request", (
Issue-Key: workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition|_error_detail#13|AssertionError|raise AssertionError(f"expected ClientError, but the call SUCCEEDED: {result!r}")
Issue-Key: workspaces-instances|DescribeInstances|InvalidInstanceID.Malformed
Issue-Commit: ee9893ed79b0abeb742411b16639d26129234894

# CreateWorkspaceInstance: 3 divergences — wrong ConflictException message for a reused ClientToken, a nonexistent AMI accepted with 200, and a reported EC2ManagedInstance.InstanceId that resolves nowhere

> _Note: this issue was identified against the code base at commit `ee9893ed79b0abeb742411b16639d26129234894`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaceInstance

## Description
Three independent root causes in the CreateWorkspaceInstance path of the workspaces-instances emulator, all grounded in failing behavioral checks. A fixer must close all three before submitting.

Cause 1 of 3: the ConflictException a reused ClientToken is refused with carries the wrong Message, and an identical-parameter reuse is not refused at all.
AWS: Smithy — CreateWorkspaceInstance is @idempotent, input member ClientToken targets ClientToken (@idempotencyToken, @sensitive, @length 1..64, @pattern ^[\x20-\x7E]+$), and the operation models ConflictException @httpError 409. The recorded behavior this check asserts is that a ClientToken already spent on a CreateWorkspaceInstance is rejected with code ConflictException and the message "The client token provided does not match the request" — whether the replay's parameters differ from the first call's or are byte-identical, i.e. the token is single-use here and NOT a replay key — and that neither rejected replay creates a second WorkSpace Instance (ListWorkspaceInstances still shows exactly one). The message is fixed text in full: it names no id, token or parameter, so there are no variable parts to substitute.
Emulator: the conflicting-parameters replay is refused with code ConflictException (correct) but Message "ClientToken was already used to create a resource with different parameters" (services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/state.rs, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE / idempotent_parameter_mismatch). The check aborts on that assertion. The same state.rs also shows the second half of the cause: resolve_client_token compares a request_fingerprint and answers TokenOutcome::Replay for a byte-identical retry, so a reuse with identical parameters is answered 200 with the first instance's WorkspaceInstanceId instead of ConflictException.
Why: a client that branches on the ConflictException message (or logs/surfaces it) sees text AWS never sends, and a client that re-submits a create with a token it has already spent gets a success and the original id from the emulator where AWS refuses the call — so retry/duplicate-submission logic that AWS forces down the error path takes the success path here.
Fix: the ClientToken resolution and the ConflictException construction for this operation — ops/create_workspace_instance.rs together with resolve_client_token / IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE in src/state.rs: use AWS's fixed message, and treat a spent CreateWorkspaceInstance token as single-use rather than a replay key.
Traces: workspaces-instances.create-workspace-instance-duplicate-client-token-conflicting-params-rejected.

Cause 2 of 3: a well-formed but nonexistent ImageId is accepted and an instance is created.
AWS: Smithy — ManagedInstance (ManagedInstanceRequest) is @required and its ImageId names an AMI that must exist in the caller's account/Region; the operation models ValidationException @httpError 400. Recorded behavior: CreateWorkspaceInstance with a well-formed ami- id that does not exist fails at request time with code ValidationException and the message "Invalid AMI ID provided" — fixed text, no variable part (the offending id is not echoed) — and adds no WorkSpace Instance (the ListWorkspaceInstances id set is unchanged across the rejected call).
Emulator: the call SUCCEEDS with HTTP 200 and returns a WorkspaceInstanceId (observed: a wsinst- id minted for the bogus AMI), so a resource is created for an AMI that does not exist.
Why: a client relying on request-time rejection of a bad AMI sees a successful create, then has a WorkSpace Instance that can never allocate; the failure is silently deferred (or never surfaces at all) instead of arriving as ValidationException at the call, and quota/state is consumed by a resource AWS would never have created.
Fix: input validation of ManagedInstance.ImageId in the CreateWorkspaceInstance handler (ops/create_workspace_instance.rs) — resolve the AMI against the image state and refuse with the modeled ValidationException before minting anything.
Traces: workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition.

Cause 3 of 3: the EC2ManagedInstance.InstanceId a successful create reports resolves to no EC2 instance, and is not even accepted by the EC2 endpoint as an instance id.
AWS: Smithy — GetWorkspaceInstanceResponse$EC2ManagedInstance -> EC2ManagedInstance{InstanceId} ("Unique identifier of the managed EC2 instance"), and ManagedInstanceRequest mirrors EC2 RunInstances input (ImageId, InstanceType, SubnetId, SecurityGroupIds, KeyName, IamInstanceProfile, BlockDeviceMappings, Placement, UserData, TagSpecifications) — the launch happens in the caller's own account, so after the instance reaches ProvisionState ALLOCATED, ec2:DescribeInstances with the reported InstanceId resolves a reservation holding that instance in state pending or running.
Emulator: the WorkSpace Instance reaches ALLOCATED and reports an EC2ManagedInstance.InstanceId minted purely inside this crate (src/state.rs mint_ids: format!("i-{n:017x}")); no instance is created in EC2 state. ec2:DescribeInstances(InstanceIds=[that id]) is refused — observed code InvalidInstanceID.Malformed with message 'Invalid id: "<the reported i- id>"' (the quoted id is the value from the request; the rest is fixed text) — so the id is rejected outright rather than resolving, and the check cannot read the backing instance's State.Name. Note both halves: the backing instance is never propagated into EC2 state at all, AND the id this service mints is not accepted by the EC2 endpoint's instance-id validation even though it is i- plus 17 hex digits, so a client cannot even reach a NotFound.
Why: the only reason the API returns this id is so the caller can inspect and manage the underlying EC2 instance; against the emulator every such call fails, so cross-service workflows (describe, tag, stop, attach) that AWS supports are impossible.
Related open issue: 20261024T122725Z-workspaces-instances-createworkspaceinstance-event-propagation.md describes the same non-propagation (it was filed against commit f0788135 and does not carry this check's replay key, and the refusal it predicts is InvalidInstanceID.NotFound while the emulator now answers InvalidInstanceID.Malformed). This issue carries the behavioral check to re-replay.
Fix: the CreateWorkspaceInstance handler and its state (ops/create_workspace_instance.rs, src/state.rs mint_ids) — the launch must create the instance in the EC2 service's state from the supplied ManagedInstance, and the reported id must be an id the EC2 endpoint accepts and resolves (with DeleteWorkspaceInstance terminating it).
Traces: workspaces-instances.create-workspace-instance-state-transition-to-allocated.

## Evidence
Behavioral suite: workspaces-instances 10/25 passed (40%), 15 failed; 12 of the failures are OWNED by 20261029T190356Z-workspaces-instances-6-fidelity-divergences-across-createvolume-listregions.md and skipped. The three remaining failures are the three causes above.

Cause 1 — check create-workspace-instance-duplicate-client-token-conflicting-params-rejected [mismatch]:
  assert conflicting_message == "The client token provided does not match the request"
  AssertionError: ClientToken was already used to create a resource with different parameters
Expected: code ConflictException (HTTP 409) with Message "The client token provided does not match the request", for BOTH a differing-parameter and a byte-identical reuse, with no second resource created.
Actual: ConflictException with Message "ClientToken was already used to create a resource with different parameters"; and per src/state.rs resolve_client_token, a byte-identical reuse returns TokenOutcome::Replay, i.e. HTTP 200 with the original WorkspaceInstanceId instead of a refusal.

Cause 2 — check create-workspace-instance-invalid-ami-error-allocating-transition [mismatch]:
  AssertionError: expected ClientError, but the call SUCCEEDED: {'WorkspaceInstanceId': 'wsinst-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Expected: code ValidationException (HTTP 400), Message "Invalid AMI ID provided", and the ListWorkspaceInstances id set unchanged.
Actual: HTTP 200 and a newly minted WorkspaceInstanceId for ManagedInstance={"ImageId": a well-formed nonexistent ami- id, "InstanceType":"m5.large","SubnetId": <a subnet of the signed Region>}.

Cause 3 — check create-workspace-instance-state-transition-to-allocated [error]:
  botocore.exceptions.ClientError: An error occurred (InvalidInstanceID.Malformed) when calling the DescribeInstances operation: Invalid id: "i-00000000000000001"
Expected: ec2:DescribeInstances(InstanceIds=[final["EC2ManagedInstance"]["InstanceId"]]) returns a reservation whose instance State.Name is pending or running.
Actual: the call is refused with InvalidInstanceID.Malformed; no EC2 instance was ever created for the WorkSpace Instance (src/state.rs mint_ids fabricates the i- id locally).

## Steps to Reproduce
Boot the multi-service emulator so both workspaces-instances and ec2 are mounted, create the workspaces-instances service-linked role via iam:CreateServiceLinkedRole, and pick a subnet of the signed Region.

Cause 1:
1. CreateWorkspaceInstance with ClientToken=<T> and ManagedInstance={"ImageId":<a real AMI>,"InstanceType":"m5.large","SubnetId":<subnet>}; keep WorkspaceInstanceId.
2. CreateWorkspaceInstance again with the SAME ClientToken=<T> but InstanceType="m5.xlarge" — the emulator answers ConflictException with Message "ClientToken was already used to create a resource with different parameters"; expected Message is "The client token provided does not match the request".
3. CreateWorkspaceInstance a third time with ClientToken=<T> and parameters byte-identical to step 1 — the emulator answers HTTP 200 replaying the first WorkspaceInstanceId; expected is ConflictException with the same message as step 2.
4. ListWorkspaceInstances(ProvisionStates=["ALLOCATING","ALLOCATED"]) — exactly one record for the id must exist after each rejected replay.

Cause 2:
1. CreateWorkspaceInstance with ManagedInstance={"ImageId":"ami-0000000000deadbee" (well-formed, nonexistent),"InstanceType":"m5.large","SubnetId":<subnet>}.
2. The emulator returns HTTP 200 with a WorkspaceInstanceId; expected ValidationException / "Invalid AMI ID provided" and no new entry in ListWorkspaceInstances.

Cause 3:
1. CreateWorkspaceInstance with a real AMI and <subnet>; poll GetWorkspaceInstance until ProvisionState is ALLOCATED.
2. Read EC2ManagedInstance.InstanceId from that response.
3. Call ec2:DescribeInstances(InstanceIds=[that id]) — the emulator refuses with InvalidInstanceID.Malformed; expected a reservation whose instance State.Name is pending or running.
4. DeleteWorkspaceInstance, then ec2:DescribeInstances again — nothing was ever launched, so nothing is terminated.
