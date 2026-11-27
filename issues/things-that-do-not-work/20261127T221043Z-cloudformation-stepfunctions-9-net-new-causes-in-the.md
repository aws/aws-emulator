Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.deploy-cfn-activity-encryption-change-plans-replacement cloudformation__stepfunctions.deploy-state-machine-type-change-replaces-machine-and-sync-start-rejected-until-express cloudformation__stepfunctions.deploy-cfn-alias-deployment-preference-routes-all-traffic-and-is-absent-from-drift-read cloudformation__stepfunctions.deploy-cfn-state-machine-version-new-revision-replaces-version cloudformation__stepfunctions.deploy-cfn-getatt-of-write-only-definition-rejected-while-read-only-arn-resolves cloudformation__stepfunctions.deploy-getatt-writable-state-machine-name-property-rejected-and-rolled-back cloudformation__stepfunctions.deploy-read-only-name-property-ignored-while-state-machine-name-is-applied cloudformation__stepfunctions.deploy-getatt-name-matches-generated-and-explicit-state-machine-names cloudformation__stepfunctions.deploy-cfn-state-machine-alias-rename-replaces-alias-and-routes-new-arn
Issue-Key: cloudformation__stepfunctions.deploy-cfn-activity-encryption-change-plans-replacement|run#17|AssertionError|assert (worker["Action"], worker["Replacement"], worker["PhysicalResourceId"]) == (
Issue-Key: cloudformation__stepfunctions.deploy-state-machine-type-change-replaces-machine-and-sync-start-rejected-until-express|run#23|AssertionError|assert (machine_change["Action"], machine_change["Replacement"]) == ("Modify", "True"), (
Issue-Key: cloudformation__stepfunctions.deploy-cfn-alias-deployment-preference-routes-all-traffic-and-is-absent-from-drift-read|run#9|AssertionError|assert made["StackStatus"] == "CREATE_COMPLETE", f"stack ended {made['StackStatus']!r}"
Issue-Key: cloudformation__stepfunctions.deploy-cfn-state-machine-version-new-revision-replaces-version|run#8|AssertionError|assert created["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation__stepfunctions.deploy-cfn-getatt-of-write-only-definition-rejected-while-read-only-arn-resolves|run#21|AssertionError|assert ended["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__stepfunctions.deploy-getatt-writable-state-machine-name-property-rejected-and-rolled-back|run#14|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"GetAtt StateMachineName stack ended {bad_status!r}"
Issue-Key: cloudformation__stepfunctions.deploy-read-only-name-property-ignored-while-state-machine-name-is-applied|run#7|AssertionError|assert status == "CREATE_COMPLETE", f"stack ended {status!r}, expected CREATE_COMPLETE"
Issue-Key: cloudformation__stepfunctions.deploy-getatt-name-matches-generated-and-explicit-state-machine-names|run#13|AssertionError|assert re.fullmatch(r"Unnamed-[A-Za-z0-9]+", generated), (
Issue-Key: cloudformation__stepfunctions.deploy-cfn-state-machine-alias-rename-replaces-alias-and-routes-new-arn|run#36|AssertionError|assert refused == (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# cloudformation__stepfunctions: 9 net-new causes in the AWS::StepFunctions::* resource providers — replacement planning, Version/Alias properties, and read-only/write-only attribute handling

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 9: a change to an AWS::StepFunctions::Activity's EncryptionConfiguration is planned as a non-replacing Modify.
AWS: EncryptionConfiguration is create-only on AWS::StepFunctions::Activity, so a change to it is planned with Target.RequiresRecreation 'Always' and the change's Replacement 'True'; executing it creates a new activity ARN. Recorded behavior only for the exact change-set shape.
Emulator: the planned change is {'Action': 'Modify', 'Replacement': 'False', Details: [{Target: {Attribute: 'Properties', Name: 'EncryptionConfiguration', RequiresRecreation: 'Never'}}]}.
Why: a client reading the change set is told an encryption change is applied in place, so it does not expect a new activity ARN and does not re-point its workers.
Fix: the AWS::StepFunctions::Activity resource provider's property schema (create-only markers) feeding change-set planning.
Traces: cloudformation__stepfunctions.deploy-cfn-activity-encryption-change-plans-replacement.

Cause 2 of 9: a change to AWS::StepFunctions::StateMachine's StateMachineType is planned as a non-replacing Modify.
AWS: StateMachineType is create-only, so switching STANDARD↔EXPRESS is planned as Modify with Replacement 'True' and executing it replaces the machine (after which StartSyncExecution is accepted only on the EXPRESS machine). Recorded behavior only.
Emulator: planned as 'Modify' with Replacement 'False'.
Why: a client is told the type flips in place; the new ARN and the changed StartSyncExecution admission are a surprise.
Fix: the AWS::StepFunctions::StateMachine provider's property schema (create-only markers) feeding change-set planning.
Traces: cloudformation__stepfunctions.deploy-state-machine-type-change-replaces-machine-and-sync-start-rejected-until-express.

Cause 3 of 9: AWS::StepFunctions::StateMachineAlias with a DeploymentPreference cannot be created — the stack rolls back.
AWS: an alias declared with DeploymentPreference {StateMachineVersionArn, Type: ALL_AT_ONCE} is created, the stack reaches CREATE_COMPLETE, every execution started through the alias runs the named version (DescribeExecution reports that stateMachineVersionArn and the alias ARN), and DeploymentPreference is write-only, so a drift read reports only {'Name': <alias>}. Recorded behavior only.
Emulator: the stack ends 'ROLLBACK_COMPLETE', so no alias exists at all.
Why: the documented ALL_AT_ONCE/gradual deployment form of a Step Functions alias cannot be deployed by CloudFormation.
Fix: the AWS::StepFunctions::StateMachineAlias resource provider — accept DeploymentPreference, apply it as routing, and keep it out of the read-back/drift properties.
Traces: cloudformation__stepfunctions.deploy-cfn-alias-deployment-preference-routes-all-traffic-and-is-absent-from-drift-read.

Cause 4 of 9: AWS::StepFunctions::StateMachineVersion carrying StateMachineRevisionId fails to create — the stack rolls back.
AWS: a version resource whose properties are {StateMachineArn: {Ref: <machine>}, StateMachineRevisionId: {Fn::GetAtt: [<machine>, StateMachineRevisionId]}} is created (the revision id pins which revision is published), and a new revision of the machine replaces the version resource. Recorded behavior only.
Emulator: the stack create ends 'ROLLBACK_COMPLETE' with 'The following resource(s) failed to create: [Snapshot]. '.
Why: the documented way to publish a pinned state-machine version from CloudFormation cannot be deployed, so versioned deployments are impossible.
Fix: the AWS::StepFunctions::StateMachineVersion resource provider (StateMachineRevisionId handling in its create path).
Traces: cloudformation__stepfunctions.deploy-cfn-state-machine-version-new-revision-replaces-version.

Cause 5 of 9: Fn::GetAtt of the write-only Definition attribute is accepted.
AWS: Definition is a write-only property of AWS::StepFunctions::StateMachine, so a template whose Output reads {Fn::GetAtt: [<machine>, Definition]} is refused and the stack does not reach CREATE_COMPLETE, while {Fn::GetAtt: [<machine>, Arn]} resolves normally.
Emulator: the stack with the GetAtt on Definition ends 'CREATE_COMPLETE'.
Why: a template that reads a write-only attribute deploys here and fails on AWS; and the value the emulator hands out for a write-only attribute is not something AWS ever exposes.
Fix: the AWS::StepFunctions::StateMachine provider's attribute schema — only read-only attributes may be resolved by Fn::GetAtt.
Traces: cloudformation__stepfunctions.deploy-cfn-getatt-of-write-only-definition-rejected-while-read-only-arn-resolves.

Cause 6 of 9: Fn::GetAtt of the writable StateMachineName property is accepted.
AWS: StateMachineName is a writable property, not a readable attribute, so a template whose Output reads {Fn::GetAtt: [<machine>, StateMachineName]} fails the stack, which rolls back to ROLLBACK_COMPLETE and carries no Outputs; {Fn::GetAtt: [<machine>, Name]} is the readable one.
Emulator: that stack ends 'CREATE_COMPLETE'.
Why: same as cause 5 — the emulator resolves an attribute AWS rejects, so templates that are broken on AWS pass here.
Fix: the AWS::StepFunctions::StateMachine provider's attribute schema.
Traces: cloudformation__stepfunctions.deploy-getatt-writable-state-machine-name-property-rejected-and-rolled-back.

Cause 7 of 9: a template value for the read-only Name property fails the stack instead of being ignored.
AWS: Name is read-only on AWS::StepFunctions::StateMachine. A resource given only {"Name": <string>} is created with a CloudFormation-generated name (the Name value is ignored), the stack reaches CREATE_COMPLETE, and a sibling resource given {"StateMachineName": <the same string>} is the one that actually gets that name.
Emulator: the stack ends 'ROLLBACK_COMPLETE'.
Why: templates that carry a stray read-only Name — which AWS silently ignores — cannot be deployed at all.
Fix: the AWS::StepFunctions::StateMachine provider's property schema (ignore read-only properties on create instead of failing).
Traces: cloudformation__stepfunctions.deploy-read-only-name-property-ignored-while-state-machine-name-is-applied.

Cause 8 of 9: the name CloudFormation generates for an unnamed state machine has the wrong form.
AWS: when the template names no StateMachineName, the generated name is '<LogicalId>-<random suffix>' (for logical id 'Unnamed': 'Unnamed-<suffix>'), and Fn::GetAtt Name, the Ref'd ARN's resource part and DescribeStateMachine's name all agree with it.
Emulator: the generated name is '<StackName>-<LogicalId>-<suffix>' — e.g. 'sim2real-xsvc-sfnname<...>-Unnamed-A0527F759951'.
Why: a client that derives or matches the machine name from the logical id (or that is near the 80-character Name limit) gets a different name here than on AWS.
Fix: the AWS::StepFunctions::StateMachine provider's physical-id/name generation.
Traces: cloudformation__stepfunctions.deploy-getatt-name-matches-generated-and-explicit-state-machine-names.

Cause 9 of 9: StartExecution against a deleted alias ARN answers the state-machine message, not the alias one.
AWS: after an alias is replaced/removed, DescribeStateMachineAlias answers ResourceNotFound "Resource not found: '<alias arn>'" and StartExecution on that alias ARN answers StateMachineDoesNotExist with the message "State Machine Alias Does Not Exist: '<alias arn>'" — the fixed text is 'State Machine Alias Does Not Exist: ' and the quoted ARN is the one in the request.
Emulator: StateMachineDoesNotExist with "State Machine Does Not Exist: '<alias arn>'" — the alias wording is lost.
Why: a client that distinguishes a missing alias from a missing state machine by the message (the error code is the same for both) cannot.
Fix: StartExecution's not-found path in the Step Functions handler — pick the alias message when the ARN names an alias.
Traces: cloudformation__stepfunctions.deploy-cfn-state-machine-alias-rename-replaces-alias-and-routes-new-arn.

## Evidence
Cause 1: change Details carried RequiresRecreation 'Never', Replacement 'False'.
Cause 2: "the type change is planned as 'Modify' with Replacement 'False', expected Modify with Replacement True".
Cause 3: "stack ended 'ROLLBACK_COMPLETE'".
Cause 4: "stack create ended 'ROLLBACK_COMPLETE': 'The following resource(s) failed to create: [Snapshot]. '".
Cause 5: "a template reading write-only Definition through Fn::GetAtt ended 'CREATE_COMPLETE'".
Cause 6: "GetAtt StateMachineName stack ended 'CREATE_COMPLETE'" (expected ROLLBACK_COMPLETE).
Cause 7: "stack ended 'ROLLBACK_COMPLETE', expected CREATE_COMPLETE".
Cause 8: "generated state machine name 'sim2real-xsvc-sfnname3765bc1067ef-Unnamed-A0527F759951' is not '<LogicalId>-<suffix>'" (check asserts re.fullmatch(r"Unnamed-[A-Za-z0-9]+")).
Cause 9: expected ('StateMachineDoesNotExist', "State Machine Alias Does Not Exist: '<arn>'"), got ('StateMachineDoesNotExist', "State Machine Does Not Exist: '<arn>'").

## Steps to Reproduce
1. Create a change set that alters an AWS::StepFunctions::Activity EncryptionConfiguration, and one that alters a StateMachine's StateMachineType; read the planned Replacement/RequiresRecreation (causes 1-2).
2. Deploy a stack with AWS::StepFunctions::StateMachineAlias + DeploymentPreference, and one with AWS::StepFunctions::StateMachineVersion + StateMachineRevisionId (causes 3-4).
3. Deploy templates whose Outputs use Fn::GetAtt on Definition and on StateMachineName (causes 5-6), and one that sets the read-only Name property (cause 7).
4. Deploy a state machine with no StateMachineName and read Fn::GetAtt Name (cause 8).
5. Rename a StateMachineAlias in a stack update and call StartExecution on the old alias ARN (cause 9).
6. Or replay the check ids in trace_ids.
