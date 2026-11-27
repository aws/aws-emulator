// SPDX-License-Identifier: CC0-1.0

//! Where a member the WIRE carries that the on-disk model does not declare is declared.
//!
//! `#[smithy_macros::smithy(shape = ...)]` derives a structure's `Schema`, its serializer
//! and its deserializer from the member list of the shape it names, and it admits EXACTLY
//! that list: a Rust field with no member behind it is a compile error, so a member the
//! service really reports but the published snapshot omits cannot be declared against that
//! snapshot at all.
//!
//! It does not have to be. The derive reads whichever model file the module it sits in
//! loaded, so such a member is declared by loading a small OVERLAY model beside the
//! snapshot — `smithy-model/local-additions.json`, which restates the shape's modelled
//! members with their own targets and traits and adds the ones AWS answers with — and
//! deriving the structure from it in a hand-written module beside the generated ones
//! (`service_report_types`, `task_report_types`, `task_definition_report_types`,
//! `managed_scaling_types`). Nothing about the wire is then hand-written: the member names,
//! the shape ids, the targets and the constraint traits all come from a model file, exactly
//! as they do for every modelled shape, and the next refresh of the snapshot overwrites
//! neither the overlay nor those modules.
//!
//! `smithy-model/LOCAL-ADDITIONS.md` states which member each shape adds and why, and
//! `tests/all/local_model_additions.rs` pins every restated declaration against the
//! snapshot, so a refresh that declares one of those members fails there rather than
//! silently leaving two declarations of it.
//!
//! This module therefore declares no item: it is the note that says where to look.
