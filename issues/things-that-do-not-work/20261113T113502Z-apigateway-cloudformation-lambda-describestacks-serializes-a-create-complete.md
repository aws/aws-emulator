Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure
Issue-Key: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure|_deploy_cfn#18|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 2bc4f9350f4794788afdc3a762d704c686e40d96

# apigateway__cloudformation__lambda: DescribeStacks serializes a CREATE_COMPLETE stack's Outputs entries without OutputValue, so reading a declared stack output crashes the client

> _Note: this issue was identified against the code base at commit `2bc4f9350f4794788afdc3a762d704c686e40d96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause, reached by the positive-control leg of the custom-resource rollback check. This check is NOT marked OWNED or ALREADY FILED in the replay report: its earlier replay key was the rollback assertion (`run#8|AssertionError|expected ROLLBACK_COMPLETE`), and that leg now PASSES — the stack reaches ROLLBACK_COMPLETE, the sibling SNS topic is deleted and GetTopicAttributes answers NotFound / 'Topic does not exist'. The failure has moved to the positive control, with a different key (`_deploy_cfn#3|KeyError|OutputValue`).

Cause 1 of 1: DescribeStacks returns Stack.Outputs entries that carry OutputKey but no OutputValue member at all.

AWS: Smithy services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json — DescribeStacks -> Stack$Outputs: list<com.amazonaws.cloudformation#Output>, and Output has members OutputKey ('The key associated with the output'), OutputValue ('The value associated with the output'), Description and ExportName. AWS resolves every declared template Output and populates OutputValue with the resolved value; a stack whose output cannot be resolved does not reach CREATE_COMPLETE. Clients (and the AWS CLI's `--query 'Stacks[0].Outputs[?OutputKey==...].OutputValue'`) index the member unconditionally.

Emulator: for a stack that has reached CREATE_COMPLETE, each entry of Stacks[0].Outputs is serialized with OutputKey present and OutputValue absent. Here the template declares two outputs — RequestType, whose Value is {'Fn::GetAtt': ['MyResource', 'RequestType']} on a Lambda-backed Custom:: resource, and TopicArn, whose Value is {'Ref': 'AlwaysTopic'} on an AWS::SNS::Topic — and reading OutputValue raises KeyError: 'OutputValue' immediately after the stack settles.

Why: a client relying on AWS's behavior crashes rather than degrading. Every consumer of a stack's declared outputs breaks: it cannot learn the topic ARN, queue URL, function name, table name or API endpoint the template published, so nothing the stack created can be addressed through the contract the template defines, and custom-resource response data surfaced through Fn::GetAtt is unreadable.

Fix: the CloudFormation DescribeStacks response serializer and the stack-output resolution in the stack state that builds Stack.Outputs — resolve each declared Output's Value (Ref, Fn::GetAtt, including custom-resource response Data) and emit it as the OutputValue member.

Related open issues that describe the same symptom but do NOT carry this check's replay key, named for the fixer: 20261105T022225Z-apigateway-cloudformation-lambda-3-net-new-causes-describestacks.md (its cause 1; it names this check only under the now-passing rollback key), 20261105T022123Z-apigateway-cloudformation-lambda-4-net-new-root-causes.md, 20261104T232822Z-cloudformation-lambda-s3-sqs-2-net-new-root.md, 20261105T022225Z-cloudformation-lambda-2-net-new-causes-describestacks-outputs.md and 20261109T154607Z-cloudformation-lambda-signer-2-net-new-causes-stack.md. None of them is resolved, and none covers this replay key.

Traces: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[error] apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure
  File "check_apigateway__cloudformation__lambda_cfn_stack_rollback_on_custom_resource_failure.py", line 78, in run
    ok = _deploy_cfn(ctx, template, parameters={"ForceFail": "false"})
  File "...", line 258, in _deploy_cfn
    outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'

The KeyError is raised only after `stack["StackStatus"] == "CREATE_COMPLETE"` has been asserted (line 254 of the check), so the stack settled green and DescribeStacks answered 200 — the divergence is purely in the serialized Output entries.

Expected (AWS): Stacks[0].Outputs == [{"OutputKey": "RequestType", "OutputValue": "Create"}, {"OutputKey": "TopicArn", "OutputValue": "<arn of the AWS::SNS::Topic>"}] — the keys are the template's fixed Output names; the values are resolved from the stack's own state and are per-run.
Actual (emulator): each entry carries OutputKey but no OutputValue member.

The negative leg of the same check now passes: ROLLBACK_COMPLETE is reached, MyResource has a CREATE_FAILED event, AlwaysTopic has a CREATE_COMPLETE event, and the rolled-back topic answers NotFound with 'Topic does not exist'.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateStack with Capabilities CAPABILITY_IAM,CAPABILITY_NAMED_IAM on a template that declares: an IAM execution role for lambda.amazonaws.com; an AWS::Lambda::Function (python3.12, inline ZipFile) acting as a cfnresponse custom-resource provider that answers SUCCESS with Data {'RequestType': event['RequestType']}; a Custom::Sim2RealTest resource whose ServiceToken is Fn::GetAtt of that function; an AWS::SNS::Topic; and Outputs {RequestType: {Value: {Fn::GetAtt: [MyResource, RequestType]}}, TopicArn: {Value: {Ref: AlwaysTopic}}}.
3. Poll DescribeStacks until StackStatus is CREATE_COMPLETE.
4. Read Stacks[0].Outputs and index "OutputValue" on each entry — the member is absent (KeyError), where AWS returns the resolved value for both outputs.
5. Re-replay apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure.
