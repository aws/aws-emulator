Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.browser-delete-idempotent-replay-after-confirmed-gone bedrock-agentcore-control.payment-connector-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|DeleteBrowser|ResourceNotFoundException
Issue-Key: bedrock-agentcore-control.payment-connector-lifecycle-transitions|run#36|AssertionError|assert update_message == expected_update, f"expected message {expected_update!r}, got {update_message!r}"
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# bedrock-agentcore-control: 2 net-new causes — DeleteBrowser does not honour a replayed clientToken after the browser is gone, and UpdatePaymentConnector's not-found message omits the connectorId=/managerId= scoping

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBrowser

## Description
Two net-new root causes in the bedrock-agentcore-control control plane, both found by replaying behavioral checks against the emulator. Each is listed separately below with its own failing check id; a fixer must close BOTH before submitting.

=== Cause 1 of 2: DeleteBrowser forgets its idempotency token, so a replay of an accepted delete is refused 404 instead of answering the first call's result ===

AWS: Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json): DeleteBrowser is DELETE /browsers/{browserId}, success status 202, trait @idempotent, input member `clientToken` (ClientToken, @idempotencyToken, httpQuery "clientToken", @length 33..256), output members `browserId`, `status` (BrowserStatus) and `lastUpdatedAt` all @required; modelled errors include ResourceNotFoundException (client, httpError 404). This service's own model states the idempotency contract for its @idempotencyToken members: "A unique, case-sensitive identifier to ensure idempotency of the request" — and, spelled out on the sibling payment operations of the same service, "If this token matches a previous request, the service ignores the request, but doesn't return an error." So a DeleteBrowser REPLAYED with the clientToken a previous DeleteBrowser already accepted is served from that first call's result — it answers 202 with the same `browserId`, the same terminal `status` and that call's `lastUpdatedAt` — even once the browser itself is gone and ordinary reads of the id are the modelled 404. A delete of the same id carrying a DIFFERENT clientToken is NOT a replay and is the modelled 404 ResourceNotFoundException whose message is the fixed text "Browser with ID " + the browserId httpLabel from the request + the fixed text " does not exist" (variable part: the browserId; the id in any one run merely fills that template).

Emulator: the DeleteBrowser handler removes the record from the store and never records the request's clientToken anywhere, so the token is unavailable to any later call. Replaying the delete with the ORIGINAL clientToken, after GetBrowser has confirmed the browser gone, is refused `400/404 ResourceNotFoundException: Browser with ID <browserId> does not exist` — the same answer it gives a brand-new token — instead of returning the first call's browserId/status. Replay output: `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteBrowser operation: Browser with ID <browserId> does not exist`. The not-found message itself is correct; what diverges is that a token-matched replay is classified as not-found at all. (The same handler's sibling UpdatePaymentConnector/DeletePaymentConnector path in this service DOES remember tokens via `remember_client_token`/`replayed`, so the behaviour is simply missing on the browser plane.)

Why: a client relying on AWS's behavior cannot safely retry a delete. SDK retries, step-function/at-least-once workflows and teardown code all re-send DeleteBrowser with the same idempotency token after a timeout or a lost response; against AWS the retry succeeds and reports the terminal status, against the emulator it throws ResourceNotFoundException and the caller concludes the delete failed — turning a successful teardown into a hard error, and masking the real not-found case (a genuinely unknown id) behind the same code and message.

Fix: the DeleteBrowser handler and the browser plane's state. The handler must record the accepted delete against its `clientToken` (an idempotency record holding browserId, terminal status and lastUpdatedAt) and consult that record BEFORE the store lookup, so a token-matched replay returns the first call's output while a request with no/other token still falls through to the modelled 404. File and line citations: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/delete_browser.rs lines 163-187 (the handler removes the record and returns without ever reading or remembering `input.client_token`).

Traces: bedrock-agentcore-control.browser-delete-idempotent-replay-after-confirmed-gone.

=== Cause 2 of 2: UpdatePaymentConnector's ResourceNotFoundException message drops the connectorId=/managerId= labelling ===

AWS: Smithy: UpdatePaymentConnector is PATCH /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId} (both path members @required httpLabel), success 202, modelled error ResourceNotFoundException (client, httpError 404). The message wording is recorded behavior only: a mutation addressing a connector that does not exist under the addressed manager answers 404 ResourceNotFoundException whose message is the fixed text "Payment connector not found: connectorId=", then the paymentConnectorId taken from the request URI, then the fixed text ", managerId=", then the paymentManagerId taken from the request URI. Fixed text: "Payment connector not found: connectorId=" and ", managerId=". Variable parts: the two path-label ids from the request. Note the READ path differs and is already correct: GetPaymentConnector's not-found message is the unlabelled "Payment connector not found: " + paymentConnectorId, and the same check asserts and passes that form — so this is a per-operation message, not one shared helper's wording.

Emulator: UpdatePaymentConnector answers the correct 404 and the correct ResourceNotFoundException code, but reuses the read path's helper and produces the unlabelled, manager-less message. Replay output: `AssertionError: expected message 'Payment connector not found: connectorId=<connectorId>, managerId=<managerId>', got 'Payment connector not found: <connectorId>'` — the "connectorId=" label and the entire ", managerId=<paymentManagerId>" clause are missing.

Why: a client relying on AWS's behavior loses the scoping the error is meant to carry. A connector id is only meaningful under a manager, so AWS's labelled message says which half of the pair is missing; the emulator's cannot. Tests, log scrapers and support tooling that parse the connectorId=/managerId= pairs out of these AgentCore payment errors match nothing, and the mutation path becomes indistinguishable from the read path even though AWS words them differently.

Fix: the error construction in the UpdatePaymentConnector handler — the not-found branch that resolves a connector under its manager. It must format the labelled message from BOTH request path labels rather than calling the read path's connector-id-only helper. This is the same defect class as the already-open issue 20261127T133648Z-bedrock-agentcore-control-deletepaymentconnector-not-found-message-omits-the-manager-connector.md, which covers DeletePaymentConnector only; UpdatePaymentConnector is a separate call site and is not covered by it. File and line citations: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_payment_connector.rs lines 249-253 (`crate::state::payment_connector_not_found(&input.payment_connector_id)`), and the shared helper `payment_connector_not_found` in src/state.rs which must not be reused verbatim for the mutation paths.

Traces: bedrock-agentcore-control.payment-connector-lifecycle-transitions.

## Evidence
Cause 1 — check bedrock-agentcore-control.browser-delete-idempotent-replay-after-confirmed-gone (verdict: error).
Sequence: CreateBrowser(name, networkConfiguration={networkMode: PUBLIC}) -> GetBrowser polled to status READY -> DeleteBrowser(browserId, clientToken=T1) accepted, returns browserId -> GetBrowser polled until ResourceNotFoundException -> DeleteBrowser(browserId, clientToken=T1) REPLAYED.
Expected (AWS): the replay is accepted, returning the first call's browserId and the first call's status (202 with required browserId/status/lastUpdatedAt).
Actual (emulator): ResourceNotFoundException — 'An error occurred (ResourceNotFoundException) when calling the DeleteBrowser operation: Browser with ID <browserId> does not exist'.
Fixed text of that message: 'Browser with ID ' + ' does not exist'; variable part: the browserId httpLabel from the request. The message is correct for a NON-replay; the defect is that a token-matched replay is answered with it at all.
The later assertions of the same check — GetBrowser and a DeleteBrowser with a DIFFERENT clientToken (T2) both failing ResourceNotFoundException with exactly 'Browser with ID <browserId> does not exist' — are AWS-correct and are never reached.
Smithy: DeleteBrowser DELETE /browsers/{browserId}, @idempotent, http success 202; clientToken @idempotencyToken httpQuery; output browserId/status/lastUpdatedAt @required; ResourceNotFoundException httpError 404.
Code: src/ops/delete_browser.rs:163-187 removes the record (store.remove_browser) and returns record.delete_output() without ever reading input.client_token.

Cause 2 — check bedrock-agentcore-control.payment-connector-lifecycle-transitions (verdict: mismatch).
Sequence: CreatePaymentCredentialProvider(StripePrivy) -> CreatePaymentManager -> poll to READY -> CreatePaymentConnector -> poll to READY -> UpdatePaymentConnector -> poll back to READY -> DeletePaymentConnector -> GetPaymentConnector polled until refused -> UpdatePaymentConnector against the now-dead connector id.
GetPaymentConnector after the delete: ResourceNotFoundException with 'Payment connector not found: <connectorId>' — asserted and PASSES.
UpdatePaymentConnector against the dead id: code ResourceNotFoundException is correct; message diverges.
  expected: 'Payment connector not found: connectorId=<connectorId>, managerId=<managerId>'
  actual:   'Payment connector not found: <connectorId>'
Assertion text from the replay: "AssertionError: expected message 'Payment connector not found: connectorId=sim2realconn...-7aqwauvu70, managerId=sim2realpcm...-6gsmdcuals', got 'Payment connector not found: sim2realconn...-7aqwauvu70'" — the ids shown are that run's generated values filling the template, not requirements.
Fixed text: 'Payment connector not found: connectorId=' and ', managerId='. Variable parts: the paymentConnectorId and paymentManagerId httpLabel values from the request URI.
Smithy: UpdatePaymentConnector PATCH /payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}; ResourceNotFoundException httpError 404.
Code: src/ops/update_payment_connector.rs:249-253 builds the error with crate::state::payment_connector_not_found(&input.payment_connector_id), the connector-id-only helper the read path shares.

## Steps to Reproduce
Cause 1 (DeleteBrowser idempotent replay):
1. Clone the repo and boot the emulator built from it.
2. Replay behavioral check bedrock-agentcore-control.browser-delete-idempotent-replay-after-confirmed-gone, or by hand:
3. bedrock-agentcore-control CreateBrowser(name=<sandbox name>, networkConfiguration={networkMode: 'PUBLIC'}); poll GetBrowser until status != CREATING and assert READY.
4. DeleteBrowser(browserId=<id>, clientToken=<T1>) — note the returned browserId and status.
5. Poll GetBrowser(browserId=<id>) until it answers ResourceNotFoundException.
6. Re-issue DeleteBrowser(browserId=<id>, clientToken=<T1>) with the SAME token.
7. Observe 404 ResourceNotFoundException 'Browser with ID <id> does not exist' instead of a 202 echoing step 4's browserId and status.
8. Also confirm the control: DeleteBrowser(browserId=<id>, clientToken=<a different T2>) must still be the 404 with that same message.

Cause 2 (UpdatePaymentConnector not-found message):
1. With the same emulator running, replay behavioral check bedrock-agentcore-control.payment-connector-lifecycle-transitions, or by hand:
2. CreatePaymentCredentialProvider(credentialProviderVendor='StripePrivy', ...) and CreatePaymentManager(authorizerType='AWS_IAM', roleArn=<role>); poll GetPaymentManager until READY.
3. CreatePaymentConnector(paymentManagerId=<mgr>, name=<n>, type='StripePrivy', credentialProviderConfigurations=[{stripePrivy:{credentialProviderArn:<cred arn>}}]); poll GetPaymentConnector until READY.
4. DeletePaymentConnector(paymentManagerId=<mgr>, paymentConnectorId=<conn>); poll GetPaymentConnector until it is refused (ResourceNotFoundException, message 'Payment connector not found: <conn>').
5. Issue PATCH /payments/managers/<mgr>/connectors/<conn> (UpdatePaymentConnector) against the deleted connector.
6. Observe the 404 body message is 'Payment connector not found: <conn>' instead of 'Payment connector not found: connectorId=<conn>, managerId=<mgr>'.
