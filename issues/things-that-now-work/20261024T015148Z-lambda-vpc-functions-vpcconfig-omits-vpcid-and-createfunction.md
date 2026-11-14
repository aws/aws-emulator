Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda.xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected
Issue-Key: ec2__lambda.xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected|run#25|AssertionError|assert vc.get("VpcId") == vpc, f"VpcConfig.VpcId is {vc.get('VpcId')!r}, expected {vpc!r}"
Issue-Key: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected|_create_error#18|AssertionError|raise AssertionError(f"CreateFunction expected to be refused succeeded: {resp!r}")
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Lambda VPC functions: VpcConfig omits VpcId, and CreateFunction accepts an execution role that cannot create the ENI (2 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Two independent defects on Lambda's VPC-attachment path. Both must be closed before submitting.

Cause 1 of 2: the VpcConfig a function reports back carries no VpcId.
AWS: Lambda resolves the configured SubnetIds to their VPC and reports it. Smithy (com.amazonaws.lambda#VpcConfigResponse): members SubnetIds, SecurityGroupIds, VpcId ('The ID of the VPC.') and Ipv6AllowedForDualStack — VpcId is part of the response shape for CreateFunction/GetFunctionConfiguration/UpdateFunctionConfiguration.
Emulator: VpcConfig is returned with SubnetIds and SecurityGroupIds only; VpcConfig.VpcId is None.
Why: a client cannot tell which VPC a function is attached to without re-resolving subnets itself, and anything that asserts the ENI/VPC binding (for example before deleting a VPC) reads nothing.
Fix: VpcId resolution in the CreateFunction/GetFunctionConfiguration serializer (resolve the subnets against EC2 state).
Traces: ec2__lambda.xsvc-ec2-3-lambda-vpc-eni-dependency-violation-rejected.

Cause 2 of 2: CreateFunction with VpcConfig is accepted when the execution role cannot call ec2:CreateNetworkInterface.
AWS: refused with InvalidParameterValueException (HTTP 400) whose message names the missing CreateNetworkInterface permission; no function and no ENI come into being, and attaching AWSLambdaVPCAccessExecutionRole to the same role makes the identical request succeed. Smithy: InvalidParameterValueException is modelled on CreateFunction; the role-permission evaluation itself is recorded behavior only.
Emulator: HTTP 201 with a full function configuration (State 'Pending', StateReasonCode 'Creating'), so the function is created.
Why: the single most common Lambda-in-VPC misconfiguration is reported as success; in AWS the client is told at create time, here it is never told.
Fix: execution-role permission evaluation for the ENI on the VpcConfig path of the CreateFunction handler.
Traces: ec2__lambda.xsvc-ec2-3-lambda-vpc-role-without-create-network-interface-rejected.

## Evidence
Cause 1 — AssertionError: VpcConfig.VpcId is None, expected 'vpc-0000000000000001c'.
Cause 2 — AssertionError: CreateFunction expected to be refused succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'FunctionName': 'sim2real-xsvc-lperm-fn-...', 'Role': 'arn:aws:iam::000000000000:role/sim2real-xsvc-lperm-role-...', 'VpcConfig': {'SubnetIds': ['subnet-...'], 'SecurityGroupIds': ['sg-...']}, 'State': 'Pending', 'StateReasonCode': 'Creating', ...} (expected err['Code'] == 'InvalidParameterValueException').

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) lambda:CreateFunction with VpcConfig={'SubnetIds':[<subnet>],'SecurityGroupIds':[<sg>]} and a role that has AWSLambdaVPCAccessExecutionRole, then lambda:GetFunctionConfiguration and read VpcConfig.VpcId. (2) create an IAM role trusted by lambda with NO ec2:CreateNetworkInterface permission and lambda:CreateFunction with the same VpcConfig; then attach AWSLambdaVPCAccessExecutionRole to that role and repeat the identical call.
