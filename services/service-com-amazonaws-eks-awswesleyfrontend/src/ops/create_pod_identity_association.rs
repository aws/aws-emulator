// SPDX-License-Identifier: CC0-1.0

//! @op CreatePodIdentityAssociation
//! IMPLEMENTED. Writes the [`crate::state::PodIdentityAssociationRecord`] that
//! `DescribePodIdentityAssociation`, `UpdatePodIdentityAssociation`,
//! `DeletePodIdentityAssociation` and `ListPodIdentityAssociations` read: the
//! `associationId`, `associationArn`, `externalId`, `createdAt` and `modifiedAt`
//! it answers with are minted once here and stored, never recomputed on a read.
//!
//! The `roleArn` the request asks Amazon EKS to pass is authorized FIRST — before
//! the body's forms and before the cluster is resolved — and then every identifier
//! the request carries is checked for its documented FORM before any resource is
//! resolved: the cluster name, the target IAM role ARN and the Kubernetes names, so
//! a request that cannot name anything is an `InvalidParameterException` rather than
//! a not-found. With the forms sound, the CLUSTER the association would be a child
//! of is resolved ([`cluster_refusal`]) — a request naming no cluster of this account
//! is the not-found it is, whatever the role it carries trusts — and only then is the
//! role's TRUST RELATIONSHIP resolved against IAM: an association is only written for
//! a role the EKS Pod Identity principal may assume
//! ([`crate::state::pod_identity_role_trust_refusal`]),
//! because one bound to any other role could never issue credentials to a Pod.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreatePodIdentityAssociation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an EKS Pod Identity association between a service account in an Amazon EKS
//! cluster and an IAM role with EKS Pod Identity. Use EKS Pod Identity to give temporary
//! IAM credentials to Pods and the credentials are rotated automatically. Amazon EKS Pod
//! Identity associations provide the ability to manage credentials for your applications,
//! similar to the way that Amazon EC2 instance profiles provide credentials to Amazon EC2
//! instances. If a Pod uses a service account that has an association, Amazon EKS sets
//! environment variables in the containers of the Pod. The environment variables configure
//! the Amazon Web Services SDKs, including the Command Line Interface, to use the EKS Pod
//! Identity credentials. EKS Pod Identity is a simpler method than IAM roles for service
//! accounts, as this method doesn't use OIDC identity providers. Additionally, you can
//! configure a role for EKS Pod Identity once, and reuse it across clusters. Similar to
//! Amazon Web Services IAM behavior, EKS Pod Identity associations are eventually
//! consistent, and may take several seconds to be effective after the initial API call
//! returns successfully. You must design your applications to account for these potential
//! delays. We recommend that you don’t include association create/updates in the critical,
//! high-availability code paths of your application. Instead, make changes in a separate
//! initialization or setup routine that you run less frequently. You can set a target IAM
//! role in the same or a different account for advanced scenarios. With a target role, EKS
//! Pod Identity automatically performs two role assumptions in sequence: first assuming the
//! role in the association that is in this account, then using those credentials to assume
//! the target IAM role. This process provides your Pod with temporary credentials that have
//! the permissions defined in the target role, allowing secure access to resources in
//! another Amazon Web Services account.
//!
//! ## Input
//!
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (String) **required** — The name of the cluster to create the EKS Pod
//!   Identity association in.
//! * `disableSessionTags` (BoxedBoolean) — Disable the automatic sessions tags that are
//!   appended by EKS Pod Identity. EKS Pod Identity adds a pre-defined set of session tags
//!   when it assumes the role. You can use these tags to author a single role that can work
//!   across resources by allowing access to Amazon Web Services resources based on matching
//!   tags. By default, EKS Pod Identity attaches six tags, including tags for cluster name,
//!   namespace, and service account name. For the list of tags added by EKS Pod Identity, see
//!   List of session tags added by EKS Pod Identity in the Amazon EKS User Guide. Amazon Web
//!   Services compresses inline session policies, managed policy ARNs, and session tags into
//!   a packed binary format that has a separate …
//! * `namespace` (String) **required** — The name of the Kubernetes namespace inside the
//!   cluster to create the EKS Pod Identity association in. The service account and the Pods
//!   that use the service account must be in this namespace.
//! * `policy` (String) — An optional IAM policy in JSON format (as an escaped string) that
//!   applies additional restrictions to this pod identity association beyond the IAM policies
//!   attached to the IAM role. This policy is applied as the intersection of the role's
//!   policies and this policy, allowing you to reduce the permissions that applications in
//!   the pods can use. Use this policy to enforce least privilege access while still
//!   leveraging a shared IAM role across multiple applications. Important considerations
//!   Session tags: When using this policy, disableSessionTags must be set to true. Target
//!   role permissions: If you specify both a TargetRoleArn and a policy, the policy
//!   restrictions apply only to the target role's …
//! * `roleArn` (String) **required** — The Amazon Resource Name (ARN) of the IAM role to
//!   associate with the service account. The EKS Pod Identity agent manages credentials to
//!   assume this role for applications in the containers in the Pods that use this service
//!   account.
//! * `serviceAccount` (String) **required** — The name of the Kubernetes service account
//!   inside the cluster to associate the IAM credentials with.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources. The following basic restrictions
//!   apply to tags: Maximum number of tags per resource – 50 For each resource, each tag key
//!   must be unique, and each tag key can have only one value. Maximum key length – 128
//!   Unicode characters in UTF-8 Maximum value length – 256 Unicode characters in UTF-8 If
//!   your tagging schema is used across multiple services and resources, remember that other
//!   services may have restrictions on allowed characters. Generally allowed characters are:
//!   letters, numbers, and spaces …
//! * `targetRoleArn` (String) — The Amazon Resource Name (ARN) of the target IAM role to
//!   associate with the service account. This role is assumed by using the EKS Pod Identity
//!   association role, then the credentials for this role are injected into the Pod. When you
//!   run applications on Amazon EKS, your application might need to access Amazon Web
//!   Services resources from a different role that exists in the same or different Amazon Web
//!   Services account. For example, your application running in “Account A” might need to
//!   access resources, such as Amazon S3 buckets in “Account B” or within “Account A” itself.
//!   You can create a association to access Amazon Web Services resources in “Account B” by
//!   creating two IAM roles: a role in …
//!
//! ## Output
//!
//! * `association` (PodIdentityAssociation) — The full description of your new association.
//!   The description includes an ID for the association. Use the ID of the association in
//!   further actions to manage the association.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — widened in through `extra_errors` (see below):
//!   the class the pass-role authorization of `roleArn` answers with.
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
    self, PodIdentityAssociationRecord, KUBERNETES_NAME_PATTERN,
    MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreatePodIdentityAssociationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreatePodIdentityAssociationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// Two classes are widened in through `extra_errors`: this SERVICE models both shapes
// and many of its operations declare them, but this one lists neither.
// `ClientException` is the class every operation of the Amazon EKS-managed plane
// refuses a cluster this service does not manage with
// ([`crate::state::connected_cluster_refusal`]), and `AccessDeniedException` — 403 —
// is the class the pass-role authorization of `roleArn` answers with, the same one
// `CreateCluster`, `CreateNodegroup`, `CreateFargateProfile` and `CreateCapability`
// widen in for the same question ([`crate::state::pass_role_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#CreatePodIdentityAssociation",
    extra_errors = ["ClientException", "AccessDeniedException"]
)]
#[derive(Debug)]
pub enum CreatePodIdentityAssociationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_pod_identity_association::CreatePodIdentityAssociationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreatePodIdentityAssociationError = crate::scaffolded::Scaffolded<CreatePodIdentityAssociationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreatePodIdentityAssociationErrorModeled> for CreatePodIdentityAssociationError {
    fn from(modeled: CreatePodIdentityAssociationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePodIdentityAssociation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreatePodIdentityAssociation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePodIdentityAssociation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePodIdentityAssociation<Eng>
{
    type Input = CreatePodIdentityAssociationInput;
    type Output = CreatePodIdentityAssociationOutput;
    type Error = CreatePodIdentityAssociationError;

    const NAME: &'static str = "CreatePodIdentityAssociation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POD_IDENTITY_ASSOCIATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_POD_IDENTITY_ASSOCIATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreatePodIdentityAssociationError::from(
                CreatePodIdentityAssociationErrorModeled::ServerException(ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        // The argument-level refusal: it names the member at fault and the value the
        // request carried, so the caller has to fix the request from this text alone
        // — in the operation's own wording where it has one
        // ([`crate::state::role_arn_refusal`]), and the way the boundary validator
        // words it for the members it would have refused itself.
        let invalid_parameter = |message: String| {
            CreatePodIdentityAssociationError::from(
                CreatePodIdentityAssociationErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // AUTHORIZATION FIRST. `roleArn` is the role the EKS Pod Identity agent has
        // Amazon EKS assume for the Pods of this service account, so the caller is
        // passing it to the service: Amazon EKS authorizes that `iam:PassRole`
        // BEFORE it reports on anything else the request carries, and a value that
        // names no role of this account is a PERMISSION failure whatever else is
        // wrong with the request — including a value that is not an ARN at all,
        // which is evaluated as the role of an account that is not this one
        // ([`crate::state::pass_role_refusal`]). The question needs no state, which
        // is why it is answered before the body's forms and before the cluster is
        // resolved: a caller fixing its policy must not be told about a missing
        // cluster or a malformed name instead.
        //
        // `targetRoleArn` is NOT passed here: "you can set a target IAM role in the
        // same or a different account", and EKS Pod Identity assumes it with the
        // association's own role rather than with the caller's `iam:PassRole`, so a
        // target role of another account is ordinary use and only its FORM is
        // judged below.
        if let Some(why) = state::pass_role_refusal(&input.role_arn) {
            return Err(CreatePodIdentityAssociationErrorModeled::AccessDeniedException(
                AccessDeniedException { message: Some(why) },
            )
            .into());
        }

        // FORM NEXT: an identifier outside its documented form names nothing, in
        // any account or Region, so it is an invalid parameter and never a
        // not-found. None of these members carries a modelled constraint, so
        // nothing refused them before this handler ran.
        if !state::is_cluster_name(&input.cluster_name) {
            return Err(invalid_parameter(state::cluster_name_refusal(
                "clusterName",
                &input.cluster_name,
            )));
        }
        for (member, value) in [
            ("namespace", &input.namespace),
            ("serviceAccount", &input.service_account),
        ] {
            if !state::is_kubernetes_name(value) {
                return Err(invalid_parameter(format!(
                    "Value '{value}' at '{member}' failed to satisfy constraint: Member \
                     must satisfy regular expression pattern: {KUBERNETES_NAME_PATTERN}"
                )));
            }
        }
        // `roleArn` is not re-judged here: the authorization above accepts only an
        // IAM role ARN of this account and partition, so every value outside the
        // form has already been answered as the permission failure it is.
        if let Some(target_role_arn) = input.target_role_arn.as_deref() {
            if !state::is_iam_role_arn(target_role_arn) {
                return Err(invalid_parameter(state::role_arn_refusal(
                    "targetRoleArn",
                    target_role_arn,
                )));
            }
        }
        // `policy`'s own documentation: "Session tags: When using this policy,
        // disableSessionTags must be set to true."
        if input.policy.is_some() && input.disable_session_tags != Some(true) {
            return Err(invalid_parameter(
                state::POLICY_REQUIRES_DISABLED_SESSION_TAGS.to_string(),
            ));
        }

        let region = state::request_region(ctx.parts());

        let now = SystemTime::now();

        // THE CLUSTER COMES BEFORE THE ROLE'S TRUST RELATIONSHIP. An association is a
        // child of a cluster ("You can view your available clusters with
        // ListClusters"), so the cluster the request names is resolved FIRST: a request
        // naming a cluster this account does not hold is answered as the not-found it
        // is, whatever the trust policy of the role it carries says, because a role is
        // only judged for an association there is a cluster to write it under. The
        // guard is taken for that resolution alone and released again, because the
        // trust question below is a call out to another service.
        {
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);
            if let Some(refusal) = cluster_refusal(
                guard
                    .clusters
                    .get(&(region.clone(), input.cluster_name.clone())),
                &input.cluster_name,
            ) {
                return Err(refusal);
            }
        }

        // THE ROLE'S TRUST RELATIONSHIP, once the request's own forms are sound:
        // the role is one Amazon EKS assumes FOR THIS ASSOCIATION, so an
        // association bound to a role that does not let the EKS Pod Identity
        // principal assume it could never issue credentials to a Pod and is
        // refused rather than stored
        // ([`state::pod_identity_role_trust_refusal`]). It runs with no lock held:
        // it is a call out to another service.
        if let Some(why) =
            state::pod_identity_role_trust_refusal(ctx, &region, &input.role_arn).await
        {
            return Err(invalid_parameter(why.to_string()));
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The cluster is resolved AGAIN under the guard that writes, because the guard
        // taken for the resolution above was released while IAM was asked: nothing is
        // written under a cluster that stopped existing in between.
        if let Some(refusal) = cluster_refusal(
            guard
                .clusters
                .get(&(region.clone(), input.cluster_name.clone())),
            &input.cluster_name,
        ) {
            return Err(refusal);
        }

        // One service account in one namespace can hold at most one association:
        // the pair is what EKS Pod Identity resolves a Pod's credentials by, so a
        // second association for the same pair would make that lookup ambiguous.
        let existing = guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .find(|record| {
                record.namespace == input.namespace
                    && record.service_account == input.service_account
            })
            .cloned();
        if let Some(existing) = existing {
            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once
            // per call and RESENDS the same value when it retries, so a retry of
            // the call that created this association is absorbed and answered with
            // that association rather than with a conflict. A blank token
            // identifies nothing, so it never matches.
            let replayed = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if let Some(token) = replayed {
                if existing.client_request_token.as_deref() == Some(token) {
                    return Ok(CreatePodIdentityAssociationOutput {
                        association: Some(existing.to_association()),
                    });
                }
            }
            // What the caller could not know is which IAM role the pair is
            // already bound to, so that is what the refusal names — read off the
            // STORED association, never off this request.
            return Err(
                CreatePodIdentityAssociationErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(crate::state::service_account_already_associated(
                            &existing.role_arn,
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        if guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .count()
            >= MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER
        {
            return Err(
                CreatePodIdentityAssociationErrorModeled::ResourceLimitExceededException(
                    ResourceLimitExceededException {
                        cluster_name: Some(input.cluster_name.clone()),
                        message: Some(format!(
                            "You have reached the limit of \
                             {MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER} EKS Pod Identity \
                             associations for cluster {}",
                            input.cluster_name
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let association_id = guard.mint_association_id(
            &region,
            &input.cluster_name,
            &input.namespace,
            &input.service_account,
        );
        let association_arn =
            state::derive_association_arn(&region, &input.cluster_name, &association_id);
        let record = PodIdentityAssociationRecord {
            cluster_name: input.cluster_name.clone(),
            association_id: association_id.clone(),
            // `externalId` exists to be matched in the TARGET role's trust policy,
            // so an association with no target role has nothing to condition and
            // carries none.
            external_id: input
                .target_role_arn
                .as_ref()
                .map(|_| state::derive_association_external_id(&association_arn)),
            association_arn,
            namespace: input.namespace.clone(),
            service_account: input.service_account.clone(),
            role_arn: input.role_arn.clone(),
            target_role_arn: input.target_role_arn.clone(),
            policy: input.policy.clone(),
            disable_session_tags: input.disable_session_tags,
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            // A create is also the association's last modification, so both
            // timestamps start equal; only an update moves `modifiedAt`.
            created_at: now,
            modified_at: now,
            client_request_token: input.client_request_token.clone(),
            // No update has applied to a brand-new association.
            update_request_token: None,
            // An association created through THIS API is owned by the caller:
            // `ownerArn` is set only on one an Amazon EKS add-on created
            // (`CreateAddon$podIdentityAssociations`).
            owner_arn: None,
        };

        let association = record.to_association();
        guard.pod_identity_associations.insert(
            (region.clone(), input.cluster_name.clone(), association_id),
            record,
        );
        // The store's lock is released before the EKS Auth plane is programmed
        // with the set this service now holds: that plane is what resolves a
        // pod identity token to this association, so a created association can be
        // exchanged for credentials as soon as this call returns.
        drop(guard);
        crate::state::pod_identity_plane::program_cluster_associations_on(
            &*self.engine,
            &store,
            &(region, input.cluster_name),
        );

        Ok(CreatePodIdentityAssociationOutput {
            association: Some(association),
        })
    }
}

/// Why no association can be written under the cluster `cluster_name` names, or `None`
/// when it is a cluster of this account that EKS Pod Identity applies to.
///
/// Both refusals are facts about the CLUSTER rather than about the body, which is why
/// they are answered as soon as the cluster is resolved:
///
/// * a cluster this store does not hold is the not-found this operation models
///   ([`crate::state::cluster_not_found`]);
/// * EKS Pod Identity hands credentials to Pods through the Amazon EKS-managed plane,
///   which a cluster `RegisterCluster` connected has none of: this operation does not
///   apply to it ([`crate::state::connected_cluster_refusal`]).
///
/// ONE reading, shared by the resolution that precedes the role's trust question and the
/// one the writing guard repeats, so the two cannot answer differently.
fn cluster_refusal(
    cluster: Option<&crate::state::ClusterRecord>,
    cluster_name: &str,
) -> Option<CreatePodIdentityAssociationError> {
    let Some(cluster) = cluster else {
        return Some(
            CreatePodIdentityAssociationErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(cluster_name),
            )
            .into(),
        );
    };
    cluster.is_connected().then(|| {
        CreatePodIdentityAssociationErrorModeled::ClientException(
            crate::state::connected_cluster_refusal(cluster_name),
        )
        .into()
    })
}
