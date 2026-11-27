// SPDX-License-Identifier: CC0-1.0

//! WHICH resource the IAM decision about a request to this service is taken on.
//!
//! IAM decides an action against the RESOURCE the request acts on
//! ([`crate::service::authorization`]), and a request to this service names that
//! resource the way the operation's own input does: a platform version by its
//! name and version, an application by its name, an environment by either of the
//! two identifiers an environment is addressed by. The ARN of the named resource
//! is what an identity-based policy's `Resource` element is matched against, so
//! an operation that addresses the same kind of resource as another must build
//! the same ARN for it — which is why the resolutions live here together rather
//! than one per handler.
//!
//! An ENVIRONMENT is the one resource a request can name without naming the
//! application it belongs to (`EnvironmentId` / `EnvironmentName`), and an
//! environment's ARN carries both, so the record this store holds for that
//! identifier is what the ARN is read off — the same ARN every read of the
//! environment reports as its `EnvironmentArn`. For an identifier this account
//! holds no environment for there is no such ARN, and the decision is taken on
//! the account's environments in the Region instead: the request addressed none
//! of them in particular, and a refusal must not depend on whether a resource
//! this caller may not even see exists.

use super::*;

/// The ARN segment that stands for EVERY resource of its kind, which is what a
/// request names when it names no resource this account holds.
const EVERY: &str = "*";

/// The resource an environment-addressed request acts on: the ARN of the
/// environment `environment_id` or `environment_name` names, or every environment
/// of the account in `region` when neither names one this store holds (see the
/// module docs).
///
/// Takes and releases this service's store itself, so a handler calls it BEFORE
/// locking the store for its own work.
pub fn environment_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    environment_id: Option<&str>,
    environment_name: Option<&str>,
) -> String {
    let held = service_state(engine).and_then(|state| {
        let store = state.lock();
        store
            .resolve_environment_id(environment_id, environment_name)
            .and_then(|id| store.environments.get(&id).map(|held| held.arn.clone()))
    });
    held.unwrap_or_else(|| environment_arn(region, EVERY, EVERY))
}

/// The resource a request that acts on an application as a whole — its
/// versions, the group of environments composed out of them — names: the ARN of
/// the application the request named, or every application of the account in
/// `region` when it named none.
pub fn application_resource(region: &str, application_name: Option<&str>) -> String {
    application_arn(region, application_name.unwrap_or(EVERY))
}

/// The resource a LAUNCH acts on: the environment it is asked to create, which
/// the request names by the application it belongs to and the environment name
/// it is to carry. A launch that leaves the name to the environment manifest in
/// a version's source bundle names no environment yet, so the decision is taken
/// on the application's environments.
pub fn launched_environment_resource(
    region: &str,
    application_name: &str,
    environment_name: Option<&str>,
) -> String {
    environment_arn(region, application_name, environment_name.unwrap_or(EVERY))
}
