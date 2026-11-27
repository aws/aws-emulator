Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers
Issue-Key: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers|run#20|AssertionError|assert first_refusal >= RATE_LIMIT, (
Issue-Commit: e6b18a1abd0b4a83e5ccd770d063cf52582aa2f0

# apigateway__wafv2: a REGIONAL rate-based rule refuses the source before Limit requests have been served, so a WAF-protected stage rate-limits traffic AWS allows

> _Note: this issue was identified against the code base at commit `e6b18a1abd0b4a83e5ccd770d063cf52582aa2f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
One net-new root cause in the apigateway__wafv2 surface; the replay report marks it neither OWNED nor ALREADY FILED (every other apigateway__wafv2 failure in this run is owned by 20261029T221828Z-apigateway-wafv2-2-net-new-root-causes-describemanagedrulegroup.md, whose replay key does not cover this check).

=== Cause 1 of 1: the rate-based rule's Block action is applied before the configured Limit is exceeded ===
Failing check id: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers

AWS: Smithy (wafv2, com.amazonaws.wafv2, awswaf-20190729) — RateBasedStatement.Limit is required, target RateLimit, @range min 10 max 2000000000, documented as "The limit on requests during the specified evaluation window for a single aggregation instance for the rate-based rule." RateBasedStatement.EvaluationWindowSec is "The amount of time, in seconds, that WAF should include in its request counts, looking back from the current time... Valid settings are 60, 120, 300, and 600" and explicitly states "This setting doesn't determine how often WAF checks the rate, but how far back it looks each time it checks. WAF checks the rate about every 10 seconds." AggregateKeyType=IP means "this is the limit on requests from any single IP address", and the aggregate-key doc states the rule action is applied only "When the count of all requests ... goes over the limit". So two things are structural: (a) the action is applied only once an aggregation instance's count in the look-back window is OVER Limit — a source at or below Limit is NOT rate-limited; and (b) rate evaluation is periodic (about every 10 s) and its result reaches the enforcement point with a lag, so a request burst issued as fast as a client can send is not refused on its first handful of requests. Recorded behavior only for the edge itself (execute-api is not Smithy-modelled): a Block action terminates the request at the API Gateway stage with HTTP 403 and a WAF-generated body, an allowed request returns the origin's 200 body, and once the look-back window drains below Limit the same source is served 200 again.

Emulator: with a REGIONAL WebACL (DefaultAction=Allow) carrying a single Priority-0 rule Statement=RateBasedStatement{Limit=10, AggregateKeyType=IP, EvaluationWindowSec=60}, Action=Block, associated to a deployed REST API stage, the stage served only 7 requests of the burst and then refused every following one: the observed status sequence was [200, 200, 200, 200, 200, 200, 200, 403, 403, 403, 403, 403] — the first 403 is request #8 of a burst against a Limit of 10. The replay detail is `AssertionError: request #8 was refused, which is at or below the configured Limit of 10, so the refusal cannot be rate-based: [200, 200, 200, 200, 200, 200, 200, 403, 403, 403, 403, 403]`. The counts/statuses quoted here are one run's values, not the requirement; the requirement is that no refusal occurs until the source's count in the look-back window exceeds Limit. Hypothesis (neither the model nor the recording states the internal rule): the emulator counts the aggregation instance cumulatively from the moment of AssociateWebACL rather than over a trailing EvaluationWindowSec window, and/or it refuses at count >= Limit instead of count > Limit, and/or it evaluates synchronously per request with no ~10 s aggregation interval — any of these fires the Block action earlier than AWS. Because the check dies at this assertion, the downstream legs are unobserved and their fidelity is unknown: GetRateBasedStatementManagedKeys naming the throttled source in ManagedKeysIPV4 (IPAddressVersion=IPV4, Addresses as IPv4 networks), GetSampledRequests attributing a BLOCK to the same ClientIP, and the source recovering to 200 once the 60 s window drains.

Why: a client that sizes a rate-based rule to its real traffic (Limit is the per-IP request allowance for the window) gets requests refused with 403 well below that allowance on the emulator. Any application or test that exercises its documented allowance — a burst of up to Limit requests from one address, which AWS serves — sees spurious edge refusals, so capacity tests, load smoke tests and anything verifying "under the limit is allowed" fail against the emulator while passing on AWS. Conversely a client cannot trust the emulator to tell it where its real limit sits, since the refusal point is not the configured Limit.

Fix: the wafv2 rate-based-statement evaluation in the web-ACL inspection hook on the execute-api/API Gateway data plane and the per-aggregation-instance counter state it reads — the counter must be a trailing EvaluationWindowSec window per aggregation instance (IP for AggregateKeyType=IP), the Block action must apply only once that windowed count goes over Limit, and the evaluated rate must be refreshed periodically rather than decided exactly per request, so a source at or below Limit is always served by the origin and drains back to 200 as the window ages out.

Traces: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers (the only apigateway__wafv2 failure in this run not marked OWNED).

## Evidence
[mismatch] apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers
Error: File "check_apigateway__wafv2_wafv2_rate_based_rule_refuses_the_caller_lists_its_key_then_recovers.py", line 325, in run
  assert first_refusal >= RATE_LIMIT, (
  AssertionError: request #8 was refused, which is at or below the configured Limit of 10, so the refusal cannot be rate-based: [200, 200, 200, 200, 200, 200, 200, 403, 403, 403, 403, 403]

Configuration under test: wafv2 CreateWebACL Scope=REGIONAL, DefaultAction={Allow:{}}, Rules=[{Name:'rater', Priority:0, Action:{Block:{}}, Statement:{RateBasedStatement:{Limit:10, AggregateKeyType:'IP', EvaluationWindowSec:60}}}], associated via AssociateWebACL to arn:aws:apigateway:<region>::/restapis/<api-id>/stages/<stage> of a deployed REGIONAL REST API whose '/flood' resource is a MOCK integration echoing an origin marker.

Expected (AWS): the stage serves the origin's 200 for every request while the source's request count in the trailing EvaluationWindowSec window is at or below Limit; the Block action (HTTP 403 at the stage, WAF-generated body) begins only after that windowed count goes OVER Limit, and because WAF re-evaluates the rate about every 10 s the first refusal lands no earlier than the Limit-th request of a fast burst. Smithy: RateBasedStatement.Limit (required, @range min 10 max 2000000000) 'The limit on requests during the specified evaluation window for a single aggregation instance'; EvaluationWindowSec 'how far back it looks each time it checks... WAF checks the rate about every 10 seconds'; AggregateKeyType=IP 'the limit on requests from any single IP address'; the action applies 'When the count of all requests ... goes over the limit'.
Actual (emulator): the 8th request of the burst is already refused with 403 against Limit=10 — statuses [200 x7, 403 x5] — so the enforcement point is below the configured allowance. The baseline leg immediately before the burst had settled on 3 consecutive 200s, so the rule was attached and idle-allowing before the load.

Not observed because the check stops here (fidelity unknown, not claimed as defects): GetRateBasedStatementManagedKeys reporting ManagedKeysIPV4 {IPAddressVersion:'IPV4', Addresses:[<IPv4 networks>]} for the throttled source; GetSampledRequests returning a SampledRequest with Action='BLOCK' whose Request.ClientIP matches one of those managed keys; the source being served 200 with the origin marker again once the 60 s window drains below Limit.

## Steps to Reproduce
1. apigateway CreateRestApi with endpointConfiguration={'types':['REGIONAL']}; under the root resource create a child resource 'flood' with PutMethod(httpMethod='GET', authorizationType='NONE'), PutIntegration(type='MOCK', requestTemplates={'application/json':'{"statusCode": 200}'}), PutMethodResponse(statusCode='200') and PutIntegrationResponse(statusCode='200', selectionPattern='', responseTemplates={'application/json':'{"origin":"<marker>"}'}).
2. CreateDeployment, then CreateStage for that deployment; confirm GET <invoke-url>/<stage>/flood returns 200 with the marker.
3. wafv2 CreateWebACL Scope='REGIONAL', DefaultAction={'Allow':{}}, Rules=[{'Name':'rater','Priority':0,'Action':{'Block':{}},'Statement':{'RateBasedStatement':{'Limit':10,'AggregateKeyType':'IP','EvaluationWindowSec':60}},'VisibilityConfig':{...}}].
4. wafv2 AssociateWebACL(WebACLArn=<acl arn>, ResourceArn='arn:aws:apigateway:<region>::/restapis/<api-id>/stages/<stage>'), retrying WAFUnavailableEntityException.
5. Confirm the stage still answers 200 on a few idle requests (the rule is attached and not yet limiting).
6. From a single source, issue a sequential burst of GETs to <invoke-url>/<stage>/flood and record each status. Find the index of the first 403.
7. AWS serves at least the first Limit (10) requests of the burst with the origin's 200 before any 403. Observe here that the first 403 arrives at request #8 — at or below the configured Limit — i.e. [200 x7, 403 x5].
8. Or replay check apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers.
