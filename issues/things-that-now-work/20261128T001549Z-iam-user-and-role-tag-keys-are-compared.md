Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.user-and-role-tag-keys-case-insensitive
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.user-and-role-tag-keys-case-insensitive|run#11|AssertionError|assert len(user_tags) == 1, (
Issue-Commit: 7e7f1782eb72d186b4a02ad6793c94a317471370

# iam: user and role tag keys are compared case-sensitively, so Department and department coexist as two tags and UntagRole with other casing removes nothing

> _Note: this issue was identified against the code base at commit `7e7f1782eb72d186b4a02ad6793c94a317471370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagUser

## Description
Cause 1 of 1: IAM stores and matches tag keys on users and roles by exact byte comparison, so two keys differing only in letter case are kept as two distinct tags instead of one, and an untag by a differently-cased key matches nothing. Failing check: iam.user-and-role-tag-keys-case-insensitive.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508) `TagUser` — "Adds one or more tags to an IAM user. If a tag with the same key name already exists, then that tag is overwritten with the new value"; `TagRole` carries the same sentence, and `UntagRole$TagKeys`/`UntagUser$TagKeys` name the keys to remove. The model does not itself state the case rule; AWS states it in the IAM User Guide the check cites, https://docs.aws.amazon.com/IAM/latest/UserGuide/id_tags.html — an IAM tag key-value pair is not case sensitive, but case is preserved, so a caller cannot have separate `Department` and `department` keys on one user or role: the later TagUser/TagRole REPLACES the existing tag's value, the entry that remains keeps the ORIGINALLY stored key spelling, and UntagUser/UntagRole matches the key in any casing. The rule is key identity under a case-insensitive comparison plus case preservation of the stored key; `Department`/`department`/`CostCenter`/`COSTCENTER` in the recording are just the values the check happens to use. No status or error code is at stake: every call here is answered 2xx and the state the 200s leave behind is wrong.

Emulator: TagUser(UserName=u, Tags=[{Department: finance}]) followed by TagUser(UserName=u, Tags=[{department: hr}]) both return 200, and ListUserTags then returns TWO entries — recorded `[{'Key': 'Department', 'Value': 'finance'}, {'Key': 'department', 'Value': 'hr'}]` — where AWS returns exactly one entry whose Key is the originally stored `Department` and whose Value is the replacing `hr`. The same divergence is in the role path: TagRole with `CostCenter=1234` then `COSTCENTER=5678` leaves two tags instead of one valued `5678`, and UntagRole(TagKeys=['costcenter']) then removes nothing, leaving ListRoleTags non-empty where AWS leaves it empty. One serving component (the tag store/key comparison shared by the user and role tag operations) explains all of it.

Why: a client relying on AWS's behavior over-counts tags and silently keeps stale values. Tag-based access control breaks in the dangerous direction: a caller who re-tags `Department=hr` to move a principal between teams still has the old `Department=finance` key present, so a policy condition on `aws:PrincipalTag/Department` or `aws:ResourceTag/Department` can still match the abandoned value; a cleanup that untags with a differently-cased key believes it removed the tag while the grant it gated is still in force. The 50-tag-per-entity quota is also consumed twice per logical key, so a caller can be refused at half the real limit.

Fix: the IAM tag state and the key comparison used by TagUser/TagRole (upsert: match the existing key case-insensitively, keep the stored key's case, overwrite the value) and by UntagUser/UntagRole and the per-entity tag-count limit check (match case-insensitively) — i.e. the shared tag store for IAM entities, not one operation's handler.

Traces: iam.user-and-role-tag-keys-case-insensitive — assertion at check_iam_user_and_role_tag_keys_case_insensitive.py line 55 (the ListUserTags length assertion); the role-side assertions at lines 72552 and 72557 of the report's inlined source are not reached because the user-side one fails first, so expect them to surface next.

## Evidence
Suite: iam scored 46/70 (66%), 24 failed; 23 of those are owned by open issues, this one is not marked ALREADY FILED or OWNED.

[mismatch] iam.user-and-role-tag-keys-case-insensitive
  check_iam_user_and_role_tag_keys_case_insensitive.py line 55:
    user_tags = iam.list_user_tags(UserName=user)["Tags"]
    assert len(user_tags) == 1
  AssertionError: Department and department are one key on a user, so one tag must remain: [{'Key': 'Department', 'Value': 'finance'}, {'Key': 'department', 'Value': 'hr'}]

Expected (AWS), after TagUser Department=finance then TagUser department=hr on the same user:
  HTTP 200 from both TagUser calls; ListUserTags -> Tags == [{'Key': 'Department', 'Value': 'hr'}] (one entry, the stored key keeps its original case, the value is replaced).
  Then on a role: TagRole CostCenter=1234, TagRole COSTCENTER=5678 -> ListRoleTags values == ['5678'] (one entry); UntagRole TagKeys=['costcenter'] -> ListRoleTags == [].
Actual (emulator):
  Both TagUser calls 200; ListUserTags -> two entries, [{'Key': 'Department', 'Value': 'finance'}, {'Key': 'department', 'Value': 'hr'}] — the second tag was inserted as a new key rather than overwriting the first.

No HTTP status, error code or error message diverges; the divergence is in the state the successful calls leave and in the ListUserTags/ListRoleTags bodies read back from it. The specific key spellings and values in the recording fill the rule's variable slots and are not themselves requirements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateUser, then TagUser(UserName=<user>, Tags=[{Key: 'Department', Value: 'finance'}]) and TagUser(UserName=<user>, Tags=[{Key: 'department', Value: 'hr'}]).
3. ListUserTags(UserName=<user>).
   -> AWS: exactly one tag, Key 'Department' (original case preserved), Value 'hr'.
   -> Emulator: two tags, 'Department'='finance' and 'department'='hr'.
4. CreateRole, then TagRole(Tags=[{Key: 'CostCenter', Value: '1234'}]) and TagRole(Tags=[{Key: 'COSTCENTER', Value: '5678'}]); ListRoleTags.
   -> AWS: one tag valued '5678'. -> Emulator: two tags.
5. UntagRole(RoleName=<role>, TagKeys=['costcenter']); ListRoleTags.
   -> AWS: []. -> Emulator: the 'CostCenter' tag is still present.
6. Re-replay check iam.user-and-role-tag-keys-case-insensitive to confirm the fix.
