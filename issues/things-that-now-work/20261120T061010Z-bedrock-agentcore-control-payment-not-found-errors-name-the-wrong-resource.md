Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-payment-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-payment-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreatePaymentConnector|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetPaymentConnector|4xx->4xx|$.message
Issue-Commit: e12f9e5cb3515bb2dfa0c1f17fb3c3b71d90760b

# Payment not-found errors name the wrong resource and omit the managerId label

> _Note: this issue was identified against the code base at commit `e12f9e5cb3515bb2dfa0c1f17fb3c3b71d90760b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePaymentConnector

## Description
The emulator's ResourceNotFoundException messages on the bedrock-agentcore-control payments surface diverge from AWS: one drops the labelled identifier AWS uses, and the other reports an entirely different resource class and identifier than the one the request asked for. The HTTP status (404) and the error type are correct in both cases; only the message body diverges, which is still a requirement. 2 causes, both in the payment connector handlers' not-found paths.

Cause 1 of 2: CreatePaymentConnector against an unknown payment manager omits the `managerId=` label from the not-found message.
  AWS: Smithy: CreatePaymentConnector POST /payments/managers/{paymentManagerId}/connectors, paymentManagerId is an @httpLabel @required member of PaymentManagerId; modeled error ResourceNotFoundException (@httpError 404). Recorded behavior: when the path's paymentManagerId names no existing manager, AWS answers 404 ResourceNotFoundException with the message `Payment manager not found: managerId=<paymentManagerId>`. Fixed text: `Payment manager not found: managerId=`; variable part: the paymentManagerId value taken verbatim from the request path (in the trace, the recorded run's value was a probe id, which is a value and not part of the template).
  Emulator: answers 404 ResourceNotFoundException but with `Payment manager not found: <paymentManagerId>` — the identifier is interpolated bare, with the `managerId=` key label dropped.
  Why: a client (or an operator reading logs) that parses the message for the labelled `managerId=<id>` key/value pair AWS emits finds no match, so error-reporting and remediation logic keyed on that label silently fails to extract which manager was missing.
  Fix: the CreatePaymentConnector handler's payment-manager lookup/not-found path in the bedrock-agentcore-control service crate — the message template used when the resolved paymentManagerId is absent.
  Traces: bedrock-agentcore-control-payment-error-paths#task#4.

Cause 2 of 2: GetPaymentConnector for an unknown connector reports the payment MANAGER as not found, naming the manager id instead of the connector id.
  AWS: Smithy: GetPaymentConnector GET /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}; both paymentManagerId and paymentConnectorId are @httpLabel @required members, and ResourceNotFoundException (@httpError 404) is modeled. Recorded behavior: for this request AWS answers 404 ResourceNotFoundException with `Payment connector not found: <paymentConnectorId>` — it reports the connector, the resource the operation is asked to read, and names the paymentConnectorId from the request path. Fixed text: `Payment connector not found: `; variable part: the paymentConnectorId value from the request path.
  Emulator: answers 404 but with `Payment manager not found: <paymentManagerId>` — the wrong resource class (manager rather than connector) and an identifier that is not the connector the caller asked for. Hypothesis: the handler resolves the parent manager first and returns that lookup's not-found error, never reaching the connector lookup whose error AWS actually surfaces.
  Why: a client cannot tell which resource is missing. Code that branches on the message (or on the named id) to decide whether to create the connector versus to create/repair the manager takes the wrong branch, and a caller retrying with a different connector id is told the manager is at fault.
  Fix: the GetPaymentConnector handler's lookup order and not-found error construction in the bedrock-agentcore-control service crate — it must report the connector-not-found error naming the request's paymentConnectorId for this operation.
  Traces: bedrock-agentcore-control-payment-error-paths#task#5.

No open issue covers either cause: the existing payment-related issues are for the separate payment-cryptography / payment-cryptography-data services and for the bedrock-agentcore-control DeletePaymentManager stub (20261118T170231Z-bedrock-agentcore-control-deletepaymentmanager-stub.md), none of which touch the payment connector not-found message paths.

## Evidence
Cause 1 — fixture bedrock-agentcore-control-payment-error-paths step task#4, CreatePaymentConnector:
  expected status 404, actual status 404 (status matches)
  $.message: value_mismatch — expected 'Payment manager not found: managerId=missingmgr-aaaaaaaaaa', got 'Payment manager not found: missingmgr-aaaaaaaaaa'
  (the recorded id missingmgr-aaaaaaaaaa is the request's value; the divergence is the missing fixed text 'managerId=')

Cause 2 — fixture bedrock-agentcore-control-payment-error-paths step task#5, GetPaymentConnector:
  expected status 404, actual status 404 (status matches)
  $.message: value_mismatch — expected 'Payment connector not found: missingconn-aaaaaaaaaa', got 'Payment manager not found: missingmgr-aaaaaaaaaa'
  (expected names the request's paymentConnectorId; the emulator names the paymentManagerId and the wrong resource class)

Both clusters: verdict=mismatch, service=bedrock-agentcore-control, stub_path empty, owned_by empty.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture 'bedrock-agentcore-control-payment-error-paths' step 'task#4' — call CreatePaymentConnector with a paymentManagerId that names no existing payment manager (POST /payments/managers/{paymentManagerId}/connectors). Observe the 404 body's $.message lacks the 'managerId=' label AWS emits.
4. Cause 2: replay the same fixture's step 'task#5' — call GetPaymentConnector (GET /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}) for a connector that does not exist. Observe the 404 body's $.message reports 'Payment manager not found: <managerId>' instead of 'Payment connector not found: <paymentConnectorId>'.
5. Both steps report verdict=mismatch on $.message with the status already matching at 404.
