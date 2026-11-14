Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-network-interface-permission-grant-and-revoke
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-network-interface-permission-grant-and-revoke ec2-network-interface-permission-lifecycle ec2-dry-run-delete-validation-across-network-resources ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-vfd-cloudwatch-logs-destination-and-delivery-role ec2-vfd-firehose-destination-mistakes ec2-vfd-log-format-field-name-refusals ec2-vfd-s3-destination-mistakes-then-recovery
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateNetworkInterfacePermission|2xx->4xx|$.CreateNetworkInterfacePermissionResponse
Issue-Key: ec2|CreateNetworkInterfacePermission|2xx->4xx|status
Issue-Key: ec2|DeleteIpamPrefixListResolver|2xx->4xx|$.DeleteIpamPrefixListResolverResponse
Issue-Key: ec2|DeleteIpamPrefixListResolver|2xx->4xx|status
Issue-Key: ec2|CreateFlowLogs|4xx->2xx|$.Response
Issue-Key: ec2|CreateFlowLogs|4xx->2xx|status
Issue-Commit: 588eab0496abf0a5bc1b8cec7adc2c989702bf8c

# ec2 inverts refusal verdicts: valid requests answer HTTP 400 while an unsupported flow-log option is accepted (3 causes)

> _Note: this issue was identified against the code base at commit `588eab0496abf0a5bc1b8cec7adc2c989702bf8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkInterfacePermission

## Description
Three ec2 operations return the opposite verdict from AWS: two refuse a well-formed request with HTTP 400 and serve no response body, and one accepts a request AWS refuses.

Cause 1 of 3: CreateNetworkInterfacePermission refuses a valid grant.
AWS: Smithy: CreateNetworkInterfacePermission input requires NetworkInterfaceId and Permission (enum InterfacePermissionType, e.g. INSTANCE-ATTACH) with optional AwsAccountId/AwsService/DryRun; output member InterfacePermission (xmlName interfacePermission) carries awsAccountId, networkInterfaceId, networkInterfacePermissionId, permission and permissionState.state. Granting INSTANCE-ATTACH on an existing ENI to another account id answers HTTP 200 with CreateNetworkInterfacePermissionResponse.interfacePermission and permissionState.state 'GRANTED'.
Emulator: HTTP 400 and no CreateNetworkInterfacePermissionResponse element at all, for the existing ENI the fixture just created.
Why: the permission is never created, so every dependent step (describe, revoke, cross-account attach) fails; a client cannot share an ENI at all.
Fix: the CreateNetworkInterfacePermission handler - its input validation/ENI resolution, the permission record in ec2 state, and the response serializer.
Traces: ec2-network-interface-permission-grant-and-revoke#task#4 (cluster also covers ec2-network-interface-permission-lifecycle#task#5) and ec2-dry-run-delete-validation-across-network-resources#setup#6 (same operation, status 200->400).

Cause 2 of 3: DeleteIpamPrefixListResolver refuses a valid delete.
AWS: Smithy: DeleteIpamPrefixListResolver input requires IpamPrefixListResolverId; output member IpamPrefixListResolver (xmlName ipamPrefixListResolver). Deleting a resolver that exists answers HTTP 200 with the resolver projection, whose state member reports 'delete-in-progress' (and the ipamPrefixListResolverId, ipamPrefixListResolverArn, ipamArn, addressFamily, ipamRegion, ownerId and lastVersionCreationStatus members of the stored resolver).
Emulator: HTTP 400 with no DeleteIpamPrefixListResolverResponse body, for the resolver created earlier in the same fixture.
Why: the IPAM prefix-list resolver can never be torn down, so the whole delete/deprovision lifecycle wedges and leaks state.
Fix: the DeleteIpamPrefixListResolver handler's id resolution/state transition and its response serializer.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#32.

Cause 3 of 3: CreateFlowLogs accepts DestinationOptions that the chosen LogDestinationType does not support.
AWS: recorded behavior only for the text (Smithy: CreateFlowLogs input members LogDestinationType (enum LogDestinationType), LogDestination and DestinationOptions (DestinationOptionsRequest), documented only as "The destination options."; errors: []). A request combining DestinationOptions with a LogDestinationType other than s3 is refused HTTP 400 with Code 'InvalidParameter' and Message 'DestinationOptions is not supported when LogDestinationType is <LogDestinationType>' - fixed text 'DestinationOptions is not supported when LogDestinationType is ' plus the LogDestinationType value from the request (recorded: cloud-watch-logs).
Emulator: HTTP 200 with no Response/Errors element - it creates the flow log instead of refusing, and the same cluster covers the equivalent unsupported destination combinations in the firehose and s3 flow-log fixtures.
Why: a client gets a flow log that AWS would never have created, so an invalid configuration is silently taken live and the caller's validation path is never exercised.
Fix: the CreateFlowLogs handler's request validation (DestinationOptions / LogDestinationType / LogDestination compatibility) and its refusal mapping.
Traces: ec2-vfd-cloudwatch-logs-destination-and-delivery-role#task#7 (cluster also covers ec2-vfd-firehose-destination-mistakes#task#3, #task#4, #task#5, ec2-vfd-log-format-field-name-refusals#task#7, ec2-vfd-s3-destination-mistakes-then-recovery#task#3 and #task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 (4 divergence clusters, verdict=mismatch, stub_path empty, owned_by empty):
1. ec2-network-interface-permission-grant-and-revoke#task#4 CreateNetworkInterfacePermission: status expected 200, got 400; $.CreateNetworkInterfacePermissionResponse missing (expected requestId + interfacePermission{awsAccountId, networkInterfaceId eni-..., networkInterfacePermissionId, permission 'INSTANCE-ATTACH', permissionState{state 'GRANTED'}}, got None). Cluster also affects ec2-network-interface-permission-lifecycle#task#5.
2. ec2-dry-run-delete-validation-across-network-resources#setup#6 CreateNetworkInterfacePermission: status expected 200, got 400.
3. ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#32 DeleteIpamPrefixListResolver: status expected 200, got 400; $.DeleteIpamPrefixListResolverResponse missing (expected ipamPrefixListResolver{... state 'delete-in-progress', lastVersionCreationStatus 'success' ...}, got None).
4. ec2-vfd-cloudwatch-logs-destination-and-delivery-role#task#7 CreateFlowLogs: status expected 400, got 200; $.Response missing (expected Errors.Error{Code 'InvalidParameter', Message 'DestinationOptions is not supported when LogDestinationType is cloud-watch-logs'}, got None); affected_count 7 across 5 flow-log fixtures.
Model evidence: smithy_query(query=operation, model_path=services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) for CreateNetworkInterfacePermission, DeleteIpamPrefixListResolver and CreateFlowLogs (input/output members quoted above; errors: [] for all three).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay ec2-network-interface-permission-grant-and-revoke step task#4: create an ENI, then CreateNetworkInterfacePermission with Permission=INSTANCE-ATTACH for another account id - the emulator answers 400 instead of 200 with interfacePermission.permissionState.state GRANTED. Same on ec2-dry-run-delete-validation-across-network-resources#setup#6.
3. Replay ec2-ipam-consolidated-delete-and-deprovision-lifecycle step task#32: DeleteIpamPrefixListResolver for the resolver created earlier in the fixture - the emulator answers 400 instead of 200 with state 'delete-in-progress'.
4. Replay ec2-vfd-cloudwatch-logs-destination-and-delivery-role step task#7: CreateFlowLogs with LogDestinationType=cloud-watch-logs plus DestinationOptions - the emulator answers 200 instead of 400 InvalidParameter.
