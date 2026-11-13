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
#[path = "all/authorization_before_policy_parsing.rs"]
mod authorization_before_policy_parsing;

#[path = "all/decode_authorization_message.rs"]
mod decode_authorization_message;

#[path = "all/delegated_access_token.rs"]
mod delegated_access_token;

#[path = "all/encoded_message_decoding.rs"]
mod encoded_message_decoding;

#[path = "all/malformed_session_policy.rs"]
mod malformed_session_policy;

#[path = "all/mfa_challenged_mints.rs"]
mod mfa_challenged_mints;

#[path = "all/packed_policy_budget.rs"]
mod packed_policy_budget;

#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;

#[path = "all/query_parameter_signing.rs"]
mod query_parameter_signing;

#[path = "all/refusal_message_bytes.rs"]
mod refusal_message_bytes;

#[path = "all/request_admission.rs"]
mod request_admission;

#[path = "all/saml_federation.rs"]
mod saml_federation;

#[path = "all/session_audit_records.rs"]
mod session_audit_records;

#[path = "all/session_policy_limits.rs"]
mod session_policy_limits;

#[path = "all/session_provenance_chain.rs"]
mod session_provenance_chain;

#[path = "all/session_record_construction.rs"]
mod session_record_construction;

#[path = "all/session_tag_conditions.rs"]
mod session_tag_conditions;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/sts_authorization.rs"]
mod sts_authorization;

#[path = "all/sts_refusal_wording.rs"]
mod sts_refusal_wording;

#[path = "all/sts_sessions.rs"]
mod sts_sessions;

#[path = "all/trust_policy_convergence.rs"]
mod trust_policy_convergence;

#[path = "all/web_identity_federation.rs"]
mod web_identity_federation;

#[path = "all/web_identity_verification.rs"]
mod web_identity_verification;
