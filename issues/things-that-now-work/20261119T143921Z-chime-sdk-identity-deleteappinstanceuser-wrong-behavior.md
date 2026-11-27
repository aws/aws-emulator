Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a0a325cd7a8f100e0b15b3c52772a25e70aa00e8

# Deleting an AppInstanceUser (or Bot) leaves its admin promotion in the shared directory, so a deleted identity is still reported as an administrator

_Filed: 2026-11-19T14:39:21Z_
_Operation: DeleteAppInstanceUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a0a325cd7a8f100e0b15b3c52772a25e70aa00e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteAppInstanceUser` keeps the two other shared Chime directories in step with the delete — it drops the resource's tags and its published identity name — but never drops the promotion the shared app-instance ADMIN directory holds for that identity, even though the crate already uses that directory's `demote` helper in `DeleteAppInstanceAdmin`. The record outlives the identity, so after deleting a promoted user (under an app instance that is still alive) `DescribeAppInstanceAdmin` still answers 200 for the pair, and `ListAppInstanceAdmins` still lists the dead user's ARN — with `Identity$Name` now absent, because `identity_name` can no longer resolve the deleted record. `DeleteAppInstanceBot` has the same omission, and `DeleteAppInstance`'s cascade (which does cascade tags and names) leaves every promotion of the instance behind as well.

## Evidence

The handler keeps two shared directories in step and ignores the third —
services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/delete_app_instance_user.rs:182-190,221-222:
```
        // A deleted identity leaves no published name behind either, so the family's shared
        // identity NAME directory is resolved before the delete and the entry dropped with
        // the resource.
        let name_directory = crate::state::identity_names(&*self.engine).ok_or_else(|| {
...
        tag_directory.lock().forget(&input.app_instance_user_arn);
        name_directory.lock().forget(&input.app_instance_user_arn);
```
There is no `app_instance_admins(..)` / `demote(..)` call in the file: `grep -n 'demote|app_instance_admins\('` over `src/` matches only create_app_instance_admin.rs:172, delete_app_instance_admin.rs:191,220, list_app_instance_admins.rs:160 and describe_app_instance_admin.rs:160.

The helper the delete path skips is already used by a sibling —
src/ops/delete_app_instance_admin.rs:216-220:
```
        // only the promotion is dropped. `demote` answers whether a record was there,
        // which is deliberately discarded: the answer is 204 either way.
        directory
            .lock()
            .demote(&input.app_instance_arn, &input.app_instance_admin_arn);
```
and is declared in services/common/src/chime_app_instance_admins.rs:119 (`pub fn demote(&mut self, app_instance_arn: &str, admin_arn: &str) -> bool`).

The stale record is observable because the admin readers consult the directory first and the store only for the NAME —
src/ops/describe_app_instance_admin.rs:195-208:
```
        let record = directory
            .lock()
            .record(&input.app_instance_arn, &input.app_instance_admin_arn)
...
        let name = state.lock().identity_name(&record.admin_arn);
```
and src/ops/list_app_instance_admins.rs:223-230 projects `Identity { arn: record.admin_arn, name: store.identity_name(..) }` for every record the directory holds.

The crate's own doc states the rule the delete path breaks — src/state.rs:730-732: "This service's own record stays the authority, so every operation that registers, renames or deletes an identity keeps the directory in step with it".

Test coverage: tests/all/app_instance_admin_demotion.rs pins that an explicit demotion removes the record from both readers (lines 235-292), but no test deletes a promoted identity, so nothing pins the current stale answer.

## Steps to Reproduce

1. CreateAppInstance -> A
2. CreateAppInstanceUser under A -> U
3. POST /app-instances/{A}/admins with AppInstanceAdminArn=U -> 201
4. DELETE /app-instance-users/{U} -> 204
5. GET /app-instance-users/{U} -> 400 NotFound (the user is gone)
6. GET /app-instances/{A}/admins/{U} (DescribeAppInstanceAdmin) -> still 200 with AppInstanceAdmin.Admin.Arn = U and no Name
7. GET /app-instances/{A}/admins (ListAppInstanceAdmins) -> the deleted user's ARN is still listed as an administrator
