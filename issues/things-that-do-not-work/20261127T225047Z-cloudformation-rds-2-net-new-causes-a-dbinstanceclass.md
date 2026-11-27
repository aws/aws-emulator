Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds.cfn-instance-class-update-applies-in-place cloudformation__rds.cfn-read-replica-from-source-instance
Issue-Key: cloudformation__rds.cfn-instance-class-update-applies-in-place|run#17|AssertionError|assert inst["DBInstanceClass"] == "db.t3.small", (
Issue-Key: cloudformation__rds.cfn-read-replica-from-source-instance|run#8|AssertionError|raise AssertionError(f"stack ended {status!r}: {reasons!r}")
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + rds: 2 net-new causes — a DBInstanceClass update reports UPDATE_COMPLETE without changing the instance, and SourceDBInstanceIdentifier read replicas are refused

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBInstance

## Description
Two net-new causes on CloudFormation-provisioned RDS resources.

Cause 1 of 2: a stack update that changes AWS::RDS::DBInstance DBInstanceClass reports UPDATE_COMPLETE while the live instance keeps its old class.
AWS: DBInstanceClass is a mutable property; CloudFormation calls ModifyDBInstance, and once the stack reaches UPDATE_COMPLETE DescribeDBInstances reports the NEW DBInstanceClass with PendingModifiedValues empty (the class is either applied immediately or applied and settled before the resource completes). Recorded behaviour only for the ordering.
Emulator: the stack reports UPDATE_COMPLETE but the instance still reads the original class (e.g. `db.t3.micro`) and PendingModifiedValues is empty — nothing is pending and nothing was applied, so the change was silently dropped.
Why: a successful stack update is a client's guarantee that the declared configuration is live. A caller that scales an instance up through CloudFormation is told the resize succeeded while the database keeps the old instance class, so capacity planning and cost reporting both read the wrong value and there is no pending-change signal to recover from.
Fix: the AWS::RDS::DBInstance resource handler's update path — DBInstanceClass must be carried to ModifyDBInstance and reflected in the instance state before the resource reports UPDATE_COMPLETE.

Cause 2 of 2: an AWS::RDS::DBInstance that declares SourceDBInstanceIdentifier (a read replica) cannot be created.
AWS: the property makes CloudFormation call CreateDBInstanceReadReplica against the named source; the replica is created and the stack reaches CREATE_COMPLETE.
Emulator: the resource fails with the emulator-authored reason `Resource handler returned message: "this emulator does not send AWS::RDS::DBInstance's SourceDBInstanceIdentifier property to the service that owns the type, so the resource was not created" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` and the stack settles ROLLBACK_COMPLETE.
Why: read replicas are the standard way a template expresses a read-scaling or cross-AZ topology. Any such template fails to deploy, and the reason text is not a message AWS ever emits, so a client cannot classify the failure.
Fix: the AWS::RDS::DBInstance resource handler — route a create that carries SourceDBInstanceIdentifier to the read-replica create path instead of refusing it.

Traces: cloudformation__rds.cfn-instance-class-update-applies-in-place; cloudformation__rds.cfn-read-replica-from-source-instance.

## Evidence
Cause 1: "stack reports UPDATE_COMPLETE but <db-instance-id> is 'db.t3.micro' (pending {})".
Cause 2: "stack ended 'ROLLBACK_COMPLETE': [('Replica', 'Resource handler returned message: \"this emulator does not send AWS::RDS::DBInstance's SourceDBInstanceIdentifier property to the service that owns the type, so the resource was not created\" (..., HandlerErrorCode: InvalidRequest)')]".

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::RDS::DBInstance whose DBInstanceClass is db.t3.micro; wait for CREATE_COMPLETE.
2. UpdateStack changing only DBInstanceClass to a larger class; wait for UPDATE_COMPLETE.
3. DescribeDBInstances and read DBInstanceClass and PendingModifiedValues — expect the new class.

Cause 2:
1. CreateStack with a source AWS::RDS::DBInstance and a second AWS::RDS::DBInstance whose Properties set SourceDBInstanceIdentifier to the source.
2. Expect CREATE_COMPLETE and a replica readable through DescribeDBInstances.
