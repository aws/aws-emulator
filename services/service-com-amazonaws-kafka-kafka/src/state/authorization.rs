// SPDX-License-Identifier: CC0-1.0

//! The resource-scoped and identity-based authorization decisions a cluster-scoped
//! operation takes BEFORE it resolves the cluster its target ARN names.
//!
//! Every operation of this service that acts on ONE cluster names that cluster by
//! ARN — in the URI (`ClusterArn` is an `@httpLabel` on all of them) or, for
//! `CreateVpcConnection` and `GetCompatibleKafkaVersions`, in the request. The
//! action it asks for therefore applies to that ARN, and whether the caller may
//! perform it is decided from the REQUEST alone: the action, the calling principal
//! and the ARN as submitted. So the decision comes first — before the handler
//! resolves the cluster, and before it validates the rest of the body — which is
//! why it lives here, next to the state the resolution then reads, rather than
//! inside any one handler.
//!
//! A resource in another ACCOUNT is the case this emulator can decide. The
//! emulator serves exactly one account ([`caller_account`]), whose single principal
//! is [`caller_principal`]; it therefore holds no cluster of any other account, and
//! no resource-based policy of one either. A target ARN whose account segment is
//! not the caller's is consequently allowed by nothing, and the answer is the
//! authorization refusal rather than the absence the registry would otherwise
//! report — "you may not touch this" and "it is not there" are different answers,
//! and a client acts on them differently.
//!
//! An ARN whose account segment IS the caller's is the owning account's own
//! request, and whether THAT request is permitted is the identity-based half of the
//! decision below. A value that is not an ARN at all
//! carries no account to decide against, so the account test passes it; whether it
//! is then refused depends on what could have granted the action.
//!
//! A value that REFERENCES a cluster and identifies none is the first case: the
//! action a cluster-scoped call asks for applies to a CLUSTER, and a permission for
//! it is scoped to the cluster resource it is granted on, so a cluster reference
//! that identifies no cluster — a `cluster` resource field of the wrong arity, one
//! carrying no id — resolves to no resource for a statement to be matched against,
//! reaches no grant, and is refused by [`no_grant_for_cluster`] before anything the
//! request names is resolved or any member of the body is read. A CONTROL-plane
//! decision was taken over no resource, so that is what it names
//! ([`ANY_RESOURCE`]); the DATA-plane one a topic call takes names the value as
//! the request's URI carried it ([`authorize_cluster_connect`]), that call having
//! read its cluster out of a percent-encoded path segment.
//!
//! A value naming a resource of ANOTHER KIND — another vendor's, or another of this
//! service's — is no cluster reference at all, so no permission scoped to a cluster
//! is ever matched against it and this gate decides nothing about it: it is left to
//! the operation, which answers the reference it cannot use, the same way the
//! configuration family reads a value naming no configuration
//! ([`crate::state::configuration_arn`]). A value that is no ARN AT ALL names no
//! resource for a statement to be matched against either, so for those actions it is
//! left to the operation's own parameter refusal, which is what the service answers
//! for a mistyped path segment.
//!
//! For the four actions a cluster's own resource-based policy GOVERNS
//! ([`CLUSTER_POLICY_ACTIONS`]) there is nothing else a grant can rest on: that
//! document lives ON the cluster, so a value that stands for no resource this
//! endpoint could hold a document on can reach no grant, and the authorization
//! refusal is the answer for a value that is no ARN too — which is why
//! `CreateVpcConnection`'s target has always been decided that way
//! ([`authorize_target_cluster`]). WHAT such a refusal names depends on whether the
//! request ADDRESSED that target: a value carried in the URI is named as it
//! arrived, and a target named in the BODY is no addressed resource, so the
//! decision over it is taken against the wildcard
//! ([`UngrantedResource::Unaddressed`]).
//!
//! That is one decision for every call whose target is a cluster, whichever plane
//! it acts on: the control-plane gate ([`authorize_caller_operation`], taken by
//! every operation that names a cluster by `ClusterArn`), the data-plane one a
//! TOPIC call takes ([`authorize_cluster_connect`]), and the readers of one cluster
//! OPERATION ([`authorize_cluster_operation_target`]), whose target is named by ARN
//! just the same.
//!
//! A cluster OPERATION differs in ONE reading, and it is the account's. The record
//! of work done on a cluster is not a resource a policy is attached to, so no
//! document over it is one an account can hold or withhold: what such a read turns
//! on is whether this endpoint HOLDS the record, and a well-formed
//! cluster-operation ARN naming none is answered as the absence it is — the
//! modelled `NotFoundException`, as this service's recorded answer for an
//! operation ARN of another account shows — rather than refused on the account the
//! ARN names ([`TargetReading::decided_by_the_targets_account`]). The caller's own
//! identity-based decision is taken for it exactly as for every other target.
//!
//! An operation that acts on the surface clients REACH a cluster over is decided
//! one step further ([`no_resource_policy_for_cluster`]). Public access and
//! multi-VPC private connectivity are the surface a cluster's own resource-based
//! policy is attached for, and the document that can grant such a call lives ON the
//! target cluster: a cluster this endpoint holds no record of carries no document at
//! all, so the call reaches no grant and the answer is the authorization refusal
//! naming the ARN as submitted, rather than a report of what this registry holds.
//! That is the recorded answer for the call that CONFIGURES that surface
//! (`UpdateConnectivity`) and for one that MANAGES a client VPC connection made over
//! it (`RejectClientVpcConnection`), and the one `CreateVpcConnection`'s target has
//! always been decided by for a value it can hold no document on
//! ([`authorize_target_cluster`]).
//!
//! A call that CHANGES or REPORTS any other part of a cluster — its storage, its
//! brokers, its monitoring, its security, its version, its own description — rests on
//! the caller's own identity-based documents, which are in force whether or not the
//! cluster exists, so an absence there is answered as the absence it is: the modelled
//! `NotFoundException` with this service's fixed sentence, which is what this
//! service's recorded answers for an absent cluster on those surfaces show. So does a
//! call that REPORTS the connections made over the connectivity surface, which
//! describes rather than reaches the cluster.
//!
//! What the CALLER's own identity-based policies say about the action is the other
//! half of the same question, and it is asked FIRST, inside
//! [`authorize_caller_action`]: AWS authorizes at the IAM layer ahead of the
//! service, so every operation that authorizes a target takes that decision before
//! this module's own — an explicit `Deny` about the request refuses it, and so does
//! IAM's default deny of a request no readable document allows. It is decided
//! in [`crate::state::identity_authorization`], which reads the IAM and STS stores
//! for the principal the request's credential speaks for, where every decision here
//! is taken from the request alone — which is why the two sit in separate modules.
//! Both word their refusal from the parts below, and both name the principal that
//! signed, so one service says one thing about an unauthorized request however it
//! was refused.
//!
//! A cluster also carries a resource-based policy OF ITS OWN
//! ([`authorize_cluster_policy`]), which the operations MSK's cluster policy
//! governs ([`CLUSTER_POLICY_ACTIONS`]) take into account once the cluster is
//! resolved, before the operation looks at anything else about the cluster or the
//! request: an explicit deny in it refuses the call whatever else allows it, and an
//! allow of the calling principal GRANTS a call that principal's own policies do
//! not — which is what a cluster policy is attached for. Those two decisions are
//! therefore taken TOGETHER there, as IAM takes them, which is why the gate holds
//! back the default deny for an action a cluster policy can grant.
//!
//! Passing a ROLE to the service ([`authorize_pass_role`]) is the same question
//! read the other way round, and it is the one case where a value that names no
//! account is refused rather than passed: there the caller must NAME a role of its
//! own account for the service to assume, so anything the account cannot be read
//! out of names no such role.
//!
//! A refusal that rests on a resource-based policy is worded in the shared IAM
//! sentence ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]),
//! so this service cannot drift from the other services that refuse an
//! unauthorized request; so are the sentences AWS words the other decisions in — a
//! request nothing grants, and one an attached document explicitly denied.
//!
//! The decision is taken by the operations whose target IS a cluster — the ones
//! whose action applies to it. The TAGGING operations take any MSK resource ARN
//! instead, and the behavioural record for a foreign-account resource there is the
//! not-found refusal (`tests/all/resource_tags.rs`, its other-account row), so they
//! deliberately do not call this module: the two answers are each their own
//! operation's, not an inconsistency to even out.

/// The IAM action namespace MSK publishes its CONTROL-plane actions under, which
/// `kafka:<ApiName>` is the action for the API of that name.
pub const CONTROL_PLANE_ACTION_NAMESPACE: &str = "kafka";

/// The IAM action a control-plane call to `operation` asks for: `kafka:<ApiName>`.
///
/// One spelling, taken by every decision this service makes about such a call —
/// the resource-scoped ones here and the identity-based one in
/// [`crate::state::identity_authorization`] — so the action a refusal names cannot
/// drift between them. The caller passes its own `Operation::NAME`, so the action
/// can never drift from the operation taking it either.
pub fn control_plane_action(operation: &str) -> String {
    format!("{CONTROL_PLANE_ACTION_NAMESPACE}:{operation}")
}

/// The IAM action a TOPIC Management call is authorized under.
///
/// The topic operations act on a cluster's Apache Kafka data plane rather than on
/// the control plane's cluster resource, and MSK publishes that plane's actions in
/// its own `kafka-cluster` namespace; reaching a cluster's topics at all is the
/// `Connect` action on the cluster, so that is the action those calls are refused
/// for whatever they then do to a topic.
pub const CLUSTER_CONNECT_ACTION: &str = "kafka-cluster:Connect";

/// The decision a TOPIC Management call takes on the cluster its `ClusterArn`
/// names, before anything resolves or parses that value.
///
/// Two gates, in AWS's order. The API call itself asks for this service's own
/// action on the cluster the request names, and reaching that cluster's Apache
/// Kafka plane asks for [`CLUSTER_CONNECT_ACTION`] on it — which is the action
/// this gate takes the decision for, since connecting to the cluster is what every
/// topic call needs whatever it then does to a topic. A permission for it is
/// scoped to the CLUSTER resource it is granted on, so the decision needs a value
/// that identifies one: a cluster REFERENCE that identifies no cluster — a
/// `cluster` resource field of the wrong arity — matches no resource-scoped
/// permission, so nothing grants the call and it is refused with the authorization
/// refusal rather than with a description of what the value looks like: the decision
/// comes first, and a caller it refuses learns nothing about what this registry
/// holds. A value naming a resource of another kind is no cluster reference at all,
/// and a value that is not an ARN carries no resource for a policy to be matched
/// against either way; both are left to the operation's own refusal, which is what
/// the service answers for a mistyped path segment.
///
/// The resource such a refusal NAMES is the submitted value as the request
/// carried it ([`http_label`]): every topic call names its cluster in a non-greedy
/// `@httpLabel` path segment, so the value this decision was taken from arrived
/// percent-encoded and the refusal quotes those bytes — which is what lets a
/// client recover the offending reference by decoding the resource clause.
///
/// `operation` is the caller's own `Operation::NAME`, so nothing about the
/// decision depends on which topic call took it beyond the cluster it named.
pub fn authorize_cluster_connect<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    cluster_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_cluster_action(
        engine,
        ctx,
        CLUSTER_CONNECT_ACTION,
        cluster_arn,
        UngrantedResource::SubmittedLabel,
    )
}

/// What a pre-resolution refusal names as the resource, for a value that
/// REFERENCES the action's own kind of resource and identifies none of it.
///
/// The two readings this service takes, which is the ONLY thing its control-plane
/// and data-plane gates differ in: both decide over the same values, through the
/// one reading in [`ungranted_resource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UngrantedResource {
    /// The resource the reference RESOLVES to in the decision's context — none, so
    /// [`ANY_RESOURCE`]: what a control-plane decision, taken after the value has
    /// been read as a reference to a resource of this service, is taken over.
    Resolved,
    /// The value as the request's URI carried it ([`http_label`]): what a TOPIC
    /// call's data-plane decision is taken from, that surface reading its cluster
    /// out of one non-greedy `@httpLabel` path segment, so the value it decided
    /// from arrived percent-encoded — which is what lets a client recover the
    /// offending reference by decoding the resource clause.
    SubmittedLabel,
    /// NO resource at all — [`ANY_RESOURCE`], whatever the submitted value was
    /// written as: what a decision over a target the request did not ADDRESS is
    /// taken over.
    ///
    /// A cluster named in the request's BODY (`CreateVpcConnection`'s
    /// `TargetClusterArn`) is not the resource the request addressed, so a value of
    /// it that resolves to no cluster leaves the action scoped to nothing and the
    /// decision is taken against the wildcard — where a target carried in the URI
    /// is the resource the request addressed, and is named as it arrived
    /// ([`Resolved`](Self::Resolved), [`SubmittedLabel`](Self::SubmittedLabel)).
    Unaddressed,
}

/// The characters a non-greedy `@httpLabel` keeps as they are: RFC 3986's
/// unreserved set (the ASCII letters and digits plus `-`, `.`, `_` and `~`).
/// Everything else — an ARN's every `:` and `/` — travels as the `%XX` of its
/// UTF-8 bytes, which is the encoding an AWS SDK applies to such a segment.
const LABEL_UNRESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `value` as the request's URI carried it in one non-greedy `@httpLabel` path
/// segment: percent-encoded over [`LABEL_UNRESERVED`], through the crate that owns
/// that encoding rather than a second encoder of this tree's own.
fn http_label(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, LABEL_UNRESERVED).to_string()
}

/// The same decision for the CONTROL-plane action of `operation` on the cluster its
/// request names: the gate every operation whose target is one cluster takes,
/// before it resolves that cluster or reads anything else of the request.
///
/// `kafka:<ApiName>` applies to a CLUSTER resource, and a permission for it is
/// scoped to the cluster it is granted on — so the decision needs a value that
/// identifies one. A cluster REFERENCE that identifies no MSK cluster (a
/// `cluster/<name>/<id>` resource field it does not have) matches no
/// resource-scoped permission of the caller's, reaches no grant, and is refused
/// with the authorization refusal rather than with a description of what the value
/// looks like: authorization is evaluated ahead of the service, so the request
/// never reaches the parameter check that would describe the target, and the caller
/// learns nothing about what this registry holds. A value naming another kind of
/// resource is no cluster reference, so this gate decides nothing about it and the
/// operation answers it.
///
/// `operation` is the caller's own `Operation::NAME`, so the action can never drift
/// from the operation taking the decision.
pub fn authorize_caller_operation<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    cluster_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_cluster_action(
        engine,
        ctx,
        &control_plane_action(operation),
        cluster_arn,
        UngrantedResource::Resolved,
    )
}

/// The same decision for an operation whose target is one cluster OPERATION,
/// named by `ClusterOperationArn`.
///
/// Same rule, same reason: `kafka:<ApiName>` applies to the resource the request
/// names, a permission for it is scoped to that resource, and a reference of THAT
/// kind identifying no cluster operation of this service matches none — so the
/// decision is the authorization refusal rather than a description of the value's
/// shape, taken before the operation is resolved. A value naming another kind of
/// resource is left to the operation, as [`authorize_named_resource`] says.
///
/// What this target's ACCOUNT says is NOT part of the decision
/// ([`TargetReading::decided_by_the_targets_account`]): a cluster operation is a
/// RECORD of work done on a cluster rather than a resource a document is attached
/// to, so there is no resource-based policy over it for an account to withhold,
/// and whether this endpoint holds the record is what the request turns on. A
/// well-formed cluster-operation ARN naming no operation of this Region is
/// therefore reported absent — the modelled `NotFoundException` the readers answer
/// — whichever account the ARN names.
pub fn authorize_cluster_operation_target<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    operation_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_named_resource(
        engine,
        ctx,
        &control_plane_action(operation),
        operation_arn,
        TargetReading {
            resource_type: crate::state::CLUSTER_OPERATION_RESOURCE_TYPE,
            names_the_resource: crate::state::is_cluster_operation_arn(operation_arn),
            only_a_resource_policy_can_grant: grantable_by_cluster_policy(
                &control_plane_action(operation),
            ),
            decided_by_the_targets_account: false,
            naming: UngrantedResource::Resolved,
        },
    )
}

/// BOTH decisions over the cluster `cluster_arn` is meant to name, for `action` (a
/// fully qualified IAM action): what the caller's own policies and the ARN's account
/// say ([`authorize_caller_action`]), and then whether the value names a cluster for
/// a resource-scoped permission to be matched against at all.
///
/// The ONE implementation behind the control-plane gate
/// ([`authorize_caller_operation`]) and the data-plane one
/// ([`authorize_cluster_connect`]), so the two cannot drift apart on which values
/// reach a grant: they pass the same `action`-derived flags through the same
/// reading and differ only in `naming`, the resource their refusal names.
fn authorize_cluster_action<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    cluster_arn: &str,
    naming: UngrantedResource,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_named_resource(
        engine,
        ctx,
        action,
        cluster_arn,
        TargetReading {
            resource_type: crate::state::CLUSTER_RESOURCE_TYPE,
            names_the_resource: crate::state::is_cluster_arn(cluster_arn),
            only_a_resource_policy_can_grant: grantable_by_cluster_policy(action),
            decided_by_the_targets_account: true,
            naming,
        },
    )
}

/// How the pre-resolution half of a decision reads the target it was given: the
/// kind of resource the action applies to, whether the submitted value names one
/// of that kind, which documents could have granted the action, and how a refusal
/// names what the decision was taken from.
///
/// One value rather than four arguments, so every gate passes the same set and a
/// new target kind cannot leave one of them out.
struct TargetReading<'reading> {
    /// The ARN resource type of the kind `action` applies to.
    resource_type: &'reading str,
    /// Whether the submitted value names a resource of that kind.
    names_the_resource: bool,
    /// Whether the only document that can grant `action` is the RESOURCE's own.
    only_a_resource_policy_can_grant: bool,
    /// Whether the target's ACCOUNT can refuse the request: whether a document
    /// over the resource it names is one this endpoint's account could hold, so
    /// that an ARN of another account reaches no resource-based grant
    /// ([`authorize_action`]). A target that is a RECORD of a resource rather than
    /// a resource a document is attached to is decided by nothing in its ARN: the
    /// endpoint either holds the record or does not, and the operation answers
    /// that.
    decided_by_the_targets_account: bool,
    /// Which reading of the submitted value a refusal quotes as the resource.
    naming: UngrantedResource,
}

/// The decision for `action` on the resource `value` is meant to name, as
/// `reading` reads that target ([`TargetReading`]).
///
/// The ONE place the order lives, for every target this service decides over: the
/// caller's own policies and the ARN's account first
/// ([`authorize_caller_action`]), then whether there is a resource for a grant to
/// rest on at all.
///
/// The decision is taken over the resource of `resource_type` the value REFERENCES,
/// so it is taken only where the value is a reference to one. A value naming a
/// resource of ANOTHER kind — another vendor's, or another of this service's — is no
/// reference to that kind at all, so no permission scoped to the action's own
/// resource is ever matched against it and the gate takes no decision: it is left
/// to the operation, which answers the reference it cannot use (the same reading
/// [`crate::state::configuration_arn`] makes for the configuration family). A value
/// that reads as a reference of the right kind and identifies no resource of it
/// reaches no grant, and is refused by [`no_grant_for_cluster`].
///
/// A value that is no ARN at all names no resource for a statement to be matched
/// against either, so where an identity-based document can grant the action it too
/// is left to the operation's own parameter refusal — which is what this service
/// answers for a mistyped path segment. Where only the resource's OWN policy can
/// grant the action ([`CLUSTER_POLICY_ACTIONS`]) there is nothing else to fall back
/// on: that document lives ON the cluster, so such a value reaches no grant and the
/// authorization refusal is the answer for it too.
fn authorize_named_resource<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    value: &str,
    reading: TargetReading<'_>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    match reading.decided_by_the_targets_account {
        true => authorize_caller_action(engine, ctx, action, value)?,
        false => authorize_calling_identity(engine, ctx, action, value)?,
    }
    if reading.names_the_resource {
        return Ok(());
    }
    match ungranted_resource(
        value,
        reading.resource_type,
        reading.only_a_resource_policy_can_grant,
    ) {
        None => Ok(()),
        // WHICH values reach a grant is decided above, for every target alike; the
        // reading below decides only how the refusal names what it decided from.
        Some(resource) => {
            let resource = match reading.naming {
                UngrantedResource::Resolved => resource.to_string(),
                UngrantedResource::SubmittedLabel => http_label(value),
                UngrantedResource::Unaddressed => ANY_RESOURCE.to_string(),
            };
            Err(no_grant_for_cluster(engine, ctx, action, &resource))
        }
    }
}

/// The resource an authorization decision over `value` is taken against when
/// `value` identifies no resource of `resource_type` — and `None` where the gate
/// takes no decision over it at all, leaving the value to the operation.
///
/// Three readings of one value, which is what makes the refusal's resource token
/// the one AWS names:
///
/// * a reference of the action's own kind that identifies no resource of it (a
///   resource field of the wrong arity, a field carrying no id) resolves to NO
///   resource in the decision's context, so the decision is taken over
///   [`ANY_RESOURCE`];
/// * an ARN naming a resource of another kind is no reference of this kind, so
///   there is nothing for the decision to be taken over and the operation answers;
/// * a value that is no ARN resolves to nothing either, so the decision — where one
///   is taken at all, which is where only the resource's own policy could have
///   granted the action — is taken over the VALUE as submitted, there being nothing
///   else it stands for.
///
/// WHAT the refusal then names is the caller's reading ([`UngrantedResource`]): a
/// target the request did not address is named by neither value.
fn ungranted_resource<'value>(
    value: &'value str,
    resource_type: &str,
    only_a_resource_policy_can_grant: bool,
) -> Option<&'value str> {
    // An ARN's resource type is read through the tree's one caller-supplied-ARN
    // parse, under this service's own namespace — which is the namespace its
    // actions are published under, so one constant says both.
    match ::service_common::arn::parse_of(value, CONTROL_PLANE_ACTION_NAMESPACE) {
        Some(parsed) if parsed.resource_type() == resource_type => Some(ANY_RESOURCE),
        Some(_) => None,
        None if ::service_common::arn::parse(value).is_some() => None,
        None => only_a_resource_policy_can_grant.then_some(value),
    }
}

/// The wire error code an authorization refusal carries.
///
/// This service's model declares no shape for it — it lists `ForbiddenException`
/// (403) and `UnauthorizedException` (401) — so the refusal leaves through
/// [`crate::scaffolded::Scaffolded::unmodeled`]: a client branches on the code, so
/// the code the real service answers is the code that goes on the wire.
pub const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The HTTP status that refusal carries, which is the status this service's own
/// 403 error models.
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// What MSK answers a `CreateReplicator` whose service-execution role belongs to
/// another account: fixed text end to end, carrying nothing from the request.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// The account every request to this emulator is served for, and therefore the
/// account a resource has to belong to for the caller to hold any policy over it.
pub fn caller_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The IAM principal every request to this emulator speaks for — the one the
/// account holds and `sts:GetCallerIdentity` reports — which is the principal an
/// authorization refusal names.
///
/// Read from the shared definition, so the principal this service refuses in the
/// name of is the same one every other service of this emulator names.
pub fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The RESOURCE-SCOPED authorization decision for `action` (a fully qualified IAM
/// action) on the resource `target_arn` names, as submitted: whether the caller
/// holds any policy over that resource, which an ARN of another account is allowed
/// by nothing.
///
/// `Ok(())` when nothing about the resource refuses the request; otherwise the
/// refusal, ready to be returned from any operation's error channel. The CALLER's
/// own identity policies are the other half of the question and are asked FIRST,
/// in [`authorize_caller_action`] — the only caller of this half, so the two
/// decisions cannot drift apart.
fn authorize_action<E>(
    action: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let Some(parsed) = ::service_common::arn::parse(target_arn) else {
        return Ok(());
    };
    if parsed.account.is_empty() || parsed.account == caller_account() {
        return Ok(());
    }
    Err(no_resource_policy_refusal(
        &caller_principal(),
        action,
        target_arn,
    ))
}

/// The ONE refusal of a request no RESOURCE-based policy allows: the code and
/// status this service answers ([`access_denied`]) carrying the sentence AWS words
/// that decision in
/// ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]).
///
/// Both decisions that reach it are built from here — the ARN of another account
/// ([`authorize_action`]) and the target no document can be attached to
/// ([`no_resource_policy_for_cluster`]) — so the two cannot drift apart on the
/// wording; which PRINCIPAL each blames stays the caller's, since the two resolve
/// it differently.
fn no_resource_policy_refusal<E>(
    principal: &str,
    action: &str,
    resource: &str,
) -> crate::scaffolded::Scaffolded<E> {
    access_denied(
        ::service_common::iam_authorization::not_authorized_by_resource_policy(
            principal, action, resource,
        ),
    )
}

/// BOTH authorization decisions, in AWS's own order, taken before the operation
/// resolves anything the request names:
///
/// 1. what the CALLING PRINCIPAL's own identity-based policies say about `action`
///    ([`crate::state::identity_authorization`]), which is the decision IAM takes
///    ahead of the service seeing the request at all, and which refuses the call
///    whatever it would otherwise have been answered: an explicit `Deny` about the
///    request, and the DEFAULT deny of a request no readable document allows;
/// 2. the resource-scoped decision above.
///
/// This is the gate an operation takes, and the ONE place that order lives.
///
/// For an action a cluster's OWN resource-based policy may grant
/// ([`CLUSTER_POLICY_ACTIONS`]) only the explicit `Deny` is decided here: that
/// document can allow the call on its own, and it cannot be read before the cluster
/// is resolved, so the rest of the identity-based decision is taken together with it
/// in [`authorize_cluster_policy`] — one combined decision, as IAM takes it.
pub fn authorize_caller_action<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_calling_identity(engine, ctx, action, target_arn)?;
    authorize_action(action, target_arn)
}

/// The FIRST of those two decisions on its own: what the CALLING PRINCIPAL's own
/// identity-based policies say about `action` on `target_arn`.
///
/// Every gate here takes it, whatever its target is, because IAM is evaluated
/// ahead of the service for every request alike. It is factored out of
/// [`authorize_caller_action`] so that a target whose ACCOUNT decides nothing
/// ([`TargetReading::decided_by_the_targets_account`]) still takes this half, and
/// so the two halves cannot be reordered by accident.
fn authorize_calling_identity<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let caller = crate::state::identity_authorization::caller_identity(engine, ctx);
    let refusal = if grantable_by_cluster_policy(action) {
        caller.explicit_deny(action, target_arn)
    } else {
        caller.refusal(action, target_arn)
    };
    match refusal {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// The resource an authorization decision is taken over when the request named
/// none this service can act on: IAM's wildcard, which is what an action with no
/// resource to scope it is decided against, and what the refusal then names.
pub const ANY_RESOURCE: &str = "*";

/// The decision for the control-plane action of `operation` on the CLUSTER
/// `target_arn` is meant to name, taken before anything resolves or parses that
/// target.
///
/// Every action this is taken for is one a cluster POLICY can grant, so it is the
/// same decision [`authorize_caller_operation`] takes, over one difference:
/// `CreateVpcConnection` names its target in the BODY rather than in a path
/// segment, so that target is not the resource the request ADDRESSED. A value of it
/// that resolves to no cluster therefore leaves the action scoped to no resource,
/// and the refusal names the wildcard rather than the submitted value
/// ([`UngrantedResource::Unaddressed`]).
pub fn authorize_target_cluster<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    target_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    authorize_cluster_action(
        engine,
        ctx,
        &control_plane_action(operation),
        target_arn,
        UngrantedResource::Unaddressed,
    )
}

/// The refusal for a request whose target value identifies no cluster of this
/// service: there is no cluster resource for a permission to be scoped to, so
/// nothing grants the action on it.
///
/// ONE sentence, naming the principal that signed, the action asked for and
/// `resource` — the resource the decision was actually taken over, which
/// [`ungranted_resource`] reads off the submitted value. It closes THERE: no clause
/// names a document, because no document was reached to withhold the grant
/// ([`::service_common::iam_authorization::not_authorized_to_perform`], the form
/// this service's own recorded refusals carry).
///
/// The principal is the one the REQUEST's credential speaks for
/// ([`crate::state::identity_authorization::calling_principal`]), so a refusal
/// blames the identity that signed the call rather than the account's own
/// published one.
fn no_grant_for_cluster<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> crate::scaffolded::Scaffolded<E> {
    let principal = crate::state::identity_authorization::calling_principal(engine, ctx);
    access_denied(
        ::service_common::iam_authorization::not_authorized_to_perform(
            &principal, action, resource,
        ),
    )
}

/// The refusal a call on the surface clients REACH a cluster over earns when this
/// endpoint holds no cluster under the ARN it named.
///
/// That surface — the cluster's public access and its multi-VPC private
/// connectivity — is the one a cluster's own resource-based policy is attached for,
/// so the grant for such a call can rest on a document that lives ON the target
/// cluster — and a cluster this service holds no record of carries no document at
/// all. There is therefore nothing for the decision to rest on: the call reaches no
/// grant, and the answer is the authorization refusal in the sentence AWS words a
/// decision no resource-based policy allowed
/// ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]),
/// naming the ARN exactly as the request submitted it — so a caller it refuses
/// learns nothing about what this registry holds.
///
/// `operation` is the caller's own `Operation::NAME`, so the action can never drift
/// from the operation taking the decision, and the principal is the one the
/// REQUEST's credential speaks for
/// ([`crate::state::identity_authorization::calling_principal`]), as in every other
/// refusal of this service.
pub fn no_resource_policy_for_cluster<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    cluster_arn: &str,
) -> crate::scaffolded::Scaffolded<E> {
    let action = control_plane_action(operation);
    let principal = crate::state::identity_authorization::calling_principal(engine, ctx);
    no_resource_policy_refusal(&principal, &action, cluster_arn)
}

/// The IAM actions an MSK CLUSTER POLICY governs — the resource-based policy
/// `PutClusterPolicy` attaches to a cluster, which Amazon MSK documents as granting
/// another account's principals multi-VPC private connectivity to it: creating a
/// VPC connection, and the two cluster readers plus the bootstrap-broker reader a
/// client needs to use one.
///
/// Spelled as API names, which is what the operations taking the decision pass as
/// their own `Operation::NAME`; `kafka:` prefixed, they are the actions a stored
/// document is evaluated for.
pub const CLUSTER_POLICY_ACTIONS: [&str; 4] = [
    "CreateVpcConnection",
    "GetBootstrapBrokers",
    "DescribeCluster",
    "DescribeClusterV2",
];

/// Whether a cluster's own resource-based policy can GRANT `action` (a fully
/// qualified IAM action) — the question that decides where the identity-based
/// default deny is taken, since a document that can allow the call on its own has
/// to be read before the absence of an identity grant refuses it.
fn grantable_by_cluster_policy(action: &str) -> bool {
    CLUSTER_POLICY_ACTIONS
        .iter()
        .any(|operation| control_plane_action(operation) == action)
}

/// The decision the cluster's OWN resource-based policy takes about the
/// control-plane action of `operation` on `cluster`, COMBINED with the caller's
/// identity-based policies, after the cluster has been resolved and before the
/// operation does anything else.
///
/// IAM's order, through the tree's one evaluation of policy documents
/// ([`::service_common::iam_authorization`]): an explicit `Deny` in either place
/// refuses the request whatever else allows it; otherwise, within one account, "the
/// resource-based policy and the identity-based policy are evaluated together, and
/// access is granted if either allows it"
/// ([`::service_common::iam_authorization::is_allowed`]), so an attached document
/// that allows the calling principal grants a caller its own policies do not — which
/// is exactly what a cluster policy is attached for — and a request neither allows
/// falls to IAM's default deny.
///
/// The principals a document may name the caller by are every spelling of the
/// calling principal ([`crate::state::identity_authorization::Caller::principals`]):
/// the session ARN and, for an assumed-role session, the role the session speaks
/// for. A DENY is decided against the whole ACCOUNT's spellings as well, because a
/// statement denying an account refuses each of its principals, where one that
/// ALLOWS an account delegates the decision to that account's own identity-based
/// policies rather than granting it here
/// ([`::service_common::iam_authorization::account_principals`]).
///
/// Taken by the operations [`CLUSTER_POLICY_ACTIONS`] names, which are the ones a
/// cluster policy governs. The work it does is bounded by the stored document
/// rather than by the request: `PutClusterPolicy` caps what one cluster may hold
/// ([`crate::state::MAX_CLUSTER_POLICY_BYTES`]), and the shared evaluation reads no
/// document longer than its own ceiling.
///
/// It is taken with the cluster already resolved, so the caller may still hold this
/// service's own guard while the IAM and STS stores are read for the principal —
/// which is the only order in which this service reaches those stores.
pub fn authorize_cluster_policy<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    cluster: &crate::state::Cluster,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let action = control_plane_action(operation);
    let caller = crate::state::identity_authorization::caller_identity(engine, ctx);
    let stated = cluster_policy_decision(&caller, &action, cluster);
    if stated == ::service_common::iam_authorization::Decision::Denied {
        return Err(access_denied(not_authorized_by_explicit_deny(
            &caller.principal(),
            &action,
            &cluster.arn,
        )));
    }
    if let Some(refusal) = caller.explicit_deny(&action, &cluster.arn) {
        return Err(refusal);
    }
    let identity = if caller.grants(&action, &cluster.arn) {
        ::service_common::iam_authorization::Decision::Allowed
    } else {
        ::service_common::iam_authorization::Decision::NotStated
    };
    if ::service_common::iam_authorization::is_allowed(identity, stated) {
        return Ok(());
    }
    Err(caller.no_grant(&action, &cluster.arn))
}

/// What the document attached to `cluster` — if it carries one — says about `action`
/// for this caller: `Denied` when a statement denies it, `Allowed` when one grants
/// it, and `NotStated` when the cluster carries no policy or none of its statements
/// is about the request.
fn cluster_policy_decision(
    caller: &crate::state::identity_authorization::Caller,
    action: &str,
    cluster: &crate::state::Cluster,
) -> ::service_common::iam_authorization::Decision {
    use ::service_common::iam_authorization as iam;
    let Some(policy) = cluster.policy.as_ref() else {
        return iam::Decision::NotStated;
    };
    let mut named: Vec<String> = caller.principals().to_vec();
    if named.is_empty() {
        named.push(caller.principal());
    }
    let mut denied_by: Vec<String> = named.clone();
    for principal in &named {
        denied_by.extend(iam::account_principals(principal));
    }
    if iam::resource_decision(
        &policy.document,
        &iam::Request::new(action, &cluster.arn, &denied_by),
    ) == iam::Decision::Denied
    {
        return iam::Decision::Denied;
    }
    iam::resource_decision(
        &policy.document,
        &iam::Request::new(action, &cluster.arn, &named),
    )
}

/// The refusal an authorization decision leaves, carrying `message`: the code and
/// status the real service answers, through the unmodeled channel this service's
/// model gives no shape for.
pub(crate) fn access_denied<E>(message: String) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(ACCESS_DENIED_CODE, ACCESS_DENIED_STATUS, message)
}

/// The sentence AWS refuses a request a resource-based policy explicitly DENIED with,
/// closed by the clause AWS names that document with: a client reading the message
/// learns that fixing its own permissions will not help. Every other wording of this
/// refusal is the shared one ([`::service_common::iam_authorization`]), so two
/// decisions that reach the same answer cannot word it differently. The principal is the one
/// the REQUEST's credential speaks for, so the refusal blames the identity that
/// signed the call.
fn not_authorized_by_explicit_deny(principal: &str, action: &str, resource: &str) -> String {
    ::service_common::iam_authorization::not_authorized_by_identity_policy(
        principal,
        action,
        resource,
        ::service_common::iam_authorization::RESOURCE_POLICY_EXPLICIT_DENY,
    )
}

/// The decision for PASSING `role_arn` to this service, as `CreateReplicator`'s
/// `ServiceExecutionRoleArn` does.
///
/// Passing a role is the caller granting the service that role's permissions, and
/// MSK accepts only a role of the caller's OWN account for it. The question is
/// answered from the ARN, through the tree's one reading of it
/// ([`::service_common::iam_role::role_name`]): the value has to name an IAM ROLE
/// of this account, and any other value — a role of another account, an ARN of some
/// other identity or service, or text no role of this account can be read out of at
/// all — names no role to pass. All of them are refused outright, with one fixed
/// sentence that mentions neither the role nor anything else of the request, before
/// anything the request asks for is created.
pub fn authorize_pass_role<E>(role_arn: &str) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    // The ONE reading of "an IAM role ARN of this account" the tree has
    // (`service_common::iam_role`), which is also what settles the forms around the
    // edges: an IAM ARN carries no Region, a role under a path is the role IAM holds
    // under that name, and an empty account field names no OTHER account.
    if ::service_common::iam_role::role_name(role_arn).is_some() {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::unmodeled(
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
        CROSS_ACCOUNT_PASS_ROLE_MESSAGE,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the actions a cluster policy can grant are exactly the published set,
    /// read as the fully qualified IAM actions the gate decides on — so the gate
    /// holds back the default deny for those and for nothing else.
    #[test]
    fn exactly_the_published_actions_are_grantable_by_a_cluster_policy() {
        for operation in CLUSTER_POLICY_ACTIONS {
            let action = control_plane_action(operation);
            assert!(
                grantable_by_cluster_policy(&action),
                "case {operation:?}: expected {action} to be grantable by a cluster policy"
            );
        }
        for operation in ["ListScramSecrets", "ListNodes", "DeleteCluster"] {
            let action = control_plane_action(operation);
            assert!(
                !grantable_by_cluster_policy(&action),
                "case {operation:?}: a cluster policy does not govern {action}"
            );
        }
        assert!(
            !grantable_by_cluster_policy(CLUSTER_CONNECT_ACTION),
            "case \"the data-plane action\": a cluster policy does not govern \
             {CLUSTER_CONNECT_ACTION}"
        );
    }

    /// RULE: the resource a pre-resolution decision is taken over follows what the
    /// submitted value REFERENCES — the wildcard for a reference of the action's own
    /// kind that identifies no resource of it, the value itself for a value nothing
    /// resolves (where only the resource's own policy could have granted the
    /// action), and no decision at all for an ARN naming a resource of another kind.
    #[test]
    fn the_resource_decided_over_follows_what_the_value_references() {
        let account = ::engine::emulator_account_id();
        let cluster = crate::state::CLUSTER_RESOURCE_TYPE;
        // `(case, value, only a resource policy can grant, the resource decided over)`.
        let rows: Vec<(&str, String, bool, Option<&str>)> = vec![
            (
                "a cluster reference without its id",
                format!("arn:aws:kafka:us-west-2:{account}:cluster/only-a-name"),
                false,
                Some(ANY_RESOURCE),
            ),
            (
                "a cluster reference with an extra segment",
                format!("arn:aws:kafka:us-west-2:{account}:cluster/a/b/c"),
                true,
                Some(ANY_RESOURCE),
            ),
            (
                "another kind of this service's resources",
                format!("arn:aws:kafka:us-west-2:{account}:configuration/a/b"),
                true,
                None,
            ),
            (
                "another vendor's resource",
                format!("arn:aws:ec2:us-west-2:{account}:instance/i-0a1b2c3d"),
                false,
                None,
            ),
            (
                "no ARN at all, where only a resource policy could grant",
                "not-an-arn".to_string(),
                true,
                Some("not-an-arn"),
            ),
            (
                "no ARN at all, where an identity policy could grant",
                "not-an-arn".to_string(),
                false,
                None,
            ),
        ];
        for (case, value, only_a_resource_policy_can_grant, expected) in rows {
            let decided = ungranted_resource(&value, cluster, only_a_resource_policy_can_grant);
            assert_eq!(
                decided, expected,
                "case {case:?}: expected the decision to be taken over {expected:?}, got \
                 {decided:?}"
            );
        }
    }
}
