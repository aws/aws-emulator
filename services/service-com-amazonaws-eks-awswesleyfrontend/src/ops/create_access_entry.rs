// SPDX-License-Identifier: CC0-1.0

//! @op CreateAccessEntry
//! IMPLEMENTED. Writes a [`crate::state::AccessEntryRecord`] onto the cluster's
//! [`crate::state::ClusterRecord`], which is what `DescribeAccessEntry` reads back
//! and what `AssociateAccessPolicy` hangs an association off.
//!
//! The entry is keyed by its IAM principal ARN: "You can specify one ARN for each
//! access entry. You can't specify the same ARN in more than one access entry"
//! (`CreateAccessEntryRequest$principalArn`), so a second create for the same
//! principal is `ResourceInUseException` — unless it carries the
//! `@idempotencyToken` of the create that already succeeded, which is a retry.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAccessEntry`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an access entry. An access entry allows an IAM principal to access your cluster.
//! Access entries can replace the need to maintain entries in the aws-auth ConfigMap for
//! authentication. You have the following options for authorizing an IAM principal to
//! access Kubernetes objects on your cluster: Kubernetes role-based access control (RBAC),
//! Amazon EKS, or both. Kubernetes RBAC authorization requires you to create and manage
//! Kubernetes Role, ClusterRole, RoleBinding, and ClusterRoleBinding objects, in addition
//! to managing access entries. If you use Amazon EKS authorization exclusively, you don't
//! need to create and manage Kubernetes Role, ClusterRole, RoleBinding, and
//! ClusterRoleBinding objects. For more information about access entries, see Access
//! entries in the Amazon EKS User Guide.
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
//!   You can specify one ARN for each access entry. You can't specify the same ARN in more
//!   than one access entry. This value can't be changed after access entry creation. The
//!   valid principals differ depending on the type of the access entry in the type field. For
//!   STANDARD access entries, you can use every IAM principal type. For nodes (EC2 (for EKS
//!   Auto Mode), EC2_LINUX, EC2_WINDOWS, FARGATE_LINUX, and HYBRID_LINUX), the only valid ARN
//!   is IAM roles. You can't use the STS session principal type with access entries because
//!   this is a temporary principal for each session and not a permanent identity that can be
//!   assigned permissions. IAM best …
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//! * `type` (String) — The type of the new access entry. Valid values are STANDARD,
//!   FARGATE_LINUX, EC2_LINUX, EC2_WINDOWS, EC2 (for EKS Auto Mode), HYBRID_LINUX, and
//!   HYPERPOD_LINUX. If the principalArn is for an IAM role that's used for self-managed
//!   Amazon EC2 nodes, specify EC2_LINUX or EC2_WINDOWS. Amazon EKS grants the necessary
//!   permissions to the node for you. If the principalArn is for any other purpose, specify
//!   STANDARD. If you don't specify a value, Amazon EKS sets the value to STANDARD. If you
//!   have the access mode of the cluster set to API_AND_CONFIG_MAP, it's unnecessary to
//!   create access entries for IAM roles used with Fargate profiles or managed Amazon EC2
//!   nodes, because Amazon EKS creates entries in …
//! * `username` (String) — The username to authenticate to Kubernetes with. We recommend not
//!   specifying a username and letting Amazon EKS specify it for you. For more information
//!   about the value Amazon EKS specifies for you, or constraints before specifying your own
//!   username, see Creating access entries in the Amazon EKS User Guide.
//!
//! ## Output
//!
//! * `accessEntry` (AccessEntry) — An access entry allows an IAM principal (user or role) to
//!   access your cluster. Access entries can replace the need to maintain the aws-auth
//!   ConfigMap for authentication. For more information about access entries, see Access
//!   entries in the Amazon EKS User Guide.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, AccessEntryRecord, ACCESS_ENTRY_TYPES, ACCESS_ENTRY_TYPE_STANDARD,
    MAX_ACCESS_ENTRIES_PER_CLUSTER,
};
use std::collections::BTreeMap;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAccessEntryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAccessEntryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#CreateAccessEntry",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum CreateAccessEntryErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_access_entry::CreateAccessEntryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAccessEntryError = crate::scaffolded::Scaffolded<CreateAccessEntryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAccessEntryErrorModeled> for CreateAccessEntryError {
    fn from(modeled: CreateAccessEntryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAccessEntry` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAccessEntry<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAccessEntry<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAccessEntry<Eng>
{
    type Input = CreateAccessEntryInput;
    type Output = CreateAccessEntryOutput;
    type Error = CreateAccessEntryError;

    const NAME: &'static str = "CreateAccessEntry";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ACCESS_ENTRY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ACCESS_ENTRY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateAccessEntryError::from(CreateAccessEntryErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let invalid_parameter = |message: String| {
            CreateAccessEntryError::from(
                CreateAccessEntryErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // An access entry is created ON a cluster, so the cluster is resolved first:
        // a name identifies a cluster only within the region the request is signed
        // for.
        let Some(cluster) = guard
            .clusters
            .get_mut(&(region.clone(), input.cluster_name.clone()))
        else {
            // The cluster is the resource this reports missing, so the payload names
            // it: `CreateAccessEntry` only validates the cluster before writing into
            // it, and the recorded corpus shows EKS naming the cluster on this path
            // ([`state::cluster_not_found`]).
            return Err(CreateAccessEntryErrorModeled::ResourceNotFoundException(
                state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };

        // An access entry grants a principal access to the cluster's Kubernetes API
        // through Amazon EKS, which a cluster `RegisterCluster` connected does not go
        // through: EKS manages no access on it, so this operation does not apply to it.
        // The gate is a fact about the CLUSTER, so it is answered as soon as the cluster
        // is resolved, before anything in the body is judged against it.
        if cluster.is_connected() {
            return Err(CreateAccessEntryErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: a cluster still authenticating through
        // the `aws-auth` ConfigMap alone has no access-entry plane for this call to
        // write into ([`state::serves_access_entries`]). The gate is a fact about the
        // CLUSTER too, so it is answered before anything in the body is judged
        // against it.
        if !cluster.serves_access_entries() {
            return Err(CreateAccessEntryErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(state::access_entry_authentication_mode_refusal()),
                    ..Default::default()
                },
            )
            .into());
        }

        // "If you don't specify a value, Amazon EKS sets the value to `STANDARD`", and the
        // set of values the member documents is the set this accepts — `type` is a
        // plain string member, so nothing refused it before this handler ran.
        let entry_type = input
            .r#type
            .clone()
            .unwrap_or_else(|| ACCESS_ENTRY_TYPE_STANDARD.to_string());
        if !ACCESS_ENTRY_TYPES.contains(&entry_type.as_str()) {
            return Err(invalid_parameter(format!(
                "type must be one of {ACCESS_ENTRY_TYPES:?}, got {entry_type:?}."
            )));
        }

        // "The ARN of the IAM principal for the AccessEntry" — an IAM user or role
        // ARN. "You can't use the STS session principal type with access entries
        // because this is a temporary principal for each session", and for the node
        // types "the only valid ARN is IAM roles"
        // (`CreateAccessEntryRequest$principalArn`).
        let Some(principal) = state::parse_iam_principal_arn(&input.principal_arn) else {
            return Err(invalid_parameter(format!(
                "principalArn {} is not the ARN of an IAM user or role \
                 (arn:<partition>:iam::<account>:role/<name>).",
                input.principal_arn
            )));
        };
        let node_entry_type = state::is_node_access_entry_type(&entry_type);
        if node_entry_type && principal.kind != "role" {
            return Err(invalid_parameter(format!(
                "principalArn must be the ARN of an IAM role for an access entry of \
                 type {entry_type}, got {}.",
                input.principal_arn
            )));
        }

        // "If you set the value to `EC2_LINUX` or `EC2_WINDOWS`, you can't specify
        // values for `kubernetesGroups`" (`CreateAccessEntryRequest$type`): EKS
        // manages the node's groups itself.
        let kubernetes_groups = input.kubernetes_groups.clone().unwrap_or_default();
        if state::is_self_managed_node_entry_type(&entry_type) && !kubernetes_groups.is_empty() {
            return Err(invalid_parameter(format!(
                "kubernetesGroups cannot be specified for an access entry of type \
                 {entry_type}."
            )));
        }

        // What the entry REPORTS as its groups: the caller's own for a `STANDARD`
        // entry, and for an entry whose type Amazon EKS admits a node through, the
        // groups EKS derives for that type — the node-authorization grant the type
        // exists to confer, which the request does not carry
        // ([`state::server_managed_kubernetes_groups`]).
        let kubernetes_groups = if kubernetes_groups.is_empty() {
            state::server_managed_kubernetes_groups(&entry_type)
                .iter()
                .map(|group| (*group).to_string())
                .collect()
        } else {
            kubernetes_groups
        };

        // And the username is Amazon EKS's to specify for every type but `STANDARD`
        // ([`state::username_refusal`]): those entries stand for a node or a pod EKS
        // admits itself, authenticating as the username it templates for that type,
        // so a request that names one for them is refused and no entry is written. A
        // blank username names nobody, which this slice treats as absent.
        let requested_username = input
            .username
            .clone()
            .filter(|username| !username.trim().is_empty());
        if requested_username.is_some() {
            if let Some(why) = state::username_refusal(&entry_type) {
                return Err(invalid_parameter(why));
            }
        }

        // "You can specify one ARN for each access entry. You can't specify the same
        // ARN in more than one access entry." `clientRequestToken` is
        // `@idempotencyToken`, which an SDK resends when it RETRIES the same call, so
        // a retry of the create that wrote this entry is absorbed and answered with
        // it rather than with a conflict. A blank token identifies nothing, so it
        // never matches.
        if let Some(existing) = cluster.access_entries.get(&input.principal_arn) {
            let replayed_token = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if let Some(token) = replayed_token {
                if existing.client_request_token.as_deref() == Some(token) {
                    return Ok(CreateAccessEntryOutput {
                        access_entry: Some(existing.to_access_entry()),
                    });
                }
            }
            return Err(CreateAccessEntryErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(format!(
                        "The access entry for principal ARN {} already exists for \
                         cluster {}.",
                        input.principal_arn, input.cluster_name
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // What BOUNDS the store: a cluster holds only so many access entries, and
        // exceeding a service limit is what `ResourceLimitExceededException` reports.
        if cluster.access_entries.len() >= MAX_ACCESS_ENTRIES_PER_CLUSTER {
            return Err(
                CreateAccessEntryErrorModeled::ResourceLimitExceededException(
                    ResourceLimitExceededException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "You have reached the limit of \
                             {MAX_ACCESS_ENTRIES_PER_CLUSTER} access entries for \
                             cluster {}",
                            input.cluster_name
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let record = AccessEntryRecord {
            cluster_name: input.cluster_name.clone(),
            principal_arn: input.principal_arn.clone(),
            arn: state::access_entry_arn(
                &region,
                &input.cluster_name,
                &principal,
                &input.principal_arn,
            ),
            created_at: now,
            // Nothing has modified the entry yet, so it was last modified when it was
            // created.
            modified_at: now,
            kubernetes_groups,
            // "We recommend not specifying a username and letting Amazon EKS specify it
            // for you" — see [`crate::state::default_username`] for the value it picks.
            // A blank username names nobody, so it is treated as absent, the same
            // blankness test the rest of this slice uses.
            username: requested_username
                .unwrap_or_else(|| state::default_username(&entry_type, &input.principal_arn)),
            entry_type,
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            client_request_token: input.client_request_token.clone(),
            // A new entry has no associated access policies: each one is written by
            // an `AssociateAccessPolicy` call.
            associated_policies: BTreeMap::new(),
        };

        let access_entry = record.to_access_entry();
        cluster
            .access_entries
            .insert(input.principal_arn.clone(), record);

        Ok(CreateAccessEntryOutput {
            access_entry: Some(access_entry),
        })
    }
}
