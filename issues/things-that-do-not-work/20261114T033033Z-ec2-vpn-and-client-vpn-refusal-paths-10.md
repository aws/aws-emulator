Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-modify-vpn-connection-options ec2.modify-vpn-connection-options-on-deleted-connection-blocked ec2.op-contract-modify-vpn-tunnel-options ec2.e2e-vpn-tunnel-option-out-of-range-rejected ec2.op-contract-modify-vpn-connection-notfound-rejected ec2.op-contract-replace-vpn-tunnel ec2.op-contract-describe-vpn-connections ec2.op-contract-deleteVpnConnectionRoute ec2.op-contract-authorizeClientVpnIngress ec2.op-contract-disassociate-client-vpn-target-network ec2.op-contract-modify-client-vpn-endpoint ec2.op-contract-terminate-client-vpn-connections
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-modify-vpn-connection-options|run#13|AssertionError|assert err == "DryRunOperation", err
Issue-Key: ec2.modify-vpn-connection-options-on-deleted-connection-blocked|run#18|AssertionError|assert code == "InvalidVpnConnectionID.NotFound", f"expected InvalidVpnConnectionID.NotFound, got {code!r}"
Issue-Key: ec2.op-contract-modify-vpn-tunnel-options|run#15|AssertionError|assert err == "DryRunOperation", err
Issue-Key: ec2.e2e-vpn-tunnel-option-out-of-range-rejected|run#18|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue for DPDTimeoutSeconds=10 (below the documented minimum of 30), got {code!r}"
Issue-Key: ec2.op-contract-modify-vpn-connection-notfound-rejected|run#16|AssertionError|assert err == "InvalidVpnConnectionId", err
Issue-Key: ec2.op-contract-replace-vpn-tunnel|run#19|AssertionError|assert code == "InvalidVpnTunnelOutsideIp.NotFound", (
Issue-Key: ec2.op-contract-describe-vpn-connections|run#16|AssertionError|assert code == "InvalidVpnConnectionID.NotFound", (
Issue-Key: ec2.op-contract-deleteVpnConnectionRoute|run#21|AssertionError|assert message == "The specified route does not exist", message
Issue-Key: ec2.op-contract-authorizeClientVpnIngress|run#20|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-disassociate-client-vpn-target-network|run#18|AssertionError|assert code == "DryRunOperation", code
Issue-Key: ec2.op-contract-modify-client-vpn-endpoint|run#16|AssertionError|assert dry_code == "DryRunOperation", f"expected DryRunOperation, got {dry_code!r}"
Issue-Key: ec2.op-contract-terminate-client-vpn-connections|run#12|AssertionError|assert message == "No connections exists for the given username sim2real-behavioral-probe", (
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# EC2 VPN and Client VPN refusal paths: 10 causes (ModifyVpnConnectionOptions/ModifyVpnTunnelOptions/ModifyVpnConnection/ReplaceVpnTunnel answer NotImplemented; wrong not-found codes and refusal messages)

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpnTunnelOptions

## Description
Ten net-new root causes in the EC2 VPN-connection and Client-VPN refusal surface. Each cause is its own defect; they are bundled because they all belong to the ec2 service. Smithy: all of these operations are modelled on com.amazonaws.ec2#AmazonEC2 with an explicit `DryRun: Boolean` input member and no modelled error shapes (ec2Query reports refusals as <Errors><Error><Code>/<Message>), so the refusal code and message are the contract a client sees.

Cause 1 of 10: ModifyVpnConnectionOptions never refuses — it answers NotImplemented.
AWS: ModifyVpnConnectionOptions (input VpnConnectionId required, LocalIpv4NetworkCidr, RemoteIpv4NetworkCidr, LocalIpv6NetworkCidr, RemoteIpv6NetworkCidr, TunnelBandwidth, DryRun) evaluates DryRun first and refuses an authorized dry run with HTTP 412 and code `DryRunOperation` (message: the fixed text "Request would have succeeded, but DryRun flag is set."), and refuses a call naming a deleted/unknown VPN connection with HTTP 400 and code `InvalidVpnConnectionID.NotFound` (message fixed text "The vpnConnection ID '<id>' does not exist", the id taken from the request).
Emulator: both paths answer error code `NotImplemented` (HTTP 501).
Why: a client that probes permissions with DryRun, or that distinguishes a deleted tunnel from a transient fault, sees an unretryable server-side 501 instead of the modelled client refusal; wrappers that branch on InvalidVpnConnectionID.NotFound treat the connection as still existing.
Fix: the ModifyVpnConnectionOptions handler and its validation (DryRun short-circuit plus VpnConnectionId existence lookup against VPN-connection state).
Traces: ec2.op-contract-modify-vpn-connection-options, ec2.modify-vpn-connection-options-on-deleted-connection-blocked.

Cause 2 of 10: ModifyVpnTunnelOptions never refuses — DryRun and out-of-range tunnel options both answer NotImplemented.
AWS: ModifyVpnTunnelOptions (VpnConnectionId, VpnTunnelOutsideIpAddress, TunnelOptions all required; DryRun present) refuses an authorized dry run with code `DryRunOperation`, and refuses a TunnelOptions value below its documented minimum — DPDTimeoutSeconds has a documented minimum of 30 — with HTTP 400 and code `InvalidParameterValue`, whose message names the member and the rejected value taken from the request, leaving the tunnel's stored DPD timeout untouched.
Emulator: both answer `NotImplemented`; nothing validates the TunnelOptions ranges.
Why: a caller can push an out-of-range tunnel configuration and gets a 501 rather than a parameter refusal, so invalid IKE/DPD settings are indistinguishable from an emulator gap and the positive control (a valid value on the same tunnel) cannot be compared against a rejection.
Fix: the ModifyVpnTunnelOptions handler and its ModifyVpnTunnelOptionsSpecification validation (range checks per member, DryRun short-circuit).
Traces: ec2.op-contract-modify-vpn-tunnel-options, ec2.e2e-vpn-tunnel-option-out-of-range-rejected.

Cause 3 of 10: ModifyVpnConnection answers NotImplemented for an unknown VPN connection.
AWS: refuses with a client error whose code is `InvalidVpnConnectionId` (per the recorded behaviour this operation's unknown-id code), message fixed text "The specified VpnConnection (<id>) for owner (<account-id>) does not exist" with the id from the request and the account id from the caller's identity.
Emulator: code `NotImplemented`.
Why: a client cannot tell a wrong id from an outage; retry logic retries a permanent failure.
Fix: the ModifyVpnConnection handler's VpnConnectionId existence check.
Traces: ec2.op-contract-modify-vpn-connection-notfound-rejected.

Cause 4 of 10: ReplaceVpnTunnel answers NotImplemented instead of refusing an unknown outside IP.
AWS: ReplaceVpnTunnel (VpnConnectionId and VpnTunnelOutsideIpAddress required, ApplyPendingMaintenance, DryRun) refuses a VpnTunnelOutsideIpAddress that is not one of the connection's tunnel outside addresses with code `InvalidVpnTunnelOutsideIp.NotFound`, the address in the message taken from the request.
Emulator: code `NotImplemented`.
Why: tunnel-rotation tooling cannot discover that it named the wrong tunnel.
Fix: the ReplaceVpnTunnel handler's tunnel-address lookup.
Traces: ec2.op-contract-replace-vpn-tunnel.

Cause 5 of 10: DescribeVpnConnections uses the wrong not-found error code.
AWS: a bogus VpnConnectionId is refused with code `InvalidVpnConnectionID.NotFound` (the read path's documented code, note the upper-case ID and the .NotFound suffix).
Emulator: code `InvalidVpnConnectionId` with message "The specified VpnConnection (<id>) for owner (<account-id>) does not exist".
Why: every SDK/CLI error handler and every waiter that keys on InvalidVpnConnectionID.NotFound falls through to a generic failure, so "connection gone" is never recognised.
Fix: the DescribeVpnConnections handler's not-found refusal (error-code mapping), not the message.
Traces: ec2.op-contract-describe-vpn-connections.

Cause 6 of 10: DeleteVpnConnectionRoute's InvalidRoute.NotFound message is emulator-authored prose.
AWS: deleting a destination CIDR that was never created on the connection is refused with code `InvalidRoute.NotFound` and the fixed message text "The specified route  does not exist" (AWS emits that fixed text, including its double space, and interpolates no request values).
Emulator: same class of refusal but the message is "The route '<cidr>' does not exist in the VPN connection '<vpn-id>'" — an emulator-authored template that interpolates the request CIDR and connection id.
Why: tests and operators that compare the refusal text against AWS's see a mismatch; the message is part of the API contract for this error class.
Fix: the DeleteVpnConnectionRoute handler's not-found refusal message.
Traces: ec2.op-contract-deleteVpnConnectionRoute.

Cause 7 of 10: AuthorizeClientVpnIngress ignores DryRun and answers NotImplemented.
AWS: with DryRun set and the caller authorized, the call is refused with code `DryRunOperation` and nothing is created.
Emulator: code `NotImplemented`. Hypothesis: the handler is a scaffold stub — the suite separately reports AuthorizeClientVpnIngress as UNIMPLEMENTED when reached from ec2.op-contract-revoke-client-vpn-ingress, and an issue naming services/service-com-amazonaws-ec2-amazonec2/src/ops/authorize_client_vpn_ingress.rs is auto-filed for that check; this cause is the same handler reached through its DryRun contract, so closing that stub likely closes this too.
Fix: the AuthorizeClientVpnIngress handler (DryRun short-circuit before any state change).
Traces: ec2.op-contract-authorizeClientVpnIngress.

Cause 8 of 10: DisassociateClientVpnTargetNetwork ignores DryRun and answers NotImplemented.
AWS: refuses an authorized dry run with code `DryRunOperation`, leaving the association in place.
Emulator: code `NotImplemented`.
Why: a client cannot pre-flight the call; the 501 is classified as a server fault.
Fix: the DisassociateClientVpnTargetNetwork handler (DryRun short-circuit plus AssociationId lookup).
Traces: ec2.op-contract-disassociate-client-vpn-target-network.

Cause 9 of 10: ModifyClientVpnEndpoint ignores DryRun and answers NotImplemented.
AWS: refuses an authorized dry run with code `DryRunOperation` and does not modify the endpoint.
Emulator: code `NotImplemented`.
Why: same as above — permission pre-flight is impossible and the status class is wrong (501 vs 412).
Fix: the ModifyClientVpnEndpoint handler's DryRun short-circuit.
Traces: ec2.op-contract-modify-client-vpn-endpoint.

Cause 10 of 10: TerminateClientVpnConnections' user-not-found message is emulator-authored prose.
AWS: terminating by Username when no connection exists for that user is refused with code `InvalidClientVpnConnection.UserNotFound` and the message "No connections exists for the given username <username>" — fixed text plus the Username taken from the request.
Emulator: the code is right but the message is "User <username> does not exist".
Why: the message is the only place AWS states that the *endpoint* has no connection for that user rather than that the user is unknown; clients surfacing the text mislead operators.
Fix: the TerminateClientVpnConnections handler's refusal message for the Username path.
Traces: ec2.op-contract-terminate-client-vpn-connections.

## Evidence
Replayed behavioral suite, service heading `### ec2`:
- ec2.op-contract-modify-vpn-connection-options — AssertionError: err == "DryRunOperation" → NotImplemented
- ec2.modify-vpn-connection-options-on-deleted-connection-blocked — AssertionError: expected InvalidVpnConnectionID.NotFound, got 'NotImplemented'
- ec2.op-contract-modify-vpn-tunnel-options — AssertionError: err == "DryRunOperation" → NotImplemented
- ec2.e2e-vpn-tunnel-option-out-of-range-rejected — AssertionError: expected InvalidParameterValue for DPDTimeoutSeconds=10 (below the documented minimum of 30), got 'NotImplemented'
- ec2.op-contract-modify-vpn-connection-notfound-rejected — AssertionError: err == "InvalidVpnConnectionId" → NotImplemented
- ec2.op-contract-replace-vpn-tunnel — AssertionError: expected InvalidVpnTunnelOutsideIp.NotFound, got NotImplemented
- ec2.op-contract-describe-vpn-connections — AssertionError: expected InvalidVpnConnectionID.NotFound for a bogus VpnConnectionId, got 'InvalidVpnConnectionId' ({'Code': 'InvalidVpnConnectionId', 'Message': 'The specified VpnConnection (vpn-...) for owner (...) does not exist'})
- ec2.op-contract-deleteVpnConnectionRoute — expected code InvalidRoute.NotFound with message "The specified route  does not exist"; got message "The route '172.31.9.0/24' does not exist in the VPN connection 'vpn-...'"
- ec2.op-contract-authorizeClientVpnIngress — AssertionError: expected DryRunOperation, got NotImplemented
- ec2.op-contract-disassociate-client-vpn-target-network — AssertionError: code == "DryRunOperation" → NotImplemented
- ec2.op-contract-modify-client-vpn-endpoint — AssertionError: expected DryRunOperation, got 'NotImplemented'
- ec2.op-contract-terminate-client-vpn-connections — AssertionError: unexpected InvalidClientVpnConnection.UserNotFound message 'User <username> does not exist'; expected "No connections exists for the given username <username>"

Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): ModifyVpnConnectionOptions, ModifyVpnTunnelOptions, ReplaceVpnTunnel, AuthorizeClientVpnIngress, DisassociateClientVpnTargetNetwork, TerminateClientVpnConnections and DescribeVpnConnections each carry a `DryRun: Boolean` input member; ModifyVpnTunnelOptions requires VpnConnectionId + VpnTunnelOutsideIpAddress + TunnelOptions (ModifyVpnTunnelOptionsSpecification); DeleteVpnConnectionRoute requires VpnConnectionId + DestinationCidrBlock and returns Unit. No error shapes are modelled for these ops (ec2Query refusals), so code and message are the whole observable contract.

## Steps to Reproduce
Re-replay the listed check ids against the emulator (behavioral suite simparity-behavioral-aws-v1). Per cause, briefly: (1) create a customer gateway + VPN gateway + VPN connection, then call ModifyVpnConnectionOptions with DryRun=true and again after DeleteVpnConnection; (2) call ModifyVpnTunnelOptions with DryRun=true, then with TunnelOptions={'DPDTimeoutSeconds': 10}; (3) ModifyVpnConnection with VpnConnectionId=vpn-00000000000000000; (4) ReplaceVpnTunnel with a VpnTunnelOutsideIpAddress not on the connection; (5) DescribeVpnConnections(VpnConnectionIds=['vpn-00000000000000000']); (6) CreateVpnConnectionRoute then DeleteVpnConnectionRoute for a CIDR never created; (7)-(9) AuthorizeClientVpnIngress / DisassociateClientVpnTargetNetwork / ModifyClientVpnEndpoint with DryRun=true on a Client VPN endpoint; (10) TerminateClientVpnConnections(Username=<a user with no connection>). Compare the returned HTTP status, error Code and Message with the AWS values quoted above.
