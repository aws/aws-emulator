Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3__ssm.ssm-run-command-output-lands-in-the-named-s3-bucket
Issue-Key: ec2__s3__ssm.ssm-run-command-output-lands-in-the-named-s3-bucket|run#14|AssertionError|assert inv["StandardOutputUrl"], f"StandardOutputUrl must name the S3 object, got {inv['StandardOutputUrl']!r}"
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ec2__s3__ssm: a command invocation's StandardOutputUrl is empty, so Run Command output in the named S3 bucket is unaddressable

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommandInvocation

## Description
Cause 1 of 1: StandardOutputUrl is not set for a command with S3 output.
AWS: SendCommand with OutputS3BucketName/OutputS3KeyPrefix writes each invocation's stdout to `s3://<bucket>/<prefix>/<command id>/<instance id>/awsrunShellScript/<step>/stdout` and GetCommandInvocation/ListCommandInvocations return that object's URL in StandardOutputUrl (StandardErrorUrl likewise). The members are the object's URL built from the request's bucket/prefix and the command and instance ids.
Emulator: StandardOutputUrl is the empty string, so the output object cannot be located from the API response.
Why: the documented way to collect large command output is unusable — a client has nothing to fetch, and cannot tell 'no output' from 'URL not reported'.
Fix: the S3-output path of the Run Command invocation runner and the StandardOutputUrl/StandardErrorUrl serializers (services/service-com-amazonaws-ssm-amazonssm/src/run_command/… and ops/get_command_invocation.rs).
Traces: ec2__s3__ssm.ssm-run-command-output-lands-in-the-named-s3-bucket.

## Evidence
'StandardOutputUrl must name the S3 object, got \'\''.

## Steps to Reproduce
Create a bucket, SendCommand AWS-RunShellScript to a managed node with OutputS3BucketName/OutputS3KeyPrefix set, wait for the invocation to settle, then read StandardOutputUrl from GetCommandInvocation and fetch the object.
