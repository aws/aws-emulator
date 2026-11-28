// SPDX-License-Identifier: CC0-1.0

//! @op CreateAddon
//!
//! IMPLEMENTED. `CreateAddon` installs one add-on ON an existing cluster: it mints a
//! [`crate::state::AddonRecord`] in the shared engine's store, keyed by the cluster's
//! own `(region, name)` key plus the add-on name, and answers with the `Addon` it just
//! wrote. `DescribeAddon` reads that same record back.
//!
//! An add-on is a child of a cluster, so the layers this handler checks run from the
//! OUTSIDE IN, and each layer's refusal is the modelled error for what it found:
//!
//! 0. the request's OWN ADDRESSING: the `@httpLabel` `clusterName` must be a name a
//!    cluster of this API could carry, and the role the add-on's service account is
//!    GIVEN — named in `serviceAccountRoleArn` and in the `roleArn` of each
//!    `podIdentityAssociations` entry — must be a role this caller may hand Amazon EKS
//!    (answered before dispatch as the 403 this operation does not model,
//!    `crate::service::precedence`) that names a role a member of this API could carry
//!    at all — otherwise `InvalidParameterException` (400). Neither
//!    question needs any state, and a label outside the cluster-name grammar names no
//!    cluster in any Region, so answering it as a not-found would invite a caller to
//!    create a cluster under a name Amazon EKS will never accept. The LABEL half runs
//!    ahead of the framework's own refusal of a body member, which only a pre-dispatch
//!    layer can order (`crate::service::precedence`);
//! 0b. the request must NAME AN ADD-ON at all: `addonName` says what to install, so a
//!    request carrying none asks for nothing and is refused with the sentence this API
//!    states for that parameter — `InvalidParameterException` (400) — before the
//!    cluster is resolved, since no cluster could hold what the request did not name;
//! 1. the cluster named by the `@httpLabel` must exist IN THE REQUEST'S REGION —
//!    otherwise `ResourceNotFoundException` (404), before any member of the body is
//!    looked at, because a request against a cluster that does not exist cannot be
//!    answered whatever else it says;
//! 2. that cluster must have an Amazon EKS-managed control plane for an add-on to
//!    live in — otherwise the connected-cluster refusal, which no wait can change;
//! 3. the body's own members must hold up — otherwise `InvalidParameterException`
//!    (400), naming the member and what it must be. The add-on and the build asked
//!    for are held to the CATALOG the same `DescribeAddonVersions` serves: a name
//!    the catalog publishes no build of for the cluster's Kubernetes version, and a
//!    build whose compatibilities do not carry that version, are both refused, and
//!    the `configurationValues` document is read and held to the schema
//!    `DescribeAddonConfiguration` publishes for that build. What the request ASKS
//!    FOR does not depend on what the cluster is doing, so this layer runs BEFORE
//!    the one below: a caller debugging its request is told what is wrong with it
//!    rather than what the cluster is busy with;
//! 4. that cluster must be `ACTIVE` — otherwise `InvalidRequestException` (400), the
//!    modelled error for "the request is invalid given the state of the cluster";
//! 5. the add-on must not already be installed — otherwise `ResourceInUseException`
//!    (409), unless the request replays the `clientRequestToken` that created it, in
//!    which case that add-on is returned again. This layer runs BEFORE the checks on
//!    the child resources the add-on would create, so a retry of a call that already
//!    succeeded is absorbed rather than colliding with its own children;
//! 6. the EKS Pod Identity associations the add-on would create must fit the
//!    cluster's association ceiling and must not double-book a `(namespace, service
//!    account)` pair — otherwise `InvalidRequestException` (400) or
//!    `ResourceInUseException` (409), decided before anything is written, so a
//!    request that cannot be carried out in full writes nothing.
//!
//! What a refusal's PAYLOAD reports beside the sentence is a rule of its own: a
//! refusal about the add-on the request names, or about the cluster, reports them in
//! the exception's `addonName`/`clusterName`, while one about the request's own text
//! carries the sentence alone — see the two refusal shapes the handler builds.
//!
//! The record carries both the caller's input and the server-generated parts
//! (`addonArn`, `createdAt`, `modifiedAt`, the assigned version, the namespace the
//! add-on went into, the ARNs of the EKS Pod Identity associations it owns), so a
//! later read returns exactly these bytes rather than recomputing them.
//!
//! An add-on asked for `podIdentityAssociations` CREATES those associations: one
//! [`crate::state::PodIdentityAssociationRecord`] per entry, minted through the same
//! path `CreatePodIdentityAssociation` uses and carrying this add-on's ARN as its
//! `ownerArn`, so `DescribePodIdentityAssociation` and `ListPodIdentityAssociations`
//! find exactly the associations the add-on reports.
//!
//! A new add-on is reported `CREATING` and the clock owes it the TERMINAL state its
//! install settles in [`crate::state::ADDON_ACTIVATION_DELAY`] later: an install is
//! asynchronous and settles in the state that reflects what the cluster could do with
//! it — running, running without the replica it needs, or failed because the
//! Kubernetes resources it is applied against are not there or refused it
//! ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::addon_install_outcome`]).
//! Both the outcome and the due instant are stored on the record and materialised by
//! whichever read first runs after it.
//!
//! An install the service never STARTS is the exception, and it is the same decision
//! made at the same point: where the unmet precondition is the ADD-ON's own rather
//! than the cluster's — the AWS Marketplace subscription its software cannot be
//! pulled without — nothing is applied, so no later instant can change the answer and
//! there is no `CREATING` to report. Such a request is still ACCEPTED: the add-on is
//! created in the terminal state it is already in, carrying the health issue that
//! says what is missing, which is what a client reads off it and remediates
//! ([`crate::state::AddonInstallOutcome::refused`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateAddon`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an Amazon EKS add-on. Amazon EKS add-ons help to automate the provisioning and
//! lifecycle management of common operational software for Amazon EKS clusters. For more
//! information, see Amazon EKS add-ons in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `addonName` (String) **required** — The name of the add-on. The name must match one of
//!   the names returned by DescribeAddonVersions.
//! * `addonVersion` (String) — The version of the add-on. The version must match one of the
//!   versions returned by DescribeAddonVersions .
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `clusterName` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The name of your cluster.
//! * `configurationValues` (String) — The set of configuration values for the add-on that's
//!   created. The values that you provide are validated against the schema returned by
//!   DescribeAddonConfiguration.
//! * `namespaceConfig` (AddonNamespaceConfigRequest) — The namespace configuration for the
//!   addon. If specified, this will override the default namespace for the addon.
//! * `podIdentityAssociations` (AddonPodIdentityAssociationsList) — An array of EKS Pod
//!   Identity associations to be created. Each association maps a Kubernetes service account
//!   to an IAM role. For more information, see Attach an IAM Role to an Amazon EKS add-on
//!   using EKS Pod Identity in the Amazon EKS User Guide.
//! * `resolveConflicts` (ResolveConflicts, one of: NONE | OVERWRITE | PRESERVE) — How to
//!   resolve field value conflicts for an Amazon EKS add-on. Conflicts are handled based on
//!   the value you choose: None – If the self-managed version of the add-on is installed on
//!   your cluster, Amazon EKS doesn't change the value. Creation of the add-on might fail.
//!   Overwrite – If the self-managed version of the add-on is installed on your cluster and
//!   the Amazon EKS default value is different than the existing value, Amazon EKS changes
//!   the value to the Amazon EKS default value. Preserve – This is similar to the NONE
//!   option. If the self-managed version of the add-on is installed on your cluster Amazon
//!   EKS doesn't change the add-on resource properties. Creation of the add-on might fail if
//!   …
//! * `serviceAccountRoleArn` (RoleArn, len 1..255) — The Amazon Resource Name (ARN) of an
//!   existing IAM role to bind to the add-on's service account. The role must be assigned the
//!   IAM permissions required by the add-on. If you don't specify an existing IAM role, then
//!   the add-on uses the permissions assigned to the node IAM role. For more information, see
//!   Amazon EKS node IAM role in the Amazon EKS User Guide. To specify an existing IAM role,
//!   you must have an IAM OpenID Connect (OIDC) provider created for your cluster. For more
//!   information, see Enabling IAM roles for service accounts on your cluster in the Amazon
//!   EKS User Guide.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//!
//! ## Output
//!
//! * `addon` (Addon) — An Amazon EKS add-on. For more information, see Amazon EKS add-ons in
//!   the Amazon EKS User Guide.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, AddonRecord,
    ADDON_DEFAULT_NAMESPACE, ADDON_STATUS_CREATING,
    CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN, DEFAULT_KUBERNETES_VERSION, MAX_ADDONS_PER_CLUSTER,
    MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER,
};
use std::time::SystemTime;

/// The member a `CreateAddon` names the add-on to install in, as the model spells it
/// and as the walk that refuses an omitted one names it — the member this handler's
/// required-parameter refusal is about.
const ADDON_NAME_MEMBER: &str = "addonName";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAddonError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateAddonError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#CreateAddon")]
#[derive(Debug)]
pub enum CreateAddonErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_addon::CreateAddonError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateAddonError = crate::scaffolded::Scaffolded<CreateAddonErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateAddonErrorModeled> for CreateAddonError {
    fn from(modeled: CreateAddonErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAddon` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateAddon<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAddon<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAddon<Eng>
{
    type Input = CreateAddonInput;
    type Output = CreateAddonOutput;
    type Error = CreateAddonError;

    const NAME: &'static str = "CreateAddon";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ADDON_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ADDON_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateAddonError::from(CreateAddonErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                addon_name: Some(input.addon_name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        // The two refusal shapes every parameter layer below reports through, both the
        // `InvalidParameterException` the operation models for a member of the body —
        // "The specified parameter is invalid. Review the available parameters for the
        // API request." — and differing only in WHAT the payload reports beside the
        // sentence.
        //
        // RULE: a refusal ABOUT THE ADD-ON the request names — the add-on this cluster
        // is not offered, the build it is not offered, the configuration that add-on
        // does not take — reports the add-on and its cluster in the exception's
        // `addonName`/`clusterName`; one about the REQUEST'S OWN TEXT — a member of a
        // form no add-on name, service account or namespace can have, the role that
        // cannot be given to the add-on, and a document that cannot be read at all —
        // carries the sentence alone, as the recorded service answers each.
        let invalid_parameter = |message: String| {
            CreateAddonError::from(CreateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };
        let invalid_parameter_about_the_addon = |message: String| {
            CreateAddonError::from(CreateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // LAYER 0 — the request's own addressing, which needs nothing from the store
        // and so is judged before it is even locked. `clusterName` arrives in the path
        // (`POST /clusters/{clusterName}/addons`) and every cluster name of this API
        // obeys one grammar and one length maximum (`ClusterName` is `@length(1..100)`
        // + `@pattern`, `crate::state::is_cluster_name`), so a label outside it names
        // no cluster in any Region rather than one this Region does not hold: it is
        // refused as the invalid parameter it is, before anything is looked up, in the
        // sentence Amazon EKS states for that parameter and reporting the value the
        // request supplied ([`crate::state::cluster_name_refusal`]). A 404 here would
        // invite a caller that passed a cluster ARN where a bare name belongs to create
        // a cluster under a name Amazon EKS will never accept.
        //
        // The same answer is given BEFORE the framework's validating walk could refuse
        // a member of the body, which is the order the recorded service keeps and which
        // only a pre-dispatch layer can restore (see `crate::service::precedence`); the
        // check stands here too, so this operation reports it whichever path reaches it.
        if !state::is_cluster_name(&input.cluster_name) {
            return Err(CreateAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(state::cluster_name_refusal(
                        "clusterName",
                        &input.cluster_name,
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // Still the request's own addressing: the role the add-on's Kubernetes service
        // account is GIVEN, which this request names in `serviceAccountRoleArn` and in
        // the `roleArn` of each `podIdentityAssociations` entry — the same role, bound
        // through EKS Pod Identity rather than through the cluster's OIDC provider. The
        // add-on operations ask two questions about it, both decided from the value
        // alone and both before the cluster is resolved. Whether this caller may PASS it
        // is the service-wide pass-role question, answered as the 403 this operation
        // does not model, before dispatch (`crate::service::precedence`), so it is not
        // re-judged here. Whether the value names a role a member of this API could
        // carry at all is this one ([`crate::state::addon_role_naming_no_role`]): a bare
        // role name where an ARN belongs, a role name or path outside the grammar IAM
        // publishes, and a value past the length the model states for `RoleArn` all name
        // no role in any account, and Amazon EKS refuses every one of them with the
        // sentence it states for the add-on's service account role
        // ([`crate::state::ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL`]), which takes nothing
        // from the request.
        if state::addon_role_naming_no_role(
            input.service_account_role_arn.as_deref(),
            input.pod_identity_associations.as_deref().unwrap_or_default(),
        )
        .is_some()
        {
            return Err(invalid_parameter(
                state::ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL.to_string(),
            ));
        }
        // And whether IAM HOLDS the role that value names, which only IAM can answer:
        // a role the account does not have cannot be given to the add-on's service
        // account either, and it is the same refusal
        // ([`crate::state::iam_holds_no_such_role`]). It runs with no lock held — it is
        // a call out to another service — and refuses nothing where there is no IAM to
        // ask, because a refusal is grounded in what IAM answered.
        if let Some(role_arn) = input.service_account_role_arn.as_deref() {
            if state::iam_holds_no_such_role(ctx, &region, role_arn).await {
                return Err(invalid_parameter(
                    state::ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL.to_string(),
                ));
            }
        }

        // STILL BEFORE THE STORE — the request's COMPLETENESS. `addonName` is the
        // parameter that says WHAT to install, and the model makes it `@required`, so a
        // request that names no add-on at all asks for nothing: there is no name for
        // the catalog to be asked about and nothing for any cluster to hold. Amazon EKS
        // answers such a request with the sentence it states for that parameter
        // ([`crate::state::parameter_required_message`]) rather than with the cluster it
        // was addressed to, and it answers it whether the member is absent — which this
        // service's codec reads as blank — or present and blank, because the two name an
        // add-on equally little. A value that is present and NAMES something, but
        // nothing an add-on could be called, is a different refusal: it is judged with
        // the rest of the body, below, after the cluster.
        if input.addon_name.trim().is_empty() {
            return Err(invalid_parameter(state::parameter_required_message(
                ADDON_NAME_MEMBER,
            )));
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // LAYER 1 — the cluster. An add-on is created ON a cluster
        // (`POST /clusters/{clusterName}/addons`) and a cluster name identifies a
        // cluster only within one region, so the cluster this installs onto is the one
        // in the region the request is signed for. Nothing in the body can make a
        // request against a cluster that does not exist answerable, so this is checked
        // FIRST and reported as the modelled 404.
        let cluster_key = (region.clone(), input.cluster_name.clone());
        let Some(cluster) = guard.clusters.get(&cluster_key) else {
            // The cluster is the resource this reports missing, so the payload names
            // it and nothing else: the requested add-on's name is NOT echoed into
            // the exception ([`crate::state::cluster_not_found`]).
            return Err(CreateAddonErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };

        // LAYER 2 — whether this cluster has an Amazon EKS-managed plane at all. An
        // add-on is a piece of software Amazon EKS installs and manages in the cluster's
        // control plane, and a cluster `RegisterCluster` connected has none of that
        // plane: the operation does not apply to it, which the connected-cluster gate
        // reports permanently. It precedes the STATUS check below because no wait makes a
        // connected cluster take this call.
        if cluster.is_connected() {
            return Err(CreateAddonErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // LAYER 3 — the body's own members. Each is a rule the model cannot express,
        // so the boundary validator passed the request and this is where it is caught.
        //
        // They are judged BEFORE the state of the cluster (LAYER 4 below): what this
        // request ASKS FOR does not depend on what the cluster is doing, and the
        // recorded service answers a request whose members do not hold up with the
        // refusal those members earn rather than with the state of the cluster — so a
        // caller debugging its request is told what is wrong with it.
        //
        // The cluster's own Kubernetes version, which the catalog layers below narrow
        // to and the add-on's default version is taken from, and its status, which
        // LAYER 4 judges, are both read off the record here so nothing below holds a
        // borrow of it. Every cluster this far into the handler is an Amazon EKS
        // cluster and reports a version; the service's default Kubernetes version
        // stands in for a record that somehow carries none.
        let cluster_status = cluster.status.clone();
        let cluster_version = cluster
            .version
            .clone()
            .unwrap_or_else(|| DEFAULT_KUBERNETES_VERSION.to_string());

        if !state::is_addon_name_shaped(&input.addon_name) {
            return Err(invalid_parameter(format!(
                "addonName {:?} is not an add-on name: it must be a non-empty string of \
                 letters, digits, '-', '.' and '_'.",
                input.addon_name
            )));
        }
        // "The name of the add-on. The name must match one of the names returned by
        // DescribeAddonVersions" — and that listing is per Kubernetes version, so the
        // name has to be one the catalog publishes a build of FOR THE CLUSTER'S OWN
        // Kubernetes version. An add-on this cluster could not run is refused here,
        // before any version is resolved and before anything is written, rather than
        // installed at a version nothing published.
        if !guard.publishes_addon_for(&input.addon_name, &cluster_version, now) {
            return Err(invalid_parameter_about_the_addon(
                state::addon_not_supported_message(&input.addon_name, &cluster_version),
            ));
        }
        // "The version of the add-on. The version must match one of the versions
        // returned by DescribeAddonVersions" — the same listing, narrowed the same
        // way: a build the catalog does not publish, and a published build whose
        // `compatibilities` do not carry this cluster's Kubernetes version, are both
        // builds this cluster is not offered. A blank value names no build at all and
        // is read as a request that names none (see below).
        let requested_version = input
            .addon_version
            .clone()
            .filter(|version| !version.trim().is_empty());
        if let Some(requested) = requested_version.as_deref() {
            if !guard.publishes_addon_build_for(
                &input.addon_name,
                requested,
                &cluster_version,
                now,
            ) {
                // The two identity members of THIS refusal carry each other's request
                // parameter, which is the shape the service this emulates answers a
                // build it does not offer with
                // ([`crate::state::addon_version_not_supported`]) — unlike the add-on
                // refusals around it, which report each name in its own member.
                return Err(CreateAddonErrorModeled::InvalidParameterException(
                    state::addon_version_not_supported(&input.cluster_name, &input.addon_name),
                )
                .into());
            }
        }
        // The build this install runs: the caller's version when it named one, else
        // the one the catalog publishes as the DEFAULT for the cluster's Kubernetes
        // version (`DescribeAddonVersions`), so an installed add-on reports a version
        // the catalog offers. The catalog publishes a default build for every
        // Kubernetes version it publishes a build for at all, which the check above
        // has already established; the derived form stands in only for a record that
        // somehow reports a Kubernetes version the calendar does not carry.
        let addon_version = requested_version.unwrap_or_else(|| {
            guard
                .default_addon_version(&input.addon_name, &cluster_version, now)
                .unwrap_or_else(|| state::derive_default_addon_version(&cluster_version))
        });
        // `serviceAccountRoleArn` is judged with the request's own addressing, above:
        // the authorization of a role the request asks Amazon EKS to pass needs nothing
        // from the cluster, so it is not re-judged here.
        // "The set of configuration values for the add-on that's created. The values
        // that you provide are validated against the schema returned by
        // DescribeAddonConfiguration" — so the document is READ first and then held to
        // exactly that schema, the one the catalog publishes for the build being
        // installed (`crate::state::AddonCatalogRecord::configuration_violation`): a
        // key the add-on's schema does not declare, a declared key of the wrong JSON
        // type and a missing required property are each refused, naming the path at
        // fault. An add-on configuration is written in JSON or in YAML and EITHER form
        // is held to that schema, so the refusal names the format the document was
        // read as; a document that is neither cannot be read at all and is refused
        // for that ([`crate::state::read_addon_configuration`]) before any schema is
        // consulted. The build is one the catalog offers this cluster by the time this
        // runs (see above), so the schema it is held to is the one
        // `DescribeAddonConfiguration` publishes for that same build.
        let configuration = state::read_addon_configuration(input.configuration_values.as_deref());
        match &configuration {
            state::AddonConfiguration::Unreadable => {
                return Err(invalid_parameter(
                    state::ADDON_CONFIGURATION_UNREADABLE_MESSAGE.to_string(),
                ));
            }
            state::AddonConfiguration::Read { document, .. } => {
                if let Some(violation) = guard
                    .published_addon_build(&input.addon_name, &addon_version, now)
                    .and_then(|(addon, build)| addon.configuration_violation(build, document))
                {
                    return Err(invalid_parameter_about_the_addon(
                        state::addon_configuration_violation_message(
                            configuration.format(),
                            &violation,
                        ),
                    ));
                }
            }
            state::AddonConfiguration::Absent => {}
        }
        // `podIdentityAssociations` maps a Kubernetes service account to an IAM role,
        // so each entry's role must be a role ARN and each service account must be a
        // name a Kubernetes object can have, and one service account cannot be mapped
        // twice by the same add-on. Both form checks are the ones the pod identity
        // slice already shares ([`state::is_iam_role_arn`],
        // [`state::is_kubernetes_name`]).
        //
        // The list's LENGTH is refused first, before any per-entry work: the model
        // puts no `length` on `AddonPodIdentityAssociationsList`, so the boundary
        // validator accepts a list of any size, and a list longer than a cluster may
        // ever hold cannot be carried out whatever its entries say. Every loop below
        // therefore runs over at most
        // [`MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER`] entries.
        let requested_associations = input.pod_identity_associations.clone().unwrap_or_default();
        // Reported as `InvalidRequestException` rather than the
        // `ResourceLimitExceededException` `CreatePodIdentityAssociation` answers for
        // the same ceiling: `CreateAddon` does not model that error, and a cluster
        // that cannot hold the associations asked for is a statement about the state
        // of the cluster.
        let too_many = |requested: usize, held: Option<usize>| {
            // The held count is stated only where it was MEASURED: the
            // length-only refusal below returns before the store is walked.
            let held = match held {
                Some(held) => format!(" and cluster {} already holds {held}", input.cluster_name),
                None => String::new(),
            };
            CreateAddonError::from(CreateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "podIdentityAssociations asks for {requested} associations{held}; a \
                         cluster holds at most \
                         {MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER} EKS Pod Identity \
                         associations."
                    )),
                    ..Default::default()
                },
            ))
        };
        if requested_associations.len() > MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER {
            // The list alone is past the ceiling, so no walk of the store is needed to
            // know the request cannot be carried out.
            return Err(too_many(requested_associations.len(), None));
        }
        let mut seen_service_accounts = ::std::collections::BTreeSet::new();
        for association in &requested_associations {
            // The entry's `roleArn` is judged with the request's own addressing, at the
            // top of this handler: it names the same role `serviceAccountRoleArn` does,
            // so it earns the same refusal and earns it before the cluster is resolved.
            if !state::is_kubernetes_name(&association.service_account) {
                return Err(invalid_parameter(format!(
                    "podIdentityAssociations[].serviceAccount {:?} is not a Kubernetes \
                     object name (pattern {}).",
                    association.service_account,
                    state::KUBERNETES_NAME_PATTERN
                )));
            }
            if !seen_service_accounts.insert(association.service_account.clone()) {
                return Err(invalid_parameter(format!(
                    "podIdentityAssociations names serviceAccount {:?} more than once; \
                     each service account can be mapped at most once.",
                    association.service_account
                )));
            }
        }
        // `namespaceConfig.namespace` names a Kubernetes namespace in the cluster, so
        // a value no Kubernetes namespace can be named is refused rather than stored.
        // The namespace the add-on goes into is also the namespace of the service
        // accounts its EKS Pod Identity associations are for.
        if let Some(namespace) = input
            .namespace_config
            .as_ref()
            .and_then(|config| config.namespace.as_deref())
        {
            if !state::is_kubernetes_name(namespace) {
                return Err(invalid_parameter(format!(
                    "namespaceConfig.namespace {namespace:?} is not a Kubernetes object \
                     name (pattern {}).",
                    state::KUBERNETES_NAME_PATTERN
                )));
            }
        }
        // "The namespace configuration for the addon. If specified, this will
        // override the DEFAULT namespace for the addon": the default namespace of an
        // add-on is the one the catalog publishes for it
        // (`AddonInfo$defaultNamespace`, `DescribeAddonVersions`), so a request that
        // names no namespace installs the add-on where the catalog says it goes.
        // Every add-on this far into the handler is one the catalog publishes (see
        // LAYER 3 above), so [`ADDON_DEFAULT_NAMESPACE`] stands in only for a catalog
        // entry that somehow publishes no namespace of its own.
        let namespace = input
            .namespace_config
            .as_ref()
            .and_then(|config| config.namespace.clone())
            .or_else(|| {
                guard
                    .published_addon(&input.addon_name, now)
                    .map(|published| published.default_namespace.clone())
            })
            .unwrap_or_else(|| ADDON_DEFAULT_NAMESPACE.to_string());

        // LAYER 4 — the cluster's state. `InvalidRequestException` is "The request is
        // invalid given the state of the cluster. Check the state of the cluster and
        // the associated operations". An add-on install is ASYNCHRONOUS, so a cluster
        // Amazon EKS is still provisioning takes one — the install settles after that
        // cluster is running (see [`crate::state::AddonRecord::active_at`] and
        // [`crate::state::ComAmazonawsEksAwswesleyfrontendState::addon_install_settles_at`])
        // — and what is refused is a cluster whose state no add-on of it can outlive
        // ([`crate::state::CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN`]). It is a
        // statement about the CLUSTER, so the exception names it and the add-on that
        // could not be installed onto it.
        if !CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN.contains(&cluster_status.as_str()) {
            return Err(CreateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(state::cluster_cannot_manage_an_addon(
                        &input.cluster_name,
                        &cluster_status,
                        "take an add-on",
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // LAYER 5 — the add-on itself. One add-on name installs at most once per
        // cluster. This layer runs BEFORE the checks on the child associations below:
        // those exist to stop a request that cannot be carried out in full from
        // writing anything, which says nothing about a call whose write ALREADY
        // happened — a retry of it would otherwise collide with its own associations
        // and be answered with a conflict instead of being absorbed.
        let addon_key = (
            region.clone(),
            input.cluster_name.clone(),
            input.addon_name.clone(),
        );
        if let Some(existing) = guard.addons.get(&addon_key) {
            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
            // call and RESENDS the same value when it retries, so a retry of the call
            // that installed this add-on is absorbed and answered with that add-on
            // rather than with a conflict. A blank token identifies nothing, so it
            // never matches — absorbing on it would make two unrelated calls that both
            // sent `""` look like a retry of each other.
            //
            // The comparison is on the TOKEN alone: this does not check that the rest
            // of the retry's body matches the stored record (the same simplification
            // the cluster slice makes). An SDK retry resends the identical body, so the
            // case this would catch is a caller reusing a token for a different
            // request, which the real service refuses as an invalid parameter.
            let replayed_token = input
                .client_request_token
                .as_deref()
                .filter(|token| !token.trim().is_empty());
            if let Some(token) = replayed_token {
                if existing.client_request_token.as_deref() == Some(token) {
                    let live = guard.association_arns_owned_by(&existing.arn);
                    return Ok(CreateAddonOutput {
                        addon: Some(existing.to_addon(&live)),
                    });
                }
            }
            return Err(CreateAddonErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    // The sentence Amazon EKS answers a duplicate install with, in
                    // its entirety: the recorded error body carries this sentence
                    // and nothing else, so neither the cluster nor the add-on is
                    // reported in an identity member.
                    message: Some("Addon already exists.".to_string()),
                    ..Default::default()
                },
            )
            .into());
        }

        // What bounds the store: add-on names are caller-supplied, so without a
        // ceiling repeated creates would let one cluster retain an unbounded number of
        // records. A cluster that already holds its maximum cannot take another
        // add-on, which is a statement about the state of the cluster.
        if guard
            .addon_names_in(&region, &input.cluster_name)
            .count()
            >= MAX_ADDONS_PER_CLUSTER
        {
            return Err(CreateAddonErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(format!(
                        "Cluster {} already has the limit of {MAX_ADDONS_PER_CLUSTER} \
                         add-ons.",
                        input.cluster_name
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        // LAYER 6 — the child resources this add-on would create. The associations
        // count against the cluster's association ceiling like any others, so a
        // request that would push the cluster past it is refused BEFORE the store is
        // walked for the pairs below: the ceiling bounds both the stored side and the
        // requested side of that comparison.
        let held_associations = guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .count();
        if held_associations + requested_associations.len()
            > MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER
        {
            return Err(too_many(
                requested_associations.len(),
                Some(held_associations),
            ));
        }

        // An association the add-on would create for a `(namespace, service account)`
        // pair that already holds one would make the credential lookup that pair
        // resolves ambiguous, which is why the pod identity slice refuses a second
        // association for it; the same refusal belongs here, BEFORE anything is
        // written, so a request that cannot be carried out in full writes nothing.
        //
        // The cluster's pairs are collected in ONE walk and each requested entry is
        // looked up in that set, rather than re-walking the store per entry: the work
        // is the sum of the two bounded sides, not their product. Each taken service
        // account is kept with the association holding it — its id and the IAM role it
        // is bound to — because those are what the conflict is reported as
        // ([`crate::state::duplicate_association_refusal`]).
        let taken_service_accounts: ::std::collections::BTreeMap<&str, (&str, &str)> = guard
            .pod_identity_associations_in(&region, &input.cluster_name)
            .filter(|record| record.namespace == namespace)
            .map(|record| {
                (
                    record.service_account.as_str(),
                    (record.association_id.as_str(), record.role_arn.as_str()),
                )
            })
            .collect();
        if let Some((bound, requested_role_arn)) =
            requested_associations.iter().find_map(|association| {
                taken_service_accounts
                    .get(association.service_account.as_str())
                    .map(|bound| (*bound, association.role_arn.as_str()))
            })
        {
            let (bound_association_id, bound_role_arn) = bound;
            return Err(CreateAddonErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    message: Some(state::duplicate_association_refusal(
                        bound_association_id,
                        bound_role_arn,
                        requested_role_arn,
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let resource_id =
            state::derive_addon_resource_id(&region, &input.cluster_name, &input.addon_name);
        let addon_arn = state::derive_addon_arn(
            &region,
            &input.cluster_name,
            &input.addon_name,
            &resource_id,
        );

        // The add-on OWNS the EKS Pod Identity associations it was asked for, so each
        // one is created here as a record of its own, carrying this add-on's ARN as its
        // `ownerArn` — through [`crate::state::ComAmazonawsEksAwswesleyfrontendState::create_addon_owned_association`],
        // the one path an association an add-on owns is written by, which `UpdateAddon`
        // replaces that set through as well. `DescribePodIdentityAssociation` and
        // `ListPodIdentityAssociations` therefore find exactly the associations whose
        // ARNs the add-on reports.
        let mut association_arns = Vec::with_capacity(requested_associations.len());
        for association in &requested_associations {
            association_arns.push(guard.create_addon_owned_association(
                &region,
                &input.cluster_name,
                &namespace,
                &addon_arn,
                association,
                now,
            ));
        }

        // WHAT THE INSTALL SETTLES IN. An add-on install is asynchronous: it leaves
        // `CREATING` for the terminal state that reflects what the cluster actually
        // did with it, which is decided HERE, from the state of the cluster this
        // install runs against, and materialised when the clock reaches the due
        // instant below. The decision is made once and stored, so every later read
        // answers the same thing.
        let install_outcome = guard.addon_install_outcome(
            &region,
            &input.cluster_name,
            &input.addon_name,
            &namespace,
            now,
        );
        // WHEN IT REPORTS THAT STATE. An install that reaches the cluster is
        // asynchronous, so the add-on reports `CREATING` until the clock owes it the
        // outcome above — and an add-on applied against another add-on's resources
        // settles after that add-on does, so a chain installed in one go takes a span
        // per level. An install the service never STARTS, because a precondition of
        // the add-on itself is unmet before anything is applied to the cluster
        // ([`crate::state::AddonInstallOutcome::refused`]), has no such span: there is
        // no work under way for a later instant to finish, so the record is written in
        // the terminal state it is already in and this call answers with it.
        let (status, health_issues, active_at) = if install_outcome.settled_at_the_call {
            (
                install_outcome.status.clone(),
                install_outcome.issues.clone(),
                None,
            )
        } else {
            (
                ADDON_STATUS_CREATING.to_string(),
                // An install that has not settled yet reports nothing wrong with it:
                // the issues of the state it settles in are written onto the record
                // when that state is materialised.
                Vec::new(),
                Some(guard.addon_install_settles_at(
                    &region,
                    &input.cluster_name,
                    &input.addon_name,
                    now,
                )),
            )
        };

        let record = AddonRecord {
            arn: addon_arn,
            // The caller's version if it named one, else the default version for the
            // cluster's Kubernetes version (resolved above).
            addon_version,
            status,
            created_at: now,
            modified_at: now,
            active_at,
            // Nothing has asked to remove a freshly installed add-on.
            removed_at: None,
            service_account_role_arn: input.service_account_role_arn.clone(),
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            configuration_values: input.configuration_values.clone(),
            // The ARNs of the associations created just above, in the order the
            // request asked for them.
            pod_identity_association_arns: association_arns,
            namespace,
            client_request_token: input.client_request_token.clone(),
            cluster_name: input.cluster_name.clone(),
            addon_name: input.addon_name.clone(),
            // Nothing is in flight on a freshly installed add-on: `UpdateAddon` is
            // what puts a pending change on the record.
            pending_update: None,
            // What the add-on reports as wrong with it: nothing while an install is
            // still under way, and the issues of the state it settled in for one this
            // call has already settled (see above).
            health_issues,
            install_outcome,
            // `resolveConflicts` decides what happens to a SELF-MANAGED copy of the
            // add-on already running on the cluster; this emulator runs no Kubernetes,
            // so there is never a self-managed copy to resolve against, and the member
            // is not part of what an `Addon` reports. A value outside the modelled
            // enum never reaches here: it is refused before dispatch, once the cluster
            // has been resolved, in this service's own wording
            // (`crate::service::precedence`, `crate::service::resolve_conflicts_contract`).
        };

        // The associations this install created are already in the store, so the
        // answer reports the ones it actually holds for this add-on.
        let live = guard.association_arns_owned_by(&record.arn);
        let addon = record.to_addon(&live);
        guard.addons.insert(addon_key, record);
        // Nothing below reads the store under this guard, and the carrier started next
        // takes its own.
        drop(guard);

        // The CloudWatch agent this add-on installs collects the cluster's Container
        // Insights performance on its own interval and delivers it to CloudWatch Logs,
        // whether or not anybody calls Amazon EKS again — so the install hands that
        // collection to a task of this service's own
        // ([`state::container_insights::drive_container_insights`]). The reads that
        // materialise this add-on's own status settle the same work, so a client that
        // polls the add-on and one that only watches the log group see the same records.
        if input.addon_name == state::CONTAINER_INSIGHTS_ADDON {
            state::container_insights::drive_container_insights(
                ctx.callback(),
                &self.engine,
                &region,
            );
        }

        Ok(CreateAddonOutput {
            addon: Some(addon),
        })
    }
}
