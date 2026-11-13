Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected appstream-asvpce-traj-endpoint-deleted-under-stack appstream-asvpce-traj-reference-stack-on-pending-endpoint
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|DescribeStacks|2xx->4xx|$.Stacks
Issue-Key: appstream|DescribeStacks|2xx->4xx|status
Issue-Key: appstream|DeleteStack|2xx->4xx|status
Issue-Commit: 9fd63ccf820c1488063cdb300cc0894d6b49ad4b

# appstream read-after-create on a stack answers ResourceNotFoundException: DescribeStacks and DeleteStack cannot see a stack the trajectory created

> _Note: this issue was identified against the code base at commit `9fd63ccf820c1488063cdb300cc0894d6b49ad4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two net-new causes, both on the SAME fault: in a trajectory where AWS has an ACTIVE stack by this point, the emulator's stack registry holds no record for that name, so every stack-scoped read or mutation that follows is refused as not-found instead of serving the stack. This is NOT the already-open issue 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md: that one is about the STATUS and wording of the refusal when the stack genuinely does not exist (expected 400 -> got 404 / got 200). Here the stack DOES exist on AWS (expected 200) and the emulator refuses it at all. It is also not the projection issue 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md, which is about members missing from a stack the emulator DOES return.

Cause 1 of 2: DescribeStacks naming an existing stack answers a not-found refusal instead of the stack.
AWS: DescribeStacks returns HTTP 200 with `Stacks` holding one `Stack` per requested name that exists (Smithy: appstream#DescribeStacks, input {Names: StringList, NextToken}, output {Stacks: StackList, NextToken}; its only modeled error is ResourceNotFoundException, which applies to a name that names no stack). In the recording the stack created earlier in the trajectory is reported with State ACTIVE, its ARN, CreatedTime, the AccessEndpoints that were supplied (EndpointType STREAMING with the account's VPC-endpoint id), and the server-side defaults (ApplicationSettings.Enabled false, StreamingExperienceSettings.PreferredProtocol TCP, the eight default UserSettings).
Emulator: answers HTTP 404 with ResourceNotFoundException and NO `Stacks` member (replay detail: "status: expected 200, got 404" / "$.Stacks: missing"). The refusal message is the handler's not-found sentence, whose fixed text is "The stack <stack-name> for account <account-id> does not exist." with the stack name taken from the request's `Names` entry and the account id from the caller's account - correct wording for a stack that is absent, but the stack is not supposed to be absent here. The handler takes this branch purely because `state.stacks` has no entry under the requested name (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/describe_stacks.rs: `if !state.stacks.contains_key(name)`), so the record a successful CreateStack must have left was never inserted or did not survive to this step.
Hypothesis (mechanism, not observed directly): the CreateStack that establishes the stack in these three trajectories never lands on the emulator - the fixture's successful create produces no CreateStack divergence cluster of its own, unlike the trajectories where the stack IS found - so the stack-creating step is lost ahead of this read; the same trajectories also hit the unimplemented ec2 DescribeAccountAttributes (501, filed separately). The requirement being broken either way is read-after-create: a stack the service accepted must be reported by DescribeStacks until it is deleted.
Why: a client that creates a stack and then polls DescribeStacks for State=ACTIVE - the standard provisioning wait - never sees the stack and treats provisioning as failed; a name-collision pre-check reads "absent" for a stack that exists, and an HTTP-level client sees 404 and treats the call as a routing failure rather than a service answer.
Fix: the stack registry write/read path - CreateStack's insertion into `state.stacks` and the key DescribeStacks resolves against (services/service-com-amazonaws-appstream-photonadminproxyservice/src/state.rs, ops/create_stack.rs, ops/describe_stacks.rs); the describe handler must only reach its not-found branch for a name no accepted create ever registered.
Traces: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected#task#10 (representative; cluster also covers appstream-asvpce-traj-endpoint-deleted-under-stack#task#9 and appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#7).

Cause 2 of 2: DeleteStack of an existing stack answers a not-found refusal instead of deleting it.
AWS: DeleteStack of an existing, unassociated stack returns HTTP 200 with an empty result body (Smithy: appstream#DeleteStack, input {Name required, len>=1}, output {} ; errors ConcurrentModificationException/OperationNotPermittedException/ResourceInUseException 400 and ResourceNotFoundException). Deleting a stack that still references a VPC endpoint in a pending state is permitted - the recording shows the delete succeeding.
Emulator: answers HTTP 404 with ResourceNotFoundException (replay detail: "status: expected 200, got 404") carrying the same fixed sentence "The stack <stack-name> for account <account-id> does not exist." (name from the request's `Name`, account id from the caller's account). services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/delete_stack.rs refuses before any removal because `state.stacks` has no entry under that name - the same missing registry record as Cause 1, reached through the delete path.
Why: teardown is unreliable and non-idempotent in the wrong direction: a test or client cleaning up its own stack is told the stack never existed, so it cannot distinguish a leaked resource from a completed delete, and a 404 drives retry/branch logic differently from AWS's 200.
Fix: same stack registry write/read path, exercised from the DeleteStack handler (ops/delete_stack.rs lookup against state.rs `stacks`).
Traces: appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, 18 fixtures replayed, 162 steps, 34 passed, 75 mismatched, 53 blocked, score 21.0%, 15 divergence clusters). Both causes below are clusters with EMPTY owned_by and EMPTY stub_path, i.e. not covered by any filed issue.

Cause 1 - DescribeStacks, fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected, step task#10:
  expected_status 200, actual_status 404.
  detail: "status: expected 200, got 404" and "$.Stacks: missing (expected [{'AccessEndpoints': [{'EndpointType': 'STREAMING', 'VpceId': '{{vpce_id}}'}], 'ApplicationSettings': {'Enabled': False}, 'Arn': 'arn:aws:appstream:us-west-2:000000000000:stack/sim2real-probe-asvpce-api-04e3bced', 'CreatedTime': 1791536109.669, 'Name': 'sim2real-probe-asvpce-api-04e3bced', 'StackErrors': [], 'State': 'ACTIVE', 'StreamingExperienceSettings': {'PreferredProtocol': 'TCP'}, 'UserSettings': [... eight default UserSettings ...]}], got None)".
  The stack name, ARN account/region and CreatedTime above are this run's values filling the recorded shape; the requirement is that an existing stack is returned at all, with State ACTIVE and the supplied AccessEndpoints.
  divergence_key service=appstream|DescribeStacks|status=200->404|keys=$.Stacks,status; affected_count 3 (appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected#task#10, appstream-asvpce-traj-endpoint-deleted-under-stack#task#9, appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#7); owned_by [] ; stub_path "".

Cause 2 - DeleteStack, fixture appstream-asvpce-traj-reference-stack-on-pending-endpoint, step task#8:
  expected_status 200, actual_status 404. detail: "status: expected 200, got 404".
  divergence_key service=appstream|DeleteStack|status=200->404|keys=status; affected_count 1; owned_by [] ; stub_path "".

Contrast inside the same corpus (why this is a state defect, not a status-mapping defect): in appstream-asvpce-traj-duplicate-streaming-entries-rejected and appstream-asvpce-traj-invented-vpce-id-rejected the emulator DOES return the stack (HTTP 200) for the same operations - those clusters are body-projection mismatches owned by 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md. The 400->404 not-found clusters are owned by 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md. Neither covers an expected-200 step answered 404.

Code evidence: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/describe_stacks.rs lines 145-163 (ResourceNotFoundException when `state.stacks` lacks the name); src/ops/delete_stack.rs lines 143-153 (same check before removal); src/ops/create_stack.rs lines 196-224 (the only insertion into `state.stacks`, taken after a ResourceAlreadyExists / LimitExceeded check).

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): DescribeStacks output {Stacks: StackList, NextToken}, errors [ResourceNotFoundException]; DeleteStack input {Name required len>=1}, output {} , errors [ConcurrentModificationException 400, OperationNotPermittedException 400, ResourceInUseException 400, ResourceNotFoundException].

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected and stop at step task#10 - a DescribeStacks naming the stack the trajectory created earlier. Observe HTTP 404 with ResourceNotFoundException ("The stack <name> for account <account-id> does not exist.") where the fixture expects HTTP 200 with Stacks[0].State ACTIVE. The same cluster reproduces at appstream-asvpce-traj-endpoint-deleted-under-stack#task#9 and appstream-asvpce-traj-reference-stack-on-pending-endpoint#task#7.
4. Cause 2: replay fixture appstream-asvpce-traj-reference-stack-on-pending-endpoint and stop at step task#8 - a DeleteStack of that same existing stack. Observe HTTP 404 ResourceNotFoundException where the fixture expects HTTP 200 with an empty body.
5. Cross-check that this is read-after-create and not the refusal-status defect: a DescribeStacks for a name that was never created in the same corpus (appstream-asvpce-traj-stack-lookup-and-delete-lag#task#2) is the 400->404 cluster already owned by 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md.
