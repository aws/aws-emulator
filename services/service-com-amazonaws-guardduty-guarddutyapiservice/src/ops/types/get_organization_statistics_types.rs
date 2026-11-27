// SPDX-License-Identifier: CC0-1.0

//! Nothing: the `GetOrganizationStatistics` shapes live beside their handler.
//!
//! This path is one `types/mod.rs` renders a declaration for only when the scaffold
//! op-stub generator emitted a stub for the operation. It emits none for
//! `GetOrganizationStatistics` (its input is `smithy.api#Unit`, recorded in
//! `src/ops/SKIPPED_STUBS.md`), so a hand-written module here is left undeclared — and
//! therefore unreachable — by every model update that re-renders that `mod.rs`.
//!
//! The operation's input, output and the statistics tree under it are declared in
//! `src/ops/get_organization_statistics.rs`, the hand-authored handler that is the only
//! reader of them and the one file no generator rewrites.
