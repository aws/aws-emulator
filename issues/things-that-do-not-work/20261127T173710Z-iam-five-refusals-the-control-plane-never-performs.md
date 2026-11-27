Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.attach-role-and-group-policy-unknown-aws-managed-arn-rejected iam.create-role-reserved-service-role-path-and-name iam.not-principal-in-a-role-trust-policy iam.delete-user-with-enabled-mfa-device-rejected iam.delete-user-with-ssh-public-key-rejected iam.pass-role-passed-to-service-condition-on-create-function
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.attach-role-and-group-policy-unknown-aws-managed-arn-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: iam.create-role-reserved-service-role-path-and-name|run#9|AssertionError|assert observed == want, (
Issue-Key: iam.not-principal-in-a-role-trust-policy|run#8|AssertionError|assert observed == _EXPECTED_CREATE, (
Issue-Key: iam.delete-user-with-enabled-mfa-device-rejected|_error#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: iam.delete-user-with-ssh-public-key-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: iam.pass-role-passed-to-service-condition-on-create-function|run#34|AssertionError|assert not wrong, "; ".join(wrong).replace(account, "<account>")
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# iam: five refusals the control plane never performs — unknown AWS-managed PolicyArn attaches, reserved service-role path/name prefix accepted, NotPrincipal accepted in a trust policy, DeleteUser not blocked by dependents, and iam:PassedToService ignored

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Five net-new root causes, all of the same family: a call AWS refuses is served 200. They are distinct from the already-filed `iam` issues that own other checks in this heading (20261126T141645Z-iam-2-net-new-causes-the-accessdenied-authorization, 20261126T150000Z-iam-3-causes-still-open-..., 20261102T091623Z-iam-10-net-new-refusal-message-conflict-precedence, 20261027T214234Z-iam-6-net-new-behavioral-root-causes-enablemfadevice, 20261123T114935Z-iam-2-net-new-causes-simulateprincipalpolicy-reports-a).

=== Cause 1 of 5: AttachRolePolicy / AttachGroupPolicy accept an aws-namespace PolicyArn that names no attachable policy ===
AWS: an attach whose `PolicyArn` resolves to no policy is refused HTTP 404 / `NoSuchEntity` with `Policy <PolicyArn> does not exist or is not attachable.`, and nothing is attached (Smithy: iam `AttachRolePolicy` / `AttachGroupPolicy` error `NoSuchEntityException`, httpError 404). This holds both for a name AWS does not publish and for a REAL policy's ARN given under the wrong path (`arn:<partition>:iam::aws:policy/<Name>` when the policy actually lives at `arn:<partition>:iam::aws:policy/service-role/<Name>`). The same ARN under its correct `/service-role/` path then attaches to both a role and a group. Fixed text: `Policy `, ` does not exist or is not attachable.`; the variable part is the `PolicyArn` the request sent.
Emulator: both attaches answer HTTP 200 and the attachment is recorded.
Why: a typo or a wrong-path ARN silently "grants" a policy that does not exist, so a role or group a client believes is permissioned has no such grant and nothing reports it.
Note: this is the SAME resolution analysed in the open issue 20261126T150000Z-iam-3-causes-still-open-access-key-id-class-denial-trailer-and-unpublished-policy-arn.md (its cause 3, for `AttachUserPolicy`). That issue does not carry this check's replay key; the wrong-path leg and the role/group entry points are additional to it.
Fix: the AWS-managed-policy catalog resolution the attach handlers consult — make it authoritative for the `iam::aws:policy` namespace, including the policy's path, so the existing `policy_not_attachable` refusal fires.
Traces: iam.attach-role-and-group-policy-unknown-aws-managed-arn-rejected.

=== Cause 2 of 5: CreateRole accepts the reserved service-linked-role path and the reserved name prefix ===
AWS: `CreateRole` with a `Path` beginning `/aws-service-role` is refused `InvalidInput` / `Paths beginning with '/aws-service-role' are reserved for AWS Service Linked Roles`, and a `RoleName` beginning `AWSServiceRoleFor` is refused `InvalidInput` / `Role name prefix AWSServiceRoleFor can only be used for AWS Service Linked Roles` (Smithy: iam `CreateRole` error `InvalidInputException`, httpError 400). Both messages are entirely fixed text — nothing in them comes from the request. An ordinary path and name in the same shape succeed, which is the positive control.
Emulator: both reserved forms answer 200 and the role is created.
Why: a caller can mint roles that impersonate AWS's service-linked roles, in the path and under the name prefix AWS reserves. Code that distinguishes a service-linked role from a customer role by path or name prefix is then wrong, and a later real service-linked-role creation collides.
Fix: `CreateRole`'s input validation (the reserved-path and reserved-name-prefix checks, before the role is stored).
Traces: iam.create-role-reserved-service-role-path-and-name.

=== Cause 3 of 5: CreateRole accepts an AssumeRolePolicyDocument containing NotPrincipal ===
AWS: a role trust policy may not use `NotPrincipal`. `CreateRole` refuses both forms with `MalformedPolicyDocument` (Smithy: iam `CreateRole` error `MalformedPolicyDocumentException`, httpError 400): `Allow` with `NotPrincipal` gives `Allow with NotPrincipal is not allowed.`, and `Deny` with `NotPrincipal` gives `AssumeRole policy must not contain NotPrincipal field.`. Both messages are fixed text.
Emulator: both documents are accepted and the role is created (`'allow-not-principal': 'OK'`, `'deny-not-principal': 'OK'`).
Why: a trust policy AWS rejects as malformed is stored, so the emulator will later make trust decisions from a document that could not exist in AWS — a role appears assumable (or deny-protected) by a rule real IAM would never have accepted.
Fix: the trust-policy document validator used by `CreateRole` / `UpdateAssumeRolePolicy` (reject `NotPrincipal` in an AssumeRole policy, with the per-effect message above).
Traces: iam.not-principal-in-a-role-trust-policy.

=== Cause 4 of 5: DeleteUser is not refused while the user still has dependents ===
AWS: `DeleteUser` is refused HTTP 409 / `DeleteConflict` while the user still holds objects that must be removed first (Smithy: iam `DeleteUser` error `DeleteConflictException`, httpError 409), and the user and its dependents all survive the refusal. The message depends on the dependent class: an ENABLED virtual MFA device gives `Cannot delete entity, must remove tokens from principal first.`; an SSH public key gives `Cannot delete entity, must remove referenced objects first.`. Both are fixed text. Once the dependent is removed (DeactivateMFADevice / DeleteSSHPublicKey) the identical `DeleteUser` must succeed, after which `GetUser` is `NoSuchEntity` / `The user with name <UserName> cannot be found.`
Emulator: `DeleteUser` answers HTTP 200 in both situations — the dependency check does not exist.
Why: a user is deleted out from under a live MFA device and an SSH public key, so the account is left with orphaned credentials and a client never learns it had to clean them up first. IAM's documented delete-ordering contract is silently gone.
Fix: `DeleteUser`'s pre-delete dependency scan (enabled MFA devices, SSH public keys, and the other referenced-object classes) and the `DeleteConflict` refusal it must raise.
Traces: iam.delete-user-with-enabled-mfa-device-rejected, iam.delete-user-with-ssh-public-key-rejected.

=== Cause 5 of 5: the iam:PassedToService condition key on iam:PassRole is not evaluated ===
AWS: a grant of `iam:PassRole` conditioned on `iam:PassedToService` only authorizes passing the role to the named service principal. A `lambda:CreateFunction` that passes the role while the condition does not hold is refused with the AccessDenied class for the passing service, and the function is NOT created — a follow-up `GetFunction` must be not-found (Smithy: iam `SimulatePrincipalPolicy`/`PassRole` condition keys; IAM User Guide, "iam:PassedToService").
Emulator: the call that must be refused answers `('OK', None)`; the function is created. The check's third leg then sees the creation it expected to be the FIRST success refused with `ResourceConflictException` / `Function already exist: <name>` — a consequence of the first acceptance, not a separate defect.
Why: `iam:PassedToService` is the standard way to stop a caller handing a privileged role to the wrong service. Against the emulator that boundary does not exist, so a scoped PassRole grant authorizes passing the role anywhere.
Fix: the IAM authorization gate's request-context population and condition evaluation for `iam:PassRole` — supply `iam:PassedToService` from the passing service's principal and evaluate it before the pass is authorized.
Traces: iam.pass-role-passed-to-service-condition-on-create-function.

## Evidence
From one replay of the behavioral suite (service heading `### iam`; the other failures under that heading are owned by already-filed issues).

[mismatch] iam.attach-role-and-group-policy-unknown-aws-managed-arn-rejected
  AssertionError: expected the call to be rejected, but it succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected for both `arn:<partition>:iam::aws:policy/<absent-name>` and `arn:<partition>:iam::aws:policy/<RealName>` (real policy, wrong path): ('NoSuchEntity', 'Policy <arn> does not exist or is not attachable.') on AttachRolePolicy AND AttachGroupPolicy, with nothing attached.

[mismatch] iam.create-role-reserved-service-role-path-and-name
  AssertionError: CreateRole on reserved forms: expected {'ordinary': 'OK', 'reserved-path': ('InvalidInput', "Paths beginning with '/aws-service-role' are reserved for AWS Service Linked Roles"), 'reserved-name-prefix': ('InvalidInput', 'Role name prefix AWSServiceRoleFor can only be used for AWS Service Linked Roles')}, observed {'ordinary': 'OK', 'reserved-path': 'OK', 'reserved-name-prefix': 'OK'}

[mismatch] iam.not-principal-in-a-role-trust-policy
  AssertionError: trust policies with NotPrincipal: expected {'allow-not-principal': ('MalformedPolicyDocument', 'Allow with NotPrincipal is not allowed.'), 'deny-not-principal': ('MalformedPolicyDocument', 'AssumeRole policy must not contain NotPrincipal field.')}, observed {'allow-not-principal': 'OK', 'deny-not-principal': 'OK'}

[mismatch] iam.delete-user-with-enabled-mfa-device-rejected
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected ('DeleteConflict', 'Cannot delete entity, must remove tokens from principal first.')

[mismatch] iam.delete-user-with-ssh-public-key-rejected
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected ('DeleteConflict', 'Cannot delete entity, must remove referenced objects first.')

[mismatch] iam.pass-role-passed-to-service-condition-on-create-function
  AssertionError: refused: got ('OK', None); after_refusal: got ('OK', None); created: got ('ResourceConflictException', 'Function already exist: <generated-name>')
  (the generated function name is per-run; the requirement is that the leg labelled `refused` is refused and no function exists afterwards)

## Steps to Reproduce
1. Cause 1: CreateRole + CreateGroup, then AttachRolePolicy and AttachGroupPolicy with (a) `arn:aws:iam::aws:policy/<a-name-AWS-does-not-publish>` and (b) a real AWS-managed policy's name given WITHOUT its `/service-role/` path. Expect 404 NoSuchEntity 'Policy <arn> does not exist or is not attachable.' and no attachment; observed 200. Then attach the same policy at its correct `/service-role/` path — that must succeed.
2. Cause 2: CreateRole with `Path='/aws-service-role/sim2real/'` and, separately, `RoleName='AWSServiceRoleForSim2real'`. Expect InvalidInput with the two messages above; observed 200 for both. An ordinary path/name must still succeed.
3. Cause 3: CreateRole with an AssumeRolePolicyDocument whose single statement is `{Effect: Allow, NotPrincipal: {...}, Action: sts:AssumeRole}` and, separately, with `Effect: Deny`. Expect MalformedPolicyDocument with the two messages above; observed 200 for both.
4. Cause 4: CreateUser, CreateVirtualMFADevice + EnableMFADevice, then DeleteUser — expect 409 DeleteConflict 'Cannot delete entity, must remove tokens from principal first.' with the user and device intact; DeactivateMFADevice and the identical DeleteUser must then succeed. Repeat with UploadSSHPublicKey instead of the MFA device, expecting 'Cannot delete entity, must remove referenced objects first.'. Observed 200 in both cases.
5. Cause 5: create a role R passable to lambda, grant a caller `iam:PassRole` on R conditioned on `iam:PassedToService` NOT matching lambda.amazonaws.com, then call `lambda:CreateFunction` with `Role=R` as that caller. Expect the call refused and no function created; observed 200 and the function exists.
