Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__pricing-plan-manager__wafv2.create-subscription-without-webacl-association-rejected
Issue-Key: cloudfront__pricing-plan-manager__wafv2.create-subscription-without-webacl-association-rejected|_error#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudfront__pricing-plan-manager__wafv2: CreateSubscription does not check resource eligibility — a CloudFront distribution with no WAF web ACL associated is accepted instead of refused with ValidationException

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscription

## Description
Cause 1 of 1: CreateSubscription never evaluates whether the resources named in resourceArns are eligible for the requested plan family/tier, so a CloudFront distribution that has no WAF web ACL associated with it is subscribed instead of refused.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): CreateSubscription.resourceArns targets ResourceArns (@required, @length 1..10) and its member doc states "For subscriptions in the CloudFront plan family, the resources must include exactly one Amazon CloudFront distribution and exactly one WAF web ACL"; eligibility is a business rule, reported through the modeled ValidationException (httpError 400, members message + resourceId, doc "The request failed a business rule validation"). The modeled rule is about the resources NAMED in the call; the additional requirement this check records is that the named distribution must actually have the named web ACL associated with it at CloudFront. Recorded behavior: the refusal is HTTP 400, Error.Code "ValidationException", Error.Message "The following resources are not eligible for this subscription tier: <arns>." — the fixed text is "The following resources are not eligible for this subscription tier: " and the trailing ".", and the variable part is the list of ineligible resource ARNs taken from the request (here the distribution ARN). The same call with the identical arguments must succeed once the web ACL is associated to the distribution (the check's positive control), so this is a state-dependent gate, not a static shape check.
Emulator: with a freshly created, Deployed CloudFront distribution that has no WebACLId set, CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<dist-arn>, <webacl-arn>], approvalMode="IMMEDIATE") answers HTTP 200 and mints an ACTIVE subscription covering both ARNs. services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/create_subscription.rs handle() (lines 190-294) validates only approvalMode and account capacity, then mints and inserts the record; resource_arns is moved into state unexamined, so there is no CloudFront lookup and no WAF-association check at all.
Why: a client that relies on AWS gating the subscription on the prerequisite WAF association gets no signal that its distribution is not protected. It believes it has bought a flat-rate CloudFront plan for a resource that is not eligible for it, and because the rejected call is supposed to leave no subscription behind, the emulator additionally leaves a subscription naming the distribution that ListSubscriptions then reports — the check's effect-absent assertion. The ordering it enforces (attach the web ACL, then subscribe) is exactly what a real deployment must do, and the emulator lets the wrong order through.
Fix: CreateSubscription's handler/validation in services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/create_subscription.rs, with a resource-eligibility gate that resolves each resourceArns entry against the owning service's state (for the CloudFront plan family: the distribution's current WebACLId must name the web ACL also present in resourceArns) and returns the modeled ValidationException with the message above, listing the ineligible ARNs, before any state mutation. The same gate must admit the call once the association exists.
Traces: cloudfront__pricing-plan-manager__wafv2.create-subscription-without-webacl-association-rejected. (The sibling check cloudfront__pricing-plan-manager__wafv2.subscription-resource-association-reaches-active is marked ALREADY FILED against the unimplemented DisassociateResourcesFromSubscription stub and is not part of this issue.)

## Evidence
Check cloudfront__pricing-plan-manager__wafv2.create-subscription-without-webacl-association-rejected.
Setup: cloudfront CreateDistribution (Enabled=false, no WebACLId) polled to Deployed; wafv2 CreateWebACL(Scope=CLOUDFRONT); no association made yet.
Expected (AWS): HTTP 400, Error.Code="ValidationException", Error.Message="The following resources are not eligible for this subscription tier: <distribution-arn>." (fixed text + the ineligible ARN(s) from the request), and no subscription naming the distribution afterwards.
Actual (emulator):
  AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'eTag': '...', 'subscription': {'arn': 'arn:aws:pricingplanmanager:us-east-1:<account>:subscription/<uuid>', 'planFamily': 'CloudFront', 'planTier': 'FREE', 'status': 'ACTIVE', 'resourceArns': ['arn:aws:cloudfront::<account>:distribution/<dist-id>', 'arn:aws:wafv2:us-east-1:<account>:global/webacl/<acl-name>/<uuid>'], 'createdAt': ..., 'updatedAt': ...}}
(The distribution id, ACL name, subscription arn, eTag and timestamps are per-run values and are not part of the requirement.)
Source: services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/create_subscription.rs:190-294 — no inspection of resource_arns between destructuring (line 190) and SubscriptionRecord construction (line 274).
Smithy: CreateSubscription.resourceArns @required ResourceArns @length(1,10), member doc "the resources must include exactly one Amazon CloudFront distribution and exactly one WAF web ACL"; ValidationException httpError 400.

## Steps to Reproduce
1. cloudfront: CreateDistribution with Enabled=false, one custom origin, CachePolicyId=4135ea2d-6df8-44a3-9df3-4b5a84be39ad, and NO WebACLId; poll GetDistribution until Status=="Deployed". Note Distribution.ARN.
2. wafv2 (region us-east-1): CreateWebACL(Scope="CLOUDFRONT", DefaultAction={"Allow":{}}); note Summary.ARN.
3. pricing-plan-manager (us-east-1): CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<distribution ARN>, <web ACL ARN>], approvalMode="IMMEDIATE").
4. Observe HTTP 200 and an ACTIVE subscription. AWS refuses with HTTP 400 ValidationException "The following resources are not eligible for this subscription tier: <distribution ARN>." and creates nothing.
5. Confirm the leak: ListSubscriptions (all pages) finds a summary whose resourceArns contains the distribution ARN; AWS finds none.
6. Positive control that must keep working: cloudfront UpdateDistribution to set WebACLId=<web ACL ARN>, poll to Deployed, repeat step 3 — this call must succeed and reach status ACTIVE with both ARNs associated.

Replay: run_behavioral_suite and re-run cloudfront__pricing-plan-manager__wafv2.create-subscription-without-webacl-association-rejected.
