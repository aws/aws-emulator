Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.associate-firewall-rule-group-priority-below-floor-rejected route53resolver.update-firewall-rule-group-association-invalid-priority-rejected route53resolver.create-firewall-rule-duplicate-priority-rejected route53resolver.concurrent-firewall-rule-priority-race route53resolver.create-firewall-rule-block-without-response-rejected route53resolver.delete-firewall-domain-list-conflict-when-referenced route53resolver.delete-firewall-rule-group-active-association-rejected route53resolver.delete-outpost-resolver-with-active-endpoints-conflict-rejected route53resolver.firewall-domain-case-normalization-preserved route53resolver.firewall-domain-unicode-round-trip route53resolver.firewall-domain-unicode-wildcard-round-trip route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow route53resolver.create-firewall-rule-override-without-block-override-rejected route53resolver.firewall-rule-group-disassociate-mutation-protection-rejected route53resolver.get-firewall-rule-group-association-not-found route53resolver.mutation-protection-blocks-disassociate route53resolver.disassociate-firewall-rule-group-nonexistent-association-rejected route53resolver.disassociate-rejected-when-mutation-protection-enabled
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.associate-firewall-rule-group-priority-below-floor-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.update-firewall-rule-group-association-invalid-priority-rejected|run#9|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: route53resolver.create-firewall-rule-duplicate-priority-rejected|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.concurrent-firewall-rule-priority-race|run#20|AssertionError|assert message == expected, f"expected message {expected!r} for a losing racer, got {message!r}"
Issue-Key: route53resolver.create-firewall-rule-block-without-response-rejected|run#13|AssertionError|assert ERR_CODE_BLOCK_RESPONSE in message, (
Issue-Key: route53resolver.delete-firewall-domain-list-conflict-when-referenced|_client_error#10|AssertionError|raise AssertionError("the call succeeded; expected AWS to refuse it with a ClientError")
Issue-Key: route53resolver.delete-firewall-rule-group-active-association-rejected|_client_error#8|AssertionError|raise AssertionError("the call succeeded; expected AWS to refuse it with a ClientError")
Issue-Key: route53resolver.delete-outpost-resolver-with-active-endpoints-conflict-rejected|_client_error#10|AssertionError|raise AssertionError("the call succeeded; expected AWS to refuse it with a ClientError")
Issue-Key: route53resolver.firewall-domain-case-normalization-preserved|run#10|AssertionError|assert domains == [normalized_domain], (
Issue-Key: route53resolver.firewall-domain-unicode-round-trip|run#14|AssertionError|assert domains == [expected], (
Issue-Key: route53resolver.firewall-domain-unicode-wildcard-round-trip|run#13|AssertionError|assert expected_punycode_wildcard in domains, (
Issue-Key: route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow|run#12|AssertionError|assert expected_blocked_domain in domains, (
Issue-Key: route53resolver.create-firewall-rule-override-without-block-override-rejected|run#10|AssertionError|assert good["BlockOverrideDomain"] == CANONICAL_OVERRIDE_DOMAIN, good
Issue-Key: route53resolver|DisassociateFirewallRuleGroup|ConflictException
Issue-Key: route53resolver.disassociate-firewall-rule-group-nonexistent-association-rejected|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: route53resolver.disassociate-rejected-when-mutation-protection-enabled|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# route53resolver DNS Firewall control plane: 10 net-new causes — RSLVR-coded ValidationException wording is emulator-authored, delete-while-in-use is not refused at all, domain/override names are stored un-canonicalised, and DisassociateFirewallRuleGroup refuses a valid disassociate

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallRule

## Description
Ten net-new root causes in the DNS Firewall control plane of services/service-com-amazonaws-route53resolver-route53resolver. Causes 1-4 and 10 are refusal-message fidelity (right code, wrong sentence). Causes 5-6 accept calls AWS refuses. Causes 7-8 store a domain AWS canonicalises. Cause 9 refuses a call AWS accepts. Every cause below is grounded in the failing checks named under it.

=== Cause 1 of 10: AssociateFirewallRuleGroup's below-floor Priority refusal does not use AWS's RSLVR-02017 sentence ===
Failing check: route53resolver.associate-firewall-rule-group-priority-below-floor-rejected
AWS: Smithy models AssociateFirewallRuleGroup(CreatorRequestId, FirewallRuleGroupId, VpcId, Priority, Name, MutationProtection, Tags) -> AssociateFirewallRuleGroupResponse with errors AccessDenied | Conflict | InternalServiceError | LimitExceeded | ResourceNotFound | Throttling | ValidationException, all httpError 400 (defaulted). Priority is a plain integer with no @range, so the reserved band is recorded behavior only: for a Priority below the usable floor the recorded response is HTTP 400, Error.Code `ValidationException`, Error.Message `[RSLVR-02017] The priority value you provided is reserved. Provide a number between "100" and "9900".` Message anatomy: the whole sentence is FIXED text — the quoted "100" and "9900" are the service's own band bounds, and the requested priority does not appear at all.
Emulator: HTTP 400 / ValidationException (correct class) with a self-authored sentence of the form `Priority <requested> is outside the allowed values, which are between 100 and 9900` (recorded: `Priority 50 is outside the allowed values, which are between 100 and 9900`). No RSLVR code, different wording, and it echoes the request's priority AWS never names.
Why: a client that branches, logs or alarms on Resolver's RSLVR code cannot recognise the reserved-band refusal; negative tests written against AWS fail here and tests written against the emulator pass and then fail in production.
Fix: AssociateFirewallRuleGroup's Priority validation in the operation handler, which must build the sentence through the crate's shared RSLVR message formatter in src/state.rs instead of formatting its own string.
Traces: route53resolver.associate-firewall-rule-group-priority-below-floor-rejected

=== Cause 2 of 10: UpdateFirewallRuleGroupAssociation's above-ceiling Priority refusal does not use AWS's RSLVR-02018 sentence ===
Failing check: route53resolver.update-firewall-rule-group-association-invalid-priority-rejected
AWS: Smithy models UpdateFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId required, Priority, MutationProtection, Name) with the same 400-only error set including ValidationException; Priority carries no @range, so the bound is recorded behavior only. For a Priority at or above the ceiling the recorded response is HTTP 400, `ValidationException`, Message `[RSLVR-02018] The Priority you provided is not valid. Provide a number smaller than "10000".` Anatomy: all FIXED text; the quoted "10000" is the service's bound, and the requested value is not echoed.
Emulator: HTTP 400 / ValidationException with `Priority <requested> is outside the allowed values, which are between 100 and 9900` (recorded: `Priority 99999 is ...`) — the emulator reuses one generic band message for both the floor (cause 1) and the ceiling case, where AWS returns two distinct coded sentences (RSLVR-02017 vs RSLVR-02018).
Why: the two refusals are indistinguishable to a client, and neither carries the RSLVR code AWS error handling keys on.
Fix: UpdateFirewallRuleGroupAssociation's Priority validation; the ceiling case needs its own RSLVR-02018 sentence from the shared formatter, separate from the floor case.
Traces: route53resolver.update-firewall-rule-group-association-invalid-priority-rejected

=== Cause 3 of 10: CreateFirewallRule's duplicate-priority refusal does not use AWS's RSLVR-02203 sentence ===
Failing checks: route53resolver.create-firewall-rule-duplicate-priority-rejected, route53resolver.concurrent-firewall-rule-priority-race
AWS: Smithy models CreateFirewallRule(..., FirewallRuleGroupId, Priority required, Action required, BlockResponse, Name required, Qtype, ...) with errors AccessDenied | InternalServiceError | LimitExceeded | ResourceNotFound | Throttling | ValidationException (all 400). The uniqueness of Priority inside a rule group is not modellable, so it is recorded behavior only: a second rule at a priority the group already holds is refused HTTP 400, `ValidationException`, Message `[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "<FirewallRuleGroupId>". Provide another priority.` Anatomy: FIXED text `[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "` … `". Provide another priority.`; the single VARIABLE part is the rule group's id from the request. Under the concurrency check the same sentence is required for the losing racer, so exactly one creation wins.
Emulator: `Priority <n> is already in use by another rule in rule group <group-id>` — right code, self-authored prose, no RSLVR-02203.
Why: clients cannot key on RSLVR-02203; a racer that must retry with a new priority cannot recognise why it lost.
Fix: the duplicate-priority check in CreateFirewallRule's handler (and the same check on the concurrent path), routed through the one shared RSLVR formatter. Related open issue 20261028T183627Z-route53resolver-6-net-new-causes-dns-firewall-duplicate.md names the same RSLVR-02203 sentence for route53resolver.create-firewall-rule-qtype-does-not-scope-priority-uniqueness; it does not carry these two checks' replay keys, so they are filed here and may be closed by the same fix.
Traces: route53resolver.create-firewall-rule-duplicate-priority-rejected, route53resolver.concurrent-firewall-rule-priority-race

=== Cause 4 of 10: CreateFirewallRule's BLOCK-without-BlockResponse refusal does not use AWS's RSLVR-02016 sentence ===
Failing check: route53resolver.create-firewall-rule-block-without-response-rejected
AWS: BlockResponse is a plain optional enum on CreateFirewallRuleRequest, so the conditional requirement (Action=BLOCK requires BlockResponse) is recorded behavior only: HTTP 400, `ValidationException`, Message `[RSLVR-02016] Your block option BLOCK RESPONSE is not valid.` Anatomy: entirely FIXED text — it names the block option (BLOCK RESPONSE) and nothing from the request.
Emulator: HTTP 400 with `BlockResponse is required when Action is BLOCK` — no RSLVR-02016, and it names the API member spelling rather than the option name AWS prints.
Why: a client matching on the code, or on the "BLOCK RESPONSE" option name, cannot tell this conditional-requirement failure from any other validation failure on the same operation.
Fix: CreateFirewallRule's conditional BLOCK/BlockResponse validation, taking its sentence from the shared RSLVR formatter.
Traces: route53resolver.create-firewall-rule-block-without-response-rejected

=== Cause 5 of 10: DeleteFirewallDomainList deletes a domain list a FirewallRule still references instead of refusing ConflictException RSLVR-02304 ===
Failing check: route53resolver.delete-firewall-domain-list-conflict-when-referenced
AWS: Smithy models DeleteFirewallDomainList(FirewallDomainListId required) with errors AccessDenied | ConflictException | InternalServiceError | ResourceNotFound | Throttling (all 400). The in-use rule is recorded behavior only: while a FirewallRule references the list, the call is refused HTTP 400, Error.Code `ConflictException`, Message `[RSLVR-02304] The DNS Firewall domain list is associated with a DNS Firewall rule : "<FirewallRuleGroupId of the referencing rule>". Delete the rule and then try again. Trace Id: "<Root of the caller's X-Amzn-Trace-Id>"` — FIXED text with one VARIABLE id plus the per-request trace Root (see cause 10); the domain list itself stays in place and usable, and the identical call succeeds once the referencing rule is removed.
Emulator: the delete SUCCEEDS (HTTP 200) with the domain list still referenced by a live rule; the check's refusal assertion never gets a ClientError (`the call succeeded; expected AWS to refuse it with a ClientError`).
Why: the emulator orphans the rule — a FirewallRule is left pointing at a FirewallDomainListId that no longer exists, a state AWS makes unreachable. Cleanup/IaC code that relies on the conflict to order its deletes silently corrupts its own rule group here and then breaks against real AWS.
Fix: DeleteFirewallDomainList's handler must consult the rule index in src/state.rs for any FirewallRule referencing the list and refuse with the RSLVR-02304 ConflictException before mutating state.
Traces: route53resolver.delete-firewall-domain-list-conflict-when-referenced

=== Cause 6 of 10: DeleteFirewallRuleGroup deletes a rule group that still has a live VPC association instead of refusing ConflictException RSLVR-02101 ===
Failing checks: route53resolver.delete-firewall-rule-group-active-association-rejected, route53resolver.delete-outpost-resolver-with-active-endpoints-conflict-rejected
AWS: Smithy models DeleteFirewallRuleGroup(FirewallRuleGroupId required) with errors AccessDenied | ConflictException | InternalServiceError | ResourceNotFound | Throttling | ValidationException (all 400). The delete-while-associated rule is recorded behavior only: with a FirewallRuleGroupAssociation in COMPLETE on any VPC, the delete is refused HTTP 400, `ConflictException`, Message `[RSLVR-02101] The DNS Firewall rule group is associated to a VPC: "<FirewallRuleGroupId>". Delete the association and try again later. Trace Id: "<trace Root>"` — FIXED text, one VARIABLE id from the request, plus the trace Root. The group must still exist afterwards, and the same delete must succeed after DisassociateFirewallRuleGroup.
Emulator: the delete SUCCEEDS while the association is live; both checks fail at `the call succeeded; expected AWS to refuse it with a ClientError`.
Why: the association is orphaned — it names a rule group that no longer exists — and a caller gets no signal that it is tearing resources down out of order. (The second check uses DeleteFirewallRuleGroup deliberately as the stand-in for DeleteOutpostResolver's analogous in-use invariant, which needs physical Outposts hardware; the defect it exercises is this one.)
Fix: DeleteFirewallRuleGroup's handler must query the association index in src/state.rs and refuse with the RSLVR-02101 ConflictException before deleting.
Traces: route53resolver.delete-firewall-rule-group-active-association-rejected, route53resolver.delete-outpost-resolver-with-active-endpoints-conflict-rejected

=== Cause 7 of 10: UpdateFirewallDomains stores domains verbatim instead of canonicalising them to lowercase trailing-dot FQDNs ===
Failing checks: route53resolver.firewall-domain-case-normalization-preserved, route53resolver.firewall-domain-unicode-round-trip, route53resolver.firewall-domain-unicode-wildcard-round-trip, route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow
AWS: Smithy models UpdateFirewallDomains(FirewallDomainListId, Operation, Domains required) -> {Id, Name, Status, StatusMessage}; FirewallDomains members carry no pattern or format trait, so canonicalisation is recorded behavior only. Recorded behavior: a domain added to a FirewallDomainList is stored as a DNS-canonical name — lowercased and rooted with a trailing dot — and ListFirewallDomains reads back that canonical spelling, idempotently (an already-rooted name does not grow a second dot). This holds for ASCII, for punycode/ACE labels and for wildcard (`*.`) entries.
Emulator: the submitted spelling is echoed verbatim. Recorded: `ExAmple-MiXeDcAsE.TEST.` stays `ExAmple-MiXeDcAsE.TEST.` (expected `example-mixedcase.test.`); `xn--e1afmkfd.xn--80akhbyknj4f.` reads back as `xn--e1afmkfd.xn--80akhbyknj4f` (trailing dot dropped); the wildcard entry reads back as `*.xn--mnchen-3ya-<n>.example` with a sibling `CaseTest-<n>.Example` left mixed-case; and the blocklist lifecycle check gets `blocked-<n>.test` where `blocked-<n>.test.` is required.
Why: a client that stores the canonical name it expects AWS to return, or that compares a configured blocklist entry against ListFirewallDomains output, never finds its own domain; two spellings of the same name (`Foo.test` and `foo.test.`) become two distinct list entries here while AWS collapses them, so DomainCount and blocklist membership both diverge.
Fix: the domain-canonicalisation step on the UpdateFirewallDomains write path (lowercase + root the name once) in the handler or the FirewallDomainList writer in src/state.rs, so every reader projects the canonical form.
Traces: route53resolver.firewall-domain-case-normalization-preserved, route53resolver.firewall-domain-unicode-round-trip, route53resolver.firewall-domain-unicode-wildcard-round-trip, route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow

=== Cause 8 of 10: CreateFirewallRule echoes BlockOverrideDomain verbatim instead of canonicalising it to a lowercase trailing-dot FQDN ===
Failing check: route53resolver.create-firewall-rule-override-without-block-override-rejected
AWS: BlockOverrideDomain is modelled only as a 1-255 length string on CreateFirewallRuleRequest, so canonicalisation is recorded behavior only: `override.example.com`, `override.example.com.` and `OverRide.Example.COM` are all stored and projected as `override.example.com.` — a DNS canonicalisation, not a verbatim echo, and idempotent.
Emulator: CreateFirewallRule with BlockResponse=OVERRIDE and BlockOverrideDomain=`override.example.com` projects `BlockOverrideDomain: 'override.example.com'` on the FirewallRule (no trailing dot), so the round-trip assertion against the canonical form fails.
Why: a client that writes the override target and then compares it with what the rule reports sees a permanent diff and re-writes the rule forever; the un-rooted name is also the value a resolver would answer the blocked query with.
Fix: the BlockOverrideDomain canonicalisation on CreateFirewallRule (and the matching update path), sharing the one canonicaliser used for cause 7.
Traces: route53resolver.create-firewall-rule-override-without-block-override-rejected

=== Cause 9 of 10: DisassociateFirewallRuleGroup refuses a valid disassociate with ConflictException "The requested state transition isn't valid." ===
Failing checks: route53resolver.firewall-rule-group-disassociate-mutation-protection-rejected, route53resolver.get-firewall-rule-group-association-not-found, route53resolver.mutation-protection-blocks-disassociate
AWS: Smithy models DisassociateFirewallRuleGroup(FirewallRuleGroupAssociationId required) -> FirewallRuleGroupAssociation with errors AccessDenied | Conflict | InternalServiceError | ResourceNotFound | Throttling | ValidationException (all 400). Recorded behavior: disassociating an association the caller just created (and whose MutationProtection is not ENABLED) is ACCEPTED — HTTP 200 with the association projected in DELETING, after which it disappears and reads flip to ResourceNotFoundException RSLVR-02025. ConflictException is reserved for a genuine conflict (mutation protection, or an association not in a disassociable state).
Emulator: every one of these three checks dies on a disassociate the service should accept, with HTTP 400 `ConflictException: The requested state transition isn't valid.` — so the association can never be torn down, and the checks never reach their own assertions (two of them are about mutation protection, the third about the not-found flip after disassociate).
Why: the whole association lifecycle is a dead end — a caller can associate a rule group with a VPC and then can never disassociate it, so the VPC cannot be cleaned up, the rule group can never be deleted (cause 6's precondition), and no client code that follows AWS's associate/disassociate lifecycle can run against the emulator at all.
Fix: the state-transition guard in DisassociateFirewallRuleGroup's handler and the association state machine it reads in src/state.rs — a COMPLETE association with MutationProtection unset must transition to DELETING, not be refused.
Traces: route53resolver.firewall-rule-group-disassociate-mutation-protection-rejected, route53resolver.get-firewall-rule-group-association-not-found, route53resolver.mutation-protection-blocks-disassociate

=== Cause 10 of 10: DisassociateFirewallRuleGroup refusal Messages omit the trailing X-Ray `Trace Id: "<Root>"` clause ===
Failing checks: route53resolver.disassociate-firewall-rule-group-nonexistent-association-rejected, route53resolver.disassociate-rejected-when-mutation-protection-enabled
AWS: ResourceNotFoundException and ConflictException carry only {Message}, so the text is recorded behavior only. Recorded behavior: a DNS Firewall refusal Message ends with the caller's X-Ray trace Root, appended as the literal ` Trace Id: "<Root>"` where <Root> is the Root value of the request's X-Amzn-Trace-Id header. Full required Messages: `[RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupAssociationId>". Trace Id: "<Root>"` for an unknown association, and `[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again. Trace Id: "<Root>"` for a protected one. FIXED text is everything but the id (from the request) and the Root (from the request's header) — the Root is a per-request value, but the PRESENCE of the clause and its exact `. Trace Id: "` framing are the requirement.
Emulator: the RSLVR code, the sentence and the id are all correct, but the Message stops at the period: `[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-...".` and `[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again.` — the whole last field is missing.
Why: a client or support workflow that parses the trace Root out of the Message to correlate a refusal with its request has nothing to parse, and exact-message assertions written against AWS fail.
Fix: the error serializer / RSLVR message formatter for this crate must append the ` Trace Id: "<Root>"` clause from the request's X-Amzn-Trace-Id on every RSLVR-coded refusal, including the DisassociateFirewallRuleGroup paths. Open issue 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md describes this same omission for other operations but does not carry these two checks' replay keys; they are filed here and may be closed by that fix.
Traces: route53resolver.disassociate-firewall-rule-group-nonexistent-association-rejected, route53resolver.disassociate-rejected-when-mutation-protection-enabled

## Evidence
Behavioral suite, service route53resolver (bootstrapped), 18 failing checks in this bundle.

Cause 1 — associate-firewall-rule-group-priority-below-floor-rejected:
  expected message '[RSLVR-02017] The priority value you provided is reserved. Provide a number between "100" and "9900".'
  got            'Priority 50 is outside the allowed values, which are between 100 and 9900'

Cause 2 — update-firewall-rule-group-association-invalid-priority-rejected:
  expected ('ValidationException', '[RSLVR-02018] The Priority you provided is not valid. Provide a number smaller than "10000".')
  got      ('ValidationException', 'Priority 99999 is outside the allowed values, which are between 100 and 9900')

Cause 3 — create-firewall-rule-duplicate-priority-rejected:
  expected message '[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same priority: "rslvr-frg-910a2dec89025cc1". Provide another priority.'
  got            'Priority 200 is already in use by another rule in rule group rslvr-frg-910a2dec89025cc1'
  concurrent-firewall-rule-priority-race: same expected sentence for the losing racer; got 'Priority 999 is already in use by another rule in rule group rslvr-frg-910a2dec89025cc1'

Cause 4 — create-firewall-rule-block-without-response-rejected:
  expected '[RSLVR-02016] Your block option BLOCK RESPONSE is not valid.' (code RSLVR-02016 must appear in Message)
  got      'BlockResponse is required when Action is BLOCK'

Cause 5 — delete-firewall-domain-list-conflict-when-referenced:
  expected ConflictException '[RSLVR-02304] The DNS Firewall domain list is associated with a DNS Firewall rule : "<group-id>". Delete the rule and then try again. Trace Id: "<Root>"'
  got      HTTP 200 — AssertionError: the call succeeded; expected AWS to refuse it with a ClientError

Cause 6 — delete-firewall-rule-group-active-association-rejected and delete-outpost-resolver-with-active-endpoints-conflict-rejected:
  expected ConflictException '[RSLVR-02101] The DNS Firewall rule group is associated to a VPC: "<group-id>". Delete the association and try again later. Trace Id: "<Root>"'
  got      HTTP 200 — AssertionError: the call succeeded; expected AWS to refuse it with a ClientError

Cause 7 — domain canonicalisation:
  firewall-domain-case-normalization-preserved: expected 'ExAmple-MiXeDcAsE.TEST.' normalized to 'example-mixedcase.test.' on storage, got ['ExAmple-MiXeDcAsE.TEST.']
  firewall-domain-unicode-round-trip: expected ['xn--e1afmkfd.xn--80akhbyknj4f.'], got ['xn--e1afmkfd.xn--80akhbyknj4f'] (DomainCount=1)
  firewall-domain-unicode-wildcard-round-trip: punycode wildcard domain '*.xn--mnchen-3ya-<n>.example.' not found, got ['*.xn--mnchen-3ya-<n>.example', 'CaseTest-<n>.Example']
  e2e-firewall-domain-blocklist-lifecycle-flow: expected 'blocked-<n>.test.' in domain list, got ['blocked-<n>.test']

Cause 8 — create-firewall-rule-override-without-block-override-rejected:
  assert good["BlockOverrideDomain"] == 'override.example.com.' failed; rule projected 'BlockOverrideDomain': 'override.example.com' (BlockResponse=OVERRIDE, BlockOverrideDnsType=CNAME, BlockOverrideTtl=60)

Cause 9 — firewall-rule-group-disassociate-mutation-protection-rejected, get-firewall-rule-group-association-not-found, mutation-protection-blocks-disassociate:
  all three: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the DisassociateFirewallRuleGroup operation: The requested state transition isn't valid.
  expected: HTTP 200, association projected DELETING, then reads flip to ResourceNotFoundException RSLVR-02025

Cause 10 — Trace Id clause:
  disassociate-firewall-rule-group-nonexistent-association-rejected: expected '[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-00000000000000000". Trace Id: "1-6ae39d33-b2e33c68b26149b289c01230"', got '[RSLVR-02025] Can’t find the resource with ID "rslvr-frgassoc-00000000000000000".'
  disassociate-rejected-when-mutation-protection-enabled: expected '[RSLVR-02307] The DNS Firewall rule group association is under mutation protection. Remove the mutation protection and try again. Trace Id: "1-6ae39b74-bec36bf9db7a424780af7beb"', got the same sentence with no Trace Id clause (the Root value itself is per-request; the clause is the requirement)

## Steps to Reproduce
Replay the 18 behavioral checks listed in trace_ids against the emulator (service route53resolver). Minimal reproductions per cause:
1. AssociateFirewallRuleGroup(CreatorRequestId, FirewallRuleGroupId=<group>, VpcId=<vpc>, Priority=50, Name=<n>) -> compare Error.Message with RSLVR-02017 sentence.
2. AssociateFirewallRuleGroup(..., Priority=200) then UpdateFirewallRuleGroupAssociation(FirewallRuleGroupAssociationId=<assoc>, Priority=99999) -> compare with RSLVR-02018 sentence.
3. CreateFirewallDomainList + CreateFirewallRuleGroup, CreateFirewallRule(Priority=200, Action=BLOCK, BlockResponse=NXDOMAIN), then a second CreateFirewallRule at Priority=200 -> compare with RSLVR-02203 sentence. Issue two such creates concurrently for the race variant.
4. CreateFirewallRule(Action=BLOCK) with BlockResponse omitted -> compare with RSLVR-02016 sentence.
5. CreateFirewallDomainList + CreateFirewallRuleGroup + CreateFirewallRule referencing the list, then DeleteFirewallDomainList(FirewallDomainListId=<list>) -> must be ConflictException RSLVR-02304, not 200.
6. CreateFirewallRuleGroup + AssociateFirewallRuleGroup(VpcId=<vpc>, Priority=150), poll the association to COMPLETE, then DeleteFirewallRuleGroup(FirewallRuleGroupId=<group>) -> must be ConflictException RSLVR-02101, not 200.
7. UpdateFirewallDomains(FirewallDomainListId=<list>, Operation=ADD, Domains=['ExAmple-MiXeDcAsE.TEST.', 'xn--e1afmkfd.xn--80akhbyknj4f.', '*.xn--mnchen-3ya-x.example.']) then ListFirewallDomains -> every entry must read back lowercased and trailing-dot rooted.
8. CreateFirewallRule(Action=BLOCK, BlockResponse=OVERRIDE, BlockOverrideDomain='override.example.com', BlockOverrideDnsType=CNAME, BlockOverrideTtl=60) -> response BlockOverrideDomain must be 'override.example.com.'.
9. CreateFirewallRuleGroup + AssociateFirewallRuleGroup (MutationProtection unset), wait for COMPLETE, then DisassociateFirewallRuleGroup(FirewallRuleGroupAssociationId=<assoc>) -> must be accepted (DELETING), not ConflictException "The requested state transition isn't valid.".
10. Send X-Amzn-Trace-Id: Root=<root>;Sampled=0 on the client, then DisassociateFirewallRuleGroup with an unknown association id, and (separately) on an association whose MutationProtection=ENABLED -> each Error.Message must end with ' Trace Id: "<root>"'.
