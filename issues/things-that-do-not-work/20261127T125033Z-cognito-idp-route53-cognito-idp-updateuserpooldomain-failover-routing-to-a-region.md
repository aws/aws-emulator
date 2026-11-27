Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__route53.domain-failover-to-region-without-replica-rejected
Issue-Key: cognito-idp__route53.domain-failover-to-region-without-replica-rejected|run#17|AssertionError|assert got == expected, f"failover to a Region without a replica was refused with {got!r}, expected {expected!r}"
Issue-Commit: 760ea4c235b0ff162849feaaf99865499e566e9b

# cognito-idp UpdateUserPoolDomain: failover routing to a Region with no replica is refused with a prefix-domain message instead of naming the inactive Region

> _Note: this issue was identified against the code base at commit `760ea4c235b0ff162849feaaf99865499e566e9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPoolDomain

## Description
Cause 1 of 1: UpdateUserPoolDomain refuses a Routing.Failover update for the wrong reason.
AWS: a user-pool domain that is asked to fail over to a Region holding no replica of the pool is refused with InvalidParameterException, HTTP 400, and the message "Region <secondary region> is not active for the user pool" — fixed text with the Region taken from Routing.Failover.SecondaryRegion in the request; the refusal names the inactive Region, not the absent Route 53 health check, and applies nothing (the domain keeps its previous ManagedLoginVersion and carries no Routing). The identical update without Routing is accepted. Reference: Cognito API_FailoverType.
Emulator: refuses with the right code but a different message — InvalidParameterException, "Routing is supported only for a custom domain, and <domain prefix> is an Amazon Cognito prefix domain." — reporting the domain kind rather than the Region's replica state.
Why: a client that reads Error.Message to tell 'add a replica in that Region' from 'this domain kind cannot route at all' gets the wrong diagnosis and takes the wrong remediation; the Region it must activate is never named.
Fix: the refusal path of the UpdateUserPoolDomain handler's routing validation — it must evaluate the pool's active Regions for Routing.Failover.SecondaryRegion and emit AWS's message for that class.
Traces: cognito-idp__route53.domain-failover-to-region-without-replica-rejected.

## Evidence
From the replay output (cognito-idp__route53.domain-failover-to-region-without-replica-rejected):
AssertionError: failover to a Region without a replica was refused with ('InvalidParameterException', 'Routing is supported only for a custom domain, and sim2real-fo-<suffix> is an Amazon Cognito prefix domain.'), expected ('InvalidParameterException', 'Region us-east-1 is not active for the user pool').
The fixed text required is 'Region <region> is not active for the user pool'; the region is the request's Routing.Failover.SecondaryRegion (us-east-1 in this run, us-west-2 when the pool is in us-east-1) and the domain prefix in the emulator's message is a per-run generated name. The earlier GetHealthCheck control in the same check passes, so only this refusal diverges.

## Steps to Reproduce
1. CreateUserPool, then CreateUserPoolDomain with a prefix domain and ManagedLoginVersion=1; wait for DescribeUserPoolDomain Status=ACTIVE.
2. Call UpdateUserPoolDomain(Domain=<prefix>, UserPoolId=<pool>, ManagedLoginVersion=2, Routing={'Failover': {'SecondaryRegion': '<any other region>', 'PrimaryRoute53HealthCheckId': '<any uuid>'}}).
3. Observe InvalidParameterException with the prefix-domain message instead of 'Region <secondary region> is not active for the user pool'.
4. DescribeUserPoolDomain still reports ManagedLoginVersion 1 and no Routing (correct), and the same update without Routing is accepted (correct).
