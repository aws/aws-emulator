Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-missing-sink-grant-blocks-propagation
Issue-Key: cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-missing-sink-grant-blocks-propagation|_cdk#46|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__dynamodb__lambda: stack delete ends DELETE_FAILED because the role's inline policies are never removed first — DeleteRole is refused with "Cannot delete entity, must delete policies first."

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: CloudFormation deletes an AWS::IAM::Role before detaching/deleting the inline policies it owns, so IAM refuses the delete and the stack is stuck in DELETE_FAILED.
AWS:      recorded behavior only for the ordering; the refusal itself is modeled — IAM DeleteRole answers DeleteConflictException (HTTP 409) with the message `Cannot delete entity, must delete policies first.` when the role still has policies, which is exactly the text the emulator returned, so the error wording is right. What AWS does differently is the delete order: CloudFormation deletes the role's dependent policy resources (the AWS::IAM::Policy / inline policy a CDK-synthesized role owns) before the role itself, so DeleteRole is never issued while a policy is attached and the stack reaches DELETE_COMPLETE. `cdk destroy` therefore exits 0.
Emulator: DeleteStack reaches DELETE_FAILED with `The following resource(s) failed to delete: [BadConsumerServiceRoleC4A1D63C]` and the resource reason `Resource handler returned message: "Cannot delete entity, must delete policies first. (Service: Iam, Status Code: 409 ...)" (HandlerErrorCode: ResourceConflict)`, and `cdk destroy --force` exits 1.
Why:      a client cannot tear down any stack whose role carries an inline policy — which is every CDK stack with a grant. The stack is left in DELETE_FAILED with orphaned IAM entities, the role name is then taken for any re-deploy, and automated cleanup (cdk destroy, CI teardown) fails even when the deploy under test succeeded.
Fix:      the CloudFormation deletion-order/dependency graph in the stack-delete path (dependent policy resources must be deleted before the role they attach to), and the AWS::IAM::Role delete handler, which must remove the inline policies the role resource itself declares before calling DeleteRole.
Traces:   cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-missing-sink-grant-blocks-propagation.

## Evidence
Expected: `cdk destroy --force` on the deployed stack exits 0 and the stack reaches DELETE_COMPLETE.
Actual:
  AssertionError: `cdk destroy` rc=1, expected 0:
  '... ❌ <stack>: destroy failed DeploymentError: The stack named <stack> is in a failed state. You may need to delete it from the AWS console : DELETE_FAILED (The following resource(s) failed to delete: [BadConsumerServiceRoleC4A1D63C]. ): Resource handler returned message: "Cannot delete entity, must delete policies first. (Service: Iam, Status Code: 409, Request ID: ...) (SDK Attempt Count: 1)" (RequestToken: ..., HandlerErrorCode: ResourceConflict)'

The IAM message text and 409 status match AWS; the divergence is the stack status (DELETE_FAILED instead of DELETE_COMPLETE) caused by issuing DeleteRole while the role still owns an inline policy. Stack name, request id and request token are per-run values.

## Steps to Reproduce
1. Synthesize and deploy a CDK stack with a DynamoDB table with a stream and a Lambda consumer whose role gets an inline grant (so the synthesized role carries an inline policy).
2. Run `cdk destroy --force` (or DeleteStack + wait).
3. Observe DELETE_FAILED on the stack, the role listed as the resource that failed to delete, and the IAM refusal "Cannot delete entity, must delete policies first." (409) on the resource event.
