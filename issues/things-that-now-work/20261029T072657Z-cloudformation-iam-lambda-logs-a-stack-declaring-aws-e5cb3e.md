Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|run#16|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=collide_fn, WaiterConfig={"Delay": 2, "MaxAttempts": 150})
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission|run#14|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=fn, WaiterConfig={"Delay": 2, "MaxAttempts": 150})
Issue-Commit: 37c02434d582c8370b608107fbfd29504c12ec2b

# cloudformation__iam__lambda__logs: a stack declaring AWS::Lambda::Function reaches CREATE_COMPLETE while the function was never created in Lambda (1 cause)

> _Note: this issue was identified against the code base at commit `37c02434d582c8370b608107fbfd29504c12ec2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause. Cause count: 1.

=== Cause 1 of 1: the CloudFormation resource provider for AWS::Lambda::Function reports success without creating the function ===

AWS: Smithy com.amazonaws.cloudformation#StackStatus enumerates CREATE_COMPLETE, and that status is the contract that every resource declared in the template has been created in its owning service. For a template holding AWS::IAM::Role + AWS::Logs::LogGroup + AWS::Lambda::Function (the function DependsOn the role/group), once DescribeStacks reports StackStatus CREATE_COMPLETE the function exists in Lambda: lambda GetFunctionConfiguration/GetFunction on the template's FunctionName answer 200, the function reaches State=Active, and botocore's FunctionActiveV2 waiter (which polls GetFunctionConfiguration) returns. lambda's ResourceNotFoundException (HTTP 404) with message `Function not found: arn:aws:lambda:<region>:<account-id>:function:<function-name>` is AWS's refusal for a function that does NOT exist — its fixed text is `Function not found: ` and its variable part is the function ARN assembled from the request's FunctionName plus the caller's region and account id.

Emulator: both checks deploy such a template, DescribeStacks reports CREATE_COMPLETE (the checks' `assert status == 'CREATE_COMPLETE'` passes), and the immediately following `lambda.get_waiter('function_active_v2').wait(FunctionName=<the template's FunctionName>)` fails because GetFunctionConfiguration answers ResourceNotFoundException `Function not found: arn:aws:lambda:<region>:<account-id>:function:<function-name>` for the very function the stack just claimed to create. The stack's CFN-side resource and the Lambda service's state are not the same object, so the function provider is an inert no-op that nonetheless settles the stack as successful.

Why: a client cannot deploy a Lambda function as infrastructure-as-code at all. The stack reports success, so no caller has any signal that nothing was provisioned; the failure surfaces far from its cause, at the first Lambda API call or waiter. Everything the two checks were written to grade is unreachable behind this: neither can get as far as the implicit `/aws/lambda/<fn>` log group the function creates on first invoke, the CREATE_FAILED + DetailedStatus VALIDATION_FAILED stack outcome (Smithy com.amazonaws.cloudformation#DetailedStatus does model VALIDATION_FAILED) for a template that re-declares an existing log group, the NAME_CONFLICT_VALIDATION VALIDATION_ERROR event with reason `Resource of type 'AWS::Logs::LogGroup' with identifier '<log-group-name>' already exists.`, nor the denied-permission arm. Those expectations are blocked, not separately observed, so they are not filed as additional causes.

Fix: the CloudFormation resource provider for AWS::Lambda::Function — it must create the function through the Lambda service's state (and propagate its name/ARN as the physical resource id) before the stack is allowed to reach CREATE_COMPLETE, and a provider failure must drive the stack to CREATE_FAILED/ROLLBACK_COMPLETE instead of success.

Related open issues that describe the same shape of defect for OTHER resource types and do not carry these two replay keys: 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md (ELBv2 TargetGroup/TrustStore, ECS Service, unresolved stack Outputs) and 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md. Neither names AWS::Lambda::Function.

Traces: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected, cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission.

## Evidence
Both checks, verdict [error]:

  Error: File "context.py", line 123, in wrapper / File "waiter.py", line 363, in wait
  botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:<region>:<account-id>:function:<function-name>

(the recorded runs name sim2real-xsvc-collfn-<suffix> and sim2real-xsvc-ctlfn-<suffix>; the generated function name and the account/region in the ARN are per-run values, the requirement is the status/code/fixed message text.)

Expected (real AWS): DescribeStacks StackStatus == CREATE_COMPLETE, then GetFunctionConfiguration(FunctionName=<fn>) -> 200 with State eventually Active, and the FunctionActiveV2 waiter returns.
Actual (emulator): DescribeStacks StackStatus == CREATE_COMPLETE (so the stack claims success), then GetFunctionConfiguration(FunctionName=<fn>) -> 404 ResourceNotFoundException "Function not found: arn:aws:lambda:<region>:<account-id>:function:<fn>".

## Steps to Reproduce
1. Boot the emulator.
2. cloudformation.create_stack with a template containing: AWS::IAM::Role (trust lambda.amazonaws.com), AWS::Logs::LogGroup (LogGroupName /aws/lambda/${FunctionName}, RetentionInDays 3), and AWS::Lambda::Function (FunctionName from a parameter, Runtime python3.12, Handler index.handler, Role Fn::GetAtt ExecutionRole.Arn, Code.ZipFile inline), the function DependsOn the log group.
3. Poll cloudformation.describe_stacks until a terminal StackStatus — observe CREATE_COMPLETE.
4. lambda.get_waiter('function_active_v2').wait(FunctionName=<the FunctionName parameter>) — observe ResourceNotFoundException "Function not found: arn:aws:lambda:<region>:<account>:function:<fn>". lambda.get_function on the same name fails the same way.
5. Re-replay the two check ids in trace_ids to confirm a fix.
