Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
Issue-Key: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle|_reap_log_group#9|Timeout|return
Issue-Commit: 70eb2a3dfdc5763d75751d1a0ee4a95a331f97e0

# apigateway__dynamodb__iam__lambda: the IAM-authorization debugging lifecycle reaches no verdict — the signed GET on an AWS_IAM REST method never settles on the 403 AccessDeniedException, so the check is cut off at the harness budget (1 cause)

> _Note: this issue was identified against the code base at commit `70eb2a3dfdc5763d75751d1a0ee4a95a331f97e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMethod

## Description
THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Relation to already-open issues (none was reported as owning this check in this replay): 20261127T214652Z-apigateway-lambda-10-net-new-causes-aws-iam.md records that AWS_IAM is not enforced on a deployed REST stage, and 20261127T214652Z-apigateway-7-net-new-execute-api-causes-sigv4.md records that execute-api does not check the SigV4 credential scope and that a resource-policy deny hits the admitted caller. Neither covers this check, which also requires the denial's response SHAPE (status, error type header and message) and the subsequent 200 through the same URL for a correctly-scoped role.

Cause 1 of 1: a signed GET issued by a role whose only execute-api:Invoke grant names a different HTTP verb than the method being invoked never settles on API Gateway's 403 AccessDeniedException on a deployed REST stage whose method is authorizationType AWS_IAM, so the check's bounded wait for that durable decision never completes and the whole lifecycle is cut off with no verdict.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle.

  AWS: Smithy (com.amazonaws.apigateway): PutMethod/GetMethod carry a required `authorizationType` member and GetStage/CreateDeployment bind the method to a stage; a method deployed with authorizationType AWS_IAM has every execute-api request to it authorized by IAM against the caller's identity-based (and resource) policies — the control-plane model stops there, so the data-plane contract below is AWS's documented execute-api behavior as the recording exercises it. A SigV4-signed request from a principal with NO matching execute-api:Invoke allow is answered 403 with the response header `x-amzn-ErrorType: AccessDeniedException` and a JSON body whose `Message` is the fixed sentence "User: <caller-arn> is not authorized to perform: execute-api:Invoke on resource: <method-arn> because no identity-based policy allows the execute-api:Invoke action". Fixed text: "User: ", " is not authorized to perform: execute-api:Invoke on resource: ", " because no identity-based policy allows the execute-api:Invoke action". Variable parts, taken from the request and from state and never from any one recording: <caller-arn> is the signing principal's assumed-role session ARN, and <method-arn> is arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/<verb>/<resource-path> built from the invoked request. A principal whose grant does name that method ARN is served the integration's 200 response (here the Lambda AWS_PROXY integration's JSON item) through the same URL — including when the request path contains an empty segment (`/<stage>//data-agreements/<id>`), which API Gateway routes to the same `/data-agreements/{agreementId}` method.

  Emulator: the check is killed by the harness with `Error: Check timed out after 60.0s` and no traceback, so no assertion was reached and no verdict was recorded for any of its five phases (deploy, setup, solve, cleanup, destroy). The only unbounded-by-assertion waits in the check are the two execute-api probes in the `solve` phase: the first re-issues the restricted role's signed GET until two consecutive responses carry `x-amzn-errortype == "AccessDeniedException"` (240 s budget, far above the harness cap), the second re-issues a correctly-scoped role's GET until it returns 200. Everything before them is either an SDK call or a bounded waiter that would have raised rather than hung. So the lifecycle stops on the execute-api decision: the deployed AWS_IAM method does not answer the restricted role's GET with 403 / x-amzn-ErrorType: AccessDeniedException (and consequently the derived-fix 200 leg is never reached).
  Hypothesis: AWS_IAM authorization is not applied on the deployed REST stage at all, so the restricted role's GET is answered by the integration (a non-403) and the predicate never holds — this is the cause already recorded for other apigateway checks in 20261127T214652Z-apigateway-lambda-10-net-new-causes-aws-iam.md. Neither the model nor the recording states which of 'not enforced' or 'enforced with the wrong shape' applies here, because the harness captured no response.

  Why: IAM authorization on a REST method is the mechanism this whole scenario exists to debug. A client that deploys an AWS_IAM method and then drives it from a least-privilege role gets no refusal it can act on: the 403 that tells it the grant's verb is wrong never arrives, nothing distinguishes an under-privileged caller from an authorized one, and the x-amzn-ErrorType header and Message a client parses to find the offending method ARN are absent. Any automation that asserts a deny-then-fix flow, or that relies on AWS_IAM to keep an unauthorized caller out of a Lambda integration backed by a DynamoDB table, is silently wrong — and in this check it blocks indefinitely instead of failing.

  Fix: the execute-api data-plane front door of the apigateway service — the request-authorization step that resolves the invoked method's authorizationType and, for AWS_IAM, evaluates the signing principal's execute-api:Invoke grants against the method ARN, plus the serializer for its refusal (403, `x-amzn-ErrorType: AccessDeniedException`, JSON body with the Message sentence above). The route-matching that maps a path with an empty segment onto the `{agreementId}` method belongs to the same front door.

  Traces: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle (cut off at the 60 s harness budget in its `solve` phase).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Service heading `### apigateway__dynamodb__iam__lambda`: 5/6 passed, 1 failed.

[mismatch] apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
  Error: Check timed out after 60.0s

No traceback was produced: the harness cut the check off, so no assertion was reached and the phase journal recorded no verdict.

What the check deploys (phase `deploy`): a DynamoDB table with key agreementId, a python3.12 Lambda reading one item from it, a REGIONAL REST API with /data-agreements/{agreementId}, PutMethod(httpMethod='GET', authorizationType='AWS_IAM'), an AWS_PROXY integration to the function, a deployment to stage 'beta'; plus two client roles, each with a single inline grant {"Action":["execute-api:Invoke"],"Resource":[<arn>]} — the 'restricted' role scoped to <api-arn>/beta/POST/* and a decoy scoped to another api id. It asserts GetMethod authorizationType == 'AWS_IAM' and GetStage deploymentId == the created deployment, and those pass.

Where it stops (phase `solve`), after deriving the fix from live control-plane state:
  status, headers, body = _await(
      lambda: _invoke(ctx, request_url, _session(ctx, {"arn": role_arn})),
      lambda r: r[1].get("x-amzn-errortype") == "AccessDeniedException",
      "the restricted role's GET to be decided by IAM")
  assert status == 403 and headers.get("x-amzn-errortype") == "AccessDeniedException"
  assert (_json(body) or {}).get("Message", "").endswith(
      f":{api_id}/{_STAGE}/GET{est['agreement_path']} because no identity-based policy allows "
      "the execute-api:Invoke action")
where _invoke SigV4-signs the request for service 'execute-api' with the assumed-role session's credentials and _await re-probes every 1-3 s until the predicate holds twice in a row, with a 240 s budget. It then creates a 'verifier' role holding exactly <api-arn>/beta/GET/* and awaits a 200 carrying the seeded agreement through the same URL.

Expected: 403 with header x-amzn-ErrorType: AccessDeniedException and body Message "User: <session-arn> is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:<region>:<account>:<api-id>/beta/GET/data-agreements/<id> because no identity-based policy allows the execute-api:Invoke action", followed by 200 with the item for the GET-scoped role.
Actual: the predicate never holds within the harness budget — the deployed AWS_IAM method never answers the restricted role's signed GET with that 403/AccessDeniedException — so the check is killed at 60 s and reaches no verdict.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Create a DynamoDB table with HASH key agreementId and put one item; create a Lambda execution role and a python3.12 function that reads the item by event['pathParameters']['agreementId'].
3. CreateRestApi (REGIONAL), CreateResource 'data-agreements' and '{agreementId}', PutMethod(httpMethod='GET', authorizationType='AWS_IAM'), PutIntegration(type='AWS_PROXY', integrationHttpMethod='POST', uri=the function's invoke ARN), AddPermission for apigateway.amazonaws.com, then CreateDeployment(stageName='beta'). Confirm GetMethod reports authorizationType 'AWS_IAM'.
4. Create role A with one inline grant {"Action":["execute-api:Invoke"],"Resource":["arn:aws:execute-api:<region>:<account>:<api-id>/beta/POST/*"]} and role B with the same grant on .../beta/GET/*; AssumeRole each.
5. As role A, issue a SigV4-signed (service 'execute-api') GET to the stage URL path /beta//data-agreements/<id>: expect 403, response header x-amzn-ErrorType: AccessDeniedException, and a JSON body whose Message ends ":<api-id>/beta/GET/data-agreements/<id> because no identity-based policy allows the execute-api:Invoke action". Observe that no such 403 is returned.
6. As role B, issue the same signed GET: expect 200 with the seeded item as JSON.
7. Replay apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle; it currently exhausts the 60 s harness budget in its `solve` phase at step 5.
