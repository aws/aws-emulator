// SPDX-License-Identifier: CC0-1.0

//! @op UpdateAccessEntry
//! IMPLEMENTED. Rewrites the two members of an existing
//! [`crate::state::AccessEntryRecord`] that are not fixed at creation — its
//! Kubernetes groups and its username — on the record `CreateAccessEntry` wrote
//! and `DescribeAccessEntry` reads back, and moves its `modifiedAt`.
//!
//! The entry's identity is NOT updatable: `clusterName` and `principalArn` are
//! `@httpLabel`s that ADDRESS the entry, the principal "can't be changed after
//! access entry creation" (`CreateAccessEntryRequest$principalArn`), and `type`
//! is not an input here at all — so this call can only change what the request
//! body carries.
//!
//! A body member the request OMITS names no new value, so it leaves the stored
//! one alone; a member it carries replaces it (an empty `kubernetesGroups` list
//! is a value, so it clears the groups). A blank `username` names nobody, which
//! is the create path's blankness rule too: Amazon EKS then fills in the username
//! it would have chosen for the entry's type
//! ([`crate::state::default_username`]).
//!
//! `clientRequestToken` is `@idempotencyToken`, and this call is already
//! idempotent — applying the same body twice leaves the same two members stored —
//! so there is nothing for a replayed token to absorb and the record keeps the
//! token of the create that made it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateAccessEntry`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates an access entry.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `kubernetesGroups` (StringList) — The value for name that you've specified for kind:
//!   Group as a subject in a Kubernetes RoleBinding or ClusterRoleBinding object. Amazon EKS
//!   doesn't confirm that the value for name exists in any bindings on your cluster. You can
//!   specify one or more names. Kubernetes authorizes the principalArn of the access entry to
//!   access any cluster objects that you've specified in a Kubernetes Role or ClusterRole
//!   object that is also specified in a binding's roleRef. For more information about
//!   creating Kubernetes RoleBinding, ClusterRoleBinding, Role, or ClusterRole objects, see
//!   Using RBAC Authorization in the Kubernetes documentation. If you want Amazon EKS to
//!   authorize the principalArn (instead of, or in …
//! * `principalArn` (String) **required** — The ARN of the IAM principal for the AccessEntry.
//! * `username` (String) — The username to authenticate to Kubernetes with. We recommend not
//!   specifying a username and letting Amazon EKS specify it for you. For more information
//!   about the value Amazon EKS specifies for you, or constraints before specifying your own
//!   username, see Creating access entries in the Amazon EKS User Guide.
//!
//! ## Output
//!
//! * `accessEntry` (AccessEntry) — The ARN of the IAM principal for the AccessEntry.
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
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateAccessEntryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateAccessEntryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#UpdateAccessEntry",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum UpdateAccessEntryErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_access_entry::UpdateAccessEntryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateAccessEntryError = crate::scaffolded::Scaffolded<UpdateAccessEntryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateAccessEntryErrorModeled> for UpdateAccessEntryError {
    fn from(modeled: UpdateAccessEntryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateAccessEntry` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateAccessEntry<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateAccessEntry<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateAccessEntry<Eng>
{
    type Input = UpdateAccessEntryInput;
    type Output = UpdateAccessEntryOutput;
    type Error = UpdateAccessEntryError;

    const NAME: &'static str = "UpdateAccessEntry";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ACCESS_ENTRY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ACCESS_ENTRY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateAccessEntryError::from(UpdateAccessEntryErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This update addresses ONE resource — the `(cluster, access entry)` pair —
        // so the miss is reported without naming either half and without any of the
        // exception's identity members: see [`crate::state::resource_not_found`], the
        // same payload `DescribeAccessEntry` and `DeleteAccessEntry` answer for the
        // same pair.
        let not_found = || {
            UpdateAccessEntryError::from(UpdateAccessEntryErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            ))
        };

        let region = crate::state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // An access entry is addressed THROUGH its cluster, and a cluster name
        // identifies a cluster only within the region the request is signed for.
        let Some(cluster) = guard
            .clusters
            .get_mut(&(region, input.cluster_name.clone()))
        else {
            return Err(not_found());
        };

        // Access entries are part of the Amazon EKS-managed plane, which a cluster
        // `RegisterCluster` connected has none of: EKS grants no Kubernetes access on
        // it, so this operation does not apply to it. The gate is a fact about the
        // CLUSTER, so it precedes resolving the entry.
        if cluster.is_connected() {
            return Err(UpdateAccessEntryErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }
        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: one still authenticating through the
        // `aws-auth` ConfigMap alone keeps no access entries for this call to change
        // ([`crate::state::serves_access_entries`]). The gate is a fact about the
        // CLUSTER too, so it precedes resolving the entry.
        if !cluster.serves_access_entries() {
            return Err(UpdateAccessEntryErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(crate::state::access_entry_authentication_mode_refusal()),
                    ..Default::default()
                },
            )
            .into());
        }

        let Some(entry) = cluster.access_entries.get_mut(&input.principal_arn) else {
            return Err(not_found());
        };

        // "If you set the value to `EC2_LINUX` or `EC2_WINDOWS`, you can't specify
        // values for `kubernetesGroups`" (`CreateAccessEntryRequest$type`): EKS manages
        // a self-managed Amazon EC2 node's groups itself. That is a rule about the
        // ENTRY's type, so it holds whichever call tries to set them — and the type is
        // not an input here, so it is the stored one that decides.
        let groups_requested = input
            .kubernetes_groups
            .as_ref()
            .is_some_and(|groups| !groups.is_empty());
        if groups_requested && crate::state::is_self_managed_node_entry_type(&entry.entry_type) {
            return Err(UpdateAccessEntryError::from(
                UpdateAccessEntryErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "kubernetesGroups cannot be specified for an access entry of \
                             type {}.",
                            entry.entry_type
                        )),
                        ..Default::default()
                    },
                ),
            ));
        }

        // And the username is Amazon EKS's to specify for every type but `STANDARD`
        // ([`crate::state::username_refusal`]) — a rule about the ENTRY's type, so it
        // holds whichever call tries to set one, and the type is not an input here,
        // so it is the stored one that decides. A blank username names nobody, which
        // is the create path's blankness rule, and asks EKS to specify it instead.
        if input
            .username
            .as_ref()
            .is_some_and(|username| !username.trim().is_empty())
        {
            if let Some(why) = crate::state::username_refusal(&entry.entry_type) {
                return Err(UpdateAccessEntryError::from(
                    UpdateAccessEntryErrorModeled::InvalidParameterException(
                        InvalidParameterException {
                            cluster_name: Some(input.cluster_name.clone()),
                            message: Some(why),
                            ..Default::default()
                        },
                    ),
                ));
            }
        }

        // A member the request carries replaces the stored one; a member it omits
        // names no new value, so the stored one stands.
        if let Some(groups) = input.kubernetes_groups.clone() {
            // An EMPTY list asks for no group at all, which on an entry whose groups
            // Amazon EKS derives for its type would drop the node-authorization grant
            // that type exists to confer: those groups are the server's, so they
            // stand ([`crate::state::server_managed_kubernetes_groups`]).
            let derived = crate::state::server_managed_kubernetes_groups(&entry.entry_type);
            if !groups.is_empty() || derived.is_empty() {
                entry.kubernetes_groups = groups;
            }
        }
        if let Some(username) = input.username.clone() {
            entry.username = if username.trim().is_empty() {
                // Blank names nobody, so Amazon EKS specifies the username for this
                // entry's type — the same blankness rule the create path applies.
                crate::state::default_username(&entry.entry_type, &entry.principal_arn)
            } else {
                username
            };
        }
        entry.modified_at = now;

        Ok(UpdateAccessEntryOutput {
            access_entry: Some(entry.to_access_entry()),
        })
    }
}
