Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-firewall-identifier-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-firewall-identifier-mistakes network-firewall-nfwr-traj-list-filter-and-paging-mistakes network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes network-firewall-nfwr-traj-stateless-rule-priority-mistakes
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|DescribeFirewall|4xx->4xx|$.Message
Issue-Key: network-firewall|DescribeFirewall|4xx->4xx|$.__type
Issue-Key: network-firewall|ListFirewalls|4xx->4xx|$.message
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.message
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->4xx|$.message
Issue-Commit: c27f8675843924c63f0ee6aedd1be01f08149ed1

# Network Firewall request-validation errors use the wrong exception class and wrong message text

> _Note: this issue was identified against the code base at commit `c27f8675843924c63f0ee6aedd1be01f08149ed1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFirewall

## Description
Network Firewall rejects malformed identifiers and malformed members with a 400, but the emulator renders those rejections differently from AWS: the wrong error code, an error body with no Message member at all, or a constraint-violation message whose text does not match AWS's. Five independent causes, all in the request-validation/error-serialization path of this service, all still open after the already-filed issues above (none of the open network-firewall issues - CreateRuleGroup misses rule-group type, capacity and priority validation; CreateFirewallPolicy does not validate StatefulDefaultActions; Describe responses omit the empty Tags list - covers any of them).

Cause 1 of 5: DescribeFirewall with a FirewallArn that is not a well-formed ARN.
AWS: Smithy: DescribeFirewall.FirewallArn targets ResourceArn (@length 1..256, @pattern "^arn:aws"); DescribeFirewall models InvalidRequestException (client, HTTP 400) whose only member is Message. The recorded response is HTTP 400, __type com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException, and Message is the fixed text "Resource ARN has an invalid format." - the whole message is fixed text, with no values interpolated from the request.
Emulator: returns HTTP 400 with the matching __type but NO Message member at all ($.Message missing).
Why: a client that surfaces or branches on the exception's Message gets null/empty, so the caller cannot tell a malformed ARN from any other InvalidRequestException and error reporting shows an empty reason.
Fix: the DescribeFirewall handler's identifier validation / the InvalidRequestException serializer for this service - the exception must be populated with its Message member.
Traces: network-firewall-nfwr-traj-firewall-identifier-mistakes#task#2.

Cause 2 of 5: DescribeFirewall with a FirewallName that violates the resource-name format.
AWS: Smithy: DescribeFirewall.FirewallName targets ResourceName (@length 1..128, @pattern "^[a-zA-Z0-9-]+$"). AWS does NOT report this as a generic framework validation error: the recorded response is HTTP 400, __type com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException with Message "Resource name has an invalid format" (entirely fixed text, no request values interpolated).
Emulator: HTTP 400 with __type "ValidationException" and no Message member - i.e. it falls through to the generic model-constraint validator instead of the service's own identifier check.
Why: the error code is part of the contract; a client catching InvalidRequestException (the only client error DescribeFirewall models for a bad name) does not catch ValidationException, so a malformed-name call raises an unmodeled error and escapes the caller's handling.
Fix: DescribeFirewall's input validation - name-format rejection must raise InvalidRequestException with that message rather than the generic ValidationException path.
Traces: network-firewall-nfwr-traj-firewall-identifier-mistakes#task#3.

Cause 3 of 5: ListFirewalls validation message quotes the raw regex including its leading anchor.
AWS: Smithy: ListFirewalls.NextToken targets PaginationToken (@length 1..4096, @pattern "^[0-9A-Za-z:\\/+=]+$"). Recorded behavior for a framework constraint violation is HTTP 400 and the message template "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where the fixed text is everything but the member path and the pattern; AWS emits the pattern WITHOUT the leading "^" anchor, i.e. "[0-9A-Za-z:\\/+=]+$". The member path ('nextToken') comes from the request.
Emulator: emits the same template but with the model's pattern verbatim, "^[0-9A-Za-z:\\/+=]+$", so the message text diverges by the leading anchor.
Why: callers and test suites that assert on, log or parse the validation message see text AWS never produces; the pattern a user is shown is not the one AWS reports.
Fix: the shared constraint-violation message builder for this service's pattern violations (strip the leading "^" anchor when rendering the pattern).
Traces: network-firewall-nfwr-traj-list-filter-and-paging-mistakes#task#4.

Cause 4 of 5: CreateRuleGroup enum-violation message lists the allowed values in the wrong order.
AWS: Smithy: CreateRuleGroup.RuleGroup.RulesSource.RulesSourceList.GeneratedRulesType targets the GeneratedRulesType enum. Recorded behavior: HTTP 400 with "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [ALLOWLIST, DENYLIST, ALERTLIST, REJECTLIST]" - the value set is fixed text in AWS's own order, which is NOT the model's declaration order ([ALLOWLIST, DENYLIST, REJECTLIST, ALERTLIST]); only the member path comes from the request.
Emulator: renders the set in model declaration order, "[ALLOWLIST, DENYLIST, REJECTLIST, ALERTLIST]", so the message diverges.
Why: the enumerated set is part of the message a client shows the user and that suites assert on; the emulator's ordering is text AWS never emits.
Fix: the enum constraint-violation message rendering for this service (the value-set ordering must match AWS's, not the model's member order).
Traces: network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes#task#6.

Cause 5 of 5: CreateFirewallPolicy reports an out-of-range stateless rule-group priority as a missing member.
AWS: Smithy: StatelessRuleGroupReference.Priority is @required and targets Priority (integer, @range min 1 max 65535). A priority below the minimum is a RANGE violation: recorded HTTP 400 with "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must have value greater than or equal to 1", the member path ('firewallPolicy.statelessRuleGroupReferences.<index>.member.priority') coming from the request and the limit (1) from the model's range.
Emulator: returns HTTP 400 but with "... failed to satisfy constraint: Member must not be null" - it treats the supplied below-minimum integer as an absent member (Hypothesis: a zero/falsy value is read as unset before the range check runs), so it reports a required-member violation instead of the range violation.
Why: the caller is told the priority was omitted when it was in fact supplied and merely out of range, which points the fix at the wrong field and hides the real constraint (minimum 1).
Fix: CreateFirewallPolicy's input validation - presence detection for required integer members must distinguish a supplied 0 from an absent member so the @range check produces the greater-than-or-equal message.
Traces: network-firewall-nfwr-traj-stateless-rule-priority-mistakes#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (15 fixtures, 119 steps) against the emulator built from this checkout.

Cause 1 - network-firewall-nfwr-traj-firewall-identifier-mistakes#task#2, DescribeFirewall: expected 400 / InvalidRequestException with $.Message 'Resource ARN has an invalid format.'; actual 400 with $.Message missing (None).

Cause 2 - network-firewall-nfwr-traj-firewall-identifier-mistakes#task#3, DescribeFirewall: expected $.__type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException' and $.Message 'Resource name has an invalid format'; actual $.__type 'ValidationException', $.Message missing.

Cause 3 - network-firewall-nfwr-traj-list-filter-and-paging-mistakes#task#4, ListFirewalls: expected $.message "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z:\\/+=]+$"; actual the same text with the pattern rendered as "^[0-9A-Za-z:\\/+=]+$".

Cause 4 - network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes#task#6, CreateRuleGroup: expected $.message "...enum value set: [ALLOWLIST, DENYLIST, ALERTLIST, REJECTLIST]"; actual "...enum value set: [ALLOWLIST, DENYLIST, REJECTLIST, ALERTLIST]".

Cause 5 - network-firewall-nfwr-traj-stateless-rule-priority-mistakes#task#5, CreateFirewallPolicy: expected $.message "1 validation error detected: Value at 'firewallPolicy.statelessRuleGroupReferences.1.member.priority' failed to satisfy constraint: Member must have value greater than or equal to 1"; actual the same path with "Member must not be null".

All five steps returned HTTP 400 as expected except where noted; the divergence is in the error code and/or the message body only.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a Network Firewall client at it.
2. Cause 1: DescribeFirewall with FirewallArn set to a string that is not a well-formed Network Firewall ARN -> expect InvalidRequestException with Message 'Resource ARN has an invalid format.'; observe an InvalidRequestException body with no Message.
3. Cause 2: DescribeFirewall with FirewallName containing a character outside ^[a-zA-Z0-9-]+$ -> expect InvalidRequestException with Message 'Resource name has an invalid format'; observe __type ValidationException and no Message.
4. Cause 3: ListFirewalls with NextToken containing a character outside the PaginationToken pattern -> compare the rendered pattern in the validation message (AWS omits the leading ^ anchor).
5. Cause 4: CreateRuleGroup with RuleGroup.RulesSource.RulesSourceList.GeneratedRulesType set to an unknown value -> compare the enum value-set ordering in the validation message.
6. Cause 5: CreateFirewallPolicy with a StatelessRuleGroupReferences entry whose Priority is 0 -> expect 'Member must have value greater than or equal to 1'; observe 'Member must not be null'.
7. Replay the fixtures listed under Traces to confirm all five causes.
