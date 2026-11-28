Issue-Source: trace-sweep-divergence
Issue-Trace: route53domains-account-reads-and-price-list
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53domains-account-reads-and-price-list
Issue-Shape-Id: com.amazonaws.route53domains#Route53Domains_v20140515
Issue-Key: route53domains|ListPrices|2xx->2xx|$.Prices[Name="com"].ChangeOwnershipPrice
Issue-Key: route53domains|ListPrices|2xx->2xx|$.Prices[Name="com"].RegistrationPrice
Issue-Key: route53domains|ListPrices|2xx->2xx|$.Prices[Name="com"].RenewalPrice
Issue-Key: route53domains|ListPrices|2xx->2xx|$.Prices[Name="com"].RestorationPrice
Issue-Key: route53domains|ListPrices|2xx->2xx|$.Prices[Name="com"].TransferPrice
Issue-Commit: e52cf748fe15a6a8583f9d9789193e5a6472c9ef

# route53domains ListPrices returns wrong RestorationPrice and emits unmodeled-for-TLD PriceType

> _Note: this issue was identified against the code base at commit `e52cf748fe15a6a8583f9d9789193e5a6472c9ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPrices

## Description
ListPrices for a TLD returns a Prices list whose PriceWithCurrency members diverge from AWS in two independent ways. Both causes are exercised by the same recorded step (fixture route53domains-account-reads-and-price-list, step task#5, a ListPrices call for Tld="com"), and BOTH must be closed before this issue is resolved.

Cause 1 of 2: RestorationPrice for the "com" TLD carries the wrong amount.
AWS: Smithy: ListPrices -> ListPricesResponse.Prices : DomainPriceList of DomainPrice; DomainPrice.RestorationPrice : PriceWithCurrency {Price: double @required @range(min 0.0), Currency: string @required @length(3,3)}. The model does not fix the amount, so the amount itself is recorded behavior only: the recording shows that for the "com" TLD AWS reports a restoration price distinctly higher than the registration/renewal/transfer price for the same TLD (recorded 78.0 USD against 16.0 USD registration), i.e. restoration is priced as its own line item and is not derived from the registration price. Status 200, no error.
Emulator: returns 200 with $.Prices[Name="com"].RestorationPrice = {Price: 40.0, Currency: "USD", ...} where the recording has {Price: 78.0, Currency: "USD"} — the per-TLD restoration amount served by the emulator's price table does not match the amount AWS reports for that TLD.
Why: a client that quotes or budgets domain restoration from ListPrices (a console price page, a cost estimator, a registrar-integration test asserting the restoration fee) reads a materially wrong figure for the TLD and so under-quotes restoration while every other price on the same record is correct — a silently wrong number, not a visible failure.
Fix: the per-TLD price table backing the ListPrices handler in the route53domains service (the seeded restoration amount per TLD), not the serializer: the surrounding structure and currency are already correct.
Traces: route53domains-account-reads-and-price-list#task#5.

Cause 2 of 2: every PriceWithCurrency is serialized with a PriceType member that AWS omits for a TLD price list.
AWS: Smithy: PriceWithCurrency.PriceType : PriceType enum, NOT @required (only Price and Currency are @required), documented as "Indicates whether the domain has standard or premium pricing ... REGISTRY_PREMIUM means the domain has registry premium pricing" — i.e. it qualifies an individual DOMAIN's pricing. Recorded behavior: for a ListPrices call scoped to a TLD, AWS leaves PriceType absent on all five price members (RegistrationPrice, TransferPrice, RenewalPrice, ChangeOwnershipPrice, RestorationPrice), returning only {Price, Currency}. Status 200, no error.
Emulator: returns 200 but adds "PriceType": "STANDARD" to all five PriceWithCurrency structures, e.g. RegistrationPrice {Price: 16.0, Currency: "USD", PriceType: "STANDARD"} where AWS returns {Price: 16.0, Currency: "USD"}. The member is populated unconditionally rather than left unset.
Why: an optional member is the signal clients use to distinguish "AWS did not say" from "AWS said STANDARD". A client that branches on PriceType presence to decide whether a premium-pricing check applies, or that asserts response equality against a recorded TLD price list, sees a field AWS never sends; conversely a test written against the emulator will wrongly expect PriceType to be present everywhere and break against real AWS.
Fix: the ListPrices response serializer / price-record construction in the route53domains service — leave PriceWithCurrency.PriceType unset for TLD-scoped price records instead of defaulting it to STANDARD.
Traces: route53domains-account-reads-and-price-list#task#5.

Not filed here: the GetDomainSuggestions / domain-input-validation InvalidInput message divergence in the same corpus is already owned by open issue 20261120T205219Z-route53domains-invalidinput-message-for-a-single-label-domain.md and is deliberately excluded from this bundle.

## Evidence
Replay of fixture route53domains-account-reads-and-price-list, step task#5 (ListPrices, Tld="com"). Expected status 200, actual status 200; verdict mismatch on five response keys.

Cause 1 — wrong value:
  $.Prices[Name="com"].RestorationPrice
    expected {"Currency": "USD", "Price": 78.0}
    actual   {"Price": 40.0, "Currency": "USD", "PriceType": "STANDARD"}

Cause 2 — unmodeled-for-TLD member present on all five price members:
  $.Prices[Name="com"].RegistrationPrice   expected {"Currency": "USD", "Price": 16.0}   actual {"Price": 16.0, "Currency": "USD", "PriceType": "STANDARD"}
  $.Prices[Name="com"].TransferPrice       expected {"Currency": "USD", "Price": 16.0}   actual {"Price": 16.0, "Currency": "USD", "PriceType": "STANDARD"}
  $.Prices[Name="com"].RenewalPrice        expected {"Currency": "USD", "Price": 16.0}   actual {"Price": 16.0, "Currency": "USD", "PriceType": "STANDARD"}
  $.Prices[Name="com"].ChangeOwnershipPrice expected {"Currency": "USD", "Price": 0.0}   actual {"Price": 0.0, "Currency": "USD", "PriceType": "STANDARD"}
  $.Prices[Name="com"].RestorationPrice    (also carries the spurious PriceType, see Cause 1)

Note the Price/Currency amounts agree on four of the five members, so the only difference there is the extra PriceType member; RestorationPrice differs in both the member set and the amount. Member ordering inside the JSON object is not part of the finding.

Model basis: PriceWithCurrency declares Price (@required, double, @range min 0.0), Currency (@required, @length 3..3) and PriceType (optional enum). ListPrices returns ListPricesResponse{Prices: DomainPriceList, NextPageMarker} and models errors InvalidInput (400) and UnsupportedTLD (400); no error is involved in this step.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS client at its data listener with region us-east-1 (route53domains is bootstrapped and answering).
2. Replay the corpus step: call ListPrices with Tld="com" (no Marker, no MaxItems) — e.g. `aws route53domains list-prices --tld com --endpoint-url <emulator>`.
3. Inspect the returned Prices entry whose Name is "com".
4. Observe Cause 2: every one of RegistrationPrice, TransferPrice, RenewalPrice, ChangeOwnershipPrice and RestorationPrice includes "PriceType": "STANDARD"; AWS's recorded TLD-scoped response returns only {Price, Currency} on each.
5. Observe Cause 1: RestorationPrice.Price is 40.0, while the recording for the same TLD reports 78.0 USD (a restoration fee priced well above the 16.0 USD registration/renewal/transfer prices on the same record).
6. Equivalently: run replay_wire_corpus() and look at the divergence for fixture route53domains-account-reads-and-price-list, step task#5 (service route53domains, operation ListPrices, verdict mismatch, 200 -> 200).
