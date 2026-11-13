Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptunedata.loader-job-unknown-loadid-rejected-by-get-and-cancel
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata|AddRoleToDBCluster|NotImplemented
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# Neptune AddRoleToDBCluster answers 501 NotImplemented, so no neptunedata bulk-loader scenario can be set up (loadId refusal paths unreachable)

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBCluster

## Description
Cause 1 of 1: associating the bulk-load IAM role with a Neptune DB cluster is unimplemented, which blocks the whole neptunedata loader surface.
AWS: Smithy (com.amazonaws.neptune, amazonrdsv19) models AddRoleToDBCluster with required DBClusterIdentifier and RoleArn plus optional FeatureName, an EMPTY output (output shape Unit — HTTP 200 with the query-protocol response wrapper and no members), and client errors DBClusterNotFoundFault (404), DBClusterRoleAlreadyExists (400), DBClusterRoleQuotaExceeded (400) and InvalidDBClusterStateFault (400). Associating a role that is not yet attached to an available cluster therefore succeeds with 200 and no body members; it is the prerequisite for StartLoaderJob, which is in turn what makes the modelled loadId refusal paths of GetLoaderJobStatus and CancelLoaderJob (BulkLoadIdNotFoundException 404 / InvalidParameterException 400) reachable at all.
Emulator: the neptune crate's handler is a scaffold stub — services/service-com-amazonaws-neptune-amazonrdsv19/src/ops/add_role_to_db_cluster.rs:4 carries "SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented" — so the call fails with ClientError (NotImplemented) "operation AddRoleToDBCluster is scaffolded but not implemented in this emulator". The open issue 20261104T053605Z-rds-addroletodbcluster-stub.md names the RDS crate's operation, NOT this one: the neptune service has its own handler file and is unaffected by that item, so this is a separate work item.
Why: any client that follows AWS's documented bulk-load setup (create role, attach to cluster with AddRoleToDBCluster, then StartLoaderJob with that role ARN) cannot proceed; the loader data-plane behaviour — including the not-found refusals for a well-formed but never-issued loadId — is unreachable and silently untested.
Fix: the neptune AddRoleToDBCluster handler and the DB-cluster associated-roles state it must write (so DescribeDBClusters reports the role and StartLoaderJob can authorize against it).
Traces: neptunedata.loader-job-unknown-loadid-rejected-by-get-and-cancel. NOTE: this check is listed under the neptunedata service heading in the suite report, which is the service token recorded here, but the defective handler lives in the neptune crate.

## Evidence
Expected: neptune AddRoleToDBCluster(DBClusterIdentifier=<the check's cluster>, RoleArn=<the IAM role it just created>) returns HTTP 200 with an empty result, after which StartLoaderJob is accepted and GetLoaderJobStatus/CancelLoaderJob refuse an unissued loadId with the modelled not-found class and its "does not exist" message naming that loadId.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddRoleToDBCluster operation: operation AddRoleToDBCluster is scaffolded but not implemented in this emulator — raised during the check's setup, so no loader-job call was ever made and the loadId refusal assertions were never reached.
Source: services/service-com-amazonaws-neptune-amazonrdsv19/src/ops/add_role_to_db_cluster.rs:3-6 (SCAFFOLDED, NOT IMPLEMENTED marker).

## Steps to Reproduce
1. Create a Neptune DB cluster + db.t4g.medium instance in the private network with IAM database authentication (the check's _own_neptune_cluster helper).
2. Create an S3 bucket in the cluster's Region with one object under data/, and an IAM role trusted by rds.amazonaws.com with s3:GetObject/s3:ListBucket on that bucket.
3. Call neptune AddRoleToDBCluster(DBClusterIdentifier=<cluster>, RoleArn=<role arn>) — observe HTTP 501 NotImplemented instead of an empty 200.
4. Replay: run_behavioral_suite and re-check neptunedata.loader-job-unknown-loadid-rejected-by-get-and-cancel, which should then proceed to StartLoaderJob and the GetLoaderJobStatus/CancelLoaderJob unknown-loadId refusals.
