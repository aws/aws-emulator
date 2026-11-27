Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative rds__s3.cfn-deploy-rds-s3-role-policy-wrong-bucket-arn-negative rds__s3.deploy-cfn-rds-instance-with-s3-import-role-positive rds__s3.deploy-rds-aurora-s3-import-extension-positive rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive
Issue-Key: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative|run#10|AssertionError|raise AssertionError(
Issue-Key: rds__s3.cfn-deploy-rds-s3-role-policy-wrong-bucket-arn-negative|run#21|AssertionError|assert message == expected_message, (
Issue-Key: rds__s3.deploy-cfn-rds-instance-with-s3-import-role-positive|run#19|PublishedUrlRejected|endpoint, port = ctx.endpoint_host(outputs["ClusterEndpoint"], int(outputs["ClusterPort"]))
Issue-Key: rds__s3.deploy-rds-aurora-s3-import-extension-positive|run#25|PublishedUrlRejected|endpoint, port = ctx.endpoint_host(endpoint, port)
Issue-Key: rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive|_poll_until#20|Timeout|return last
Issue-Key: rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 2b46cf3df3e6c8d83849f7668fee251a8b60f8ab

# rds__s3: 4 net-new causes — AWS::RDS::DBCluster accepts an s3Import AssociatedRoles entry on aurora-mysql, StartExportTask's IamRoleMissingPermissions names the wrong S3 action, DB cluster endpoints are published as unreachable rds.amazonaws.com hosts, and long-running RDS states never settle

> _Note: this issue was identified against the code base at commit `2b46cf3df3e6c8d83849f7668fee251a8b60f8ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
This issue bundles 4 distinct net-new root causes in the RDS-to-S3 surface. A fixer must close ALL FOUR before submitting.

Smithy consulted (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): StartExportTask input ExportTaskIdentifier/SourceArn/S3BucketName/IamRoleArn/KmsKeyId required, S3Prefix and ExportOnly optional; output ExportTask with Status, PercentProgress, FailureCause; modeled errors include IamRoleMissingPermissionsFault (client, httpError 400, awsQueryError code 'IamRoleMissingPermissions'), IamRoleNotFoundFault 404, InvalidS3BucketFault 400, InvalidExportSourceStateFault 400, KMSKeyNotAccessibleFault 400, DBClusterSnapshotNotFoundFault 404. AddRoleToDBCluster input DBClusterIdentifier/RoleArn required (IAMRoleArn pattern ^arn:aws[a-z-]*:iam::[0-9]*:role/.*$), FeatureName optional, empty output; modeled errors DBClusterNotFoundFault 404, DBClusterRoleAlreadyExistsFault 400, DBClusterRoleQuotaExceededFault 400, InvalidDBClusterStateFault 400 — note InvalidParameterValue is a service-common query refusal, not one of the four, and is what AWS uses for an unsupported FeatureName.

---
Cause 1 of 4: an AWS::RDS::DBCluster whose inline AssociatedRoles entry uses FeatureName 's3Import' on engine aurora-mysql is created instead of refused.
AWS: s3Import is not a feature name Aurora MySQL supports on a DB cluster role association (Aurora MySQL reaches S3 through the aurora_load_from_s3_role cluster parameter, not through this FeatureName). CloudFormation fails the resource and the stack does not reach CREATE_COMPLETE; the resource's failure reason names the feature and the engine — the fixed parts under test are the token 's3Import' and the clause 'not valid for the Aurora MySQL' (engine and feature are taken from the template). The cluster is never created, so DescribeDBClusters on its identifier answers DBClusterNotFoundFault with the message 'DBCluster <identifier> not found.' (identifier from the request).
Emulator: the stack carrying the inline AssociatedRoles entry reaches CREATE_COMPLETE, so the invalid association is accepted and a cluster exists. The same check's control stack (identical template without the AssociatedRoles entry) deploys and reports the cluster 'available', so this is not a general CloudFormation/Aurora failure.
Why: an over-permissiveness defect — a template that AWS rejects deploys clean against the emulator, so a stack that cannot exist in production passes pre-production validation, and the caller never learns the feature name is wrong for the engine.
Fix: validation of AWS::RDS::DBCluster's AssociatedRoles in the CloudFormation resource provider, against the engine's supported feature names (the same table AddRoleToDBCluster must consult), failing the resource with a reason naming the feature and engine.
Traces: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative.

---
Cause 2 of 4: StartExportTask's IamRoleMissingPermissions refusal names the wrong S3 action.
AWS: when the IamRoleArn's S3 policy does not cover the target bucket, StartExportTask is refused with HTTP 400, Error.Code 'IamRoleMissingPermissions' (modeled IamRoleMissingPermissionsFault, awsQueryError code IamRoleMissingPermissions) and the message 'The IAM role <role arn> isn't authorized to call s3:GetBucketLocation on the S3 bucket <bucket>.' — the first S3 action the export service calls on the destination is s3:GetBucketLocation, so that is the action named. Fixed text: "The IAM role ", " isn't authorized to call s3:GetBucketLocation on the S3 bucket ", "."; the variable parts are the role ARN and the bucket name taken from the request. Nothing is written to the bucket.
Emulator: the status and the error code are right, but the message names s3:PutObject instead of s3:GetBucketLocation: 'The IAM role <role arn> isn't authorized to call s3:PutObject on the S3 bucket <bucket>.'
Why: the message is the only place the refusal says which S3 action to grant. A caller told to add s3:PutObject grants a write permission and still cannot start the export, because the action actually missing is s3:GetBucketLocation; the remediation the error prescribes does not fix the refusal. An operator or client matching AWS's wording also fails to recognise the refusal.
Fix: the StartExportTask handler's destination-authorization step and its IamRoleMissingPermissions message construction — report the action whose authorization check failed, beginning with s3:GetBucketLocation.
Traces: rds__s3.cfn-deploy-rds-s3-role-policy-wrong-bucket-arn-negative.

---
Cause 3 of 4: a DB cluster's endpoint is published as a real-AWS *.rds.amazonaws.com host that is not the emulator's own host, so no client can reach the database engine.
AWS: the endpoint address RDS reports for a cluster is a host the caller can actually connect to on the reported port (3306 for Aurora MySQL, 5432 for Aurora PostgreSQL). For an emulator, the endpoint it announces must be one of its own hosts — the endpoint host, a documented own host, or a loopback address for a loopback endpoint — because that address is the whole contract of the endpoint field.
Emulator: the cluster endpoint is announced as '<cluster-id>.cluster-<hash>.<region>.rds.amazonaws.com:3306' (Aurora MySQL) and '...:5432' (Aurora PostgreSQL) — a host that is not one of the emulator's own hosts and that no customer of the emulator can resolve or reach. The harness refuses the published target rather than attempting egress to a non-emulator address (simparity.published.PublishedUrlRejected: "its host is not one of the emulator's own hosts"). This is the emulator's own divergence: it announced an address other than the one it is reached on.
Why: every data-plane use of a provisioned cluster is unreachable. A client that reads Endpoint.Address from DescribeDBClusters/DescribeDBInstances and opens a MySQL or PostgreSQL connection — which is how `LOAD DATA FROM S3` and the aws_s3 extension import paths are exercised — cannot connect at all, so the whole RDS-to-S3 data plane is unusable even when the control plane answers correctly.
Fix: endpoint-address derivation in the RDS cluster/instance state and the DescribeDBClusters / DescribeDBInstances serializers — publish the emulator's own reachable host[:port] for the engine's wire protocol instead of a synthesised amazonaws.com name.
Traces: rds__s3.deploy-cfn-rds-instance-with-s3-import-role-positive, rds__s3.deploy-rds-aurora-s3-import-extension-positive.

---
Cause 4 of 4: long-running RDS states reached from a CloudFormation-deployed Aurora cluster never settle, so the checks that poll them exhaust their budget without the state ever advancing.
AWS: these states are terminal within the polls' own budgets: a cluster snapshot becomes 'available', a StartExportTask reaches Status 'COMPLETE' and writes objects under its S3Prefix, and an AssociatedRoles entry added to a cluster reaches Status 'ACTIVE'. Each poll returns as soon as its predicate holds, so a settled emulator answers immediately and the check proceeds (as it does for the control stack in Cause 1, which reaches CREATE_COMPLETE and reports the cluster 'available' well inside the same budget).
Emulator: both checks end as 'Check timed out after 60.0s'. Neither has any fixed-duration hold or drain window — every wait is a predicate poll — so the budget is exhausted because no response ever satisfies the predicate. cfn-deploy-rds-cluster-s3-export-role-positive polls DescribeDBClusterSnapshots to 'available' and then DescribeExportTasks to 'COMPLETE'; cfn-stack-update-rds-cluster-add-s3-role-positive polls the cluster's AssociatedRoles entry to Status 'ACTIVE' before it updates the stack. Hypothesis: these two long-running states are never advanced by the emulator (the snapshot/export-task progression and the role-association Status transition), which is the only thing consistent with a predicate poll never being satisfied; the recorded evidence is the timeout itself plus the absence of any fixed wait in either check.
Why: every caller that waits for an RDS long-running state to settle — the normal shape of snapshot, export and role-association workflows — blocks forever instead of observing a terminal state, so neither the success path nor the failure path of those workflows can be reached.
Fix: the state progression behind DescribeDBClusterSnapshots / DescribeExportTasks and the AssociatedRoles Status field in the RDS cluster state — advance each to its terminal value ('available', 'COMPLETE' with objects written under S3Prefix, 'ACTIVE') rather than parking it.
Related open issues that do NOT cover these two checks: 20261112T074631Z-rds-s3-the-whole-snapshot-export-to-s3.md and 20261113T183248Z-rds-s3-5-net-new-causes-in-startexporttask.md carry other checks' replay keys; the replay report marked neither of these two as OWNED.
Traces: rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive, rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive.

## Evidence
Cause 1 — CreateStack with AWS::RDS::DBCluster{Engine: aurora-mysql, AssociatedRoles: [{FeatureName: s3Import, RoleArn: <role>}]} (the control stack, identical minus AssociatedRoles, had already reached CREATE_COMPLETE with the cluster 'available').
  Expected (AWS): the stack does not reach CREATE_COMPLETE; the resource failure reason contains 's3Import' and 'not valid for the Aurora MySQL'; DescribeDBClusters(<rejected id>) then answers DBClusterNotFoundFault with 'DBCluster <rejected id> not found.'
  Actual (emulator): AssertionError: "expected CloudFormation to reject an inline AssociatedRoles entry on AWS::RDS::DBCluster for aurora-mysql, but the stack reached CREATE_COMPLETE".

Cause 2 — StartExportTask(ExportTaskIdentifier=<id>, SourceArn=<cluster snapshot arn>, S3BucketName=<bucket outside the role's policy>, IamRoleArn=<role>, KmsKeyId=<key>, S3Prefix='export-data'). Error.Code was 'IamRoleMissingPermissions' as expected; only the message diverged.
  Expected (AWS): 'The IAM role <role arn> isn't authorized to call s3:GetBucketLocation on the S3 bucket <bucket>.'
  Actual (emulator): 'The IAM role <role arn> isn't authorized to call s3:PutObject on the S3 bucket <bucket>.'
  Recorded instance: expected "The IAM role arn:aws:iam::000000000000:role/sim2real-xsvc-role-2a32f8bb8992 isn't authorized to call s3:GetBucketLocation on the S3 bucket sim2real-xsvc-bucket-wrong-3cefe0e58c80.", got the same sentence with 's3:PutObject'. The role ARN and bucket name are per-run values from the request; the diverging fixed text is the action name.

Cause 3 — the endpoint host published for a CloudFormation-deployed Aurora cluster, read back to open a MySQL/PostgreSQL connection.
  Expected (AWS/emulator contract): an own host[:port] of the emulator (its endpoint host, a documented own host, or a loopback address) on 3306 / 5432.
  Actual (emulator): simparity.published.PublishedUrlRejected: "endpoint_host was given sim2real-xsvc-auc-f570f316b775.cluster-98dff2aef906.us-west-2.rds.amazonaws.com:3306 (published by the emulator or built by the check): its host is not one of the emulator's own hosts"; and for the PostgreSQL leg "sim2real-xsvc-aur-37ec1b533ed6.cluster-6db8b48ac378.us-west-2.rds.amazonaws.com:5432". The generated cluster ids, hashes and region are per-run values; the defect is the announced non-own host.

Cause 4 — the two polling checks.
  Expected (AWS): DescribeDBClusterSnapshots -> 'available'; DescribeExportTasks -> Status 'COMPLETE' with KeyCount > 0 under the S3Prefix; the cluster's AssociatedRoles entry -> Status 'ACTIVE'.
  Actual (emulator): rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive — "Error: Check timed out after 60.0s"; rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive — "Error: Check timed out after 60.0s". Neither check contains a fixed-duration wait; every wait is a poll that returns on its predicate.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite against the booted emulator.
2. Cause 1: CreateStack with an AWS::RDS::DBCluster (Engine: aurora-mysql, MasterUsername/MasterUserPassword) carrying AssociatedRoles: [{FeatureName: s3Import, RoleArn: <an IAM role trusting rds.amazonaws.com>}]. Observe CREATE_COMPLETE instead of a failed resource whose reason names s3Import and the Aurora MySQL engine; then DescribeDBClusters on that identifier and observe the cluster exists.
3. Cause 2: deploy a stack with two buckets and an export role whose S3 policy covers only the first; snapshot an Aurora cluster; call rds.start_export_task against the SECOND bucket with that role. Read Error.Code (IamRoleMissingPermissions, correct) and Error.Message, and observe 's3:PutObject' where AWS names 's3:GetBucketLocation'.
4. Cause 3: deploy an Aurora MySQL cluster (and an Aurora PostgreSQL one) by CloudFormation and read Endpoint.Address/Endpoint.Port from rds.describe_db_clusters. Observe a '<id>.cluster-<hash>.<region>.rds.amazonaws.com' host, which is not an address the emulator serves, so a MySQL/PostgreSQL connection cannot be opened.
5. Cause 4: with the stack from step 3 deployed, poll rds.describe_db_cluster_snapshots to 'available' then rds.describe_export_tasks to 'COMPLETE'; separately add a valid-feature role to a cluster and poll its AssociatedRoles entry's Status to 'ACTIVE'. Observe the predicates are never satisfied and the check exhausts its budget.
6. Replay the six check ids in trace_ids to confirm a fix.
