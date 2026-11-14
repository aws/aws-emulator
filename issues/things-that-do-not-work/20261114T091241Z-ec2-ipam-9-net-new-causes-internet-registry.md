Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-batch-modify-ipam-routing-policy-registrations ec2.op-contract-enableIpamInternetRegistryAssociation ec2.op-contract-disassociate-ipam-byoasn ec2.op-contract-get-ipam-prefix-list-resolver-rules ec2.op-contract-get-ipam-prefix-list-resolver-version-entries ec2.op-contract-get-ipam-prefix-list-resolver-versions ec2.op-contract-get-ipam-route-origin-authorizations ec2.op-contract-get-ipam-routing-policy-registration-deltas ec2.op-contract-get-ipam-routing-policy-registrations
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-batch-modify-ipam-routing-policy-registrations|run#8|AssertionError|assert real_code == "InvalidIpamInternetRegistryAssociationId.Malformed", (
Issue-Key: ec2.op-contract-enableIpamInternetRegistryAssociation|run#9|AssertionError|assert real_code == "InvalidIpamInternetRegistryAssociationId.Malformed", (
Issue-Key: ec2.op-contract-disassociate-ipam-byoasn|run#4|AssertionError|assert real_code == "InvalidCidr.NotFound", f"expected InvalidCidr.NotFound, got {real_code}"
Issue-Key: ec2.op-contract-get-ipam-prefix-list-resolver-rules|run#7|AssertionError|assert real_code == "InvalidIpamPrefixListResolverId.NotFound", (
Issue-Key: ec2.op-contract-get-ipam-prefix-list-resolver-version-entries|run#7|AssertionError|assert real_code == "InvalidIpamPrefixListResolverId.NotFound", (
Issue-Key: ec2.op-contract-get-ipam-prefix-list-resolver-versions|run#8|AssertionError|assert status == 400, f"expected a client (400) error, got HTTP {status}"
Issue-Key: ec2.op-contract-get-ipam-route-origin-authorizations|run#7|AssertionError|assert status == 400, f"expected a client (400) error, got HTTP {status}"
Issue-Key: ec2.op-contract-get-ipam-routing-policy-registration-deltas|run#8|AssertionError|assert status == 400, f"expected a client (400) error, got HTTP {status}"
Issue-Key: ec2.op-contract-get-ipam-routing-policy-registrations|run#7|AssertionError|assert status == 400, f"expected a client (400) error, got HTTP {status}"
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# EC2 IPAM: 9 net-new causes — internet-registry and prefix-list-resolver operations answer HTTP 501 NotImplemented instead of the documented 400 client refusals

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIpamRoutingPolicyRegistrations

## Description
Nine net-new root causes, one per IPAM operation, all in the ec2 control plane. Every one of these operations answers HTTP 501 with error code NotImplemented where AWS answers HTTP 400 with a concrete, id-shaped client refusal. The EC2 Smithy model declares errors: [] for each of them (ec2Query carries error codes at the protocol level), so the requirement is AWS's documented/recorded refusal and HTTP status for the call. Each cause is a separate operation and must be closed separately.

Cause 1 of 9: BatchModifyIpamRoutingPolicyRegistrations.
AWS: a malformed IpamInternetRegistryAssociationId is refused with HTTP 400 and InvalidIpamInternetRegistryAssociationId.Malformed, the message quoting the offending id from the request.
Emulator: HTTP 501, code NotImplemented.

Cause 2 of 9: EnableIpamInternetRegistryAssociation.
AWS: HTTP 400, InvalidIpamInternetRegistryAssociationId.Malformed for a malformed association id.
Emulator: HTTP 501, code NotImplemented.

Cause 3 of 9: DisassociateIpamByoasn.
AWS: for an ASN/CIDR pair that was never provisioned, HTTP 400 and InvalidCidr.NotFound with its message naming the requested CIDR; the DryRun leg answers DryRunOperation with "Request would have succeeded, but DryRun flag is set."
Emulator: HTTP 501, code NotImplemented.

Cause 4 of 9: GetIpamPrefixListResolverRules.
AWS: a well-formed but nonexistent ipam-plr- id is refused with HTTP 400 and InvalidIpamPrefixListResolverId.NotFound naming that id.
Emulator: HTTP 501, code NotImplemented.

Cause 5 of 9: GetIpamPrefixListResolverVersionEntries.
AWS: HTTP 400, InvalidIpamPrefixListResolverId.NotFound for a well-formed but nonexistent resolver id.
Emulator: HTTP 501, code NotImplemented.

Cause 6 of 9: GetIpamPrefixListResolverVersions.
AWS: the non-DryRun call on a placeholder resolver id is a CLIENT error — HTTP 400 with the id-specific code and message.
Emulator: HTTP 501.

Cause 7 of 9: GetIpamRouteOriginAuthorizations.
AWS: HTTP 400 with InvalidIpamInternetRegistryAssociationId.Malformed and its exact message for a placeholder association id; DryRun answers DryRunOperation with the standard message.
Emulator: HTTP 501.

Cause 8 of 9: GetIpamRoutingPolicyRegistrationDeltas.
AWS: HTTP 400 with InvalidIpamInternetRegistryAssociationId.Malformed and its exact message.
Emulator: HTTP 501.

Cause 9 of 9: GetIpamRoutingPolicyRegistrations.
AWS: HTTP 400 with InvalidIpamInternetRegistryAssociationId.Malformed and its exact message. Smithy: the operation requires IpamInternetRegistryAssociationId and bounds MaxResults to 5..1000.
Emulator: HTTP 501.

Why (all nine): 501/NotImplemented is a server fault. SDK retry policies retry it and then surface a generic failure, so a caller passing a bad or unknown IPAM id never learns the id was rejected, cannot branch on NotFound vs Malformed, and burns its retry budget. Code paths that gate on "does this registration/resolver exist?" become unreachable.
Fix (all nine): each operation's handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/ — the id-shape validation and existence check must run and refuse with the EC2 error code and 400 status before any NotImplemented fallback.
Traces: one check per cause, listed in trace_ids in the same order as the causes above.

## Evidence
Cause 1 — AssertionError: expected BatchModifyIpamRoutingPolicyRegistrations to reject a malformed IpamInternetRegistryAssociationId with InvalidIpamInternetRegistryAssociationId.Malformed, got NotImplemented.
Cause 2 — AssertionError: expected InvalidIpamInternetRegistryAssociationId.Malformed, got NotImplemented.
Cause 3 — AssertionError: expected InvalidCidr.NotFound, got NotImplemented.
Cause 4 — AssertionError: expected InvalidIpamPrefixListResolverId.NotFound, got 'NotImplemented' (id ipam-plr-0000000000000000).
Cause 5 — AssertionError: expected InvalidIpamPrefixListResolverId.NotFound for a well-formed but nonexistent resolver id, got 'NotImplemented'.
Cause 6 — AssertionError: expected a client (400) error, got HTTP 501.
Cause 7 — AssertionError: expected a client (400) error, got HTTP 501 (id ipam-ira-0aaaaaaaaaaaaaaaa).
Cause 8 — AssertionError: expected a client (400) error, got HTTP 501.
Cause 9 — AssertionError: expected a client (400) error, got HTTP 501.

## Steps to Reproduce
For each operation, call it twice against the emulator: once with DryRun=true (expect DryRunOperation, "Request would have succeeded, but DryRun flag is set.") and once without DryRun using a placeholder id — ipam-ira-0aaaaaaaaaaaaaaaa for the internet-registry/routing-policy operations, ipam-plr-0000000000000000 (or ipam-prefix-list-resolver-0aaaaaaaaaaaaaaaa) for the prefix-list-resolver operations, and a never-provisioned ASN/CIDR pair for DisassociateIpamByoasn. Observe HTTP 501/NotImplemented instead of HTTP 400 with the id-specific code.
Replay the trace_ids listed.
