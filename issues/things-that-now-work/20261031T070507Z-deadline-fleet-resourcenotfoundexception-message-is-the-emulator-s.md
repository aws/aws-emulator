Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.create-fleet-idempotent-client-token-concurrent
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline|GetFleet|ResourceNotFoundException
Issue-Commit: 7846fec8a86647e77da10e6415de80579a2805a4

# deadline fleet ResourceNotFoundException message is the emulator's own wording ("The fleet <id> can't be found.") instead of AWS's "Resource of type fleet with id <id> does not exist."

> _Note: this issue was identified against the code base at commit `7846fec8a86647e77da10e6415de80579a2805a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFleet

## Description
Cause 1 of 1 — the fleet not-found refusal message is reworded, on the fleet-addressing operations (GetFleet and UpdateFleet after a DeleteFleet).

Related open issue: 20261029T172830Z-deadline-resourcenotfoundexception-message-is-the-emulator-s-own.md reports the same wording defect reached through GetFarm (and names farm/queue/job call sites). It does NOT carry this check's replay key, and the failing check here is grounded in the FLEET path (GetFleet/UpdateFleet on a deleted fleet), so this is filed separately and should be closed together with it; a fixer who changes only the farm wording will not clear this check.

AWS: Smithy (services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json): GetFleet is GET /2023-10-12/farms/{farmId}/fleets/{fleetId} with fleetId @pattern ^fleet-[0-9a-f]{32}$, UpdateFleet is PATCH on the same URI, and both model ResourceNotFoundException (httpError 404); the model documents the exception only as "The requested resource can't be found." Recorded behavior: addressing a fleet that was just deleted answers HTTP 404 ResourceNotFoundException with the message 'Resource of type fleet with id <fleetId> does not exist.' — the fixed text is 'Resource of type <resourceType> with id <resourceId> does not exist.' (trailing period included), where <resourceType> is the resource kind the path addresses ('fleet' here, matching the exception's own resourceType member) and <resourceId> is the id taken verbatim from the request. Nothing in the message is per-run except the id and type drawn from the request/state.

Emulator: the same calls answer 404 ResourceNotFoundException with "The fleet <fleetId> can't be found." — the template "The {resource_type} {resource_id} can't be found." rendered by crate::state::not_found. The HTTP status (404), the error code (ResourceNotFoundException) and the resourceId/resourceType members are correct; only the message text diverges. Because that helper is single-homed, every resource type this crate refuses (farm, queue, fleet, job, budget, limit, storageProfile — see the RESOURCE_TYPE_* constants) carries the same wrong sentence.

Why: an error message is part of the contract. A client, test fixture, log scraper or support tool that matches AWS's 'Resource of type X with id Y does not exist.' finds no match here, so deleted-resource handling that works against AWS falls through to a generic error branch — and a delete-then-read guard that asserts on the message fails against the emulator while passing against AWS.

Fix: the crate's single-homed refusal wording — crate::state::not_found in services/service-com-amazonaws-deadline-deadline/src/state.rs (change the template, not the call sites), so the fleet 404s raised from src/ops/get_fleet.rs and src/ops/update_fleet.rs, and every other RESOURCE_TYPE_* 404, pick up AWS's sentence at once.

Traces: deadline.create-fleet-idempotent-client-token-concurrent (its idempotency-under-contention body passes — 5 concurrent CreateFleet calls sharing one clientToken do collapse to one fleetId, ListFleets shows one fleet, DeleteFleet succeeds — and the check fails only on the post-delete GetFleet message assertion; the identical UpdateFleet message assertion immediately after is unreached).

## Evidence
Suite: [mismatch] deadline.create-fleet-idempotent-client-token-concurrent
  File "check_deadline_create_fleet_idempotent_client_token_concurrent.py", line 91, in run
    assert error.get("Message") == expected_message
  AssertionError: expected message 'Resource of type fleet with id fleet-6b80d243a81ce23fff2cad02d23eea4c does not exist.', got "The fleet fleet-6b80d243a81ce23fff2cad02d23eea4c can't be found."

(The fleetId shown is the per-run id the check created and deleted; the requirement is the message TEMPLATE — fixed text 'Resource of type ' + <resourceType> + ' with id ' + <resourceId> + ' does not exist.' — not that id.)

The check's Code assertion passes: error Code is already ResourceNotFoundException and the status is already 404.

Code evidence: services/service-com-amazonaws-deadline-deadline/src/ops/get_fleet.rs:192-197 raises
  GetFleetErrorModeled::ResourceNotFoundException(crate::state::not_found(crate::state::RESOURCE_TYPE_FLEET, &input.fleet_id))
and crate::state::not_found (src/state.rs) renders message: format!("The {resource_type} {resource_id} can't be found."). RESOURCE_TYPE_FLEET = "fleet" (src/state.rs:91).

Model evidence: GetFleet / UpdateFleet / DeleteFleet each list ResourceNotFoundException with httpError 404 (smithy_query query="operation", operation="GetFleet,UpdateFleet,DeleteFleet").

## Steps to Reproduce
1. deadline.create_farm(displayName=...) -> farmId.
2. deadline.create_fleet(farmId=farmId, clientToken=<tok>, displayName=..., roleArn=..., maxWorkerCount=1, configuration=<customerManaged>) -> fleetId (the idempotent/concurrent variant is not required to reproduce the message; one CreateFleet suffices).
3. deadline.delete_fleet(farmId=farmId, fleetId=fleetId) -> succeeds.
4. deadline.get_fleet(farmId=farmId, fleetId=fleetId).
   Observed: HTTP 404, Code ResourceNotFoundException, Message "The fleet <fleetId> can't be found."
   Expected: HTTP 404, Code ResourceNotFoundException, Message 'Resource of type fleet with id <fleetId> does not exist.'
5. deadline.update_fleet(farmId=farmId, fleetId=fleetId, displayName="anything") shows the same reworded message.
6. The same divergence appears for every other resource type this crate refuses (farm, queue, job, budget, limit, storageProfile), since one helper renders them all.
