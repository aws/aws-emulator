Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: e47284ab37991d11b5f4fdcc5d2b5ad1491b8a6a

# An uncomputable deletion-schedule instant is answered with the 400 that blames PendingWindowDays, where the crate's rule (and the same handler's sibling instant) answers the modelled server error

_Filed: 2026-11-27T09:46:06Z_
_Operation: StartActiveApprovalTeamDeletion | Class: error-response_

> _Note: this issue was identified against the code base at commit `e47284ab37991d11b5f4fdcc5d2b5ad1491b8a6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::deletion_schedule` answers `None` for TWO unrelated reasons: the request's `PendingWindowDays` is outside the window this emulator will record (a CLIENT fault), and the checked clock arithmetic not being representable for the handler's `now` (the SERVER-side condition every other clock instant in this crate is guarded for).

`StartActiveApprovalTeamDeletion` collapses both into one answer: a 400 `ValidationException` whose message states `PendingWindowDays (<days>) must be between 1 and 36500 days.` So for the arithmetic cause the refusal blames a request member that is in range and names a rule the request did not break.

That contradicts the rule this crate states for exactly this class — and `Approver::invited`'s doc names `deletion_schedule` as one of the constructions the rule covers: "the calling handler answers a `None` with the server error it declares". Every sibling does that: `UpdateApprovalTeam` answers `InternalServerException` for an uncomputable approval instant, `CreateIdentitySource` for an uncomputable activation instant, `DeleteIdentitySource` for an uncomputable removal instant, `CreateApprovalTeam` for an uncomputable invitation response. The sharpest comparison is inside THIS handler: the other clock instant it computes (the deletion session's expiration, through `start_session`) is answered with `InternalServerException` ("the session's expiration instant could not be computed"), which this operation declares (`smithy_query(query="operation",operation="StartActiveApprovalTeamDeletion")` lists `InternalServerException`, httpError 500). So one handler answers two classes for one failure class.

No behavioural impact with a real clock (the arithmetic cause needs a `now` near `SystemTime`'s ceiling, which is why this is LOW): the defect is that the two causes are indistinguishable at the one call site, so the one reachable cause carries the right sentence only by accident and the other is misclassified as a client fault.

## Evidence

The single call site that conflates both causes — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/start_active_approval_team_deletion.rs:216-235:
```
        // The whole schedule — when the team's answer comes due, and when the window that
        // answer starts ends — is computed BEFORE anything is mutated, with checked
        // arithmetic throughout, so a request can neither panic this handler nor leave a
        // team half-moved into DELETING.
        let days = input
            .pending_window_days
            .unwrap_or(crate::state::DEFAULT_PENDING_WINDOW_DAYS);
        let Some(schedule) = crate::state::deletion_schedule(now, days) else {
            return Err(
                StartActiveApprovalTeamDeletionErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "PendingWindowDays ({days}) must be between 1 and {} days.",
                            crate::state::MAX_PENDING_WINDOW_DAYS
                        ),
                    },
                )
                .into(),
            );
        };
```

The two distinct `None` causes — src/state.rs:1122-1127 and :1148-1153:
```
pub fn deletion_completion(start: SystemTime, days: i32) -> Option<SystemTime> {
    if !(1..=MAX_PENDING_WINDOW_DAYS).contains(&days) {
        return None;
    }
    start.checked_add(pending_window(days)?)
}
```
```
pub fn deletion_schedule(start: SystemTime, days: i32) -> Option<DeletionSchedule> {
    let approval_due = start.checked_add(DELETION_APPROVAL_DELAY)?;
    Some(DeletionSchedule {
        approval_due,
        completion_time: deletion_completion(approval_due, days)?,
    })
}
```

The crate's stated rule for this class, naming `deletion_schedule` — src/state.rs:1348-1356 (`Approver::invited`):
```
    /// The instant is computed with CHECKED arithmetic, like every other clock-owed
    /// instant this module records ([`PendingTeamUpdate::requested`],
    /// [`Session::for_team`], [`deletion_schedule`]): the calling handler answers a
    /// `None` with the server error it declares, so no approver is ever stored waiting on
    /// a response owed at no instant at all and no request can panic part-way through an
    /// approver set.
```

The same handler answering its OTHER clock instant the way that rule says — start_active_approval_team_deletion.rs:281-298:
```
                let Some(session_arn) = state.start_session(
...
                ) else {
                    return Err(
                        StartActiveApprovalTeamDeletionErrorModeled::InternalServerException(
                            InternalServerException {
                                message: "the session's expiration instant could not be \
                                          computed"
                                    .to_string(),
```

And the sibling write path — src/ops/update_approval_team.rs:370-380:
```
            if now
                .checked_add(crate::state::UPDATE_APPROVAL_DELAY)
                .is_none()
            {
                return Err(UpdateApprovalTeamErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the update's approval instant could not be computed".to_string(),
                    },
                )
                .into());
            }
```
(the same shape in src/ops/create_identity_source.rs:336-351 and src/ops/delete_identity_source.rs:306-316).

Test coverage: tests/all/clock_due_instant_arithmetic.rs drives `Owed::DeletionApproval` through `state::deletion_schedule(now, state::DEFAULT_PENDING_WINDOW_DAYS)` at `last_representable_instant()` (lines 183-186, 79-104) and asserts only that the construction records nothing — grepping that suite for `ValidationException|InternalServer` returns no match, so nothing pins (or justifies) the 400 this handler answers for the arithmetic cause. The in-range refusal itself is justified by MAX_PENDING_WINDOW_DAYS' own doc (state.rs:429-437) and is NOT what this issue asks to change.

## Steps to Reproduce

1. Read src/ops/start_active_approval_team_deletion.rs:223-235: the one `else` branch of `deletion_schedule(now, days)` returns `ValidationException` naming `PendingWindowDays`.
2. Read src/state.rs:1148-1153 with :1122-1127: `deletion_schedule` answers `None` both when `days` is outside `1..=MAX_PENDING_WINDOW_DAYS` and when `start.checked_add(DELETION_APPROVAL_DELAY)` / `approval_due.checked_add(window)` is not representable.
3. Read src/state.rs:1348-1356: the crate's rule for a `None` from these checked constructions — `deletion_schedule` named explicitly — is that "the calling handler answers a `None` with the server error it declares".
4. Compare start_active_approval_team_deletion.rs:281-298 (same handler, uncomputable session expiration -> InternalServerException) and update_approval_team.rs:370-380 / create_identity_source.rs:336-351 / delete_identity_source.rs:306-316.
5. Confirm the server error is available to this operation: smithy_query(query="operation",operation="StartActiveApprovalTeamDeletion") lists InternalServerException (httpError 500).
6. Exercise the arithmetic cause through the store path the suite already uses (tests/all/clock_due_instant_arithmetic.rs::last_representable_instant): `state::deletion_schedule(last_representable_instant(), 7)` is `None` with a `days` value of 7, i.e. one the refusal's own sentence calls valid.
