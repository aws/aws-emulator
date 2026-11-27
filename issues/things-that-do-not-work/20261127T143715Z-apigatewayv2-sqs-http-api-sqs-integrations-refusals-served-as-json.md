Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-above-ten-messages-rejected apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-without-send-permission-rejected-until-granted apigatewayv2__sqs.http-api-sqs-sendmessage-role-without-send-permission-rejected-until-granted apigatewayv2__sqs.sendmessage-subtype-to-queue-outside-role-scope-denied-nothing-delivered apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-carries-mapped-message-attributes
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-above-ten-messages-rejected|_fault#23|ParseError|root = ET.fromstring(text)
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-without-send-permission-rejected-until-granted|_error_of#20|ParseError|root = ET.fromstring(text)
Issue-Key: apigatewayv2__sqs.http-api-sqs-sendmessage-role-without-send-permission-rejected-until-granted|_xml_field#24|ParseError|for el in ET.fromstring(text).iter():
Issue-Key: apigatewayv2__sqs.sendmessage-subtype-to-queue-outside-role-scope-denied-nothing-delivered|_sqs_error#25|ParseError|err = ET.fromstring(text).find("{http://queue.amazonaws.com/doc/2012-11-05/}Error")
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-carries-mapped-message-attributes|_poll_http#11|AssertionError|raise AssertionError(f"{method} {url} never answered {want}; last {last[0]!r}: {last[2][:300]!r}")
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# HTTP API SQS integrations: refusals served as JSON instead of the Query XML ErrorResponse, and a mapped-attribute SendMessage answers 500

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Two causes.

Cause 1 of 2: an SQS refusal reaching an HTTP API AWS_PROXY / SQS-subtype integration is served as a JSON error body, not SQS's Query-protocol XML ErrorResponse.
AWS: SQS is an awsQuery service (Smithy: com.amazonaws.sqs with aws.protocols#awsQuery; ReceiveMessage/SendMessage client errors each carry an awsQueryError code and httpResponseCode), so every refusal is an XML document rooted at `ErrorResponse` with `Error/Type` (`Sender` for a client error), `Error/Code` and `Error/Message`. An HTTP API AWS_PROXY integration forwards that document. For ReceiveMessage with MaxNumberOfMessages above 10 the code is `InvalidParameterValue`; for a send the caller's role may not perform the refusal is `AccessDenied`. The message's fixed text is the error-class wording and its variable parts are the parameter name, the principal ARN, the action and the queue ARN taken from the request and from state.
Emulator: the body is not XML at all — `xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0`, i.e. the first byte is not `<`. The same emulator path, exercised under the apigatewayv2__iam__sqs__sts heading, serves `{"__type":"AccessDenied","Message":"User: ... is not authorized to perform: sqs:sendmessage on resource: ... because no identity-based policy allows ..."}`, so the error is being serialized in the JSON form instead of the Query form.
Why: a client parsing the integration response as SQS XML (every AWS SDK and every consumer written against an SQS-backed HTTP API) sees an unparseable body and loses the error Type/Code/Message entirely; an authorization refusal is indistinguishable from a malformed response.
Fix: the SQS awsQuery error serializer and the apigatewayv2 AWS_PROXY/SQS-subtype integration response path, which must forward the integrated service's wire form.
Traces: apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-above-ten-messages-rejected, apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-without-send-permission-rejected-until-granted, apigatewayv2__sqs.http-api-sqs-sendmessage-role-without-send-permission-rejected-until-granted, apigatewayv2__sqs.sendmessage-subtype-to-queue-outside-role-scope-denied-nothing-delivered.

Cause 2 of 2: an AWS_PROXY SQS-SendMessage subtype integration that maps MessageAttributes answers HTTP 500.
AWS: an HTTP API SQS-SendMessage subtype integration whose request parameters map `MessageAttributes` sends the attributes with the message and answers 200 with the SendMessage result (Smithy: SendMessage.MessageAttributes targets MessageBodyAttributeMap; the modelled success carries MessageId and MD5OfMessageBody). No modelled error of SendMessage is a 500 for a well-formed attribute map.
Emulator: the stage never answers 200; every attempt answers HTTP 500 with the body `{"message":"Internal Server Error"}`.
Why: a client cannot send message attributes through an SQS subtype integration at all, and a 500 is retried by SDK retry policies, so the caller both loses the feature and burns its retry budget.
Fix: the apigatewayv2 SQS-SendMessage subtype integration's parameter mapping for MessageAttributes (request mapping, then the SendMessage handler).
Traces: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-carries-mapped-message-attributes.

## Evidence
Cause 1 — expected: HTTP 4xx whose body is an `<ErrorResponse>` XML document with Type/Code/Message (e.g. Sender / AccessDenied / `User: <role-session-arn> is not authorized to perform: sqs:SendMessage on resource: <queue-arn> because no identity-based policy allows the sqs:SendMessage action`). Actual: `xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0` in all four checks (_fault/_error_of/_xml_field/_sqs_error helpers). Related evidence from the same emulator path: body `{"__type":"AccessDenied","Message":"User: arn:aws:sts::<acct>:assumed-role/<role>/<session> is not authorized to perform: sqs:sendmessage on resource: <queue-arn> because no identity-based p..."}`.
Cause 2 — expected: HTTP 200 with the SendMessage result. Actual: `POST https://<host>/_aws/execute-api/<api-id>/send never answered 200; last 500: '{"message":"Internal Server Error"}'`.

## Steps to Reproduce
Cause 1: create an HTTP API with an AWS_PROXY integration of subtype SQS-SendMessage (or SQS-ReceiveMessage) to a queue, with an execution role that lacks sqs:SendMessage (or request MaxNumberOfMessages=11), invoke the route, and parse the response body as XML — it is not XML.
Cause 2: create an HTTP API AWS_PROXY SQS-SendMessage subtype integration that maps MessageAttributes from the request and invoke it; the stage answers 500.
Replay: the four Cause-1 check ids and apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-carries-mapped-message-attributes.
