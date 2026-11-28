// SPDX-License-Identifier: CC0-1.0

//! The declaration of this service's HAND-AUTHORED type modules.
//!
//! WHY THIS MODULE EXISTS. The scaffold op-stub generator declines
//! `CreateSupportPermit`, `DeleteSupportPermit`, `GetSupportPermit` and
//! `ListSupportPermits` for the one reason `src/ops/SKIPPED_STUBS.md` records:
//! the `Permit` they carry reaches two unions whose "all" member targets the
//! Smithy prelude shape `smithy.api#Unit`, which the model's JSON AST does not
//! serialize. Those four handlers are hand-authored in `src/ops/`, and so are the
//! shapes they exchange — which live, like every other shape of this service, in
//! `src/ops/types/`.
//!
//! What cannot live there is their DECLARATION. `types/mod.rs` carries
//! `@generated`: the model update rewrites it from the model and lists exactly the
//! modules the generator rendered, so a refresh drops every hand-authored module
//! from it and the crate stops building. The `mod` lines live here instead, in a
//! module the generator does not write, declared from `src/service.rs` — the one
//! file this crate's scaffold emits once and never regenerates.
//!
//! Each module keeps its file where the update left it (a file at a generated path
//! without the `@generated` marker is hand-written, and the update neither
//! overwrites nor deletes one), so the `#[path]` below points back into `types/`.
//! Every shape is declared exactly once across these modules and the rendered
//! ones, and everything is re-exported here, so one
//! `use crate::service::types_local::*;` in a handler resolves every shape of its
//! operation.

/// The shapes the four declined operations share, plus the prelude structure two
/// of them carry. Named for the RESOURCE rather than for one operation, because
/// all four reach it.
#[path = "../types/permit_shapes.rs"]
pub mod permit_shapes;

#[path = "../types/create_support_permit_types.rs"]
pub mod create_support_permit_types;

#[path = "../types/delete_support_permit_types.rs"]
pub mod delete_support_permit_types;

#[path = "../types/get_support_permit_types.rs"]
pub mod get_support_permit_types;

#[path = "../types/list_support_permits_types.rs"]
pub mod list_support_permits_types;

#[allow(unused_imports)]
pub use permit_shapes::*;
#[allow(unused_imports)]
pub use create_support_permit_types::*;
#[allow(unused_imports)]
pub use delete_support_permit_types::*;
#[allow(unused_imports)]
pub use get_support_permit_types::*;
#[allow(unused_imports)]
pub use list_support_permits_types::*;
