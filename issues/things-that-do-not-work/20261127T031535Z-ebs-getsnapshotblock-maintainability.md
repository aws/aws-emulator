Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0ebf0829cad241d824be0f94d62bd70dc38785d4

# The snapshot-id resolution rule (malformed parameter vs not-found vs not-readable) is implemented five times, once as a helper and four times inline

_Filed: 2026-11-27T03:15:35Z_
_Operation: GetSnapshotBlock | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0ebf0829cad241d824be0f94d62bd70dc38785d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has ONE rule for turning a `SnapshotId` a request carried into a stored snapshot: look the id up; if it does not resolve (and, on the read path, if the snapshot's content is not final) then a value that is not even shaped like a snapshot id is the modelled `ValidationException` with reason `INVALID_SNAPSHOT_ID` and `malformed_snapshot_id_message(<parameter>, value)`, while a well-formed id is the modelled `ResourceNotFoundException` with reason `SNAPSHOT_NOT_FOUND` and `snapshot_not_found_message(value)`.

`list_changed_blocks.rs` already factors that rule into a helper, `resolve()`, and its doc says the refusals are "the ones every operation of this service makes for a snapshot id it cannot resolve". Four sibling handlers nevertheless carry their own inline copy of the same branch: `get_snapshot_block.rs`, `list_snapshot_blocks.rs` (both including the `is_readable()` filter, exactly as `resolve()` does), and `put_snapshot_block.rs` / `complete_snapshot.rs` (the same two-way malformed/not-found branch without the readable filter).

Five copies of one rule can drift: a change to which refusal a malformed id earns, or to the order the two conditions are judged in, has to be made in five places, and a copy left behind would answer one operation's caller a 404 where its siblings answer a 400 (or the reverse). The crate already has the shape a shared version needs: `crate::state::page_resume_index` returns a neutral `PageTokenRefusal { reason, message }` that each operation lifts into its OWN error enum, which is precisely the obstacle (per-operation error types) that keeps this rule inlined today.

A fix hoists the resolution into `crate::state` behind a neutral refusal value (as `PageTokenRefusal` already does) and has all five call sites use it. Every response — status, error code, `Reason` and `Message` — and every existing test stay exactly as they are; this is a de-duplication, not a behaviour change.

## Evidence

The rule, already factored, with the doc stating it is shared by every operation — services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs:338-363:
```
/// The stored snapshot `snapshot_id` names, as the READ path resolves one.
///
/// The refusals are the ones every operation of this service makes for a snapshot id it
/// cannot resolve: a value that is not even shaped like a snapshot id is a malformed
/// PARAMETER naming the member it arrived in, and a well-formed id that resolves to
/// nothing — or to a snapshot whose content is not final — is the snapshot not-found 404.
fn resolve<'store>(... ) -> Result<&'store Snapshot, ListChangedBlocksError> {
    if let Some(snapshot) = store.snapshots.get(snapshot_id).filter(|snapshot| snapshot.is_readable()) { return Ok(snapshot); }
    if !is_well_formed_snapshot_id(snapshot_id) {
        return Err(validation("INVALID_SNAPSHOT_ID", malformed_snapshot_id_message(parameter, snapshot_id)));
    }
    Err(not_found(snapshot_not_found_message(snapshot_id)))
}
```

Copy 2 — services/service-com-amazonaws-ebs-ebs/src/ops/get_snapshot_block.rs:226-240:
```
        let readable = store
            .snapshots
            .get(&input.snapshot_id)
            .filter(|snapshot| snapshot.is_readable());
        let Some(snapshot) = readable else {
            if !is_well_formed_snapshot_id(&input.snapshot_id) {
                return Err(validation(
                    "INVALID_SNAPSHOT_ID",
                    malformed_snapshot_id_message("snapshotId", &input.snapshot_id),
                ));
            }
            return Err(not_found(snapshot_not_found_message(&input.snapshot_id)));
        };
```

Copy 3 — services/service-com-amazonaws-ebs-ebs/src/ops/list_snapshot_blocks.rs:204-229:
```
        let Some(snapshot) = store
            .snapshots
            .get(&input.snapshot_id)
            .filter(|snapshot| snapshot.is_readable())
        else {
            if !crate::state::is_well_formed_snapshot_id(&input.snapshot_id) {
                return Err(validation(
                    "INVALID_SNAPSHOT_ID",
                    crate::state::malformed_snapshot_id_message(
                        "snapshotId",
                        &input.snapshot_id,
                    ),
                ));
            }
            ...
            return Err(not_found(crate::state::snapshot_not_found_message(
                &input.snapshot_id,
            )));
        };
```

Copy 4 — services/service-com-amazonaws-ebs-ebs/src/ops/put_snapshot_block.rs:268-278 (same branch, no readable filter):
```
        let Some(snapshot) = store.snapshots.get(&input.snapshot_id) else {
            if !is_well_formed_snapshot_id(&input.snapshot_id) {
                return Err(validation(
                    "INVALID_SNAPSHOT_ID",
                    malformed_snapshot_id_message("snapshotId", &input.snapshot_id),
                ));
            }
            return Err(not_found(snapshot_not_found_message(&input.snapshot_id)));
        };
```

Copy 5 — services/service-com-amazonaws-ebs-ebs/src/ops/complete_snapshot.rs:229-239 (byte-for-byte the same branch as copy 4):
```
        let Some(snapshot) = store.snapshots.get_mut(&input.snapshot_id) else {
            if !is_well_formed_snapshot_id(&input.snapshot_id) {
                return Err(validation(
                    "INVALID_SNAPSHOT_ID",
                    malformed_snapshot_id_message("snapshotId", &input.snapshot_id),
                ));
            }
            return Err(not_found(snapshot_not_found_message(&input.snapshot_id)));
        };
```

The crate's own precedent for sharing a refusal across operations with different error enums — services/service-com-amazonaws-ebs-ebs/src/state.rs:1146-1158:
```
/// A page token this service did not issue, as the reason and the message the refusing
/// operation's own modelled `ValidationException` carries ([`page_resume_index`]).
///
/// The two travel together so that neither listing can refuse the same condition under a
/// different reason or in different words.
pub struct PageTokenRefusal { pub reason: &'static str, pub message: String }
```

Existing test coverage that must stay green and unchanged:
- services/service-com-amazonaws-ebs-ebs/tests/all/read_path_requires_a_completed_snapshot.rs (its `assert_not_found` is reused by the other suites)
- services/service-com-amazonaws-ebs-ebs/tests/all/refusal_order.rs (`UNSTORED_ID` vs `MALFORMED_ID` rows across the write and read paths)
- services/service-com-amazonaws-ebs-ebs/tests/all/shared_listing_pagination.rs:434-464 `an_unresolvable_snapshot_is_refused_before_the_page_token_is_judged` (404 + `SNAPSHOT_NOT_FOUND` through BOTH listings)
- services/service-com-amazonaws-ebs-ebs/tests/all/a_removed_snapshot_resolves_nowhere.rs

## Steps to Reproduce

1. Open services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs and read the `resolve` helper at lines 338-363, whose doc states the refusals are "the ones every operation of this service makes for a snapshot id it cannot resolve".
2. Open services/service-com-amazonaws-ebs-ebs/src/ops/get_snapshot_block.rs:226-240 and services/service-com-amazonaws-ebs-ebs/src/ops/list_snapshot_blocks.rs:204-229: each re-implements that helper inline, readable filter included.
3. Open services/service-com-amazonaws-ebs-ebs/src/ops/put_snapshot_block.rs:268-278 and services/service-com-amazonaws-ebs-ebs/src/ops/complete_snapshot.rs:229-239: each re-implements the malformed-vs-not-found half inline, identically.
4. Change the rule in one copy only (for example swap the two branches so a malformed id answers 404): four operations keep the old answer and two callers of the same bad id now see different error classes from sibling operations — nothing in the crate forces the five copies to agree.
5. Compare with services/service-com-amazonaws-ebs-ebs/src/state.rs:1146-1190, where the page-token rule is stated once and each operation lifts the neutral `PageTokenRefusal` into its own error enum — the pattern this rule needs.
