Issue-Source: trace-sweep-divergence
Issue-Trace: autocomplete-validation-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autocomplete-validation-rejected suggest-validation-rejected
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places|Autocomplete|2xx->2xx|$.ResultItems[*].Highlights.Title
Issue-Key: geo-places|Suggest|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# Autocomplete and Suggest success responses drop Highlights and mis-derive PricingBucket

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Autocomplete

## Description
Two causes in the 200-response path of the geo-places text operations: a result item omits the Highlights the operation exists to report, and the required pricing-bucket header is derived as a single global tier rather than per operation. BOTH causes must be closed.

--- Cause 1 of 2: Autocomplete result items carry no Highlights for a partially typed query term ---
AWS: Smithy: AutocompleteResultItem$Highlights targets AutocompleteHighlights{Title: HighlightList, Address: AutocompleteAddressHighlights}, documented as "Indicates the starting and ending index of the place in the text query that match the found title", and the shape as a whole "describes how the parts of the response element matched the input query by returning the sections of the response which matched to input query terms". Autocomplete "completes partial queries with valid address completion" (Smithy: Autocomplete doc), so the PARTIALLY TYPED trailing term of the query is a matched section: AWS returns Highlights.Title with one Highlight per matched span, each carrying StartIndex and EndIndex (the span's bounds) and Value (the matched text of that span). The spans and their text are values derived from the request's query and the matched record; only the presence and shape of Highlights.Title is the requirement.
Emulator: ResultItems[0].Highlights is absent entirely for a query whose trailing term is a prefix of a term in the matched result's Title — the recorded response carried a one-entry Highlights.Title for that prefix span.
Why: a client that renders the matched substring in bold (the whole point of an autocomplete widget) gets nothing to render for exactly the partial keystrokes the user is typing, and a client that dereferences Highlights.Title sees a null member the recording shows populated.
Fix: the highlight matcher state::highlight_matches behind state::modeled_highlights (services/service-com-amazonaws-geoplaces-placesservice/src/state.rs ~line 804), used by ops/autocomplete.rs::highlights (~line 436) — it must also locate a prefix match for the query's final, partially typed token, not whole tokens only.
Traces: autocomplete-validation-rejected#task#1.

--- Cause 2 of 2: Suggest answers the wrong x-amz-geo-pricing-bucket tier ---
AWS: Smithy: SuggestResponse$PricingBucket is @required and bound to httpHeader x-amz-geo-pricing-bucket ("the pricing bucket for which the query is charged at"). The model enumerates no values, so recorded behavior only: the recorded Suggest response carries a different bucket token than the recorded Autocomplete/SearchText responses do, i.e. the bucket a Places query is charged in is not one global value shared by every operation. Hypothesis: Suggest is charged in its own bucket while the catalog-search operations share the default tier.
Emulator: returns the same default tier for Suggest as for every other Places operation, because state::pricing_bucket(operation, intended_use) falls back to PRICING_BUCKET_CORE for any operation missing from the seeded pricing_buckets map and only varies on IntendedUse == "Storage".
Why: a client or cost-attribution consumer reading x-amz-geo-pricing-bucket attributes Suggest traffic to the wrong pricing bucket, and the header value never differs between operations as it does on AWS.
Fix: the pricing-bucket derivation in src/state.rs (state::pricing_bucket and the seeded pricing_buckets map) — the bucket must be resolved per operation, with Suggest's own entry seeded, rather than defaulting every operation to the shared tier.
Traces: suggest-validation-rejected#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 25%, 15/20 steps mismatched).

Cause 1 — autocomplete-validation-rejected#task#1, Autocomplete, status 200 vs 200: $.ResultItems[0].Highlights.Title missing — expected a one-entry HighlightList for the span matched by the query's partial trailing term (recorded run: [{'StartIndex': 26, 'EndIndex': 32, 'Value': 'Seattl'}] — those indices and that text are the recording's values, the requirement is that the member is present and reports the matched span), got None.

Cause 2 — suggest-validation-rejected#task#1, Suggest, status 200 vs 200: header x-amz-geo-pricing-bucket expected 'Label', got 'Core'. The same header matched on the Autocomplete 200 in autocomplete-validation-rejected#task#1, so the divergence is Suggest-specific rather than a global tier rename.

Code evidence: state::pricing_bucket (src/state.rs ~line 2961) keys the seeded pricing_buckets map by operation name but falls back to PRICING_BUCKET_CORE ("Core", ~line 78) for any unseeded operation; state::modeled_highlights (~line 804) returns None whenever highlight_matches finds no span, and ops/autocomplete.rs::highlights (~line 436) therefore omits the member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: fixture autocomplete-validation-rejected, step task#1 — POST /v2/autocomplete with a QueryText whose last term is a prefix of a term in the matched place's Title; assert ResultItems[0].Highlights.Title is present and reports the matched span.
3. Cause 2: fixture suggest-validation-rejected, step task#1 — POST /v2/suggest with a valid request; assert the x-amz-geo-pricing-bucket response header carries Suggest's recorded bucket ('Label') rather than the shared default 'Core'.
