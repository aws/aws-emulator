// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/configuration_session_poll.rs"]
mod configuration_session_poll;
#[path = "all/deletion_does_not_break_a_bound_session.rs"]
mod deletion_does_not_break_a_bound_session;
#[path = "all/feature_flag_resolution.rs"]
mod feature_flag_resolution;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/name_bindings_survive_renames.rs"]
mod name_bindings_survive_renames;
#[path = "all/next_poll_token_poll_rate.rs"]
mod next_poll_token_poll_rate;
#[path = "all/rollout_percentage_splits_sessions.rs"]
mod rollout_percentage_splits_sessions;
#[path = "all/session_requires_a_deployment.rs"]
mod session_requires_a_deployment;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_mid_rollout_withdraws_the_release.rs"]
mod stop_mid_rollout_withdraws_the_release;
#[path = "all/token_validation_and_lifetime.rs"]
mod token_validation_and_lifetime;
// A clock-driven suite: it waits out a rollout's growth intervals rather than writing a
// percentage into the store, so its file name sorts it last.
#[path = "all/zz_timing.rs"]
mod zz_timing;
