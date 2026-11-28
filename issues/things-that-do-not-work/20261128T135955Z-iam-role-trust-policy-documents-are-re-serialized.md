Issue-Source: trace-sweep-divergence
Issue-Trace: iam-role-explicit-session-duration-and-path-assumed-description-cleared-tag-change-then-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-role-explicit-session-duration-and-path-assumed-description-cleared-tag-change-then-not-found iam-trust-policy-size-quota iam-instance-profile-explicit-path-tags-launches-an-instance-delete-refused-with-role-and-not-found iam-service-linked-role-deletion-status-and-acquire-role-error iam-service-linked-role-lifecycle iam-service-linked-role-supported-services
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRole|2xx->2xx|$.GetRoleResponse.GetRoleResult.Role.AssumeRolePolicyDocument
Issue-Key: iam|UpdateRoleDescription|2xx->2xx|$.UpdateRoleDescriptionResponse.UpdateRoleDescriptionResult.Role.AssumeRolePolicyDocument
Issue-Key: iam|GetInstanceProfile|2xx->2xx|$.GetInstanceProfileResponse.GetInstanceProfileResult.InstanceProfile.Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|CreateServiceLinkedRole|2xx->2xx|$.CreateServiceLinkedRoleResponse.CreateServiceLinkedRoleResult.Role.AssumeRolePolicyDocument
Issue-Commit: 045aa7b4fc15c1aef135f1e9d9cb0cfc8b5fe67c

# iam: role trust-policy documents are re-serialized from parsed JSON instead of returned as stored

> _Note: this issue was identified against the code base at commit `045aa7b4fc15c1aef135f1e9d9cb0cfc8b5fe67c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Every IAM read path that projects a role's trust policy returns a DIFFERENT document text from the one that was stored. The emulator is parsing the submitted/canned policy into a map and re-serializing it on read, so the JSON object members come back in alphabetical order (and, for service-linked roles, in a shape the service never emits), instead of the stored document text being echoed back percent-encoded. There are 5 causes, one per affected projection.

Cause 1 of 5: GetRole.Role.AssumeRolePolicyDocument member order.
AWS: Smithy iam (2010-05-08): Role.AssumeRolePolicyDocument -> policyDocumentType (string, @length min 1 max 131072, pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$). The model constrains the characters only and says nothing about member order, so: recorded behavior only — the recording shows AWS returning the document the caller submitted, percent-encoded, with its object members in the submitted order (decoded: {"Version":"2012-10-17","Statement":[{"Sid":"","Effect":"Allow","Principal":{...},"Action":"sts:AssumeRole"}]}). The document is opaque stored text, not a normalized structure.
Emulator: returns the same policy with every object's members sorted alphabetically at both levels (decoded: {"Statement":[{"Action":"sts:AssumeRole","Effect":"Allow","Principal":{...},"Sid":""}],"Version":"2012-10-17"}). Status is 200 on both sides; only the document text differs.
Why: a client that round-trips the trust policy — Terraform/CloudFormation drift detection, a CDK diff, or any read-compare-write loop that hashes or string-compares the document it submitted — sees a permanent difference it can never converge, and re-submits the role update on every plan. Clients that parse the document positionally (first statement member) also read a different element.
Fix: the role state/serializer seam for AssumeRolePolicyDocument — store the caller's document bytes verbatim and percent-encode those on projection, rather than storing a parsed map and re-serializing it.
Traces: iam-role-explicit-session-duration-and-path-assumed-description-cleared-tag-change-then-not-found#task#2 (also #task#11, #task#13, #task#15 and iam-trust-policy-size-quota#task#5).

Cause 2 of 5: UpdateRoleDescription.Role.AssumeRolePolicyDocument member order.
AWS: Smithy iam: UpdateRoleDescription output Role -> the same Role shape, so its AssumeRolePolicyDocument is the same stored document; recorded behavior only — the recording returns it in submitted member order.
Emulator: the same alphabetical re-serialization as Cause 1 in the UpdateRoleDescription response body (200 on both sides).
Why: the mutating call's own response is what a client caches as the new role state, so the divergence is carried into the client's model even when it never issues a GetRole.
Fix: same seam as Cause 1 — the shared Role projection used by UpdateRoleDescription's handler.
Traces: iam-role-explicit-session-duration-and-path-assumed-description-cleared-tag-change-then-not-found#task#10 (also #task#12).

Cause 3 of 5: GetInstanceProfile.InstanceProfile.Roles.member.AssumeRolePolicyDocument member order.
AWS: Smithy iam: GetInstanceProfile output InstanceProfile.Roles is a list of the same Role shape; recorded behavior only — the embedded trust policy is the stored document, submitted member order ({"Version":...,"Statement":[{"Sid":"","Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}).
Emulator: alphabetically re-serialized, as in Cause 1 (200 on both sides).
Why: instance-profile reads are how EC2 tooling discovers the role a profile carries; the same never-converging diff appears there.
Fix: the instance-profile projection's embedded Role serializer (same stored-text rule as Cause 1). Note this is NOT the already-resolved percent-encoding defect in these projections — the document here IS percent-encoded, its member order is wrong.
Traces: iam-instance-profile-explicit-path-tags-launches-an-instance-delete-refused-with-role-and-not-found#task#7 (also #task#16).

Cause 4 of 5: ListInstanceProfilesForRole.InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument member order.
AWS: Smithy iam: the paginated list projects the same Role shape; recorded behavior only — stored document, submitted member order.
Emulator: alphabetically re-serialized, as in Cause 1 (200 on both sides).
Why: same as Cause 3, on the paginated listing path.
Fix: the list projection for instance profiles, same stored-text rule.
Traces: iam-instance-profile-explicit-path-tags-launches-an-instance-delete-refused-with-role-and-not-found#task#8.

Cause 5 of 5: CreateServiceLinkedRole synthesizes its own trust-policy text instead of the service's canned document.
AWS: Smithy iam: CreateServiceLinkedRole(AWSServiceName) -> Role. Model says nothing about the generated document, so: recorded behavior only — the recording shows AWS attaching the service's canned service-linked-role trust policy, whose text spells Action and Principal.Service as single-element ARRAYS and carries a space after each colon/comma (decoded: {"Version": "2012-10-17", "Statement": [{"Action": ["sts:AssumeRole"], "Effect": "Allow", "Principal": {"Service": ["<service principal>"]}}]}), percent-encoded.
Emulator: emits a hand-built compact document with scalar Action and scalar Principal.Service and no whitespace (decoded: {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"<service principal>"},"Action":"sts:AssumeRole"}]}). Status 200 on both sides.
Why: a client that reads the SLR trust policy and asserts the list-typed Action/Principal.Service members (policy linters, access-analyzer style tooling, drift checks) mis-reads scalars for lists; the role also no longer matches the document AWS publishes for that service.
Fix: the CreateServiceLinkedRole handler's state — store the service's canned trust-policy document text for the requested AWSServiceName and project that text verbatim.
Traces: iam-service-linked-role-deletion-status-and-acquire-role-error#task#1 (also iam-service-linked-role-lifecycle#task#3, iam-service-linked-role-supported-services#task#2).

## Evidence
Cause 1 — GetRole, iam-role-explicit-session-duration-and-path-assumed-description-cleared-tag-change-then-not-found#task#2, 200 vs 200:
  expected $.GetRoleResponse.GetRoleResult.Role.AssumeRolePolicyDocument (decoded) {"Version":"2012-10-17","Statement":[{"Sid":"","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"sts:AssumeRole"}]}
  actual   (decoded) {"Statement":[{"Action":"sts:AssumeRole","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Sid":""}],"Version":"2012-10-17"}
  (4 further steps in the cluster: #task#11, #task#13, #task#15, iam-trust-policy-size-quota#task#5)

Cause 2 — UpdateRoleDescription, same fixture #task#10 (and #task#12), 200 vs 200: $.UpdateRoleDescriptionResponse.UpdateRoleDescriptionResult.Role.AssumeRolePolicyDocument shows the identical member re-ordering.

Cause 3 — GetInstanceProfile, iam-instance-profile-explicit-path-tags-launches-an-instance-delete-refused-with-role-and-not-found#task#7 (and #task#16), 200 vs 200:
  expected $.GetInstanceProfileResponse.GetInstanceProfileResult.InstanceProfile.Roles.member.AssumeRolePolicyDocument (decoded) {"Version":"2012-10-17","Statement":[{"Sid":"","Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}
  actual   (decoded) {"Statement":[{"Action":"sts:AssumeRole","Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Sid":""}],"Version":"2012-10-17"}

Cause 4 — ListInstanceProfilesForRole, same fixture #task#8, 200 vs 200: $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument, identical re-ordering.

Cause 5 — CreateServiceLinkedRole, iam-service-linked-role-deletion-status-and-acquire-role-error#task#1, 200 vs 200:
  expected $.CreateServiceLinkedRoleResponse.CreateServiceLinkedRoleResult.Role.AssumeRolePolicyDocument (decoded) {"Version": "2012-10-17", "Statement": [{"Action": ["sts:AssumeRole"], "Effect": "Allow", "Principal": {"Service": ["elasticbeanstalk.amazonaws.com"]}}]}
  actual   (decoded) {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"elasticbeanstalk.amazonaws.com"},"Action":"sts:AssumeRole"}]}
  (service principal shown is the fixture's; the requirement is the canned document's SHAPE and spacing for whichever AWSServiceName was requested)

Note: the resolved issues 20261111T010549Z / 20261111T111935Z covered these same projections emitting the document UNENCODED; that is fixed and is not this defect — the document is percent-encoded now, but its text is rebuilt rather than echoed.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator built from this checkout.
2. Cause 1: fixture iam-role-explicit-session-duration-and-path-assumed-description-cleared-tag-change-then-not-found, step task#2 — CreateRole with a trust policy whose statement members are ordered Sid, Effect, Principal, Action, then GetRole and percent-decode Role.AssumeRolePolicyDocument: the members come back alphabetized and Version moves after Statement.
3. Cause 2: same fixture, step task#10 — UpdateRoleDescription on that role and inspect Role.AssumeRolePolicyDocument in the response body.
4. Causes 3 and 4: fixture iam-instance-profile-explicit-path-tags-launches-an-instance-delete-refused-with-role-and-not-found, steps task#7 and task#8 — GetInstanceProfile and ListInstanceProfilesForRole on a profile holding an ec2.amazonaws.com role, and decode Roles.member.AssumeRolePolicyDocument.
5. Cause 5: fixture iam-service-linked-role-deletion-status-and-acquire-role-error, step task#1 — CreateServiceLinkedRole for a service and decode Role.AssumeRolePolicyDocument: Action and Principal.Service are scalars and the canned document's spacing is gone.
