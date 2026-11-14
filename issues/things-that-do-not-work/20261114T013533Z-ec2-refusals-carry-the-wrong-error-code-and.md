Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-error-paths-invalid-table-entry-parents
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-error-paths-invalid-table-entry-parents ec2-network-interface-permission-grant-and-revoke ec2-sshp-traj-referenced-group-source-has-no-cidr ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes ec2-terminate-client-vpn-connections-error-paths ec2-trajwire-lia-instance-status-route-mistakes ec2-vpce-traj-endpoint-lookup-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateTransitGatewayPolicyTableEntry|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateTransitGatewayPolicyTableEntry|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateNetworkInterfacePermission|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AuthorizeSecurityGroupIngress|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|TerminateClientVpnConnections|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeInstanceStatus|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeInstanceStatus|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|ModifyVpcEndpoint|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261103T071133Z-ec2-refusal-messages-are-reworded-across-network-security.md
Issue-Commit: 588eab0496abf0a5bc1b8cec7adc2c989702bf8c

# ec2 refusals carry the wrong error code and emulator-authored message text (8 causes)

> _Note: this issue was identified against the code base at commit `588eab0496abf0a5bc1b8cec7adc2c989702bf8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransitGatewayPolicyTableEntry

## Description
Eight independent refusal paths in the ec2 emulator answer the right HTTP status but the wrong ec2Query error Code and/or an emulator-authored Message, so a client that branches on AWS's code or matches AWS's wording mis-handles the refusal. EC2 models no operation errors (smithy_query query=operation for every operation below returns errors: []), so the error code and message text are grounded in the recorded responses; member names are cited from the model.

Cause 1 of 8: CreateTransitGatewayPolicyTableEntry validates the wrong parameter first.
AWS: recorded behavior only (Smithy: CreateTransitGatewayPolicyTableEntry input requires TransitGatewayPolicyTableId, PolicyRuleNumber and TargetRouteTableId; errors: []). When the referenced target route table does not exist AWS answers HTTP 400 with Code 'InvalidTransitGatewayRouteTableId.NotFound' and Message "The transit-gateway-route-table ID '<TargetRouteTableId>' does not exist" - fixed text "The transit-gateway-route-table ID '" ... "' does not exist", variable part the TargetRouteTableId value taken from the request.
Emulator: HTTP 400 with Code 'InvalidTransitGatewayPolicyTableId.Malformed' and Message 'Invalid id: "<TransitGatewayPolicyTableId>"' - it rejects the policy-table id as malformed instead of resolving the target route table.
Why: a client that handles InvalidTransitGatewayRouteTableId.NotFound by creating/awaiting the route table and retrying instead sees a Malformed error about a different parameter and treats its own request as unfixable.
Fix: the CreateTransitGatewayPolicyTableEntry handler's input validation (id-syntax acceptance for the tgw-ptb- prefix and validation ordering) and its refusal mapping.
Traces: ec2-error-paths-invalid-table-entry-parents#task#1.

Cause 2 of 8: CreateNetworkInterfacePermission names the member, not the resource, for an unknown ENI.
AWS: recorded behavior only. For an unknown NetworkInterfaceId AWS answers HTTP 400 with Message "The interface ID '<NetworkInterfaceId>' does not exist" - fixed text "The interface ID '" ... "' does not exist", variable part the NetworkInterfaceId from the request (Smithy: CreateNetworkInterfacePermission input member NetworkInterfaceId, required).
Emulator: same status and code but Message "The networkInterface ID '<NetworkInterfaceId>' does not exist".
Why: the message is a documented, client-visible contract; tooling that matches AWS's wording (or shows it to an operator) reports a resource type EC2 never names.
Fix: the CreateNetworkInterfacePermission handler's not-found refusal text.
Traces: ec2-network-interface-permission-grant-and-revoke#task#10.

Cause 3 of 8: AuthorizeSecurityGroupIngress reports a cross-VPC referenced group as nonexistent.
AWS: recorded behavior only. When a rule's referenced source group lives in another VPC than the group being authorized, AWS answers HTTP 400 with Message 'You have specified two resources that belong to different networks.' - the whole message is fixed text with no variable part.
Emulator: same status and code but Message "The security group '<group-id>' does not exist in VPC '<vpc-id>'", naming the referenced group id and the target VPC id.
Why: the emulator says the group does not exist, so a client retries creation of a group it already has instead of correcting the cross-VPC reference.
Fix: the AuthorizeSecurityGroupIngress handler's validation of referenced UserIdGroupPairs (cross-VPC case) and its refusal text.
Traces: ec2-sshp-traj-referenced-group-source-has-no-cidr#task#6 (cluster also covers ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#4, #task#6, #task#7).

Cause 4 of 8: AuthorizeSecurityGroupIngress mis-classifies an inverted port range.
AWS: recorded behavior only. For FromPort greater than ToPort AWS answers HTTP 400 with Code 'InvalidParameterValue' and Message 'Invalid TCP/UDP port range(<FromPort>:<ToPort>)' - fixed text 'Invalid TCP/UDP port range(' + ':' + ')', variable parts the FromPort and ToPort values from the request.
Emulator: HTTP 400 with Code 'InvalidPermission.Malformed' and Message 'ipPermissions.<n> ends at <ToPort>, before it starts at <FromPort>'.
Why: InvalidParameterValue and InvalidPermission.Malformed are different client branches; the emulator also invents an index-qualified member name AWS does not use here.
Fix: the AuthorizeSecurityGroupIngress handler's port-range validation and refusal mapping.
Traces: ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#5.

Cause 5 of 8: TerminateClientVpnConnections treats an unknown username as a nonexistent principal.
AWS: recorded behavior only (Smithy: TerminateClientVpnConnections input members ClientVpnEndpointId required, Username optional). When no connection matches the Username AWS answers HTTP 400 with Message 'No connections exists for the given username <Username>' - fixed text 'No connections exists for the given username ' plus the Username from the request.
Emulator: same status and code but Message 'User <Username> does not exist'.
Why: AWS's contract is about connections, not identities; a client that reads the emulator's text concludes the user is unknown rather than that there is nothing to terminate.
Fix: the TerminateClientVpnConnections handler's no-matching-connection refusal text.
Traces: ec2-terminate-client-vpn-connections-error-paths#task#3.

Cause 6 of 8: DescribeInstanceStatus names the wrong parameter in its mutually-exclusive-parameter refusal.
AWS: recorded behavior only (Smithy: DescribeInstanceStatus input members InstanceIds (wire InstanceId.N) and MaxResults). Combining instance ids with MaxResults is refused HTTP 400 with Message 'The parameter instanceIdsSet cannot be used with the parameter maxResults' - the whole message is fixed text and AWS names the sets 'instanceIdsSet' and 'maxResults'.
Emulator: same status and code but Message 'The parameter instancesSet cannot be used with the parameter maxResults'.
Why: the message names a parameter the caller never sent, so the operator cannot map it back to a request field.
Fix: the DescribeInstanceStatus handler's parameter-conflict validation text.
Traces: ec2-trajwire-lia-instance-status-route-mistakes#task#2.

Cause 7 of 8: DescribeInstanceStatus skips instance-id syntax validation.
AWS: recorded behavior only. A syntactically invalid instance id is refused before any lookup, HTTP 400 with Code 'InvalidInstanceID.Malformed' and Message 'Invalid id: "<the malformed id as sent>"' - fixed text 'Invalid id: "' ... '"', variable part the offending id from the request.
Emulator: HTTP 400 with Code 'InvalidInstanceID.NotFound' and Message "The instance ID '<some other requested id>' does not exist": it accepts the malformed id and instead reports the first id it cannot resolve as not found.
Why: Malformed is a permanent client error while NotFound reads as eventual consistency, so a client retries a request that can never succeed, and the message points at the wrong id.
Fix: the DescribeInstanceStatus handler's instance-id syntax validation (ordered before resolution) and its refusal mapping.
Traces: ec2-trajwire-lia-instance-status-route-mistakes#task#3.

Cause 8 of 8: ModifyVpcEndpoint's unknown-endpoint message uses the model member name.
AWS: recorded behavior only. For an unknown endpoint id AWS answers HTTP 400 with Message "The Vpc Endpoint Id '<VpcEndpointId>' does not exist" - fixed text "The Vpc Endpoint Id '" ... "' does not exist", variable part the VpcEndpointId from the request.
Emulator: same status and code but Message "The VpcEndpoint Id '<VpcEndpointId>' does not exist".
Why: same client-visible wording contract as above.
Fix: the ModifyVpcEndpoint handler's not-found refusal text.
Traces: ec2-vpce-traj-endpoint-lookup-mistakes#task#7.

Related open issues that do NOT carry these replay keys and do not cover these steps: 20261111T001230Z-ec2-refusals-use-emulator-authored-error-codes-and.md, 20261112T061909Z-ec2-modifyvpcendpoint-answers-http-501-notimplemented-instead-of.md, 20261112T172205Z-ec2-createcoippool-s-invalidlocalgatewayroutetableid-malformed-refusal-message-is.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 divergence clusters, all verdict=mismatch, stub_path empty, owned_by empty):
1. ec2-error-paths-invalid-table-entry-parents#task#1 CreateTransitGatewayPolicyTableEntry 400->400: $.Response.Errors.Error.Code expected 'InvalidTransitGatewayRouteTableId.NotFound', got 'InvalidTransitGatewayPolicyTableId.Malformed'; $.Response.Errors.Error.Message expected "The transit-gateway-route-table ID 'tgw-rtb-0123456789abcde' does not exist", got 'Invalid id: "tgw-ptb-0123456789abcde"'.
2. ec2-network-interface-permission-grant-and-revoke#task#10 CreateNetworkInterfacePermission 400->400: Message expected "The interface ID 'eni-...' does not exist", got "The networkInterface ID 'eni-...' does not exist".
3. ec2-sshp-traj-referenced-group-source-has-no-cidr#task#6 AuthorizeSecurityGroupIngress 400->400 (4 steps affected): Message expected 'You have specified two resources that belong to different networks.', got "The security group 'sg-...' does not exist in VPC 'vpc-...'".
4. ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#5 AuthorizeSecurityGroupIngress 400->400: Code expected 'InvalidParameterValue', got 'InvalidPermission.Malformed'; Message expected 'Invalid TCP/UDP port range(23:22)', got 'ipPermissions.1 ends at 22, before it starts at 23'.
5. ec2-terminate-client-vpn-connections-error-paths#task#3 TerminateClientVpnConnections 400->400: Message expected 'No connections exists for the given username <username>', got 'User <username> does not exist'.
6. ec2-trajwire-lia-instance-status-route-mistakes#task#2 DescribeInstanceStatus 400->400: Message expected 'The parameter instanceIdsSet cannot be used with the parameter maxResults', got 'The parameter instancesSet cannot be used with the parameter maxResults'.
7. ec2-trajwire-lia-instance-status-route-mistakes#task#3 DescribeInstanceStatus 400->400: Code expected 'InvalidInstanceID.Malformed', got 'InvalidInstanceID.NotFound'; Message expected 'Invalid id: "i-7a4e2c91f6b803d5e"', got "The instance ID 'i-12345678' does not exist".
8. ec2-vpce-traj-endpoint-lookup-mistakes#task#7 ModifyVpcEndpoint 400->400: Message expected "The Vpc Endpoint Id 'vpce-...' does not exist", got "The VpcEndpoint Id 'vpce-...' does not exist".
Model evidence: smithy_query(query=operation, model_path=services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) for CreateTransitGatewayPolicyTableEntry, CreateNetworkInterfacePermission, DescribeInstanceStatus and TerminateClientVpnConnections - all list errors: [] (ec2Query errors are unmodeled), with the input members cited above.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture ec2-error-paths-invalid-table-entry-parents step task#1 (CreateTransitGatewayPolicyTableEntry naming a nonexistent TargetRouteTableId) and compare $.Response.Errors.Error.Code/.Message.
3. Replay ec2-network-interface-permission-grant-and-revoke#task#10, ec2-sshp-traj-referenced-group-source-has-no-cidr#task#6, ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#5, ec2-terminate-client-vpn-connections-error-paths#task#3, ec2-trajwire-lia-instance-status-route-mistakes#task#2 and #task#3, ec2-vpce-traj-endpoint-lookup-mistakes#task#7 and compare the error Code and Message of each refusal against the recorded response.
