Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__iam__stepfunctions.cloudcontrol-iam-managed-policy-lifecycle-identifier-is-arn cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-object-round-trip
Issue-Key: cloudcontrol__iam__stepfunctions.cloudcontrol-iam-managed-policy-lifecycle-identifier-is-arn|run#10|AssertionError|assert identifier == expected_arn, (
Issue-Key: cloudcontrol__iam__stepfunctions.cloudcontrol-state-machine-definition-object-round-trip|run#11|AssertionError|assert identifier == expected_arn, (
Issue-Commit: 723aa94a65a7375dec1c2b71e7ed3185ecc54583

# cloudcontrol CreateResource reports the resource NAME as ProgressEvent.Identifier for types whose primary identifier is an ARN (AWS::IAM::ManagedPolicy, AWS::StepFunctions::StateMachine)

> _Note: this issue was identified against the code base at commit `723aa94a65a7375dec1c2b71e7ed3185ecc54583`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
Two net-new root causes, both in the cloudcontrol service: the primary identifier Cloud Control hands back for a created resource is derived from a name-shaped property of DesiredState instead of the resource type's actual primary identifier, so for every type whose primary identifier is an ARN the caller is handed a bare name it cannot address the resource with.

Cause 1 of 2: CreateResource(TypeName="AWS::IAM::ManagedPolicy") answers ProgressEvent.Identifier = the ManagedPolicyName from DesiredState, not the policy ARN.
Failing check: cloudcontrol__iam__sfn.cloudcontrol-iam-managed-policy-lifecycle-identifier-is-arn
AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) — CreateResource takes TypeName (required, pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$) and DesiredState (required) and answers a ProgressEvent; ProgressEvent$Identifier is documented as "The primary identifier for the resource.", and GetResource$Identifier / UpdateResource$Identifier / DeleteResource$Identifier are that same value (required, length 1..1024). Which property IS the primary identifier is stated by the resource type's registry schema, not by Smithy: for AWS::IAM::ManagedPolicy the primaryIdentifier is /properties/PolicyArn, a readOnly property, so the identifier is the IAM policy ARN of the form arn:<partition>:iam::<account-id>:policy<path><ManagedPolicyName> (the fixed text is "arn:", ":iam::", ":policy"; partition, account id, path and the ManagedPolicyName are taken from the request and the emulator's state). ManagedPolicyName is create-only. A Cloud Control AWS::IAM::ManagedPolicy IS the IAM policy, so the identifier Cloud Control reports is exactly the ARN IAM's GetPolicy/GetPolicyVersion/DeletePolicy accept.
Emulator: the returned identifier carries no colons at all — the replay shows identifier.split(':')[:5] == ['sim2real-xsvc-ccmp-2975981c4cf5'], i.e. the identifier is the ManagedPolicyName that DesiredState carried. The create itself is accepted and reaches SUCCESS; only the identifier is wrong, and every later call in the flow (IAM GetPolicy(PolicyArn=identifier), Cloud Control GetResource/UpdateResource/DeleteResource with that Identifier) is built on it.
Why: Cloud Control is the generic control plane a client uses without knowing the owning service's API; ProgressEvent.Identifier is the ONLY handle the client is given. A client that hands that identifier to IAM (GetPolicy/AttachRolePolicy/DeletePolicy all take a PolicyArn) gets an invalid-ARN refusal, and a client that stores it as the resource's primary identifier records a value that is not unique across paths and cannot be re-resolved. Generic IaC/resource-management tooling that round-trips primaryIdentifier breaks on every ARN-identified type.
Fix: the cloudcontrol crate's identifier derivation — crate::state::derive_identifier / identifier_property_candidates — must resolve the type's primary identifier from the resource type's schema rather than guessing "<ResourceSegment>Name" first, and for a type whose primary identifier is a readOnly ARN it must SYNTHESIZE that ARN for the resource being provisioned (partition/region/account from the request context, remaining parts from the desired state) and report that as ProgressEvent.Identifier, recording it under the identifier property (PolicyArn) in the stored resource model so GetResource/ListResources/UpdateResource/DeleteResource resolve the same value.

Cause 2 of 2: CreateResource(TypeName="AWS::StepFunctions::StateMachine") answers ProgressEvent.Identifier = the StateMachineName from DesiredState, not the state machine ARN.
Failing check: cloudcontrol__iam__sfn.cloudcontrol-state-machine-definition-object-round-trip
AWS: Smithy as above (ProgressEvent$Identifier = "The primary identifier for the resource."). The AWS::StepFunctions::StateMachine registry schema gives primaryIdentifier /properties/Arn (readOnly), so the identifier is the state machine ARN of the form arn:<partition>:states:<region>:<account-id>:stateMachine:<StateMachineName> (fixed text "arn:", ":states:", ":stateMachine:"; partition, region, account id and StateMachineName come from the request and state). That identifier is what Step Functions' DescribeStateMachine/StartExecution accept as stateMachineArn, and GetResource must report it as the Arn property.
Emulator: the identifier reported is the StateMachineName (the replay records "ends with the name: True" with no ARN shape), so the check fails at the identifier assertion before the rest of the round trip (write-only Definition read back as DefinitionString, the /Definition patch, the execution, the delete) is reached.
Why: same client impact as Cause 1, and worse for this type because the identifier is also the handle for the DATA plane: a client that creates a state machine through Cloud Control and then calls sfn.StartExecution(stateMachineArn=<identifier>) or sfn.DescribeStateMachine(stateMachineArn=<identifier>) is passing a bare name where an ARN is required and is refused, even though Cloud Control reported SUCCESS.
Fix: same component as Cause 1 (crate::state::derive_identifier and the identifier recorded into ResourceEffect::Create), extended so AWS::StepFunctions::StateMachine resolves its primary identifier to the synthesized states ARN rather than to StateMachineName.

No open issue covers these two causes. The one open cloudcontrol issue, 20261102T041105Z-cloudcontrol-dynamodb-cloudcontrol-createresource-deleteresource-reach-a-terminal-success-without.md, is a DIFFERENT cause (resource effects are applied only to the cloudcontrol-local store and never provision the resource in the owning service, observed on AWS::DynamoDB::Table — a type whose primary identifier IS the TableName, so the identifier defect does not arise there). The checks here fail earlier, on the identifier itself, and would still fail with that issue fixed; conversely fixing the identifier here does not provision the native resource, so both of these checks additionally depend on that issue's cause for the rest of their assertions.

Traces: cloudcontrol__iam__sfn.cloudcontrol-iam-managed-policy-lifecycle-identifier-is-arn, cloudcontrol__iam__sfn.cloudcontrol-state-machine-definition-object-round-trip. Code citations: services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs lines 237-269 (parse DesiredState, then crate::state::derive_identifier to pick the identifier and its property) and lines 361-372 (the identifier recorded into RequestRecord/ResourceEffect::Create and reported through crate::state::progress_event); services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs lines 722-746 (identifier_property_candidates: "<Resource>Name", "<Resource>Id", "<Resource>Arn", "Name", "Id", "Arn" — most specific first, so ManagedPolicyName/StateMachineName win over the ARN) and lines 796-815 (derive_identifier).

## Evidence
Cause 1 — check cloudcontrol__iam__sfn.cloudcontrol-iam-managed-policy-lifecycle-identifier-is-arn (verdict: mismatch).
Expected (AWS): CreateResource(TypeName="AWS::IAM::ManagedPolicy", DesiredState={"ManagedPolicyName": <name>, "PolicyDocument": {...}, "Description": "..."}) reaches OperationStatus "SUCCESS" with ProgressEvent.Identifier == the policy ARN arn:aws:iam::<account-id>:policy/<name> (the type's primaryIdentifier /properties/PolicyArn); IAM GetPolicy(PolicyArn=<that identifier>) then resolves to PolicyName == <name> with DefaultVersionId "v1", and Cloud Control GetResource reports Properties.PolicyArn == that same identifier.
Actual (emulator):
  File "check_cloudcontrol__iam__sfn_cloudcontrol_iam_managed_policy_lifecycle_identifier_is_arn.py", line 65, in run
    assert identifier == expected_arn, (
  AssertionError: the primary identifier is not the policy ARN for the created name: shape ['sim2real-xsvc-ccmp-2975981c4cf5'], ends with the name: True
The diagnostic prints identifier.split(':')[:5]: it is a single element with no colons, i.e. ProgressEvent.Identifier is the bare ManagedPolicyName. (The name itself is a per-run generated value; the requirement is the ARN SHAPE arn:<partition>:iam::<account-id>:policy<path><ManagedPolicyName>, not that any particular name appears.)

Cause 2 — check cloudcontrol__iam__sfn.cloudcontrol-state-machine-definition-object-round-trip (verdict: mismatch).
Expected (AWS): CreateResource(TypeName="AWS::StepFunctions::StateMachine", DesiredState={"StateMachineName": <name>, "RoleArn": <role>, "Definition": {...}, "StateMachineType": "STANDARD"}) reaches "SUCCESS" with ProgressEvent.Identifier == arn:aws:states:<region>:<account-id>:stateMachine:<name> (the type's primaryIdentifier /properties/Arn); sfn DescribeStateMachine(stateMachineArn=<that identifier>) then answers name == <name> and the definition that was sent, and Cloud Control GetResource reports Properties.Arn == that identifier with the write-only Definition absent and DefinitionString carrying it.
Actual (emulator):
  File "check_cloudcontrol__iam__sfn_cloudcontrol_state_machine_definition_object_round_trip.py", line 45, in run
    assert identifier == expected_arn, (
  AssertionError: the primary identifier is not the state machine ARN for the created name (ends with the name: True)
The identifier equals the StateMachineName (ends-with-the-name is True and it is not an ARN), so no Step Functions call can be made with it.

Both failures are produced by one code path: crate::state::derive_identifier picks the first of identifier_property_candidates(type_name) present in DesiredState, and that list starts with "<ResourceSegment>Name" — "ManagedPolicyName" for AWS::IAM::ManagedPolicy and "StateMachineName" for AWS::StepFunctions::StateMachine — so a name-carrying desired state always wins over the type's real ARN primary identifier.

## Steps to Reproduce
Cause 1 (AWS::IAM::ManagedPolicy):
1. Boot the emulator and point botocore at it. cc = client("cloudcontrol"); iam = client("iam").
2. name = "<unique-name>"; desired = json.dumps({"ManagedPolicyName": name, "PolicyDocument": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": "s3:GetObject", "Resource": "arn:aws:s3:::example/*"}]}, "Description": "x"}).
3. ev = cc.create_resource(TypeName="AWS::IAM::ManagedPolicy", DesiredState=desired)["ProgressEvent"]; poll cc.get_resource_request_status(RequestToken=ev["RequestToken"])["ProgressEvent"] until OperationStatus is terminal (it becomes "SUCCESS").
4. Read ProgressEvent.Identifier.
   Observed: the bare ManagedPolicyName (no colons).
   Expected: arn:aws:iam::<account-id>:policy/<name>, and iam.get_policy(PolicyArn=<identifier>) resolving to that policy with DefaultVersionId "v1".
5. (Blocked by step 4) cc.get_resource(TypeName="AWS::IAM::ManagedPolicy", Identifier=<identifier>) must report Properties.PolicyArn == the identifier.

Cause 2 (AWS::StepFunctions::StateMachine):
1. cc = client("cloudcontrol"); sfn = client("stepfunctions"); create an IAM role trusted by states.amazonaws.com and take its ARN as role_arn.
2. name = "<unique-name>"; desired = json.dumps({"StateMachineName": name, "RoleArn": role_arn, "Definition": {"Comment": "v1", "StartAt": "P", "States": {"P": {"Type": "Pass", "Result": {"version": 1}, "End": True}}}, "StateMachineType": "STANDARD"}).
3. ev = cc.create_resource(TypeName="AWS::StepFunctions::StateMachine", DesiredState=desired)["ProgressEvent"]; poll to terminal SUCCESS.
4. Read ProgressEvent.Identifier.
   Observed: the bare StateMachineName.
   Expected: arn:aws:states:<region>:<account-id>:stateMachine:<name>, usable directly as sfn.describe_state_machine(stateMachineArn=<identifier>) and sfn.start_execution(stateMachineArn=<identifier>).
5. (Blocked by step 4) cc.get_resource(TypeName="AWS::StepFunctions::StateMachine", Identifier=<identifier>) must report Properties.Arn == the identifier, omit the write-only Definition and carry DefinitionString.
