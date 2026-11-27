Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__sqs__sts.aws-iam-route-invoke-policy-scoped-to-one-route-forbids-the-sibling-route apigatewayv2__iam__sqs__sts.sqs-sendmessage-integration-role-not-trusting-apigateway-delivers-nothing apigatewayv2__iam__sqs__sts.sqs-sendmessage-integration-role-without-send-permission-delivers-nothing
Issue-Key: apigatewayv2__iam__sqs__sts.aws-iam-route-invoke-policy-scoped-to-one-route-forbids-the-sibling-route|run#30|AssertionError|assert status == 403, (
Issue-Key: apigatewayv2__iam__sqs__sts.sqs-sendmessage-integration-role-not-trusting-apigateway-delivers-nothing|run#28|AssertionError|assert status == 500, (
Issue-Key: apigatewayv2__iam__sqs__sts.sqs-sendmessage-integration-role-without-send-permission-delivers-nothing|_sqs_error#30|AssertionError|raise AssertionError(f"expected an SQS ErrorResponse, got {text[:300]!r}") from None
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# apigatewayv2 execute-api: route-scoped AWS_IAM authorization and the SQS integration's credentials role are both unenforced, and the integration relays SQS errors as JSON instead of Query XML (3 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api) / CreateIntegration

## Description
Three net-new causes on the HTTP-API execute-api data plane with an AWS_PROXY SQS-SendMessage integration.

Cause 1 of 3: an AWS_IAM route serves a request whose caller policy does not name that route.
AWS: recorded behavior only for the data-plane decision (authorization is per-route, against the route's own `arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/<method>/<route path>` resource). A session whose identity policy allows `execute-api:Invoke` on ONE route's ARN is served that route and REFUSED the sibling route with HTTP 403; the forbidden route's integration never runs, so nothing is delivered to the backend.
Emulator: HTTP 200 for the sibling route the policy does not name, and the SQS integration runs — the reply is a `<SendMessageResponse>` carrying a real MD5OfMessageBody and MessageId, i.e. the message was actually enqueued.
Why: an AWS_IAM route is not a boundary at all. A caller scoped to one route reaches every route of the API, and the side effect (a queued message) is permanent, so a least-privilege policy provides no isolation and the 403 class is unobservable.
Fix: the execute-api front door's authorizer for AuthorizationType=AWS_IAM — build the per-route execute-api resource ARN and authorize the signed caller against it before dispatching the integration.
Traces: apigatewayv2__iam__sqs__sts.aws-iam-route-invoke-policy-scoped-to-one-route-forbids-the-sibling-route.

Cause 2 of 3: an integration whose CredentialsArn role does not trust apigateway.amazonaws.com still delivers.
AWS: recorded behavior only. The integration assumes the role named by CredentialsArn; a role whose trust policy does not allow `sts:AssumeRole` for the `apigateway.amazonaws.com` service principal cannot be assumed, so the request fails with HTTP 500 and NOTHING is delivered to the queue, while the sibling route whose role does trust API Gateway succeeds.
Emulator: HTTP 200 and a `<SendMessageResponse>` with a MessageId — the message is enqueued even though the integration role is not assumable by API Gateway.
Why: the integration's trust relationship is never exercised, so a misconfigured role is indistinguishable from a correct one. A client cannot detect the single most common integration-role misconfiguration, and the 500 failure class never appears.
Fix: the execute-api integration dispatch path — assume the integration's CredentialsArn role (honouring the role's trust policy for the apigateway service principal) and fail the request 500 when the assume is refused, instead of calling the backend with ambient credentials.
Traces: apigatewayv2__iam__sqs__sts.sqs-sendmessage-integration-role-not-trusting-apigateway-delivers-nothing.

Cause 3 of 3: the relayed SQS refusal is serialized as JSON, not as the Query-protocol XML ErrorResponse the same integration uses for success.
AWS: recorded behavior only. The SQS-SendMessage AWS_PROXY integration relays SQS's Query-protocol response verbatim, so a refusal arrives as an XML `<ErrorResponse><Error><Type>Sender</Type><Code>AccessDenied</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>` document — the same wire shape family as the `<SendMessageResponse>` the success path returns. The message's fixed text is "User: " + the integration role's session ARN + " is not authorized to perform: sqs:sendmessage on resource: " + the queue ARN + " because no identity-based policy allows the sqs:sendmessage action"; the ARNs come from the request and the emulator's state.
Emulator: the body is a JSON document — `{"__type":"AccessDenied","Message":"User: <session arn> is not authorized to perform: sqs:sendmessage on resource: <queue arn> because no identity-based p…"}` — while the SAME integration's success path (causes 1 and 2 above) returns XML. The code and message text are right; only the serialization is wrong, and it is inconsistent with the emulator's own success path.
Why: a client parsing the integration's response as XML (the only shape it ever sees on success) cannot read the error at all: XML parsing fails and the refusal degrades into an unparseable body, so the caller sees neither the code nor the message and cannot distinguish a permission failure from a corrupt response.
Fix: the SQS integration relay's error serializer — serialize the refusal in the Query protocol (XML ErrorResponse) that the success path already uses. Note the open issue 20261127T103003Z-apigatewayv2-kms-sqs-an-http-api-aws-proxy.md reports a non-XML relayed body for SQS's KMS refusal; it does not carry this check's replay key and does not cover the AccessDenied path reported here.
Traces: apigatewayv2__iam__sqs__sts.sqs-sendmessage-integration-role-without-send-permission-delivers-nothing.

## Evidence
Cause 1 — expected HTTP 403 for the unnamed route. Actual, verbatim: 'AssertionError: a route the caller\'s policy does not name must be refused with 403, got 200: \'<SendMessageResponse xmlns="http://queue.amazonaws.com/doc/2012-11-05/"><SendMessageResult><MD5OfMessageBody>…</MD5OfMessageBody><MessageId>…</MessageId></SendMessageResult>…\''

Cause 2 — expected HTTP 500 and nothing delivered. Actual, verbatim: 'AssertionError: a role API Gateway may not assume must fail the request with 500, got 200: \'<SendMessageResponse xmlns="http://queue.amazonaws.com/doc/2012-11-05/"><SendMessageResult><MD5OfMessageBody>…</MD5OfMessageBody><MessageId>…</MessageId></SendMessageResult>…\''

Cause 3 — expected an SQS XML ErrorResponse. Actual, verbatim: 'AssertionError: expected an SQS ErrorResponse, got \'{"__type":"AccessDenied","Message":"User: arn:aws:sts::<account>:assumed-role/<role>/BackplaneAssumeRoleSession is not authorized to perform: sqs:sendmessage on resource: arn:aws:sqs:<region>:<account>:<queue> because no identity-based p\''

MD5s, MessageIds, api ids, role/queue names and session names above are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Create an SQS queue and an HTTP API with two routes, each an AWS_PROXY SQS-SendMessage integration with its own CredentialsArn.
2. Cause 1: set AuthorizationType=AWS_IAM on both routes; assume a role whose policy allows execute-api:Invoke on ONLY the first route's ARN and send a SigV4-signed request to the SECOND route. Expect 403; observe 200 and a queued message.
3. Cause 2: give the second route a CredentialsArn whose trust policy does NOT allow sts:AssumeRole for apigateway.amazonaws.com, and invoke it. Expect 500 and nothing delivered; observe 200 and a queued message.
4. Cause 3: give a route a CredentialsArn role that API Gateway may assume but that holds NO sqs:SendMessage grant, and invoke it. Expect an XML <ErrorResponse> with Code AccessDenied; observe a JSON {"__type":"AccessDenied",…} body.
