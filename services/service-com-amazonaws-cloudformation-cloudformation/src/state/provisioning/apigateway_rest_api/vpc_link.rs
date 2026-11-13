// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApiGateway::VpcLink`: the VPC link a PRIVATE integration reaches a VPC through,
//! created in Amazon API Gateway itself.
//!
//! A child module of [`super`], which owns the api and the resources it is built out of: the
//! same seam, the same calls and the same helpers (a declared property is read with
//! [`super::stated`], the identifier API Gateway assigned with [`super::own_id`], a property a
//! call cannot be made without is refused with [`super::missing_property`], and a property no
//! call here carries with [`super::refuse_unpublished`]), so no two arms of this control plane
//! word a refusal or place a member differently.
//!
//! ## Why it is really created
//!
//! A VPC link is what a method's private integration names in its `connectionId`
//! (`Integration$connectionId`: "The ID of the VpcLink used for the integration when
//! `connectionType=VPC_LINK`"), and only API Gateway assigns that id — `CreateVpcLink` takes
//! no identifier and answers the `id` it chose, and "Ref returns the VPC link ID"
//! ([`crate::state::template::identity_is_service_assigned`] names this type for it). So a
//! stack whose link exists nowhere publishes a value no integration can be wired with:
//! `GetVpcLinks` lists no such link, `GetVpcLink` answers `NotFoundException` for the
//! identifier the stack published, and nothing can be deleted through that service either.
//!
//! ## The call, and the wait after it
//!
//! The type is the one call that service publishes for it, on the URI that operation's own
//! `@http` binding declares: `CreateVpcLink` (`POST /vpclinks`), whose `name`, `description`,
//! `targetArns` and `tags` the declaration's properties are.
//!
//! It is also the ONE type of this control plane whose create leaves the resource still
//! coming up: creating a link is "an asynchronous operation that typically takes 2-4 minutes
//! to complete and become operational" (`CreateVpcLink`'s own documentation), and the answer
//! carries the `status` it is in — `PENDING` while it is provisioned, then `AVAILABLE` for a
//! link that came up or `FAILED` for one that cannot. So this module answers [`readiness`]
//! from that service's own read of the link: the stack holds the resource's creation open
//! while the link is `PENDING`, reports it created once API Gateway reports it `AVAILABLE`,
//! and FAILS the resource — which rolls the stack back — for a link that settled `FAILED`,
//! rather than publishing an id for a link that carries no traffic.

use super::*;

/// The collection every VPC link of the account lives in, which is what `CreateVpcLink` is
/// bound to (`POST /vpclinks`) and what one link hangs off as `/vpclinks/{vpcLinkId}`.
const VPC_LINKS_URI: &str = "/vpclinks";

/// Every property `AWS::ApiGateway::VpcLink` publishes. A declared property this list does
/// not carry is refused rather than dropped.
///
/// Source: the type's own property set, against `CreateVpcLink`'s members
/// (`smithy_query(query="operation",operation="CreateVpcLink")`: `name`, `description`,
/// `targetArns`, `tags`).
const VPC_LINK_PROPERTIES: &[&str] = &["Description", VPC_LINK_NAME, "Tags", VPC_LINK_TARGET_ARNS];

/// The property carrying the link's name, which `CreateVpcLink` takes as its `@required`
/// `name` ("The name used to label and identify the VPC link").
const VPC_LINK_NAME: &str = "Name";

/// The property carrying the load balancers the link reaches, which that call takes as its
/// `@required` `targetArns` ("The ARN of the network load balancer of the VPC targeted by the
/// VPC link"). A LIST of ARNs, so it travels as the list of strings the member is rather than
/// through the member walk the scalar properties take.
const VPC_LINK_TARGET_ARNS: &str = "TargetArns";

/// The properties that ARE `CreateVpcLink`'s own scalar body members, as `(property, member)`
/// pairs: each is the property with a lower-cased initial.
const VPC_LINK_MEMBERS: &[(&str, &str)] = &[("Description", "description"), (VPC_LINK_NAME, "name")];

/// The properties API Gateway's own read of a link reports, as `(property, member)` pairs,
/// for drift detection.
///
/// Its declared configuration in full BUT its tags, left out for the reason
/// [`super::REST_API_REPORTED`] records: the tags a resource carries change independently of
/// its declaration. The status is no part of a declaration, so it is not among them either.
pub(super) const VPC_LINK_REPORTED: &[(&str, &str)] = &[
    ("Description", "description"),
    (VPC_LINK_NAME, "name"),
    (VPC_LINK_TARGET_ARNS, "targetArns"),
];

/// The `status` a link that finished being provisioned and carries traffic reports
/// (`VpcLinkStatus`: `AVAILABLE | PENDING | DELETING | FAILED`), which is what says the
/// resource is created.
const VPC_LINK_AVAILABLE: &str = "AVAILABLE";

/// The `status` a link still being provisioned reports: "Deploying an API will wait if the
/// status is PENDING" (`VpcLink$status`), which is the wait this module holds the resource's
/// creation open for.
const VPC_LINK_PENDING: &str = "PENDING";

/// Create the link an `AWS::ApiGateway::VpcLink` declares, so a client that deployed the
/// stack and then calls `apigateway GetVpcLinks` finds it listed — and so a private
/// integration taking the id the stack published really reaches the VPC behind it — rather
/// than an identifier that resolves to nothing.
///
/// "Ref returns the VPC link ID", which only API Gateway produces, so the `id` it answers
/// replaces the stand-in the naming rules computed and is also the `VpcLinkId` attribute the
/// type publishes.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = create_request(resource)?;
    let tags = gateway_tags(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        VPC_LINKS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let id = api_gateway_answered(&answered, "id", "CreateVpcLink")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("VpcLinkId".to_string(), id.clone());
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// The declared properties as `CreateVpcLink`'s own body members, or the refusal a
/// declaration this module cannot act on is reported as.
///
/// The TAGS are added by the caller, since those are the resource's rather than the
/// declaration's alone ([`gateway_tags`] carries the ones CloudFormation propagates beside
/// the declared ones).
fn create_request(resource: &StoredResource) -> Result<Map<String, Value>, Refusal> {
    refuse_unpublished(resource, VPC_LINK_PROPERTIES)?;
    let mut request = Map::new();
    for (property, member) in VPC_LINK_MEMBERS {
        if let Some(value) = stated(resource, property) {
            request.insert((*member).to_string(), member_value(property, value)?);
        }
    }
    // `CreateVpcLinkRequest$name` is required, and a link has nowhere else to take its name
    // from.
    if !request.contains_key("name") {
        return Err(missing_property(resource, VPC_LINK_NAME));
    }
    // `targetArns` is required too: a link with no target reaches no load balancer, so a
    // declaration stating none asks for a link that call will not make.
    let targets = text_list(stated(resource, VPC_LINK_TARGET_ARNS));
    if targets.is_empty() {
        return Err(missing_property(resource, VPC_LINK_TARGET_ARNS));
    }
    request.insert(
        "targetArns".to_string(),
        Value::Array(targets.into_iter().map(Value::String).collect()),
    );
    Ok(request)
}

/// The URI that addresses ONE VPC link: the `@httpLabel` every call on it binds.
pub(super) fn vpc_link_uri(vpc_link_id: &str) -> String {
    format!("{VPC_LINKS_URI}/{}", percent_encoded(vpc_link_id))
}

/// Whether API Gateway reports the created link ready for a client to use.
///
/// `CreateVpcLink` answers the link `PENDING`, in which no integration can carry traffic
/// through it, and that service reports when it has settled — so the stack holds the
/// resource's creation open until it does, and a client that reads the stack as complete
/// finds a link it can really wire an integration to. A link that settled in any other status
/// — `FAILED` for one that could not be provisioned, either status of a deletion, or a link
/// that service holds none of — fails the resource with what that service reports rather than
/// waiting the window out.
pub(super) async fn readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(link) = read(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "API Gateway holds no VPC link for {physical_resource_id}"
        )));
    };
    match link.get("status").and_then(Value::as_str) {
        Some(VPC_LINK_AVAILABLE) => Ok(Readiness::Ready),
        Some(VPC_LINK_PENDING) => Ok(Readiness::Pending(VPC_LINK_PENDING.to_string())),
        // The link settled somewhere nothing brings it out of. `statusMessage` is "a
        // description about the VPC link status", so it is reported where the link carries
        // one: that sentence is what says WHY the link cannot carry traffic.
        Some(other) => Ok(Readiness::Failed(
            match link.get("statusMessage").and_then(Value::as_str) {
                Some(message) if !message.is_empty() => format!(
                    "API Gateway reports VPC link {physical_resource_id} in status {other}: \
                     {message}"
                ),
                _ => format!(
                    "API Gateway reports VPC link {physical_resource_id} in status {other}"
                ),
            },
        )),
        None => Ok(Readiness::Failed(format!(
            "API Gateway reports no status for VPC link {physical_resource_id}"
        ))),
    }
}

/// What API Gateway answers about one link, or `None` for a link it does not hold
/// (`NotFoundException`, judged on the CODE and never on a message, which carries the
/// request's own identifier).
///
/// The link's own DELETE and the read behind drift detection are [`super::delete`]'s and
/// [`super::actual_configuration`]'s arms, built from [`vpc_link_uri`] and
/// [`VPC_LINK_REPORTED`] — so this module states the link's address and its reported
/// properties once, whichever question is being put.
async fn read(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_link_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "GET",
        &vpc_link_uri(vpc_link_id),
        None,
    )
    .await
    {
        Ok(answered) => Ok(Some(answered)),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// A network load balancer's ARN as Elastic Load Balancing writes one
    /// (`loadbalancer/net/<name>/<id>`), which is what a link's `TargetArns` names.
    const NLB_ARN: &str =
        "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/nlb/0123456789abcdef";

    /// A second one, for a declaration that names more than one target.
    const SECOND_NLB_ARN: &str =
        "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/other/fedcba9876543210";

    /// A resource record as a deployment hands one to these arms.
    fn declared(properties: Value) -> StoredResource {
        StoredResource {
            logical_id: "Declared".to_string(),
            resource_type: APIGATEWAY_VPC_LINK.to_string(),
            physical_resource_id: "gim7c3q1x2".to_string(),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// Every published property of the type is placed somewhere: in the create's body or as
    /// the resource's tags. A property no arm places would be dropped from a create reported
    /// as a success.
    #[test]
    fn every_published_property_is_placed() {
        let placed: Vec<&str> = VPC_LINK_MEMBERS
            .iter()
            .map(|(property, _)| *property)
            .chain([VPC_LINK_TARGET_ARNS, "Tags"])
            .collect();
        for property in VPC_LINK_PROPERTIES {
            assert!(
                placed.contains(property),
                "case {property:?}: expected the property of {APIGATEWAY_VPC_LINK} to be placed \
                 by some arm, got {placed:?}"
            );
        }
    }

    /// A link is addressed by the URI its own operations bind, built from the identifier API
    /// Gateway assigned it.
    #[test]
    fn a_link_is_addressed_by_the_uri_its_operations_bind() {
        let rows: &[(&str, &str, &str)] = &[
            ("a minted id", "gim7c3q1x2", "/vpclinks/gim7c3q1x2"),
            // A value that is no identifier of that service's is still ONE path segment: an
            // id carrying URI structure would address a different resource.
            ("an id carrying a separator", "a/b", "/vpclinks/a%2Fb"),
        ];
        for (case, id, expected) in rows {
            let uri = vpc_link_uri(id);
            assert_eq!(
                uri, *expected,
                "case {case:?}: expected the URI {expected:?}, got {uri:?}"
            );
        }
    }

    /// A declaration states its properties as `CreateVpcLink`'s own members: each scalar
    /// property with a lower-cased initial, and the declared targets as that call's list.
    #[test]
    fn a_declaration_travels_as_the_members_the_create_takes() {
        let rows: &[(&str, Value, Value)] = &[
            (
                "a name and one target",
                json!({ "Name": "link", "TargetArns": [NLB_ARN] }),
                json!({ "name": "link", "targetArns": [NLB_ARN] }),
            ),
            (
                "a description beside two targets",
                json!({
                    "Name": "link",
                    "Description": "the private path",
                    "TargetArns": [NLB_ARN, SECOND_NLB_ARN],
                }),
                json!({
                    "name": "link",
                    "description": "the private path",
                    "targetArns": [NLB_ARN, SECOND_NLB_ARN],
                }),
            ),
            (
                // Tags are the RESOURCE's, added beside these members, so they are no part of
                // what the declaration's own properties map to.
                "tags beside the members",
                json!({
                    "Name": "link",
                    "TargetArns": [NLB_ARN],
                    "Tags": [{ "Key": "owner", "Value": "platform" }],
                }),
                json!({ "name": "link", "targetArns": [NLB_ARN] }),
            ),
        ];
        for (case, properties, expected) in rows {
            let request = create_request(&declared(properties.clone()))
                .unwrap_or_else(|refusal| panic!("case {case:?}: expected a request, got {refusal:?}"));
            assert_eq!(
                Value::Object(request.clone()),
                *expected,
                "case {case:?}: expected the members CreateVpcLink takes, got {request:?}"
            );
        }
    }

    /// A declaration stating none of a property the create cannot be made without is refused
    /// naming that property, in this control plane's one wording for that refusal, rather than
    /// a link being created with less than the declaration asked for — and so is one stating a
    /// property no call here carries.
    #[test]
    fn a_declaration_the_create_cannot_be_made_from_is_refused() {
        let rows: &[(&str, Value, &str, &str)] = &[
            (
                "neither a name nor a target",
                json!({}),
                MISSING_PROPERTY,
                VPC_LINK_NAME,
            ),
            (
                "a target and no name",
                json!({ "TargetArns": [NLB_ARN] }),
                MISSING_PROPERTY,
                VPC_LINK_NAME,
            ),
            (
                "a name and no target",
                json!({ "Name": "link" }),
                MISSING_PROPERTY,
                VPC_LINK_TARGET_ARNS,
            ),
            (
                "a name and an empty target list",
                json!({ "Name": "link", "TargetArns": [] }),
                MISSING_PROPERTY,
                VPC_LINK_TARGET_ARNS,
            ),
            (
                "a property no call here carries",
                json!({ "Name": "link", "TargetArns": [NLB_ARN], "Unpublished": "x" }),
                NOT_IMPLEMENTED,
                "Unpublished",
            ),
        ];
        for (case, properties, code, named) in rows {
            let refused = create_request(&declared(properties.clone()));
            let Err(refusal) = refused else {
                panic!("case {case:?}: expected a refusal, got {refused:?}");
            };
            assert_eq!(
                refusal.code, *code,
                "case {case:?}: expected the refusal to carry {code:?}, got {refusal:?}"
            );
            assert!(
                refusal.message.contains(named) && refusal.message.contains(APIGATEWAY_VPC_LINK),
                "case {case:?}: expected the refusal to name {named:?} and the type, got \
                 {refusal:?}"
            );
        }
    }
}
