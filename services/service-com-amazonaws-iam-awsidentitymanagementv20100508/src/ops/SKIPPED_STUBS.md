# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

// SKIPPED: DeleteAccountPasswordPolicy — unsupported shape construct: operation 'DeleteAccountPasswordPolicy' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: DisableOutboundWebIdentityFederation — unsupported shape construct: operation 'DisableOutboundWebIdentityFederation' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: EnableOutboundWebIdentityFederation — unsupported shape construct: operation 'EnableOutboundWebIdentityFederation' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GenerateCredentialReport — unsupported shape construct: operation 'GenerateCredentialReport' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetAccountPasswordPolicy — unsupported shape construct: operation 'GetAccountPasswordPolicy' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetAccountSummary — unsupported shape construct: operation 'GetAccountSummary' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetCredentialReport — unsupported shape construct: operation 'GetCredentialReport' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetOutboundWebIdentityFederationInfo — unsupported shape construct: operation 'GetOutboundWebIdentityFederationInfo' has a Unit/absent input, which the generator does not emit yet

## Since hand-authored

`GenerateCredentialReport` is now hand-authored in
`src/ops/generate_credential_report.rs`: its empty input is declared as
`smithy.api#Unit` in `src/ops/types/generate_credential_report_types.rs`, and `build.rs`
wires the operation in from that file's `//! @op GenerateCredentialReport` header like any
other. The decline line above records only that the GENERATOR did not emit it, so the
operation is no longer modeled-but-unwired.

`GetCredentialReport` — the READ of that same feature — is hand-authored the same way, in
`src/ops/get_credential_report.rs` with its shapes in
`src/ops/types/get_credential_report_types.rs`, and is wired in from its own
`//! @op GetCredentialReport` header. Its decline line above likewise records only that the
generator did not emit it.

`GetAccountPasswordPolicy` — the READ of the account password policy
`UpdateAccountPasswordPolicy` writes — is hand-authored the same way, in
`src/ops/get_account_password_policy.rs` with its shapes in
`src/ops/types/get_account_password_policy_types.rs`, and is wired in from its own
`//! @op GetAccountPasswordPolicy` header. Its decline line above likewise records only
that the generator did not emit it.

`DeleteAccountPasswordPolicy` — the DELETE of that same account password policy — is
hand-authored the same way, in `src/ops/delete_account_password_policy.rs` with its input in
`src/ops/types/delete_account_password_policy_types.rs` and its unit output beside the
operation, and is wired in from its own `//! @op DeleteAccountPasswordPolicy` header. Its
decline line above likewise records only that the generator did not emit it.

The three OUTBOUND WEB IDENTITY FEDERATION operations —
`EnableOutboundWebIdentityFederation`, `GetOutboundWebIdentityFederationInfo` and
`DisableOutboundWebIdentityFederation` — are hand-authored the same way, each in its own
`src/ops/<operation>.rs` with its shapes in `src/ops/types/<operation>_types.rs`, and each
wired in from its own `//! @op` header. They read and write the one account-level record
`src/state/outbound_web_identity_federation.rs` owns. Their decline lines above likewise
record only that the generator did not emit them.

`GetAccountSummary` — the account's own usage and quota counters — is hand-authored the
same way, in `src/ops/get_account_summary.rs` with its shapes in
`src/ops/types/get_account_summary_types.rs`, and is wired in from its own
`//! @op GetAccountSummary` header. It aggregates `src/state/account_summary.rs`, which
holds the counters, the quotas and the account's global-endpoint token version in one
place. Its decline line above likewise records only that the generator did not emit it.
