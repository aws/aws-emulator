// SPDX-License-Identifier: CC0-1.0

//! @op AssociateAccessPolicy
//! IMPLEMENTED. Writes an [`crate::state::AssociatedPolicyRecord`] onto the
//! [`crate::state::AccessEntryRecord`] `CreateAccessEntry` created, so
//! `ListAssociatedAccessPolicies` reads back exactly what this stored.
//!
//! The order the checks run in is part of the contract: an access policy is
//! associated TO a resource, so the cluster and the access entry are resolved FIRST
//! and a missing one is `ResourceNotFoundException` whatever the rest of the request
//! says. Only once both exist is the policy ARN — which must name a policy from the
//! catalogue [`crate::state::ACCESS_POLICY_NAMES`] that `ListAccessPolicies`
//! reports — and the scope validated.
//!
//! Associating a policy already associated to the entry REPLACES its scope — the
//! association is keyed by policy ARN — and keeps its original `associatedAt` while
//! moving `modifiedAt`, which is what those two members mean.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AssociateAccessPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Associates an access policy and its scope to an access entry. For more information about
//! associating access policies, see Associating and disassociating access policies to and
//! from access entries in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `accessScope` (AccessScope) **required** — The scope for the AccessPolicy. You can scope
//!   access policies to an entire cluster or to specific Kubernetes namespaces.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `policyArn` (String) **required** — The ARN of the AccessPolicy that you're associating.
//!   For a list of ARNs, use ListAccessPolicies.
//! * `principalArn` (String) **required** — The Amazon Resource Name (ARN) of the IAM user or
//!   role for the AccessEntry that you're associating the access policy to.
//!
//! ## Output
//!
//! * `associatedAccessPolicy` (AssociatedAccessPolicy) — The AccessPolicy and scope
//!   associated to the AccessEntry.
//! * `clusterName` (String) — The name of your cluster.
//! * `principalArn` (String) — The ARN of the IAM principal for the AccessEntry.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, AssociatedPolicyRecord, ACCESS_SCOPE_TYPE_CLUSTER, ACCESS_SCOPE_TYPE_NAMESPACE,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateAccessPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateAccessPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#AssociateAccessPolicy",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum AssociateAccessPolicyErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_access_policy::AssociateAccessPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateAccessPolicyError = crate::scaffolded::Scaffolded<AssociateAccessPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateAccessPolicyErrorModeled> for AssociateAccessPolicyError {
    fn from(modeled: AssociateAccessPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateAccessPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateAccessPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateAccessPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateAccessPolicy<Eng>
{
    type Input = AssociateAccessPolicyInput;
    type Output = AssociateAccessPolicyOutput;
    type Error = AssociateAccessPolicyError;

    const NAME: &'static str = "AssociateAccessPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_ACCESS_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_ACCESS_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            AssociateAccessPolicyError::from(AssociateAccessPolicyErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let invalid_parameter = |message: String| {
            AssociateAccessPolicyError::from(
                AssociateAccessPolicyErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };
        let invalid_request = |message: String| {
            AssociateAccessPolicyError::from(
                AssociateAccessPolicyErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };
        // Both resource lookups report the same modelled failure with the same
        // generic, memberless payload (see [`state::resource_not_found`]): which of
        // the two is missing is not something the caller is told apart.
        let not_found = || {
            AssociateAccessPolicyError::from(
                AssociateAccessPolicyErrorModeled::ResourceNotFoundException(
                    state::resource_not_found(),
                ),
            )
        };

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A policy is associated TO an access entry on a cluster, so BOTH resources
        // are resolved before anything in the request body is judged: a request
        // naming a resource that does not exist is answered `ResourceNotFoundException`
        // however its policy ARN or scope reads. A cluster name identifies a cluster
        // only within one region, so this is the cluster in the region the request is
        // signed for.
        let Some(cluster) = guard
            .clusters
            .get_mut(&(region, input.cluster_name.clone()))
        else {
            return Err(not_found());
        };
        // Access entries are part of the Amazon EKS-managed plane, which a cluster
        // `RegisterCluster` connected has none of: EKS grants no Kubernetes access on
        // it, so this operation does not apply to it. The gate is a fact about the
        // CLUSTER, so it precedes resolving the entry and judging the body.
        if cluster.is_connected() {
            return Err(AssociateAccessPolicyErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }
        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: one still authenticating through the
        // `aws-auth` ConfigMap alone keeps no access entries for this call to act on
        // ([`state::serves_access_entries`]). The gate is a fact about the CLUSTER
        // too, so it precedes resolving the entry and judging the body.
        if !cluster.serves_access_entries() {
            return Err(AssociateAccessPolicyErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(state::access_entry_authentication_mode_refusal()),
                    ..Default::default()
                },
            )
            .into());
        }
        let Some(entry) = cluster.access_entries.get_mut(&input.principal_arn) else {
            return Err(not_found());
        };

        // "If you set the value to `EC2_LINUX` or `EC2_WINDOWS`, you can't specify
        // values for `kubernetesGroups`, or associate an `AccessPolicy` to the access
        // entry" (`CreateAccessEntryRequest$type`). The entry exists and the request
        // is well formed — it is the state of the entry that forbids this, which is
        // what `InvalidRequestException` reports.
        if state::is_self_managed_node_entry_type(&entry.entry_type) {
            return Err(invalid_request(format!(
                "An access policy cannot be associated to an access entry of type {}.",
                entry.entry_type
            )));
        }

        // "The ARN of the AccessPolicy that you're associating. For a list of ARNs,
        // use ListAccessPolicies" — so the ARN must name one of the policies Amazon
        // EKS publishes, which is the catalogue `ListAccessPolicies` reports.
        if state::access_policy_name(&input.policy_arn).is_none() {
            return Err(invalid_parameter(format!(
                "The policyArn {} does not name an Amazon EKS access policy; use \
                 ListAccessPolicies for the available ARNs.",
                input.policy_arn
            )));
        }

        // The scope's own rules. `AccessScope$type` is the `AccessScopeType` enum
        // (`cluster | namespace`) and `AccessScope$namespaces` is "A Kubernetes
        // namespace that an access policy is scoped to. A value is required if you
        // specified `namespace` for `Type`" — so the namespaces are the namespace
        // scope's value, and a cluster-scoped association has none to name.
        let scope_type = input.access_scope.r#type.clone().unwrap_or_default();
        let namespaces = input.access_scope.namespaces.clone().unwrap_or_default();
        match scope_type.as_str() {
            ACCESS_SCOPE_TYPE_CLUSTER => {
                if !namespaces.is_empty() {
                    return Err(invalid_parameter(format!(
                        "accessScope.namespaces cannot be specified for the \
                         {ACCESS_SCOPE_TYPE_CLUSTER} access scope."
                    )));
                }
            }
            ACCESS_SCOPE_TYPE_NAMESPACE => {
                if namespaces.is_empty() {
                    return Err(invalid_parameter(format!(
                        "accessScope.namespaces is required for the \
                         {ACCESS_SCOPE_TYPE_NAMESPACE} access scope."
                    )));
                }
                if let Some(blank) = namespaces.iter().position(|name| name.trim().is_empty()) {
                    return Err(invalid_parameter(format!(
                        "accessScope.namespaces[{blank}] must be a non-empty Kubernetes \
                         namespace name."
                    )));
                }
            }
            other => {
                return Err(invalid_parameter(format!(
                    "accessScope.type must be one of [{ACCESS_SCOPE_TYPE_CLUSTER}, \
                     {ACCESS_SCOPE_TYPE_NAMESPACE}], got {other:?}."
                )));
            }
        }

        // An association is keyed by policy ARN, so re-associating a policy already
        // on this entry replaces its scope instead of adding a second record. That,
        // with only a published policy being associable at all, is what bounds how
        // many records an entry can accumulate.
        let previously_associated_at = entry
            .associated_policies
            .get(&input.policy_arn)
            .map(|existing| existing.associated_at);

        let record = AssociatedPolicyRecord {
            policy_arn: input.policy_arn.clone(),
            scope_type,
            namespaces,
            // `associatedAt` is "The date and time the AccessPolicy was associated
            // with an AccessEntry", so a re-association keeps the instant the policy
            // first landed on the entry and moves only `modifiedAt`.
            associated_at: previously_associated_at.unwrap_or(now),
            modified_at: now,
        };
        let associated_access_policy = record.to_associated_access_policy();
        entry
            .associated_policies
            .insert(input.policy_arn.clone(), record);

        Ok(AssociateAccessPolicyOutput {
            cluster_name: Some(input.cluster_name),
            principal_arn: Some(input.principal_arn),
            associated_access_policy: Some(associated_access_policy),
        })
    }
}
