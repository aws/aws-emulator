Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam.cfn-iam-role-path-change-replaces-the-role cloudformation__iam.cfn-iam-managed-policy-path-change-replaces-the-policy cloudformation__iam.cfn-iam-user-path-update-refreshes-getatt-arn cloudformation__iam.cfn-iam-user-and-role-create-failure-rolled-back-leaves-no-entity cloudformation__iam.deploy-role-with-unknown-aws-managed-policy-arn-rolls-back
Issue-Key: cloudformation__iam.cfn-iam-role-path-change-replaces-the-role|_update#11|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__iam.cfn-iam-managed-policy-path-change-replaces-the-policy|_update#10|AssertionError|assert stack["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__iam.cfn-iam-user-path-update-refreshes-getatt-arn|run#15|AssertionError|assert outputs["UserArn"] == user["Arn"], (
Issue-Key: cloudformation__iam.cfn-iam-user-and-role-create-failure-rolled-back-leaves-no-entity|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation__iam.deploy-role-with-unknown-aws-managed-policy-arn-rolls-back|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# cloudformation: IAM resources are never REPLACED on an immutable-property update, GetAtt outputs go stale after an update, a rolled-back create leaves the IAM user behind, and an unknown AWS-managed ManagedPolicyArns value reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Four net-new root causes in CloudFormation's handling of `AWS::IAM::*` resources. These are distinct from the already-filed `cloudformation__iam` issues (20261117T135333Z-cloudformation-iam-4-net-new-causes-a-group, 20261109T163411Z-cloudformation-iam-4-net-new-causes-capability-iam, 20261111T103332Z-cloudformation-iam-an-update-that-changes-aws-iam), which own other checks in the same heading.

=== Cause 1 of 4: an update that changes an immutable IAM property is refused `NotImplemented` instead of replacing the resource ===
AWS: `Path` on `AWS::IAM::Role` and on `AWS::IAM::ManagedPolicy` is an update-requires-REPLACEMENT property. `UpdateStack` therefore creates a new physical resource under the new path, points the logical id at it, deletes the old one, and the stack settles `UPDATE_COMPLETE` (CloudFormation User Guide, "Update behaviors of stack resources" -> Replacement; AWS::IAM::Role / AWS::IAM::ManagedPolicy resource references, `Path`: "Update requires: Replacement").
Emulator: the update fails the resource and the stack settles `UPDATE_ROLLBACK_COMPLETE` with StatusReason `The following resource(s) failed to update: [Role].` and the per-resource reason `NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is.` (same wording with `AWS::IAM::ManagedPolicy`). No `NotImplemented` reason is part of any AWS update path; the operation itself is implemented, it just never performs a replacement.
Why: any template change that moves a role or a managed policy between paths — or changes any replacement-only IAM property — rolls the whole stack back, so a client cannot evolve an IAM stack at all and sees a reason string AWS never emits.
Fix: the CloudFormation update engine's resource-diff/replacement classification for `AWS::IAM::Role` and `AWS::IAM::ManagedPolicy` (the in-place-only update handler for those types).
Traces: cloudformation__iam.cfn-iam-role-path-change-replaces-the-role, cloudformation__iam.cfn-iam-managed-policy-path-change-replaces-the-policy.

=== Cause 2 of 4: `Fn::GetAtt` outputs are not recomputed after an update that changes the attribute ===
AWS: `Fn::GetAtt <User>.Arn` resolves to the user's CURRENT ARN. When an update changes `AWS::IAM::User.Path`, IAM's `GetUser` ARN and the stack output must agree after `UPDATE_COMPLETE` (Smithy: iam `User.Arn`; CloudFormation AWS::IAM::User return values, `Arn`).
Emulator: IAM moves the user (its `Arn` reports the new path) but the stack's `Outputs` still carry the ARN computed at create time, i.e. the OLD path. The two disagree: output `.../user/sim2real/a/<name>` vs IAM's `.../user/sim2real/b/<name>` (the `a`/`b` path segments and the generated name are this run's; the requirement is that the output equals IAM's current ARN).
Why: anything consuming a stack output or a cross-stack export — an IAM policy that names the ARN, a trust policy, a deploy script — keeps using an ARN that no longer resolves.
Fix: the CloudFormation update engine's attribute/output resolution — recompute `Fn::GetAtt` values from the updated resource state before publishing `Outputs`.
Traces: cloudformation__iam.cfn-iam-user-path-update-refreshes-getatt-arn.

=== Cause 3 of 4: a rolled-back create leaves the IAM user it created behind ===
AWS: when a create fails and the stack reaches `ROLLBACK_COMPLETE`, every resource the failed create made is deleted. A subsequent `GetUser` on the rolled-back `AWS::IAM::User` must be refused `NoSuchEntity` with `The user with name <UserName> cannot be found.` (Smithy: iam `GetUser` error `NoSuchEntityException`, httpError 404; CloudFormation User Guide, "Stack failure options" -> ROLLBACK).
Emulator: after the rollback, `GetUser` answers HTTP 200 with the full `User` structure — the user was never deleted. The role in the same template was cleaned up; the user was not.
Why: a failed deployment leaves a live IAM principal in the account. A retry of the same template then collides on the name, and the account carries a credential-bearing entity nobody owns.
Fix: the CloudFormation rollback path's delete step for `AWS::IAM::User` (the resource handler's delete invoked during ROLLBACK).
Traces: cloudformation__iam.cfn-iam-user-and-role-create-failure-rolled-back-leaves-no-entity.

=== Cause 4 of 4: `AWS::IAM::Role` with a `ManagedPolicyArns` entry naming no published AWS-managed policy reaches CREATE_COMPLETE ===
AWS: `CreateRole` + `AttachRolePolicy` for an `arn:aws:iam::aws:policy/<name>` that resolves to no policy is refused `NoSuchEntity` / `Policy <PolicyArn> does not exist or is not attachable.` (Smithy: iam `AttachRolePolicy` error `NoSuchEntityException`, httpError 404). CloudFormation surfaces that as a resource `CREATE_FAILED` and the stack settles `ROLLBACK_COMPLETE`.
Emulator: the role resource and the stack both reach `CREATE_COMPLETE` with the bogus managed-policy ARN attached.
Why: a typo in a `ManagedPolicyArns` entry deploys silently, so a role a client believes is bounded by an AWS-managed policy carries no such grant and the stack reports success.
Fix: the AWS-managed policy ARN resolution the attach path consults (see the open issue 20261126T150000Z-iam-3-causes-still-open-access-key-id-class-denial-trailer-and-unpublished-policy-arn.md, whose cause 3 analyses the same resolution for `AttachUserPolicy`; this check reaches it through CloudFormation's `AWS::IAM::Role` handler and the stack's terminal status is the additional requirement).
Traces: cloudformation__iam.deploy-role-with-unknown-aws-managed-policy-arn-rolls-back.

## Evidence
From one replay of the behavioral suite (service heading `### cloudformation__iam`, 5/23 passed; the other 18 failures are owned by already-filed issues).

[mismatch] cloudformation__iam.cfn-iam-role-path-change-replaces-the-role
  AssertionError: the update settled as 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Role]. '); failed resources: [('Role', 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::Role in place; the resource was left as it is.')]
  Expected: StackStatus == 'UPDATE_COMPLETE' with the role replaced under the new path.

[mismatch] cloudformation__iam.cfn-iam-managed-policy-path-change-replaces-the-policy
  AssertionError: the update settled as 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Policy]. '); failed resources: [('Policy', 'NotImplemented: this emulator does not change the properties of an existing AWS::IAM::ManagedPolicy in place; the resource was left as it is.')]

[mismatch] cloudformation__iam.cfn-iam-user-path-update-refreshes-getatt-arn
  AssertionError: GetAtt User.Arn after the move was 'arn:aws:iam::<account>:user/sim2real/a/<generated-name>', IAM's Arn is 'arn:aws:iam::<account>:user/sim2real/b/<generated-name>'
  (the account, the path segments and the generated name are per-run; the requirement is output == IAM's current Arn)

[mismatch] cloudformation__iam.cfn-iam-user-and-role-create-failure-rolled-back-leaves-no-entity
  AssertionError: expected the call to be refused, but it succeeded: {'User': {'Path': '/', 'UserName': '<generated>', 'UserId': '...', 'Arn': '...', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected: GetUser refused 404 / NoSuchEntity / 'The user with name <UserName> cannot be found.'

[mismatch] cloudformation__iam.deploy-role-with-unknown-aws-managed-policy-arn-rolls-back
  AssertionError: stack naming 'Sim2realNoSuchManagedPolicy' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE: [('Role', 'CREATE_COMPLETE'), ('<stack>', 'CREATE_COMPLETE')]

## Steps to Reproduce
1. Cause 1: CreateStack with an `AWS::IAM::Role` (and separately an `AWS::IAM::ManagedPolicy`) at `Path: /sim2real/a/`, CAPABILITY_NAMED_IAM. UpdateStack with the identical template except `Path: /sim2real/b/`. Expect UPDATE_COMPLETE with a new physical resource under /b/ and the old one deleted; observed UPDATE_ROLLBACK_COMPLETE with a `NotImplemented:` resource reason.
2. Cause 2: CreateStack with an `AWS::IAM::User` at `Path: /sim2real/a/` and an Output `Fn::GetAtt User.Arn`. UpdateStack moving it to `/sim2real/b/`, then compare DescribeStacks Outputs against iam GetUser's `User.Arn` — they must be equal; observed the output keeps the /a/ ARN.
3. Cause 3: CreateStack (OnFailure=ROLLBACK) with an `AWS::IAM::User` plus a resource guaranteed to fail; wait for ROLLBACK_COMPLETE, then GetUser on the user's name — expect 404 NoSuchEntity; observed 200 with the user.
4. Cause 4: CreateStack with an `AWS::IAM::Role` whose `ManagedPolicyArns` contains `arn:aws:iam::aws:policy/Sim2realNoSuchManagedPolicy`; expect the role CREATE_FAILED and the stack ROLLBACK_COMPLETE; observed both CREATE_COMPLETE.
