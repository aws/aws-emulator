Issue-Source: trace-sweep-divergence
Issue-Trace: route53domains-domain-input-validation
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53domains-domain-input-validation route53domains-get-domain-suggestions
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains|GetDomainSuggestions|4xx->4xx|$.message
Issue-Commit: 0be226932e2bdefbd81de54f36a0c43b623e9eb6

# route53domains InvalidInput message for a single-label domain name uses constraint wording instead of "Give domain name must contain more than 1 label"

> _Note: this issue was identified against the code base at commit `0be226932e2bdefbd81de54f36a0c43b623e9eb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainSuggestions

## Description
Cause 1 of 1: when a request's DomainName carries only ONE label (no dot, hence no top-level domain), route53domains refuses with the right status and the right error code but the WRONG message.

AWS: Route 53 Domains models InvalidInput on GetDomainSuggestions as a client error with HTTP 400 (Smithy: GetDomainSuggestions errors InvalidInput/TLDInMaintenance/UnsupportedTLD, all httpError 400; input member DomainName is required, target DomainName @length(0,255)). The model does not state the refusal wording, so the required message is recorded behavior only: for a name with fewer than two labels the service answers InvalidInput with the message "Invalid request. Problems: Give domain name must contain more than 1 label". That message is entirely FIXED TEXT: the service's request-problem envelope "Invalid request. Problems: " followed by the fixed problem sentence "Give domain name must contain more than 1 label". It contains NO variable part — in particular it does not echo the requested name, so the probe-generated label in the recording (a per-run value) is not part of the requirement.

Emulator: for the same input it answers 400 with the generic constraint clause instead — "Value '<domainName as sent>' at 'domainName' failed to satisfy constraint: Member must be a domain name of one label under a top-level domain, where every label is made up of letters, digits and interior hyphens". Here the fixed text is "Value '" / "' at 'domainName' failed to satisfy constraint: Member must be a domain name of one label under a top-level domain, where every label is made up of letters, digits and interior hyphens" and the one variable part is the name taken from the request. The status (400) and the error code (InvalidInput) match; only $.message diverges, on every GetDomainSuggestions step in the corpus that sends a single-label name (6 steps across two fixtures).

Why: a client that keys on the refusal text — a registration/suggestion front end that tells the user "add a TLD", a test asserting the service's documented sentence, or any log-scraping assertion — sees a message AWS never produces for this condition, so it cannot distinguish "name has no TLD" (which the real service calls out explicitly and which the caller fixes by appending a TLD) from the generic syntax refusal the emulator reports for every other malformed name. The condition is also conflated with character-syntax failures, which AWS words differently.

Fix: the service's shared domain-name refusal wording, i.e. the InvalidInput message builder every DomainName-taking operation shares, not GetDomainSuggestions alone. The label-count failure needs its own branch in that builder, in the request-problem envelope the crate already has (alongside the existing blank-name and label-too-long branches), so that a name resolving to fewer than two labels reports the fixed sentence above while other syntax failures keep their current wording. GetDomainSuggestions' handler already routes DomainNameError::Invalid to InvalidInput, so only the message selection is wrong; every other operation that resolves a DomainName inherits the same fix.

## Evidence
Fixture route53domains-domain-input-validation step task#4, operation GetDomainSuggestions.
Expected (recorded AWS): HTTP 400, InvalidInput, $.message = "Invalid request. Problems: Give domain name must contain more than 1 label".
Actual (emulator): HTTP 400, InvalidInput, $.message = "Value 'sim2real-probe-r53d-sugg-98f5f9b2' at 'domainName' failed to satisfy constraint: Member must be a domain name of one label under a top-level domain, where every label is made up of letters, digits and interior hyphens" (the quoted name is the per-run probe value filling the emulator's template, not part of the requirement).
Replay verdict: mismatch, divergence_key service=route53domains|GetDomainSuggestions|status=400->400|keys=$.message, 6 affected steps:
  route53domains-domain-input-validation#task#4, #task#5, #task#6
  route53domains-get-domain-suggestions#task#3, #task#4, #task#5
Source of the wrong wording: services/service-com-amazonaws-route53domains-route53domains-v20140515/src/state.rs lines 420-437 (invalid_domain_name_message falls through to the constraint clause, having branches only for a blank name and an over-long label) reached from resolve_domain_name's labels.len() < 2 check at lines 377-380, via src/ops/get_domain_suggestions.rs lines 139-157 and 197-198.
Unrelated open issue for this service (does NOT cover this): "Implement route53domains ListPrices: unimplemented scaffold stub" (20261114T002217Z-route53domains-listprices-stub.md).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Or call GetDomainSuggestions directly with a DomainName of a single label (no dot), e.g. {"DomainName": "example-no-tld", "SuggestionCount": 5, "OnlyAvailable": true}.
3. Observe HTTP 400 InvalidInput whose message is the constraint clause "Value '<name>' at 'domainName' failed to satisfy constraint: Member must be a domain name of one label under a top-level domain, ...".
4. AWS answers HTTP 400 InvalidInput with the fixed message "Invalid request. Problems: Give domain name must contain more than 1 label".
5. Re-replay fixtures route53domains-domain-input-validation (task#4-#6) and route53domains-get-domain-suggestions (task#3-#5) to confirm a fix.
