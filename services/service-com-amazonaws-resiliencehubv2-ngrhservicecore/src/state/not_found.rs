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

/// `resourceType` for a service function, as [`POLICY_RESOURCE_TYPE`] is for a policy.
///
/// A function is addressed by its `serviceFunctionId` rather than by an ARN of its own, so
/// a refusal naming one reports that identifier as its `resourceId` — see
/// [`service_function_not_found`].
pub const SERVICE_FUNCTION_RESOURCE_TYPE: &str = "ServiceFunction";

/// How a refusal's MESSAGE names a service function: the resource as the service words it
/// in prose, which is not always its shape name — see [`service_function_not_found`].
const SERVICE_FUNCTION_LABEL: &str = "Service function";

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

/// The refusal for a `serviceFunctionId` that names no stored service function.
///
/// A function is identified by that id within its service rather than by an ARN, so the
/// id is both the `resourceId` and what the message names. The message names the resource
/// the way the service words it in prose ([`SERVICE_FUNCTION_LABEL`]), while
/// `resourceType` carries the model's own shape name for it.
pub fn service_function_not_found(service_function_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{SERVICE_FUNCTION_LABEL} not found: {service_function_id}"),
        resource_id: Some(service_function_id.to_string()),
        resource_type: Some(SERVICE_FUNCTION_RESOURCE_TYPE.to_string()),
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
