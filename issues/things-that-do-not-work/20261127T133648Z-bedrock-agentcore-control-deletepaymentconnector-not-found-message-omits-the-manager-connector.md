Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-payment-error-paths
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-payment-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|DeletePaymentConnector|4xx->4xx|$.message
Issue-Commit: 461e190cfbed2a60f922688bafee74a0fd88eec8

# DeletePaymentConnector not-found message omits the manager/connector key=value scoping

> _Note: this issue was identified against the code base at commit `461e190cfbed2a60f922688bafee74a0fd88eec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePaymentConnector

## Description
Cause 1 of 1: DeletePaymentConnector's ResourceNotFoundException message is built from the connector id alone, dropping the labelled key=value form and the parent payment-manager id that AWS includes.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol): DeletePaymentConnector is DELETE /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}, both path members @required httpLabel, modelled error ResourceNotFoundException with httpError 404. The message wording is recorded behavior only: when the addressed connector does not exist under the addressed manager, AWS answers 404 ResourceNotFoundException whose message is the fixed text "Payment connector not found: connectorId=", then the paymentConnectorId taken from the request URI, then the fixed text ", managerId=", then the paymentManagerId taken from the request URI. Fixed text: "Payment connector not found: connectorId=", ", managerId=". Variable parts: both ids, which come from the request path labels (never a literal — the recorded run's missingconn-bbbbbbbbbb / missingmgr-aaaaaaaaaa merely fill the template).

Emulator: answers the correct 404 and the correct ResourceNotFoundException code, but the message is "Payment connector not found: " followed by the bare connectorId only — no "connectorId=" label and no ", managerId=<paymentManagerId>" clause at all. Replay reports $.message value_mismatch (expected "Payment connector not found: connectorId=missingconn-bbbbbbbbbb, managerId=missingmgr-aaaaaaaaaa", got "Payment connector not found: missingconn-bbbbbbbbbb").

Why: a client relying on AWS's behavior loses the scoping information the error is supposed to carry. Because a connector id is only meaningful under a manager, the AWS message disambiguates whether the manager or the connector is the missing half; the emulator's message cannot. Tests, log scrapers and support tooling that parse the labelled connectorId=/managerId= pairs out of the message — a common pattern for these AgentCore payment errors — match nothing against the emulator, and operators debugging against the emulator see a different error string than production for the identical request.

Fix: the error construction in the DeletePaymentConnector handler of the bedrock-agentcore-control payment plane — the not-found branch that resolves a connector under its manager. It must format the message from BOTH request path labels using the labelled template above, instead of interpolating the connector id alone.

Traces: bedrock-agentcore-control-payment-error-paths#task#6.

## Evidence
Fixture bedrock-agentcore-control-payment-error-paths, step task#6, operation DeletePaymentConnector.
Status: expected 404, got 404 (status and error code agree).
Body divergence — $.message value_mismatch:
  expected: 'Payment connector not found: connectorId=missingconn-bbbbbbbbbb, managerId=missingmgr-aaaaaaaaaa'
  actual:   'Payment connector not found: missingconn-bbbbbbbbbb'
Fixed text of the expected message: 'Payment connector not found: connectorId=' + ', managerId='.
Variable parts: the paymentConnectorId and paymentManagerId httpLabel values from the request URI.
The emulator drops the 'connectorId=' label and the entire ', managerId=<paymentManagerId>' clause.
Smithy: DeletePaymentConnector DELETE /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}; errors include ResourceNotFoundException (client, httpError 404).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus fixture 'bedrock-agentcore-control-payment-error-paths' and run to step task#6.
3. That step issues DELETE /payments/managers/<nonexistent-managerId>/connectors/<nonexistent-connectorId> against bedrock-agentcore-control.
4. Observe the 404 ResourceNotFoundException body: $.message is 'Payment connector not found: <connectorId>' instead of 'Payment connector not found: connectorId=<connectorId>, managerId=<managerId>'.
5. Compare $.message against the recorded response for the same step.
