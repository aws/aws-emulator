// SPDX-License-Identifier: CC0-1.0

//! The resource of a scalable target this service reaches OVER THE WIRE: which namespaces
//! those are, what their resource holds, and the precondition a registration of one is
//! subject to.
//!
//! Every namespace's resource is resolved before a target on it is persisted — "a scalable
//! target is a capacity dimension of the underlying service" — and for most of them that is a
//! read of the owning service's own store ([`super::resolve_target_resource`]). Three
//! namespaces cannot be read that way, so they are resolved by CALLING the service that owns
//! the resource, which is what this module does in one place:
//!
//! * `custom-resource`, whose target IS an endpoint of the caller's own application
//!   ([`super::custom_resource_endpoint`]) — reaching it is the whole of what scaling such a
//!   target does, so an endpoint that will not serve the invocation refuses the registration;
//! * `cassandra`, whose table lives in Amazon Keyspaces ([`super::keyspaces_tables`]); and
//! * `lambda`, whose version or alias lives in Lambda ([`super::lambda_functions`]).
//!
//! What each of them answers is the same two things the store-reading resolvers answer: the
//! capacity the resource holds right now, which is what the registered window is enforced on
//! ("Application Auto Scaling retrieves the current capacity of the resource. If it's below
//! the minimum capacity or above the maximum capacity, Application Auto Scaling adjusts the
//! capacity"), and a refusal for a resource that cannot carry the target at all.
//!
//! A resource this emulator cannot reach resolves NOTHING — neither a capacity nor a refusal —
//! exactly as a namespace whose owning store is absent does.
//!
//! This module owns the ONE list of which namespaces those are, because two readers need it:
//! a registration, which resolves the resource here, and the scaling clock, which re-reads
//! that capacity and delivers the movements owed to it ([`super::policy_execution`]).

use super::TargetKey;

/// The namespace whose resources are the caller's own endpoints
/// ([`super::custom_resource_endpoint`]).
pub const CUSTOM_RESOURCE_NAMESPACE: &str = "custom-resource";

/// The namespace whose resources are Amazon Keyspaces tables, which are reached by CALLING
/// that service ([`super::keyspaces_tables`]) rather than by reading its store.
pub const CASSANDRA_NAMESPACE: &str = "cassandra";

/// The namespace whose resources are qualified Lambda functions, reached by CALLING that
/// service ([`super::lambda_functions`]).
pub const LAMBDA_NAMESPACE: &str = "lambda";

/// The scaling property that names the READ half of a provisioned-throughput capacity
/// specification; its sibling names the write half.
pub const READ_CAPACITY_PROPERTY: &str = "ReadCapacityUnits";

/// Whether a namespace's resource CARRIES a capacity this service reaches over the wire,
/// which is what decides where a movement of one is delivered
/// ([`super::policy_execution`]) and which targets the clock re-reads.
///
/// `lambda` is NOT one of them: the provisioned concurrency a `lambda` target scales is
/// allocated by a configuration of its own on the qualified function, which a function that
/// carries none simply does not have, so there is no capacity of the resource to read or to
/// move — only the qualifier to resolve ([`resolve_remote_resource`]).
pub fn is_remote_capacity_namespace(service_namespace: &str) -> bool {
    matches!(
        service_namespace,
        CUSTOM_RESOURCE_NAMESPACE | CASSANDRA_NAMESPACE
    )
}

/// Resolve the resource `target` names, in the service that owns it, for the namespaces whose
/// resource is reached over the wire.
///
/// * `Ok(Some(capacity))` — the resource resolved, and this is its capacity for the property
///   the dimension names;
/// * `Ok(None)` — there is nothing this emulator can ask, so the registration is answered on
///   its own terms;
/// * `Err(message)` — the resource cannot carry this target, as a `ValidationException`
///   message.
///
/// Called with this service's store NOT held: every arm of it is a call.
pub async fn resolve_remote_resource(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    target: &TargetKey,
) -> Result<Option<i32>, String> {
    match target.service_namespace.as_str() {
        CUSTOM_RESOURCE_NAMESPACE => {
            use super::custom_resource_endpoint::Reached;

            match super::custom_resource_endpoint::probe_capacity(
                callback,
                region,
                &target.resource_id,
            )
            .await
            {
                Reached::Served(capacity) => Ok(capacity),
                Reached::Refused => {
                    Err(super::custom_resource_endpoint::missing_invoke_permissions_message())
                }
                Reached::Unserved => Ok(None),
            }
        }
        // The table has to exist, in the capacity mode that provisions throughput at all, and
        // the capacity it provisions for the half the dimension names is what that same read
        // answers.
        CASSANDRA_NAMESPACE => {
            super::keyspaces_tables::resolve_cassandra_table(
                callback,
                region,
                &target.resource_id,
                &target.scalable_dimension,
            )
            .await
        }
        LAMBDA_NAMESPACE => {
            super::lambda_functions::resolve_lambda_qualifier(
                callback,
                region,
                &target.resource_id,
                &target.scalable_dimension,
            )
            .await?;
            // See [`is_remote_capacity_namespace`]: such a target has no capacity of the
            // resource for the window to be enforced on.
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// The capacity the resource of a target in a remote-capacity namespace holds right now, or
/// `None` when this emulator could not read it.
///
/// The same read [`resolve_remote_resource`] makes, for the caller that only re-reads a
/// capacity it already resolved once (`super::policy_execution::refresh_remote_capacities`)
/// and so has no registration to refuse: a resource that has stopped resolving leaves the
/// capacity this service tracks where it was.
pub async fn read_remote_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    target: &TargetKey,
) -> Option<i32> {
    match target.service_namespace.as_str() {
        CUSTOM_RESOURCE_NAMESPACE => {
            super::custom_resource_endpoint::read_capacity(callback, region, &target.resource_id)
                .await
        }
        CASSANDRA_NAMESPACE => {
            super::keyspaces_tables::read_table_capacity(
                callback,
                region,
                &target.resource_id,
                reads_the_capacity(&target.scalable_dimension),
            )
            .await
        }
        _ => None,
    }
}

/// Whether the dimension names the READ half of a two-part capacity specification.
pub fn reads_the_capacity(scalable_dimension: &str) -> bool {
    super::dimension_scaling_property(scalable_dimension) == READ_CAPACITY_PROPERTY
}
