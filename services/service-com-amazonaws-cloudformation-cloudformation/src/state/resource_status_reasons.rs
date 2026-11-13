// SPDX-License-Identifier: CC0-1.0

//! The `ResourceStatusReason` sentences CloudFormation writes on a resource's own stack
//! events — "success/failure message associated with the resource" — for the steps of a
//! deployment that are CloudFormation's own rather than an owning service's.
//!
//! One module for that one concern, because the walk that deploys a stack's resources
//! ([`super::provisioning`]) is what records them and a suite reading a stack's event stream
//! is what asserts them, so neither side spells the sentence itself. A reason that carries an
//! owning service's own refusal is not one of these: that text is the service's.
//!
//! Each is entirely FIXED text: no part of the stack, the resource or the request reaches it,
//! and the model documents no event wording at all, so these are recorded behaviour.

/// The reason CloudFormation records on the second in-progress event of a resource — the one
/// that reports the create request reached the resource's provisioner.
pub const RESOURCE_CREATION_INITIATED: &str = "Resource creation Initiated";

/// The reason CloudFormation records on the `UPDATE_IN_PROGRESS` event that OPENS the
/// creation of a resource an update REPLACES.
///
/// A deployment that asks for a value the resource that exists cannot take creates a new
/// physical resource for the same logical one, and this is the sentence that says so — which
/// is what distinguishes a replacement from a change applied to the resource that was already
/// there. What becomes of the resource it replaced is that resource's own
/// `UpdateReplacePolicy` ([`super::deletion_policy`] carries the values it takes).
pub const RESOURCE_REPLACEMENT_IN_PROGRESS: &str =
    "Requested update requires the creation of a new physical resource; hence creating one.";
