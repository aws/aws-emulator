// SPDX-License-Identifier: CC0-1.0

//! The one shape every "that resource is not here" refusal in this crate takes.
//!
//! `ResourceNotFoundException` carries three members — `message` (`@required`),
//! `resourceId` ("The identifier of the resource that was not found") and `resourceType`
//! ("The type of the resource that was not found") — and every operation that resolves an
//! ARN fills them the same way, through [`resource_not_found`], so two readers of the same
//! missing resource cannot answer differently.

use crate::ops::types::ResourceNotFoundException;

/// `resourceType` for a resilience policy — the `Policy` shape's own name, which is what
/// the type member reports.
pub const POLICY_RESOURCE_TYPE: &str = "Policy";

/// `resourceType` for a service, as [`POLICY_RESOURCE_TYPE`] is for a policy.
pub const SERVICE_RESOURCE_TYPE: &str = "Service";

/// The refusal for a request whose `arn` names no stored resource of `resource_type`.
///
/// `resourceId` is the IDENTIFIER of the resource, which is the resource segment of the
/// ARN the request supplied (`<resource-type>/<id>`) rather than the whole ARN — the
/// account, Region and partition segments say where the resource would live, not which
/// resource it is. `message` is the resource type followed by `" not found: "` and that
/// same identifier.
pub fn resource_not_found(resource_type: &str, arn: &str) -> ResourceNotFoundException {
    let resource_id = arn_resource_id(arn);
    ResourceNotFoundException {
        message: format!("{resource_type} not found: {resource_id}"),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The resource segment of `arn`: the sixth and last field of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
///
/// The segment itself may contain further `:` separators (`Arn`'s modelled pattern admits
/// them), so everything after the fifth one belongs to it. A value carrying too few
/// fields to have a resource segment is its own identifier — the caller sent it, and no
/// part of it can be dropped without inventing one.
pub fn arn_resource_id(arn: &str) -> &str {
    arn.splitn(6, ':').nth(5).unwrap_or(arn)
}
