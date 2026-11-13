Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive
Issue-Key: cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive|run#26|AssertionError|assert assoc_message == f"Association {association_id} does not exist.", (
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# ssm DescribeAssociation: the AssociationDoesNotExist message for an association a stack delete removed is emulator prose, not AWS's "Association <id> does not exist."

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssociation

## Description
One net-new cause, reached through a CloudFormation stack delete that removes an SSM association.

Cause 1 of 1: DescribeAssociation's not-found message diverges.
AWS: DescribeAssociation for an association id that no longer exists raises AssociationDoesNotExist. Smithy: DescribeAssociation declares error AssociationDoesNotExist (client error, httpError 400). Its message is "Association <AssociationId> does not exist." — the fixed text is "Association " ... " does not exist." and the variable part is the AssociationId from the request.
Emulator: the same call answers the error class with the message "The specified association doesn't exist." — emulator-authored prose that names no association id.
Why: a client that confirms a delete by matching the not-found text, or that extracts the association id from the message for its log/report, cannot recognise the refusal; the id that identifies which association is gone is lost.
Fix: the not-found refusal text in the DescribeAssociation handler (the SSM crate's describe_association operation) — interpolate the requested AssociationId into AWS's sentence.
Traces: cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive.

## Evidence
AssertionError: DescribeAssociation on the deleted association: unexpected message "The specified association doesn't exist."
The check asserts assoc_message == f"Association {association_id} does not exist." (check_cloudformation__ec2__ssm_cli_script_cfn_stack_delete_removes_ssm_association_positive.py line 86).

## Steps to Reproduce
1. CreateStack with an AWS::SSM::Association targeting an EC2 instance; note the association id.
2. DeleteStack and wait for DELETE_COMPLETE.
3. Call ssm DescribeAssociation(AssociationId=<that id>): the error message is "The specified association doesn't exist." instead of "Association <id> does not exist.".
