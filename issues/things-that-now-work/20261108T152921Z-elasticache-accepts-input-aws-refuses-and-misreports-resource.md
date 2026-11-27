Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.create-cache-cluster-missing-required-parameter-rejected elasticache.deploy-serverless-cache-cfn-invalid-engine-version-rejected elasticache.online-migration-not-found-semantics elasticache.e2e-user-group-replication-group-auth-flow elasticache.concurrent-delete-cache-cluster-race elasticache.replication-group-user-group-ids-reject-bare-user-id elasticache.global-replication-group-lifecycle-full
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.create-cache-cluster-missing-required-parameter-rejected|_error#6|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.deploy-serverless-cache-cfn-invalid-engine-version-rejected|_error#7|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.online-migration-not-found-semantics|_error#8|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.e2e-user-group-replication-group-auth-flow|run#17|AssertionError|raise AssertionError("ModifyUser unexpectedly succeeded against the reserved 'default' user")
Issue-Key: elasticache.concurrent-delete-cache-cluster-race|run#15|AssertionError|assert failures[0][1] == "InvalidCacheClusterState", f"expected InvalidCacheClusterState, got {failures}"
Issue-Key: elasticache.replication-group-user-group-ids-reject-bare-user-id|run#20|AssertionError|assert _fault_base(control_code) == "CacheParameterGroupNotFound", (
Issue-Key: elasticache.global-replication-group-lifecycle-full|run#17|AssertionError|assert primary_rg.get("ClusterEnabled") is True, (
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# ElastiCache accepts input AWS refuses and misreports resource state across seven control-plane paths (7 causes)

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
Seven independent root causes where the emulator ANSWERS 200 (or the wrong refusal / the wrong projected state) for a request real AWS refuses or represents differently. Each is overly-permissive or mis-projected behavior in a different handler and must be closed separately. Related open issues that do NOT own any check below: 20261104T091523Z-elasticache-createserverlesscache-refuses-every-real-subnet-id-the.md (it reports the bogus MajorEngineVersion refusal using the WRONG CODE; the emulator now ACCEPTS it outright, which is Cause 2 here), 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md and 20261108T134359Z-elasticache-replication-group-refusals-never-raised-create-accepts.md.

Cause 1 of 7: CreateCacheCluster omitting the conditionally-required CacheNodeType is ACCEPTED.
AWS: Smithy: CreateCacheCluster.CacheNodeType is an optional String in the model (it is required for a standalone cluster, i.e. when no ReplicationGroupId is given); InvalidParameterValueException -> code 'InvalidParameterValue', 400. Recorded behavior: HTTP 400 InvalidParameterValue with Message 'The parameter CacheNodeType must be provided and must not be null.' — fixed text 'The parameter ' + the member name + ' must be provided and must not be null.' Nothing is created.
Emulator: HTTP 200; the call succeeds and a cluster is created with a silently chosen node type.
Why: a client that forgets the node type is billed for a cluster of a size it never asked for, and the emulator hides a class of client bug AWS surfaces synchronously.
Fix: the CreateCacheCluster required-parameter validation in the elasticache crate (conditional requirement on CacheNodeType when ReplicationGroupId is absent), before any state is written.
Traces: elasticache.create-cache-cluster-missing-required-parameter-rejected.

Cause 2 of 7: CreateServerlessCache with a MajorEngineVersion that does not exist is ACCEPTED.
AWS: Smithy: CreateServerlessCache takes MajorEngineVersion (String) and models InvalidParameterCombinationException (code 'InvalidParameterCombination', 400) and InvalidParameterValueException. Recorded behavior: HTTP 400 InvalidParameterCombination with Message 'Specified engine does not support major engine version (<version>).' — fixed text with the requested MajorEngineVersion in the parentheses; nothing is created, and a CloudFormation stack carrying that template reaches ROLLBACK_COMPLETE.
Emulator: HTTP 200 with ServerlessCache{MajorEngineVersion '999.999', FullEngineVersion '999.999.0', Status 'CREATING'} and endpoints minted, so the CFN stack cannot roll back either.
Why: an impossible engine version is provisioned and endpoint-advertised; the deploy-time rejection AWS gives (and the stack rollback that depends on it) never happens.
Fix: the CreateServerlessCache engine/MajorEngineVersion validation in the elasticache crate, checked against the engine-version catalog before the cache is stored.
Traces: elasticache.deploy-serverless-cache-cfn-invalid-engine-version-rejected.

Cause 3 of 7: StartMigration/TestMigration are ACCEPTED on a replication group with automatic failover disabled.
AWS: Smithy: StartMigration and TestMigration model InvalidParameterValueException (code 'InvalidParameterValue', 400). Recorded behavior on a cluster-mode-disabled, AutomaticFailover='disabled' group: HTTP 400 InvalidParameterValue with Message 'Migration is not supported for auto failover disabled replication group.' (whole string fixed), and the group is left untouched.
Emulator: HTTP 200 — StartMigration returns the ReplicationGroup with Status 'modifying' and AutomaticFailover 'disabled', i.e. it starts a migration AWS will not start.
Why: the precondition that makes online migration safe is unenforced; a client believes a migration is running on a topology that cannot host one.
Fix: the StartMigration/TestMigration precondition checks in the elasticache crate (reject when the group's AutomaticFailover is disabled, before any state transition).
Traces: elasticache.online-migration-not-found-semantics.

Cause 4 of 7: ModifyUser is ACCEPTED against the reserved 'default' user.
AWS: Smithy: ModifyUser takes UserId (required) and models InvalidParameterValueException (code 'InvalidParameterValue', 400). Recorded behavior for UserId='default': HTTP 400 InvalidParameterValue with Message "default user can't be modified." (whole string fixed), while the identical call against a non-reserved user succeeds — and the rejected call leaves default's AccessString unchanged.
Emulator: HTTP 200; the reserved identity is modified.
Why: 'default' governs unauthenticated access for every RBAC user group; letting it be modified lets a client (or an agent) silently change the account-wide default access path that AWS protects.
Fix: the ModifyUser handler's reserved-user guard in the elasticache crate.
Traces: elasticache.e2e-user-group-replication-group-auth-flow.

Cause 5 of 7: the loser of two concurrent DeleteCacheCluster calls answers CacheClusterNotFound instead of InvalidCacheClusterState.
AWS: Smithy: DeleteCacheCluster models CacheClusterNotFoundFault (code 'CacheClusterNotFound', 404) and InvalidCacheClusterStateFault (code 'InvalidCacheClusterState', 400). Recorded behavior for two concurrent deletes of the same available cluster: exactly one returns 200 with CacheClusterStatus 'deleting', the other returns 400 InvalidCacheClusterState (message either names the concurrent operation or ends 'Cluster <id> has state: deleting.').
Emulator: the loser gets 404 CacheClusterNotFound ('CacheCluster not found: <id>') — the cluster is reported absent while it is still deleting.
Why: 404-vs-400 is the difference between 'retry/abort, it is going away' and 'it never existed'; idempotency and teardown logic that keys off NotFound concludes the resource is gone and skips the waits AWS requires.
Fix: the DeleteCacheCluster handler's state transition and not-found-vs-invalid-state classification in the elasticache crate (a cluster in 'deleting' must still resolve and refuse with InvalidCacheClusterState).
Traces: elasticache.concurrent-delete-cache-cluster-race.

Cause 6 of 7: CreateReplicationGroup answers UserGroupNotFound for a user group that EXISTS.
AWS: Smithy: CreateReplicationGroup takes UserGroupIds (UserGroupIdListInput) and models UserGroupNotFoundFault (code 'UserGroupNotFound', 404) and CacheParameterGroupNotFoundFault (code 'CacheParameterGroupNotFound', 404). Recorded behavior: naming a real USER id in UserGroupIds is refused UserGroupNotFoundFault 'User group <id> not found.', while naming an existing USER GROUP clears that validation and the request then fails on the next unsatisfiable input — here CacheParameterGroupNotFound 'CacheParameterGroup not found: <name>'.
Emulator: the request naming an existing user group (created moments earlier by CreateUserGroup) is still refused UserGroupNotFound, so a valid RBAC-attached replication group can never be created.
Why: RBAC-attached replication groups are unreachable, and the validation order AWS documents (user-group resolution before parameter-group resolution) cannot be observed because the first check never passes.
Fix: the CreateReplicationGroup UserGroupIds resolution in the elasticache crate — resolve against the stored user groups created by CreateUserGroup.
Traces: elasticache.replication-group-user-group-ids-reject-bare-user-id.

Cause 7 of 7: a replication group created cluster-mode-enabled reports ClusterEnabled=false.
AWS: Smithy: CreateReplicationGroup takes NumNodeGroups/CacheParameterGroupName and ReplicationGroup$ClusterEnabled is a BooleanOptional projection. Recorded behavior: a group created with NumNodeGroups set and a '.cluster.on' parameter-group family reports ClusterEnabled=true, which is the precondition for every reshard and for IncreaseNodeGroupsInGlobalReplicationGroup.
Emulator: the create succeeds but DescribeReplicationGroups reports ClusterEnabled=False for a group created with NumNodeGroups=1 and CacheParameterGroupName 'default.redis7.cluster.on'.
Why: every cluster-mode operation behind it (reshard, global-datastore node-group increase) is rejected or fails opaquely, and a client cannot tell from the projection that its cluster-mode request was ignored.
Fix: the ReplicationGroup projection/creation path in the elasticache crate — derive ClusterEnabled from the requested topology and parameter-group family.
Traces: elasticache.global-replication-group-lifecycle-full.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout (68/161 passed). Expected = recorded AWS behavior, actual = emulator:
1. create-cache-cluster-missing-required-parameter-rejected: CreateCacheCluster(CacheClusterId=..., Engine='memcached', NumCacheNodes=1, CacheSubnetGroupName=...) with NO CacheNodeType -> AssertionError: real AWS rejects this call, but it SUCCEEDED and returned None. Expected 400 InvalidParameterValue 'The parameter CacheNodeType must be provided and must not be null.'
2. deploy-serverless-cache-cfn-invalid-engine-version-rejected: CreateServerlessCache(Engine='redis', MajorEngineVersion='999.999', SubnetIds=[...]) -> AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'ServerlessCache': {... 'MajorEngineVersion': '999.999', 'FullEngineVersion': '999.999.0', 'Status': 'CREATING', 'Endpoint': {...}}, 'ResponseMetadata': {'HTTPStatusCode': 200}}. Expected 400 InvalidParameterCombination 'Specified engine does not support major engine version (999.999).'
3. online-migration-not-found-semantics: StartMigration(ReplicationGroupId=<AutomaticFailover disabled group>, CustomerNodeEndpointList=[...]) -> AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ReplicationGroup': {... 'Status': 'modifying', 'AutomaticFailover': 'disabled' ...}, 'ResponseMetadata': {'HTTPStatusCode': 200}}. Expected 400 InvalidParameterValue 'Migration is not supported for auto failover disabled replication group.'
4. e2e-user-group-replication-group-auth-flow: ModifyUser(UserId='default', AccessString=<unchanged>) -> AssertionError: ModifyUser unexpectedly succeeded against the reserved 'default' user. Expected 400 InvalidParameterValue "default user can't be modified." (the identical call against the run's own user succeeded first, as the positive control).
5. concurrent-delete-cache-cluster-race: two concurrent DeleteCacheCluster -> AssertionError: expected InvalidCacheClusterState, got [('error', 'CacheClusterNotFound', 'CacheCluster not found: sim2real-cdc-e79e52575284')].
6. replication-group-user-group-ids-reject-bare-user-id: CreateUser + CreateUserGroup(UserIds=['default', <user>]) then CreateReplicationGroup(UserGroupIds=[<that group id>], CacheParameterGroupName=<nonexistent>) -> AssertionError: expected the real group id to clear UserGroupIds validation and fail with CacheParameterGroupNotFoundFault instead, got UserGroupNotFound.
7. global-replication-group-lifecycle-full: AssertionError: primary <id> was created with NumNodeGroups=1 and parameter group 'default.redis7.cluster.on' but reports ClusterEnabled=False; the later reshard (increase_node_groups_in_global_replication_group) requires cluster mode enabled and would fail opaquely without this.
Model evidence: CreateCacheCluster/CreateReplicationGroup/StartMigration/TestMigration/ModifyUser/CreateServerlessCache/DeleteCacheCluster all bind InvalidParameterValueException -> {InvalidParameterValue, 400}; DeleteCacheCluster binds CacheClusterNotFoundFault -> {CacheClusterNotFound, 404} and InvalidCacheClusterStateFault -> {InvalidCacheClusterState, 400}; CreateReplicationGroup binds UserGroupNotFoundFault -> {UserGroupNotFound, 404} and CacheParameterGroupNotFoundFault -> {CacheParameterGroupNotFound, 404}.

## Steps to Reproduce
1. Boot the emulator from this checkout and run suite simparity-behavioral-aws-v1.
2. Cause 1: CreateCacheCluster with Engine='memcached', NumCacheNodes=1, a cache subnet group and NO CacheNodeType -> expect 400 InvalidParameterValue 'The parameter CacheNodeType must be provided and must not be null.' and DescribeCacheClusters answering CacheClusterNotFound; observe 200. Re-replay elasticache.create-cache-cluster-missing-required-parameter-rejected.
3. Cause 2: CreateServerlessCache(Engine='redis', MajorEngineVersion='999.999') -> expect 400 InvalidParameterCombination 'Specified engine does not support major engine version (999.999).'; then create a CFN stack with AWS::ElastiCache::ServerlessCache carrying that MajorEngineVersion -> expect ROLLBACK_COMPLETE. Re-replay elasticache.deploy-serverless-cache-cfn-invalid-engine-version-rejected.
4. Cause 3: create a redis replication group with NumCacheClusters=1 (AutomaticFailover disabled), then StartMigration and TestMigration on it -> expect 400 InvalidParameterValue 'Migration is not supported for auto failover disabled replication group.'; observe 200 with Status 'modifying'. Re-replay elasticache.online-migration-not-found-semantics.
5. Cause 4: ensure user 'default' exists, then ModifyUser(UserId='default', AccessString=<its current value>) -> expect 400 InvalidParameterValue "default user can't be modified."; observe success. Re-replay elasticache.e2e-user-group-replication-group-auth-flow.
6. Cause 5: create a memcached cluster, wait for available, then fire two DeleteCacheCluster calls concurrently -> expect exactly one 200 with 'deleting' and one 400 InvalidCacheClusterState; observe 404 CacheClusterNotFound for the loser. Re-replay elasticache.concurrent-delete-cache-cluster-race.
7. Cause 6: CreateUser, CreateUserGroup(UserIds=['default', <user>]), then CreateReplicationGroup(UserGroupIds=[<group id>], TransitEncryptionEnabled=True, CacheParameterGroupName=<a name that does not exist>) -> expect 404 CacheParameterGroupNotFound; observe UserGroupNotFound. Re-replay elasticache.replication-group-user-group-ids-reject-bare-user-id.
8. Cause 7: CreateReplicationGroup(NumNodeGroups=1, CacheParameterGroupName='default.redis7.cluster.on', ...) then DescribeReplicationGroups -> expect ClusterEnabled=True. Re-replay elasticache.global-replication-group-lifecycle-full.
