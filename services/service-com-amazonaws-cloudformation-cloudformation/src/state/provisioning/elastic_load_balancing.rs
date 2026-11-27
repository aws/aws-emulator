// SPDX-License-Identifier: CC0-1.0

//! The Elastic Load Balancing v2 resources a stack declares — the load balancer, the target
//! group traffic is routed to, the listener that accepts connections on the load balancer, the
//! rule that routes them and the trust store its listeners authenticate client certificates
//! against — created in Elastic Load Balancing itself.
//!
//! This is the per-type half of provisioning for five resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::lambda_function`] for the reason that module records —
//! that file is past the length a change to this repository may touch — and is reached
//! through the same seam ([`super::Owner`]).
//!
//! ## Why they are really created
//!
//! A stack that declares a load balancer exists so that the load balancer can be USED: the
//! ARN the stack publishes is the handle every Elastic Load Balancing read and write takes,
//! and the whole point of deploying the template is that `DescribeLoadBalancers`,
//! `DescribeTargetGroups`, `DescribeListeners` and `DescribeRules` answer for what the stack
//! says it created. So each type's create is the owning service's own call —
//! `CreateLoadBalancer`, `CreateTargetGroup`, `CreateListener`, `CreateRule` — and a
//! declaration that service REFUSES (a subnet Amazon EC2 holds none of, a name already in
//! use, a target group no load balancer may take) fails the resource, and with it the stack,
//! rather than being reported as deployed.
//!
//! ## The identity of each
//!
//! Every one of the four is identified by an ARN carrying a 16-character identifier Elastic
//! Load Balancing MINTS (`loadbalancer/app/<name>/<id>`, `targetgroup/<name>/<id>`,
//! `listener/app/<name>/<id>/<id>`), which no naming rule could compute, and each type's
//! `Ref` returns that ARN. So the identity in force is the one the create answered
//! ([`super::super::template::identity_is_service_assigned`] names all four), and the ARN
//! CloudFormation's naming rules compute is the stand-in a declaration is resolved against
//! until the create has answered — which the deployment then re-points, exactly as it
//! re-points a `vpc-` id.
//!
//! ## The attributes
//!
//! What each type publishes through `Fn::GetAtt` is read off the answer of the create that
//! made the resource rather than computed here: a load balancer's `DNSName`,
//! `CanonicalHostedZoneID`, `LoadBalancerName`, `LoadBalancerFullName` and `SecurityGroups`
//! are values Elastic Load Balancing decides — the hosted zone of the Region and the family,
//! the host it publishes, the default security group of the VPC an Application Load Balancer
//! with no declared groups is given — and a target group's `LoadBalancerArns` is the load
//! balancer that service has routing to it. So an output over any of them carries the value
//! the resource really has.
//!
//! ## The properties
//!
//! Each type's properties are the members of its own create under the same names, which is
//! what [`LOAD_BALANCER_MEMBERS`] and its siblings carry — with the exceptions each
//! catalogue's own comment records: a name CloudFormation decides, a tag set that carries the
//! stack's tags beside the declaration's, and the properties that are not create members at
//! all but a second call of the owning service (`LoadBalancerAttributes` is
//! `ModifyLoadBalancerAttributes`, `TargetGroupAttributes` is `ModifyTargetGroupAttributes`,
//! `Targets` is `RegisterTargets`, `ListenerAttributes` is `ModifyListenerAttributes`).

use std::collections::BTreeMap;

use serde_json::Value;

use super::{
    call_query, xml_element, xml_member_list, xml_member_structures, xml_scalar_children, Created,
    Readiness, Refusal, Removal,
};
use crate::state::provisioner::{
    arn_resource, declared_name, push_list, push_structure, push_tag_list, query_members, tags_of,
    QueryMember,
};
use crate::state::StoredResource;

/// The resource types this module provisions. Visible to the rest of this service's state
/// because more than one module answers about them
/// ([`crate::state::template::identity_is_service_assigned`] names all five).
pub(in crate::state) use crate::state::template::{
    ELB_LISTENER, ELB_LISTENER_RULE, ELB_LOAD_BALANCER, ELB_TARGET_GROUP, ELB_TRUST_STORE,
};

/// Elastic Load Balancing's signing name — which is what routes the in-process call to its
/// own handlers — and the API version an awsQuery request names beside the action, from that
/// service's own model (`sigv4Name: elasticloadbalancing`, `version: 2015-12-01`,
/// `smithy_query(query="service_info")`). It speaks `awsQuery`.
const ELB_SIGNING_NAME: &str = "elasticloadbalancing";
/// See [`ELB_SIGNING_NAME`].
const ELB_API_VERSION: &str = "2015-12-01";

/// The codes Elastic Load Balancing refuses a call naming a resource it does not hold with
/// (`LoadBalancerNotFound`, `TargetGroupNotFound`, `ListenerNotFound`, `RuleNotFound` and
/// `TrustStoreNotFound`, each the `awsQueryError` of the type's own modelled not-found
/// exception): a DELETE answered with one of them is asking for the state that service is
/// already in.
///
/// Judged on the CODE, never on a message, which carries the request's own ARN.
const NOT_FOUND_CODES: &[&str] = &[
    "LoadBalancerNotFound",
    "TargetGroupNotFound",
    LISTENER_NOT_FOUND,
    "RuleNotFound",
    TRUST_STORE_NOT_FOUND,
];

/// The code Elastic Load Balancing refuses a call naming a TRUST STORE it does not hold with:
/// the `awsQueryError` of `TrustStoreNotFoundException`, which every call that addresses a
/// trust store by ARN answers — the delete and the read of one included.
const TRUST_STORE_NOT_FOUND: &str = "TrustStoreNotFound";

/// The code Elastic Load Balancing refuses a call naming a LISTENER it does not hold with: the
/// `awsQueryError` of `ListenerNotFoundException`, which every call that addresses a listener
/// answers — including the ones that act on a listener's CERTIFICATE LIST, where it is what says
/// the list went with the listener ([`super::elbv2_listener_certificate`]).
pub(super) const LISTENER_NOT_FOUND: &str = "ListenerNotFound";

/// The state code a load balancer is ready in (`LoadBalancerState$Code`: "the load balancer
/// is ready"), which is what ends the wait its create opens.
const LOAD_BALANCER_ACTIVE: &str = "active";

/// The state code a load balancer that is still being brought up carries
/// (`LoadBalancerState$Code`: "the load balancer is being set up").
const LOAD_BALANCER_PROVISIONING: &str = "provisioning";

/// The two statuses a trust store is reported in (`TrustStoreStatus`, whose enum values are
/// exactly these): a trust store Elastic Load Balancing is still building the certificate set
/// of, and the one it reports ready.
const TRUST_STORE_ACTIVE: &str = "ACTIVE";
/// See [`TRUST_STORE_ACTIVE`].
const TRUST_STORE_CREATING: &str = "CREATING";

/// Which of this module's types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::ElasticLoadBalancingV2::LoadBalancer`.
    LoadBalancer,
    /// `AWS::ElasticLoadBalancingV2::TargetGroup`.
    TargetGroup,
    /// `AWS::ElasticLoadBalancingV2::Listener`.
    Listener,
    /// `AWS::ElasticLoadBalancingV2::ListenerRule`.
    ListenerRule,
    /// `AWS::ElasticLoadBalancingV2::TrustStore`.
    TrustStore,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        ELB_LOAD_BALANCER => Some(Kind::LoadBalancer),
        ELB_TARGET_GROUP => Some(Kind::TargetGroup),
        ELB_LISTENER => Some(Kind::Listener),
        ELB_LISTENER_RULE => Some(Kind::ListenerRule),
        ELB_TRUST_STORE => Some(Kind::TrustStore),
        _ => None,
    }
}

/// The members of `CreateLoadBalancer` an `AWS::ElasticLoadBalancingV2::LoadBalancer`
/// declares under the same names, each list's element name being the Smithy default
/// (`member`), which every list of this model carries.
///
/// Source: `CreateLoadBalancer`'s member list
/// (`smithy_query(query="operation",operation="CreateLoadBalancer")`), intersected with the
/// properties that type publishes.
const LOAD_BALANCER_MEMBERS: &[(&str, QueryMember)] = &[
    ("EnablePrefixForIpv6SourceNat", QueryMember::Scalar),
    ("IpAddressType", QueryMember::Scalar),
    ("IpamPools", QueryMember::Structure),
    ("Scheme", QueryMember::Scalar),
    ("SecurityGroups", QueryMember::List("member")),
    ("SubnetMappings", QueryMember::List("member")),
    ("Subnets", QueryMember::List("member")),
    ("Type", QueryMember::Scalar),
];

/// The `AWS::ElasticLoadBalancingV2::LoadBalancer` properties the walk does NOT turn into a
/// `CreateLoadBalancer` member, each for a reason of its own. Every other property the
/// catalogue does not carry is refused rather than dropped ([`query_members`]).
const LOAD_BALANCER_PROPERTIES_NOT_SENT: &[&str] = &[
    // The name CloudFormation decides, which the arm sets from the resource's own record, so
    // the walk must not also push the declared spelling of it.
    "Name",
    // The tag list carries the declared tags AND the ones CloudFormation propagates, so the
    // arm builds it out of both ([`tags_of`]).
    "Tags",
    // Not a member of the create at all: the load balancer's attributes are
    // `ModifyLoadBalancerAttributes`, which the arm calls once the load balancer exists.
    "LoadBalancerAttributes",
];

/// The members of `CreateTargetGroup` an `AWS::ElasticLoadBalancingV2::TargetGroup` declares
/// under the same names.
///
/// Source: `CreateTargetGroup`'s member list, intersected with the properties that type
/// publishes.
const TARGET_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("HealthCheckEnabled", QueryMember::Scalar),
    ("HealthCheckIntervalSeconds", QueryMember::Scalar),
    ("HealthCheckPath", QueryMember::Scalar),
    ("HealthCheckPort", QueryMember::Scalar),
    ("HealthCheckProtocol", QueryMember::Scalar),
    ("HealthCheckTimeoutSeconds", QueryMember::Scalar),
    ("HealthyThresholdCount", QueryMember::Scalar),
    ("IpAddressType", QueryMember::Scalar),
    ("Matcher", QueryMember::Structure),
    ("Port", QueryMember::Scalar),
    ("Protocol", QueryMember::Scalar),
    ("ProtocolVersion", QueryMember::Scalar),
    ("TargetType", QueryMember::Scalar),
    ("UnhealthyThresholdCount", QueryMember::Scalar),
    ("VpcId", QueryMember::Scalar),
];

/// The `AWS::ElasticLoadBalancingV2::TargetGroup` properties the walk does not send, for the
/// reasons [`LOAD_BALANCER_PROPERTIES_NOT_SENT`] records for the load balancer's own: the
/// name CloudFormation decides, the merged tag set, and the two properties that are calls of
/// their own (`ModifyTargetGroupAttributes` and `RegisterTargets`).
const TARGET_GROUP_PROPERTIES_NOT_SENT: &[&str] =
    &["Name", "Tags", "TargetGroupAttributes", "Targets"];

/// The members of `CreateListener` an `AWS::ElasticLoadBalancingV2::Listener` declares under
/// the same names — `LoadBalancerArn` among them, which is the load balancer the listener is
/// created on and which the declaration itself states.
///
/// Source: `CreateListener`'s member list, intersected with the properties that type
/// publishes.
const LISTENER_MEMBERS: &[(&str, QueryMember)] = &[
    ("AlpnPolicy", QueryMember::List("member")),
    ("Certificates", QueryMember::List("member")),
    ("DefaultActions", QueryMember::List("member")),
    ("LoadBalancerArn", QueryMember::Scalar),
    ("MutualAuthentication", QueryMember::Structure),
    ("Port", QueryMember::Scalar),
    ("Protocol", QueryMember::Scalar),
    ("SslPolicy", QueryMember::Scalar),
];

/// The `AWS::ElasticLoadBalancingV2::Listener` properties the walk does not send: the merged
/// tag set, and the attributes that are `ModifyListenerAttributes`. A listener declares no
/// name at all.
const LISTENER_PROPERTIES_NOT_SENT: &[&str] = &["Tags", "ListenerAttributes"];

/// The members of `CreateRule` an `AWS::ElasticLoadBalancingV2::ListenerRule` declares under
/// the same names.
///
/// Source: `CreateRule`'s member list, intersected with the properties that type publishes.
const LISTENER_RULE_MEMBERS: &[(&str, QueryMember)] = &[
    ("Actions", QueryMember::List("member")),
    ("Conditions", QueryMember::List("member")),
    ("ListenerArn", QueryMember::Scalar),
    ("Priority", QueryMember::Scalar),
];

/// The `AWS::ElasticLoadBalancingV2::ListenerRule` properties the walk does not send: the
/// merged tag set. A rule declares no name.
const LISTENER_RULE_PROPERTIES_NOT_SENT: &[&str] = &["Tags"];

/// The members of `CreateTrustStore` an `AWS::ElasticLoadBalancingV2::TrustStore` declares
/// under the same names: the Amazon S3 object its CA certificate bundle is read from, which
/// is what that create counts the certificates of.
///
/// Source: `CreateTrustStore`'s member list
/// (`smithy_query(query="operation",operation="CreateTrustStore")`), intersected with the
/// properties that type publishes. `ModifyTrustStore` takes the same three
/// ([`update_trust_store`]).
const TRUST_STORE_MEMBERS: &[(&str, QueryMember)] = &[
    ("CaCertificatesBundleS3Bucket", QueryMember::Scalar),
    ("CaCertificatesBundleS3Key", QueryMember::Scalar),
    ("CaCertificatesBundleS3ObjectVersion", QueryMember::Scalar),
];

/// The `AWS::ElasticLoadBalancingV2::TrustStore` properties the walk does not send, for the
/// reasons [`LOAD_BALANCER_PROPERTIES_NOT_SENT`] records for the load balancer's own: the name
/// CloudFormation decides, and the merged tag set.
const TRUST_STORE_PROPERTIES_NOT_SENT: &[&str] = &["Name", "Tags"];

/// The properties of each type that cannot change on the resource that EXISTS: a declaration
/// that changes one asks for a resource of a different identity, and Elastic Load Balancing
/// publishes no call that changes it — a load balancer's name, scheme and family, a target
/// group's name and the traffic it takes, the load balancer a listener is on and the listener
/// a rule is on.
///
/// A deployment that changes one of these is reported as NOT applied ([`update`] answers
/// `None`) rather than as a success that dropped the caller's change.
const CREATE_ONLY_PROPERTIES: &[(&str, &[&str])] = &[
    (ELB_LOAD_BALANCER, &["Name", "Scheme", "Type"]),
    (
        ELB_TARGET_GROUP,
        &[
            "IpAddressType",
            "Name",
            "Port",
            "Protocol",
            "ProtocolVersion",
            "TargetType",
            "VpcId",
        ],
    ),
    (ELB_LISTENER, &["LoadBalancerArn"]),
    (ELB_LISTENER_RULE, &["ListenerArn"]),
    // A trust store's `Name` is documented "This name must be unique per region and can't be
    // changed after creation" (`CreateTrustStore$Name`), and `ModifyTrustStore` takes no name
    // at all.
    (ELB_TRUST_STORE, &["Name"]),
];

/// Create the resource one of these types declares, in Elastic Load Balancing itself — so a
/// client that deployed the stack reads the load balancer back with `DescribeLoadBalancers`,
/// its listeners with `DescribeListeners`, the target group with `DescribeTargetGroups`, the
/// rule with `DescribeRules` and the trust store with `DescribeTrustStores` and
/// `DescribeTrustStoreAssociations`, every one by the ARN the stack published.
///
/// Every one of these types publishes a `Tags` property, and every one of their creates
/// takes that list as its own member, so the resource carries the tags CloudFormation gives
/// it beside the ones its declaration asks for.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::LoadBalancer => create_load_balancer(ctx, region, resource, propagated).await,
        Kind::TargetGroup => create_target_group(ctx, region, resource, propagated).await,
        Kind::Listener => create_listener(ctx, region, resource, propagated).await,
        Kind::ListenerRule => create_rule(ctx, region, resource, propagated).await,
        Kind::TrustStore => create_trust_store(ctx, region, resource, propagated).await,
    }
}

/// Create the load balancer an `AWS::ElasticLoadBalancingV2::LoadBalancer` declares, and
/// report the identity and the attributes Elastic Load Balancing answered for it.
async fn create_load_balancer(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        LOAD_BALANCER_MEMBERS,
        &[],
        LOAD_BALANCER_PROPERTIES_NOT_SENT,
        ELB_LOAD_BALANCER,
    )?;
    // `CreateLoadBalancer$Name` is required by that call while the type's own `Name` is
    // optional, so the load balancer is created under the name the stack decided.
    parameters.push(("Name".to_string(), declared_name(resource)));
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_elb(ctx, region, "CreateLoadBalancer", &parameters).await?;
    let arn = answered_arn(&answered, "LoadBalancerArn", ELB_LOAD_BALANCER)?;
    apply_attributes(
        ctx,
        region,
        "ModifyLoadBalancerAttributes",
        "LoadBalancerArn",
        &arn,
        resource.properties.get("LoadBalancerAttributes"),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(arn.clone()),
        attributes: load_balancer_attributes(&answered, &arn),
    })
}

/// Create the target group an `AWS::ElasticLoadBalancingV2::TargetGroup` declares, apply the
/// attributes its declaration asks for and register the targets it names.
async fn create_target_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        TARGET_GROUP_MEMBERS,
        &[],
        TARGET_GROUP_PROPERTIES_NOT_SENT,
        ELB_TARGET_GROUP,
    )?;
    parameters.push(("Name".to_string(), declared_name(resource)));
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_elb(ctx, region, "CreateTargetGroup", &parameters).await?;
    let arn = answered_arn(&answered, "TargetGroupArn", ELB_TARGET_GROUP)?;
    apply_attributes(
        ctx,
        region,
        "ModifyTargetGroupAttributes",
        "TargetGroupArn",
        &arn,
        resource.properties.get("TargetGroupAttributes"),
    )
    .await?;
    register_targets(ctx, region, &arn, resource.properties.get("Targets")).await?;
    Ok(Created {
        physical_resource_id: Some(arn.clone()),
        attributes: target_group_attributes(&answered, &arn),
    })
}

/// Create the listener an `AWS::ElasticLoadBalancingV2::Listener` declares, on the load
/// balancer its declaration names, and apply the listener attributes it asks for.
async fn create_listener(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        LISTENER_MEMBERS,
        &[],
        LISTENER_PROPERTIES_NOT_SENT,
        ELB_LISTENER,
    )?;
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_elb(ctx, region, "CreateListener", &parameters).await?;
    let arn = answered_arn(&answered, "ListenerArn", ELB_LISTENER)?;
    apply_attributes(
        ctx,
        region,
        "ModifyListenerAttributes",
        "ListenerArn",
        &arn,
        resource.properties.get("ListenerAttributes"),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(arn.clone()),
        attributes: arn_only_attributes(ELB_LISTENER, &arn),
    })
}

/// Create the rule an `AWS::ElasticLoadBalancingV2::ListenerRule` declares, on the listener
/// its declaration names.
async fn create_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        LISTENER_RULE_MEMBERS,
        &[],
        LISTENER_RULE_PROPERTIES_NOT_SENT,
        ELB_LISTENER_RULE,
    )?;
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_elb(ctx, region, "CreateRule", &parameters).await?;
    let arn = answered_arn(&answered, "RuleArn", ELB_LISTENER_RULE)?;
    let mut attributes = arn_only_attributes(ELB_LISTENER_RULE, &arn);
    // The attribute the type publishes is `RuleArn` — "the ARN of the listener rule" — which
    // the uniform `<Type>Arn` convention spells `ListenerRuleArn` for this type, so it is
    // re-pointed by name here beside the conventional pair.
    attributes.insert("RuleArn".to_string(), arn.clone());
    // "IsDefault: returns true if the rule is a default rule" — the rule's own, as that
    // service reports it, rather than a value decided here.
    if let Some(is_default) = reported(&answered, "IsDefault") {
        attributes.insert("IsDefault".to_string(), is_default);
    }
    Ok(Created {
        physical_resource_id: Some(arn.clone()),
        attributes,
    })
}

/// Create the trust store an `AWS::ElasticLoadBalancingV2::TrustStore` declares, from the CA
/// certificate bundle its declaration names in Amazon S3.
///
/// The ARN that create answers is the resource's identity, which is what a listener's
/// `MutualAuthentication.TrustStoreArn` resolves to — so the trust store a stack's mTLS
/// listener authenticates against is one Elastic Load Balancing really holds, and
/// `DescribeTrustStores`, `DescribeTrustStoreAssociations` and `DeleteTrustStore` all answer
/// for it.
async fn create_trust_store(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        TRUST_STORE_MEMBERS,
        &[],
        TRUST_STORE_PROPERTIES_NOT_SENT,
        ELB_TRUST_STORE,
    )?;
    // `CreateTrustStore$Name` is required by that call while the type's own `Name` is
    // optional, so the trust store is created under the name the stack decided.
    parameters.push(("Name".to_string(), declared_name(resource)));
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_elb(ctx, region, "CreateTrustStore", &parameters).await?;
    let arn = answered_arn(&answered, "TrustStoreArn", ELB_TRUST_STORE)?;
    Ok(Created {
        physical_resource_id: Some(arn.clone()),
        attributes: trust_store_attributes(&answered, &arn),
    })
}

/// Apply the attribute list a declaration asks for through the owning service's own
/// `Modify…Attributes` call, which is the only way those settings reach a resource: they are
/// no member of any create.
///
/// A declaration that asks for none leaves the resource with the attributes that service
/// gives it, so nothing is called at all.
async fn apply_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    action: &str,
    identity_member: &str,
    arn: &str,
    declared: Option<&Value>,
) -> Result<(), Refusal> {
    let Some(declared) = declared.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let mut parameters = vec![(identity_member.to_string(), arn.to_string())];
    push_list(&mut parameters, "Attributes", "member", declared, 0)?;
    call_elb(ctx, region, action, &parameters).await.map(|_| ())
}

/// Register the targets a target group's declaration names, which is `RegisterTargets` — the
/// one call that puts a target in a group, and no member of the create.
async fn register_targets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    declared: Option<&Value>,
) -> Result<(), Refusal> {
    let Some(declared) = declared.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let mut parameters = vec![("TargetGroupArn".to_string(), arn.to_string())];
    push_list(&mut parameters, "Targets", "member", declared, 0)?;
    call_elb(ctx, region, "RegisterTargets", &parameters)
        .await
        .map(|_| ())
}

/// Whether the owning service reports the created resource ready.
///
/// `CreateLoadBalancer` answers the load balancer [`LOAD_BALANCER_PROVISIONING`] — "the load
/// balancer is being set up" — and a load balancer in that state serves no connection, so the
/// resource's creation is held open until Elastic Load Balancing reports it
/// [`LOAD_BALANCER_ACTIVE`], "the load balancer is ready". `CreateTrustStore` is the same case:
/// it answers the trust store [`TRUST_STORE_CREATING`], and a listener authenticates no client
/// certificate against a trust store whose certificate set that service is still building, so
/// the wait ends when it reports [`TRUST_STORE_ACTIVE`]. A target group, a listener and a rule
/// each EXIST the moment their own create answers: that service reports no state one of them is
/// still coming up in.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    match kind {
        Kind::LoadBalancer => load_balancer_readiness(ctx, region, physical_resource_id).await,
        Kind::TrustStore => trust_store_readiness(ctx, region, physical_resource_id).await,
        Kind::TargetGroup | Kind::Listener | Kind::ListenerRule => Ok(Readiness::Ready),
    }
}

/// [`readiness`] for a load balancer, whose state Elastic Load Balancing reports as it brings
/// it up.
async fn load_balancer_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(state) = load_balancer_state(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Elastic Load Balancing holds no load balancer for {physical_resource_id}"
        )));
    };
    match state.as_str() {
        LOAD_BALANCER_ACTIVE => Ok(Readiness::Ready),
        LOAD_BALANCER_PROVISIONING => Ok(Readiness::Pending(state)),
        other => Ok(Readiness::Failed(format!(
            "Elastic Load Balancing reports load balancer {physical_resource_id} in state \
             {other}"
        ))),
    }
}

/// [`readiness`] for a trust store, whose `Status` that service reports while it builds the
/// certificate set the bundle holds.
async fn trust_store_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(status) = trust_store_status(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Elastic Load Balancing holds no trust store for {physical_resource_id}"
        )));
    };
    match status.as_str() {
        TRUST_STORE_ACTIVE => Ok(Readiness::Ready),
        TRUST_STORE_CREATING => Ok(Readiness::Pending(status)),
        other => Ok(Readiness::Failed(format!(
            "Elastic Load Balancing reports trust store {physical_resource_id} in status \
             {other}"
        ))),
    }
}

/// The attributes a load balancer or a trust store publishes now that Elastic Load Balancing
/// reports it up.
///
/// Only those two are waited for ([`readiness`]), and what changes while one comes up is what
/// that service fills in as it provisions the resource — the load balancer's published host and
/// hosted zone, the trust store's status — so the type's attributes are read off it again once
/// it reports it ready. A resource that service no longer holds carries none rather than an
/// invented value.
pub(super) async fn ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    match kind {
        Kind::LoadBalancer => match describe_load_balancer(ctx, region, physical_resource_id)
            .await?
        {
            Some(described) => Ok(load_balancer_attributes(&described, physical_resource_id)),
            None => Ok(BTreeMap::new()),
        },
        Kind::TrustStore => {
            match describe_one(ctx, region, Kind::TrustStore, physical_resource_id).await? {
                Some(described) => Ok(trust_store_attributes(&described, physical_resource_id)),
                None => Ok(BTreeMap::new()),
            }
        }
        Kind::TargetGroup | Kind::Listener | Kind::ListenerRule => Ok(BTreeMap::new()),
    }
}

/// Apply a CHANGED declaration to the resource the stack already has, through the owning
/// service's own calls: the attribute writes for a load balancer
/// (`ModifyLoadBalancerAttributes`, `SetSubnets`, `SetSecurityGroups`, `SetIpAddressType`),
/// `ModifyTargetGroup` and `ModifyTargetGroupAttributes` for a target group, `ModifyListener`
/// and `ModifyListenerAttributes` for a listener, and `ModifyRule` for a rule.
///
/// `Ok(None)` is a change none of those calls can make: a property of
/// [`CREATE_ONLY_PROPERTIES`] is part of the resource's identity, so the deployment reports
/// the change as not applied rather than as a success that dropped it.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if create_only_property_changed(&resource.resource_type, resource, declared) {
        return Ok(None);
    }
    let arn = resource.physical_resource_id.clone();
    match kind {
        Kind::LoadBalancer => update_load_balancer(ctx, region, &arn, resource, declared).await,
        Kind::TargetGroup => update_target_group(ctx, region, &arn, declared).await,
        Kind::Listener => update_listener(ctx, region, &arn, declared).await,
        Kind::ListenerRule => update_rule(ctx, region, &arn, declared).await,
        Kind::TrustStore => update_trust_store(ctx, region, &arn, declared).await,
    }
}

/// Apply a changed load balancer declaration: the placement, the groups and the address type
/// each have a `Set…` call of their own, and the attributes their own modify.
async fn update_load_balancer(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if changed(resource, declared, "Subnets") || changed(resource, declared, "SubnetMappings") {
        let mut parameters = vec![("LoadBalancerArn".to_string(), arn.to_string())];
        for member in ["Subnets", "SubnetMappings"] {
            if let Some(value) = declared.get(member).filter(|value| !value.is_null()) {
                push_list(&mut parameters, member, "member", value, 0)?;
            }
        }
        call_elb(ctx, region, "SetSubnets", &parameters).await?;
    }
    if changed(resource, declared, "SecurityGroups") {
        let mut parameters = vec![("LoadBalancerArn".to_string(), arn.to_string())];
        if let Some(value) = declared.get("SecurityGroups") {
            push_list(&mut parameters, "SecurityGroups", "member", value, 0)?;
        }
        call_elb(ctx, region, "SetSecurityGroups", &parameters).await?;
    }
    if changed(resource, declared, "IpAddressType") {
        if let Some(value) = declared.get("IpAddressType").and_then(Value::as_str) {
            let parameters = vec![
                ("LoadBalancerArn".to_string(), arn.to_string()),
                ("IpAddressType".to_string(), value.to_string()),
            ];
            call_elb(ctx, region, "SetIpAddressType", &parameters).await?;
        }
    }
    if changed(resource, declared, "LoadBalancerAttributes") {
        apply_attributes(
            ctx,
            region,
            "ModifyLoadBalancerAttributes",
            "LoadBalancerArn",
            arn,
            declared.get("LoadBalancerAttributes"),
        )
        .await?;
    }
    // The attributes the load balancer publishes now, read off that service rather than
    // carried over from the create: a changed placement changes what it reports.
    let attributes = match describe_load_balancer(ctx, region, arn).await? {
        Some(described) => load_balancer_attributes(&described, arn),
        None => BTreeMap::new(),
    };
    Ok(Some(Created::attributes(attributes)))
}

/// Apply a changed target group declaration: `ModifyTargetGroup` for the health check,
/// `ModifyTargetGroupAttributes` for the attributes, and `RegisterTargets` for the targets it
/// now names.
async fn update_target_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = query_members(
        declared,
        TARGET_GROUP_MEMBERS,
        &[],
        TARGET_GROUP_PROPERTIES_NOT_SENT,
        ELB_TARGET_GROUP,
    )?;
    // `ModifyTargetGroup` takes the health check alone: everything else a target group
    // declares is create-only ([`CREATE_ONLY_PROPERTIES`]), so a member outside that set is
    // not part of this call.
    parameters.retain(|(member, _)| {
        member.starts_with("HealthCheck")
            || member.starts_with("HealthyThreshold")
            || member.starts_with("UnhealthyThreshold")
            || member.starts_with("Matcher")
    });
    parameters.push(("TargetGroupArn".to_string(), arn.to_string()));
    let answered = call_elb(ctx, region, "ModifyTargetGroup", &parameters).await?;
    apply_attributes(
        ctx,
        region,
        "ModifyTargetGroupAttributes",
        "TargetGroupArn",
        arn,
        declared.get("TargetGroupAttributes"),
    )
    .await?;
    register_targets(ctx, region, arn, declared.get("Targets")).await?;
    Ok(Some(Created::attributes(target_group_attributes(
        &answered, arn,
    ))))
}

/// Apply a changed listener declaration: `ModifyListener`, which takes every member of the
/// create but the load balancer, plus the listener's own attributes.
async fn update_listener(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = query_members(
        declared,
        LISTENER_MEMBERS,
        &[],
        LISTENER_PROPERTIES_NOT_SENT,
        ELB_LISTENER,
    )?;
    // The load balancer a listener is on is not a member of `ModifyListener`: moving a
    // listener is a replacement, which [`CREATE_ONLY_PROPERTIES`] already answered for.
    parameters.retain(|(member, _)| !member.starts_with("LoadBalancerArn"));
    parameters.push(("ListenerArn".to_string(), arn.to_string()));
    call_elb(ctx, region, "ModifyListener", &parameters).await?;
    apply_attributes(
        ctx,
        region,
        "ModifyListenerAttributes",
        "ListenerArn",
        arn,
        declared.get("ListenerAttributes"),
    )
    .await?;
    Ok(Some(Created::attributes(arn_only_attributes(
        ELB_LISTENER,
        arn,
    ))))
}

/// Apply a changed rule declaration: `ModifyRule` for its conditions and actions, and
/// `SetRulePriorities` for the priority, which that call does not take.
async fn update_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = vec![("RuleArn".to_string(), arn.to_string())];
    for member in ["Conditions", "Actions"] {
        if let Some(value) = declared.get(member).filter(|value| !value.is_null()) {
            push_list(&mut parameters, member, "member", value, 0)?;
        }
    }
    let answered = call_elb(ctx, region, "ModifyRule", &parameters).await?;
    if let Some(priority) = declared.get("Priority").filter(|value| !value.is_null()) {
        let mut priorities = Vec::new();
        push_structure(
            &mut priorities,
            "RulePriorities.member.1",
            &serde_json::json!({ "RuleArn": arn, "Priority": priority }),
            0,
        )?;
        call_elb(ctx, region, "SetRulePriorities", &priorities).await?;
    }
    let mut attributes = arn_only_attributes(ELB_LISTENER_RULE, arn);
    attributes.insert("RuleArn".to_string(), arn.to_string());
    if let Some(is_default) = reported(&answered, "IsDefault") {
        attributes.insert("IsDefault".to_string(), is_default);
    }
    Ok(Some(Created::attributes(attributes)))
}

/// Apply a changed trust store declaration: `ModifyTrustStore`, the one call that re-points a
/// trust store at another CA certificate bundle — which is every property of the type that can
/// change on the trust store that exists ([`CREATE_ONLY_PROPERTIES`] holds its name).
async fn update_trust_store(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = query_members(
        declared,
        TRUST_STORE_MEMBERS,
        &[],
        TRUST_STORE_PROPERTIES_NOT_SENT,
        ELB_TRUST_STORE,
    )?;
    parameters.push(("TrustStoreArn".to_string(), arn.to_string()));
    let answered = call_elb(ctx, region, "ModifyTrustStore", &parameters).await?;
    Ok(Some(Created::attributes(trust_store_attributes(
        &answered, arn,
    ))))
}

/// Delete the resource a stack created, addressed by the ARN Elastic Load Balancing minted —
/// this resource's physical id.
///
/// A resource that service no longer holds ([`NOT_FOUND_CODES`]) is already in the state the
/// delete asks for, judged on the CODE and never on a message. Elastic Load Balancing takes no
/// snapshot of one of these on the way out — there is no such call — so a `Snapshot` deletion
/// policy on one of them deletes the resource with no snapshot invented for it.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (action, member) = match kind {
        Kind::LoadBalancer => ("DeleteLoadBalancer", "LoadBalancerArn"),
        Kind::TargetGroup => ("DeleteTargetGroup", "TargetGroupArn"),
        Kind::Listener => ("DeleteListener", "ListenerArn"),
        Kind::ListenerRule => ("DeleteRule", "RuleArn"),
        Kind::TrustStore => ("DeleteTrustStore", "TrustStoreArn"),
    };
    let parameters = vec![(
        member.to_string(),
        resource.physical_resource_id.clone(),
    )];
    match call_elb(ctx, region, action, &parameters).await {
        Ok(_) => Ok(()),
        Err(refusal) if NOT_FOUND_CODES.contains(&refusal.code.as_str()) => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether the owning service has finished removing a deleted resource.
///
/// Every one of these deletes takes the resource away as it answers — Elastic Load Balancing
/// reports no state a deleted load balancer, target group, listener, rule or trust store is
/// still being removed in — so nothing is asked and nothing waits.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// The resource's configuration as Elastic Load Balancing reports it, for drift detection.
///
/// Every one of these types is read back from Elastic Load Balancing itself, by the ARN the
/// create minted, through that type's own plural read ([`describe`]) — so what a drift check
/// compares a declaration against is the configuration the OWNING service reports, and the one
/// answer that reports a resource as gone is that service's own not-found code for it.
///
/// Each read reports its resource as a structure whose SCALAR members are its configuration (a
/// load balancer's scheme, family and address type; a target group's port, protocol, target
/// type and health check; a listener's port and protocol; a rule's priority; a trust store's
/// name and status), so those are the actual side of the comparison. A declared property the
/// read spells under another name, or
/// reports as a nested document rather than as a scalar, is reported by it in no comparable
/// form — [`super::super::drift`] compares only the properties BOTH sides name, so such a
/// property establishes nothing rather than being guessed at.
///
/// The one declared property that is not a member of any of these reads is the type's own
/// ATTRIBUTE collection, which is a second call of the owning service on both sides
/// ([`apply_attributes`]): it is read back through that call's own read
/// ([`attribute_collection`]), so an attribute set outside the stack is compared too.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let arn = &resource.physical_resource_id;
    let Some(described) = describe_one(ctx, region, kind, arn).await? else {
        return Ok(None);
    };
    let mut reported = xml_scalar_children(&described, ENTRY);
    if let Some((property, action, identity_member)) = attribute_collection(kind) {
        // Only a declaration that STATES the collection is compared against it: "only
        // resource properties explicitly defined in the stack template are checked for
        // drift", and that service reports every attribute of the resource — the ones its
        // defaults decide included — so the entries compared are the ones the template
        // explicitly names.
        if let Some(declared) = resource.properties.get(property).and_then(Value::as_array) {
            let answered = call_elb(
                ctx,
                region,
                action,
                &[(identity_member.to_string(), arn.to_string())],
            )
            .await?;
            reported.insert(property.to_string(), declared_attributes(declared, &answered));
        }
    }
    Ok(Some(Value::Object(reported)))
}

/// The declared property holding a type's ATTRIBUTE collection, the read that answers what the
/// resource's attributes ARE and the member that read takes the resource's ARN under — or
/// `None` for a type that publishes no such collection.
///
/// Source: each type's attribute read in that service's model
/// (`smithy_query(query="operation",operation="DescribeLoadBalancerAttributes,DescribeTargetGroupAttributes,DescribeListenerAttributes")`),
/// paired with the property whose declaration the matching `Modify…Attributes` applies
/// ([`LOAD_BALANCER_PROPERTIES_NOT_SENT`] and its siblings). Neither a rule nor a trust store
/// carries attributes.
fn attribute_collection(kind: Kind) -> Option<(&'static str, &'static str, &'static str)> {
    match kind {
        Kind::LoadBalancer => Some((
            "LoadBalancerAttributes",
            "DescribeLoadBalancerAttributes",
            "LoadBalancerArn",
        )),
        Kind::TargetGroup => Some((
            "TargetGroupAttributes",
            "DescribeTargetGroupAttributes",
            "TargetGroupArn",
        )),
        Kind::Listener => Some((
            "ListenerAttributes",
            "DescribeListenerAttributes",
            "ListenerArn",
        )),
        // Neither a rule nor a trust store carries an attribute collection: that service
        // publishes no `Modify…Attributes` call for either.
        Kind::ListenerRule | Kind::TrustStore => None,
    }
}

/// The attribute collection as the comparison takes it: one entry per entry the DECLARATION
/// names, carrying that entry's key and the value Elastic Load Balancing reports the resource
/// holding that key at.
///
/// The declared order is kept, because a difference is reported at the path of the entry it is
/// in: an attribute the template declares at a value the resource no longer holds is that
/// entry's own `Value` differing, rather than the whole collection being replaced by a reported
/// list the service happens to order its own way. An entry whose key that service reports
/// nothing under carries no value at all, which the comparison reports as that value having
/// been removed.
fn declared_attributes(declared: &[Value], answered: &str) -> Value {
    let held: BTreeMap<String, String> = xml_member_structures(answered, "Attributes")
        .into_iter()
        .filter_map(|entry| {
            let key = entry.get("Key")?.as_str()?.to_string();
            let value = entry.get("Value").and_then(Value::as_str).unwrap_or_default();
            Some((key, value.to_string()))
        })
        .collect();
    let entries = declared
        .iter()
        .map(|entry| {
            let key = entry.get("Key").and_then(Value::as_str).unwrap_or_default();
            let mut reported = ::serde_json::Map::new();
            reported.insert("Key".to_string(), Value::String(key.to_string()));
            if let Some(value) = held.get(key) {
                reported.insert("Value".to_string(), Value::String(value.clone()));
            }
            Value::Object(reported)
        })
        .collect();
    Value::Array(entries)
}

/// Whether a property of the type that cannot change on the resource that exists is declared
/// differently from the way the resource was deployed.
fn create_only_property_changed(
    resource_type: &str,
    resource: &StoredResource,
    declared: &Value,
) -> bool {
    let properties = CREATE_ONLY_PROPERTIES
        .iter()
        .find(|(named, _)| *named == resource_type)
        .map(|(_, properties)| *properties)
        .unwrap_or_default();
    properties
        .iter()
        .any(|property| changed(resource, declared, property))
}

/// Whether `property` is declared differently from the value the resource was deployed with.
///
/// A property neither side states is unchanged, and so is one both state identically: the
/// comparison is of the evaluated documents, which is what reaches the owning service.
fn changed(resource: &StoredResource, declared: &Value, property: &str) -> bool {
    resource.properties.get(property) != declared.get(property)
}

/// The ARN Elastic Load Balancing answered a create with, which is the resource's identity.
///
/// An answer carrying none is a refusal rather than a resource with no handle: nothing a
/// client could address would have been created.
fn answered_arn(document: &str, member: &str, resource_type: &str) -> Result<String, Refusal> {
    reported(document, member).ok_or_else(|| Refusal {
        code: "InvalidRequest".to_string(),
        message: format!(
            "Elastic Load Balancing reported no {member} for the {resource_type} it created, \
             which is what a reference to this resource resolves to"
        ),
    })
}

/// One member of an awsQuery answer, as the non-empty text it holds.
fn reported(document: &str, member: &str) -> Option<String> {
    xml_element(document, member).filter(|value| !value.is_empty())
}

/// The attributes an `AWS::ElasticLoadBalancingV2::LoadBalancer` publishes, read off what
/// Elastic Load Balancing reported for the load balancer.
///
/// "Ref returns the ARN of the load balancer"; `Fn::GetAtt` reads `CanonicalHostedZoneID`,
/// `DNSName`, `LoadBalancerFullName`, `LoadBalancerName` and `SecurityGroups` — and the
/// service that owns the load balancer is what decides every one of them. The FULL NAME is
/// the resource part of the load balancer's own ARN past `loadbalancer/`
/// (`app/my-load-balancer/50dc6c495c0c9188`), so it is read off that ARN rather than
/// composed from the parts.
fn load_balancer_attributes(document: &str, arn: &str) -> BTreeMap<String, String> {
    let mut attributes = arn_only_attributes(ELB_LOAD_BALANCER, arn);
    // Each is the value that service reports, and the EMPTY value where it reports none for
    // this load balancer: a Gateway Load Balancer is reached through its own VPC endpoints
    // rather than through a DNS name a hosted zone resolves, so Elastic Load Balancing
    // publishes no `CanonicalHostedZoneId` for one at all. The resource exists either way, so
    // the attribute it publishes carries what that service says about it rather than the
    // stand-in the naming rules computed before the create.
    for (attribute, member) in [
        ("DNSName", "DNSName"),
        // The model spells the member `CanonicalHostedZoneId`; the attribute the resource
        // type publishes is `CanonicalHostedZoneID`.
        ("CanonicalHostedZoneID", "CanonicalHostedZoneId"),
        ("LoadBalancerName", "LoadBalancerName"),
    ] {
        attributes.insert(
            attribute.to_string(),
            reported(document, member).unwrap_or_default(),
        );
    }
    if let Some(full_name) = arn_resource(arn).and_then(|resource| {
        resource
            .strip_prefix(LOAD_BALANCER_SEGMENT)
            .map(ToString::to_string)
    }) {
        attributes.insert("LoadBalancerFullName".to_string(), full_name);
    }
    attributes.insert(
        "SecurityGroups".to_string(),
        xml_member_list(document, "SecurityGroups").join(","),
    );
    attributes
}

/// The attributes an `AWS::ElasticLoadBalancingV2::TargetGroup` publishes, read off what
/// Elastic Load Balancing reported for the target group.
///
/// "Ref returns the ARN of the target group"; `Fn::GetAtt` reads `LoadBalancerArns`,
/// `TargetGroupArn`, `TargetGroupFullName` and `TargetGroupName`. The full name is the
/// resource part of the group's own ARN (`targetgroup/my-target-group/6d0ecf831eec9f09`), and
/// `LoadBalancerArns` is "the ARN of the load balancer that routes traffic to this target
/// group" — a list that service keeps, which is empty until a listener forwards to the group.
fn target_group_attributes(document: &str, arn: &str) -> BTreeMap<String, String> {
    let mut attributes = arn_only_attributes(ELB_TARGET_GROUP, arn);
    if let Some(name) = reported(document, "TargetGroupName") {
        attributes.insert("TargetGroupName".to_string(), name);
    }
    if let Some(full_name) = arn_resource(arn) {
        attributes.insert("TargetGroupFullName".to_string(), full_name.to_string());
    }
    attributes.insert(
        "LoadBalancerArns".to_string(),
        xml_member_list(document, "LoadBalancerArns").join(","),
    );
    attributes
}

/// The attributes an `AWS::ElasticLoadBalancingV2::TrustStore` publishes, read off what
/// Elastic Load Balancing reported for the trust store.
///
/// "Ref returns the ARN of the trust store"; `Fn::GetAtt` reads `NumberOfCaCertificates`,
/// `Status` and `TrustStoreArn` — and the two beside the ARN are that service's own report on
/// the trust store: the number of certificates it counted in the bundle the declaration named
/// (`TrustStore$NumberOfCaCertificates`) and the status it holds the store in
/// (`TrustStore$Status`). A value it reports none of is carried EMPTY rather than as the
/// stand-in the naming rules computed before the create.
fn trust_store_attributes(document: &str, arn: &str) -> BTreeMap<String, String> {
    let mut attributes = arn_only_attributes(ELB_TRUST_STORE, arn);
    for member in ["NumberOfCaCertificates", "Status"] {
        attributes.insert(
            member.to_string(),
            reported(document, member).unwrap_or_default(),
        );
    }
    attributes
}

/// The ARN attributes a resource of `resource_type` carries, re-pointed at the ARN its owning
/// service minted ([`crate::state::template::arn_attributes`], the one place that pair is
/// written).
fn arn_only_attributes(resource_type: &str, arn: &str) -> BTreeMap<String, String> {
    crate::state::template::arn_attributes(resource_type, arn)
        .into_iter()
        .collect()
}

/// The resource-type segment a load balancer's own ARN carries ahead of its full name
/// (`loadbalancer/app/my-load-balancer/50dc6c495c0c9188`), which is what that full name is the
/// rest of.
const LOAD_BALANCER_SEGMENT: &str = "loadbalancer/";

/// The state Elastic Load Balancing reports the load balancer `arn` in, or `None` for one it
/// does not hold.
async fn load_balancer_state(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> Result<Option<String>, Refusal> {
    let Some(described) = describe_load_balancer(ctx, region, arn).await? else {
        return Ok(None);
    };
    // The state's own `<Code>`, read as a direct child of the `<State>` it belongs to, so a
    // `Code` elsewhere in the answer is not mistaken for it.
    Ok(xml_scalar_children(&described, "State")
        .get("Code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .map(ToString::to_string))
}

/// The status Elastic Load Balancing reports the trust store `arn` in, or `None` for one it
/// does not hold.
async fn trust_store_status(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> Result<Option<String>, Refusal> {
    let Some(described) = describe_one(ctx, region, Kind::TrustStore, arn).await? else {
        return Ok(None);
    };
    Ok(reported(&described, "Status"))
}

/// The load balancer `arn`, as Elastic Load Balancing's own read of it, or `None` for one that
/// service does not hold.
async fn describe_load_balancer(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> Result<Option<String>, Refusal> {
    describe_one(ctx, region, Kind::LoadBalancer, arn).await
}

/// The element one of these plural reads carries each reported resource in, which is the Smithy
/// default list member name every list of this model uses.
const ENTRY: &str = "member";

/// How one of these types is read back from Elastic Load Balancing: its own plural read, the
/// member that selects ONE resource by ARN, and the code that read refuses an ARN that service
/// holds nothing under with.
///
/// Source: each type's own read in that service's model
/// (`smithy_query(query="operation",operation="DescribeTargetGroups,DescribeListeners,DescribeRules,DescribeTrustStores")`),
/// whose selection member is the plural of the resource's ARN; the codes are the
/// `awsQueryError` of each read's own not-found exception ([`NOT_FOUND_CODES`]).
fn describe(kind: Kind) -> (&'static str, &'static str, &'static str) {
    match kind {
        Kind::LoadBalancer => (
            "DescribeLoadBalancers",
            "LoadBalancerArns.member.1",
            "LoadBalancerNotFound",
        ),
        Kind::TargetGroup => (
            "DescribeTargetGroups",
            "TargetGroupArns.member.1",
            "TargetGroupNotFound",
        ),
        Kind::Listener => (
            "DescribeListeners",
            "ListenerArns.member.1",
            LISTENER_NOT_FOUND,
        ),
        Kind::ListenerRule => ("DescribeRules", "RuleArns.member.1", "RuleNotFound"),
        Kind::TrustStore => (
            "DescribeTrustStores",
            "TrustStoreArns.member.1",
            TRUST_STORE_NOT_FOUND,
        ),
    }
}

/// ONE resource of `kind`, as Elastic Load Balancing's own read of the ARN it minted for it, or
/// `None` for one that service answers it no longer holds.
///
/// The absence is judged on the read's own not-found CODE and never on a message, exactly as
/// [`delete`] judges a deletion that has already happened.
async fn describe_one(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    arn: &str,
) -> Result<Option<String>, Refusal> {
    let (action, member, not_found) = describe(kind);
    let parameters = vec![(member.to_string(), arn.to_string())];
    match call_elb(ctx, region, action, &parameters).await {
        Ok(answered) => Ok(Some(answered)),
        Err(refusal) if refusal.code == not_found => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Call one Elastic Load Balancing operation on the awsQuery wire its model declares.
///
/// The ONE place this crate addresses that service, so the type whose resource is no resource of
/// its own — the certificate list entry an `AWS::ElasticLoadBalancingV2::ListenerCertificate`
/// declares ([`super::elbv2_listener_certificate`]) — reaches it the same way the four types
/// here do.
pub(super) async fn call_elb(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    action: &str,
    parameters: &[(String, String)],
) -> Result<String, Refusal> {
    call_query(
        ctx,
        ELB_SIGNING_NAME,
        region,
        ELB_API_VERSION,
        action,
        parameters,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the catalogue table: a type, the properties it publishes, the members of
    /// its create and the properties the arm places itself.
    struct Catalogue {
        resource_type: &'static str,
        properties: &'static [&'static str],
        members: &'static [(&'static str, QueryMember)],
        placed: &'static [&'static str],
    }

    /// RULE: every member these arms send is a member of the call it is sent to, and every
    /// property of the type is either one of those members or named as one the arm places
    /// itself — so a declaration is carried out in the owning service's own vocabulary and
    /// nothing a template asked for is dropped.
    ///
    /// The property lists are the "Properties" sections of the five resource types, and the
    /// member lists are the creates' own ([`LOAD_BALANCER_MEMBERS`] and its siblings).
    #[test]
    fn every_published_property_is_sent_or_placed_by_the_arm() {
        let rows: &[Catalogue] = &[
            Catalogue {
                resource_type: ELB_LOAD_BALANCER,
                properties: &[
                    "EnablePrefixForIpv6SourceNat",
                    "IpAddressType",
                    "IpamPools",
                    "LoadBalancerAttributes",
                    "Name",
                    "Scheme",
                    "SecurityGroups",
                    "SubnetMappings",
                    "Subnets",
                    "Tags",
                    "Type",
                ],
                members: LOAD_BALANCER_MEMBERS,
                placed: LOAD_BALANCER_PROPERTIES_NOT_SENT,
            },
            Catalogue {
                resource_type: ELB_TARGET_GROUP,
                properties: &[
                    "HealthCheckEnabled",
                    "HealthCheckIntervalSeconds",
                    "HealthCheckPath",
                    "HealthCheckPort",
                    "HealthCheckProtocol",
                    "HealthCheckTimeoutSeconds",
                    "HealthyThresholdCount",
                    "IpAddressType",
                    "Matcher",
                    "Name",
                    "Port",
                    "Protocol",
                    "ProtocolVersion",
                    "Tags",
                    "TargetGroupAttributes",
                    "TargetType",
                    "Targets",
                    "UnhealthyThresholdCount",
                    "VpcId",
                ],
                members: TARGET_GROUP_MEMBERS,
                placed: TARGET_GROUP_PROPERTIES_NOT_SENT,
            },
            Catalogue {
                resource_type: ELB_LISTENER,
                properties: &[
                    "AlpnPolicy",
                    "Certificates",
                    "DefaultActions",
                    "ListenerAttributes",
                    "LoadBalancerArn",
                    "MutualAuthentication",
                    "Port",
                    "Protocol",
                    "SslPolicy",
                    "Tags",
                ],
                members: LISTENER_MEMBERS,
                placed: LISTENER_PROPERTIES_NOT_SENT,
            },
            Catalogue {
                resource_type: ELB_LISTENER_RULE,
                properties: &["Actions", "Conditions", "ListenerArn", "Priority", "Tags"],
                members: LISTENER_RULE_MEMBERS,
                placed: LISTENER_RULE_PROPERTIES_NOT_SENT,
            },
            Catalogue {
                resource_type: ELB_TRUST_STORE,
                properties: &[
                    "CaCertificatesBundleS3Bucket",
                    "CaCertificatesBundleS3Key",
                    "CaCertificatesBundleS3ObjectVersion",
                    "Name",
                    "Tags",
                ],
                members: TRUST_STORE_MEMBERS,
                placed: TRUST_STORE_PROPERTIES_NOT_SENT,
            },
        ];
        for row in rows {
            let resource_type = row.resource_type;
            for property in row.properties {
                let carried = row.members.iter().any(|(member, _)| member == property)
                    || row.placed.contains(property);
                assert!(
                    carried,
                    "case {resource_type:?}/{property:?}: expected the property to be a \
                     create member or one the arm places, got neither"
                );
            }
            for (member, _) in row.members {
                assert!(
                    row.properties.contains(member),
                    "case {resource_type:?}/{member:?}: expected the sent member to be a \
                     published property of the type, got none"
                );
            }
        }
    }

    /// RULE: an identifier and a full name a resource publishes are read off the ARN its
    /// owning service minted, so neither can name a resource that service does not hold.
    #[test]
    fn the_full_names_are_read_off_the_minted_arn() {
        // (case, the ARN, the full name it carries)
        let rows: &[(&str, &str, &str)] = &[
            (
                "an application load balancer's ARN",
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/web/50dc6c495c0c9188",
                "app/web/50dc6c495c0c9188",
            ),
            (
                "a network load balancer's ARN",
                "arn:aws:elasticloadbalancing:eu-west-1:000000000000:loadbalancer/net/edge/1a2b3c4d5e6f7a8b",
                "net/edge/1a2b3c4d5e6f7a8b",
            ),
        ];
        for (case, arn, full_name) in rows {
            let attributes = load_balancer_attributes("", arn);
            assert_eq!(
                attributes.get("LoadBalancerFullName").map(String::as_str),
                Some(*full_name),
                "case {case:?}: expected the full name of {arn:?}, got {attributes:?}"
            );
        }
        let group = "arn:aws:elasticloadbalancing:us-east-1:000000000000:targetgroup/web/6d0ecf831eec9f09";
        let attributes = target_group_attributes("", group);
        assert_eq!(
            attributes.get("TargetGroupFullName").map(String::as_str),
            Some("targetgroup/web/6d0ecf831eec9f09"),
            "case \"a target group's ARN\": expected its full name, got {attributes:?}"
        );
        // A value that is no ARN at all publishes no full name rather than a guess at one.
        assert_eq!(
            arn_resource("web"),
            None,
            "case \"a value that is no ARN\": expected no resource part"
        );
    }

    /// RULE: a list-valued attribute is the list the owning service reports, scoped to its own
    /// element — so one list of an answer is never read as another's.
    #[test]
    fn a_list_attribute_is_the_one_its_own_element_holds() {
        let document = "<LoadBalancers><member>\
             <SecurityGroups><member>sg-1</member><member>sg-2</member></SecurityGroups>\
             <AvailabilityZones><member><SubnetId>subnet-9</SubnetId></member>\
             </AvailabilityZones></member></LoadBalancers>";
        assert_eq!(
            xml_member_list(document, "SecurityGroups"),
            vec!["sg-1".to_string(), "sg-2".to_string()],
            "case \"two security groups beside a zone list\": expected only the groups"
        );
        assert_eq!(
            xml_member_list(document, "LoadBalancerArns"),
            Vec::<String>::new(),
            "case \"a list the answer does not carry\": expected no entries"
        );
        // The attribute a resource publishes is the joined form of exactly those entries.
        let attributes = load_balancer_attributes(
            document,
            "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/web/50dc6c495c0c9188",
        );
        assert_eq!(
            attributes.get("SecurityGroups").map(String::as_str),
            Some("sg-1,sg-2"),
            "case \"the published attribute\": expected the reported entries, got {attributes:?}"
        );
    }
}
