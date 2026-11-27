Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__ssm.fsx-credentials-parameter-bare-name-rejected ecs__ssm.fsx-credentials-parameter-from-another-provider-rejected
Issue-Key: ecs__ssm.fsx-credentials-parameter-bare-name-rejected|_refused_with#12|AssertionError|raise AssertionError(f"expected a refusal, the call returned {answer!r}")
Issue-Key: ecs__ssm.fsx-credentials-parameter-from-another-provider-rejected|_refusal#20|AssertionError|raise AssertionError(f"the registration was accepted: {result.get('taskDefinition', result)!r}")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# ecs__ssm: RegisterTaskDefinition accepts any FSx for Windows File Server credentialsParameter — neither a bare Parameter Store name nor a foreign-service ARN is refused

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
Two independent net-new causes in the validation of FSxWindowsFileServerVolumeConfiguration.authorizationConfig.credentialsParameter. Both are separate work items; close both before submitting.

Cause 1 of 2: a bare Parameter Store name is accepted where AWS requires an ARN.
AWS: credentialsParameter must be an ARN (Smithy: FSxWindowsFileServerAuthorizationConfig.credentialsParameter required — “the authorization credential option … provided using either the Amazon Resource Name (ARN) of a Secrets Manager secret or SSM Parameter Store parameter”). RegisterTaskDefinition given the parameter's bare name is refused with HTTP 400 ClientException whose message starts with the fixed text `ARNs must start with 'arn:': ` followed by the offending value copied from the request (here the Parameter Store name), and NO task-definition family or revision is created. Registering the same volume with the parameter's ARN succeeds as revision 1 and the stored volume echoes that ARN.
Emulator: returns HTTP 200 and registers revision 1 with `credentialsParameter` set to the bare name.
Why: a template or client that would be rejected at registration time on AWS is accepted here, so the mis-configured task definition reaches the launch path and the fail-fast refusal clients rely on never fires.
Fix: credentialsParameter validation in the RegisterTaskDefinition handler's volume/authorizationConfig validation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs), rejecting before any revision is written.
Traces: check ecs__ssm.fsx-credentials-parameter-bare-name-rejected.

Cause 2 of 2: an ARN belonging to another service is accepted as the credentials parameter.
AWS: only an SSM Parameter Store ARN or a Secrets Manager secret ARN is allowed. Any other service's ARN is refused with HTTP 400 ClientException and the message `The credentialsParameter ARN has an unsupported provider. It should either be a Systems Manager ARN or a Secrets Manager ARN.` (fixed text, no variable part), and no revision of the family is created; registration with a real Parameter Store ARN succeeds and lists exactly its one revision.
Emulator: returns HTTP 200 and registers revision 1 whose authorizationConfig.credentialsParameter is the SQS queue ARN that was supplied.
Why: the provider check is the only thing standing between a task definition and an unusable credential source; accepting a foreign ARN hides the mis-wiring until the container tries to mount the share, and leaves a revision behind that AWS would never have created.
Fix: the same credentialsParameter validation (src/ops/register_task_definition.rs) — classify the ARN's service namespace and accept only ssm/secretsmanager.
Traces: check ecs__ssm.fsx-credentials-parameter-from-another-provider-rejected.

## Evidence
Cause 1 — expected ('ClientException', a message starting `ARNs must start with 'arn:': ` and ending with the supplied parameter name) and no family created; got HTTP 200 with taskDefinition revision 1, volumes[0].fsxWindowsFileServerVolumeConfiguration.authorizationConfig.credentialsParameter = '/<parameter-name>' (AssertionError: expected a refusal, the call returned {...'status': 'ACTIVE', 'revision': 1...}).
Cause 2 — expected ('ClientException', 'The credentialsParameter ARN has an unsupported provider. It should either be a Systems Manager ARN or a Secrets Manager ARN.') and no revision created; got HTTP 200 with revision 1 whose credentialsParameter is 'arn:aws:sqs:<region>:<account>:<name>' (AssertionError: the registration was accepted: {...}).

## Steps to Reproduce
1. ecs__ssm.fsx-credentials-parameter-bare-name-rejected — ssm.PutParameter a SecureString, create an ecs-tasks trust role, then ecs.RegisterTaskDefinition with volumes[0].fsxWindowsFileServerVolumeConfiguration.authorizationConfig.credentialsParameter set to the parameter's bare Name; expect the ClientException and that ListTaskDefinitionFamilies(status='ALL') does not list the family.
2. ecs__ssm.fsx-credentials-parameter-from-another-provider-rejected — register the same volume once with the parameter's ARN (succeeds), then again with credentialsParameter = 'arn:aws:sqs:<region>:<account>:<name>'; expect the unsupported-provider ClientException and no revision for that family.
