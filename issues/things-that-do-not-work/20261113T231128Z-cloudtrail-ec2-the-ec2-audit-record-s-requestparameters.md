Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id
Issue-Key: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id|run#46|KeyError|req_items = run_rec["requestParameters"]["instancesSet"]["items"]
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudtrail__ec2: the EC2 audit record's requestParameters/responseElements use ec2Query wire and model member names instead of CloudTrail's EC2 set/items grouping (instancesSet, tagSpecificationSet)

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause (Cause 1 of 1).

NOTE on an open issue that does NOT cover this: 20261028T021552Z-cloudtrail-ec2-cloudtrail-records-no-ec2-ec2-amazonaws-com-management.md owns the earlier state of this same check, where NO ec2.amazonaws.com record existed at all and the check died in its LookupEvents poll (its Issue-Key binds the `_await_records` timeout site). The emulator now DOES record and serve the records: in this replay the check got past every record-identity assertion (eventSource 'ec2.amazonaws.com', eventName 'RunInstances', requestID == the SDK's ResponseMetadata.RequestId, awsRegion, readOnly false, no errorCode, envelope EventId == record eventID) and failed one assertion later, on the CONTENT of requestParameters. The suite did not mark this check as owned by that issue, so the surviving defect is filed here as its own root cause.

Cause 1 of 1 — the audit projection of an EC2 call names its parameters (and its response) with the ec2Query request's own keys and the modelled output's members, not with the legacy EC2 'set/items' grouping CloudTrail actually records, so `requestParameters.instancesSet` does not exist on a RunInstances record.
Failing check ids: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id

AWS: recorded behavior only — the CloudTrail model says nothing about the content of an event's requestParameters/responseElements (Smithy, cloudtrail model: LookupEvents returns Events[].CloudTrailEvent as an opaque String carrying the JSON record; the record's fields are not modelled). What real AWS records for EC2 is the service's internal set/items shape, not the ec2Query form the request was sent in and not the Smithy output shape: for RunInstances, requestParameters groups the per-instance request members under `instancesSet.items[]` (`imageId`, `minCount`, `maxCount`), keeps the launch-wide members flat (`instanceType`), and groups tag specifications under `tagSpecificationSet.items[]` with `resourceType` plus `tags[].key`/`tags[].value`; responseElements carries `reservationId` and `instancesSet.items[]` with `instanceId`, `subnetId` and `instanceState.name` = 'pending'. For TerminateInstances, requestParameters is `instancesSet.items[].instanceId` and responseElements is `instancesSet.items[].instanceId` with `currentState.name` equal to the 'shutting-down' the SDK saw in the same response. Member names in these documents are lowerCamel and list members are always wrapped in a `<name>Set`/`items` pair; the values are taken from the request and from what EC2 minted, never from a fixed example.

Emulator: the record is served and correlates correctly, but its requestParameters has no `instancesSet` member at all — reading `run_rec["requestParameters"]["instancesSet"]` raises `KeyError: 'instancesSet'`. The audit leg builds requestParameters from the ec2Query parameter list read back as the paths its keys name (services/service-com-amazonaws-ec2-amazonec2/src/service/management_events.rs, module doc: "requestParameters is the parameter list the ec2Query request carried, read back as the paths its keys name"; see also `ENVELOPE_PARAMETERS`/`MAX_RECORDED_PARAMETERS` there), and responseElements from the modelled output's own schema (`service_common::management_events::recorded_shape`). Neither projection performs the set/items regrouping, so a RunInstances record carries the request's flat `ImageId`/`MinCount`/`MaxCount`/`SubnetId`/`TagSpecification.N.*` paths and a response document shaped like the modelled `Reservation`, with none of `instancesSet`, `tagSpecificationSet` or `instanceState`. The downstream assertions on responseElements.instancesSet / reservationId and on the TerminateInstances record's instancesSet/currentState are unreached in this replay, but they read the same absent grouping and come from the same projection.

Why: a client reading audit records for EC2 cannot find the parameters or the minted ids where every consumer looks for them. Incident reconstruction, compliance evidence and EventBridge/Athena rules that key on `$.requestParameters.instancesSet.items[0].imageId`, `$.requestParameters.tagSpecificationSet`, `$.responseElements.instancesSet.items[0].instanceId` or `...instanceState.name` match nothing or crash on a missing key — the record looks present and correlated, so the failure surfaces as silently empty audit queries rather than as an error.

Fix: the EC2 service's audit projection — the requestParameters/responseElements rendering in the ec2 front door's audit leg (services/service-com-amazonaws-ec2-amazonec2/src/service/management_events.rs) and the shared helpers it calls (`service_common::management_events::recorded_request_parameters` / `recorded_shape`) — must emit the EC2 set/items document CloudTrail records for ec2.amazonaws.com instead of the raw ec2Query key paths and the modelled output shape: list-valued groups wrapped as `<name>Set`/`items`, per-instance request members nested under `instancesSet.items[]`, tag specifications under `tagSpecificationSet.items[]` with key/value tags, and the response's instance entries carrying instanceId/subnetId/instanceState (RunInstances) and instanceId/currentState/previousState (TerminateInstances) beside reservationId.

Traces: cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id.

## Evidence
Failing check cloudtrail__ec2.obs-ec2-3-run-and-terminate-instances-audit-records-join-the-sdk-request-id — verdict 'error':

  File "check_cloudtrail__ec2_obs_ec2_3_run_and_terminate_instances_audit_records_join_the_sdk_request_id.py", line 140, in run
    req_items = run_rec["requestParameters"]["instancesSet"]["items"]
  KeyError: 'instancesSet'

Everything before that line passed, which is what localizes the defect: the RunInstances record was found by requestID, and run_rec['eventSource'] == 'ec2.amazonaws.com', run_rec['eventName'] == 'RunInstances', run_rec['requestID'] == run_resp['ResponseMetadata']['RequestId'], run_rec['awsRegion'] == the caller's Region, run_rec['readOnly'] is False, 'errorCode' not in run_rec and run_env['EventId'] == run_rec['eventID'] all held.

Expected (real AWS, as the check asserts):
  run_rec['requestParameters']['instancesSet']['items'][0]['imageId'] == the ImageId sent
  run_rec['requestParameters']['instanceType'] == the InstanceType sent ('t3.micro' in this run)
  run_rec['requestParameters']['tagSpecificationSet']['items'] containing the resourceType 'instance' spec whose tags carry the key/value pairs the request sent
  run_rec['responseElements']['instancesSet']['items'][0]['instanceId'] == the created instance id, ['subnetId'] == the SubnetId sent, ['instanceState']['name'] == 'pending'
  run_rec['responseElements']['reservationId'] == run_resp['ReservationId']
  term_rec['requestParameters']['instancesSet']['items'][0]['instanceId'] == that instance id
  term_rec['responseElements']['instancesSet']['items'][0]['currentState']['name'] == 'shutting-down' (the same value the SDK read in the TerminateInstances response)
(The ids, AMI, subnet, tag values, Region and request ids above are this run's values filling those slots; the requirement is the key structure and the join to what the request sent and what the response returned.)

Actual (emulator): the RunInstances record's requestParameters document has no 'instancesSet' member — the lookup raises KeyError — so none of the above paths resolve. Per services/service-com-amazonaws-ec2-amazonec2/src/service/management_events.rs lines 15-33, requestParameters is the ec2Query request's own key paths and responseElements is rendered from the modelled output schema, neither of which contains the set/items grouping.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 CreateVpc(CidrBlock=10.42.0.0/16); ec2 CreateSubnet(CidrBlock=10.42.1.0/24) in an available AZ that offers t3.micro; resolve an Amazon-owned available al2023 x86_64 AMI with ec2 DescribeImages.
3. ec2 RunInstances(ImageId=<ami>, InstanceType=t3.micro, MinCount=1, MaxCount=1, SubnetId=<subnet>, TagSpecifications=[{instance tags},{volume tags}]); keep the response's ResponseMetadata.RequestId, ReservationId and the instance id.
4. ec2 TerminateInstances(InstanceIds=[<instance id>]); keep that response's ResponseMetadata.RequestId.
5. cloudtrail LookupEvents(StartTime=<just before step 3>, EndTime=<now+5min>, MaxResults=50, LookupAttributes=[{AttributeKey: 'EventName', AttributeValue: 'RunInstances'}]), paging NextToken to exhaustion; select the event whose json.loads(CloudTrailEvent)['requestID'] equals step 3's request id. Repeat with AttributeValue 'TerminateInstances' for step 4's request id.
6. Read record['requestParameters']['instancesSet']['items'] on the RunInstances record.
7. Observe KeyError: 'instancesSet' — requestParameters carries the flat ec2Query parameter paths instead, and responseElements likewise lacks instancesSet/reservationId in CloudTrail's EC2 form.
