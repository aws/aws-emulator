Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cdk-aws-custom-resource-failed-sdk-call-fails-the-stack-unless-its-code-is-ignored
Issue-Key: cloudformation__lambda__ssm.cdk-aws-custom-resource-failed-sdk-call-fails-the-stack-unless-its-code-is-ignored|run#19|AssertionError|assert reasons == [f"{head}(RequestId: {request_id})"] and _is_uuid(request_id), (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# cloudformation__lambda__ssm: a Custom::AWS resource whose SDK call is refused reports 'Internal Error' instead of the SDK error's name

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: the AwsCustomResource (Custom::AWS) failure reason does not carry the refused SDK call's error.
AWS: when the custom resource's SDK call is refused, the handler returns the SDK error's NAME and message, so the CREATE_FAILED ResourceStatusReason reads `Received response status [FAILED] from custom resource. Message returned: <SDK error name> <SDK error message>(RequestId: <uuid>)` — for a GetParameter whose parameter does not exist the name is `UnknownError` and the message is empty, i.e. `Received response status [FAILED] from custom resource. Message returned: UnknownError (RequestId: <uuid>)`. The fixed text is that sentence; the error name/message and the request id are the variable parts.
Emulator: the reason is `Received response status [FAILED] from custom resource. Message returned: Internal Error (RequestId: <uuid>)` — a generic 'Internal Error' in place of the SDK error's name, with no indication of which call failed or why.
Why: the only place a CDK AwsCustomResource surfaces its failure is this reason string; an operator (and the `ignoreErrorCodesMatching` behaviour that depends on the code) cannot see the refused call's error.
Fix: the Custom::AWS / AwsCustomResource handler's error mapping, which must propagate the SDK error name and message into the cfn-response Reason instead of substituting a generic string.
Traces: cloudformation__lambda__ssm.cdk-aws-custom-resource-failed-sdk-call-fails-the-stack-unless-its-code-is-ignored.

## Evidence
CREATE_FAILED reasons were ['Received response status [FAILED] from custom resource. Message returned: Internal Error (RequestId: 781d0df9-cbcc-0cb8-1762-410c258b2606)']; expected 'Received response status [FAILED] from custom resource. Message returned: UnknownError (RequestId: <uuid>)'.

## Steps to Reproduce
Deploy a CDK stack with an AwsCustomResource whose onCreate is SSM getParameter for a parameter that does not exist (install_latest_aws_sdk=False, no ignore_error_codes_matching); the stack rolls back — read the Custom::AWS CREATE_FAILED ResourceStatusReason from DescribeStackEvents.
