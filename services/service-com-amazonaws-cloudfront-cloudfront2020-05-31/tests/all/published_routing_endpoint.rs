// SPDX-License-Identifier: CC0-1.0

//! The address a connection group publishes as its `RoutingEndpoint`, end to end over HTTP.
//!
//! RULE: `ConnectionGroup$RoutingEndpoint` is "the routing endpoint (also known as the DNS
//! name) that is assigned to the connection group" — the address a distribution tenant's own
//! domain is pointed at, and so an address a VIEWER then dials. The value published for it
//! is therefore an address a client on the machine this emulator runs on can dial and that
//! this emulator answers on FOR THAT GROUP: the `host:port` of the listener the group is
//! served on. A name under CloudFront's own zone resolves to nothing here, so a group that
//! reported one would send every tenant's traffic away from the emulator.
//!
//! The address belongs to the group RECORD and not to the call that happened to mint it, so
//! every projection of one group reports the one address — the create, the read by id, by
//! name and by ARN, the read BY that endpoint, and the update — and two groups are two
//! addresses.
//!
//! The rows span what the rule generalises over: which optional members the create named at
//! all (none, both booleans, a tag set), how the group was made (by a CALL, and the one
//! CloudFront mints BY ITSELF for a distribution tenant that named no group), and which
//! projection reports the address. Then each published address is DIALLED, which is what a
//! viewer handed that name does: the address serves the tenant whose domain the viewer
//! addressed, and serves no tenant of another group.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the connection group calls are
//! [`crate::connection_groups`]' and [`crate::connection_group_update`]'s, the read by
//! endpoint is [`crate::connection_group_by_routing_endpoint`]'s, the tenant create is
//! [`crate::distribution_tenant_lifecycle`]'s and the viewer dial is
//! [`crate::published_edge_address`]'s — each reused rather than copied.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_group_by_routing_endpoint::get_by_routing_endpoint;
use crate::connection_group_update::update_connection_group;
use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_lifecycle::{
    create_distribution, emulator, is_reachable_authority, Answer, API_VERSION, HOST,
};
use crate::distribution_tenant_lifecycle::{create_distribution_tenant, escape};
use crate::published_edge_address::dial_as;

/// The element the origin's answer carries, which is how a row tells an origin answer from
/// an answer the edge — or the address itself — produced.
const ORIGIN_DOCUMENT: &str = "DistributionList";

/// The path a viewer fetches through a published address. The origin below is this service's
/// own control-plane endpoint, so this is an object the origin really serves and what comes
/// back is the origin's own answer.
fn viewer_path() -> String {
    format!("/{API_VERSION}/distribution")
}

// ---------------------------------------------------------------------------
// The groups under test
// ---------------------------------------------------------------------------

/// One `CreateConnectionGroup` request a row submits: the members it names, and nothing
/// else.
struct Create {
    case: &'static str,
    name: &'static str,
    /// The optional members the request states, already in XML.
    members: &'static str,
}

/// The creates every assertion below runs over: a request naming no optional member at all,
/// one naming both booleans, and one carrying a tag set.
fn create_table() -> Vec<Create> {
    vec![
        Create {
            case: "every optional member left out",
            name: "routing-endpoint-minimal",
            members: "",
        },
        Create {
            case: "both booleans stated",
            name: "routing-endpoint-booleans",
            members: "<Ipv6Enabled>false</Ipv6Enabled><Enabled>true</Enabled>",
        },
        Create {
            case: "a tag set",
            name: "routing-endpoint-tagged",
            members: "<Tags><Items><Tag><Key>team</Key><Value>edge</Value></Tag></Items></Tags>",
        },
    ]
}

impl Create {
    fn xml(&self) -> String {
        format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{}</Name>{}</CreateConnectionGroupRequest>",
            self.name, self.members
        )
    }
}

/// The text of `member` in `answer`'s `ConnectionGroup`, which every read below is compared
/// through.
fn group_member(answer: &Answer, member: &str, case: &str) -> String {
    answer
        .root()
        .find_text(member)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {member}: {}", answer.raw))
        .to_string()
}

/// Create the connection group `create` describes and answer `(Id, RoutingEndpoint, ETag)`.
async fn created_group(router: &MultiServiceRouter, create: &Create) -> (String, String, String) {
    let answer = create_connection_group(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected the group to be created, got {} with {}",
        create.case, answer.status, answer.raw
    );
    (
        group_member(&answer, "Id", create.case),
        group_member(&answer, "RoutingEndpoint", create.case),
        answer.header("etag").to_string(),
    )
}

// ---------------------------------------------------------------------------
// The rule over every projection
// ---------------------------------------------------------------------------

/// Every connection group publishes an address a client can DIAL, that address is the
/// group's own, and every projection of the group reports the one value: the read by id, by
/// name and by ARN, the read by the endpoint itself, and the update — which changes the
/// group's settings without moving the address it is reached at.
#[tokio::test]
async fn every_connection_group_publishes_one_dialable_address() {
    let router = emulator();
    let mut published: BTreeSet<String> = BTreeSet::new();
    for create in create_table() {
        let (id, endpoint, etag) = created_group(&router, &create).await;
        let case = create.case;
        assert!(
            is_reachable_authority(&endpoint),
            "case {case}: expected a routing endpoint a viewer can dial - the address of the \
             listener this group is served on - got {endpoint:?}"
        );
        assert!(
            published.insert(endpoint.clone()),
            "case {case}: the address {endpoint:?} was published for a second group"
        );

        // Every read of the group reports that one address, whichever identifier addresses
        // it: "The ID, name, or Amazon Resource Name (ARN) of the connection group".
        let arn = group_member(&get_connection_group(&router, &id).await, "Arn", case);
        for identifier in [id.clone(), create.name.to_string(), arn] {
            let read = get_connection_group(&router, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected the group to be readable by {identifier:?}, got {} with {}",
                read.status, read.raw
            );
            let reported = group_member(&read, "RoutingEndpoint", case);
            assert_eq!(
                reported, endpoint,
                "case {case}: expected the read by {identifier:?} to report the published \
                 address {endpoint:?}, got {reported:?}"
            );
        }

        // The group is also read BY that address, which is what makes the published value
        // the group's own rather than a label beside it.
        let by_endpoint = get_by_routing_endpoint(&router, &endpoint).await;
        assert_eq!(
            by_endpoint.status, 200,
            "case {case}: expected the published address {endpoint:?} to resolve the group, got \
             {} with {}",
            by_endpoint.status, by_endpoint.raw
        );
        assert_eq!(
            group_member(&by_endpoint, "Id", case),
            id,
            "case {case}: expected the published address to resolve the group that published it"
        );

        // An UPDATE of the group's settings reports the same address: the endpoint is
        // assigned at creation, so a mutation that versions the group does not move it.
        let updated =
            update_connection_group(&router, &id, &etag, None, Some(false), Some(true)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected the update to land, got {} with {}",
            updated.status, updated.raw
        );
        let after_update = group_member(&updated, "RoutingEndpoint", case);
        assert_eq!(
            after_update, endpoint,
            "case {case}: expected the update to report the published address {endpoint:?}, got \
             {after_update:?}"
        );
    }
}

/// The group CloudFront mints BY ITSELF — "when you first create a distribution tenant and
/// you don't specify a connection group, CloudFront will automatically create a default
/// connection group for you" — publishes such an address too: the tenant that needed it is
/// routed through an address this emulator answers on, exactly as a group a call created is.
#[tokio::test]
async fn the_default_connection_group_publishes_a_dialable_address() {
    let router = emulator();
    let case = "the account's default connection group";
    let distribution = multi_tenant_distribution(&router, "ref-routing-default").await;
    // The tenant names NO connection group, which is what mints the default one. Its domain
    // is the distribution's own published address, a name this service resolves to a
    // CloudFront resource, so the claim on it is proven without a certificate request.
    let domain = tenant_domain(&router, "ref-routing-default-domain").await;
    let tenant = create_tenant(&router, &distribution, "routing-default-tenant", &domain, None).await;
    let group_id = tenant
        .root()
        .find_text("ConnectionGroupId")
        .unwrap_or_else(|| panic!("case {case}: the tenant carries no ConnectionGroupId"))
        .to_string();

    let read = get_connection_group(&router, &group_id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected the default group to be readable, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        group_member(&read, "IsDefault", case),
        "true",
        "case {case}: expected the group the tenant was routed through to be the default one"
    );
    let endpoint = group_member(&read, "RoutingEndpoint", case);
    assert!(
        is_reachable_authority(&endpoint),
        "case {case}: expected a routing endpoint a viewer can dial, got {endpoint:?}"
    );
}

// ---------------------------------------------------------------------------
// What the published address SERVES
// ---------------------------------------------------------------------------

/// A multi-tenant distribution whose one origin is this service's own control-plane
/// endpoint, so a viewer served through it is served the origin's real answer.
///
/// `ConnectionMode` `tenant-only` selects "a multi-tenant distribution with distribution
/// tenants"; such a distribution has no traffic domain of its own, which is exactly why its
/// viewers arrive through a connection group's routing endpoint.
async fn multi_tenant_distribution(router: &MultiServiceRouter, reference: &str) -> String {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>{HOST}</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{reference}</Comment><Enabled>true</Enabled>\
         <ConnectionMode>tenant-only</ConnectionMode>\
         </DistributionConfig>"
    );
    let created = create_distribution(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {reference}: expected the multi-tenant distribution to be created, got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("Id")
        .unwrap_or_else(|| panic!("case {reference}: the create carries no Id"))
        .to_string()
}

/// A domain a tenant may claim without a certificate request: a name this service already
/// resolves to a CloudFront resource of this account, which is the one proof of control its
/// DNS view holds. A standard distribution's own published address is such a name.
async fn tenant_domain(router: &MultiServiceRouter, reference: &str) -> String {
    let created = create_distribution(
        router,
        &crate::distribution_lifecycle::config("the domain a tenant claims", reference).xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {reference}: expected the distribution whose address the tenant claims to be \
         created, got {} with {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("DomainName")
        .unwrap_or_else(|| panic!("case {reference}: the create carries no DomainName"))
        .to_string()
}

/// Create a distribution tenant of `distribution` holding `domain`, routed through
/// `connection_group` when the caller names one.
async fn create_tenant(
    router: &MultiServiceRouter,
    distribution: &str,
    name: &str,
    domain: &str,
    connection_group: Option<&str>,
) -> Answer {
    let group = connection_group
        .map(|group| format!("<ConnectionGroupId>{group}</ConnectionGroupId>"))
        .unwrap_or_default();
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution}</DistributionId><Name>{name}</Name>\
         <Domains><member><Domain>{}</Domain></member></Domains>{group}\
         <Enabled>true</Enabled></CreateDistributionTenantRequest>",
        escape(domain)
    );
    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {name}: expected the tenant to be created, got {} with {}",
        created.status, created.raw
    );
    created
}

/// The published address ANSWERS: a viewer that dials it under the DOMAIN of a tenant the
/// group routes is served that tenant's distribution — the origin's own answer, reported as
/// a cache miss — while the same address serves no tenant of another group and none at all
/// for a host it routes nowhere.
#[tokio::test]
async fn the_published_routing_endpoint_serves_the_tenants_the_group_routes() {
    let router = emulator();
    let distribution = multi_tenant_distribution(&router, "ref-routing-serves").await;
    let (routing_group, routed_endpoint, _) = created_group(
        &router,
        &Create {
            case: "the group a tenant is routed through",
            name: "routing-endpoint-serving",
            members: "",
        },
    )
    .await;
    let (_, other_endpoint, _) = created_group(
        &router,
        &Create {
            case: "a group that routes nothing",
            name: "routing-endpoint-empty",
            members: "",
        },
    )
    .await;

    // The tenant's domain is the address a standard distribution of this account publishes:
    // a name this service resolves, so the claim is proven without a certificate request.
    let domain = tenant_domain(&router, "ref-routing-serves-domain").await;
    create_tenant(
        &router,
        &distribution,
        "routing-serving-tenant",
        &domain,
        Some(&routing_group),
    )
    .await;

    // The viewer dials the address the group published, under the tenant's own domain —
    // which is what a DNS record pointing that domain at the routing endpoint makes a client
    // do.
    let served = dial_as(&routed_endpoint, "GET", &viewer_path(), &domain).await;
    assert!(
        served.starts_with("HTTP/1.1 200"),
        "expected the published routing endpoint to serve the tenant's distribution, got \
         {served:?}"
    );
    assert!(
        served.contains(ORIGIN_DOCUMENT),
        "expected the origin's own document through the tenant's distribution, got {served:?}"
    );
    assert!(
        served
            .to_ascii_lowercase()
            .contains("x-cache: miss from cloudfront"),
        "expected the edge to report a cache miss, got {served:?}"
    );

    // The SAME address serves nothing for a host it routes to no tenant: the address is the
    // group's, and a group routes exactly the tenants whose domains it holds.
    let unrouted = dial_as(&routed_endpoint, "GET", &viewer_path(), &routed_endpoint).await;
    assert!(
        !unrouted.contains(ORIGIN_DOCUMENT),
        "expected no distribution to be served for a host this address routes nowhere, got \
         {unrouted:?}"
    );

    // And another group's address does not serve this group's tenant: two groups are two
    // addresses, each serving its own tenants.
    let elsewhere = dial_as(&other_endpoint, "GET", &viewer_path(), &domain).await;
    assert!(
        !elsewhere.contains(ORIGIN_DOCUMENT),
        "expected a group that routes no tenant to serve none, got {elsewhere:?}"
    );
}
