// SPDX-License-Identifier: CC0-1.0

//! What this service learns from the OTHER emulated services it calls in process:
//! the endpoint of a peer, the IAM trust questions it asks, the subnets EC2
//! resolves for a cluster, and the cluster security group EC2 mints and releases.
//!
//! Every refusal built here is grounded in what the peer ANSWERED; the in-process
//! client the calls go through is `super::callback_client!`.

use super::*;

/// The endpoint the in-process callback reaches `service` in `region` at, or
/// `None` when `region` is not a Region LABEL.
///
/// The Region a request is signed for is CALLER-SUPPLIED text, and here it ends
/// up in an endpoint URL's authority rather than in a response, so it is read as
/// a Region label (`service_common::arn::region_label`, the one place this tree
/// states that grammar) instead of being interpolated as given: a value carrying
/// a host or path delimiter would name an authority the caller chose rather than
/// the peer service this emulator asks. `None` means there is no peer endpoint to
/// ask, which the two callers below answer the way they answer any unreachable
/// peer — by grounding no refusal in it.
pub(super) fn peer_endpoint(service: &str, region: &str) -> Option<String> {
    let Some(region) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!(
            peer = %service,
            "the request names no Region label, so {service} cannot be asked"
        );
        return None;
    };
    Some(format!("https://{service}.{region}.amazonaws.com"))
}

/// The refusal a role whose trust policy admits NONE of `principals` earns, or
/// `None` when IAM holds the role and its trust policy lets at least one of them
/// assume it — and also when this emulator has no IAM to ask, because a refusal
/// has to be grounded in what IAM answered rather than assumed from silence.
///
/// `principals` is every service principal AWS publishes for the purpose the
/// role is named for, because one documented purpose can have more than one:
/// admitting any of them makes the role usable, so a caller that followed any of
/// AWS's own trust policies for that role is not refused.
///
/// The trust document is read through the ONE evaluator every service that
/// resolves a caller-supplied role shares
/// (`service_common::iam_trust_policy::admits_service_principal`), so two
/// services asked about the same document cannot answer differently. A document
/// this emulator cannot read as a policy is not one Amazon EKS can act on, so it
/// does not admit the assume.
pub async fn service_principal_trust_violation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    principals: &[&str],
) -> Option<String> {
    match role_trust(ctx, region, role_arn, principals).await {
        RoleTrust::Withholds | RoleTrust::NoSuchRole => {
            Some(trust_policy_refusal(role_arn, principals))
        }
        RoleTrust::Admits | RoleTrust::Unanswered => None,
    }
}

/// What IAM answered about the role a request names and the principals that
/// would assume it.
///
/// The two cases a refusal can be grounded in are kept APART here, because the
/// operations that ask differ in which of them they refuse: a role Amazon EKS is
/// to assume itself is refused for either ([`service_principal_trust_violation`]
/// reports them in one sentence, since a caller cannot tell them apart without a
/// role it may not be allowed to read), while a refusal whose sentence is about
/// the TRUST POLICY is grounded in a trust policy IAM holds and nothing else
/// ([`pod_identity_role_trust_refusal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleTrust {
    /// IAM holds the role and its trust policy lets at least one of the
    /// principals assume it.
    Admits,
    /// IAM holds the role and its trust policy lets none of them assume it.
    Withholds,
    /// IAM holds no role of that ARN, so there is no trust policy at all.
    NoSuchRole,
    /// Nothing was answered that anything may be grounded in: the value is not an
    /// in-account role ARN (the case [`pass_role_refusal`] reports, which has
    /// already run by the time a handler asks this), or this emulator has no IAM
    /// to ask.
    Unanswered,
}

/// Ask IAM about the role `role_arn` names and the principals that would assume
/// it — the ONE reading of a role's trust relationship in this service, so two
/// operations asked about the same document cannot answer differently.
///
/// `principals` is every service principal AWS publishes for the purpose the role
/// is named for, because one documented purpose can have more than one: admitting
/// any of them makes the role usable, so a caller that followed any of AWS's own
/// trust policies for that role is not refused.
pub async fn role_trust(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    principals: &[&str],
) -> RoleTrust {
    let Some(role_name) = iam_role_name(role_arn) else {
        return RoleTrust::Unanswered;
    };
    let Some(endpoint) = peer_endpoint("iam", region) else {
        return RoleTrust::Unanswered;
    };
    let answer = callback_client!(aws_sdk_iam, ctx, endpoint, region)
        .get_role()
        .role_name(role_name)
        .send()
        .await;
    match answer {
        Ok(output) => {
            let document = output
                .role()
                .and_then(::aws_sdk_iam::types::Role::assume_role_policy_document);
            let admitted = document.is_some_and(|document| {
                principals.iter().any(|principal| {
                    ::service_common::iam_trust_policy::admits_service_principal(
                        document, principal,
                    )
                    .unwrap_or(false)
                })
            });
            if admitted {
                RoleTrust::Admits
            } else {
                RoleTrust::Withholds
            }
        }
        // IAM says this account holds no such role: an answer, and a grounded one.
        Err(error) if iam_role_is_absent(&error) => RoleTrust::NoSuchRole,
        // Any other answer is this emulator failing to ask, not IAM reporting a
        // bad role, so it grounds nothing — and it is worth a warning, because a
        // deployment with no IAM to ask is one where this gate does not run.
        Err(error) => {
            ::tracing::warn!(
                %role_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the role could not be verified against IAM"
            );
            RoleTrust::Unanswered
        }
    }
}

/// Why the `roleArn` an EKS Pod Identity association binds to a service account
/// is not a role EKS Pod Identity can assume for that account's Pods, or `None`
/// when nothing about its trust policy says so.
///
/// EKS Pod Identity "manages credentials to assume this role for applications in
/// the containers in the Pods that use this service account"
/// (`CreatePodIdentityAssociationRequest$roleArn`), and the agent that assumes it
/// speaks as [`POD_IDENTITY_SERVICE_PRINCIPAL`], so a role whose trust policy does
/// not let that principal assume it is a role no association could ever issue
/// credentials from. The reading is the service's one reading ([`role_trust`]);
/// only the sentence differs, and it is the fixed one Amazon EKS answers this
/// class with.
///
/// The refusal is grounded in a TRUST POLICY IAM HOLDS and in nothing else, which
/// is what the sentence is about: a role IAM does not hold has no trust policy to
/// have read, and neither has an IAM this emulator could not ask — so, as
/// everywhere else in this module, silence refuses nothing.
pub async fn pod_identity_role_trust_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<&'static str> {
    match role_trust(ctx, region, role_arn, POD_IDENTITY_ROLE_PRINCIPALS).await {
        RoleTrust::Withholds => Some(POD_IDENTITY_TRUST_POLICY_REFUSAL),
        RoleTrust::Admits | RoleTrust::NoSuchRole | RoleTrust::Unanswered => None,
    }
}

/// Whether IAM reports that this account holds NO role of this ARN — the second
/// question the operations that are handed a caller-supplied role ask about it.
///
/// An add-on's `serviceAccountRoleArn` is assumed by a Kubernetes service account
/// of the cluster — through the cluster's OIDC provider, or EKS Pod Identity —
/// rather than by an AWS service principal, so there is no principal to require
/// in its trust policy and existence is all there is to ask.
/// [`service_principal_trust_violation`] is the same `GetRole` call for the roles
/// whose contract DOES name one.
///
/// `false` for a value that is not an in-account role ARN at all
/// ([`pass_role_refusal`] is what reports that), and `false` when this emulator
/// has no IAM to ask: a refusal has to be grounded in what IAM answered rather
/// than assumed from silence.
pub async fn iam_holds_no_such_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    let Some(role_name) = iam_role_name(role_arn) else {
        return false;
    };
    let Some(endpoint) = peer_endpoint("iam", region) else {
        return false;
    };
    let answer = callback_client!(aws_sdk_iam, ctx, endpoint, region)
        .get_role()
        .role_name(role_name)
        .send()
        .await;
    match answer {
        Ok(_) => false,
        // IAM says this account holds no such role: the refusal is grounded.
        Err(error) if iam_role_is_absent(&error) => true,
        // Any other answer is this emulator failing to ask, not IAM reporting an
        // absent role, so it refuses nothing — and it is worth a warning, because
        // a deployment with no IAM to ask is one where this gate does not run.
        Err(error) => {
            ::tracing::warn!(
                %role_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the role could not be verified against IAM"
            );
            false
        }
    }
}

/// The message a role that IAM does not hold, or whose trust policy lets none of
/// `principals` assume it, is refused with: the two cases are one refusal,
/// because a caller cannot tell them apart without a role it may not be allowed
/// to read, and it names the role at fault.
///
/// It names the SERVICE those principals speak for rather than one principal of
/// it: the principals a purpose publishes all belong to one service, and the
/// caller's fix is to trust that service. A purpose with no principal at all is
/// not one a role can be refused FOR, so that case names no service rather than
/// leaving a gap in the sentence.
pub(super) fn trust_policy_refusal(role_arn: &str, principals: &[&str]) -> String {
    match principals.first() {
        Some(principal) => format!(
            "Nonexistent role or missing {} service principal in trust policy: {role_arn}",
            principal_service_label(principal)
        ),
        None => {
            format!("Nonexistent role or missing service principal in trust policy: {role_arn}")
        }
    }
}

/// The service a service principal speaks for.
///
/// A service principal is a host name under AWS's own domain whose last label
/// before that domain is the service: both `eks.amazonaws.com` and the
/// capability plane's `capabilities.eks.amazonaws.com` speak for `eks`.
pub(super) fn principal_service_label(principal: &str) -> &str {
    let host = principal
        .strip_suffix(".amazonaws.com")
        .unwrap_or(principal);
    host.split('.').next_back().unwrap_or(host)
}

/// The internal service that answers for a cluster REGISTRATION.
///
/// `RegisterCluster` connects a cluster that runs outside Amazon Web Services, and
/// the plane that manages those external clusters is what judges the connector
/// role: a refusal it answered reaches the caller WRAPPED, naming the service
/// behind it ([`wrapped_peer_tail`]).
pub const EXTERNAL_CLUSTER_MANAGER: &str = "EksExternalClusterManager";

/// How many attempts the tail of a wrapped refusal reports.
///
/// The clients this service calls another with are built with retries disabled
/// (`callback_client!`), so the call behind a wrapped refusal was attempted exactly
/// once; the count is a fact about that call rather than a value to pick.
pub(super) const PEER_SDK_ATTEMPT_COUNT: u32 = 1;

/// The tail AWS appends to a message it carries through from an inner `service`:
/// the service that answered, the status it answered with, the id of that call, and
/// how many attempts were made to reach it.
///
/// A caller (or its tooling) correlates the refusal with the call behind it by this
/// tail, so it is part of the message rather than an addition to it. The request id
/// is drawn afresh per call ([`new_peer_request_id`]) because that is what a request
/// id IS; nothing in the request decides it.
///
/// NOT the only wrapped form: a refusal carried through from EC2 comes back in that
/// service's own older shape — semicolon-separated, with its error code and proxy,
/// and no attempt count — which [`describe_subnets_once`] composes where it reads the
/// answer. The two are what the respective recordings show, so neither may be routed
/// through the other.
#[must_use]
pub fn wrapped_peer_tail(service: &str, status: u16) -> String {
    format!(
        " (Service: {service}, Status Code: {status}, Request ID: {}) \
         (SDK Attempt Count: {PEER_SDK_ATTEMPT_COUNT})",
        new_peer_request_id()
    )
}

/// Whether an IAM `GetRole` failure is IAM reporting that the role does not
/// exist (`NoSuchEntity`), as opposed to the call not reaching IAM.
pub(super) fn iam_role_is_absent(
    error: &::aws_sdk_iam::error::SdkError<
        ::aws_sdk_iam::operation::get_role::GetRoleError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception)
}

/// The refusal a `subnetIds` list earns when one of the subnets it names does
/// not exist, or `None` when EC2 resolved every one of them — and also when this
/// emulator has no EC2 to ask, for the same reason
/// [`service_principal_trust_violation`] refuses nothing then.
///
/// Amazon EKS places the control plane's (or a node group's) network interfaces
/// in the subnets a request names, so it resolves them before it provisions
/// anything. The subnets of the account are EC2's, so EC2 is asked: it answers
/// `InvalidSubnetID.NotFound` for an id no subnet of the account carries, and
/// the message it gives — which names that id — is carried through to the
/// caller with the service, status, error code, request id and proxy of the call
/// it came from, the way Amazon EKS reports a failure of a call it made on the
/// caller's behalf.
pub async fn unresolved_subnet_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<String> {
    resolve_subnets(ctx, region, subnet_ids).await.refusal
}

/// What EC2 answered about the subnets a request names.
///
/// One `DescribeSubnets` over the whole list answers both of the questions a create
/// asks about them: whether every id resolves, and — because Amazon EKS provisions
/// the cluster's own EC2 resources in the network those subnets sit in — which VPC
/// that is.
#[derive(Debug, Clone, Default)]
pub struct ResolvedSubnets {
    /// Why the list does not resolve, in the wrapped form Amazon EKS reports a
    /// failure of a call it made on the caller's behalf in. `None` when EC2
    /// resolved every id, and also when this emulator has no EC2 to ask.
    pub refusal: Option<String>,
    /// The VPC every subnet EC2 reported belongs to. `None` when nothing
    /// answered, and when the reported subnets do not agree on one VPC.
    pub vpc_id: Option<String>,
}

/// How many of a request's subnet ids this service resolves INDIVIDUALLY when the
/// read over the whole list is refused — the cap on the narrowing walk below, and the
/// same ceiling every other per-subnet reading of this service is held to.
///
/// The subnets one Amazon EKS resource is placed in all sit in a single VPC
/// ([`resolve_subnets`] reports a VPC only when the ids agree on one), and Amazon VPC
/// publishes 200 subnets per VPC ("Amazon VPC quotas", *Subnets per VPC*), so a list
/// naming more distinct ids than this names more subnets than one VPC can hold. The
/// walk therefore does bounded work whatever length of list a request carries, and a
/// list past the cap is answered with what EC2 said about the list as a whole.
pub const MAX_SUBNETS_RESOLVED_INDIVIDUALLY: usize = 200;

/// Resolve `subnet_ids` against EC2: the one reading of a request's subnets, which
/// [`unresolved_subnet_refusal`] is the refusal half of.
///
/// WHICH id a refusal names: a list can carry more than one id the account holds no
/// subnet for, and the one reported is the LAST of them in the order the request
/// listed them. One `DescribeSubnets` over the whole list names only one of them, so
/// when that read is refused the ids are resolved ON THEIR OWN, walking the request's
/// list from the end backwards, and the first id EC2 refuses is the one carried
/// through — that id is the last unresolvable member of the list, and the sentence
/// naming it is still EC2's own.
///
/// That walk is BOUNDED where it grows, not by the size of the request: a repeated id
/// is the same question and is asked once, at most
/// [`MAX_SUBNETS_RESOLVED_INDIVIDUALLY`] distinct ids are asked about at all, the walk
/// stops at the first refusal (so the ordinary case costs one more read), and every
/// read goes to the one client built here rather than one client per id. A list that
/// reaches the cap without a refusal is answered with EC2's own answer to the whole
/// list.
pub async fn resolve_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> ResolvedSubnets {
    if subnet_ids.is_empty() {
        return ResolvedSubnets::default();
    }
    // No Region label, so no endpoint to ask EC2 at: the same answer an
    // unreachable EC2 gets, which grounds no refusal and names no VPC.
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return ResolvedSubnets::default();
    };
    let ec2 = callback_client!(aws_sdk_ec2, ctx, endpoint, region);
    match describe_subnets_once(&ec2, subnet_ids).await {
        SubnetAnswer::Resolved(vpc_id) => ResolvedSubnets {
            refusal: None,
            vpc_id,
        },
        SubnetAnswer::Unanswered => ResolvedSubnets::default(),
        SubnetAnswer::NotFound(whole_list) => {
            // A one-id list has only one id EC2 could have been complaining about,
            // so the answer already names it.
            if subnet_ids.len() == 1 {
                return ResolvedSubnets {
                    refusal: Some(whole_list),
                    vpc_id: None,
                };
            }
            // The two halves of the bound this walk is held to: an id already asked
            // about is not asked again (it has the same answer), and the number of
            // ids asked about at all is capped. Both are counted HERE, at the loop
            // that does the work, so the length of the list a caller sends is never
            // what decides how many reads happen.
            let mut asked: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
            for subnet_id in subnet_ids.iter().rev() {
                if !asked.insert(subnet_id.as_str()) {
                    continue;
                }
                if asked.len() > MAX_SUBNETS_RESOLVED_INDIVIDUALLY {
                    break;
                }
                if let SubnetAnswer::NotFound(one) =
                    describe_subnets_once(&ec2, ::std::slice::from_ref(subnet_id)).await
                {
                    return ResolvedSubnets {
                        refusal: Some(one),
                        vpc_id: None,
                    };
                }
            }
            // Every id the walk reached resolved on its own, so either the list as a
            // whole was refused for something other than a missing id, or it is
            // longer than the walk resolves: EC2's own answer to the list is what the
            // caller is told.
            ResolvedSubnets {
                refusal: Some(whole_list),
                vpc_id: None,
            }
        }
    }
}

/// What ONE `DescribeSubnets` answered.
pub(super) enum SubnetAnswer {
    /// Every id it named resolved, and the VPC the reported subnets agree on —
    /// `None` when they do not agree on one.
    Resolved(Option<String>),
    /// EC2 refused the read because an id it named carries no subnet of the
    /// account, in the wrapped form this service reports an inner service's
    /// failure in.
    NotFound(String),
    /// Nothing EC2 reported that a refusal may be grounded in — including this
    /// emulator failing to ask it at all.
    Unanswered,
}

/// Ask `ec2` about exactly the ids in `subnet_ids`.
pub(super) async fn describe_subnets_once(ec2: &::aws_sdk_ec2::Client, subnet_ids: &[String]) -> SubnetAnswer {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let answer = ec2
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    // Every requested id resolved: EC2 reports the subnets rather than refusing.
    // A subnet belongs to exactly one VPC, so the VPC a cluster is created in is
    // the one every subnet it names agrees on; a list that spans two names no
    // single network to provision in, and none is reported.
    let error = match answer {
        Ok(output) => {
            let mut reported = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::vpc_id);
            let first = reported.next().map(str::to_string);
            let vpc_id = first.filter(|vpc_id| reported.all(|other| other == vpc_id.as_str()));
            return SubnetAnswer::Resolved(vpc_id);
        }
        Err(error) => error,
    };
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if code != EC2_INVALID_SUBNET_ID_NOT_FOUND {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the requested subnets could not be resolved against EC2"
        );
        return SubnetAnswer::Unanswered;
    }
    let status = error
        .raw_response()
        .map(|response| format!("Status Code: {}; ", response.status().as_u16()))
        .unwrap_or_default();
    let detail = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::message)
        // EC2 names the offending id in its message; with no message to carry,
        // the ids the request named are what is reported instead, so the caller
        // is still told which list did not resolve.
        .map_or_else(
            || format!("One of the subnet IDs {subnet_ids:?} does not exist"),
            str::to_string,
        );
    // The wrapped form AWS reports an inner service's failure in names the service,
    // the status and the error code it answered, the id of the call that was made
    // and the proxy it went through — there is none, so that field is `null`. The
    // tail is what a caller (or its tooling) correlates this refusal with the call
    // behind it by, so it is part of the message rather than an addition to it. It is
    // EC2's own older shape, which is why it is composed here rather than through
    // [`wrapped_peer_tail`] (see that function's note).
    let request_id = new_peer_request_id();
    SubnetAnswer::NotFound(format!(
        "{detail} (Service: AmazonEC2; {status}Error Code: {code}; \
         Request ID: {request_id}; Proxy: null)"
    ))
}

// ---------------------------------------------------------------------------
// The cluster security group slice.
//
// Creating a cluster provisions one security group of its own in the VPC the
// cluster's subnets sit in: the group every node group, add-on and
// cluster-to-node rule is authorised against. It is EC2's resource, so EC2 is
// where it is created and deleted — over the same in-process callback every
// cross-service reading above goes through — and deleting the cluster releases
// it, because the group exists only for that cluster.
// ---------------------------------------------------------------------------

/// The name prefix Amazon EKS gives the security group it creates with a cluster:
/// `eks-cluster-sg-<cluster name>-<suffix>`.
pub const CLUSTER_SECURITY_GROUP_NAME_PREFIX: &str = "eks-cluster-sg-";

/// The description that group carries, which says what it is attached to.
pub const CLUSTER_SECURITY_GROUP_DESCRIPTION: &str =
    "EKS created security group applied to ENI that is attached to EKS Control \
     Plane master nodes, as well as any managed workloads.";

/// The tag Amazon EKS stamps on the EC2 resources it owns for a cluster, naming
/// that cluster. It sits in AWS's own `aws:` tag namespace, so no caller can set
/// it — which is what makes it the handle a client finds the group by.
pub const CLUSTER_NAME_TAG: &str = "aws:eks:cluster-name";

/// The prefix of the Kubernetes cluster-ownership tag,
/// `kubernetes.io/cluster/<cluster name>`: the tag the in-cluster AWS cloud
/// provider reads to decide which resources belong to the cluster.
pub const KUBERNETES_CLUSTER_TAG_PREFIX: &str = "kubernetes.io/cluster/";

/// That tag's value on a resource the cluster itself OWNS, as opposed to one it
/// only shares.
pub const KUBERNETES_CLUSTER_OWNED: &str = "owned";

/// The tag an EC2 resource's display name is read from.
pub(super) const NAME_TAG: &str = "Name";

/// The number of decimal digits the numeric suffix of a cluster security group's
/// name carries.
pub(super) const CLUSTER_SECURITY_GROUP_SUFFIX_DIGITS: u32 = 10;

/// The name of the security group a cluster owns.
///
/// The numeric suffix is derived from the cluster's own stable resource id
/// ([`derive_cluster_resource_id`]), so it is distinct per cluster, the same on
/// every read, and never borrowed from the request. `resource_id` is therefore
/// expected to be that 32-hex-character id; a value that is not — which nothing
/// in this crate produces — draws its suffix from a zero seed rather than
/// refusing a name, because a group name is not what a create is refused over.
pub fn cluster_security_group_name(cluster_name: &str, resource_id: &str) -> String {
    // The low bound is the first number of this width, so the suffix never
    // renders narrower than the width Amazon EKS publishes it at.
    let floor = 10_u64.pow(CLUSTER_SECURITY_GROUP_SUFFIX_DIGITS - 1);
    let span = 10_u64.pow(CLUSTER_SECURITY_GROUP_SUFFIX_DIGITS) - floor;
    // 15 hex digits is the widest slice of the id that cannot overflow a `u64`.
    let seed = resource_id
        .get(..15)
        .and_then(|head| u64::from_str_radix(head, 16).ok())
        .unwrap_or_default();
    let suffix = floor + seed % span;
    format!("{CLUSTER_SECURITY_GROUP_NAME_PREFIX}{cluster_name}-{suffix}")
}

/// Mint the security group a new cluster owns, in `vpc_id`, and report its id.
///
/// `None` when EC2 did not create it — including when this emulator has no EC2 to
/// create it in, which is the same silence [`resolve_subnets`] refuses nothing on:
/// the cluster is still created, it just owns no group.
pub async fn mint_cluster_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_name: &str,
    resource_id: &str,
    vpc_id: &str,
) -> Option<String> {
    let ec2 = callback_client!(aws_sdk_ec2, ctx, peer_endpoint("ec2", region)?, region);
    let group_name = cluster_security_group_name(cluster_name, resource_id);
    let created = ec2
        .create_security_group()
        .group_name(&group_name)
        .description(CLUSTER_SECURITY_GROUP_DESCRIPTION)
        .vpc_id(vpc_id)
        .send()
        .await;
    let group_id = match created {
        Ok(output) => output.group_id,
        Err(error) => {
            ::tracing::warn!(
                %cluster_name,
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the cluster security group could not be created in EC2"
            );
            None
        }
    }?;
    // The group is tagged as the cluster's: the `aws:` key naming the cluster that
    // owns it, the Kubernetes ownership tag the in-cluster cloud provider reads,
    // and the display name the group was created under.
    let tags: Vec<::aws_sdk_ec2::types::Tag> = [
        (CLUSTER_NAME_TAG.to_string(), cluster_name.to_string()),
        (
            format!("{KUBERNETES_CLUSTER_TAG_PREFIX}{cluster_name}"),
            KUBERNETES_CLUSTER_OWNED.to_string(),
        ),
        (NAME_TAG.to_string(), group_name),
    ]
    .into_iter()
    .map(|(key, value)| {
        ::aws_sdk_ec2::types::Tag::builder()
            .key(key)
            .value(value)
            .build()
    })
    .collect();
    let tagged = ec2
        .create_tags()
        .resources(group_id.clone())
        .set_tags(Some(tags))
        .send()
        .await;
    if let Err(error) = tagged {
        // The group exists and is the cluster's, so it is reported and released
        // with the cluster either way; what a failure here costs is the tags a
        // client finds it by, which is worth a warning.
        ::tracing::warn!(
            %cluster_name,
            %group_id,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the cluster security group could not be tagged in EC2"
        );
    }
    Some(group_id)
}

/// The IPv4 CIDR blocks assigned to `vpc_id`, as EC2 reports them.
///
/// The VPC a cluster is created in is EC2's resource, so its address space is
/// read from EC2 — over the same in-process callback every other reading here
/// goes through. An empty answer is the SILENCE every reading in this module
/// treats as grounding no refusal: an emulator with no EC2 mounted, a VPC it does
/// not hold, or a read it refused says nothing about what the VPC's ranges are,
/// and a cluster is never refused on nothing.
///
/// Both the primary block and the additional associated blocks are reported: a
/// VPC's address space is every block associated with it
/// (`Vpc$cidrBlockAssociationSet`), and a service CIDR has to avoid all of them.
pub async fn resolve_vpc_cidr_blocks(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Vec<String> {
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return Vec::new();
    };
    let described = callback_client!(aws_sdk_ec2, ctx, endpoint, region)
        .describe_vpcs()
        .vpc_ids(vpc_id)
        .send()
        .await;
    let output = match described {
        Ok(output) => output,
        Err(error) => {
            ::tracing::warn!(
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the VPC's CIDR blocks could not be read from EC2"
            );
            return Vec::new();
        }
    };
    let mut blocks: Vec<String> = Vec::new();
    let mut seen: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for vpc in output.vpcs() {
        // Only the IPv4 associations are read: the rule this answers is the IPv4
        // service CIDR's, and a VPC's IPv6 ranges (`ipv6CidrBlockAssociationSet`)
        // cannot overlap an IPv4 block.
        for block in vpc
            .cidr_block()
            .into_iter()
            .map(str::to_string)
            .chain(
                vpc.cidr_block_association_set()
                    .iter()
                    .filter_map(|association| association.cidr_block().map(str::to_string)),
            )
        {
            if seen.insert(block.clone()) {
                blocks.push(block);
            }
        }
    }
    blocks
}

/// Release the security group a deleted cluster owned.
///
/// A group this emulator cannot delete is reported rather than refused: the
/// cluster is already gone from this service's store, and `DeleteCluster` has no
/// failure to answer the caller with for a resource of another service.
pub async fn release_cluster_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_id: &str,
) {
    // No Region label, so no endpoint to reach EC2 at: the same silence an
    // unreachable EC2 leaves, which a deleted cluster is not refused over.
    let Some(endpoint) = peer_endpoint("ec2", region) else {
        return;
    };
    let released = callback_client!(aws_sdk_ec2, ctx, endpoint, region)
        .delete_security_group()
        .group_id(group_id)
        .send()
        .await;
    if let Err(error) = released {
        ::tracing::warn!(
            %group_id,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the cluster security group could not be deleted from EC2"
        );
    }
}
