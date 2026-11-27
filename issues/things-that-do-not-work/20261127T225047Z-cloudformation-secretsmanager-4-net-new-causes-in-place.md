Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager.cfn-secret-replica-region-added-by-stack-update cloudformation__secretsmanager.deploy-drift-skips-writeonly-secretstring-but-flags-description cloudformation__secretsmanager.deploy-generate-secret-string-is-write-only-and-served-as-the-generated-value cloudformation__secretsmanager.cfn-hosted-rotation-transform-deploys-a-rotation-schedule cloudformation__secretsmanager.deploy-rotationschedule-second-schedule-on-same-secret-rejected
Issue-Key: cloudformation__secretsmanager.cfn-secret-replica-region-added-by-stack-update|run#15|AssertionError|assert st["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__secretsmanager.deploy-drift-skips-writeonly-secretstring-but-flags-description|run#22|AssertionError|assert "SecretString" not in expected and "SecretString" not in actual, (
Issue-Key: cloudformation__secretsmanager.deploy-generate-secret-string-is-write-only-and-served-as-the-generated-value|run#26|AssertionError|assert "GenerateSecretString" not in props and "SecretString" not in props, (
Issue-Key: cloudformation__secretsmanager.cfn-hosted-rotation-transform-deploys-a-rotation-schedule|run#7|AssertionError|assert st["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation__secretsmanager.deploy-rotationschedule-second-schedule-on-same-secret-rejected|run#19|AssertionError|assert refused == "UPDATE_ROLLBACK_COMPLETE", f"update reached {refused!r}: {failures!r}"
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + secretsmanager: 4 net-new causes — in-place Secret updates are NotImplemented, write-only members leak into drift, the hosted-rotation transform is refused, and a second RotationSchedule on one secret is accepted

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSecret

## Description
Four net-new causes in CloudFormation's Secrets Manager surface.

Cause 1 of 4: any in-place property update of AWS::SecretsManager::Secret is refused as not implemented.
AWS: mutable properties of the resource — including adding a region to ReplicaRegions — are applied in place (UpdateSecret / ReplicateSecretToRegions); the resource reports UPDATE_COMPLETE, the secret keeps its ARN, and the new replica is readable.
Emulator: the resource reports UPDATE_FAILED with the emulator-authored reason `NotImplemented: this emulator does not change the properties of an existing AWS::SecretsManager::Secret in place; the resource was left as it is.` and the stack settles UPDATE_ROLLBACK_COMPLETE.
Why: no stack that owns a secret can ever be updated — a description change, a replica addition or a tag edit all fail, and the stack is left rolled back. The open issue 20261117T143604Z-cloudformation-secretsmanager-2-net-new-causes-the-colliding.md carries the tag-update check that shows the same reason; this cause is the shared update path and the replica-region check is not covered by it.
Fix: the AWS::SecretsManager::Secret handler's update path — implement the in-place branch instead of returning NotImplemented.

Cause 2 of 4: write-only Secret members appear in the drift read's ExpectedProperties.
AWS: SecretString and GenerateSecretString are write-only, so DetectStackResourceDrift omits them from both property sets; a secret whose Description drifted is reported DRIFTED on Description alone, and the two sets differ only in that member.
Emulator: ExpectedProperties carries SecretString / GenerateSecretString while ActualProperties does not, so the write-only member is reported as a difference.
Why: every drift gate over a stack that owns a secret reports spurious drift on the secret's value, and the drift report leaks the template's write-only material into the comparison.
Fix: the write-only member projection in the AWS::SecretsManager::Secret drift/read path.

Cause 3 of 4: the hosted-rotation macro transform is refused.
AWS: a template declaring `Transform: AWS::SecretsManager-2024-09-16` with a HostedRotationLambda on an AWS::SecretsManager::RotationSchedule is expanded and deployed; the stack reaches CREATE_COMPLETE and DescribeSecret reports RotationEnabled with the rotation rules.
Emulator: the stack settles CREATE_FAILED with the emulator-authored reason `the template declares the macro transform(s) [AWS::SecretsManager-2024-09-16], which run as Lambda functions outside this emulator` — an explicit refusal of a supported transform, and not a message AWS ever emits.
Why: hosted rotation is the documented way to attach rotation without writing a Lambda; such templates cannot be deployed at all and the failure reason is unrecognisable to a client.
Fix: the transform registry / macro expansion for AWS::SecretsManager-2024-09-16 (it is an AWS-owned transform, not a customer macro).

Cause 4 of 4: a second AWS::SecretsManager::RotationSchedule on the SAME secret is accepted.
AWS: a secret can carry only one rotation schedule; the update that adds a second one fails the resource and the stack does not reach UPDATE_COMPLETE.
Emulator: the update reaches UPDATE_COMPLETE with no failed resources.
Why: the caller is told two conflicting rotation schedules are attached to one secret, so the rotation configuration the stack reports is one AWS would never hold.
Fix: the AWS::SecretsManager::RotationSchedule handler's create path — refuse a schedule for a secret that already has one.

## Evidence
Cause 1: "adding ReplicaRegions ended UPDATE_ROLLBACK_COMPLETE: [('AppSecret', 'NotImplemented: this emulator does not change the properties of an existing AWS::SecretsManager::Secret in place; the resource was left as it is.')]".
Cause 2: "SecretString appears in drift properties: expected=['Description','Name','SecretString'] actual=['Description','Name']"; "the expected properties of the read expose a write-only property: ['Description','GenerateSecretString','Name','Tags']".
Cause 3: "stack ended CREATE_FAILED ('the template declares the macro transform(s) [AWS::SecretsManager-2024-09-16], which run as Lambda functions outside this emulator')".
Cause 4: "update reached 'UPDATE_COMPLETE': []" (expected the update to fail).

## Steps to Reproduce
Cause 1: deploy an AWS::SecretsManager::Secret, then UpdateStack adding a ReplicaRegions entry — expect UPDATE_COMPLETE.
Cause 2: deploy a secret with SecretString (and separately with GenerateSecretString), change its Description out of band, then DetectStackResourceDrift and compare ExpectedProperties/ActualProperties.
Cause 3: CreateStack with `Transform: AWS::SecretsManager-2024-09-16` and a RotationSchedule carrying a HostedRotationLambda — expect CREATE_COMPLETE.
Cause 4: deploy a secret with one RotationSchedule, then UpdateStack adding a second RotationSchedule for the same secret — expect the update to fail.
