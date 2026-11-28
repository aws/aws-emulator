Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-update-connectivity-server-enforced
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-update-connectivity-server-enforced kafka-update-kafka-version-and-operation-lookup-notfound kafka-compatible-kafka-versions-nonexistent-cluster-not-found kafka-cluster-operations-and-policy-notfound-error-paths kafka-configuration-error-paths kafka-configuration-revision-lifecycle kafka-vpc-connection-describe-delete-notfound kafka-vpc-connection-management-workflow-malformed-arn kafka-vpc-connection-nonexistent-arn-rejected-list-clean
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|DescribeCluster|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateConnectivity|4xx->4xx|$.invalidParameter
Issue-Key: kafka|UpdateClusterKafkaVersion|4xx->4xx|$.invalidParameter
Issue-Key: kafka|GetCompatibleKafkaVersions|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DescribeClusterOperation|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DescribeConfigurationRevision|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DescribeConfiguration|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DescribeVpcConnection|4xx->4xx|$.invalidParameter
Issue-Key: kafka|DeleteVpcConnection|4xx->4xx|$.invalidParameter
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK error payloads name the Smithy member instead of the modeled invalidParameter wire name (9 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
NINE distinct root causes of one kind: whenever MSK populates the `invalidParameter` member of an error payload, the emulator writes the Smithy STRUCTURE MEMBER name (PascalCase, e.g. `ClusterArn`, `Arn`, `ClusterOperationArn`) rather than the parameter name AWS reports, which is the member's modeled wire name in camelCase (`clusterArn`, `arn`, `vpcConnectionArn`). In two cases it also names the WRONG parameter entirely. The status and the error code are correct in all nine, so this is exactly the divergence a client cannot defend against: it dispatches on `invalidParameter` to decide which input to fix and silently stops matching. All nine must be closed.

AWS (common to all nine): Smithy kafka#BadRequestException, kafka#NotFoundException and kafka#ConflictException each declare member `InvalidParameter` with jsonName `invalidParameter`, documented as "The parameter that caused the error." MSK names the parameter as the API documents it on the wire - the camelCase member name, or for a path/query-bound ARN the name the operation's reference documentation uses (`clusterArn`, `arn`, `clusterOperationArn`, `vpcConnectionArn`). The fixed text of the message is unaffected; only the parameter token differs.

Emulator (common to all nine): the handler/serializer interpolates the Rust/Smithy member identifier, so every refusal reports a PascalCase token AWS never emits.

Why (common to all nine): a client that reads `invalidParameter` to point an operator at the offending request field is handed a name that appears nowhere in its own request or in the SDK's wire shape, so automated remediation and error-classification both fail. Because the status and code are right, nothing else signals the mismatch.

Fix (common to all nine): this belongs in the ONE shared constructor the service uses to build BadRequestException/NotFoundException/ConflictException (under services/service-com-amazonaws-kafka-kafka/src/), so the parameter token is derived from the member's modeled jsonName rather than its Rust identifier - fixing it per handler leaves the rest diverging. Causes 5 and 9 additionally need the handler to pass the parameter MSK blames, not the one it happened to parse.

Cause 1 of 9: DescribeCluster, well-formed-but-never-issued cluster ARN, HTTP 404 NotFoundException. AWS reports invalidParameter `clusterArn`; emulator reports `ClusterArn`.
Traces: kafka-update-connectivity-server-enforced#task#2.

Cause 2 of 9: UpdateConnectivity, same not-found path, HTTP 404 NotFoundException. AWS `clusterArn`; emulator `ClusterArn`.
Traces: kafka-update-connectivity-server-enforced#task#1.

Cause 3 of 9: UpdateClusterKafkaVersion, HTTP 404 NotFoundException. AWS `clusterArn`; emulator `ClusterArn`.
Traces: kafka-update-kafka-version-and-operation-lookup-notfound#task#1.

Cause 4 of 9: GetCompatibleKafkaVersions (ClusterArn is httpQuery `clusterArn`), HTTP 404 NotFoundException. AWS `clusterArn`; emulator `ClusterArn` - here the modeled query name is literally `clusterArn`, so the emulator is not even using the binding it deserialized from.
Traces: kafka-compatible-kafka-versions-nonexistent-cluster-not-found#task#1.

Cause 5 of 9: DescribeClusterOperation on an operation ARN whose cluster does not exist, HTTP 404 NotFoundException. AWS blames `clusterArn` - the cluster the operation ARN refers to is what is missing; the emulator blames `ClusterOperationArn`, i.e. the wrong parameter AND the wrong case. The handler must attribute the miss to the cluster, not to the input ARN it parsed.
Traces: kafka-cluster-operations-and-policy-notfound-error-paths#task#3.

Cause 6 of 9: DescribeConfigurationRevision with a configuration ARN the service rejects, HTTP 400 BadRequestException. AWS `arn` (the operation's path label is documented as `arn`); emulator `Arn`.
Traces: kafka-configuration-error-paths#task#3.

Cause 7 of 9: DescribeConfiguration, same path, HTTP 400 BadRequestException. AWS `arn`; emulator `Arn`.
Traces: kafka-configuration-revision-lifecycle#teardown#8.

Cause 8 of 9: DescribeVpcConnection with a VPC-connection ARN the service rejects, HTTP 400 BadRequestException. AWS blames `vpcConnectionArn`; emulator blames `Arn` - again the wrong parameter name as well as the wrong case.
Traces: kafka-vpc-connection-describe-delete-notfound#task#1.

Cause 9 of 9: DeleteVpcConnection, same path, HTTP 400 BadRequestException. AWS `vpcConnectionArn`; emulator `Arn`.
Traces: kafka-vpc-connection-describe-delete-notfound#task#2.

Neighbouring filings: the open behavioral issues 20261029T175822Z-kafka-msk-error-messages-are-the-emulator-s-own.md and 20261029T175822Z-kafka-msk-arn-handling-malformed-and-cross-region-arns.md cover MSK message TEXT and refusal-CLASS ordering respectively; neither asserts the `invalidParameter` member, and neither carries any of the fixture#step keys above.

## Evidence
Corpus: simparity-wire-aws-v1 (49 fixtures, 148 steps, 37 passed). All nine clusters are verdict=mismatch with the expected and actual HTTP status EQUAL; the only diverging body key is $.invalidParameter.

Cause 1 - kafka-update-connectivity-server-enforced#task#2, DescribeCluster, status 404 == 404:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'

Cause 2 - kafka-update-connectivity-server-enforced#task#1, UpdateConnectivity, status 404 == 404:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'

Cause 3 - kafka-update-kafka-version-and-operation-lookup-notfound#task#1, UpdateClusterKafkaVersion, status 404 == 404:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'

Cause 4 - kafka-compatible-kafka-versions-nonexistent-cluster-not-found#task#1, GetCompatibleKafkaVersions, status 404 == 404:
  $.invalidParameter: expected 'clusterArn', got 'ClusterArn'

Cause 5 - kafka-cluster-operations-and-policy-notfound-error-paths#task#3, DescribeClusterOperation, status 404 == 404:
  $.invalidParameter: expected 'clusterArn', got 'ClusterOperationArn'
  (cluster also affects kafka-update-kafka-version-and-operation-lookup-notfound#task#2)

Cause 6 - kafka-configuration-error-paths#task#3, DescribeConfigurationRevision, status 400 == 400:
  $.invalidParameter: expected 'arn', got 'Arn'

Cause 7 - kafka-configuration-revision-lifecycle#teardown#8, DescribeConfiguration, status 400 == 400:
  $.invalidParameter: expected 'arn', got 'Arn'

Cause 8 - kafka-vpc-connection-describe-delete-notfound#task#1, DescribeVpcConnection, status 400 == 400:
  $.invalidParameter: expected 'vpcConnectionArn', got 'Arn'
  (cluster also affects kafka-vpc-connection-management-workflow-malformed-arn#task#1 and kafka-vpc-connection-nonexistent-arn-rejected-list-clean#task#1)

Cause 9 - kafka-vpc-connection-describe-delete-notfound#task#2, DeleteVpcConnection, status 400 == 400:
  $.invalidParameter: expected 'vpcConnectionArn', got 'Arn'
  (cluster also affects kafka-vpc-connection-management-workflow-malformed-arn#task#3 and kafka-vpc-connection-nonexistent-arn-rejected-list-clean#task#2)

Smithy cited: kafka#BadRequestException / kafka#NotFoundException / kafka#ConflictException member InvalidParameter -> jsonName 'invalidParameter'; kafka#GetCompatibleKafkaVersions input ClusterArn -> httpQuery 'clusterArn'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture kafka-update-connectivity-server-enforced and inspect step task#2 (DescribeCluster on a never-issued cluster ARN): the 404 NotFoundException body must carry invalidParameter == 'clusterArn'.
3. Replay the remaining fixture#step pairs listed in trace_ids and assert the expected camelCase parameter token in each: 'clusterArn' for UpdateConnectivity / UpdateClusterKafkaVersion / GetCompatibleKafkaVersions / DescribeClusterOperation, 'arn' for DescribeConfigurationRevision / DescribeConfiguration, 'vpcConnectionArn' for DescribeVpcConnection / DeleteVpcConnection.
4. For DescribeClusterOperation and Describe/DeleteVpcConnection also assert the parameter BLAMED is the right one (the cluster for the former, the VPC connection for the latter), not merely re-cased.
5. Re-replay all nine fixture#step bindings; every one must pass with status unchanged.
