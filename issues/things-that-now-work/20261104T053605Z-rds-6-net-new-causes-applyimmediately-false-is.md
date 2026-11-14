Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.maintenance-window-controls-pending-change-application rds.performance-insights-retention-value-enforced rds.cluster-maintenance-window-day-names-read-back-lower-case rds.blue-green-deployment-lifecycle rds.deploy-db-cluster-cfn-behaves rds.deploy-db-cluster-cfn-parameter-group-enforcement rds.deploy-rds-cluster-cfn-behavioral rds.deploy-cfn-stack-update-modifies-live-parameter-group rds.backtrack-cluster-contract rds.cluster-start-stop-reboot-failover-transitions rds.create-read-replica-cross-engine-family-rejected rds.create-read-replica-cross-engine-mismatch-rejected rds.global-cluster-failover-and-switchover-rejected rds.promote-non-replica-instance-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.maintenance-window-controls-pending-change-application|run#15|AssertionError|assert after_modify["AllocatedStorage"] == 20, "storage changed live despite ApplyImmediately=False"
Issue-Key: rds.performance-insights-retention-value-enforced|run#9|KeyError|assert good_instance["PerformanceInsightsRetentionPeriod"] == 31, (
Issue-Key: rds.cluster-maintenance-window-day-names-read-back-lower-case|run#6|AssertionError|assert created["PreferredMaintenanceWindow"] == _STORED_MAINTENANCE_WINDOW, (
Issue-Key: rds|CreateDBCluster|InvalidParameterCombination
Issue-Key: rds.deploy-db-cluster-cfn-behaves|_wait_stack_status#6|AssertionError|raise AssertionError(f"stack {stack_name} reached unexpected status {status}")
Issue-Key: rds.deploy-db-cluster-cfn-parameter-group-enforcement|_deploy_cfn#11|AssertionError|raise AssertionError(f"stack {stack_name} entered {status}: {reasons}")
Issue-Key: rds.deploy-rds-cluster-cfn-behavioral|_deploy_cfn#6|WaiterError|cfn.get_waiter("stack_create_complete").wait(
Issue-Key: rds.deploy-cfn-stack-update-modifies-live-parameter-group|_wait_stack_status#25|AssertionError|raise AssertionError(f"stack {stack_name} reached unexpected status {status}")
Issue-Key: rds.backtrack-cluster-contract|run#11|AssertionError|assert missing_code == ERR_DB_CLUSTER_NOT_FOUND, f"unexpected error code: {missing_code!r}"
Issue-Key: rds.cluster-start-stop-reboot-failover-transitions|run#14|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code!r}"
Issue-Key: rds.create-read-replica-cross-engine-family-rejected|run#18|AssertionError|assert code == "InvalidParameterCombination", f"expected InvalidParameterCombination, got {code!r}"
Issue-Key: rds.create-read-replica-cross-engine-mismatch-rejected|run#14|AssertionError|assert code == "InvalidParameterCombination", f"unexpected error code: {code!r}"
Issue-Key: rds.global-cluster-failover-and-switchover-rejected|run#16|AssertionError|assert err.get("Code") == "DBClusterNotFoundFault", f"expected DBClusterNotFoundFault, got {err.get('Code')!r}"
Issue-Key: rds.promote-non-replica-instance-rejected|run#9|AssertionError|assert code == "InvalidDBInstanceState", f"expected InvalidDBInstanceState, got {code!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: 6 net-new causes — ApplyImmediately=False is applied live, Performance Insights and maintenance-window fields are misrepresented, Serverless v2 is wrongly refused, CloudFormation RDS stacks roll back, and 6 stubbed operations answer 501 where AWS refuses with a modeled error

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBInstance

## Description
Six distinct causes in state handling, representation, CloudFormation provisioning and stub coverage.

Cause 1 of 6: ModifyDBInstance with ApplyImmediately=False applies the change at once. AWS: with ApplyImmediately=false the new value is staged in DBInstance$PendingModifiedValues and only applied in the preferred maintenance window; the live attribute keeps its old value. Emulator: the attribute changes live ('storage changed live despite ApplyImmediately=False') and PendingModifiedValues stays {}. Check: rds.maintenance-window-controls-pending-change-application.

Cause 2 of 6: Performance Insights settings are not reported. AWS: DBInstance carries PerformanceInsightsEnabled and PerformanceInsightsRetentionPeriod (Smithy: CreateDBInstance/ModifyDBInstance input PerformanceInsightsRetentionPeriod → the same member on the DBInstance envelope); the retention value accepted on create must read back. Emulator: the member is absent from the response (KeyError 'PerformanceInsightsRetentionPeriod'). Check: rds.performance-insights-retention-value-enforced.

Cause 3 of 6: PreferredMaintenanceWindow is not normalised. AWS stores and returns the window with lower-case day names ('mon:00:00-mon:03:00') whatever case the request used. Emulator echoes the request's case ('Mon:00:00-Mon:03:00'). Check: rds.cluster-maintenance-window-day-names-read-back-lower-case.

Cause 4 of 6: CreateDBCluster refuses a valid ServerlessV2ScalingConfiguration. AWS: ServerlessV2ScalingConfiguration is modeled on CreateDBCluster and is accepted for Aurora versions that support Serverless v2. Emulator answers 400 InvalidParameterCombination 'ServerlessV2ScalingConfiguration is not supported by engine aurora-mysql version 5.7.mysql_aurora.2.11.6: that version runs no Aurora Serverless v2 capacity' for the engine version the check selected as supported, so the whole blue/green lifecycle cannot start. Check: rds.blue-green-deployment-lifecycle.

Cause 5 of 6: CloudFormation stacks containing RDS resources roll back. A stack declaring AWS::RDS::DBCluster plus AWS::RDS::DBInstance fails the instance with 'Resource handler returned message: "The source cluster could not be found or cannot be accessed: <cluster id>" (HandlerErrorCode: NotFound)' even though the cluster resource reached CREATE_COMPLETE, and a stack UPDATE that modifies a live parameter group enters UPDATE_ROLLBACK_IN_PROGRESS; one cluster stack is rejected outright by the waiter with ValidationError. AWS: both templates reach CREATE_COMPLETE/UPDATE_COMPLETE and the cluster-member instance resolves its DBClusterIdentifier. Checks: rds.deploy-db-cluster-cfn-behaves, rds.deploy-db-cluster-cfn-parameter-group-enforcement, rds.deploy-rds-cluster-cfn-behavioral, rds.deploy-cfn-stack-update-modifies-live-parameter-group.

Cause 6 of 6: six operations are unimplemented scaffold stubs, so requests AWS refuses with a modeled error instead get 501 'operation <X> is scaffolded but not implemented in this emulator' with code NotImplemented and Type Receiver. The negative contracts lost are: BacktrackDBCluster on a missing/ineligible cluster (DBClusterNotFoundFault / InvalidDBClusterStateFault), StartDBCluster/StopDBCluster/FailoverDBCluster state transitions (InvalidParameterValue), CreateDBInstanceReadReplica across engine families and across engine mismatches (InvalidParameterCombination), FailoverGlobalCluster/SwitchoverGlobalCluster on an unknown cluster (DBClusterNotFoundFault) and PromoteReadReplica on a non-replica (InvalidDBInstanceState). Because the refusal is answered 501 Receiver, SDK retry layers treat a permanent client error as a server-side transient. Checks: rds.backtrack-cluster-contract, rds.cluster-start-stop-reboot-failover-transitions, rds.create-read-replica-cross-engine-family-rejected, rds.create-read-replica-cross-engine-mismatch-rejected, rds.global-cluster-failover-and-switchover-rejected, rds.promote-non-replica-instance-rejected.

Why: clients that stage a change for the maintenance window see it applied to a live database immediately (cause 1); monitoring configuration a client set cannot be read back (cause 2); window strings do not round-trip, breaking idempotent diffing and CloudFormation drift detection (cause 3); valid Aurora Serverless v2 clusters cannot be created at all (cause 4); infrastructure-as-code cannot provision RDS (cause 5); and negative-path code sees a retryable 501 instead of a terminal 400 (cause 6).

Fix: modify_db_instance.rs's pending-vs-immediate application path and the DBInstance serializer (PerformanceInsights* members, PreferredMaintenanceWindow normalisation) in the rds crate; create_db_cluster.rs's Serverless v2 support matrix; the CloudFormation AWS::RDS::DBInstance/DBCluster/DBParameterGroup resource handlers' cluster-reference resolution; and the named stub handlers (backtrack_db_cluster.rs, start/stop/failover_db_cluster.rs, create_db_instance_read_replica.rs, failover_global_cluster.rs/switchover_global_cluster.rs, promote_read_replica.rs). Note: separate already-filed stub issues exist for some of these operations, but the suite left these checks unowned.

## Evidence
Verbatim replay output:
- 'AssertionError: storage changed live despite ApplyImmediately=False'.
- "KeyError: 'PerformanceInsightsRetentionPeriod'".
- "AssertionError: CreateDBCluster answered 'Mon:00:00-Mon:03:00' for 'Mon:00:00-Mon:03:00', expected 'mon:00:00-mon:03:00'".
- 'botocore.exceptions.ClientError: An error occurred (InvalidParameterCombination) when calling the CreateDBCluster operation: ServerlessV2ScalingConfiguration is not supported by engine aurora-mysql version 5.7.mysql_aurora.2.11.6: that version runs no Aurora Serverless v2 capacity'.
- 'stack <name> reached unexpected status ROLLBACK_IN_PROGRESS', 'stack <name> reached unexpected status UPDATE_ROLLBACK_IN_PROGRESS', "stack <name> entered ROLLBACK_IN_PROGRESS: ['The following resource(s) failed to create: [DBInstance]. ', 'Resource handler returned message: \"The source cluster could not be found or cannot be accessed: <cluster id>\" (HandlerErrorCode: NotFound)']", 'botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: ... Matched expected service error code: ValidationError'.
- "unexpected error code: 'NotImplemented'", "expected InvalidParameterValue, got 'NotImplemented'", "expected InvalidParameterCombination, got 'NotImplemented'", "expected DBClusterNotFoundFault, got 'NotImplemented'", "expected InvalidDBInstanceState, got 'NotImplemented'".

## Steps to Reproduce
1. modify_db_instance(AllocatedStorage=<new>, ApplyImmediately=False) then describe_db_instances — the live value already changed and PendingModifiedValues is empty.
2. create_db_instance(EnablePerformanceInsights=True, PerformanceInsightsRetentionPeriod=<n>) then describe_db_instances — the member is absent.
3. create_db_cluster(PreferredMaintenanceWindow='Mon:00:00-Mon:03:00') and read it back.
4. create_db_cluster(Engine='aurora-mysql', ServerlessV2ScalingConfiguration={...}) on a version the engine catalog reports as Serverless-v2 capable.
5. Deploy a CFN template with AWS::RDS::DBCluster + a member AWS::RDS::DBInstance; and update a stack that modifies a live AWS::RDS::DBParameterGroup.
6. Call backtrack_db_cluster / start_db_cluster / create_db_instance_read_replica / failover_global_cluster / promote_read_replica with the invalid input each check uses — all answer 501 NotImplemented.
