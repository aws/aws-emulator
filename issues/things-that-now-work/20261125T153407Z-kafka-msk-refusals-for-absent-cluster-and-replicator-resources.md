Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-cluster-mutations-notfound-2
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-cluster-mutations-notfound-2 kafka-operate-on-nonexistent-replicator kafka-replicator-crud-notfound-error-paths
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$.message
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|status
Issue-Key: kafka|DescribeReplicator|4xx->4xx|$.message
Issue-Key: kafka|DeleteReplicator|4xx->4xx|$.message
Issue-Commit: e9279a32366be341735568d509d6722efe13b076

# MSK refusals for absent cluster and replicator resources diverge: UpdateConnectivity refusal class and the replicator not-found message (3 causes)

> _Note: this issue was identified against the code base at commit `e9279a32366be341735568d509d6722efe13b076`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnectivity

## Description
THREE distinct net-new root causes in the kafka (Amazon MSK) service, all on the refusal the control plane serves for a well-formed ARN that names no live resource. Every other divergence this wire replay reported for kafka was marked OWNED by an open issue or blocked by a stub and is skipped; these three had no owner. All three must be closed before this issue is submitted.

=== Cause 1 of 3: UpdateConnectivity on a well-formed cluster ARN that names no registered cluster is refused 404 NotFoundException where MSK refuses it 403 AccessDeniedException. (Fixture kafka-cluster-mutations-notfound-2, step task#2.)
AWS: Smithy com.amazonaws.kafka#UpdateConnectivity (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json) - PUT /v1/clusters/{ClusterArn}/connectivity, ClusterArn @required @httpLabel, CurrentVersion @required (jsonName currentVersion), ConnectivityInfo (jsonName connectivityInfo); success 200. Its modeled error set is exactly BadRequestException 400, ForbiddenException 403, InternalServerErrorException 500, NotFoundException 404, ServiceUnavailableException 503, UnauthorizedException 401. Which refusal an unregistered cluster ARN draws on this operation is recorded behavior only, and this fixture records it as the resource-policy authorization denial, NOT the not-found: HTTP 403 with header x-amzn-errortype: AccessDeniedException and body message of the form "User: <caller principal ARN> is not authorized to perform: kafka:UpdateConnectivity on resource: <the cluster ARN submitted in the path> because no resource-based policy allows the kafka:UpdateConnectivity action". Message anatomy: "User: ", " is not authorized to perform: kafka:UpdateConnectivity on resource: " and " because no resource-based policy allows the kafka:UpdateConnectivity action" are FIXED text; the only variable parts are the caller's own principal ARN (taken from the request's credentials) and the cluster ARN echoed from the request path. The account id, user name, region, cluster name and uuid in the recorded instance are per-run values and are not part of the requirement.
Emulator: answers HTTP 404 with x-amzn-errortype: NotFoundException and message "The requested resource doesn’t exist." (U+2019 apostrophe) - the authorization decision for an unregistered cluster never happens, so the connectivity mutation reports the resource as merely absent.
Why: the status class and error code are what a client branches on. A caller enabling public access / multi-VPC connectivity receives "this cluster is gone" (404) where AWS reports a permissions problem (403 AccessDenied). Reconcilers and IaC providers treat 404 as "recreate / stop polling" and silently diverge from AWS, where the same call tells an operator to fix a resource-based policy; and generic 403-retry/credential-refresh paths are never taken.
Fix: the refusal ordering and refusal class in this operation's handler - services/service-com-amazonaws-kafka-kafka/src/ops/update_connectivity.rs - and the shared helpers it calls in crate::state::authorization (authorize_registered_cluster / no_resource_policy_for_cluster). CONFLICT TO RECONCILE, do not simply flip the gate: open behavioral issue 20261124T122109Z-kafka-updateconnectivity-on-a-well-formed-cluster-arn-this.md asks for the OPPOSITE on this same operation (404 NotFoundException with invalidParameter 'clusterArn' instead of the 403 denial) for behavioral check kafka.update-connectivity-server-enforced, and the emulator currently serves what that issue asked for, which is why this wire step now diverges. Hypothesis (neither the model nor these two recordings state it): the two recordings differ in some property of the caller or of the cluster ARN (for example whether the ARN's account/region is the caller's own, or whether a cluster of that name was ever registered in this region) and MSK gates on resource-policy authorization before existence only for one of those classes; whoever fixes this must find the axis that satisfies both recordings rather than inverting the single ordering. Open issue 20261118T222834Z-kafka-msk-control-plane-answers-diverge-published-version-matrix.md covers the same inversion on DescribeClusterOperation (a different operation and handler) and carries no replay key for this step.
Traces: kafka-cluster-mutations-notfound-2#task#2 (expected status 403 / x-amzn-errortype AccessDeniedException, got 404 / NotFoundException).

=== Cause 2 of 3: DescribeReplicator's not-found refusal drops MSK's "Invalid parameter replicatorArn: " attribution. (Fixture kafka-operate-on-nonexistent-replicator, step task#1; same cluster also covers kafka-replicator-crud-notfound-error-paths#task#2.)
AWS: Smithy com.amazonaws.kafka#DescribeReplicator - GET /replication/v1/replicators/{ReplicatorArn}, ReplicatorArn @required @httpLabel, binds NotFoundException (httpError 404, members invalidParameter/message). Recorded behavior: for a replicator ARN that was never issued, MSK answers HTTP 404 NotFoundException with message "Invalid parameter replicatorArn: The requested resource doesn’t exist." Message anatomy: the whole string is FIXED text except the parameter name, which is the modeled wire (lower-camel) name of the offending input member - replicatorArn for this resource family; the apostrophe in "doesn’t" is U+2019, not ASCII. The replicator family is deliberately different from the other MSK resources, whose not-found message is the unprefixed "The requested resource doesn’t exist." and whose steps in these same fixtures pass.
Emulator: right status (404) and right error code (NotFoundException), but message "The requested resource doesn’t exist." - the replicator path reuses the generic not-found text and never prepends the "Invalid parameter <wire member>: " clause.
Why: a client or operator matching the refusal text to learn WHICH parameter failed to resolve gets no attribution, and a parity harness comparing refusal messages flags every replicator miss.
Fix: the not-found branch of services/service-com-amazonaws-kafka-kafka/src/ops/describe_replicator.rs and the shared replicator-ARN resolution it calls - emit the replicator-attributed fixed message for this resource family. Open behavioral issue 20261123T053238Z-kafka-msk-control-plane-divergences-replicator-not-found-message.md describes the same text fault from behavioral checks but carries NO wire replay key for this fixture step, so this step is not covered by it; 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md covers the invalidParameter member name, not the message text.
Traces: kafka-operate-on-nonexistent-replicator#task#1, kafka-replicator-crud-notfound-error-paths#task#2.

=== Cause 3 of 3: DeleteReplicator's not-found refusal drops the same "Invalid parameter replicatorArn: " attribution. (Fixture kafka-operate-on-nonexistent-replicator, step task#3; same cluster also covers kafka-replicator-crud-notfound-error-paths#task#4.)
AWS: Smithy com.amazonaws.kafka#DeleteReplicator - DELETE /replication/v1/replicators/{ReplicatorArn}, ReplicatorArn @required @httpLabel, CurrentVersion @httpQuery currentVersion, binds NotFoundException 404. Recorded behavior: a replicator ARN that was never issued is refused 404 NotFoundException with message "Invalid parameter replicatorArn: The requested resource doesn’t exist." - same anatomy as cause 2 (all fixed text apart from the wire member name replicatorArn).
Emulator: 404 NotFoundException with the generic "The requested resource doesn’t exist."
Why: same as cause 2 - the delete path is the one a teardown/idempotency loop reads, and it cannot tell from the message that it was the replicatorArn that did not resolve.
Fix: the not-found branch of services/service-com-amazonaws-kafka-kafka/src/ops/delete_replicator.rs; it is a separate handler from DescribeReplicator, so fixing cause 2 alone leaves this step diverging unless the shared replicator-ARN resolution helper is the seam both use.
Traces: kafka-operate-on-nonexistent-replicator#task#3, kafka-replicator-crud-notfound-error-paths#task#4.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: 51 fixtures replayed, 186 steps, 172 passed, 14 mismatched, 0 errored, 0 blocked (score 92.5%), emulator_up=true. 11 divergence clusters, 8 of them marked owned_by an open issue; the 3 below are the only clusters with no owner and no stub.

Cause 1 - kafka / UpdateConnectivity / kafka-cluster-mutations-notfound-2 # task#2 (expected_status 403, actual_status 404):
  status: expected 403, got 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'NotFoundException'
  $.message: expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: kafka:UpdateConnectivity on resource: arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-probe-kafka-missing-connectivity-90c735fd/aaaaaaaa-bbbb-cccc-dddd-aaaabbbbcccc-1 because no resource-based policy allows the kafka:UpdateConnectivity action', got 'The requested resource doesn’t exist.'
  (The account id 000000000000, user name 'emulator', region, cluster name and uuid in that expected message are per-run/per-recording values filling the template; the divergence is the status class, the error code and the message class.)

Cause 2 - kafka / DescribeReplicator / kafka-operate-on-nonexistent-replicator # task#1 (status 404 on both sides, affected_count 2, also kafka-replicator-crud-notfound-error-paths#task#2):
  $.message: expected 'Invalid parameter replicatorArn: The requested resource doesn’t exist.', got 'The requested resource doesn’t exist.'

Cause 3 - kafka / DeleteReplicator / kafka-operate-on-nonexistent-replicator # task#3 (status 404 on both sides, affected_count 2, also kafka-replicator-crud-notfound-error-paths#task#4):
  $.message: expected 'Invalid parameter replicatorArn: The requested resource doesn’t exist.', got 'The requested resource doesn’t exist.'

Model evidence (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): UpdateConnectivity PUT /v1/clusters/{ClusterArn}/connectivity, errors = BadRequestException 400 / ForbiddenException 403 / InternalServerErrorException 500 / NotFoundException 404 / ServiceUnavailableException 503 / UnauthorizedException 401 (no AccessDeniedException shape is modeled, so the recorded 403 denial is the IAM-layer refusal, recorded behavior only). DescribeReplicator GET /replication/v1/replicators/{ReplicatorArn} and DeleteReplicator DELETE on the same URI both take ReplicatorArn as @required @httpLabel and bind NotFoundException 404; NotFoundException = structure{invalidParameter (jsonName invalidParameter), message (jsonName message)}, httpError 404.

Source corroboration: services/service-com-amazonaws-kafka-kafka/src/ops/update_connectivity.rs (336 lines) holds the authorization/existence ordering for cause 1; services/service-com-amazonaws-kafka-kafka/src/ops/describe_replicator.rs (261 lines) and .../delete_replicator.rs (231 lines) hold the two replicator not-found branches.

## Steps to Reproduce
Boot the emulator built from this checkout and replay wire corpus simparity-wire-aws-v1, or reproduce each cause directly against the emulator's kafka endpoint:

Cause 1 (fixture kafka-cluster-mutations-notfound-2, step task#2): build a well-formed MSK cluster ARN that no cluster was ever created for, e.g. arn:aws:kafka:<region>:<account>:cluster/<never-created-name>/<uuid>-1, then call
  aws kafka update-connectivity --cluster-arn <that ARN> --current-version 1 --connectivity-info '{"PublicAccess":{"Type":"SERVICE_PROVIDED_EIPS"}}'
  (PUT /v1/clusters/{ClusterArn}/connectivity)
Expect HTTP 403, x-amzn-errortype: AccessDeniedException, message "User: <caller principal ARN> is not authorized to perform: kafka:UpdateConnectivity on resource: <the submitted cluster ARN> because no resource-based policy allows the kafka:UpdateConnectivity action". Observe HTTP 404, x-amzn-errortype: NotFoundException, message "The requested resource doesn’t exist." Before changing the ordering, read open issue 20261124T122109Z-kafka-updateconnectivity-on-a-well-formed-cluster-arn-this.md, which asks for the opposite answer on this operation, and satisfy both recordings.

Cause 2 (fixture kafka-operate-on-nonexistent-replicator, step task#1): call
  aws kafka describe-replicator --replicator-arn arn:aws:kafka:<region>:<account>:replicator/<never-created-name>/<uuid>-1
  (GET /replication/v1/replicators/{ReplicatorArn})
Expect HTTP 404 NotFoundException with message "Invalid parameter replicatorArn: The requested resource doesn’t exist."; observe the unprefixed "The requested resource doesn’t exist." For contrast, the same fixtures' DescribeCluster/DeleteCluster/DeleteTopic misses correctly use the unprefixed message and pass.

Cause 3 (fixture kafka-operate-on-nonexistent-replicator, step task#3): call
  aws kafka delete-replicator --replicator-arn <that same never-issued replicator ARN>
  (DELETE /replication/v1/replicators/{ReplicatorArn})
Expect the same 404 NotFoundException "Invalid parameter replicatorArn: The requested resource doesn’t exist."; observe the unprefixed message.

Then re-replay kafka-cluster-mutations-notfound-2#task#2, kafka-operate-on-nonexistent-replicator#task#1 and #task#3, plus kafka-replicator-crud-notfound-error-paths#task#2 and #task#4, to confirm all three causes are closed.
