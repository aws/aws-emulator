Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back|run#15|AssertionError|assert reason == (
Issue-Commit: 603e6927c4f4e28f4fb6d30537cb70c01235758c

# secretsmanager: the AWS::SecretsManager::ResourcePolicy CREATE_FAILED reason drops the AWS SDK "(Service: SecretsManager, Status Code: 400, ...)" diagnostics, so a PublicPolicyException refusal is not reported as CloudFormation reports it

> _Note: this issue was identified against the code base at commit `603e6927c4f4e28f4fb6d30537cb70c01235758c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
ONE net-new behavioral root cause. The failing check is marked neither ALREADY FILED nor OWNED by the replay.

Cause 1 of 1: when Secrets Manager refuses a CloudFormation-declared AWS::SecretsManager::ResourcePolicy (a wildcard-principal Allow under BlockPublicPolicy), the resource's CREATE_FAILED ResourceStatusReason carries only the service's message sentence and omits the AWS SDK diagnostic clause CloudFormation's registry handler renders around it.
AWS: Smithy (services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): PutResourcePolicy takes SecretId (required), ResourcePolicy (required) and BlockPublicPolicy, and models PublicPolicyException (client error, HTTP 400) exactly for this case; its message is the fixed sentence "You can not attach a resource policy that grants Public access to your secret." CloudFormation surfaces that refusal through the resource handler, which renders the downstream SDK exception INSIDE the quoted handler message: 'Resource handler returned message: "<service message> (Service: SecretsManager, Status Code: 400, Request ID: <id>, Extended Request ID: <id-or-null>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The fixed text is 'Resource handler returned message: "', the service's own sentence, the clause '(Service: SecretsManager, Status Code: 400, Request ID: ', and the trailing '(RequestToken: ..., HandlerErrorCode: InvalidRequest)'; the request id, extended request id and request token are per-run values and are NOT requirements. The stack ends ROLLBACK_COMPLETE and the handler error code is InvalidRequest.
Emulator: the refusal itself and its sentence are right, and HandlerErrorCode is InvalidRequest, but the quoted message is closed immediately after the sentence and the SDK diagnostics clause is absent — the reason reads 'Resource handler returned message: "You can not attach a resource policy that grants Public access to your secret." (RequestToken: f734488b-c358-7dd8-86eb-56b2e7ecd4be, HandlerErrorCode: InvalidRequest)'. It carries neither the originating service id, nor the HTTP status of the underlying refusal, nor a service request id, so the check's split on ' (Service: SecretsManager, ' finds nothing and the diagnostics half is empty instead of starting 'Status Code: 400,'.
Why: the resource-handler reason is the only place a CloudFormation client sees WHICH service refused a resource and with what HTTP status. Operators and automation parse that clause to route a stack failure to the right service and to quote a service request id in a support case; without it a stack failure caused by a downstream 400 is indistinguishable from one the handler itself produced, and there is no request id to correlate.
Fix: the CloudFormation resource-handler error path that renders a downstream AWS API exception into ResourceStatusReason for AWS::SecretsManager::ResourcePolicy (and the Secrets Manager resource types generally) — it must embed the SDK-style '(Service: <sdkId>, Status Code: <status>, Request ID: <id>, Extended Request ID: <id>)' clause inside the quoted service message before the RequestToken/HandlerErrorCode suffix. Open issue 20261113T114936Z-cloudformation-sagemaker-a-resource-handler-s-failure-reason.md records the same omission for SageMaker resource types and 20261107T060626Z-cloudformation-iam-lambda-s3-cloudformation-an-aws-cloudformation-stack-resource-failure-reason.md for nested stacks; neither carries this check's replay key or this service token, so neither covers this failure.
Traces: secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back.

## Evidence
secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back [mismatch]:
  AssertionError: unexpected failure reason: 'Resource handler returned message: "You can not attach a resource policy that grants Public access to your secret." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'

Expected (the check partitions the reason on the literal ' (Service: SecretsManager, ' and grades the head exactly, then requires the tail to start 'Status Code: 400,'):
  head  == 'Resource handler returned message: "You can not attach a resource policy that grants Public access to your secret.'
  tail.startswith('Status Code: 400,')
Actual: the head is the whole reason (no ' (Service: SecretsManager, ' occurs anywhere in it) and the tail is empty.

So the requirement is the fixed clause '(Service: SecretsManager, Status Code: 400, Request ID: ...)' rendered inside the quoted handler message; the request id and RequestToken values in the recording are per-run and are not requirements.

Smithy consulted: PutResourcePolicy errors = InternalServiceError(500), InvalidParameterException(400), InvalidRequestException(400), MalformedPolicyDocumentException(400), PublicPolicyException(400), ResourceNotFoundException(400). PublicPolicyException/400 is the refusal behind this reason; its sentence is recorded behavior and the emulator already matches it.

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run secretsmanager.deploy-cfn-public-resource-policy-rejected-by-block-public-policy-and-rolled-back. Manually:
1. CreateStack with a template containing an AWS::SecretsManager::Secret and an AWS::SecretsManager::ResourcePolicy whose SecretId Refs that secret, BlockPublicPolicy=true, and a ResourcePolicy with a single statement {Effect: Allow, Principal: "*", Action: secretsmanager:GetSecretValue, Resource: "*"}.
2. Wait for the stack to reach ROLLBACK_COMPLETE (it does).
3. DescribeStackEvents and read the CREATE_FAILED row for the ResourcePolicy resource. Its ResourceStatusReason must read 'Resource handler returned message: "You can not attach a resource policy that grants Public access to your secret. (Service: SecretsManager, Status Code: 400, Request ID: <id>, Extended Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'; the emulator closes the quote after 'your secret.' and omits the '(Service: SecretsManager, Status Code: 400, ...)' clause entirely.
