// SPDX-License-Identifier: CC0-1.0

//! @op UpdatePodIdentityAssociation
//! IMPLEMENTED. Mutates the [`crate::state::PodIdentityAssociationRecord`]
//! `CreatePodIdentityAssociation` wrote, in place: the members the model lets an
//! update change (`roleArn`, `targetRoleArn`, `disableSessionTags`, `policy`) are
//! overwritten when the request carries them, `modifiedAt` moves to now, and
//! everything else — the ids, the ARN, `createdAt`, the namespace and service
//! account the association can never be moved between — is left as stored.
//!
//! The `roleArn` an update asks Amazon EKS to pass is authorized FIRST — before the
//! body's forms and before the association is resolved — and then, once the forms
//! are sound, that role's trust relationship is resolved against IAM: an update may
//! only point the association at a role the EKS Pod Identity principal may assume
//! ([`crate::state::pod_identity_role_trust_refusal`]), the same rule the create
//! applies to the role it stores.
//!
//! The identifiers the request carries are checked for their documented FORM
//! before any resource is resolved, so a value that names nothing earns an
//! `InvalidParameterException` (400) rather than a not-found — a role ARN outside
//! its form in this operation's own wording
//! ([`crate::state::role_arn_refusal`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdatePodIdentityAssociation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates a EKS Pod Identity association. In an update, you can change the IAM role, the
//! target IAM role, or disableSessionTags. You must change at least one of these in an
//! update. An association can't be moved between clusters, namespaces, or service accounts.
//! If you need to edit the namespace or service account, you need to delete the association
//! and then create a new association with your desired settings. Similar to Amazon Web
//! Services IAM behavior, EKS Pod Identity associations are eventually consistent, and may
//! take several seconds to be effective after the initial API call returns successfully.
//! You must design your applications to account for these potential delays. We recommend
//! that you don’t include association create/updates in the critical, high-availability
//! code paths of your application. Instead, make changes in a separate initialization or
//! setup routine that you run less frequently. You can set a target IAM role in the same or
//! a different account for advanced scenarios. With a target role, EKS Pod Identity
//! automatically performs two role assumptions in sequence: first assuming the role in the
//! association that is in this account, then using those credentials to assume the target
//! IAM role. This process provides your Pod with temporary credentials that have the
//! permissions defined in the target role, allowing secure access to resources in another
//! Amazon Web Services account.
//!
//! ## Input
//!
//! * `associationId` (String) **required** — The ID of the association to be updated.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of the cluster that you want to update
//!   the association in.
//! * `disableSessionTags` (BoxedBoolean) — Disable the automatic sessions tags that are
//!   appended by EKS Pod Identity. EKS Pod Identity adds a pre-defined set of session tags
//!   when it assumes the role. You can use these tags to author a single role that can work
//!   across resources by allowing access to Amazon Web Services resources based on matching
//!   tags. By default, EKS Pod Identity attaches six tags, including tags for cluster name,
//!   namespace, and service account name. For the list of tags added by EKS Pod Identity, see
//!   List of session tags added by EKS Pod Identity in the Amazon EKS User Guide. Amazon Web
//!   Services compresses inline session policies, managed policy ARNs, and session tags into
//!   a packed binary format that has a separate …
//! * `policy` (String) — An optional IAM policy in JSON format (as an escaped string) that
//!   applies additional restrictions to this pod identity association beyond the IAM policies
//!   attached to the IAM role. This policy is applied as the intersection of the role's
//!   policies and this policy, allowing you to reduce the permissions that applications in
//!   the pods can use. Use this policy to enforce least privilege access while still
//!   leveraging a shared IAM role across multiple applications. Important considerations
//!   Session tags: When using this policy, disableSessionTags must be set to true. Target
//!   role permissions: If you specify both a TargetRoleArn and a policy, the policy
//!   restrictions apply only to the target role's …
//! * `roleArn` (String) — The new IAM role to change in the association.
//! * `targetRoleArn` (String) — The Amazon Resource Name (ARN) of the target IAM role to
//!   associate with the service account. This role is assumed by using the EKS Pod Identity
//!   association role, then the credentials for this role are injected into the Pod. When you
//!   run applications on Amazon EKS, your application might need to access Amazon Web
//!   Services resources from a different role that exists in the same or different Amazon Web
//!   Services account. For example, your application running in “Account A” might need to
//!   access resources, such as buckets in “Account B” or within “Account A” itself. You can
//!   create a association to access Amazon Web Services resources in “Account B” by creating
//!   two IAM roles: a role in “Account A” …
//!
//! ## Output
//!
//! * `association` (PodIdentityAssociation) — The full description of the association that
//!   was updated.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — widened in through `extra_errors` (see below):
//!   the class the pass-role authorization of `roleArn` answers with.
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
use crate::state;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdatePodIdentityAssociationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdatePodIdentityAssociationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// Two classes are widened in through `extra_errors`: this SERVICE models both shapes
// and many of its operations declare them, but this one lists neither.
// `ClientException` is the class every operation of the Amazon EKS-managed plane
// refuses a cluster this service does not manage with
// ([`crate::state::connected_cluster_refusal`]), and `AccessDeniedException` — 403 —
// is the class the pass-role authorization of `roleArn` answers with, the same one
// `CreatePodIdentityAssociation` widens in for the same question
// ([`crate::state::pass_role_refusal`]): an update hands Amazon EKS a role exactly as
// a create does.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#UpdatePodIdentityAssociation",
    extra_errors = ["ClientException", "AccessDeniedException"]
)]
#[derive(Debug)]
pub enum UpdatePodIdentityAssociationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_pod_identity_association::UpdatePodIdentityAssociationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdatePodIdentityAssociationError = crate::scaffolded::Scaffolded<UpdatePodIdentityAssociationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdatePodIdentityAssociationErrorModeled> for UpdatePodIdentityAssociationError {
    fn from(modeled: UpdatePodIdentityAssociationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdatePodIdentityAssociation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdatePodIdentityAssociation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdatePodIdentityAssociation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdatePodIdentityAssociation<Eng>
{
    type Input = UpdatePodIdentityAssociationInput;
    type Output = UpdatePodIdentityAssociationOutput;
    type Error = UpdatePodIdentityAssociationError;

    const NAME: &'static str = "UpdatePodIdentityAssociation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_POD_IDENTITY_ASSOCIATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdatePodIdentityAssociationError::from(
                UpdatePodIdentityAssociationErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let invalid_parameter = |message: String| {
            UpdatePodIdentityAssociationError::from(
                UpdatePodIdentityAssociationErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // AUTHORIZATION FIRST, before the body's forms and before the association is
        // resolved: a `roleArn` an update names is a role the caller is passing to
        // Amazon EKS exactly as a create's is, so the same pass-role rule decides
        // whether this caller may hand it over, and a value that names no role of this
        // account is a PERMISSION failure whatever else the request carries
        // ([`state::pass_role_refusal`]). `targetRoleArn` is not passed here, for the
        // reason `CreatePodIdentityAssociation` gives: the association's own role
        // assumes it, not the caller.
        if let Some(role_arn) = input.role_arn.as_deref() {
            if let Some(why) = state::pass_role_refusal(role_arn) {
                return Err(
                    UpdatePodIdentityAssociationErrorModeled::AccessDeniedException(
                        AccessDeniedException { message: Some(why) },
                    )
                    .into(),
                );
            }
        }

        // FORM FIRST, for the same reason as every other operation that takes
        // these labels: a value outside its documented form names nothing.
        if !state::is_cluster_name(&input.cluster_name) {
            return Err(invalid_parameter(state::cluster_name_refusal(
                "clusterName",
                &input.cluster_name,
            )));
        }
        if !state::is_association_id(&input.association_id) {
            return Err(invalid_parameter(state::association_id_refusal(
                "associationId",
                &input.association_id,
            )));
        }
        for (member, value) in [
            ("roleArn", input.role_arn.as_deref()),
            ("targetRoleArn", input.target_role_arn.as_deref()),
        ] {
            if let Some(value) = value {
                if !state::is_iam_role_arn(value) {
                    return Err(invalid_parameter(state::role_arn_refusal(member, value)));
                }
            }
        }
        // "In an update, you can change the IAM role, the target IAM role, or
        // disableSessionTags. You must change at least one of these in an update."
        // `policy` is the fourth member an update may carry, and carrying it alone
        // is also a change.
        if input.role_arn.is_none()
            && input.target_role_arn.is_none()
            && input.disable_session_tags.is_none()
            && input.policy.is_none()
        {
            return Err(invalid_parameter(
                "An update must change at least one of 'roleArn', 'targetRoleArn', \
                 'disableSessionTags' or 'policy'"
                    .to_string(),
            ));
        }

        let region = state::request_region(ctx.parts());

        // THE ROLE'S TRUST RELATIONSHIP, once the request's own forms are sound: an
        // update may only point the association at a role EKS Pod Identity can
        // assume for its Pods, which is the same rule the create applies to the role
        // it stores ([`state::pod_identity_role_trust_refusal`]) — an association
        // whose role stopped trusting the Pod Identity principal would issue no
        // credentials. It runs with no lock held: it is a call out to another service.
        if let Some(role_arn) = input.role_arn.as_deref() {
            if let Some(why) =
                state::pod_identity_role_trust_refusal(ctx, &region, role_arn).await
            {
                return Err(invalid_parameter(why.to_string()));
            }
        }

        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The two halves report DIFFERENT resources, so they carry different
        // payloads: an absent cluster is reported as the cluster (named, with
        // `clusterName` set), an absent association as the association (named by its
        // id, with no identity member) — see [`crate::state::cluster_not_found`].
        let not_found = |exception: ResourceNotFoundException| {
            UpdatePodIdentityAssociationError::from(
                UpdatePodIdentityAssociationErrorModeled::ResourceNotFoundException(exception),
            )
        };

        let Some(cluster) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(not_found(crate::state::cluster_not_found(
                &input.cluster_name,
            )));
        };
        // EKS Pod Identity hands credentials to Pods through the Amazon EKS-managed
        // plane, which a cluster `RegisterCluster` connected has none of: this operation
        // does not apply to it. The gate is a fact about the CLUSTER, so it precedes
        // resolving the association id and judging the body.
        if cluster.is_connected() {
            return Err(UpdatePodIdentityAssociationErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        let key = (
            region,
            input.cluster_name.clone(),
            input.association_id.clone(),
        );
        let Some(record) = guard.pod_identity_associations.get_mut(&key) else {
            return Err(not_found(crate::state::pod_identity_association_not_found(
                &input.association_id,
            )));
        };

        // "When using this policy, disableSessionTags must be set to true" — the
        // rule holds for the association AFTER the update, so it is checked
        // against the value this request leaves behind, not only the one it sends.
        let policy = input.policy.clone().or_else(|| record.policy.clone());
        let disable_session_tags = input.disable_session_tags.or(record.disable_session_tags);
        if policy.is_some() && disable_session_tags != Some(true) {
            return Err(invalid_parameter(
                state::POLICY_REQUIRES_DISABLED_SESSION_TAGS.to_string(),
            ));
        }

        // `clientRequestToken` is `@idempotencyToken`: a retry resends the token of
        // the call that already applied, so it is absorbed — answered with the
        // record as it stands, without moving `modifiedAt` a second time. It is
        // matched against the token of the last UPDATE only: a token absorbs a
        // retry of the call that sent it, so a create's token reused here can
        // never swallow this update's change. A blank token identifies nothing and
        // never matches.
        let replayed = input
            .client_request_token
            .as_deref()
            .filter(|token| !token.trim().is_empty());
        if let Some(token) = replayed {
            if record.update_request_token.as_deref() == Some(token) {
                return Ok(UpdatePodIdentityAssociationOutput {
                    association: Some(record.to_association()),
                });
            }
        }

        // An absent member leaves the stored value alone: "you can change the IAM
        // role, the target IAM role, or disableSessionTags", and the association
        // can never be moved between clusters, namespaces or service accounts, so
        // nothing else is touched.
        if let Some(role_arn) = input.role_arn.clone() {
            record.role_arn = role_arn;
        }
        if let Some(target_role_arn) = input.target_role_arn.clone() {
            record.target_role_arn = Some(target_role_arn);
            // `externalId` is minted for the association's target role and then
            // kept: it is what the target role's trust policy conditions on, so
            // pointing the association at another target role must not silently
            // invalidate a trust policy that already names it.
            if record.external_id.is_none() {
                record.external_id = Some(state::derive_association_external_id(
                    &record.association_arn,
                ));
            }
        }
        record.policy = policy;
        record.disable_session_tags = disable_session_tags;
        record.modified_at = now;
        // The create's own token stays where it is; only the update slot moves.
        record.update_request_token = input.client_request_token.clone();

        Ok(UpdatePodIdentityAssociationOutput {
            association: Some(record.to_association()),
        })
    }
}
