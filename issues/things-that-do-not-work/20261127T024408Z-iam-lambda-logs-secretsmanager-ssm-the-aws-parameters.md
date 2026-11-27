Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant
Issue-Key: iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant|_invoke#29|AssertionError|assert "FunctionError" not in resp, f"invoke of {name!r} raised inside the function: {body!r}"
Issue-Commit: 34ec76825aeb60426f74f39ee1ee1124c6afb7e4

# iam__lambda__logs__secretsmanager__ssm: the AWS Parameters and Secrets Lambda Extension's local endpoint (localhost:2773) is not served inside the execution environment, so the function faults instead of reading a secret

> _Note: this issue was identified against the code base at commit `34ec76825aeb60426f74f39ee1ee1124c6afb7e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a function with the AWS Parameters and Secrets Lambda Extension layer attached has no local extension endpoint. An HTTP request from the handler to `http://localhost:2773/...` is not served; the emulator's own egress layer refuses it by name resolution, and the invocation returns a FunctionError instead of the extension's response.

AWS: recorded behavior only for the wire details (the extension is a layer, not a modelled API; documented at https://docs.aws.amazon.com/secretsmanager/latest/userguide/retrieving-secrets_lambda.html). What the recordings show: with that layer in `Layers`, the execution environment serves HTTP on `localhost:2773`; `GET /secretsmanager/get?secretId=<url-encoded secret id or ARN>` with header `X-Aws-Parameters-Secrets-Token: <the environment's AWS_SESSION_TOKEN>` answers 200 with a GetSecretValue-shaped JSON body whose `SecretString` is the secret's plaintext, when the function's execution role is granted `secretsmanager:GetSecretValue` on that secret. For a secret the role is NOT granted, the same call is refused with HTTP status 400, header `X-Amzn-Errortype: AccessDeniedException` (Secrets Manager's own error code), and the body's fixed text `an unexpected error occurred while executing request` — that sentence has no variable parts, and the status and errortype are the requirement. Widening the role's grant to include the second secret makes the identical call succeed and serve that secret's own plaintext.

Emulator: the handler's `urllib` request to `http://localhost:2773/secretsmanager/get?secretId=<arn>` raises `gaierror` inside the function. `Invoke` returns a payload with `FunctionError`, `errorType: 'gaierror'` and an `errorMessage` authored by the emulator's egress layer: `"http://localhost:2773/secretsmanager/get?secretId=<arn> addresses no service this emulator serves: sqs, sns, sts, iam, events, ... ecs"`. So the positive control — the secret the role IS granted — already fails; the denial contract (400 / `X-Amzn-Errortype: AccessDeniedException` / the fixed body sentence) and the grant-widening restore step are never reached. The refusal is the emulator's own: it answered an address inside its own execution environment by announcing the list of service hosts it serves instead of serving the extension's loopback port.

Why: the extension is the documented way a function reads secrets and SSM parameters with caching, and a client using it gets a hard fault rather than a value. Code that reads `SecretString` through the extension cannot run at all; code that relies on the extension's denial contract (400 + `AccessDeniedException` errortype) to distinguish "not permitted" from "not found" sees an unhandled runtime error instead, and the IAM boundary the extension enforces per-secret is not exercised at all.

Fix: the Lambda execution environment's extension surface — the component that builds the sandbox for a function whose `Layers` include the AWS Parameters and Secrets extension must serve its HTTP listener on loopback port 2773 inside that sandbox (routing `/secretsmanager/get` to Secrets Manager GetSecretValue and `/systemsmanager/parameters/get` to SSM GetParameter under the function role's identity, authenticated by the `X-Aws-Parameters-Secrets-Token` header), and the sandbox's loopback traffic must not be handed to the host-based egress resolver that refuses unknown service hostnames.

Traces: iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant. Related open issues that do NOT cover this endpoint: 20261111T081112Z-lambda-the-python-execution-environment-and-the-npm-bundle-path.md, 20261118T120000Z-lambda-cross-region-store-isolation-response-streaming-extensions-and-the-exec-wrapper.md and 20261120T090000Z-lambda-post-runtime-extensions-duration-is-never-published.md (extensions not being run / their metric), none of which name the Parameters-and-Secrets extension's localhost:2773 HTTP surface.

## Evidence
iam__lambda__logs__secretsmanager__ssm.parameters-secrets-extension-denies-get-secret-without-the-iam-grant

Failed at the positive control, the FIRST invoke:
  control = _invoke(lam, fn_name, {"secretId": arn_granted})
  AssertionError: invoke of 'sim2real-behav-ext-fn-...' raised inside the function:
  {'errorMessage': 'http://localhost:2773/secretsmanager/get?secretId=arn%3Aaws%3Asecretsmanager%3A<region>%3A<account>%3Asecret%3Asim2real-behav-ext-sec-granted-...  addresses no service this emulator serves: sqs, sns, sts, iam, events, eventbridge, pipes, scheduler, dynamodb, logs, cloudwatchlogs, cloudwatch, apigateway, apigatewaymanagementapi, secretsmanager, kms, ssm, opensearch, es, stepfunctions, sfn, ses, sesv2, s3, xray, rds, cloudformation, ec2, rds-data, appconfigdata, appconfig, config, configservice, codedeploy, codepipeline, cloudfront, rekognition, lambda, glue, bedrock-agent-runtime, bedrock-agent, opensearchserverless, s3vectors, kinesis, cognito-idp, cognito-identity-provider, iot, iot-data, iot-data-plane, transfer, batch, ecr, ecs',
   'errorType': 'gaierror',
   'stackTrace': ['File "index.py", line 18, in handler\n    with urllib.request.urlopen(req) as resp:', ... 'urllib/request.py ... http_open ... do_open']}

The function was created Active with Layers=[the value of SSM public parameter /aws/service/aws-parameters-and-secrets-lambda-extension/x86/latest], runtime python3.13, and a role granted secretsmanager:GetSecretValue on the first secret only.

Expected (AWS), none of which was reached:
  - granted secret: the extension answers 200 and the handler returns {'ok': True, 'secretString': <that secret's plaintext>}.
  - ungranted sibling secret: HTTP 400, header X-Amzn-Errortype = 'AccessDeniedException', body exactly 'an unexpected error occurred while executing request'.
  - after PutRolePolicy widens the grant to both ARNs, the identical invoke returns the second secret's own plaintext.

## Steps to Reproduce
1. secretsmanager.CreateSecret twice (one 'granted', one 'ungranted'), each with its own SecretString.
2. iam.CreateRole trusted by lambda.amazonaws.com; PutRolePolicy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents on * and secretsmanager:GetSecretValue on the GRANTED secret's ARN only.
3. ssm.GetParameter Name=/aws/service/aws-parameters-and-secrets-lambda-extension/x86/latest to get the extension layer ARN.
4. lambda.CreateFunction runtime=python3.13, Layers=[that ARN], handler code that GETs http://localhost:2773/secretsmanager/get?secretId=<url-encoded id> with header X-Aws-Parameters-Secrets-Token set to the environment's session token, returning the parsed SecretString or, on HTTPError, the status plus the X-Amzn-Errortype header and body.
5. lambda.Invoke with {"secretId": "<granted ARN>"}.
   AWS: 200 from the extension, SecretString is the granted secret's plaintext.
   Emulator: the response carries FunctionError with errorType 'gaierror' and an errorMessage saying the localhost:2773 URL 'addresses no service this emulator serves: ...'.
6. Invoke with the UNGRANTED ARN — AWS answers 400 / X-Amzn-Errortype: AccessDeniedException / body 'an unexpected error occurred while executing request'; the emulator never serves the endpoint at all.
