Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/10a32fb6-41ea-4216-e3a1-f4f9a23cf9f3"].createdAt
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/10a32fb6-41ea-4216-e3a1-f4f9a23cf9f3"].eTag
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/10a32fb6-41ea-4216-e3a1-f4f9a23cf9f3"].updatedAt
Issue-Commit: 07e1e88d592d3fe676e21622404cc67d61ac5470

# ListSubscriptions summary eTag is a content digest and its timestamps are JSON epoch numbers

> _Note: this issue was identified against the code base at commit `07e1e88d592d3fe676e21622404cc67d61ac5470`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptions

## Description
ListSubscriptions answers the modeled 200 and every other member of the body matches, so nothing warns the caller, but the SubscriptionSummary the emulator renders for a subscription that has already taken a mutation carries three @required members in a wire form AWS never produces: the concurrency token (eTag) and the two date-time members (createdAt, updatedAt). The replay reports this cluster (index 4, fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9) with an EMPTY owned_by and an empty stub_path, i.e. no open issue carries its replay key, so it is filed here.

Five open issues describe this same symptom family but do NOT own this step, because their Issue-Key lines are pinned to a per-run minted subscription ARN and so never join this run's cluster key (this run's key names arn=...10a32fb6-41ea-4216-e3a1-f4f9a23cf9f3): 20261111T140419Z-pricing-plan-manager-listsubscriptions-renders-subscription-summaries-with-digest-etags-and.md, 20261111T215818Z-pricing-plan-manager-subscriptionsummary-concurrency-token-and-date-time-members-are.md, 20261112T225619Z-pricing-plan-manager-listsubscriptions-etag-digest-and-epoch-number-timestamps-break.md, 20261113T023620Z-pricing-plan-manager-subscriptionsummary-serializer-emits-digest-etag-and-numeric-timestamps.md and 20261113T131026Z-pricing-plan-manager-subscription-state-publishes-hashed-revision-tokens-and-raw.md. A sixth, 20261111T105841Z-pricing-plan-manager-subscription-representation-diverges-epoch-second-timestamps-mis-minted.md, owns the Get/Create representation clusters and the sibling ListSubscriptions cluster of fixture pricing-plan-manager-free-subscription-hosted-zone-association#task#11 — not this one. The repeated re-surfacing is itself the signal: the defect lives in the SHARED SubscriptionSummary serializer and in the state record's eTag derivation, so the fix must change those two components and must NOT be validated against one recorded ARN, one recorded digest or one recorded instant — keying anything on a per-run minted ARN is what has kept this cluster unowned.

This issue carries 2 causes; BOTH must be closed before submitting.

Cause 1 of 2: SubscriptionSummary.eTag is published as a 128-bit hex content digest instead of the small decimal revision AWS publishes. Fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json), shape SubscriptionSummary: member eTag -> String, required=true, doc "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription." The model fixes no literal syntax for the token, so its format is recorded behavior only: the recording for a subscription that has taken one mutation after creation carries eTag "2" — a short decimal integer. Hypothesis: AWS issues a monotonic per-subscription revision that starts at 1 and increments on each mutation, which is consistent with the single recorded value. What is required is the FORM (a small decimal integer, not a content digest); the particular integer is a function of how many mutations that subscription has seen and is not itself a requirement.
Emulator: the summary for the same subscription carries a 32-character lowercase hex digest — a content hash over the stored record — where the recording holds a decimal revision. Replay detail: $.subscriptionSummaries[arn="..."].eTag value_mismatch, expected '2', got 'c646a46ae40e5288deb1ae1e5b96650d' (that digest is a per-run value and only fills the template).
Why: the eTag is the exact token a client echoes in If-Match on CancelSubscription and the other mutation paths. A caller that persists an AWS-shaped token, compares the token it listed against the one it stored, or asserts on the token's form diverges on every list, and no optimistic-concurrency test written against AWS can be replayed against the emulator, because the token's shape differs and not merely its value.
Fix: the eTag derivation on the subscription state record plus the summary conversion that surfaces it into the ListSubscriptions output (the service crate's state module and its to-summary conversion) — publish the record's revision counter as a decimal string, and keep the If-Match comparison on the mutation path reading that same value.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

Cause 2 of 2: SubscriptionSummary.createdAt / updatedAt are serialized as epoch-second JSON numbers instead of ISO-8601 date-time strings. Fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9.
AWS: Smithy: SubscriptionSummary.createdAt and SubscriptionSummary.updatedAt are required Timestamp members carrying timestampFormat "date-time", documented "The date and time when the subscription was created / was last modified, in ISO 8601 format." On restJson1 that is a JSON STRING of the form YYYY-MM-DDThh:mm:ss(.sss)Z. The instant itself is per-run and is not part of the requirement; the wire TYPE and format are.
Emulator: both members are emitted as JSON numbers of epoch seconds with a fractional part — the restJson1 DEFAULT timestamp encoding — ignoring the member's timestampFormat "date-time". Replay detail: $.subscriptionSummaries[arn="..."].createdAt and .updatedAt value_mismatch, expected a shaped ISO-8601 date-time string ('<shaped-not-asserted>'), got the JSON number 1794594197.996.
Why: an SDK deserializing a modeled date-time member against a JSON number gets a type error or a nonsense instant for every listed subscription, so reconciliation, paging or reporting code that sorts or filters on createdAt/updatedAt cannot use the value at all.
Fix: the response serializer for the list shapes — the SubscriptionSummary serializer used by the ListSubscriptions output type must render these Timestamp members in the model's date-time format, reusing the service's RFC 3339 renderer. One serializer change, shared with the Get/Create representation path, not a per-handler one.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, score_pct=46.2, steps_total=78, steps_passed=36, steps_mismatched=42, steps_blocked=0, steps_errored=0, divergence_count=16. Concrete values quoted below are per-run (a minted subscription ARN, a hex digest, instants, account 000000000000) and only fill the templates; they are not themselves requirements.

Divergence cluster index 4 — verdict=mismatch, service=pricing-plan-manager, operation=ListSubscriptions, fixture_id=pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step_key=task#9, expected_status=200, actual_status=200, affected_count=1, owned_by=[] (NOT already filed), stub_path="":
  divergence_key: service=pricing-plan-manager|ListSubscriptions|status=200->200|keys=$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/10a32fb6-41ea-4216-e3a1-f4f9a23cf9f3"].createdAt,$.subscriptionSummaries[arn="..."].eTag,$.subscriptionSummaries[arn="..."].updatedAt
  $.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/10a32fb6-41ea-4216-e3a1-f4f9a23cf9f3"].eTag: value_mismatch — expected '2' (a small decimal revision), got 'c646a46ae40e5288deb1ae1e5b96650d' (a 32-character hex digest).
  $.subscriptionSummaries[arn="...same arn..."].createdAt: value_mismatch — expected an ISO-8601 date-time string (asserted as shaped, not literal: '<shaped-not-asserted>'), got the JSON number 1794594197.996.
  $.subscriptionSummaries[arn="...same arn..."].updatedAt: value_mismatch — expected an ISO-8601 date-time string ('<shaped-not-asserted>'), got the JSON number 1794594197.996.

Model evidence (smithy_query query=shape shape=SubscriptionSummary, model_path services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): eTag -> String, required=true, doc "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription."; createdAt -> Timestamp, required=true, timestampFormat "date-time", doc "The date and time when the subscription was created, in ISO 8601 format."; updatedAt -> Timestamp, required=true, timestampFormat "date-time", doc "The date and time when the subscription was last modified, in ISO 8601 format." The sibling full shape Subscription declares the same two date-time members, so one shared renderer covers both representations. ListSubscriptions is POST /v1/ListSubscriptions, success code 200, paginated on nextToken/subscriptionSummaries, output member subscriptionSummaries -> SubscriptionSummaryList (required) (smithy_query query=operation operation=ListSubscriptions).

No status or error-code divergence is involved: the operation answers the modeled 200 and no error message is at issue, so this is purely a response-encoding / state-representation defect on two required date-time members plus the required concurrency token.

## Steps to Reproduce
Point a client at the emulator's data listener (region us-east-1, pricingplanmanager endpoint).
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<a CloudFront distribution ARN>, <its CLOUDFRONT-scope wafv2 web ACL ARN>]).
2. Poll GetSubscription(arn=<the minted subscription ARN>) until status is ACTIVE — i.e. let the subscription take at least one state mutation after creation.
3. ListSubscriptions().
4. Inspect the returned subscriptionSummaries entry for that ARN: eTag is a 32-character hex digest where AWS publishes a small decimal revision (the recording holds "2" for a once-mutated subscription), and createdAt/updatedAt are JSON numbers of epoch seconds where AWS sends ISO-8601 date-time strings.
5. Echo the listed eTag in If-Match on CancelSubscription to see that the token's form, not just its value, differs from AWS's.

Replay: call replay_wire_corpus and re-run fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9.
