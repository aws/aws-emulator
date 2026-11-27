Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__route53.alias-to-load-balancer-with-foreign-hosted-zone-id-rejected
Issue-Key: elbv2__route53.alias-to-load-balancer-with-foreign-hosted-zone-id-rejected|_expect_error#8|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded: {result!r}")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# elbv2__route53: ChangeResourceRecordSets accepts an ALB alias declared under a foreign hosted-zone id instead of refusing it with InvalidChangeBatch

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
Cause 1 of 1: an A-record alias whose AliasTarget.HostedZoneId is not the load balancer's canonical hosted zone is accepted.
AWS: documented alias-target rule — an alias RRSet's AliasTarget.HostedZoneId must be the hosted zone of the target itself (for an ALB, the load balancer's CanonicalHostedZoneId). Declaring the ALB's DNSName under another service's zone id (CloudFront's Z2FDTNDATAQYW2) is refused with HTTP 400 InvalidChangeBatch and the message `[Tried to create an alias that targets <load balancer DNS name>., type A in zone Z2FDTNDATAQYW2, but the alias target name does not lie within the target zone]` — fixed text with the alias target name, record type and the zone id taken from the change batch — and no record set is created.
Emulator: the change batch is accepted and a change id is returned (the call succeeds), so the hosted-zone id of an alias target is never validated against the target's own zone. The positive control (the same alias under the load balancer's CanonicalHostedZoneId) also succeeded, so only the validation is missing.
Why: a client that mis-declares an alias zone id gets working DNS in the emulator and a hard InvalidChangeBatch in AWS; worse, the emulator stores an alias AWS cannot represent, so a later ListResourceRecordSets/DELETE round trip diverges too.
Fix: the ChangeResourceRecordSets handler's alias-target validation (match AliasTarget.HostedZoneId against the resolved target's hosted zone before applying the batch).
Traces: elbv2__route53.alias-to-load-balancer-with-foreign-hosted-zone-id-rejected.

## Evidence
`AssertionError: expected an alias to the load balancer under CloudFront's hosted zone id to be rejected, but the call succeeded: '/change/<change-id>'`. Expected code InvalidChangeBatch at HTTP 400 with message `[Tried to create an alias that targets <alb-dns-name>., type A in zone Z2FDTNDATAQYW2, but the alias target name does not lie within the target zone]`, and no `bad.<zone>` A record afterwards (the change id and DNS name are per-run values).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create an internal ALB and a public hosted zone.
3. ChangeResourceRecordSets with an UPSERT of an A alias `ok.<zone>` whose AliasTarget = {DNSName: <alb dns name>, HostedZoneId: <alb CanonicalHostedZoneId>, EvaluateTargetHealth: false} — this must succeed.
4. ChangeResourceRecordSets with the same alias for `bad.<zone>` but HostedZoneId='Z2FDTNDATAQYW2' — expect a 400 InvalidChangeBatch refusal and no record set; observe the change id returned instead.
