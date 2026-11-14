// SPDX-License-Identifier: CC0-1.0

//! The WINDOW a permission set's `SessionDuration` has to fall in, and the refusal a value
//! outside it earns.
//!
//! # The rule
//!
//! `SessionDuration` is "the length of time that the application user sessions are valid in
//! the ISO-8601 standard", and IAM Identity Center documents that length as being between
//! ONE HOUR and TWELVE HOURS ([`MIN_SESSION_DURATION`], [`MAX_SESSION_DURATION`]) — so a
//! request naming a span outside that window names a session length no permission set can
//! hold, and is refused with the modelled `ValidationException` (HTTP 400) before anything
//! is stored.
//!
//! The model cannot express the window: `Duration`
//! (`smithy_query(query="shape",shape="Duration")`) carries a LENGTH bound and an ISO-8601
//! FORM pattern (`^(-?)P(?=\d|T\d)…$`) and no range at all, and that pattern admits a
//! leading `-`, a bare calendar designator and a zero-length span — so the boundary
//! validator passes `PT24H`, `P1Y`, `PT0S` and `-PT1H`, and the window is the handler's to
//! ask. It is asked of the REQUEST alone, so a refused create mints no permission set and a
//! refused update leaves the stored record exactly as it was.
//!
//! The span a value names is read by the ONE shared ISO 8601 duration reading of this tree
//! (`service_common::iso8601_span`, over the maintained `iso8601-duration` crate), so this
//! module decides only WHICH spans the service serves. A value that names no fixed span at
//! all — a negative duration, or a calendar designator whose length depends on the calendar
//! (`P1Y`, `P1M`, and the week form the pattern admits) — is outside the window too: none of
//! them is a span between one and twelve hours.

use std::time::Duration;

/// The SHORTEST session a permission set can be given: one hour.
///
/// SOURCE: IAM Identity Center's published session-duration range for a permission set (1
/// hour to 12 hours). Both ends are INCLUSIVE — a request naming exactly this span is
/// served.
pub const MIN_SESSION_DURATION: Duration = Duration::from_secs(60 * 60);

/// The LONGEST session a permission set can be given: twelve hours, the other end of the
/// same published range.
pub const MAX_SESSION_DURATION: Duration = Duration::from_secs(12 * 60 * 60);

/// The refusal the `SessionDuration` of a permission-set write earns, or `None` when the
/// request names a session length the service serves.
///
/// `None` for an ABSENT member: `SessionDuration` is optional, and a create that omits it
/// stores no session duration while an update that omits it leaves the stored one alone —
/// neither names a length to be judged.
///
/// The ONE place the window is asked, so `CreatePermissionSet` and `UpdatePermissionSet`
/// cannot disagree about which lengths a permission set can hold.
#[must_use]
pub fn session_duration_refusal(session_duration: Option<&str>) -> Option<String> {
    let value = session_duration?;
    match ::service_common::iso8601_span::iso8601_span(value) {
        Some(span) if (MIN_SESSION_DURATION..=MAX_SESSION_DURATION).contains(&span) => None,
        // Either a span outside the window, or a value naming no fixed span at all: the
        // same refusal, because neither is a session length between the two bounds.
        _ => Some(session_duration_window_message(value)),
    }
}

/// The message that refusal carries, as the message of the modelled `ValidationException`
/// (HTTP 400).
///
/// Built from the request and the window: the fixed half states the requirement, the value
/// half is the duration the caller sent, and the two bounds are read off
/// [`MIN_SESSION_DURATION`] and [`MAX_SESSION_DURATION`] in hours, so the sentence always
/// states the window the request was judged against.
#[must_use]
pub fn session_duration_window_message(value: &str) -> String {
    let minimum_hours = MIN_SESSION_DURATION.as_secs() / 3_600;
    let maximum_hours = MAX_SESSION_DURATION.as_secs() / 3_600;
    format!(
        "Invalid session duration {value}. Session duration must be between \
         {minimum_hours} hour and {maximum_hours} hours."
    )
}
