// SPDX-License-Identifier: CC0-1.0

//! The data-freshness window a channel destination delivers on: the rule that belongs to
//! `DataFreshnessInSeconds`, wherever a destination carries it.
//!
//! `DataFreshnessInSeconds` is ONE member shape
//! (`smithy_query(query="shape",shape="DataFreshnessInSeconds")` — a plain integer carrying no
//! `@range`), reached from the general purpose bucket configuration, the streaming table
//! configuration and both update inputs, and it means the same thing on each of them: "The
//! maximum age, in seconds, of undelivered data before the channel delivers it to the
//! destination. The default value is 300 seconds"
//! (`S3DestinationConfiguration$DataFreshnessInSeconds`, worded identically on
//! `S3TablesDestinationConfiguration$DataFreshnessInSeconds`). The window the service delivers
//! on is therefore the MEMBER's, not one destination kind's, so the bounds, the sentence that
//! reports them and the single reading of them all live here, and every operation that sets the
//! window — the create that names a destination and the update that moves its window — shares
//! them. That is what stops a window one of them refuses from being reachable through the
//! other, or from being stored on one destination kind and refused on the other.

/// The maximum age of undelivered data a destination gets when the request names none: "The
/// maximum age, in seconds, of undelivered data before the channel delivers it to the
/// destination. The default value is 300 seconds"
/// (`S3DestinationConfiguration$DataFreshnessInSeconds`).
pub const DEFAULT_DATA_FRESHNESS_SECONDS: i32 = 300;

/// The shortest data-freshness window a channel destination delivers on.
///
/// The model states no bound — the member targets a plain integer and its documentation names
/// only the default — but the service documents the window a destination delivers on as 300 to
/// 900 seconds, and a value outside it names a window no destination has delivery behaviour
/// for, so the service refuses it instead of storing it.
pub const MIN_DATA_FRESHNESS_SECONDS: i32 = 300;

/// The longest data-freshness window a channel destination delivers on: the upper end of the
/// same documented window as [`MIN_DATA_FRESHNESS_SECONDS`].
pub const MAX_DATA_FRESHNESS_SECONDS: i32 = 900;

/// How a create or an update refuses a destination whose data-freshness window falls outside
/// the one the service delivers on.
///
/// FIXED text around the two bounds of the member: what it reports is which window a
/// destination admits, so the values interpolated are [`MIN_DATA_FRESHNESS_SECONDS`] and
/// [`MAX_DATA_FRESHNESS_SECONDS`] — the member's own bounds — and never the value the refused
/// request carried, nor which destination kind carried it.
pub fn data_freshness_window_message() -> String {
    format!(
        "DataFreshnessInSeconds must be between {MIN_DATA_FRESHNESS_SECONDS} and \
         {MAX_DATA_FRESHNESS_SECONDS}."
    )
}

/// The refusal `seconds` earns as a destination's data-freshness window, or `None` for a window
/// the service delivers on. Both bounds are INCLUSIVE: the window is closed at each end, so the
/// ends themselves are served.
///
/// One reading of that window for every destination kind and every operation that sets it, so
/// none of them can leave a stored channel delivering on a window another one refuses.
pub fn data_freshness_refusal(seconds: i32) -> Option<String> {
    (!(MIN_DATA_FRESHNESS_SECONDS..=MAX_DATA_FRESHNESS_SECONDS).contains(&seconds))
        .then(data_freshness_window_message)
}
