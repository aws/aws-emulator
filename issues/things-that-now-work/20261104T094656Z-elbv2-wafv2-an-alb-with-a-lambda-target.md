Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target
Issue-Key: elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target|_poll_until#23|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# elbv2__wafv2: an ALB with a Lambda target never answers before (or after) a WebACL is associated, so WebACL blocking cannot take effect

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: the ALB forwarding to a Lambda target produces no HTTP answer, so the check's baseline — the request succeeding before any WebACL exists — is never reached and the WebACL's Block decision is never observable.
AWS: recorded behavior only: an ALB listener forwarding to a lambda-type target group answers a client request with the function's response (statusCode/headers/body from the Lambda target response contract) while no WebACL is associated; once a WebACL with a matching Block rule is associated, the ALB itself answers HTTP 403 and never invokes the function.
Emulator: the baseline request is never answered — the probe for the ALB forwarding to the Lambda target before any WebACL exists exhausts the run budget — so neither the pre-association 2xx nor the post-association 403 is produced.
Why: a client cannot establish that WAF enforcement changed anything, because the unprotected path never worked; a WAF-protected ALB in front of a Lambda target is entirely non-functional rather than refused with a diagnosable error.
Fix: the ELBv2 data-plane listener's lambda-target forwarding path (listener/forwarder plus the lambda target-group invoker), and the WebACL check that must precede it. Note the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers Lambda Invoke answering NotImplemented; this check does not reach an Invoke error at all — the listener produces no response — and the suite did not mark it as owned by that issue.
Traces: elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target.

## Evidence
Replay output for elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target:
`TimeoutError: the ALB forwarding to the Lambda target before any WebACL exists: the run budget ended first; not graded`

Expected: before association, the ALB answers with the Lambda target's response (2xx); after AssociateWebACL with a Block-matching rule, the ALB answers HTTP 403 without invoking the function. Actual: no HTTP response at either stage.

## Steps to Reproduce
1. Replay the behavioral suite and re-run elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target.
2. Create a lambda-type target group registering a Lambda function, attach it to an ALB listener, and send a request with no WebACL associated — expect the function's response.
3. Create a WAFv2 WebACL whose rule Blocks that request and AssociateWebACL with the load balancer.
4. Re-send the request and expect HTTP 403 from the ALB with no invocation; the emulator answers nothing at either step.
