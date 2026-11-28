Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-error-config-query-conflict
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-error-config-query-conflict resource-groups-generic-group-configuration
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|CreateGroup|2xx->2xx|$.GroupConfiguration.GroupParameters
Issue-Key: resource-groups|GetGroupConfiguration|2xx->2xx|$.GroupConfiguration.GroupParameters
Issue-Key: resource-groups|GetGroupConfiguration|2xx->2xx|$.GroupIdentifier
Issue-Key: resource-groups|PutGroupConfiguration|2xx->2xx|$
Issue-Related: 20261128T154631Z-resource-groups-group-response-omits-ownerid-on-create.md
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# Resource Groups group service configuration is neither stored nor returned

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGroupConfiguration

## Description
A group created with (or updated to have) a service configuration loses the configuration's parameters: the configuration items come back without their Parameters, GetGroupConfiguration omits the group identifier, and PutGroupConfiguration answers with an empty body. One root cause - the group-configuration record in state keeps only the item Type and no handler serializes the full GroupConfiguration/GroupIdentifier payload - surfaces in three operations; all three must be closed.

AWS: Smithy - CreateGroup.Configuration and PutGroupConfiguration.Configuration take a GroupConfigurationList (0..2) of GroupConfigurationItem{Type (required, pattern ^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$), Parameters: list of GroupConfigurationParameter{Name (required), Values}}; CreateGroupOutput.GroupConfiguration and GetGroupConfigurationOutput.GroupConfiguration are the GroupConfiguration structure whose Configuration member is documented as "The configuration currently associated with the group and in effect". So the parameters a caller supplies are part of the stored, readable configuration. For the extra GroupIdentifier member on GetGroupConfiguration/PutGroupConfiguration responses and for PutGroupConfiguration returning a body at all, this is recorded behavior only: the model declares GetGroupConfigurationOutput with GroupConfiguration alone and PutGroupConfigurationOutput as empty, yet the recordings carry GroupIdentifier{GroupName, GroupArn} and, on the 202, the effective GroupConfiguration. Those values are derived from the request and from state (the group's own name and ARN, the configuration just supplied), never per-run data.
Emulator: the configuration items are returned with Type only - the replay reports `$.GroupConfiguration.GroupParameters: missing` - GetGroupConfiguration returns no GroupIdentifier, and PutGroupConfiguration returns a 202 with an empty JSON body.
Why: a client that writes a configuration and reads it back (the documented way to check which resource types a capacity-reservation or generic group admits) sees a group whose parameters vanished, so it re-applies the configuration forever or concludes the allowed-resource-types restriction is not in force.
Fix: the group-configuration state record (store each item's Parameters alongside its Type) and the serializers for CreateGroup, GetGroupConfiguration and PutGroupConfiguration (emit GroupConfiguration.Configuration[].Parameters, and GroupIdentifier{GroupName, GroupArn} where the recordings show it).

Cause 1 of 3: CreateGroup with a Configuration - the 200 response's GroupConfiguration omits the configuration items' Parameters (and the same response omits Group.OwnerId, which is the subject of the separate OwnerId issue in this batch). Fixture resource-groups-error-config-query-conflict step setup#2 (same cluster: resource-groups-generic-group-configuration#setup#1).
Cause 2 of 3: GetGroupConfiguration - the 200 response omits both GroupIdentifier{GroupName, GroupArn} and the configuration items' Parameters. Fixture resource-groups-generic-group-configuration step task#2 (same cluster: task#4).
Cause 3 of 3: PutGroupConfiguration - the 202 response body is empty where the recording returns GroupIdentifier{GroupName, GroupArn} plus the GroupConfiguration now in effect (items AWS::EC2::CapacityReservationPool and AWS::ResourceGroups::Generic with its Parameters). Fixture resource-groups-generic-group-configuration step task#3.

## Evidence
CreateGroup (resource-groups-error-config-query-conflict#setup#2): status 200/200, diff `$.GroupConfiguration.GroupParameters: missing (expected [{'Name': 'allowed-resource-types', 'Values': ['AWS::EC2::CapacityReservation']}], got None)` and `$.Group.OwnerId: missing`.
GetGroupConfiguration (resource-groups-generic-group-configuration#task#2 and #task#4): status 200/200, diffs `$.GroupIdentifier: missing (expected {'GroupName': <the group's name>, 'GroupArn': <the group's ARN>}, got None)` and `$.GroupConfiguration.GroupParameters: missing (expected the allowed-resource-types parameter list, got None)`.
PutGroupConfiguration (resource-groups-generic-group-configuration#task#3): status 202/202, `$: empty_body` - expected GroupIdentifier{GroupName, GroupArn} and GroupConfiguration{Configuration: [{Type: AWS::EC2::CapacityReservationPool}, {Type: AWS::ResourceGroups::Generic, Parameters: [{Name: allowed-resource-types, Values: [AWS::EC2::CapacityReservation]}]}]}, got {}. The group name/ARN are those of the group named in the request; the parameter name and values are those supplied by the caller.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1.
2. CreateGroup with Configuration = [{Type: AWS::EC2::CapacityReservationPool}, {Type: AWS::ResourceGroups::Generic, Parameters: [{Name: allowed-resource-types, Values: [AWS::EC2::CapacityReservation]}]}] (resource-groups-generic-group-configuration#setup#1) - the response configuration items come back with no Parameters.
3. GetGroupConfiguration on that group (step task#2) - no GroupIdentifier and no item Parameters.
4. PutGroupConfiguration on that group (step task#3) - 202 with an empty body instead of the group identifier and the effective configuration; a following GetGroupConfiguration (task#4) still shows no parameters.
