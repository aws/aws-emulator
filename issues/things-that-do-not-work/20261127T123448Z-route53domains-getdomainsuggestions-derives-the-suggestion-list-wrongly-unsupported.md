Issue-Source: trace-sweep-divergence
Issue-Trace: route53domains-suggestions-for-unregistrable-tld-seed
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53domains-suggestions-for-unregistrable-tld-seed
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains|GetDomainSuggestions|2xx->4xx|$.SuggestionsList
Issue-Key: route53domains|GetDomainSuggestions|2xx->4xx|status
Issue-Key: route53domains|GetDomainSuggestions|2xx->2xx|$.SuggestionsList[*]
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# route53domains GetDomainSuggestions derives the suggestion list wrongly: unsupported-TLD seed refused, and TLD order taken from the rate table

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainSuggestions

## Description
Two causes, both in how GetDomainSuggestions derives the names it answers with. The requested DomainName is only the BASIS for a list of possible names, so neither the seed's own TLD being one this registrar sells, nor the order of the registrar's internal rate table, is allowed to decide the answer. The emulator lets both decide it.

Cause 1 of 2: a seed DomainName whose top-level domain is not a registrable suffix is REFUSED instead of being used as a basis.
AWS: GetDomainSuggestions takes a required DomainName (Smithy: GetDomainSuggestions input DomainName required, target DomainName @length(0,255); output SuggestionsList: DomainSuggestionsList; errors InvalidInput / TLDInMaintenance / UnsupportedTLD, all errorKind client, httpError 400). The member is documented as "[a] domain name that you want to use as the basis for a list of possible domain names". The model does not say that a seed under a suffix the registrar does not sell is refused, and what the service actually does here is recorded behavior only: for a well-formed two-label seed whose TLD is not a TLD this registrar offers, the real service answers HTTP 200 with a SuggestionsList carrying the seed's second-level label under the registrable TLDs (the recording shows the label under .com, .net and .org). So UnsupportedTLD is NOT the refusal for "the seed's TLD is not in the catalogue" on this operation; the operation degrades to suggesting registrable names instead.
Emulator: the same request is refused with HTTP 400 and no SuggestionsList at all. The refusal comes from the shared name resolver, not from anything specific to suggestions: crate::state::resolve_domain_name requires everything after the first label to be a known public suffix and otherwise returns DomainNameError::UnsupportedTld, which GetDomainSuggestions' refuse_domain_name turns into the modeled UnsupportedTLD error whose message is the fixed "The " + the seed's TLD (taken from the request, lower-cased) + " TLD is not supported" (crate::state::unsupported_tld_message). Hypothesis: the error code on the wire is therefore UnsupportedTLD — the replay detail records the status (400) and the absent SuggestionsList, and that is the only 400 path this input can reach in the handler.
Why: a client that uses GetDomainSuggestions exactly as documented — hand it any name the user typed, including one under a suffix nobody registers under, and show the alternatives it comes back with — gets a hard 400 from the emulator where AWS returns a usable list. The caller cannot tell a seed it should sanitize from a seed AWS happily accepts, and the "suggest me something registrable" case, which is the whole point of passing an unregistrable name, is unreachable.
Fix: GetDomainSuggestions' handler (the seed-resolution step in src/ops/get_domain_suggestions.rs) must stop routing the seed through the registrar-grade resolver used by domain-scoped operations: a seed only needs to be a syntactically valid name, and an unregistrable or unknown suffix must select the registrable TLDs as the suffixes to suggest under rather than refuse the call. The syntax refusals (InvalidInput) stay. Alternatively the seed-specific relaxation belongs beside the derivation in src/state/suggestions.rs, so no other DomainName-taking operation loosens.
Traces: route53domains-suggestions-for-unregistrable-tld-seed#task#1, #task#2, #task#4 (3 steps, one cluster).

Cause 2 of 2: the "same label under another TLD" suggestions come back in the registrar's rate-table order instead of the order the service offers TLDs in.
AWS: the model says nothing about the order or the membership of SuggestionsList (Smithy: SuggestionsList target DomainSuggestionsList, each DomainSuggestion carrying DomainName/Availability/Price), so the requirement is recorded behavior only: for a seed label the real service answers HTTP 200 and offers that label under the common registrable TLDs first — the recording's first three suggestions are the label under .com, then .net, then .org.
Emulator: HTTP 200 with the right count and the right label, but the TLDs are the first three entries of its internal per-TLD rate table in alphabetical key order — the label under .ai, then .app, then .biz. The divergence is positional on every element of the list ($.SuggestionsList[0..2] all differ), so a caller asking for the usual small count never sees .com/.net/.org at all. Source: crate::state::suggestion_candidates builds the "other TLDs" list by iterating STANDARD_REGISTRATION_RATES in declaration order (that table is sorted by TLD name), and the handler truncates to SuggestionCount, so the alphabetically-first TLDs crowd out the ones AWS offers.
Why: a client that takes the first suggestion, or shows the top few, is handed names under TLDs a user did not ask about and that cost several times the .com rate, while the near-identical .com/.net/.org alternatives AWS returns are cut off by the count. Any test or UI that asserts on the leading suggestions diverges on every element.
Fix: the suggestion derivation in src/state/suggestions.rs needs its own ordered catalogue of the TLDs to offer, most-common first, decoupled from the price table's key order; the price table stays the source of what a name COSTS. The handler's truncation to SuggestionCount is correct once the order is.
Traces: route53domains-suggestions-for-unregistrable-tld-seed#task#3 (1 step).

Neither cause is covered by an open issue for this service. The two open route53domains issues are about other faults and do not carry these replay keys: "route53domains InvalidInput message for a single-label domain name ..." (GetDomainSuggestions message wording for a one-label seed) and "route53domains ListPrices returns wrong RestorationPrice and emits unmodeled-for-TLD PriceType".

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout; 4 divergence clusters, 2 of them net-new and both on GetDomainSuggestions in fixture route53domains-suggestions-for-unregistrable-tld-seed.

Cause 1 — representative route53domains-suggestions-for-unregistrable-tld-seed#task#1 (verdict mismatch, divergence_key service=route53domains|GetDomainSuggestions|status=200->400|keys=$.SuggestionsList,status):
  Expected (recorded AWS): HTTP 200, $.SuggestionsList = [{DomainName: <seed label>.com}, {DomainName: <seed label>.net}, {DomainName: <seed label>.org}] (the recording's seed label is a per-run probe value, not part of the requirement).
  Actual (emulator): HTTP 200 -> 400, $.SuggestionsList missing (None).
  Affected steps: route53domains-suggestions-for-unregistrable-tld-seed#task#1, #task#2, #task#4.
  Refusal path in the checkout: services/service-com-amazonaws-route53domains-route53domains-v20140515/src/state.rs lines 395-397 (resolve_domain_name returns DomainNameError::UnsupportedTld when the labels after the first are not a known public suffix, via is_public_suffix at lines 348-351), reached from src/ops/get_domain_suggestions.rs lines 197-198 and turned into UnsupportedTLD at lines 150-155, message from state.rs lines 465-477 ("The <tld> TLD is not supported": fixed "The " and " TLD is not supported", with the TLD the one variable part, taken from the request).

Cause 2 — representative route53domains-suggestions-for-unregistrable-tld-seed#task#3 (verdict mismatch, divergence_key service=route53domains|GetDomainSuggestions|status=200->200|keys=$.SuggestionsList[0],$.SuggestionsList[1],$.SuggestionsList[2]):
  Expected (recorded AWS): HTTP 200 with the seed label under .com, .net, .org in that order.
  Actual (emulator): HTTP 200 with the seed label under .ai, .app, .biz in that order — every one of the three positions diverges.
  Affected steps: route53domains-suggestions-for-unregistrable-tld-seed#task#3.
  Source in the checkout: src/state/suggestions.rs lines 148-156 (others = STANDARD_REGISTRATION_RATES in declaration order, minus the requested TLD) over the alphabetically sorted table at src/state/registry.rs lines 67-94 ("ai", "app", "biz", "ca", ... ), truncated to SuggestionCount at src/ops/get_domain_suggestions.rs lines 221-251.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus); fixture route53domains-suggestions-for-unregistrable-tld-seed reports both clusters.
2. Cause 1 directly: call GetDomainSuggestions with a well-formed two-label DomainName whose TLD is not a registrable suffix (e.g. {"DomainName": "someseedlabel.zzz", "SuggestionCount": 3, "OnlyAvailable": true}). The emulator answers HTTP 400 (UnsupportedTLD, "The zzz TLD is not supported") with no SuggestionsList; AWS answers HTTP 200 with the label under the registrable TLDs (.com, .net, .org first).
3. Cause 2 directly: call GetDomainSuggestions with a seed under a registrable TLD other than .com and SuggestionCount 3 (e.g. {"DomainName": "someseedlabel.xyz", "SuggestionCount": 3, "OnlyAvailable": true}). The emulator answers the label under .ai, .app, .biz; AWS answers the label under .com, .net, .org.
4. Re-replay route53domains-suggestions-for-unregistrable-tld-seed (task#1, task#2, task#4 for cause 1; task#3 for cause 2) to confirm BOTH causes are closed before submitting.
