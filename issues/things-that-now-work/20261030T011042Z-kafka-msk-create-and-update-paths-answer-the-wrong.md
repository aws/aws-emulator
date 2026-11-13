Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-rejected-provisioning-requests
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-rejected-provisioning-requests kafka-create-replicator-nonexistent-clusters kafka-replicator-create-describe-delete-lifecycle kafka-replicator-not-found-and-validation-errors kafka-cluster-operations-and-policy-notfound-error-paths
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateCluster|4xx->4xx|$.invalidParameter
Issue-Key: kafka|CreateCluster|4xx->4xx|$.message
Issue-Key: kafka|CreateReplicator|2xx->4xx|$.replicatorArn
Issue-Key: kafka|CreateReplicator|2xx->4xx|$.replicatorName
Issue-Key: kafka|CreateReplicator|2xx->4xx|$.replicatorState
Issue-Key: kafka|CreateReplicator|2xx->4xx|status
Issue-Key: kafka|CreateReplicator|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|CreateReplicator|4xx->4xx|status
Issue-Key: kafka|UpdateClusterConfiguration|4xx->4xx|$.message
Issue-Key: kafka|UpdateClusterConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kafka|UpdateClusterConfiguration|4xx->4xx|status
Issue-Related: 20261029T202809Z-kafka-msk-not-found-already-exists-and-tagging-refusals.md
Issue-Commit: f436928141e9968ccb87a436b810c90c79edbabf

# MSK create and update paths answer the wrong refusal: an EC2 subnet pre-check and a resource lookup pre-empt MSK's own validation, conflict and authorization (5 causes)

> _Note: this issue was identified against the code base at commit `f436928141e9968ccb87a436b810c90c79edbabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
FIVE distinct net-new root causes sharing one fault: on MSK's create/update paths the emulator answers from an internal pre-check (an EC2-style subnet-existence lookup, or the cluster resource lookup) BEFORE MSK's own validation, conflict and authorization behavior gets to decide. The result is a refusal of the wrong class, with a message that is not MSK's, and in one case a refusal of a request AWS accepts outright. All five must be closed. Causes 1-4 are all CreateCluster/CreateReplicator in the provisioning plane; cause 5 is the same ordering fault on UpdateClusterConfiguration.

Cause 1 of 5: CreateCluster with client subnets that do not exist is refused with an EC2 SDK exception string instead of MSK's sentence.
AWS: Smithy kafka#CreateCluster, POST /v1/clusters, member BrokerNodeGroupInfo (jsonName 'brokerNodeGroupInfo') is required; modeled errors include BadRequestException (400). Recorded behavior: HTTP 400 with body member `invalidParameter` naming the TOP-LEVEL MSK input member in its wire (jsonName) spelling, 'brokerNodeGroupInfo', and `message` exactly "One or more of the specified client subnets don’t exist." — that sentence is entirely fixed text; MSK does not interpolate the offending subnet id and does not name a nested member path.
Emulator: HTTP 400, but `invalidParameter` is the dotted Smithy member path 'BrokerNodeGroupInfo.ClientSubnets' and `message` is a serialized EC2 client exception of the form "The subnet ID '<subnet-id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <uuid>; Proxy: null)".
Why: a client that switches on `invalidParameter` to map a validation failure back to the field it sent gets a token that is not in MSK's wire vocabulary (MSK uses jsonName spellings, never PascalCase dotted paths), and a client or test asserting MSK's subnet sentence never matches. The message also embeds a per-run request id, so it is not even stable between runs.
Fix: CreateCluster's input validation in the kafka service crate must raise MSK's own BadRequestException (jsonName member + MSK's fixed sentence) for a non-existent client subnet, instead of letting the internal subnet-lookup error surface to the wire.
Traces: kafka-rejected-provisioning-requests#task#1.

Cause 2 of 5: CreateReplicator naming clusters that do not exist is refused 400, where AWS accepts the create.
AWS: Smithy kafka#CreateReplicator, POST /replication/v1/replicators, required KafkaClusters/ReplicationInfoList/ReplicatorName/ServiceExecutionRoleArn; success is HTTP 200 with replicatorArn, replicatorName and replicatorState. Recorded behavior: HTTP 200 with `replicatorState` 'CREATING' and `replicatorArn`/`replicatorName` echoing the created replicator — MSK admits the request and resolves cluster/subnet reachability asynchronously while the replicator is CREATING; it does not synchronously reject the call on subnet existence.
Emulator: HTTP 400 with the same EC2 'InvalidSubnetID.NotFound' string described in Cause 1, and no replicatorArn/replicatorName/replicatorState in the body.
Why: the replicator lifecycle can never be entered against the emulator — no replicator is ever created, so Describe/List/Delete/UpdateReplicationInfo on a real replicator are unreachable, and a client that expects the asynchronous CREATING state model sees a synchronous hard failure.
Fix: CreateReplicator's handler must accept the request and store the replicator in CREATING rather than running a synchronous subnet-existence pre-check in its validation path.
Traces: kafka-create-replicator-nonexistent-clusters#task#2 (cluster also covers kafka-replicator-create-describe-delete-lifecycle#task#2).

Cause 3 of 5: CreateReplicator with an already-used replicatorName does not produce MSK's duplicate-name conflict.
AWS: kafka#CreateReplicator models ConflictException (409). Recorded behavior: HTTP 409 with `invalidParameter` 'replicatorName' and `message` exactly "A resource with this name already exists." — fixed text; the conflicting name appears only in `invalidParameter`, as the member name, not interpolated into the sentence.
Emulator: HTTP 400, x-amzn-errortype 'BadRequestException', `invalidParameter` 'KafkaClusters[0].VpcConfig.SubnetIds' and the EC2 'InvalidSubnetID.NotFound' message — i.e. the subnet pre-check answers before the name-uniqueness check runs at all.
Why: a caller cannot distinguish 'this name is taken' (pick another name / treat as idempotent) from 'your request is malformed', and the 4xx class is wrong (400 vs 409), which breaks conflict-retry logic.
Fix: order the replicator name-uniqueness check ahead of any subnet pre-check in CreateReplicator, and raise ConflictException with MSK's sentence.
Traces: kafka-replicator-create-describe-delete-lifecycle#task#3.

Cause 4 of 5: CreateReplicator with a cross-account ServiceExecutionRoleArn is refused 400 BadRequestException instead of 403 AccessDeniedException.
AWS: kafka#CreateReplicator requires ServiceExecutionRoleArn and models ForbiddenException (403). Recorded behavior: HTTP 403 with header x-amzn-errortype 'AccessDeniedException' and `message` exactly "Cross-account pass role is not allowed." — entirely fixed text, nothing taken from the request.
Emulator: HTTP 400, x-amzn-errortype 'BadRequestException', with the EC2 'InvalidSubnetID.NotFound' message — the pass-role account check is never reached because the subnet pre-check refuses first.
Why: a security test that asserts MSK refuses a foreign pass-role sees a syntactic 400 and cannot tell a denial from a malformed request.
Fix: evaluate the account segment of ServiceExecutionRoleArn before (not after) any subnet/cluster pre-check in CreateReplicator, and raise the 403 AccessDeniedException refusal.
Note: the open issue 20261029T202809Z-kafka-msk-resource-authorization-is-never-evaluated-so-denied.md records the SAME pass-role rule, but from the 403->200 recording on kafka-create-replicator-malformed-role-arn#task#1; it does not carry this fixture#step and its key is CreateReplicator 4xx->2xx. On THIS step the emulator instead answers 4xx->4xx from the subnet pre-check, so fixing the pass-role gate alone leaves this step diverging until the pre-check is removed or ordered after it. It is therefore filed here rather than left to that issue.

Cause 5 of 5: UpdateClusterConfiguration on a cluster in another account answers 404 NotFoundException instead of 403 AccessDeniedException.
AWS: Smithy kafka#UpdateClusterConfiguration, PUT /v1/clusters/{ClusterArn}/configuration, ClusterArn is an httpLabel; modeled errors include ForbiddenException (403) and NotFoundException (404). Recorded behavior: when the target cluster ARN names an account the caller holds no resource-based policy for, the refusal is HTTP 403 with header x-amzn-errortype 'AccessDeniedException' and a `message` of the form "User: <caller principal ARN> is not authorized to perform: kafka:UpdateClusterConfiguration on resource: <target cluster ARN> because no resource-based policy allows the kafka:UpdateClusterConfiguration action". Fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action". Variable parts, taken from the credentials and the request: the caller's principal ARN, the IAM action name (kafka:UpdateClusterConfiguration) and the target cluster ARN from the httpLabel. The AccessDeniedException code itself is recorded behavior only (the model names ForbiddenException for 403); the 403 status is modeled.
Emulator: HTTP 404 with x-amzn-errortype 'NotFoundException' and `message` "The requested resource doesn’t exist." — the resource lookup runs before any authorization evaluation, so a denial is reported as an absence.
Why: a client cannot distinguish 'you may not touch this cluster' (escalate to IAM) from 'it is not there' (create/retry), so cross-account and least-privilege behavior cannot be exercised.
Fix: authorize the operation's IAM action against the RAW ClusterArn label in the kafka request-authorization layer before UpdateClusterConfiguration's handler resolves the cluster.
Note: the open issue 20261029T202809Z-kafka-msk-resource-authorization-is-never-evaluated-so-denied.md describes this same gate-ordering family for DescribeClusterV2, the channel surface, UpdateConnectivity and the topic surface, but it does NOT carry UpdateClusterConfiguration or this fixture#step, so this cause is net-new and must be closed separately.
Traces: kafka-cluster-operations-and-policy-notfound-error-paths#task#4.

## Evidence
Corpus: simparity-wire-aws-v1. All five clusters verdict=mismatch; none carries a stub_path or an owning open issue.

Cause 1 — kafka-rejected-provisioning-requests#task#1, CreateCluster, status 400 expected / 400 actual:
  $.invalidParameter: expected 'brokerNodeGroupInfo', got 'BrokerNodeGroupInfo.ClientSubnets'
  $.message: expected 'One or more of the specified client subnets don’t exist.', got "The subnet ID '<subnet-id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <uuid>; Proxy: null)"

Cause 2 — kafka-create-replicator-nonexistent-clusters#task#2, CreateReplicator, status expected 200, got 400:
  $.replicatorArn: expected present, got None
  $.replicatorName: expected present, got None
  $.replicatorState: expected 'CREATING', got None
  (cluster also affects kafka-replicator-create-describe-delete-lifecycle#task#2)

Cause 3 — kafka-replicator-create-describe-delete-lifecycle#task#3, CreateReplicator, status expected 409, got 400:
  $.invalidParameter: expected 'replicatorName', got 'KafkaClusters[0].VpcConfig.SubnetIds'
  $.message: expected 'A resource with this name already exists.', got the EC2 InvalidSubnetID.NotFound string
  $headers.x-amzn-errortype: got 'BadRequestException'

Cause 4 — kafka-replicator-not-found-and-validation-errors#task#2, CreateReplicator, status expected 403, got 400:
  $.message: expected 'Cross-account pass role is not allowed.', got the EC2 InvalidSubnetID.NotFound string
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'BadRequestException'

Cause 5 — kafka-cluster-operations-and-policy-notfound-error-paths#task#4, UpdateClusterConfiguration, status expected 403, got 404:
  $.message: expected 'User: <principal ARN> is not authorized to perform: kafka:UpdateClusterConfiguration on resource: <cluster ARN> because no resource-based policy allows the kafka:UpdateClusterConfiguration action', got 'The requested resource doesn’t exist.'
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'NotFoundException'

Smithy cited: kafka#CreateCluster POST /v1/clusters, BrokerNodeGroupInfo required (jsonName 'brokerNodeGroupInfo'), errors BadRequestException(400)/ConflictException(409)/ForbiddenException(403)/UnauthorizedException(401); kafka#CreateReplicator POST /replication/v1/replicators, required KafkaClusters, ReplicationInfoList, ReplicatorName (pattern ^[0-9A-Za-z][0-9A-Za-z-]{0,}$), ServiceExecutionRoleArn, output replicatorArn/replicatorName/replicatorState, errors include ConflictException(409), ForbiddenException(403), NotFoundException(404); kafka#UpdateClusterConfiguration PUT /v1/clusters/{ClusterArn}/configuration, errors include ForbiddenException(403) and NotFoundException(404).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay kafka-rejected-provisioning-requests#task#1 — CreateCluster whose brokerNodeGroupInfo.clientSubnets name subnets that do not exist must answer HTTP 400 with $.invalidParameter == 'brokerNodeGroupInfo' and $.message == 'One or more of the specified client subnets don’t exist.', with no EC2 service/error-code/request-id text.
3. Cause 2: replay kafka-create-replicator-nonexistent-clusters#task#2 — CreateReplicator naming clusters that do not exist must answer HTTP 200 with replicatorArn, replicatorName and replicatorState == 'CREATING', and the replicator must then be visible to DescribeReplicator/ListReplicators (also re-run kafka-replicator-create-describe-delete-lifecycle#task#2).
4. Cause 3: replay kafka-replicator-create-describe-delete-lifecycle#task#3 — CreateReplicator reusing an existing replicatorName must answer HTTP 409 with $.invalidParameter == 'replicatorName' and $.message == 'A resource with this name already exists.', and must not answer the subnet error.
5. Cause 4: replay kafka-replicator-not-found-and-validation-errors#task#2 — CreateReplicator with a ServiceExecutionRoleArn in another account must answer HTTP 403 with x-amzn-errortype 'AccessDeniedException' and $.message == 'Cross-account pass role is not allowed.', and must not create a replicator.
6. Cause 5: replay kafka-cluster-operations-and-policy-notfound-error-paths#task#4 — UpdateClusterConfiguration on a cluster ARN whose account segment is not the caller's must answer HTTP 403 with x-amzn-errortype 'AccessDeniedException' and the 'User: ... is not authorized to perform: kafka:UpdateClusterConfiguration on resource: ... because no resource-based policy allows the kafka:UpdateClusterConfiguration action' message, with principal and cluster ARN taken from the credentials and request.
7. Re-replay all five fixture#step bindings listed above; all five must pass before submitting.
