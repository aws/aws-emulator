Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.create-db-instance-az-not-in-subnet-group-rejected rds.create-instance-invalid-vpc-security-group-id-rejected rds.security-group-not-supported-in-vpc-blocked rds.create-instance-missing-authorization-rejected rds.create-instance-malformed-storage-type-rejected rds.create-instance-nonexistent-option-group-rejected rds.create-instance-validation-rejected rds.master-username-length-boundary-postgres rds.create-db-proxy-invalid-subnet-rejected rds.modify-instance-engine-family-change-rejected rds.modify-instance-weak-master-password-rejected rds.delete-cluster-parameter-group-in-use rds.create-shard-group-on-non-limitless-cluster-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.create-db-instance-az-not-in-subnet-group-rejected|_error#14|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: rds.create-instance-invalid-vpc-security-group-id-rejected|_create_rejected#6|AssertionError|raise AssertionError(
Issue-Key: rds.security-group-not-supported-in-vpc-blocked|_client_error#5|AssertionError|raise AssertionError(f"AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: rds.create-instance-missing-authorization-rejected|_client_error#9|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: rds.create-instance-malformed-storage-type-rejected|_error_code#11|AssertionError|raise AssertionError(f"{unexpected_success_label}: expected a ClientError, but it SUCCEEDED and returned {result!r}")
Issue-Key: rds.create-instance-nonexistent-option-group-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded and returned {result!r}")
Issue-Key: rds.create-instance-validation-rejected|_reject#5|AssertionError|raise AssertionError(
Issue-Key: rds.master-username-length-boundary-postgres|_error#10|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: rds.create-db-proxy-invalid-subnet-rejected|_error#15|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: rds.modify-instance-engine-family-change-rejected|_error#8|AssertionError|raise AssertionError(f"expected ModifyDBInstance to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: rds.modify-instance-weak-master-password-rejected|_error#7|AssertionError|raise AssertionError(
Issue-Key: rds.delete-cluster-parameter-group-in-use|_error_code#13|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: rds.create-shard-group-on-non-limitless-cluster-rejected|_error_detail#8|AssertionError|raise AssertionError("expected a ClientError, call succeeded")
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: 10 net-new causes — create/modify/delete paths accept input and state AWS refuses (no 2xx should be returned)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Ten distinct unenforced preconditions. Each is its own cause; a fixer must close all ten.

Cause 1 of 10: CreateDBInstance ignores the AvailabilityZone/DBSubnetGroupName consistency rule. AWS: the requested AvailabilityZone must be one of the AZs covered by the named DB subnet group, otherwise the request is refused with InvalidParameterValue (HTTP 400) naming the AZ and the subnet group. Smithy: CreateDBInstance input AvailabilityZone + DBSubnetGroupName; modeled errors include InvalidSubnet and DBSubnetGroupDoesNotCoverEnoughAZs. Emulator: answers 200 and creates the instance, reporting AvailabilityZone as the requested AZ even though the subnet group does not cover it. Check: rds.create-db-instance-az-not-in-subnet-group-rejected.

Cause 2 of 10: CreateDBInstance does not resolve security-group references. AWS: an unknown VpcSecurityGroupIds entry is refused (InvalidParameterValue / InvalidVPCNetworkStateFault), a legacy DBSecurityGroups entry is refused for a VPC instance with InvalidParameterCombination ('DB Security Groups can only be associated with VPC DB Instances using API versions 2012-01-15 through 2012-04-23'), and a security group the caller is not authorized for yields AuthorizationNotFound (Smithy: AuthorizationNotFoundFault, httpError 404, awsQueryError code AuthorizationNotFound). Emulator: all three answer 200; the instance comes back with VpcSecurityGroups=[{'VpcSecurityGroupId':'sg-00000000000000bad','Status':'active'}] and with DBSecurityGroups populated on a VPC instance. Checks: rds.create-instance-invalid-vpc-security-group-id-rejected, rds.security-group-not-supported-in-vpc-blocked, rds.create-instance-missing-authorization-rejected.

Cause 3 of 10: CreateDBInstance accepts an unknown StorageType. AWS: StorageType must be one of the supported values (gp2/gp3/io1/io2/standard for the engine) and anything else is refused — Smithy models StorageTypeNotSupportedFault (httpError 400, awsQueryError code StorageTypeNotSupported) and otherwise InvalidParameterValue. Emulator: StorageType='turbo-ssd-9000' is accepted with 200 and stored verbatim (the response echoes StorageType 'turbo-ssd-9000' and StorageThroughput 0). Check: rds.create-instance-malformed-storage-type-rejected.

Cause 4 of 10: CreateDBInstance never resolves OptionGroupName. AWS: a non-existent option group is refused with OptionGroupNotFoundFault (Smithy: httpError 404, awsQueryError code OptionGroupNotFoundFault), message 'Specified OptionGroupName: <name> not found.' Emulator: answers 200 with OptionGroupMemberships=[{'OptionGroupName':'<the non-existent name>','Status':'in-sync'}]. Check: rds.create-instance-nonexistent-option-group-rejected.

Cause 5 of 10: MasterUsername rules are unenforced. AWS refuses a reserved master username (e.g. 'rdsadmin' for PostgreSQL) and a username longer than the engine's limit with InvalidParameterValue (HTTP 400) naming the parameter. Emulator: both are accepted with 200. Checks: rds.create-instance-validation-rejected, rds.master-username-length-boundary-postgres.

Cause 6 of 10: CreateDBProxy does not resolve VpcSubnetIds. AWS refuses a subnet that does not exist (or is in another VPC) with InvalidSubnet (HTTP 400). Emulator: answers 200 and returns a DBProxy whose VpcSubnetIds echo the bogus ids, Status 'creating'. Check: rds.create-db-proxy-invalid-subnet-rejected.

Cause 7 of 10: ModifyDBInstance accepts an engine-family change. AWS refuses changing Engine across families with InvalidParameterCombination (HTTP 400) — the modeled surface has no Engine member for a cross-family move. Emulator: answers 200 with DBInstanceStatus 'modifying' while the record keeps Engine 'postgres' and a mysql parameter group, leaving the instance internally inconsistent (EngineVersion '8.4.4' on Engine 'postgres'). Check: rds.modify-instance-engine-family-change-rejected.

Cause 8 of 10: ModifyDBInstance applies a master password that violates the engine's password policy. AWS refuses with InvalidParameterValue (HTTP 400) naming MasterUserPassword. Emulator: answers 200 with DBInstanceStatus 'modifying'. Check: rds.modify-instance-weak-master-password-rejected.

Cause 9 of 10: DeleteDBClusterParameterGroup deletes a group that is still attached to a cluster. AWS refuses with InvalidDBParameterGroupStateFault (HTTP 400) because the group is in use. Emulator: answers 200 with an empty body and removes it. Check: rds.delete-cluster-parameter-group-in-use.

Cause 10 of 10: CreateDBShardGroup is accepted on a cluster that is not Limitless. AWS refuses with InvalidParameterValue/InvalidDBClusterStateFault because the cluster's ClusterScalabilityType is not 'limitless'. Emulator: the call succeeds. Check: rds.create-shard-group-on-non-limitless-cluster-rejected.

Why: a client gets a 200 and a resource record for requests real AWS never admits, so negative-path code is never exercised and the emulator's state diverges structurally (instances referencing non-existent option groups, security groups and storage types; clusters whose parameter group was deleted under them). Fix: the request-validation stage of each named operation's handler in the rds crate (create_db_instance.rs, create_db_proxy.rs, modify_db_instance.rs, delete_db_cluster_parameter_group.rs, create_db_shard_group.rs) plus the cross-resource resolution it must perform against EC2/option-group/parameter-group state. Related open issue 20261029T214632Z-rds-accepts-nine-requests-aws-refuses-missing-request.md covers a different set of accepted requests and does not carry these replay keys.

## Evidence
Expected: HTTP 400 with the modeled refusal for each input (InvalidParameterValue, InvalidParameterCombination, StorageTypeNotSupported, OptionGroupNotFoundFault, AuthorizationNotFound, InvalidSubnet, InvalidDBParameterGroupStateFault).
Actual, verbatim from replay: 'AssertionError: expected a rejection, but the call SUCCEEDED and returned {...AvailabilityZone: us-west-2c...}' (az-not-in-subnet-group); 'expected CreateDBInstance to be rejected for the bogus VpcSecurityGroupId, but it succeeded ... VpcSecurityGroups: [{VpcSecurityGroupId: sg-00000000000000bad, Status: active}]'; 'CreateDBInstance with StorageType=\'turbo-ssd-9000\': expected a ClientError, but it SUCCEEDED'; 'expected a ClientError but the call succeeded ... OptionGroupMemberships: [{OptionGroupName: sim2real-does-not-exist-option-group, Status: in-sync}]'; 'reserved master username: CreateDBInstance was ACCEPTED; real RDS refuses this input (mutation={MasterUsername: rdsadmin})'; 'expected the call to be rejected, but it succeeded' (missing authorization, master-username length); 'AWS refuses this call, but it succeeded ... DBSecurityGroups: [{DBSecurityGroupName: ..., Status: active}]'; 'expected ModifyDBInstance to be rejected, but it SUCCEEDED ... DBInstanceStatus: modifying'; 'the call was expected to be REJECTED but succeeded' (weak password); 'expected this call to be rejected, but it SUCCEEDED: {ResponseMetadata: {HTTPStatusCode: 200}}' (parameter group in use); 'expected a ClientError, call succeeded' (shard group).

## Steps to Reproduce
For each cause, issue the named call against the emulator and observe HTTP 200 instead of the modeled 400:
1. create_db_instance(AvailabilityZone=<az not covered by DBSubnetGroupName>).
2. create_db_instance(VpcSecurityGroupIds=['sg-00000000000000bad']); create_db_instance(DBSecurityGroups=[<name>], DBSubnetGroupName=<vpc group>).
3. create_db_instance(StorageType='turbo-ssd-9000').
4. create_db_instance(OptionGroupName=<name that does not exist>).
5. create_db_instance(MasterUsername='rdsadmin'); create_db_instance(MasterUsername=<over the engine limit>).
6. create_db_proxy(VpcSubnetIds=[<subnet id that does not exist>]).
7. modify_db_instance(DBInstanceIdentifier=<postgres instance>, Engine='mysql'-family change).
8. modify_db_instance(MasterUserPassword=<below policy>).
9. delete_db_cluster_parameter_group(<group attached to a live cluster>).
10. create_db_shard_group(DBClusterIdentifier=<non-limitless cluster>).
