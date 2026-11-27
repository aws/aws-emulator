Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant
Issue-Key: ssm|GetParameter|ParameterNotFound
Issue-Commit: d26390c9b72db2589fa327994590745892c0adba

# REOPENS: SSM GetParameter answers ParameterNotFound for the AWS-published /aws/service/aws-parameters-and-secrets-lambda-extension/<arch>/latest public parameter, so the Parameters-and-Secrets Lambda Extension scenario cannot start

> _Note: this issue was identified against the code base at commit `d26390c9b72db2589fa327994590745892c0adba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
REOPENS 20261108T222044Z-autoscaling-ec2-ecs-ssm-getparameter-answers-parameternotfound-for.md: its fix left the call refused. That issue (Issue-Key ssm|GetParameter|ParameterNotFound) claimed the AWS-published /aws/service hierarchy would be seeded and served read-only by the GetParameter read path; a read of a different name in that same published hierarchy is still refused with ParameterNotFound.

Cause 1 of 1: the AWS-published public parameter that carries the AWS Parameters and Secrets Lambda Extension layer ARN is not served.

AWS: GetParameter is a read of the AWS-published, account-agnostic public parameter namespace; any caller in any account can read /aws/service/aws-parameters-and-secrets-lambda-extension/<arch>/latest (arch segments such as x86 and arm64) and gets HTTP 200 with a Parameter whose Value is the current layer-version ARN of the extension for that Region — the form arn:aws:lambda:<region>:<aws-owned-account>:layer:AWS-Parameters-and-Secrets-Lambda-Extension:<version>, with Type String. The region, owning account and version are per-Region published values, not part of the requirement; what is required is that the name resolves to a layer ARN that Lambda will accept in CreateFunction's Layers. Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): GetParameter.Name @required, target PSParameterName @length 1..2048; modeled errors InternalServerError, InvalidKeyId, ParameterNotFound (httpError 400 / awsQueryError httpResponseCode 404), ParameterVersionNotFound. ParameterNotFound is the refusal for a parameter that does not exist, so AWS never raises it for a published public parameter. AWS publishes no Message for ParameterNotFound, so the empty message body is NOT the divergence — the divergence is that the call is refused at all. Recorded behavior only for the exact set of names published under that path. Reference: https://docs.aws.amazon.com/secretsmanager/latest/userguide/retrieving-secrets_lambda.html

Emulator: GetParameter(Name='/aws/service/aws-parameters-and-secrets-lambda-extension/x86/latest') is refused with error code ParameterNotFound and an empty message, exactly as if the caller had asked for a parameter it never created. Nothing under this AWS-published path is served, so the check dies on its first call — no role, no function, no layer, and none of the extension's local-proxy behavior (the granted-secret positive control, the ungranted-secret HTTP 400 / X-Amzn-Errortype AccessDeniedException denial, or the post-grant retry) is ever exercised.

Why: the documented way to attach the Parameters and Secrets Lambda Extension is to resolve its layer ARN from this public parameter rather than hard-code a Region- and version-bound ARN. Against the emulator every such client — including CloudFormation templates and IaC that read the same name — fails on its first call with a not-found for a parameter that exists in every real Region, and the whole in-Lambda secret-caching surface is unreachable.

Fix: the SSM GetParameter handler's read path and the parameter-store state that backs the AWS-published /aws/service hierarchy in the ssm service crate — seed and serve this name (read-only, not account-owned) alongside the account's own parameters, and have it resolve to a layer ARN the lambda crate's CreateFunction accepts in Layers. Note that even with the parameter served, the extension's localhost:2773 proxy must answer for the rest of the check to grade; open issues about Lambda extensions not executing (e.g. 20261118T120000Z-lambda-cross-region-store-isolation-response-streaming-extensions-and-the-exec-wrapper.md, 20261120T090000Z-lambda-post-runtime-extensions-duration-is-never-published.md) are related but name neither this parameter nor this check, and the suite did not mark this check as owned by any issue.

Traces: iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant.

## Evidence
Replay of simparity-behavioral-aws-v1, service heading '### iam__lambda__logs__secretsmanager__ssm':

[error] iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant
  botocore.errorfactory.ParameterNotFound: An error occurred (ParameterNotFound) when calling the GetParameter operation:

The failing call is the check's layer lookup:
  layer_arn = ssm.get_parameter(Name="/aws/service/aws-parameters-and-secrets-lambda-extension/x86/latest")["Parameter"]["Value"]
which is then passed to lam.create_function(..., Layers=[layer_arn]).

Expected (AWS): HTTP 200 with Parameter.Value = the Region's current extension layer-version ARN (arn:aws:lambda:<region>:<aws-owned-account>:layer:AWS-Parameters-and-Secrets-Lambda-Extension:<version>), Type String.
Actual (emulator): refusal with Error.Code ParameterNotFound and an empty Message; no Parameter member in the response.

The requested Name is the only variable part of the refusal; AWS publishes no fixed Message text for ParameterNotFound, so the requirement violated is the refusal itself, not its wording. The Region, owning account id and layer version in the expected Value are published per-Region values, not literal requirements.

Because the first call fails, the check's later assertions are never reached: the granted sibling secret served through http://localhost:2773/secretsmanager/get must return its own plaintext; the ungranted secret must be denied with HTTP status 400, header X-Amzn-Errortype 'AccessDeniedException' and the fixed body text 'an unexpected error occurred while executing request'; and after widening the role policy the identical call must succeed.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Call ssm.get_parameter(Name='/aws/service/aws-parameters-and-secrets-lambda-extension/x86/latest'). Expect HTTP 200 with Parameter.Value an AWS-Parameters-and-Secrets-Lambda-Extension layer ARN; the emulator refuses with ParameterNotFound and an empty message.
3. Repeat with the arm64 segment ('/aws/service/aws-parameters-and-secrets-lambda-extension/arm64/latest') and with ssm.get_parameters_by_path(Path='/aws/service/aws-parameters-and-secrets-lambda-extension', Recursive=True) — AWS serves the published hierarchy; the emulator serves nothing.
4. With the ARN resolved, pass it in lambda CreateFunction Layers for a python3.13 function and invoke the handler that GETs http://localhost:2773/secretsmanager/get?secretId=<arn> with the X-Aws-Parameters-Secrets-Token header, for a secret the role is granted and one it is not.
5. Re-replay the check listed in trace_ids.
