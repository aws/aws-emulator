Issue-Severity: HIGH
Issue-Commit: f1d70ff20b4a607718bee5570d5907dd84812972

# lambda-core VPC-egress connector data plane is unobservable: the published MicroVM endpoint answers HTTP 501 instead of the guest's own response, so no connector reachability can be exercised

> _Note: this issue was identified against the code base at commit `f1d70ff20b4a607718bee5570d5907dd84812972`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunMicrovm

## Description
One net-new root cause found by replaying the behavioral suite against the lambda-core + lambda-microvms emulator. CAUSE COUNT: 1.

--- Cause 1 of 1: the MicroVM proxy endpoint serves no guest response (HTTP 501), so the VPC_EGRESS network-connector data path cannot be exercised at all ---

AWS: RunMicrovmResponse$endpoint (Smithy: com.amazonaws.lambdamicrovms#LambdaMicrovms, RunMicrovm POST /2025-09-09/microvms, member `endpoint` target String @length 1..2048 @required) is documented as "The HTTPS endpoint URL for communicating with the MicroVM. Include a valid authentication token in the X-aws-proxy-auth header when sending requests." A GET to that address carrying the X-aws-proxy-auth header minted by CreateMicrovmAuthToken (Smithy: POST /2025-09-09/microvms/{microvmIdentifier}/auth-token, output authToken: TokenParts map, @required; input expirationInMinutes PositiveInteger @range min 1, allowedPorts @required) is PROXIED INTO THE MICROVM and the client receives the GUEST's own HTTP response — the model says nothing further about the proxied exchange, so this is recorded behavior only: HTTP 200 with the guest process's body (here a JSON document the guest serves on port 8080; the suite reads /health and /connect?host=<ip>&port=<port> on it). RunMicrovm's egressNetworkConnectors (Smithy: NetworkConnectorList @length max 10, "The list of egress network connectors to configure for the MicroVM") combined with a lambda-core network connector carrying a NetworkConnectorVpcEgressConfiguration ("Configuration for a VPC egress network connector. Specifies the VPC subnets, security groups, network protocol, and associated Lambda compute resource types.") is what gives that guest a path into the customer VPC, and the only way a client can observe that path is through the guest's response. There is no error code here: the recorded answer for an authenticated request to a RUNNING MicroVM is a 2xx carrying the guest's body, whatever the guest chooses to send; the MicroVM id, host, port and the private IPv4 addresses in the replay are per-environment values and are not part of the requirement.

Emulator: every authenticated GET to the published endpoint of a RUNNING MicroVM answers HTTP 501 with the body "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n" — that sentence is the emulator's own fixed text and has no counterpart in AWS. Replay of both checks below got as far as a RUNNING MicroVM, a minted auth token (CreateMicrovmAuthToken with expirationInMinutes=15, allowedPorts=[{allPorts:{}}]) and an ACTIVE VPC_EGRESS connector, and then: `AssertionError: GET '/connect?host=10.90.1.4&port=443' on microvm mvm-0000000000000001 answered HTTP 501: b"This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"` and the same 501 for host=10.91.1.4 on mvm-0000000000000000. Because no guest response is ever served, the whole connector data plane is unverifiable: the emulator never has to show that a MicroVM on an ACTIVE VPC_EGRESS connector reaches a private address in the connector's subnet, that the guest's source address on that path is link-local (169.254.0.0/16) rather than an address inside the customer subnet, that a MicroVM attached only to INTERNET_EGRESS times out on that same private address, or that revoking the target security group's ingress rule turns the reachable path back into a silent drop. egressNetworkConnectors is therefore stored on the MicroVM record and echoed back, but never enforced as a network path.

Why: a client runs a MicroVM precisely to serve and originate traffic. Against this emulator every request it proxies to its own guest comes back 501, a status AWS never returns on this path, so the client cannot read its guest's response, cannot distinguish "my guest is broken" from "the platform refuses", and cannot test any of the network isolation it bought the connector for: a workload that must only reach a private VPC endpoint through a VPC_EGRESS connector, and must NOT reach it without one, gets the identical 501 in both arms and so cannot tell an enforced path from an unenforced one.

Fix: the MicroVM endpoint listener's request handler in the lambda-microvms service state (the proxy that fronts the published `endpoint` and currently short-circuits to the NO_MICROVM_GUEST 501), together with the connector-attachment path that must make a MicroVM's egressNetworkConnectors decide whether a proxied outbound connection to an address in the connector's subnet succeeds, times out, or is dropped by the target security group. RunMicrovm's and GetMicrovm's handlers already publish the endpoint; the defect is behind it.

Related open issues, neither of which carries these two checks' replay keys: 20261103T220031Z-lambda-microvms-6-net-new-root-causes-deletemicrovmimage.md, whose Cause 4 describes the same 501 from four lambda-microvms-only checks (it does not cover the lambda-core connector-egress checks filed here, so closing it should be re-replayed against them), and 20261107T201003Z-lambda-microvms-microvm-endpoint-listener-presents-an-untrusted.md, which is about the listener's TLS material rather than its HTTP answer. 20261103T161149Z-lambda-core-lambda-microvms-vpc-egress-data-plane.md concerns the asynchronous lifecycle convergence of these flows, not the endpoint's answer.

Traces: lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet, lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group.

## Evidence
Suite: 23/29 passed (79.3%); 5 mismatched, 1 errored. Per-service: lambda-core 23/26 (88%), lambda-core__lambda-microvms 0/2, cloudtrail__lambda-core 0/1. Four of the six failures are marked OWNED by an existing open issue and are skipped; this issue is grounded in the two remaining failures, which fail identically.

[mismatch] lambda-core__lambda-microvms.microvm-denied-the-vpc-egress-connector-cannot-reach-the-private-subnet
  check_..._microvm_denied_the_vpc_egress_connector_cannot_reach_the_private_subnet.py, line 657, in _guest_connect:
    return _guest_get(ctx, mvm, microvm, f"/connect?{query}")
  line 648, in _guest_get:
    assert status == 200
  AssertionError: GET '/connect?host=10.90.1.4&port=443' on microvm mvm-0000000000000001 answered HTTP 501: b"This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"

[mismatch] lambda-core__lambda-microvms.vpc-egress-connector-path-is-blocked-by-the-target-security-group
  check_..._vpc_egress_connector_path_is_blocked_by_the_target_security_group.py, line 691, in _guest_connect -> line 683, in _guest_get:
  AssertionError: GET '/connect?host=10.91.1.4&port=443' on microvm mvm-0000000000000000 answered HTTP 501: b"This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served.\n"

  expected: an authenticated GET (X-aws-proxy-auth from CreateMicrovmAuthToken) to the RunMicrovm-published `endpoint` of a RUNNING MicroVM is proxied to the guest and returns HTTP 200 with the guest's own body — here the guest's JSON report of one outbound TCP connect, e.g. {"connected": true, "outcome": "connected", "peer": [<target address>, 443], "local": [<link-local source>, <port>]} for a MicroVM attached to the ACTIVE VPC_EGRESS connector, and {"connected": false, "outcome": "timed_out", ...} with NO "local" member for a MicroVM attached only to INTERNET_EGRESS or when the target security group admits nothing.
  actual: HTTP 501 with the emulator's own body "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served." for every path (/connect and /health alike). The fixed text is that whole sentence; nothing in it comes from the request.

  Both checks reached this point with everything upstream succeeding: CreateVpc/CreateSubnet/CreateSecurityGroup/CreateVpcEndpoint, CreateNetworkConnector settling ACTIVE, CreateMicrovmImage settling CREATED/SUCCESSFUL, RunMicrovm settling RUNNING with the expected echoed egressNetworkConnectors, and CreateMicrovmAuthToken minting a token. Only the data-plane read diverges, so the 501 is the emulator's answer and not a runner or environment failure (the per-run values 10.90.1.4, 10.91.1.4, mvm-0000000000000000/1 are not requirements).

## Steps to Reproduce
Against the emulator (lambda-core and lambda-microvms both stood up):

1. EC2: create a VPC (e.g. 10.90.0.0/16) with one subnet, a security group admitting TCP on the target port from the VPC CIDR, and one Interface VPC endpoint for com.amazonaws.<region>.lambda in that subnet with PrivateDnsEnabled=false; wait for state=available and read the single private IPv4 address of its ENI as <target_ip>.
2. IAM: create the connector operator role, then lambda-core CreateNetworkConnector(Name=..., Configuration={VpcEgressConfiguration:{SubnetIds:[<subnet>], SecurityGroupIds:[<group>], NetworkProtocol:"IPv4", AssociatedComputeResourceTypes:["MicroVm"]}}, OperatorRole=<role>); poll GetNetworkConnector until State=ACTIVE and keep its Arn.
3. lambda-microvms: CreateMicrovmImage from an artifact whose guest serves HTTP JSON on port 8080 with /health and /connect?host=&port= (the latter opens one outbound TCP connection and reports connected/outcome/peer/local); poll until image state=CREATED and version state=SUCCESSFUL.
4. RunMicrovm(imageIdentifier=<imageArn>, ingressNetworkConnectors=[...:aws-network-connector:HTTP_INGRESS], egressNetworkConnectors=[<connector Arn>]); poll GetMicrovm until state=RUNNING and read `endpoint`. Run a second MicroVM from the same image with egressNetworkConnectors=[...:aws-network-connector:INTERNET_EGRESS].
5. CreateMicrovmAuthToken(microvmIdentifier=<id>, expirationInMinutes=15, allowedPorts=[{allPorts:{}}]) for each, and GET <endpoint>/health and <endpoint>/connect?host=<target_ip>&port=443 with the returned authToken map as request headers.

Expected: HTTP 200 carrying the guest's own JSON — connected:true with peer[0]==<target_ip> and a link-local local[0] (inside 169.254.0.0/16, never inside the connector's subnet CIDR) for the connector-attached MicroVM; connected:false, outcome:"timed_out", no "local" member for the INTERNET_EGRESS-only MicroVM; and /health answering {"healthy": true} in both.
Observed: HTTP 501 "This MicroVM endpoint is not implemented: the emulator runs no MicroVM guest, so the guest's own response cannot be served." for every one of those requests.

6. Repeat with a target security group that admits nothing, then AuthorizeSecurityGroupIngress for the connector's group on the target port and RevokeSecurityGroupIngress again: AWS turns the connect from timed_out -> connected -> timed_out; the emulator answers 501 throughout, so the security group is never shown to gate the connector's path.
