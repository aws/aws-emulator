Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-session-tags-unicode-value-round-trip
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: iam|ListRoles|AccessDenied
Issue-Commit: 043b7f8056dcef57c0c9baaf75abb2dd363a8bd0

# A session tag whose value contains non-ASCII characters does not satisfy a StringEquals condition on aws:PrincipalTag/<key>: the correctly tagged session is refused AccessDenied where AWS allows it

> _Note: this issue was identified against the code base at commit `043b7f8056dcef57c0c9baaf75abb2dd363a8bd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
ONE root cause, grounded in 1 failing behavioral check (service token 'sts', the leading token of the check id). The report marked this check neither OWNED nor ALREADY FILED.

Not covered by, but adjacent to, the open issue 20261029T120000Z-sts-3-of-the-8-session-scoping-causes-remain-open.md: that issue lists the SIBLING check id sts.assume-role-session-tag-unicode-value-round-trip (singular 'tag') under its Cause 1 ('no session-scoped authorization is evaluated anywhere — every such call SUCCEEDS'), and it does not carry this check id at all. This run shows the opposite failure direction for the plural check: the two negative halves are now refused correctly, with AWS's exact AccessDenied message, so principal-tag conditions ARE being evaluated; what fails is the positive half. That makes this a distinct cause and it must be closed here.

Cause 1 of 1: a unicode session tag value is not preserved for condition evaluation. A role carries an identity policy allowing iam:ListRoles on '*' with Condition {"StringEquals": {"aws:PrincipalTag/Team": "<a value containing accented Latin and CJK characters>"}}. A session minted by sts:AssumeRole with Tags=[{Key: 'Team', Value: <that exact value>}] is nevertheless refused AccessDenied on iam:ListRoles. The two control sessions behave correctly — a session with NO tag, and a session whose tag value differs only by stripped diacritics, are both refused AccessDenied with AWS's exact message — so the evaluation path exists and only the non-ASCII value fails to compare equal. Hypothesis: the tag value is mangled between AssumeRole's input decoding and the condition comparison (unicode normalization, a non-UTF-8 round trip, or percent/escape handling of the stored value), or the StringEquals comparison itself is not byte-exact over non-ASCII; the report cannot distinguish these, but either way the session carrying the exact submitted value is denied.

AWS: Smithy com.amazonaws.sts#Tag$Value (target tagValueType) is a string with @length min 0 max 256 and @pattern ^[\p{L}\p{Z}\p{N}_.:/=+\-@]*$ — \p{L} admits every unicode letter, so accented Latin and CJK letters are valid tag values, and AWS documents session tags as 'plain text' key-value attributes used 'to control access to resources'. The value a session is tagged with therefore populates aws:PrincipalTag/<key> byte-exact, and a StringEquals condition on that key matches exactly that value and nothing else. For this request the consequence is: iam.ListRoles under the exactly-tagged session is answered HTTP 200 with the account's roles (including the role just created), while the untagged and diacritics-stripped sessions are refused with HTTP 403 AccessDenied.

Emulator: the exactly-tagged session is refused AccessDenied with the message 'User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::<account-id>:role/ because no identity-based policy allows the iam:ListRoles action'. Everything in that sentence except the account id, role name and session name is AWS's fixed text for a condition/implicit-deny refusal of iam:ListRoles, and those three are values taken from the request and the emulator's state — the wording itself is right; what is wrong is that it is produced at all for a session whose tag satisfies the policy's condition. The identical message for the untagged and the diacritics-stripped sessions was asserted equal and passed, which is what isolates the defect to the non-ASCII value comparison.

Why: a client whose authorization model keys on session tags (per-tenant, per-team or per-region scoping via aws:PrincipalTag) is locked out whenever a tag value is not pure ASCII — team names, tenant names and place names routinely are not. The failure is silent and looks like a policy bug on the client's side: the policy and the AssumeRole call are both valid and AWS would allow the call, so the emulator refuses work the real service performs, and conversely a client cannot trust a passing tag-condition test to mean anything about non-ASCII values.

Fix: the session-tag storage and the condition-evaluation path — AssumeRole's handling of the Tags member (its input decode and whatever record carries the minted session's tags) must preserve the submitted value byte-exact, with no unicode normalization or lossy re-encoding, and the policy condition evaluation that resolves aws:PrincipalTag/<key> must compare StringEquals byte-exact over the stored value. The same applies to the transitive-tag copy made when a session is chained.

Traces: sts.assume-role-session-tags-unicode-value-round-trip (fails on the third, positive leg — tagged_iam.list_roles() — after the no-tag leg and the stripped-diacritics leg both produced exactly the expected AccessDenied code and message).

## Evidence
Replay report, service sts:

  [error] sts.assume-role-session-tags-unicode-value-round-trip
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (AccessDenied) when calling the ListRoles operation: User: arn:aws:sts::000000000000:assumed-role/sim2real-behav-tagrt-role-12a666cdfb94/tagged-session is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::000000000000:role/ because no identity-based policy allows the iam:ListRoles action

The tag value under test in this run was 'Café-Zürich-北京' and the mismatched control value 'Cafe-Zurich-北京'; the account id, role name and session name above are per-run values. The requirement is the rule, not these strings: any value admitted by Tag$Value's @pattern (which includes \p{L}) must round-trip byte-exact into aws:PrincipalTag/<key>.

Sequencing (all of this passed before the failing call, which is what localises the cause):
  1. iam.create_user + access key + an inline user policy allowing sts:AssumeRole on '*'.
  2. iam.create_role whose trust policy allows that user both sts:AssumeRole and sts:TagSession.
  3. iam.put_role_policy: Allow iam:ListRoles on '*' with Condition {StringEquals: {aws:PrincipalTag/Team: '<unicode value>'}}.
  4. assume_role with NO Tags -> iam.list_roles refused, code == 'AccessDenied' AND message == exactly 'User: arn:aws:sts::<account>:assumed-role/<role>/no-tag-session is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::<account>:role/ because no identity-based policy allows the iam:ListRoles action' — PASSED.
  5. assume_role with Tags=[{Team: '<diacritics-stripped value>'}] -> same AccessDenied code and the same message with session name 'mismatched-tag-session' — PASSED.
  6. assume_role with Tags=[{Team: '<exact unicode value>'}] -> expected HTTP 200 from iam.list_roles with the created role present in Roles[].RoleName; actual HTTP 403 AccessDenied with the message above. FAILED.

Expected (AWS): step 6 answers 200 and the role appears in the listing. Actual (emulator): 403 AccessDenied, i.e. the condition did not match the value the session was tagged with.

## Steps to Reproduce
1. Boot the emulator; confirm IAM and STS answer.
2. iam.create_user, create an access key for it, and attach an inline user policy allowing sts:AssumeRole on '*'.
3. iam.create_role with a trust policy allowing that user ARN both sts:AssumeRole and sts:TagSession.
4. iam.put_role_policy on that role: {"Effect": "Allow", "Action": "iam:ListRoles", "Resource": "*", "Condition": {"StringEquals": {"aws:PrincipalTag/Team": "Café-Zürich-北京"}}} (any value containing non-ASCII letters admitted by Tag$Value's @pattern).
5. With the user's long-term key, sts.assume_role(RoleArn=<role>, RoleSessionName='no-tag-session') and call iam.list_roles with the session credentials -> AccessDenied with AWS's implicit-deny message (passes today).
6. sts.assume_role(..., RoleSessionName='mismatched-tag-session', Tags=[{'Key': 'Team', 'Value': 'Cafe-Zurich-北京'}]) and call iam.list_roles -> AccessDenied with the same message (passes today).
7. sts.assume_role(..., RoleSessionName='tagged-session', Tags=[{'Key': 'Team', 'Value': 'Café-Zürich-北京'}]) and call iam.list_roles with those credentials. On AWS: HTTP 200 and the role is present in Roles[]. On the emulator: HTTP 403 AccessDenied 'User: arn:aws:sts::<account>:assumed-role/<role>/tagged-session is not authorized to perform: iam:ListRoles on resource: arn:aws:iam::<account>:role/ because no identity-based policy allows the iam:ListRoles action'.
8. Re-replay sts.assume-role-session-tags-unicode-value-round-trip.
