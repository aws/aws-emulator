Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 04f6987df4e1a49fd4a14706704bdf2fcd5b4884

# The per-item attendee admission rule is implemented twice, in BatchCreateAttendee and CreateMeetingWithAttendees

_Filed: 2026-11-27T07:13:42Z_
_Operation: BatchCreateAttendee | Class: maintainability_

> _Note: this issue was identified against the code base at commit `04f6987df4e1a49fd4a14706704bdf2fcd5b4884`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Admitting one `CreateAttendeeRequestItem` into a meeting is a five-step rule: (1) refuse the item's `ExternalUserId` against the documented pattern/reserved prefix and report it in `Errors`; (2) judge a request-supplied `Capabilities` set against the meeting's `MeetingFeatures` BEFORE resolving the identity, and report a refusal in `Errors`; (3) resolve the identity's existing attendee (`ExternalUserId` is an idempotency token) and report that attendee; (4) refuse the item in `Errors` when the meeting is at its attendee ceiling; (5) mint the `AttendeeId` and `JoinToken`, insert, and read the stored record back.

That rule is written out twice in this crate's own code — `src/ops/batch_create_attendee.rs:197-298` and `src/ops/create_meeting_with_attendees.rs:333-428` — as two near-identical loop bodies differing only in which error enum a store failure is lifted into and in whether the `MeetingId` comes from the request or from the meeting just minted. (`src/ops/create_attendee.rs:188-287` is a third copy of the same sequence for the single-item route, with whole-call errors instead of `Errors` entries.)

The copies can drift, and the ORDER of steps 2 and 3 is exactly what the two copies' own comments promise is identical across all three routes — a promise nothing in the code enforces. The repository history shows the drift has already happened once per copy: the same missing "judge capabilities before resolving the repeated `ExternalUserId`" step had to be reported and fixed separately for each of the two copies (`20261119T233612Z-chime-sdk-meetings-batchcreateattendee-validation.md` and `20261119T233612Z-chime-sdk-meetings-createmeetingwithattendees-validation.md`), because fixing one copy left the other wrong.

The fix is to state the rule once — one helper taking the store, the meeting id, the meeting's features, the ceiling and one request item, answering either the stored `Attendee` or the `CreateAttendeeError2` entry — and have both loops (and, for its single-item answer, `CreateAttendee`) call it. Every response byte and every existing test stays unchanged: the helper returns the same payloads the three copies build today, all of them already minted in one place (`crate::state::external_identifier_refusal`, `crate::state::capability_problem`, `crate::state::bad_request`, `crate::state::attendee_limit_reached`).

## Evidence

Copy 1 — `src/ops/batch_create_attendee.rs:197-270` (steps 1-4):
```rust
        for item in input.attendees {
            // The documented rules on `ExternalUserId` ... read as a per-item failure ...
            if let Some(refusal) = crate::state::external_identifier_refusal(
                crate::state::EXTERNAL_USER_ID_MEMBER,
                &item.external_user_id,
            ) {
                errors.push(CreateAttendeeError2 { external_user_id: Some(item.external_user_id), error_code: refusal.code, error_message: refusal.message });
                continue;
            }
            ...
            let capabilities = match item.capabilities {
                Some(requested) => {
                    match crate::state::capability_problem(&requested, features.as_ref()) {
                        Some(problem) => { let refusal = crate::state::bad_request(problem); errors.push(CreateAttendeeError2 { ... }); continue; }
                        None => requested,
                    }
                }
                None => crate::state::default_capabilities(features.as_ref()),
            };
            let existing = store.meetings.get(&input.meeting_id)
                .and_then(|stored| stored.attendee_with_external_user_id(&item.external_user_id)).cloned();
            if let Some(existing) = existing { created.push(existing); continue; }
            let admitted = store.meetings.get(&input.meeting_id).map_or(0, |stored| stored.attendees.len());
            if admitted >= ceiling { let refusal = crate::state::attendee_limit_reached(&input.meeting_id, ceiling); errors.push(CreateAttendeeError2 { ... }); continue; }
```
and step 5 at `batch_create_attendee.rs:272-297` (`mint_attendee_id` / `mint_join_token` / `attendees.insert` / read the stored record back).

Copy 2 — `src/ops/create_meeting_with_attendees.rs:333-400`, the same five steps in the same order:
```rust
        for item in input.attendees {
            if let Some(refusal) = crate::state::external_identifier_refusal(
                crate::state::EXTERNAL_USER_ID_MEMBER,
                &item.external_user_id,
            ) { errors.push(CreateAttendeeError2 { ... }); continue; }
            let capabilities = match item.capabilities {
                Some(requested) => { match crate::state::capability_problem(&requested, features.as_ref()) { Some(problem) => { let refusal = crate::state::bad_request(problem); errors.push(CreateAttendeeError2 { ... }); continue; } None => requested } }
                None => crate::state::default_capabilities(features.as_ref()),
            };
            let existing = store.meetings.get(&meeting_id)
                .and_then(|stored| stored.attendee_with_external_user_id(&item.external_user_id)).cloned();
            if let Some(existing) = existing { created.push(existing); continue; }
            let admitted = store.meetings.get(&meeting_id).map_or(0, |stored| stored.attendees.len());
            if admitted >= ceiling { let refusal = crate::state::attendee_limit_reached(&meeting_id, ceiling); errors.push(CreateAttendeeError2 { ... }); continue; }
```
with the identical step 5 at `create_meeting_with_attendees.rs:402-427`.

Each copy's comment asserts the invariant the duplication puts at risk — `src/ops/batch_create_attendee.rs:213-219`:
```
            // A capability set the ITEM carries is judged against the meeting's features
            // BEFORE this identity's stored attendee is resolved ... A single
            // `CreateAttendee` carrying this item judges it in exactly this order.
```
`src/ops/create_meeting_with_attendees.rs:348-353`:
```
            // A capability set the ITEM carries is judged against the meeting's features
            // BEFORE this identity's attendee is resolved ... The same order `CreateAttendee`
            // and `BatchCreateAttendee` judge an item in.
```
and the third copy makes the same promise from the single-item side — `src/ops/create_attendee.rs:214-220`: "The two batch creation paths judge an item in this order too, so one item gets one answer whichever of the three it is sent through."

Second duplication in the same pair of files (same class, same fix shape): the whole meeting-record construction and create-time tag write — `src/ops/create_meeting.rs:244-287` and `:303-311` versus `src/ops/create_meeting_with_attendees.rs:281-320` and `:443-451` — are the same `Meeting { .. }` literal, the same `remember_meeting_create` + `meetings.insert`, and the same `tag_directory.lock().tag(&meeting_arn, tags)` block.

Tests that pin the behaviour the consolidation must not change: `tests/all/batch_create_attendee.rs`, `tests/all/create_meeting_with_attendees.rs`, `tests/all/capability_resolution_catalog.rs` ("the whole catalog of modelled resolutions … whichever operation supplied the set"), `tests/all/capability_refusal_message.rs`, `tests/all/external_user_id_rules.rs` and `tests/all/attendee_capabilities.rs` — all drive the real HTTP path, so they verify the responses stay byte-identical.

## Steps to Reproduce

1. Open `src/ops/batch_create_attendee.rs` at lines 197-298 and `src/ops/create_meeting_with_attendees.rs` at lines 333-428 side by side: the five admission steps appear in the same order with the same helper calls, differing only in the error enum named on a store failure and in where the `MeetingId` comes from.
2. Read the comment at `batch_create_attendee.rs:213-219` and the one at `create_meeting_with_attendees.rs:348-353`: each claims the other path judges an item in the same order — an invariant held only by the two copies happening to match.
3. Confirm the copies have already drifted apart once: the same missing step had to be fixed separately in each copy (issues `20261119T233612Z-chime-sdk-meetings-batchcreateattendee-validation` and `20261119T233612Z-chime-sdk-meetings-createmeetingwithattendees-validation`).
4. Extract one helper over (store, meeting id, features, ceiling, request item) returning either the stored `Attendee` or the `CreateAttendeeError2` entry; call it from both loops and from `CreateAttendee`'s single-item path.
5. Run the crate's suite (`tests/all/batch_create_attendee.rs`, `create_meeting_with_attendees.rs`, `capability_resolution_catalog.rs`, `capability_refusal_message.rs`, `external_user_id_rules.rs`, `attendee_capabilities.rs`): every response must be unchanged.
