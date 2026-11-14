Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__rds.cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found|_assert_instance#31|TimeoutError|raise TimeoutError(f"run budget ended with {instance_id} at {got!r}, not {want!r}")
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# CloudFormation in-place tag update on AWS::RDS::DBInstance never reaches the provisioned instance's tags

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: a stack UPDATE that changes only the Tags of an AWS::RDS::DBInstance is reported as complete but the instance's tag set still holds the pre-update values.
AWS: an AWS::RDS::DBInstance whose Tags property changes is updated in place (no replacement). CloudFormation calls RDS AddTagsToResource/RemoveTagsFromResource, so once the stack reaches UPDATE_COMPLETE, DescribeDBInstances/ListTagsForResource for that instance report the NEW tag values alongside the aws:cloudformation:* tags, and the DBInstanceIdentifier is unchanged. (recorded behavior only — the tag propagation is a CloudFormation/RDS provisioning behavior, not a modeled constraint.)
Emulator: after the update the same instance keeps the create-phase tag values; the check's poll on the instance's tag set never converges and ends with the run budget exhausted. Every other attribute of the instance (identifier, status available, engine, class, storage, subnet group, security groups) matches; only the mutated tag value lags.
Why: a client that uses stack-managed tags as the source of truth (cost allocation, backup selection, tag-based IAM conditions) reads stale values after a successful update, so a deploy that "succeeded" has not taken effect at the resource.
Fix: the CloudFormation provisioning path for AWS::RDS::DBInstance in-place updates — the step that applies the new Tags to the RDS instance (and the RDS tag state those reads come from).
Traces: cloudformation__ec2__rds.cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found.

## Evidence
TimeoutError: run budget ended with sim2real-cfndb-a-89df2385ce31 at {... 'Tags': {'sim2real-step': 'create', 'sim2real': 'cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found', 'aws:cloudformation:stack-name': ..., 'aws:cloudformation:stack-id': ..., 'aws:cloudformation:logical-id': 'Database'}}, not {... 'Tags': {'sim2real': 'cfn-rds-db...}} — the instance still reports the create-phase 'sim2real-step' value after the in-place tag update; all other reported members are identical between got and want.

## Steps to Reproduce
1. CreateStack with an AWS::RDS::DBInstance (postgres, db.t4g.micro, a DB subnet group over two subnets) tagged sim2real-step=create; wait for CREATE_COMPLETE.
2. UpdateStack with the identical template except the DBInstance's Tags value changed; wait for UPDATE_COMPLETE.
3. DescribeDBInstances / ListTagsForResource on the same DBInstanceIdentifier and read the tag set — it still carries the step-1 value.
Replay: cloudformation__ec2__rds.cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found.
