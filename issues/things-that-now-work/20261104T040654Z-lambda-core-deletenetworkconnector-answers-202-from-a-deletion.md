Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core.network-connector-delete-transitions-to-removed
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core.network-connector-delete-transitions-to-removed|_error#13|AssertionError|raise AssertionError(f"expected the delete of the removed connector to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# lambda-core: DeleteNetworkConnector answers 202 from a deletion tombstone for a connector whose deletion already completed, where AWS refuses with 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNetworkConnector

## Description
One net-new root cause on lambda-core's network-connector delete path (com.amazonaws.lambdacore#LambdaCoreApiService), grounded in the one failing lambda-core check that is neither ALREADY FILED (network-connector-dualstack-network-protocol-persists, blocked by the unimplemented EC2 AssociateVpcCidrBlock) nor OWNED (network-connector-create-nonexistent-subnet-rejected).

Cause 1 of 1 - once a connector's deletion has COMPLETED (GetNetworkConnector on it already answers ResourceNotFoundException), a further DeleteNetworkConnector on the same identifier must be refused, but the emulator serves a 202 success out of a tombstone it keeps for the deleted connector.

AWS: Smithy DeleteNetworkConnector (DELETE /2026-04-04/network-connectors/{Identifier}, success code 202, idempotent:true) models ResourceNotFoundException at httpError 404, documented "The specified network connector does not exist. Verify the identifier (ID, name, or ARN) and Region." Recorded behavior draws the line the operation's prose ("calling delete on a connector that is already deleting or has been deleted succeeds without error") leaves ambiguous: the idempotent 202 covers the window in which the connector still EXISTS in DELETING state, and once the record is gone - the same instant GetNetworkConnector starts answering ResourceNotFoundException - a repeat delete is refused with HTTP 404, error code ResourceNotFoundException and the message "Network connector not found: <connectorArn>". In that message "Network connector not found: " is fixed text and the variable part is the connector's unqualified ARN (arn:aws:lambda:<region>:<account>:network-connector:<Id>) - note it is the ARN form even when the request addressed the connector by its bare Id, which is what the check asserts (Identifier=<Id> on the request, expected message built from the created connector's Arn).

Emulator: the delete handler answers HTTP 202 with {Arn, Name, Id, State: "DELETING"} for an identifier whose deletion has already completed. services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/delete_network_connector.rs lines 190-219: when `settled_mut` finds no live record it consults `store.completed_deletion(&input.identifier)` and, if a tombstone is found, returns the success envelope with State = the state the record carried when its deletion completed; only an identifier with no tombstone falls through to `connector_not_found`. The tombstones live in `ComAmazonawsLambdacoreLambdacoreapiserviceState::deleted` (src/state.rs lines 330-351 and 448-454, bounded by MAX_DELETED_CONNECTORS = 1_000, src/state.rs line 141), so for the first 1_000 deletions of a run the removed id keeps answering a delete with success. The result is also self-inconsistent: GET /2026-04-04/network-connectors/<Id> answers 404 ResourceNotFoundException for exactly the identifier the DELETE accepts with 202.

Why a client gets this wrong: the 202 makes a mutating call on a resource that no longer exists look like work that was scheduled. "Delete, confirm gone, then assert the dead identifier is refused" - the ordinary way to prove a teardown really happened, and the shape of lambda-core's own documented error handling - passes against a dangling id, so an identifier bug (stale id cached, wrong Region, wrong account) is masked instead of surfaced: the client is told DELETING for something that cannot be deleted and will never transition again. A client that branches on ResourceNotFoundException to mean "already gone, stop polling" never takes that branch and keeps polling or retrying. And because the answer depends on whether a tombstone is still remembered, the SAME request is answered 202 or 404 depending only on how many other connectors were deleted since.

Fix: lambda-core's DeleteNetworkConnector handler together with the retention it reads. The idempotent 202 must be limited to a connector that still exists (state DELETING, i.e. the live record `settled_mut` returns); once the removal has been materialised and the record forgotten, the handler must raise ResourceNotFoundException (HTTP 404) with the message "Network connector not found: " + the connector's unqualified ARN, which is the same answer GetNetworkConnector already gives for that identifier - i.e. drop the `completed_deletion` tombstone arm (and with it the `deleted` tombstone store, or restrict it so it can no longer serve a success for a removed connector). This is a state/lifecycle and refusal-classification defect in the handler, not a serializer one.

Traces: lambda-core.network-connector-delete-transitions-to-removed. Code citations for the fixer: services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/delete_network_connector.rs lines 188-219 (the tombstone success arm) and services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/state.rs lines 130-141, 330-351, 448-454, 611-619 (MAX_DELETED_CONNECTORS, DeletedConnector, the `deleted` queue, `forget`). Relation to what is already filed: the open issue 20261030T090527Z-lambda-core-network-connector-refusals-diverge-from-aws.md carries, as its Cause 3, the DIFFERENT case of an identifier that never named a connector (202 instead of 404) and explicitly states that the post-deletion path is not its subject; the resolved issue 20261027T060836Z-lambda-core-deletenetworkconnector-is-not-idempotent-once-deletion.md asked for the tombstone that now serves this wrongful 202, and its own grounding check captured the repeat delete while the connector was still DELETING, which must keep answering 202 - so the fix here must narrow that tombstone to the DELETING window rather than remove idempotency. No open issue carries this replay key.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 23/29 passed (79.3%), lambda-core 23/26 (88%), bootstrap_signal false. Of lambda-core's three failures, one is ALREADY FILED (blocked by unimplemented EC2 AssociateVpcCidrBlock) and one is OWNED by 20261103T161149Z-lambda-core-a-created-network-connector-is-never.md; this check is the remaining one.

[mismatch] lambda-core.network-connector-delete-transitions-to-removed
  File "check_lambda_core_network_connector_delete_transitions_to_removed.py", line 65, in run
    redelete_code, redelete_message = _error(lambda: lc.delete_network_connector(Identifier=connector_id))
  File "check_lambda_core_network_connector_delete_transitions_to_removed.py", line 113, in _error
  AssertionError: expected the delete of the removed connector to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'Arn': 'arn:aws:lambda:us-west-2:000000000000:network-connector:nc-0000000000000001', 'Name': 'sim2real-lnc-<id>', 'Id': 'nc-0000000000000001', 'State': 'DELETING'}

The check reached this point having already passed its earlier legs: the connector was created, observed ACTIVE, deleted, and then polled with GetNetworkConnector until that call answered ResourceNotFoundException (its `_get_or_none` returned "REMOVED" and `assert final == "REMOVED"` held). So at the moment of the failing call the emulator itself already reported the connector as absent.

Expected (AWS, from the check's assertions): DeleteNetworkConnector on the removed identifier raises ClientError with Error.Code == 'ResourceNotFoundException' (HTTP 404) and Error.Message == 'Network connector not found: ' + the connector's unqualified ARN.
Actual (emulator): HTTP 202 with the success envelope above and State='DELETING'; no error at all, so neither the code nor the message assertion was reachable.
(The ARN, account id, generated connector id and name suffix above are per-run values and are not part of the requirement; the diverging facts are the HTTP status 202 where 404 is required and the absent ResourceNotFoundException/message.)

## Steps to Reproduce
1. git_clone, then run_behavioral_suite() and look at check lambda-core.network-connector-delete-transitions-to-removed.
2. Or by hand against the booted emulator (region us-west-2):
   a. Create a VPC, a subnet in it and a security group, plus an IAM role whose trust policy names lambda.amazonaws.com with AWSLambdaNetworkConnectorOperatorPolicy attached.
   b. POST /2026-04-04/network-connectors with Name=<unique>, Configuration={"VpcEgressConfiguration":{"SubnetIds":[<subnetId>],"SecurityGroupIds":[<sgId>],"NetworkProtocol":"IPv4","AssociatedComputeResourceTypes":["MicroVm"]}}, OperatorRole=<roleArn>. Note the returned Id and Arn.
   c. Poll GET /2026-04-04/network-connectors/<Id> until State is ACTIVE.
3. DELETE /2026-04-04/network-connectors/<Id>. Observe 202 with State='DELETING' (correct).
4. Poll GET /2026-04-04/network-connectors/<Id> until it answers HTTP 404 ResourceNotFoundException - the deletion has now completed and the emulator itself reports the connector as absent.
5. DELETE /2026-04-04/network-connectors/<Id> again. Expect HTTP 404 with error code ResourceNotFoundException and message 'Network connector not found: arn:aws:lambda:<region>:<account>:network-connector:<Id>' (fixed text plus the connector's unqualified ARN, in ARN form even though the request addressed it by bare Id). Observe HTTP 202 with {Arn, Name, Id, State: 'DELETING'} - the same identifier GET refuses as absent is accepted by DELETE as work scheduled.
6. Contrast with the case that must keep succeeding: issue DELETE twice in quick succession, i.e. the second one while the connector still exists in DELETING state (before step 4's 404 appears). That repeat must still answer 202 - only the post-removal repeat changes.
