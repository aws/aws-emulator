Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.deploy-ec2-launch-template-duplicate-name-rejected
Issue-Key: cloudformation__ec2.deploy-ec2-launch-template-duplicate-name-rejected|run#17|AssertionError|assert match is not None, f"the launch template failure reason is not an EC2 error: {reason!r}"
Issue-Commit: 22d8facbc3b601c7c60aed2bc6871542e8162809

# cloudformation__ec2: the CREATE_FAILED reason for an AWS::EC2::LaunchTemplate reusing a registered LaunchTemplateName is the emulator's own wording and carries no EC2 service-error envelope

> _Note: this issue was identified against the code base at commit `22d8facbc3b601c7c60aed2bc6871542e8162809`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
1 net-new root cause in the AWS::EC2::LaunchTemplate create handler's already-exists refusal.

Cause 1 of 1: when a stack declares an AWS::EC2::LaunchTemplate whose LaunchTemplateName is already registered in EC2, the stack does roll back, but the CREATE_FAILED event's ResourceStatusReason is the emulator's own sentence and omits the EC2 service-error envelope that AWS relays, so the underlying error code and status are unavailable.

AWS: CloudFormation relays the downstream service's error verbatim inside its resource-handler wrapper, so the CREATE_FAILED ResourceStatusReason for this refusal reads:
  Resource handler returned message: "Launch template name already in use. (Service: Ec2; Status Code: 400; Error Code: InvalidLaunchTemplateName.AlreadyExistsException; Request ID: <id>; Proxy: null)" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)
Fixed text: the wrapper "Resource handler returned message: \"" ... "\" (RequestToken: ..., HandlerErrorCode: AlreadyExists)", EC2's message "Launch template name already in use.", and the envelope keys "Service:", "Status Code:", "Error Code:", "Request ID:", "Proxy: null". Variable parts taken from the request/state and the run: the Service token (Ec2), the status 400, the error code InvalidLaunchTemplateName.AlreadyExistsException, the request id and the request token. Smithy: ec2#CreateLaunchTemplate models no errors (EC2 is ec2Query and its errors are unmodelled), so the message/code/status above are recorded behavior only; the stack-level outcome is ROLLBACK_COMPLETE with exactly one CREATE_FAILED event, on the launch-template resource.

Emulator: the stack does reach ROLLBACK_COMPLETE with one CREATE_FAILED event on the launch template, but its ResourceStatusReason is:
  Resource handler returned message: "The launch template name '<name>' is already in use" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)
That diverges twice in one reason: the inner message is "The launch template name '<name>' is already in use" instead of AWS's fixed "Launch template name already in use." (note the trailing period and that AWS does not interpolate the name), and the whole "(Service: Ec2; Status Code: 400; Error Code: InvalidLaunchTemplateName.AlreadyExistsException; Request ID: <id>; Proxy: null)" envelope is absent, so neither the HTTP status 400 nor the error code InvalidLaunchTemplateName.AlreadyExistsException is reachable.

Why: a client that classifies stack failures by parsing the relayed service error — the standard way to tell an AlreadyExists collision from a quota or permission failure, and what CDK/CLI tooling and runbooks match on — finds no Error Code or Status Code in the reason and cannot distinguish this refusal from any other launch-template failure. The free-text message also does not match AWS's documented wording.

Fix: the AWS::EC2::LaunchTemplate create handler's error relay (and the shared resource-handler reason formatter it uses) — route the duplicate-name refusal through EC2's CreateLaunchTemplate error so the handler reports EC2's message and wraps it in the "(Service: ...; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)" envelope before CloudFormation's "Resource handler returned message: ..." wrapper.

The open issue 20261109T223435Z-cloudformation-ec2-6-net-new-causes-26-aws.md collects other cloudformation__ec2 causes but does not carry this check's replay key.

Traces: cloudformation__ec2.deploy-ec2-launch-template-duplicate-name-rejected.

## Evidence
Expected (AWS) CREATE_FAILED ResourceStatusReason, parsed into (message, status, code):
  ("Launch template name already in use.", "400", "InvalidLaunchTemplateName.AlreadyExistsException")
rendered as: Resource handler returned message: "Launch template name already in use. (Service: Ec2; Status Code: 400; Error Code: InvalidLaunchTemplateName.AlreadyExistsException; Request ID: <id>; Proxy: null)" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)

Actual (emulator):
  AssertionError: the launch template failure reason is not an EC2 error: 'Resource handler returned message: "The launch template name \'sim2real-xsvc-ltdup-ba019a07c4b3\' is already in use" (RequestToken: c3193271-5ede-cadc-d8f7-3f5b355e57b5, HandlerErrorCode: AlreadyExists)'

(The template name, request id and request token in that line are per-run values; the divergence is the inner message wording and the missing Service/Status Code/Error Code/Request ID/Proxy envelope.) The stack status ROLLBACK_COMPLETE, the single CREATE_FAILED event on the launch-template resource and HandlerErrorCode: AlreadyExists all match AWS.

## Steps to Reproduce
1. Call ec2:CreateLaunchTemplate to register a launch template under some name N.
2. CreateStack a template whose only resource is an AWS::EC2::LaunchTemplate with LaunchTemplateName = N.
3. Wait for the stack to settle (ROLLBACK_COMPLETE) and call DescribeStackEvents; take the CREATE_FAILED event for the launch-template logical id.
4. Its ResourceStatusReason carries the emulator's own sentence and no "(Service: ...; Status Code: 400; Error Code: InvalidLaunchTemplateName.AlreadyExistsException; Request ID: ...; Proxy: null)" envelope, so message/status/code cannot be parsed out.
5. Re-replay cloudformation__ec2.deploy-ec2-launch-template-duplicate-name-rejected to confirm the fix.
