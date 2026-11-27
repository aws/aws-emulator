Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__sqs.stage-fifo-send-without-group-id-rejected-as-400-nothing-enqueued
Issue-Key: apigateway__sqs.stage-fifo-send-without-group-id-rejected-as-400-nothing-enqueued|run#32|AssertionError|assert err == want, f"the stage's 400 carried {err!r}, expected {want!r}"
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# API Gateway stage proxying SQS SendMessage answers 400 with a body that is not an SQS Query <ErrorResponse>

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
One cause.

Cause 1 of 1: a REST API AWS-integration to SQS does not pass SQS's Query-protocol error document through to the caller.

AWS: an API Gateway AWS integration forwards the integrated service's response. SQS speaks the awsQuery protocol (Smithy: com.amazonaws.sqs, protocol aws.protocols#awsQuery; SendMessage's modelled client errors each carry an awsQueryError code), so a refusal is an XML document whose root element is `ErrorResponse` and which carries `Error/Type`, `Error/Code` and `Error/Message`. For a FIFO SendMessage with no MessageGroupId the refusal is HTTP 400 with Type `Sender`, Code `MissingParameter` and the fixed message `The request must contain the parameter MessageGroupId.` (that message has no variable part).

Emulator: the stage does answer HTTP 400, but the body is not an `ErrorResponse` document: parsing it as XML yields no ErrorResponse root, so the (Type, Code, Message) triple the check extracts is `None`. The sibling HTTP-API/AWS_PROXY paths in this suite serve `{"__type":"...","Message":"..."}` JSON on the same SQS refusals, which is consistent with a JSON error body being produced here too instead of the Query XML.

Why: a client reading the stage response with an XML/Query parser (every AWS SDK signing SQS through such a proxy, and every hand-written consumer of an SQS-backed API) cannot see the error Type/Code/Message at all. A refusal becomes an unparseable 400, so the caller cannot distinguish a missing MessageGroupId from any other client error and cannot retry correctly.

Fix: the SQS awsQuery error serializer, and the API Gateway AWS-integration response passthrough that must forward the integrated service's body verbatim.

Traces: apigateway__sqs.stage-fifo-send-without-group-id-rejected-as-400-nothing-enqueued.

## Evidence
Expected: HTTP 400 and an XML `<ErrorResponse>` resolving to ('Sender', 'MissingParameter', 'The request must contain the parameter MessageGroupId.').
Actual (suite report): `AssertionError: the stage's 400 carried None, expected ('Sender', 'MissingParameter', 'The request must contain the parameter MessageGroupId.')` — status was 400, but the body did not parse as an ErrorResponse document.
Fixed text of the required message: the whole string `The request must contain the parameter MessageGroupId.`; no part of it is taken from the request or from state.

## Steps to Reproduce
1. Create a FIFO queue and a REST API whose root POST method is an AWS integration to SQS SendMessage for that queue.
2. Deploy a stage and POST a body that does not set MessageGroupId.
3. Read the response: status is 400, but the payload is not an `<ErrorResponse>` XML document carrying Type/Code/Message.
Replay: apigateway__sqs.stage-fifo-send-without-group-id-rejected-as-400-nothing-enqueued.
