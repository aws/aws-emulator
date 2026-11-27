Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda.nodejs-function-sdk-v3-describes-its-own-vpc-and-security-group
Issue-Key: ec2__lambda.nodejs-function-sdk-v3-describes-its-own-vpc-and-security-group|run#12|AssertionError|assert "error" not in out, f"the function's EC2 call failed: {out!r}"
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__lambda: the Node.js Lambda runtime's bundled AWS SDK binds only five EC2 operations, so a function calling DescribeVpcs faults SdkCallNotMade instead of reaching EC2

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the AWS SDK the in-sandbox Node.js Lambda runtime supplies exposes almost no EC2 surface, so a function that describes its own VPC cannot make the call at all.

AWS: recorded behavior only for the runtime. A VPC-attached Node.js function using AWS SDK v3 can call any EC2 API its execution role permits; the recording has the handler call DescribeVpcs and DescribeSecurityGroups and return its own VPC id and security group, with no error key in the payload.

Emulator: the invocation returns an emulator-internal fault instead of an EC2 reply - `{'error': 'SdkCallNotMade', 'message': 'the AWS SDK this runtime supplies binds no DescribeVpcs call on Amazon EC2; the operations it binds are DescribeSecurityGroups, AuthorizeSecurityGroupIngress, AuthorizeSecurityGroupEgress, RevokeSecurityGroupIngress, RevokeSecurityGroupEgress'}`. Only those five EC2 operations exist inside the runtime's SDK.

Why: any Lambda function that reads EC2 state - the ordinary pattern for VPC-aware functions, custom resources and remediation handlers - fails on its first SDK call, and the payload carries a non-AWS error name (`SdkCallNotMade`) that no client error-handling path recognises.

Fix: the Lambda runtime's bundled AWS SDK EC2 client binding - route the sandbox SDK's calls through the same EC2 operation surface the emulator serves on the wire instead of a hand-listed subset.

Note: sibling issues already describe the bundled-SDK gap for S3/IAM/FSx in the Lambda runtimes (open: 20261111T040849Z-lambda-s3-the-lambda-node-js-runtime-s-bundled-sdk.md, 20261127T101452Z-fsx-lambda-the-in-sandbox-lambda-aws-sdk.md). Neither covers the EC2 client.

Traces: ec2__lambda.nodejs-function-sdk-v3-describes-its-own-vpc-and-security-group.

## Evidence
Expected (AWS): the invocation payload names the function's own VpcId and security group with no 'error' key.

Actual (emulator): "the function's EC2 call failed: {'error': 'SdkCallNotMade', 'message': 'the AWS SDK this runtime supplies binds no DescribeVpcs call on Amazon EC2; the operations it binds are DescribeSecurityGroups, AuthorizeSecurityGroupIngress, AuthorizeSecurityGroupEgress, RevokeSecurityGroupIngress, RevokeSecurityGroupEgress'}".

## Steps to Reproduce
1. CreateFunction (nodejs runtime) with VpcConfig naming a subnet and security group, and a handler that uses AWS SDK v3 to call EC2 DescribeVpcs and DescribeSecurityGroups.
2. Invoke the function.
3. Observe the payload carries {'error': 'SdkCallNotMade', ...} naming DescribeVpcs as unbound instead of the VPC's details.
