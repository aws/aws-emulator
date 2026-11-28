// SPDX-License-Identifier: CC0-1.0

//! WHY a read of a function's code-signing association that finds none still answers a 200,
//! and why that answer still carries both of the read's members.
//!
//! `GetFunctionCodeSigningConfig` "Returns the code signing configuration for the specified
//! function", and the two members it answers with —
//! [`crate::ops::types::GetFunctionCodeSigningConfigOutput`], which is where that member set
//! and its rendering are decided — describe ONE attachment between a configuration and the
//! function it covers. The read REPORTS that attachment whether or not the function holds
//! one, so both members are rendered on every answer and a function holding none is told
//! apart by their values being null rather than by the keys being left out.
//!
//! Nothing about that body is corrected here: the one status with no room for a body is the
//! no-content status (`service_common::no_content_responses`), and a 200 of this service
//! answers the JSON object the protocol renders from the output structure, whichever members
//! that structure's serializer writes.
//!
//! So this seam has no correction to make for such a read, and makes none: the member set is
//! the output type's, the status is the operation's, and the body is the protocol's. The rule
//! is recorded here because this is where a response-shaping correction WOULD live — beside
//! the class of a fault ([`super::fault_class`]) and the public-access flag of a policy read
//! ([`super::public_access`]) — so a later reader does not add one in the belief that none
//! was considered.
