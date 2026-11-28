Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
Issue-Key: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle|_reap_log_group#9|Timeout|return
Issue-Commit: 18a314cdb2529802996db00768c0e97bdee3f604

# The IAM-authorization lifecycle on a deployed REST stage reaches no verdict — a signed execute-api GET on an AWS_IAM method never settles on either the 403 AccessDeniedException or the granted 200

> _Resolution: the cause as REPORTED — "one of those [two signed execute-api invokes] never
> converges", i.e. the stage keeps answering something other than the graded answer or never
> answers at all — is not present at this commit, and
> `core/router/tests/all/apigateway_session_signed_stage_target.rs` now pins the whole graded
> path on an emulator whose request authentication is ON, which is the one condition no
> neighbouring suite drove: the invoke is signed with the credential STS minted for an
> assumed-role session (its session token presented, its secret held only by the emulator's own
> credential store), over the request target NORMALISED the way every AWS SDK signer normalises
> it before signing, while the wire carries the target as written — including the recorded case's
> doubled slash after the stage. Over that estate the stage answers in milliseconds, in every
> spelling of the target (plain, doubled after the stage, doubled inside the path, both, trailing
> slash) and for each of the three grants (`GET` on the stage, another verb only, none): a
> session no document allows is refused `403` with `x-amzn-ErrorType: AccessDeniedException` and
> the sentence `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to
> perform: execute-api:Invoke on resource: <the method ARN the target selected> because no
> identity-based policy allows the execute-api:Invoke action`, and the session that holds the
> grant is served the method's integration — including an `AWS_PROXY` one, whose function is
> invoked as API Gateway's own service principal and so runs for a caller holding nothing but the
> invoke grant. Both suite tests together take 2.5 s. What remains of the recorded failure is the
> harness's own WALL CLOCK, as the earlier report of this same replay key found
> (`20261110T155738Z-apigateway-dynamodb-iam-lambda-the-rest-api-iam.md`): this five-phase
> scenario pays its client's own poll intervals — a `table_exists`/`table_not_exists` waiter looks
> again only every 20 s, around a created and a deleted table's `CREATING`/`DELETING` spans that
> the project's own transition rule requires, and the check's `_await` helper sleeps between
> probes — and the report states no AWS-published latency for any step, so no AWS rule shortens
> it and nothing on the emulator's side of it is left to shorten: the whole lifecycle, the
> teardown included, settles in seconds against the assembled emulator
> (`core/router/tests/all/apigateway_iam_authorization_debug_lifecycle.rs`)._

> _Note: this issue was identified against the code base at commit `18a314cdb2529802996db00768c0e97bdee3f604`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api Invoke (deployed REST stage GET)

## Description
1 net-new root cause (a NEW replay key; the suite reported it unowned, and it is the only failure in this service group — the other five checks pass).

Cause 1 of 1: the end-to-end check that grades API Gateway's AWS_IAM authorization on a deployed REST stage produces no observable outcome inside the harness budget — the replay detail is exactly `Error: Check timed out after 60.0s`, with no stack frame. The only waits in the check that depend on an emulator answer converging are the two signed execute-api invokes in its solve phase, each polled to a 240 s deadline requiring the same answer twice in a row; neither can be reached in the 60 s cap unless the emulator keeps returning something other than the graded answer (or never answers at all). Several open issues record the same shape of stall for this data plane — most closely 20261118T150346Z-apigateway-events-sqs-the-execute-api-denial-check.md ("the execute-api denial check still reaches no verdict"), and also 20261027T030000Z-apigateway-a-token-lambda-authorizer-allow-decision-is-never-obtained.md and 20261115T090135Z-apigateway-apiendpoint-is-published-as-a-loopback-host.md — but none of them carries this check's key, so none covers it.

AWS: a REST method whose authorizationType is AWS_IAM is authorized against the caller's identity policies for execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/<METHOD><resource-path>. A SigV4-signed request from a session that holds no matching grant is refused at the stage with HTTP 403, response header x-amzn-ErrorType: AccessDeniedException, and a JSON body whose Message is the fixed template `User: <caller arn> is not authorized to perform: execute-api:Invoke on resource: <the method ARN above> because no identity-based policy allows the execute-api:Invoke action` — the fixed text is everything outside the angle brackets, while the caller ARN (the assumed-role session ARN), the api id, the stage, the HTTP method and the resource path all come from the request and the emulator's state. A session that DOES hold execute-api:Invoke on that ARN is served the integration's response — here HTTP 200 with the Lambda proxy integration's JSON body, the seeded DynamoDB item. The stage must also normalise the request target: a URL containing a double slash (`/<stage>//data-agreements/<id>`) selects the same `/data-agreements/{agreementId}` method. Recorded behavior only for the invoke URL's host form; the Smithy model does not describe the execute-api data plane, and AWS's documented denial wording above is the requirement.

Emulator: nothing is observed at all. The whole reported detail is `Error: Check timed out after 60.0s` — no status, no error type, no frame naming the call in flight. Everything in the check's deploy and setup phases is control plane the emulator does answer (CreateTable + table_exists, CreateRole/AttachRolePolicy/PutRolePolicy, CreateFunction, CreateResource/PutMethod/PutIntegration, CreateDeployment, GetMethod reporting authorizationType AWS_IAM, GetStage pointing at the deployment, PutItem/Scan/GetItem round-tripping the seeded agreement), as are the solve phase's read-back calls (GetResources paginated, GetMethod, ListRoles by PathPrefix, ListRolePolicies, GetRolePolicy, GetRole). The two remaining waits are signed GETs to the stage URL: the first polls until the response carries x-amzn-ErrorType: AccessDeniedException twice consecutively (3 s between probes), the second until a role holding the derived grant is answered 200 twice consecutively. One of those never converges, so the check is cut off at the cap, and the graded assertions — status 403 with x-amzn-ErrorType AccessDeniedException, the denial Message ending in the `...:<api-id>/<stage>/GET<path> because no identity-based policy allows the execute-api:Invoke action` template, and the granted role being served the seeded item — are never evaluated.

Why: a client cannot tell an IAM-authorized API apart from a broken one. A request that must be refused 403 with AccessDeniedException instead hangs, so no caller timeout or retry policy ever observes a decision; and a correctly granted role is never served its integration response, so the fix a client derives from live API Gateway/IAM state cannot be verified. A hang is strictly worse than a wrong status, because there is no code for the client to branch on.

Fix: the execute-api data-plane path for an AWS_IAM-authorized REST method on a deployed stage — it must always terminate in a method response: authorize the SigV4 identity for execute-api:Invoke against the request's method ARN and, on denial, serve 403 with the x-amzn-ErrorType: AccessDeniedException header and the documented Message template; on allow, run the method's integration and serve its response. Request-target normalisation (collapsing the double slash after the stage) belongs in the same front-door routing step, before method selection.

Traces: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle (replay detail `Error: Check timed out after 60.0s`, no frame).

## Evidence
Expected (AWS): with a REST API whose GET /data-agreements/{agreementId} is AWS_IAM-authorized and proxied to a Lambda that reads the item from DynamoDB, deployed to stage `beta`:
- a SigV4-signed GET of `<stage url>//data-agreements/<id>` from an assumed-role session whose only grant is execute-api:Invoke on `arn:aws:execute-api:<region>:<account>:<api-id>/beta/POST/*` returns HTTP 403 with header `x-amzn-errortype: AccessDeniedException` and a body whose Message ends `...:<api-id>/beta/GET/data-agreements/<id> because no identity-based policy allows the execute-api:Invoke action`;
- the same request from a session granted execute-api:Invoke on `.../beta/GET/*` returns HTTP 200 whose body is the seeded agreement JSON;
- the double slash after the stage does not change method selection.

Actual (emulator): neither response is ever observed. The complete replay detail is:
  Error: Check timed out after 60.0s
There is no frame, so the stalling call is not identifiable from the recording; the only non-converging waits in the check are the two signed stage invokes described above (each requires two consecutive matching responses within a 240 s deadline, which the 60 s cap truncates). Account id, region, api id, stage host/port, role names and ARNs, the IAM path and the generated table/function names are per-run values and are not requirements; the requirements are the 403 status, the AccessDeniedException error type, the denial Message template above, and the 200 serving the integration's body for the granted session.

## Steps to Reproduce
1. dynamodb.CreateTable (PAY_PER_REQUEST, HASH key agreementId:S) and wait for table_exists; iam.CreateRole + AttachRolePolicy(AWSLambdaBasicExecutionRole) + PutRolePolicy granting the DynamoDB read actions on the table; lambda.CreateFunction with a proxy handler that returns the item.
2. apigateway: CreateRestApi, CreateResource `/data-agreements` and `/{agreementId}`, PutMethod GET with authorizationType=AWS_IAM, PutIntegration AWS_PROXY to the function, lambda.AddPermission for apigateway.amazonaws.com on `arn:aws:execute-api:<region>:<account>:<api-id>/beta/GET/data-agreements/*`, CreateDeployment(stageName=beta).
3. iam.CreateRole under a unique Path, trusted by the account root, with one inline policy allowing execute-api:Invoke on `arn:aws:execute-api:<region>:<account>:<api-id>/beta/POST/*`.
4. dynamodb.PutItem the agreement keyed `dua-analytics-001`.
5. sts.AssumeRole into the restricted role and send a SigV4-signed (service execute-api) GET of `<stage url>//data-agreements/dua-analytics-001`.
   -> EXPECT HTTP 403, header x-amzn-errortype: AccessDeniedException, body Message ending `...:<api-id>/beta/GET/data-agreements/dua-analytics-001 because no identity-based policy allows the execute-api:Invoke action`.
6. Create a second role whose grant is the same ARN with GET in place of POST, assume it and repeat the same signed GET.
   -> EXPECT HTTP 200 whose body is the seeded agreement JSON.
   -> OBSERVE neither: the check is cut off at the 60 s harness cap with no frame.
7. Or replay the check id in trace_ids.
