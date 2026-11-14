Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.create-firewall-rule-override-without-block-override-rejected route53resolver.delete-firewall-domain-list-conflict-when-referenced route53resolver.delete-firewall-rule-group-active-association-rejected route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver.create-firewall-rule-override-without-block-override-rejected|run#24|AssertionError|assert ERR_CODE_OVERRIDE in message, (
Issue-Key: route53resolver.delete-firewall-domain-list-conflict-when-referenced|run#24|AssertionError|assert deleted_message == gone_message, (
Issue-Key: route53resolver.delete-firewall-rule-group-active-association-rejected|run#20|AssertionError|assert deleted_message == expected, f"expected message {expected!r}, got {deleted_message!r}"
Issue-Key: route53resolver|GetFirewallRuleGroup|ResourceNotFoundException
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# Route 53 Resolver DNS Firewall refusals: RSLVR-02025 not-found Messages still omit the trailing `Trace Id: "<Root>"` clause, and a partially-specified OVERRIDE rule is refused with RSLVR-02016 instead of RSLVR-02013 (2 causes)

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallRule

## Description
Two net-new root causes in the Route 53 Resolver DNS Firewall control plane. Both are refusal-message defects: the status (HTTP 400) and the exception Code are right in every case, the Message text is not.

=== Cause 1 of 2: every RSLVR-coded ResourceNotFoundException Message ends at the sentence period — the trailing ` Trace Id: "<Root>"` clause is missing (DeleteFirewallDomainList / GetFirewallDomainList and DeleteFirewallRuleGroup / GetFirewallRuleGroup not-found paths) ===

Failing check ids: route53resolver.delete-firewall-domain-list-conflict-when-referenced, route53resolver.delete-firewall-rule-group-active-association-rejected, route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow

AWS: Smithy models no message text for this class — ResourceNotFoundException{Message: String, ResourceType: String}, errorKind client, httpError 400, modeled on DeleteFirewallDomainList, GetFirewallDomainList, DeleteFirewallRuleGroup and GetFirewallRuleGroup (Smithy: DeleteFirewallDomainList(FirewallDomainListId: ResourceId required, len 1..64) errors AccessDeniedException | ConflictException | InternalServiceErrorException | ResourceNotFoundException | ThrottlingException; GetFirewallRuleGroup the same minus ConflictException). So the wording is recorded behavior only. The recorded Route 53 Resolver response for a DNS Firewall resource id that does not exist is HTTP 400, Error.Code `ResourceNotFoundException`, Error.Message of the form:

    [RSLVR-02025] Can’t find the resource with ID "<resource id>". Trace Id: "<Root>"

Message anatomy. FIXED text: `[RSLVR-02025] Can’t find the resource with ID "` … `". Trace Id: "` … `"` — note the service's U+2019 right single quote in `Can’t`, the double quotes around both values, and the period that closes the sentence BEFORE the Trace Id clause. VARIABLE parts: (a) the resource id taken verbatim from the request (`FirewallDomainListId` or `FirewallRuleGroupId`), and (b) the X-Ray trace Root, which is NOT a per-run value the service invents: it is read from the caller's `X-Amzn-Trace-Id` request header (`Root=<root>;Sampled=0`), which is exactly why these checks can pin the whole message byte-exactly (each registers a before-sign hook that sends one fixed Root on every route53resolver call). Only when the caller sends no such header does the service mint a Root itself; it still emits the clause. The e2e check does not pin a Root at all — it asserts only that the clause exists and that its Root matches `"1-[0-9a-f]{8}-[0-9a-f]{24}"` — so the shape of the clause, not any one Root value, is the requirement.

Emulator: the refusal is correct except for the missing final clause. On GetFirewallDomainList/DeleteFirewallDomainList after a successful delete it returns HTTP 400, Code `ResourceNotFoundException`, Message `[RSLVR-02025] Can’t find the resource with ID "<FirewallDomainListId>".`; on GetFirewallRuleGroup after a successful delete, `[RSLVR-02025] Can’t find the resource with ID "<FirewallRuleGroupId>".` The ` Trace Id: "..."` clause is never present, on any Resolver message, because the message is built by one shared formatter that has no trace-id parameter and because no handler reads the request's `X-Amzn-Trace-Id` header (the DNS Firewall handlers open their bodies with `let _ = ctx;`).

Why: the trace Root is the key a caller uses to correlate a Resolver refusal with its X-Ray trace and with a support case; against the emulator that key is simply absent from the message, so error-triage tooling that extracts it silently loses it, and any negative test, log matcher or SDK wrapper written against AWS's exact message (which ends in `Trace Id: "..."`) fails here while a test written against the emulator's short message fails in production. These three checks abort at their FIRST message assertion, so the contracts behind them are never exercised at all: the in-use domain-list ConflictException, the rule-group in-use ConflictException and the post-disassociation delete-succeeds path, and the whole e2e blocklist teardown observation.

NOTE — related open issues, neither of which covers these checks: 20261028T035605Z-route53resolver-route-53-resolver-error-messages-omit-the-trailing.md describes the same formatter defect but carries only the replay key route53resolver.delete-firewall-rule-group-with-vpc-association-conflict; 20261102T105111Z-route53resolver-route-53-resolver-refusal-messages-still-omit-the.md carries AssociateFirewallRuleGroup (RSLVR-02303) and GetFirewallRuleGroupPolicy; 20261029T234450Z-route53resolver-route-53-resolver-not-found-refusal-messages-fail.md covers a different set of not-found operations. The three checks listed above are not keyed by any of them and must be re-replayed to close this.

Fix: the shared refusal-message formatter and the plumbing of the request's trace Root into it — not the individual operations. `rslvr_message(code, sentence)` in services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs returns the sentence only; `resource_not_found_message`, `firewall_rule_group_not_found` and the firewall-domain-list equivalent build on it. The Root must be threaded from the handler's `OperationContext` request parts (`X-Amzn-Trace-Id`, with a service-minted Root as fallback) into that formatter and appended as ` Trace Id: "<Root>"` after the sentence-ending period for every Resolver exception body that carries a Message, and the DNS Firewall handlers that today discard `ctx` must pass it in.

=== Cause 2 of 2: CreateFirewallRule with BlockResponse=OVERRIDE and BlockOverrideDomain omitted is refused with the wrong RSLVR code and invented wording ===

Failing check id: route53resolver.create-firewall-rule-override-without-block-override-rejected

AWS: the conditional requirement is documented on the operation, not expressible in Smithy — BlockOverrideDomain, BlockOverrideDnsType and BlockOverrideTtl are each documented as "required if the BlockResponse setting is OVERRIDE" (Smithy: CreateFirewallRule.BlockOverrideDomain, BlockOverrideDnsType, BlockOverrideTtl are all optional members; errors AccessDeniedException | InternalServiceErrorException | LimitExceededException | ResourceNotFoundException | ThrottlingException | ValidationException, all httpError 400). The recorded refusal for a partially-specified OVERRIDE rule is HTTP 400, Error.Code `ValidationException`, Error.Message:

    [RSLVR-02013] You must specify an override DNS type, an override domain, and an override Ttl when you are using OVERRIDE.

Message anatomy: this message is entirely FIXED text — there are no request- or state-derived values in it. The `[RSLVR-02013]` code is what pins the refusal to THIS conditional requirement rather than to any other validation failure, and the message names the BlockResponse value (`OVERRIDE`) whose requirement was not met. (In real Resolver this sentence, like every RSLVR message, is followed by the ` Trace Id: "<Root>"` clause of Cause 1; this check strips that clause before comparing, so Cause 2 is independent of Cause 1.)

Emulator: it does refuse, with the right status and the right Code (`ValidationException`), but the Message is `[RSLVR-02016] Your block option BLOCK OVERRIDE DOMAIN is not valid.` — the wrong RSLVR code (02016, Resolver's code for an invalid block option value, instead of 02013), and wording the service never emits: it reports the omitted member as an invalid *block option* value in a shouty upper-case spelling (`BLOCK OVERRIDE DOMAIN`) rather than stating the three-member conditional requirement. The emulator has classified a missing conditionally-required member as a bad enum/option value.

Why: a client cannot distinguish "you omitted the override fields" from "the block option you gave is not a valid one" — the RSLVR code is precisely the discriminator SDK wrappers, retry logic and operator runbooks key on, and both codes arrive as the same ValidationException, so the only machine-readable signal is wrong. Code that branches on RSLVR-02013 to prompt for the three override fields never fires against the emulator, and a test written against the emulator's RSLVR-02016 message fails in production.

Fix: the CreateFirewallRule validation in the store — `crate::state::ComAmazonawsRoute53resolverRoute53resolverState::create_firewall_rule` in services/service-com-amazonaws-route53resolver-route53resolver/src/state.rs, which produces `FirewallRuleCreateRefusal::Validation` (services/service-com-amazonaws-route53resolver-route53resolver/src/ops/create_firewall_rule.rs:214-228 maps it to ValidationException). The OVERRIDE completeness check must be a distinct rule from the block-option value check, raise RSLVR-02013, and use the modeled member-set wording; it must also cover BlockOverrideDnsType and BlockOverrideTtl omitted, not only BlockOverrideDomain. The same refusal must leave nothing stored: the check also asserts the rejected rule does not appear in ListFirewallRules and that the surviving control rules are untouched.

Traces: see the per-cause failing check ids above; all four are listed in trace_ids.

## Evidence
Cause 1 — three checks, same assertion, same shape.

route53resolver.delete-firewall-domain-list-conflict-when-referenced [mismatch]
  File "check_route53resolver_delete_firewall_domain_list_conflict_when_referenced.py", line 136, in run
    assert deleted_message == gone_message, (
  AssertionError: expected message '[RSLVR-02025] Can’t find the resource with ID "rslvr-fdl-910a2dec89025cc1". Trace Id: "1-6af7c539-609f12c15023464a806a7558"', got '[RSLVR-02025] Can’t find the resource with ID "rslvr-fdl-910a2dec89025cc1".'
  (the id and the Root are this run's values filling the template; the check pins the Root itself via `_traced(client)`, which registers a before-sign hook setting a fixed `X-Amzn-Trace-Id: Root=<root>;Sampled=0` — its docstring: "Route 53 Resolver ends a DNS Firewall refusal message with the caller's trace Root, so a fixed Root makes the whole message exact.")

route53resolver.delete-firewall-rule-group-active-association-rejected [mismatch]
  File "check_route53resolver_delete_firewall_rule_group_active_association_rejected.py", line 81, in run
    assert deleted_message == expected, f"expected message {expected!r}, got {deleted_message!r}"
  AssertionError: expected message '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-910a2dec89025cc1". Trace Id: "1-6af7c3e0-9bc38a2d1e1a47539c4ac8f5"', got '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-910a2dec89025cc1".'

route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow [mismatch]
  File "check_route53resolver_e2e_firewall_domain_blocklist_lifecycle_flow.py", line 301, in _assert_not_found_shape
    assert re.fullmatch(r'"1-[0-9a-f]{8}-[0-9a-f]{24}"', trace), f"{what}: unexpected trace id in {message!r}"
  AssertionError: GetFirewallRuleGroup after delete: unexpected trace id in '[RSLVR-02025] Can’t find the resource with ID "rslvr-frg-975835de1c9756ce".'
  (this one asserts only the PRESENCE and shape of the clause — no pinned Root — so the requirement is the clause, not any value)

Expected (AWS) in all three: HTTP 400, Error.Code = "ResourceNotFoundException", Error.Message = '[RSLVR-02025] Can’t find the resource with ID "<id from the request>". Trace Id: "<Root>"'.
Actual (emulator): HTTP 400, Error.Code = "ResourceNotFoundException" (correct), Message = the same sentence with the ` Trace Id: "..."` clause absent.

Model: ResourceNotFoundException{Message: String, ResourceType: String}, httpError 400 (defaulted), modeled on DeleteFirewallDomainList, GetFirewallDomainList, DeleteFirewallRuleGroup, GetFirewallRuleGroup — no message text modeled, so the wording is recorded behavior only.

Cause 2 — route53resolver.create-firewall-rule-override-without-block-override-rejected [mismatch]

  File "check_route53resolver_create_firewall_rule_override_without_block_override_rejected.py", line 154, in run
    assert ERR_CODE_OVERRIDE in message, (
  AssertionError: expected Resolver's own RSLVR-02013 code for a partially-specified OVERRIDE rule, got '[RSLVR-02016] Your block option BLOCK OVERRIDE DOMAIN is not valid.'

The request that was refused (everything else valid — only BlockOverrideDomain omitted):
  create_firewall_rule(CreatorRequestId=<unique>, FirewallRuleGroupId=<group>, FirewallDomainListId=<list>, Priority=620, Action="BLOCK", BlockResponse="OVERRIDE", BlockOverrideDnsType="CNAME", BlockOverrideTtl=60, Name=<unique>)

Expected (AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = '[RSLVR-02013] You must specify an override DNS type, an override domain, and an override Ttl when you are using OVERRIDE.' (all fixed text; the check strips the trailing ' Trace Id: "..."' clause before comparing, via `_without_trace_id`).
Actual (emulator): HTTP 400, Error.Code = "ValidationException" (correct), Error.Message = '[RSLVR-02016] Your block option BLOCK OVERRIDE DOMAIN is not valid.' — wrong RSLVR code, wrong error class in prose (invalid block option value rather than the three-member conditional requirement), and a member name rendered as upper-case words.

The check's positive controls all PASS against the emulator before this point, so the defect is isolated to the refusal: a fully-specified OVERRIDE rule is accepted and projects BlockResponse=OVERRIDE, BlockOverrideDnsType=CNAME, BlockOverrideTtl=60 and the canonicalized BlockOverrideDomain 'override.example.com.' (idempotently — the already-rooted form does not grow a second dot).

Model: CreateFirewallRule.BlockOverrideDomain (len 1..255), BlockOverrideDnsType (enum: CNAME), BlockOverrideTtl (range 0..604800) are all OPTIONAL members, each documented "This setting is required if the BlockResponse setting is OVERRIDE" — a conditional requirement Smithy cannot express, so it must be enforced by the handler's validation. ValidationException httpError 400 is modeled on the operation.

## Steps to Reproduce
Cause 1 (missing ` Trace Id: "<Root>"` clause on RSLVR-02025 not-found):
1. Create a boto3 route53resolver client and register a before-sign hook that sets a FIXED header on every request: `X-Amzn-Trace-Id: Root=1-<8 hex>-<24 hex>;Sampled=0`. Keep that Root.
2. dl = route53resolver.create_firewall_domain_list(CreatorRequestId=<unique>, Name=<unique>)["FirewallDomainList"]["Id"]; then route53resolver.delete_firewall_domain_list(FirewallDomainListId=dl) — succeeds.
3. route53resolver.get_firewall_domain_list(FirewallDomainListId=dl) (or delete it again) — refused HTTP 400 / ResourceNotFoundException (correct). Inspect Error.Message: the emulator returns '[RSLVR-02025] Can’t find the resource with ID "<dl>".' where AWS returns that sentence followed by ' Trace Id: "<the Root sent in step 1>"'.
4. Same on the rule-group path: grp = create_firewall_rule_group(...)["FirewallRuleGroup"]["Id"]; delete_firewall_rule_group(FirewallRuleGroupId=grp); then get_firewall_rule_group(FirewallRuleGroupId=grp) — same sentence, same missing clause with "rslvr-frg-...".
5. Also reproducible with NO X-Amzn-Trace-Id header: AWS still ends the message with ' Trace Id: "<service-minted Root>"' matching `1-[0-9a-f]{8}-[0-9a-f]{24}`; the emulator never emits the clause.

Cause 2 (CreateFirewallRule partially-specified OVERRIDE):
1. grp = route53resolver.create_firewall_rule_group(CreatorRequestId=<unique>, Name=<unique>)["FirewallRuleGroup"]["Id"].
2. dl = route53resolver.create_firewall_domain_list(CreatorRequestId=<unique>, Name=<unique>)["FirewallDomainList"]["Id"].
3. Control (passes today): create_firewall_rule(CreatorRequestId=<unique>, FirewallRuleGroupId=grp, FirewallDomainListId=dl, Priority=610, Action="BLOCK", BlockResponse="OVERRIDE", BlockOverrideDomain="override.example.com", BlockOverrideDnsType="CNAME", BlockOverrideTtl=60, Name=<unique>) — accepted, BlockOverrideDomain comes back as "override.example.com.".
4. dl2 = create_firewall_domain_list(...) ; then the same call against dl2 with Priority=620 and BlockOverrideDomain OMITTED (BlockResponse="OVERRIDE", BlockOverrideDnsType="CNAME", BlockOverrideTtl=60 all still present).
5. Observe the ClientError: Code is ValidationException (correct) but Message is '[RSLVR-02016] Your block option BLOCK OVERRIDE DOMAIN is not valid.' AWS returns '[RSLVR-02013] You must specify an override DNS type, an override domain, and an override Ttl when you are using OVERRIDE.' (plus the Trace Id clause of Cause 1).
6. Repeat step 4 omitting BlockOverrideDnsType only, and again omitting BlockOverrideTtl only — AWS answers the identical RSLVR-02013 message for each.
7. list_firewall_rules(FirewallRuleGroupId=grp) must still hold only the accepted control rules — the rejected rule must not be stored.

Replay: run_behavioral_suite and re-run route53resolver.create-firewall-rule-override-without-block-override-rejected, route53resolver.delete-firewall-domain-list-conflict-when-referenced, route53resolver.delete-firewall-rule-group-active-association-rejected, route53resolver.e2e-firewall-domain-blocklist-lifecycle-flow.
