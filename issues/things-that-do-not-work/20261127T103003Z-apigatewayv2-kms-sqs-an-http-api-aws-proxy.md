Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__kms__sqs.http-api-sqs-sendmessage-to-kms-queue-without-key-grant-rejected-until-granted
Issue-Key: apigatewayv2__kms__sqs.http-api-sqs-sendmessage-to-kms-queue-without-key-grant-rejected-until-granted|_xml_field#27|ParseError|for el in ET.fromstring(text).iter():
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# apigatewayv2__kms__sqs: an HTTP API AWS_PROXY SQS-SendMessage integration relays SQS's KMS refusal as a non-XML body

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: the 400 an HTTP API returns when its AWS_PROXY SQS-SendMessage integration is refused by KMS is not an SQS query-protocol XML body.
  AWS: recorded behavior only for the pass-through shape (SQS's own wire is the aws.query protocol, so its error body is XML: an <ErrorResponse>/<Error> document carrying <Code> and <Message>). The recording shows the route whose role may send to the queue but holds no kms:GenerateDataKey on the queue's CMK answering HTTP 400 with Code 'KMS.AccessDeniedException' and Message 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: kms:GenerateDataKey on resource: <key arn> because no identity-based policy allows the kms:GenerateDataKey action (Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: null)'. Fixed text: everything outside <role>, <session>, <key arn>, <account> and <id>, which come from the request, the queue's key and the per-request id.
  Emulator: the status is 400 as recorded, but the body cannot be parsed as XML at all — xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0 on the first byte, i.e. the integration returns a non-XML (JSON-looking) error document, so no <Code>/<Message> element exists.
  Why: a client reading the integration's response the way AWS serves it (SQS's XML error document, which is what an AWS_PROXY integration passes through verbatim) cannot extract the error code or message at all; error handling that branches on KMS.AccessDeniedException never fires.
  Fix: the apigatewayv2 AWS_PROXY integration's response relay for SQS targets — the upstream SQS error must be passed through in SQS's own query-protocol serialization rather than re-serialized.
  Traces: apigatewayv2__kms__sqs.http-api-sqs-sendmessage-to-kms-queue-without-key-grant-rejected-until-granted.

## Evidence
apigatewayv2__kms__sqs.http-api-sqs-sendmessage-to-kms-queue-without-key-grant-rejected-until-granted — the status assertion (400) passed, then the body parse failed:
File "check_...py", line 95, in _xml_field -> for el in ET.fromstring(text).iter(): -> xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 0.
Expected: an XML body whose <Code> is 'KMS.AccessDeniedException' and whose <Message> is SQS's relayed KMS denial naming the route's role and the queue's key.

## Steps to Reproduce
1. Create a symmetric CMK, an SQS queue encrypted with it, and two IAM roles: one allowed sqs:SendMessage + kms:GenerateDataKey/kms:Decrypt on the key, one allowed only sqs:SendMessage.
2. Create an HTTP API with AWS_PROXY SQS-SendMessage integrations for both roles and a $default auto-deploy stage.
3. SigV4-POST a JSON payload to the route backed by the role without key access.
4. Observe HTTP 400 whose body is not SQS query-protocol XML; AWS returns <ErrorResponse>...<Code>KMS.AccessDeniedException</Code><Message>User: ... is not authorized to perform: kms:GenerateDataKey on resource: <key arn> ... (Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: null)</Message>.
