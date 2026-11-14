Issue-Source: trace-sweep-divergence
Issue-Trace: route53domains-check-domain-transferability
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53domains-check-domain-transferability route53domains-domain-availability-and-transferability-checks
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains|CheckDomainTransferability|2xx->2xx|$.Message
Issue-Commit: 7178e01e2fdf9b066fd2aad4180957cad1a114a8

# route53domains CheckDomainTransferability Message explains an untransferable domain with emulator-invented wording

> _Note: this issue was identified against the code base at commit `7178e01e2fdf9b066fd2aad4180957cad1a114a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckDomainTransferability

## Description
ONE net-new root cause in this service's CheckDomainTransferability response body. The HTTP status (200) and Transferability.Transferable (UNTRANSFERABLE) are already correct; only the response's `Message` member diverges, so a client that branches on the enum is unaffected while any client, log consumer, runbook or assertion that reads or displays the explanation sees prose AWS never produces.

Cause 1 of 1 — the `Message` explaining why a domain the calling account does not hold cannot be transferred in uses the emulator's own "another registrant" explanation instead of AWS's registry-lock wording.

AWS: CheckDomainTransferability answers HTTP 200 with Transferability.Transferable and an explanatory Message — Smithy: CheckDomainTransferability -> CheckDomainTransferabilityResponse {Transferability: DomainTransferability, Message: Message (string), Price: PriceWithCurrency}; Transferable enum [TRANSFERABLE, UNTRANSFERABLE, DONT_KNOW, DOMAIN_IN_OWN_ACCOUNT, DOMAIN_IN_ANOTHER_ACCOUNT, PREMIUM_DOMAIN, PENDING]; the Message member's doc is "Provides an explanation for when a domain can't be transferred." The model does not state the sentence, so the wording is recorded behavior only: for a registered name that the calling account does not hold and that the registry will not release, the recording shows the fixed text "The domain " + <domain name> + " is locked at the registry. Unlock the domain at the registry and try again." — the fixed parts are the leading "The domain ", the clause " is locked at the registry. Unlock the domain at the registry and try again." including the trailing period; the single variable part is the DomainName taken from the request, unquoted and lower-cased. AWS attributes the refusal to the REGISTRY LOCK on the registration and tells the caller to clear that lock, which is the action a transfer-in actually requires (the registration must not be in clientTransferProhibited).

Emulator: HTTP 200, Transferable UNTRANSFERABLE (correct), Message "The domain <name> is registered with another registrant. Ask the current registrant to authorize the transfer and then try again." — entirely different fixed text, attributing the refusal to the identity of the registrant and directing the caller to a third party rather than to the registry lock.

Why: a caller is told to perform an action that is not the one that unblocks the transfer, and any client, test or support runbook that matches AWS's "is locked at the registry" explanation (or extracts the required remediation from it) never matches. The emulator's sentence appears in no AWS document or log.

Fix: the Message branch for a name held outside the calling account — Transferability::message's HeldElsewhere arm in the registrar's state module (crate::state::registry), reached from the CheckDomainTransferability handler's response construction. The handler's status, Transferable member and Price are correct and must not change.

Traces: route53domains-check-domain-transferability#task#1 (same cluster also covers route53domains-domain-availability-and-transferability-checks#task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, 60 steps, 16 passed, 8 divergence clusters).

Cause 1 — route53domains-check-domain-transferability#task#1, CheckDomainTransferability, expected status 200, actual status 200; diverging key $.Message:
  expected $.Message: 'The domain example.com is locked at the registry. Unlock the domain at the registry and try again.'
  actual   $.Message: 'The domain example.com is registered with another registrant. Ask the current registrant to authorize the transfer and then try again.'
  ('example.com' is the fixture's requested DomainName — the only variable part of the sentence; the divergence is the fixed text, not that value.)
  Cluster of 2 steps: route53domains-check-domain-transferability#task#1 and route53domains-domain-availability-and-transferability-checks#task#4.
  divergence_key: service=route53domains|CheckDomainTransferability|status=200->200|keys=$.Message

Smithy (services/service-com-amazonaws-route53domains-route53domains-v20140515/smithy-model/com-amazonaws-route53domains-route53domains-v20140515.json): CheckDomainTransferability output members Transferability (DomainTransferability), Message (Message), Price (PriceWithCurrency); DomainTransferability.Transferable targets the Transferable enum; Message member doc "Provides an explanation for when a domain can't be transferred."

Emulator source: services/service-com-amazonaws-route53domains-route53domains-v20140515/src/state/registry.rs:188-201 (Transferability::message, HeldElsewhere arm at lines 193-196), called from services/service-com-amazonaws-route53domains-route53domains-v20140515/src/ops/check_domain_transferability.rs:206.

Related open issues do NOT cover this: 20261110T154504Z-route53domains-refusal-and-constraint-violation-message-text-diverges.md and 20261109T193137Z-route53domains-refusal-messages-use-emulator-invented-wording-the.md both concern 4xx ERROR messages (InvalidInput / UnsupportedTLD / constraint-violation envelopes) and name other helpers; neither touches the 200-response Message of CheckDomainTransferability, and neither carries this fixture step as a replay key.

## Steps to Reproduce
Region us-east-1 against the emulator endpoint.

Cause 1: aws route53domains check-domain-transferability --domain-name example.com
  (any registered name the calling account does not hold reaches the same branch)
  -> expect HTTP 200 with Transferability.Transferable = UNTRANSFERABLE and
     Message = 'The domain example.com is locked at the registry. Unlock the domain at the registry and try again.'
     (fixed text as above, with the requested domain name substituted).
  -> observe instead Message = 'The domain example.com is registered with another registrant. Ask the current registrant to authorize the transfer and then try again.'

Replay check: replay fixture route53domains-check-domain-transferability step task#1 (and route53domains-domain-availability-and-transferability-checks step task#4, same cluster).
