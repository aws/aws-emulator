Issue-Source: trace-sweep-divergence
Issue-Trace: sustainability-carbon-dimension-values-required-and-range-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sustainability-carbon-dimension-values-required-and-range-rejected sustainability-carbon-dimension-values-unissued-next-token-rejected sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected sustainability-carbon-emissions-next-token-at-max-length-unissued-rejected sustainability-carbon-emissions-next-token-over-max-length-rejected sustainability-carbon-emissions-unissued-next-token-rejected sustainability-water-dimension-values-unissued-next-token-rejected sustainability-water-dimension-values-window-and-request-shape-rejected
Issue-Shape-Id: com.amazonaws.sustainability#AwsSustainabilityApiService
Issue-Key: sustainability|GetEstimatedCarbonEmissionsDimensionValues|2xx->4xx|$.Results
Issue-Key: sustainability|GetEstimatedCarbonEmissionsDimensionValues|2xx->4xx|status
Issue-Key: sustainability|GetEstimatedCarbonEmissionsDimensionValues|4xx->4xx|$.message
Issue-Key: sustainability|GetEstimatedCarbonEmissionsDimensionValues|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sustainability|GetEstimatedCarbonEmissionsDimensionValues|4xx->4xx|status
Issue-Key: sustainability|GetEstimatedCarbonEmissions|2xx->4xx|$.Results
Issue-Key: sustainability|GetEstimatedCarbonEmissions|2xx->4xx|status
Issue-Key: sustainability|GetEstimatedCarbonEmissions|4xx->4xx|$.message
Issue-Key: sustainability|GetEstimatedCarbonEmissions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sustainability|GetEstimatedCarbonEmissions|4xx->4xx|status
Issue-Key: sustainability|GetEstimatedWaterAllocationDimensionValues|4xx->4xx|$.message
Issue-Key: sustainability|GetEstimatedWaterAllocationDimensionValues|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sustainability|GetEstimatedWaterAllocationDimensionValues|4xx->4xx|status
Issue-Key: sustainability|GetEstimatedWaterAllocationDimensionValues|2xx->4xx|$.Results
Issue-Key: sustainability|GetEstimatedWaterAllocationDimensionValues|2xx->4xx|status
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# sustainability carbon and dimension-values wire surface refused 403 AccessDeniedException before dispatch, masking 400 ValidationException and 200 Results

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEstimatedCarbonEmissionsDimensionValues

## Description
6 causes, all in the sustainability service and all produced by the same pre-dispatch refusal: every request to this service is answered HTTP 403 with x-amzn-errortype AccessDeniedException and the fixed sentence "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)." before any operation handler runs, so neither a modelled 200 answer nor a modelled 400 ValidationException is reachable on any of the three operations below. Smithy: AwsSustainabilityApiService, protocol restJson1, authSchemes [sigv4, sigv4a]; common errors AccessDeniedException @httpError(403), ValidationException @httpError(400) with required Message (jsonName "message"). AccessDeniedException is reserved for an authorization failure of the caller, never for a well-signed call by an authorized principal.

RELATION TO OPEN ISSUES: open issue 20261029T184222Z-sustainability-getestimatedwaterallocation-refused-403-accessdeniedexception-before-dispatch-so.md carries the wire replay keys for GetEstimatedWaterAllocation ONLY (those two clusters are reported OWNED and are deliberately not re-filed here); open issue 20261029T200631Z-sustainability-every-operation-refused-403-accessdeniedexception-before-dispatch.md is a BEHAVIORAL issue and carries no wire fixture/step keys. Neither covers the six wire causes below, so they are filed here with their own replay bindings.

Cause 1 of 6: GetEstimatedCarbonEmissionsDimensionValues answers 403 AccessDeniedException instead of 200 with its Results list (fixture sustainability-carbon-dimension-values-required-and-range-rejected, step task#1).
AWS: GetEstimatedCarbonEmissionsDimensionValues is @readonly, POST /v1/estimated-carbon-emissions-dimension-values, and answers HTTP 200 with GetEstimatedCarbonEmissionsDimensionValuesResponse whose Results (DimensionEntryList) is serialized as a present-but-empty list when the requested TimePeriod/Dimensions select no rows; NextToken is optional (Smithy: http code 200, output Results -> DimensionEntryList, NextToken @length 1..2000). The recorded response is HTTP 200 with $.Results = [].
Emulator: HTTP 403, x-amzn-errortype AccessDeniedException, body message "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)." and no Results member at all.
Why: a client cannot distinguish "no dimension values in the window" from an authorization failure; it will re-sign or re-credential rather than read an empty page, and the whole dimension-discovery surface is unusable.
Fix: the service's pre-dispatch request-authentication gate (services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/service.rs can_handle / override_refusal and the framework authentication it feeds) must authenticate the SigV4 (AWS4-HMAC-SHA256) and SigV4A (AWS4-ECDSA-P256-SHA256) emulator credentials this model's auth schemes imply so the request reaches the handler; the handler must then serialize Results as an empty list rather than omitting it.
Traces: sustainability-carbon-dimension-values-required-and-range-rejected#task#1.

Cause 2 of 6: an out-of-range MaxResults on GetEstimatedCarbonEmissionsDimensionValues is answered 403 AccessDeniedException instead of the modelled 400 ValidationException (fixture sustainability-carbon-dimension-values-required-and-range-rejected, step task#2).
AWS: MaxResults targets MaxResults, an integer with @range(min 1, max 5000) and @default 1000, so a value below the minimum (and likewise a missing required member, or an unissued NextToken) is refused with ValidationException (Smithy: GetEstimatedCarbonEmissionsDimensionValues errors include ValidationException @error("client") @httpError(400), required Message jsonName "message"; input TimePeriod and Dimensions required, NextToken @length 1..2000). The recorded response is HTTP 400 with AWS's constraint-violation envelope: fixed text "<n> validation error(s) detected: Value at '", "' failed to satisfy constraint: Member must have value greater than or equal to "; the variable parts are the violation count, the input member path (maxResults) and the limit taken from the model (1). x-amzn-errortype must name a shape this model declares (ValidationException).
Emulator: HTTP 403 with x-amzn-errortype AccessDeniedException and the fixed signing sentence above; input validation is never reached.
Why: a client that branches on ValidationException to repair its own request instead sees an auth failure and retries credentials; the member path and limit the error is supposed to carry are lost. This cluster also covers the required-member, window and unissued-next-token refusals of the same operation (affected: sustainability-carbon-dimension-values-required-and-range-rejected#task#3..#task#5, sustainability-carbon-dimension-values-unissued-next-token-rejected#task#1).
Fix: same gate as Cause 1, so the request reaches the operation's constrained-input validation; the ValidationException rendering itself is the framework's.
Traces: sustainability-carbon-dimension-values-required-and-range-rejected#task#2 (plus #task#3, #task#4, #task#5 and sustainability-carbon-dimension-values-unissued-next-token-rejected#task#1).

Cause 3 of 6: GetEstimatedCarbonEmissions answers 403 AccessDeniedException instead of 200 with its REQUIRED Results list (fixture sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected, step task#1).
AWS: GetEstimatedCarbonEmissions is @readonly, POST /v1/estimated-carbon-emissions, and answers HTTP 200 with GetEstimatedCarbonEmissionsResponse whose Results (EstimatedCarbonEmissionsList) is REQUIRED and present-but-empty when nothing is selected; NextToken is optional (Smithy: http code 200, output Results required). The recorded response is HTTP 200 with $.Results = [].
Emulator: HTTP 403 AccessDeniedException with no body members, so $.Results is absent where a required empty list belongs.
Why: Results is a required output member, so a generated SDK client cannot deserialize the answer at all; every read of the carbon-emissions surface fails and an empty result set is indistinguishable from a denial.
Fix: same pre-dispatch authentication gate as Cause 1; once dispatched, the handler must emit Results as [] rather than omitting it.
Traces: sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected#task#1.

Cause 4 of 6: an out-of-range GranularityConfiguration.FiscalYearStartMonth on GetEstimatedCarbonEmissions is answered 403 AccessDeniedException instead of the modelled 400 ValidationException (fixture sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected, step task#2).
AWS: GranularityConfiguration.FiscalYearStartMonth targets Month, an integer with @range(min 1, max 12) and @default 1, so a value above 12 is refused with ValidationException (Smithy: GranularityConfiguration.FiscalYearStartMonth @range 1..12; GetEstimatedCarbonEmissions errors include ValidationException @httpError(400) with required Message jsonName "message"; MaxResults @range 1..5000; NextToken @length 1..2000). The recorded response is HTTP 400 with the constraint envelope: fixed text "<n> validation error(s) detected: Value at '", "' failed to satisfy constraint: Member must have value less than or equal to "; the variable parts are the violation count, the nested input member path (granularityConfiguration.fiscalYearStartMonth) and the model's limit (12).
Emulator: HTTP 403 with x-amzn-errortype AccessDeniedException and the fixed signing sentence.
Why: the nested member path and limit that tell a client exactly which field to fix are replaced by an auth error, so the client re-credentials instead of correcting the fiscal-year configuration. This cluster also covers the remaining request-shape refusals and the NextToken length/unissued refusals of this operation (affected: sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected#task#3..#task#5, sustainability-carbon-emissions-next-token-at-max-length-unissued-rejected#task#1, sustainability-carbon-emissions-next-token-over-max-length-rejected#task#1, sustainability-carbon-emissions-unissued-next-token-rejected#task#1).
Fix: same gate as Cause 1, so constrained-input validation of the nested GranularityConfiguration member runs.
Traces: sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected#task#2 (plus #task#3..#task#5, sustainability-carbon-emissions-next-token-at-max-length-unissued-rejected#task#1, sustainability-carbon-emissions-next-token-over-max-length-rejected#task#1, sustainability-carbon-emissions-unissued-next-token-rejected#task#1).

Cause 5 of 6: an unissued NextToken on GetEstimatedWaterAllocationDimensionValues is answered 403 AccessDeniedException instead of the modelled 400 ValidationException (fixture sustainability-water-dimension-values-unissued-next-token-rejected, step task#1).
AWS: NextToken targets NextToken, a string with @length(min 1, max 2000); a syntactically acceptable but never-issued pagination token is refused with ValidationException (Smithy: GetEstimatedWaterAllocationDimensionValues errors include ValidationException @httpError(400) with required Message jsonName "message"; paginated inputToken/outputToken NextToken). The recorded response is HTTP 400 whose message is AWS's invalid-input form for this class: fixed text "Invalid input: invalid next token: must be a valid token string" (no variable parts in this instance; the rejected token itself comes from the request).
Emulator: HTTP 403 with x-amzn-errortype AccessDeniedException and the fixed signing sentence.
Why: pagination cannot be driven or recovered — a client that walks pages and hits a stale token gets an auth error, so it abandons the walk or re-credentials instead of restarting pagination. This cluster also covers the window/request-shape refusals of the same operation (affected: sustainability-water-dimension-values-window-and-request-shape-rejected#task#2..#task#5).
Fix: same gate as Cause 1, then the operation's NextToken validation in its handler/state must reject an unissued token with ValidationException worded as above.
Traces: sustainability-water-dimension-values-unissued-next-token-rejected#task#1 (plus sustainability-water-dimension-values-window-and-request-shape-rejected#task#2..#task#5).

Cause 6 of 6: GetEstimatedWaterAllocationDimensionValues answers 403 AccessDeniedException instead of 200 with its REQUIRED Results list (fixture sustainability-water-dimension-values-window-and-request-shape-rejected, step task#1).
AWS: GetEstimatedWaterAllocationDimensionValues is @readonly, POST /v1/estimated-water-allocation-dimension-values, and answers HTTP 200 with Results (DimensionEntryList) REQUIRED and present-but-empty when the requested TimePeriod/Dimensions select no rows (Smithy: http code 200, output Results required, NextToken optional @length 1..2000). The recorded response is HTTP 200 with $.Results = [].
Emulator: HTTP 403 AccessDeniedException with no body members, so the required $.Results is absent.
Why: a required output member is missing, so a generated client cannot deserialize the response and the water dimension-discovery surface is entirely unreachable.
Fix: same pre-dispatch authentication gate as Cause 1; after dispatch the handler must serialize Results as [] rather than omitting it.
Traces: sustainability-water-dimension-values-window-and-request-shape-rejected#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up: /__admin/health 200, data listener :43551 accepting). 11 fixtures replayed, 29 of 29 steps mismatched, 0 passed, score 0.0%, bootstrap_signal false — the emulator IS serving, it refuses every sustainability request. 8 divergence clusters, all service=sustainability; 2 of them (GetEstimatedWaterAllocation) are reported owned_by 20261029T184222Z-...-getestimatedwaterallocation-...md and are excluded here. The 6 clusters below are the net-new ones.

Cluster 1 — GetEstimatedCarbonEmissionsDimensionValues, fixture sustainability-carbon-dimension-values-required-and-range-rejected step task#1 (1 affected step):
  status: expected 200, got 403
  $.Results: missing (expected [], got None)

Cluster 2 — GetEstimatedCarbonEmissionsDimensionValues, fixture sustainability-carbon-dimension-values-required-and-range-rejected step task#2 (5 affected steps: #task#2, #task#3, #task#4, #task#5, sustainability-carbon-dimension-values-unissued-next-token-rejected#task#1):
  status: expected 400, got 403
  $.message: expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1", got "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)."
  $headers.x-amzn-errortype: expected a shape of this model's error set, got AccessDeniedException

Cluster 3 — GetEstimatedCarbonEmissions, fixture sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected step task#1 (1 affected step):
  status: expected 200, got 403
  $.Results: missing (expected [], got None)

Cluster 4 — GetEstimatedCarbonEmissions, fixture sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected step task#2 (7 affected steps: #task#2..#task#5, sustainability-carbon-emissions-next-token-at-max-length-unissued-rejected#task#1, sustainability-carbon-emissions-next-token-over-max-length-rejected#task#1, sustainability-carbon-emissions-unissued-next-token-rejected#task#1):
  status: expected 400, got 403
  $.message: expected "1 validation error detected: Value at 'granularityConfiguration.fiscalYearStartMonth' failed to satisfy constraint: Member must have value less than or equal to 12", got "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)."
  $headers.x-amzn-errortype: expected a shape of this model's error set, got AccessDeniedException

Cluster 5 — GetEstimatedWaterAllocationDimensionValues, fixture sustainability-water-dimension-values-unissued-next-token-rejected step task#1 (5 affected steps: that step plus sustainability-water-dimension-values-window-and-request-shape-rejected#task#2..#task#5):
  status: expected 400, got 403
  $.message: expected "Invalid input: invalid next token: must be a valid token string", got "You do not have sufficient access to perform this action. Sign the request with the emulator credentials, using SigV4 (AWS4-HMAC-SHA256)."
  $headers.x-amzn-errortype: expected a shape of this model's error set, got AccessDeniedException

Cluster 6 — GetEstimatedWaterAllocationDimensionValues, fixture sustainability-water-dimension-values-window-and-request-shape-rejected step task#1 (1 affected step):
  status: expected 200, got 403
  $.Results: missing (expected [], got None)

Model evidence (smithy_query on services/service-com-amazonaws-sustainability-awssustainabilityapiservice/smithy-model/com-amazonaws-sustainability-awssustainabilityapiservice.json):
  GetEstimatedCarbonEmissions — POST /v1/estimated-carbon-emissions, http code 200, output Results REQUIRED, input MaxResults @range 1..5000 default 1000, NextToken @length 1..2000, GranularityConfiguration.FiscalYearStartMonth -> Month @range 1..12 default 1; errors AccessDeniedException(403), ValidationException(400), ThrottlingException(429), InternalServerException(500).
  GetEstimatedCarbonEmissionsDimensionValues — POST /v1/estimated-carbon-emissions-dimension-values, http code 200, input TimePeriod + Dimensions required, MaxResults @range 1..5000, output Results -> DimensionEntryList; same error set.
  GetEstimatedWaterAllocationDimensionValues — POST /v1/estimated-water-allocation-dimension-values, http code 200, output Results REQUIRED; same error set.
  ValidationException — @error("client") @httpError(400), required Message with jsonName "message".

Code evidence: services/service-com-amazonaws-sustainability-awssustainabilityapiservice/src/service.rs can_handle (claims SigV4-family credential scopes) and override_refusal (Refusal::Unauthenticated -> AccessDeniedException with the fixed signing message) — the request is claimed and then refused ahead of the generated routing table, which is why every step of every fixture diverges identically.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout (sustainability is bootstrapped; /__admin/health answers 200 and the data listener accepts).
2. Cause 1: replay fixture sustainability-carbon-dimension-values-required-and-range-rejected step task#1 — POST /v1/estimated-carbon-emissions-dimension-values, signed as an SDK configured for this model signs it (SigV4 AWS4-HMAC-SHA256 or SigV4A AWS4-ECDSA-P256-SHA256), with a valid body (required TimePeriod and Dimensions). Expect HTTP 200 with $.Results present (empty list when nothing is selected); observe HTTP 403 x-amzn-errortype AccessDeniedException with the fixed signing sentence and no Results member.
3. Cause 2: same fixture step task#2 — same URI with MaxResults below the modelled minimum of 1. Expect 400 ValidationException with "<n> validation error(s) detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"; observe 403 AccessDeniedException. Steps task#3..task#5 and fixture sustainability-carbon-dimension-values-unissued-next-token-rejected step task#1 (missing required members, bad window, unissued NextToken) refuse identically.
4. Cause 3: replay fixture sustainability-carbon-emissions-fiscal-month-and-request-shape-rejected step task#1 — POST /v1/estimated-carbon-emissions with a valid body. Expect 200 with the REQUIRED Results list present; observe 403 AccessDeniedException with Results absent.
5. Cause 4: same fixture step task#2 — same URI with GranularityConfiguration.FiscalYearStartMonth above the modelled maximum of 12. Expect 400 ValidationException with "<n> validation error(s) detected: Value at 'granularityConfiguration.fiscalYearStartMonth' failed to satisfy constraint: Member must have value less than or equal to 12"; observe 403 AccessDeniedException. Steps task#3..task#5 and fixtures sustainability-carbon-emissions-next-token-at-max-length-unissued-rejected#task#1, sustainability-carbon-emissions-next-token-over-max-length-rejected#task#1 and sustainability-carbon-emissions-unissued-next-token-rejected#task#1 refuse identically.
6. Cause 5: replay fixture sustainability-water-dimension-values-unissued-next-token-rejected step task#1 — POST /v1/estimated-water-allocation-dimension-values with a NextToken within @length 1..2000 that this emulator never issued. Expect 400 ValidationException with message "Invalid input: invalid next token: must be a valid token string"; observe 403 AccessDeniedException. Fixture sustainability-water-dimension-values-window-and-request-shape-rejected steps task#2..task#5 refuse identically.
7. Cause 6: replay fixture sustainability-water-dimension-values-window-and-request-shape-rejected step task#1 — POST the same URI with a valid body. Expect 200 with the REQUIRED Results list present (empty when nothing is selected); observe 403 AccessDeniedException with Results absent.
8. All six steps are fixed together by making the service's pre-dispatch authentication gate accept the emulator credentials for this model's auth schemes; re-replay the whole trace set and confirm the six clusters above now match.
