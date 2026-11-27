Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__wafv2.wafv2-associate-web-acl-with-a-never-created-load-balancer-rejected
Issue-Key: elbv2__wafv2.wafv2-associate-web-acl-with-a-never-created-load-balancer-rejected|run#18|AssertionError|assert code == "WAFNonexistentItemException", f"associating a never-created load balancer gave {code!r}"
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# elbv2__wafv2: AssociateWebACL with a non-existent load balancer ARN is refused with WAFUnavailableEntityException instead of WAFNonexistentItemException

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: the wrong error class is used when the association's ResourceArn names no resource.
AWS: Smithy (wafv2) — AssociateWebACL models both WAFNonexistentItemException and WAFUnavailableEntityException. When ResourceArn is a well-formed regional load balancer ARN in the caller's own account and region that names no load balancer, the call is refused with WAFNonexistentItemException and the message `AWS WAF couldn’t perform the operation because your resource doesn’t exist.` (fixed text, nothing interpolated; note the typographic apostrophe). WAFUnavailableEntityException is AWS's class for an entity that exists but is not in a usable state, not for one that was never created.
Emulator: refuses the same call with WAFUnavailableEntityException, so the error code (and therefore the SDK exception type a client catches) is wrong. The positive control — associating the same web ACL with a real load balancer, then GetWebACLForResource — behaved as expected.
Why: a client distinguishing 'this resource does not exist' (fix the ARN) from 'this entity is temporarily unusable' (retry) branches the wrong way: a typo'd or deleted ARN looks retryable forever, and code catching WAFNonexistentItemException never sees it.
Fix: the AssociateWebACL handler's resource-resolution error mapping (unknown ResourceArn must raise the nonexistent-item error with AWS's message).
Traces: elbv2__wafv2.wafv2-associate-web-acl-with-a-never-created-load-balancer-rejected.

## Evidence
`AssertionError: associating a never-created load balancer gave 'WAFUnavailableEntityException'` — expected code `WAFNonexistentItemException` with message `AWS WAF couldn’t perform the operation because your resource doesn’t exist.`, and ListResourcesForWebACL(ResourceType='APPLICATION_LOAD_BALANCER') listing only the real load balancer afterwards. The ghost ARN is built from the real load balancer's ARN with an unused name and id, so only its identifiers are per-run values.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateWebACL(Scope='REGIONAL'), create an internal ALB and wait for it to become active.
3. AssociateWebACL(WebACLArn=<acl>, ResourceArn=<alb arn>) as the positive control, then assert GetWebACLForResource returns the ACL.
4. AssociateWebACL(WebACLArn=<acl>, ResourceArn=<same ARN prefix>/ghost-<unused name>/0123456789abcdef) and inspect Error.Code and Error.Message.
