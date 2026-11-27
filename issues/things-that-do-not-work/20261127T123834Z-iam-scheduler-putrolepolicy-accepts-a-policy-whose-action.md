Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__scheduler.policy-action-without-scheduler-prefix-rejected
Issue-Key: iam__scheduler.policy-action-without-scheduler-prefix-rejected|_refusal#24|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# iam__scheduler: PutRolePolicy accepts a policy whose Action has no vendor prefix instead of refusing it with MalformedPolicyDocument

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRolePolicy

## Description
One net-new cause.

Cause 1 of 1: an inline role policy whose Statement Action names an operation with no service prefix is accepted.
AWS: an IAM policy Action (and a condition key) must be prefaced by a service vendor, so a document whose Action is the bare operation name "ListSchedules" is refused with 400 MalformedPolicyDocument and the message "Actions/Conditions must be prefaced by a vendor, e.g., iam, sdb, ec2, etc." — entirely fixed text. Nothing is stored, so ListRolePolicies on the role returns an empty PolicyNames list. The same document with a prefixed action ("events:ListSchedules" or "scheduler:ListSchedules") is accepted.
Emulator: returns HTTP 200 with a text/xml PutRolePolicy response and attaches the policy.
Why: a malformed policy document is persisted, so the authorization decisions made from it are unpredictable — the role appears to hold a grant whose action names no service, and the caller is never told the document is invalid. In the same scenario the prefixed-but-wrong-service grant ("events:ListSchedules") must still deny scheduler:ListSchedules while "scheduler:ListSchedules" allows it; storing an unprefixed action makes which of those two paths applies undefined.
Fix: the iam crate's policy-document parser/validator reached from PutRolePolicy (and the other policy-writing operations) — require a vendor prefix on every Action and Condition key.
Traces: iam__scheduler.policy-action-without-scheduler-prefix-rejected

## Evidence
iam__scheduler.policy-action-without-scheduler-prefix-rejected:
  PutRolePolicy(RoleName=<caller role>, PolicyName="bare", PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ListSchedules","Resource":"..."}]})
  expected MalformedPolicyDocument "Actions/Conditions must be prefaced by a vendor, e.g., iam, sdb, ec2, etc.", and ListRolePolicies(RoleName=<caller role>) returning PolicyNames []
  got      HTTP 200 (content-type text/xml) and the policy attached

## Steps to Reproduce
1. Boot the emulator. iam CreateRole for a role assumable by the account root.
2. iam PutRolePolicy on that role with PolicyDocument {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ListSchedules","Resource":"arn:aws:scheduler:*:*:schedule/*/*"}]} — expect MalformedPolicyDocument "Actions/Conditions must be prefaced by a vendor, e.g., iam, sdb, ec2, etc.", observe HTTP 200.
3. iam ListRolePolicies on the role — expect PolicyNames [], observe the "bare" policy present.
