Issue-Source: trace-sweep-divergence
Issue-Trace: iam-instance-profile-listing-pagination
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-instance-profile-listing-pagination
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-0-d2ffbf93"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-1-41fe08c3"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-2-23020946"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-3-55de6973"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-0-d2ffbf93"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-1-41fe08c3"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-2-23020946"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-6bead248/sim2real-probe-iam-ipp-profile-3-55de6973"].Roles.member.AssumeRolePolicyDocument
Issue-Commit: 2082c0d6dcf3d4f1b6ce91e028a235b3f94c10a0

# iam instance-profile listing pages serve AssumeRolePolicyDocument unencoded on paginated reads

> _Note: this issue was identified against the code base at commit `2082c0d6dcf3d4f1b6ce91e028a235b3f94c10a0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceProfiles

## Description
2 net-new causes, one shared fault: the embedded Role projection used by IAM's instance-profile listing operations emits Role.AssumeRolePolicyDocument as the stored raw JSON text instead of the RFC-3986 percent-encoded form AWS returns. Both causes must be closed before this issue is submitted.

NOTE ON EXISTING ISSUES: the open issues 20261109T145343Z-iam-instance-profile-listing-pages-return-assumerolepolicydocument-as.md and 20261109T134638Z-iam-query-responses-omit-or-mis-encode-modeled.md describe the same fault family, but their replay keys are pinned to per-run instance-profile ARNs from earlier recordings (sim2real-probe-iam-ipp-16445403 / -b838c917), so neither owns the replay keys reported here (this run's profiles are under sim2real-probe-iam-ipp-6bead248). The replay reports both clusters below as owned by no open issue, so they are filed here and must be re-replayed against these keys to confirm a fix.

Cause 1 of 2: ListInstanceProfiles paginated page (fixture iam-instance-profile-listing-pagination, step task#11).
AWS: Smithy (com-amazonaws-iam-awsidentitymanagementv20100508) — ListInstanceProfiles output carries InstanceProfiles, a list of instance profiles each with Roles, a list of com.amazonaws.iam#Role; Role.AssumeRolePolicyDocument targets com.amazonaws.iam#policyDocumentType (@length 1..131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$). The model does not state the transfer encoding, so this is recorded behavior plus AWS's documented rule for IAM policy-document response members: IAM returns the policy document URL-encoded (percent-encoded, RFC 3986) and the caller must URL-decode it before parsing it as JSON. The requirement is the TRANSFORM, not any one document: percent-encode the stored JSON byte-for-byte, preserving whatever whitespace the caller submitted ('{' -> %7B, '"' -> %22, ':' -> %3A, ',' -> %2C, '[' -> %5B, ']' -> %5D, '*' -> %2A, ' ' -> %20). It applies to every InstanceProfiles.member.Roles.member element of every page, not only the first. Status is 200 on both sides; no error code or message is involved.
Emulator: returns the raw JSON text verbatim for each Roles.member.AssumeRolePolicyDocument of the page — the percent-encoding step is simply not applied in this projection. Every instance profile in the page (4 in this recording) reproduces it.
Why: a client that follows AWS's contract URL-decodes the member before json.loads(). Decoding an unencoded document is a silent no-op only while it contains no '%' and no '+'; a trust policy carrying either (a percent in a condition value, a '+' in a principal or ARN) is corrupted on decode — '+' becomes a space, a stray '%xx' becomes a different byte — so the parse fails or yields a policy the caller never wrote. Code that compares a round-tripped trust policy against the encoded form it submitted never matches, so Get -> edit -> Put of a trust policy cannot be done against the emulator the way it is done against AWS. Because this is the PAGINATED read, the breakage surfaces exactly when a caller enumerates instance profiles with Marker/MaxItems, the normal way to list them.
Fix: the IAM Query response serializer for policyDocumentType-targeted members, applied in the shared embedded-Role projection that the instance-profile listings write out, so pages and single-resource reads encode identically — one encode step in the serializer rather than per handler. Role-level reads (GetRole/ListRoles) do not diverge on this member in this corpus, so the encode exists elsewhere and is not reached here.
Traces: iam-instance-profile-listing-pagination#task#11.

Cause 2 of 2: ListInstanceProfilesForRole (fixture iam-instance-profile-listing-pagination, step task#13).
AWS: identical contract — ListInstanceProfilesForRole output InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument targets the same policyDocumentType and is returned percent-encoded on every page. Status 200.
Emulator: the same raw-JSON passthrough on every profile returned for the role; status 200, only the member value diverges.
Why: same as Cause 1 — a client resolving which instance profiles a role is attached to and reading the role's trust policy from that response gets a document in an encoding the documented contract says it is not in.
Fix: same component as Cause 1; both causes vanish with the one serializer edit and are listed separately only because each carries its own replay key.
Traces: iam-instance-profile-listing-pagination#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; /__admin/health answered 200). Both causes are 200-vs-200 body divergences; the status line, the error code and the error message are not involved on either side.

Cause 1 — ListInstanceProfiles, fixture iam-instance-profile-listing-pagination step task#11, expected status 200 / actual status 200, verdict mismatch:
  key: $.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::<account>:instance-profile/<path>/<profile-name>"].Roles.member.AssumeRolePolicyDocument
  expected (recorded AWS): '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D'
  actual (emulator): '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}'
  reported for all 4 profiles in the page. Expected and actual are the SAME policy document — only the encoding differs. The account id, instance-profile path and profile names in the key, and the trust policy text itself, are per-run values of this recording; the percent-encoding of the member is the requirement.

Cause 2 — ListInstanceProfilesForRole, fixture iam-instance-profile-listing-pagination step task#13, expected status 200 / actual status 200, verdict mismatch:
  key: $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::<account>:instance-profile/<path>/<profile-name>"].Roles.member.AssumeRolePolicyDocument
  expected: the same '%7B%22Version%22%3A...%7D' percent-encoded text; actual: the same raw '{"Version": "2012-10-17", ...}' JSON, on every profile returned for the role.

Every other member of both pages agrees, so the divergence is purely the serialization of the policyDocumentType member.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture iam-instance-profile-listing-pagination (it creates a role with an ec2.amazonaws.com trust policy and several instance profiles under one path, then walks them with Marker/MaxItems).
3. At step task#11 (ListInstanceProfiles, paginated page) read InstanceProfiles.member[*].Roles.member.AssumeRolePolicyDocument: AWS returns the percent-encoded trust policy, the emulator returns raw JSON.
4. At step task#13 (ListInstanceProfilesForRole) repeat the comparison on InstanceProfiles.member[*].Roles.member.AssumeRolePolicyDocument — same divergence.
5. Or directly: CreateRole with a trust policy, CreateInstanceProfile, AddRoleToInstanceProfile, then call ListInstanceProfiles (with MaxItems to force a page) and ListInstanceProfilesForRole for that role. URL-decode the returned AssumeRolePolicyDocument as the documented contract requires: against the emulator the decode is a no-op, proving the member was never encoded. Use a trust policy containing a literal '+' or '%' to observe the corruption a raw document causes on decode.
