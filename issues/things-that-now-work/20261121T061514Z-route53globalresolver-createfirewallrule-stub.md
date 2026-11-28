Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53globalresolver.create-firewall-rule-layers-field-validation-ahead-of-dns-view-lookup
Issue-Shape-Id: com.amazonaws.route53globalresolver#EC2DNSGlobalResolverCustomerAPI
Issue-Key: route53globalresolver.create-firewall-rule-layers-field-validation-ahead-of-dns-view-lookup|run#10|AssertionError|assert code == "ValidationException", (
Issue-Commit: bab32ab3ecd51bb55a27809c2e18220867c710f4

# Implement route53globalresolver CreateFirewallRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bab32ab3ecd51bb55a27809c2e18220867c710f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallRule

## Description
`CreateFirewallRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi/src/ops/create_firewall_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateFirewallRule` call, so each one unblocks only when `CreateFirewallRule` works: `route53globalresolver.create-firewall-rule-layers-field-validation-ahead-of-dns-view-lookup`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi/src/ops/create_firewall_rule.rs`. Replaying the corpus, the emulator refused the `CreateFirewallRule` call: File "run.py", line 456, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_route53globalresolver_create_firewall_rule_layers_field_validation_ahead_of_dns_view_lookup.py", line 64, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a rule naming no target (neither FirewallDomainListId nor DnsAdvancedProtection) must fail that cross-field rule before dnsViewId is ever looked up, got NotImplemented: 'operation CreateFirewallRule is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the route53globalresolver emulator
Call CreateFirewallRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
