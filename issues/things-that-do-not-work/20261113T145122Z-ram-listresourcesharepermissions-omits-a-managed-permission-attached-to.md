Issue-Source: trace-sweep-divergence
Issue-Trace: ram-resource-share-permission-association
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-resource-share-permission-association
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|ListResourceSharePermissions|2xx->2xx|$.permissions[arn="{{managed_perm_arn}}"]
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# ram ListResourceSharePermissions omits a managed permission attached to the share, so an attached permission is invisible through the share-scoped read

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceSharePermissions

## Description
One cause (Cause 1 of 1), grounded in the wire divergence ram-resource-share-permission-association#task#8 (verdict mismatch, expected status 200 / actual status 200 — the status agrees, the body does not).

Cause 1 of 1: a managed permission that the resource share is governed by is not reported at all by the share-scoped read ListResourceSharePermissions. The response is a well-formed 200 whose `permissions` list simply does not contain an entry for that permission's ARN, so after the permission-association step of the share's lifecycle the share reads back as if that permission were never attached.

  AWS: ListResourceSharePermissions "lists the RAM permissions that are associated with a resource share". POST /listresourcesharepermissions answers 200 with one ResourceSharePermissionSummary per managed permission currently governing the share named by the required `resourceShareArn`. A permission becomes one of those either because CreateResourceShare named it in `permissionArns`, or because AssociateResourceSharePermission attached it afterwards ("Adds or replaces the RAM permission for a resource type included in a resource share" — and "A return value of true indicates that the request succeeded"), or because it is the resource type's default managed permission for a shared type no named permission applies to. Once attached, it MUST appear in this read, with `arn` = the permission's ARN, `resourceType` = the type the permission applies to, `version` = the attached version as a string, `defaultVersion` = whether that version is the permission's current default, `featureSet` = the permission's feature set, and `status` = the status of that permission's association with THAT share. Smithy (com.amazonaws.ram): ListResourceSharePermissions input {resourceShareArn required, nextToken, maxResults @range 1..500}, output {permissions: ResourceSharePermissionList, nextToken}, http POST /listresourcesharepermissions 200, paginated {inputToken nextToken, outputToken nextToken, pageSize maxResults}; AssociateResourceSharePermission input {resourceShareArn required, permissionArn required, replace, clientToken, permissionVersion}, output {returnValue, clientToken}. The model does not enumerate ResourceSharePermissionSummary$status, so the concrete value ASSOCIATED for a permission attached to the share is recorded behavior only: the recording shows the entry for that permission present with status "ASSOCIATED", defaultVersion true, featureSet "STANDARD", resourceType the permission's resource type and version the attached version as a string.
  Emulator: the read answers HTTP 200 with a `permissions` list that contains NO element whose `arn` is the permission the fixture's share is governed by — the whole summary object is absent (`$.permissions[arn="{{managed_perm_arn}}"]: missing (expected {...status 'ASSOCIATED', resourceType 'imagebuilder:Component', version '1', defaultVersion true, featureSet 'STANDARD'...}, got None)`), while every earlier step of the share/permission-association sequence answered as recorded. So the emulator reports success for the attachment and then does not report the attachment.
  Why: ListResourceSharePermissions is the only public read that says which managed permissions govern a share. A client that attaches a permission and then enumerates the share's permissions — to confirm the attach, to audit which permission governs a resource type, to decide whether to replace it, or to find the permission ARN/version to pass to a later ReplacePermissionAssociations / DisassociateResourceSharePermission call — sees an empty or incomplete list and concludes the permission is not attached. Code that polls for the permission to appear (or for its status to reach ASSOCIATED) never terminates, and code that reacts by attaching again can be refused for a collision on a permission the read never showed it. The data is simply unobservable, so no retry recovers.
  Hypothesis: the share-scoped projection loses the attachment either because the attach is not recorded on the share's attached-permission map under the ARN the read keys on, or because the projection drops a holder whose stored version or resource-type spelling does not resolve (the projection returns None for an attachment whose permission or version does not look up, and only derives a resource type's AWS-managed default when no named attachment matches that type) — neither the model nor the recording states the mechanism.
  Fix: the share-permission attachment state and the ListResourceSharePermissions handler's projection of it — the attachment a successful AssociateResourceSharePermission (and a CreateResourceShare naming permissionArns) records must be reported by every later share-scoped read, and the projection must not silently drop an attachment it cannot resolve. File and line citations are under Traces.
  Traces: wire fixture ram-resource-share-permission-association, step task#8 (ListResourceSharePermissions).

Related open issues that do NOT cover this (the replay did not report this cluster as owned by either): 20261110T181341Z-ram-listresourcesharepermissions-never-reports-a-share-attachment-status.md and 20261108T153647Z-ram-listresourcesharepermissions-reports-a-share-s-attached-managed.md both concern the STATUS value (ATTACHABLE instead of ASSOCIATED) of a permission that IS listed; here the permission's summary is absent from the list entirely, which no status projection change fixes.

Code the behavior comes from (reference only, this checkout):
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/list_resource_share_permissions.rs:237-252 — the handler lists `store.share_permissions(&input.resource_share_arn)` and pushes `holder.summary()`
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state/permission_attachments.rs:200-241 — share_permissions(): projects the share's named attachments, `filter_map`ping away any whose permission ARN or stored version does not resolve, then derives a resource type's AWS-managed default only when no named attachment matches that type
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state/permission_attachments.rs:163-183 — attach_share_permission(), the only writer of a named attachment
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/associate_resource_share_permission.rs:378-412 — the attach handler, which answers returnValue true once attach_share_permission() reports it stored
  services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:496-503 — ShareRecord::attached_permissions, keyed by permission ARN

## Evidence
Wire fixture ram-resource-share-permission-association, step task#8 — ram:ListResourceSharePermissions(resourceShareArn=<the share created earlier in the fixture>):
  expected status 200, actual status 200 (verdict: mismatch — divergence_key service=ram|ListResourceSharePermissions|status=200->200|keys=$.permissions[arn="{{managed_perm_arn}}"])
  expected $.permissions[arn="{{managed_perm_arn}}"] = {arn: <the managed permission attached to the share>, defaultVersion: true, featureSet: "STANDARD", lastUpdatedTime: <shaped-not-asserted>, resourceType: "imagebuilder:Component", status: "ASSOCIATED", version: "1"}
  actual   $.permissions[arn="{{managed_perm_arn}}"] = None — there is no element in `permissions` with that ARN at all.
  affected_count 1 (this step only); stub_path empty and owned_by empty, so no unimplemented operation blocks it and no open issue carries its replay key.

Requirement vs. per-run values: the requirement is that a managed permission the share is governed by is reported by ListResourceSharePermissions as a ResourceSharePermissionSummary carrying that permission's ARN, its resource type, the attached version as a string, defaultVersion for that version, the permission's featureSet, and the share-attachment status (ASSOCIATED). The specific ARN ({{managed_perm_arn}} is captured earlier in the fixture), the resource type of this run (imagebuilder:Component), the version string "1" and the timestamps are values from the request and the emulator's state, not part of the contract.

No status code, error code or error message diverges here: both sides answer 200 with no error. The divergence is a missing member of a success body.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (branch issues/finder-wi-2649fc9696cc4529a17b003aaa4704ac-1794580633200) and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture ram-resource-share-permission-association; step task#8 reports verdict mismatch with detail '$.permissions[arn="{{managed_perm_arn}}"]: missing'.
3. Minimal reproduction of the diverging read:
   a. ram:CreatePermission (or capture a managed permission ARN from ram:ListPermissions) for resource type imagebuilder:Component — note its ARN and default version.
   b. ram:CreateResourceShare naming an imagebuilder:Component resource ARN in resourceArns.
   c. ram:AssociateResourceSharePermission(resourceShareArn=<share ARN>, permissionArn=<that permission ARN>, replace=true) — observe 200 with returnValue true.
   d. ram:ListResourceSharePermissions(resourceShareArn=<share ARN>) with no nextToken.
4. Observe: 200, but the `permissions` list carries no entry whose arn equals the permission just attached.
5. After the fix, re-replay ram-resource-share-permission-association#task#8: the read must carry a ResourceSharePermissionSummary for the attached permission with arn = the attached permission's ARN, resourceType = that permission's resource type, version = the attached version as a string, defaultVersion = true while that version is the permission's default, featureSet = "STANDARD" and status = "ASSOCIATED", and the rest of the fixture's permission-association sequence must still replay unchanged.
