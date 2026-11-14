Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asvpce-streaming-vpce-round-trips-interface-shape ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume ec2.op-contract-acceptVpcEndpointConnections ec2.tgwc-attachments-associate-propagate-and-route-both-vpcs ec2.tgwc-unpropagated-route-table-blocked-from-return-route
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asvpce-streaming-vpce-round-trips-interface-shape|run#14|KeyError|assert got["RouteTableIds"] == [], f"an Interface endpoint has no route tables; got {got['RouteTableIds']!r}"
Issue-Key: ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not|_agent#3|PublishedUrlRejected|sock = socket.create_connection(ctx.endpoint_host(ip, _AGENT_PORT), timeout=_CONNECT_TIMEOUT_S)
Issue-Key: ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume|_poll#26|Timeout|time.sleep(interval_s)
Issue-Key: ec2.op-contract-acceptVpcEndpointConnections|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: ec2.tgwc-attachments-associate-propagate-and-route-both-vpcs|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: ec2.tgwc-unpropagated-route-table-blocked-from-return-route|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# EC2: 5 net-new causes — DescribeVpcEndpoints omits RouteTableIds for an Interface endpoint, a resumed instance is published on a foreign address, and hibernation / VPC-endpoint-connection / transit-gateway scenarios never converge

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpoints

## Description
Five net-new root causes in the ec2 surface. Each must be closed separately.

Cause 1 of 5: DescribeVpcEndpoints omits the RouteTableIds member for an Interface endpoint.
AWS: every VpcEndpoint in DescribeVpcEndpoints carries its RouteTableIds list; for a VpcEndpointType=Interface endpoint the list is present and empty (route tables belong to Gateway endpoints). Smithy: DescribeVpcEndpoints returns VpcEndpoints (xmlName vpcEndpointSet) whose member shape declares RouteTableIds; the model marks no member as conditionally absent.
Emulator: the endpoint object has no RouteTableIds key at all, so an SDK read of response["VpcEndpoints"][0]["RouteTableIds"] raises KeyError. Every other asserted member of the same endpoint (type, service name, VPC, SubnetIds, Groups, PrivateDnsEnabled, tags) round-trips.
Why: a client that reads RouteTableIds unconditionally — to decide whether an endpoint is attached to any route table, or to diff endpoint shape between Gateway and Interface — crashes instead of seeing an empty list.
Fix: the DescribeVpcEndpoints serializer/state for interface endpoints — emit the (empty) RouteTableIds member.
Traces: ec2.asvpce-streaming-vpce-round-trips-interface-shape.

Cause 2 of 5: a resumed/restarted instance is announced on an address the emulator is not reached on.
AWS: an instance's reachable address is the address the caller can connect to; the API and the data plane agree on it.
Emulator: the check was handed 54.0.0.2:8080 as the instance endpoint host — an address that is not one of the emulator's own hosts — and the harness refused the egress. The emulator announced an address other than the one it is reached on, so the data-plane leg of the scenario cannot run at all.
Why: a client that connects to the address the API reports (the public IPv4/DNS of the instance) cannot reach the workload; every data-plane assertion built on the reported address fails.
Fix: the address allocation/serialization that fills an instance's public address — it must publish an own host of the emulator rather than a fabricated public range.
Traces: ec2.hibernate-restores-the-running-process-and-a-plain-stop-does-not.

Cause 3 of 5: StopInstances(Hibernate=true) / StartInstances never completes a hibernate-resume cycle.
AWS: an instance launched with HibernationOptions.Configured=true and stopped with Hibernate=true writes its memory image to the root volume, reaches stopped, and on StartInstances resumes with its processes and in-memory state intact; the transitions are observable within the usual stop/start times.
Emulator: the scenario never reaches the state it waits for and the check exhausts its 60 s budget (Error: Check timed out after 60.0s). Hypothesis: the hibernate stop/start transition is never completed, so the poll for the resumed guest never succeeds.
Why: a client exercising hibernation hangs instead of observing stopped-then-running; the feature is unusable.
Fix: the StopInstances(Hibernate)/StartInstances state machine for hibernation-configured instances.
Traces: ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume.

Cause 4 of 5: the AcceptVpcEndpointConnections scenario never converges.
AWS: an endpoint created against a service with acceptance required appears in DescribeVpcEndpointConnections as pendingAcceptance within seconds, AcceptVpcEndpointConnections moves it to available, and an unowned/malformed service id is refused immediately.
Emulator: the check exhausts its 60 s budget (Error: Check timed out after 60.0s) before the connection state it polls for appears. Hypothesis: the endpoint connection is never recorded against the service, so no state transition is ever observable.
Why: a private-link flow that waits for pendingAcceptance and then accepts never proceeds.
Fix: the VPC-endpoint connection state created by CreateVpcEndpoint for an acceptance-required service and read by DescribeVpcEndpointConnections / AcceptVpcEndpointConnections.
Traces: ec2.op-contract-acceptVpcEndpointConnections.

Cause 5 of 5: transit-gateway association/propagation scenarios never converge.
AWS: with DefaultRouteTableAssociation/Propagation disabled, a new VPC attachment reaches available, AssociateTransitGatewayRouteTable and EnableTransitGatewayRouteTablePropagation then make the propagated VPC route appear in the associated table (and only there) via SearchTransitGatewayRoutes, each transition within tens of seconds.
Emulator: both multi-step TGW checks exhaust their 60 s budget (Error: Check timed out after 60.0s) before the state they poll for appears. Hypothesis: one of the attachment/route-table/propagation states never reaches available, so every downstream poll stalls.
Why: any client that waits for a TGW attachment or propagated route — the normal way these APIs are driven — never gets past the wait.
Fix: the transit-gateway attachment, route-table and propagation state machines and the routes SearchTransitGatewayRoutes derives from them.
Traces: ec2.tgwc-attachments-associate-propagate-and-route-both-vpcs, ec2.tgwc-unpropagated-route-table-blocked-from-return-route.

## Evidence
Cause 1 — KeyError: 'RouteTableIds' at `assert got["RouteTableIds"] == []` after DescribeVpcEndpoints(VpcEndpointIds=[vpce-...]) returned an available Interface endpoint whose other members all matched.
Cause 2 — simparity.published.PublishedUrlRejected: endpoint_host was given 54.0.0.2:8080 (published by the emulator or built by the check): its host is not one of the emulator's own hosts.
Cause 3 — Error: Check timed out after 60.0s (hibernate stop/resume data-plane scenario).
Cause 4 — Error: Check timed out after 60.0s (AcceptVpcEndpointConnections contract).
Cause 5 — Error: Check timed out after 60.0s on both ec2.tgwc-attachments-associate-propagate-and-route-both-vpcs and ec2.tgwc-unpropagated-route-table-blocked-from-return-route.

## Steps to Reproduce
1. Create a VPC + subnets + security group, CreateVpcEndpoint(VpcEndpointType=Interface) for an AWS service, poll to available, then DescribeVpcEndpoints and read RouteTableIds.
2. RunInstances with HibernationOptions.Configured=true, StopInstances(Hibernate=true), StartInstances, then connect to the public address the API reports for the instance.
3. Create a VPC endpoint against an acceptance-required endpoint service and poll DescribeVpcEndpointConnections for pendingAcceptance, then AcceptVpcEndpointConnections.
4. CreateTransitGateway with DefaultRouteTableAssociation/Propagation disabled, two route tables, a VPC attachment per VPC; associate + enable propagation and SearchTransitGatewayRoutes on each table.
Replay the trace_ids listed.
