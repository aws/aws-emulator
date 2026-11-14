Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.create-permission-version-disallowed-action-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.create-permission-version-disallowed-action-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# ram CreatePermissionVersion accepts a policy template naming an action RAM does not allow for the permission's resource type, instead of refusing it with InvalidPolicyException

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePermissionVersion

## Description
Cause 1 of 1: CreatePermissionVersion's policy-template validation only checks that each `Action` entry's service namespace matches the service that owns the permission's `resourceType`. It does not check the action against the set of actions RAM allows a managed permission for that resource type to grant, so a mutating action RAM never permits in a shared-resource permission is stored as a new (and new default) version.

AWS: `CreatePermissionVersion$policyTemplate` is documented as "A string in JSON format string that contains the following elements of a resource-based policy: Effect: must be set to ALLOW. Action: specifies the actions that are allowed by this customer managed permission. **The list must contain only actions that are supported by the specified resource type.**" The operation models `InvalidPolicyException` (Smithy: CreatePermissionVersion errors include InvalidPolicyException, httpError 400, awsQueryError code `InvalidPolicy`; doc: "The operation failed because a policy you specified isn't valid."). Recorded behavior: for a CUSTOMER_MANAGED permission whose `resourceType` is `codebuild:Project`, a template `{"Effect":"Allow","Action":["codebuild:DeleteProject"]}` is refused with HTTP 400, `Error.Code` = `InvalidPolicyException`, and `Error.Message` = `InvalidPolicy: Input policy containing action that is not allowed for resource type codebuild:Project`. In that message the fixed text is `InvalidPolicy: Input policy containing action that is not allowed for resource type ` and the only variable part is the resource type, taken from the named permission's stored `resourceType` (here `codebuild:Project`); no request id, action name or permission ARN appears in it. AWS also leaves the permission untouched after the refusal: the pre-existing version 1 remains the only version and the default.

Emulator: the call is answered HTTP 200 with a fully-formed `permission` whose `version` is `2`, `defaultVersion` is `true`, `permissionType` is `CUSTOMER_MANAGED`, `status` is `ATTACHABLE`, and `permission` is the disallowed template `{"Effect": "Allow", "Action": ["codebuild:DeleteProject"]}`. No error is raised, and the new version becomes the permission's default. The refusal is never reached because `crate::state::policy_template_rejection` accepts any action whose namespace equals the resource type's service: it rejects only an action of a *different* service namespace ("policyTemplate Action '<a>' is not an action supported by resource type '<t>'"), which `codebuild:DeleteProject` passes.

Why: a client relying on AWS's behavior gets a permission that grants a destructive action on a shared resource type where AWS would have refused to create it. Code that treats the 200 as "this grant is legal" ships an over-permissive customer managed permission, and any code branching on `InvalidPolicyException` (or on the documented `InvalidPolicy: Input policy containing action that is not allowed for resource type <resourceType>` message) never takes that branch. The permission's default version also silently advances, so shares that later attach the default pick up the disallowed grant. The same gap applies to `CreatePermission`, which shares this validation helper.

Fix: the allowed-action rule belongs in the policy-template validation shared by CreatePermission/CreatePermissionVersion — `policy_template_rejection` must hold each `Action` entry against the actions RAM allows for the permission's `resourceType` (not just the owning service namespace) and return `PolicyTemplateRejection::InvalidPolicy` with AWS's fixed sentence and the resource type substituted, which the handler already maps to `InvalidPolicyException` / 400. The handler must make this check before any state write, so the refused version is never added and the existing default is unchanged.

Traces: ram.create-permission-version-disallowed-action-rejected (mismatch: "expected the call to be rejected, but it succeeded" with the version-2 permission detail shown above). Note the same check's later assertions use `ListPermissionVersions`, whose unimplemented scaffold stub is filed separately; this issue is about the accepted CreatePermissionVersion call only.

## Evidence
Expected (AWS): CreatePermissionVersion(permissionArn=<customer managed permission on codebuild:Project>, policyTemplate={"Effect":"Allow","Action":["codebuild:DeleteProject"]}) -> HTTP 400, Error.Code=InvalidPolicyException, Error.Message="InvalidPolicy: Input policy containing action that is not allowed for resource type codebuild:Project" (fixed text + the permission's resourceType); permission still has exactly version "1" as its only and default version.

Actual (emulator): HTTP 200 with {'permission': {'arn': 'arn:aws:ram:<region>:<account>:permission/<name>', 'version': '2', 'defaultVersion': True, 'resourceType': 'codebuild:Project', 'permission': '{"Effect": "Allow", "Action": ["codebuild:DeleteProject"]}', 'permissionType': 'CUSTOMER_MANAGED', 'status': 'ATTACHABLE', ...}, 'clientToken': '<uuid>'} — no refusal, and version 2 becomes the default. (ARN, name, uuid and timestamps are per-run values, not part of the requirement.)

Code path: services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_permission_version.rs:300-319 calls crate::state::policy_template_rejection, whose action check at services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:2311-2332 only compares the action's service namespace with resource_type_service(resource_type), so any same-service action — including a mutating one RAM disallows — is accepted.

## Steps to Reproduce
1. ram.CreatePermission(name=<unique>, resourceType="codebuild:Project", policyTemplate='{"Effect": "Allow", "Action": ["codebuild:BatchGetProjects"]}') -> note the returned permission arn (version 1, CUSTOMER_MANAGED).
2. ram.CreatePermissionVersion(permissionArn=<that arn>, policyTemplate='{"Effect": "Allow", "Action": ["codebuild:DeleteProject"]}').
3. AWS refuses with 400 InvalidPolicyException and message "InvalidPolicy: Input policy containing action that is not allowed for resource type codebuild:Project"; the emulator answers 200 with version "2" as the new default.
4. ram.ListPermissionVersions(permissionArn=<that arn>) on AWS reports only version "1"; the emulator has stored a second version (this read is itself blocked by the separately filed ListPermissionVersions stub).
5. Replay check: ram.create-permission-version-disallowed-action-rejected.
