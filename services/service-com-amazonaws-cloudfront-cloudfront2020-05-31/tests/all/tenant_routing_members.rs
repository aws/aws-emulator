// SPDX-License-Identifier: CC0-1.0

//! WHERE a viewer's request has to arrive for CloudFront to answer it, end to end over
//! HTTP: the three members of the distribution tenant and connection group surface that
//! decide it.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * `DomainItem$ServeOn` is a property of ONE domain of a tenant, held as stated and
//!   reported back on every projection of the tenant, with `any` — "this is the default if
//!   you don't specify a value" — for a domain that stated none; a tenant's domains may
//!   each carry a different value, and an update replaces the value of the domains it
//!   states;
//! * a domain is routed to the tenant "only when [requests] arrive on the Anycast static
//!   IPs of the distribution tenant's connection group" when it is served
//!   `anycast-ip-list-only`, and CloudFront "treats requests that arrive on other IP
//!   addresses as requests for an unknown domain" — so while the group the tenant is routed
//!   through names no Anycast static IP list, no request for that domain can arrive
//!   anywhere the tenant answers and the domain is reported `inactive`;
//! * `OnDomainTakeover` decides what becomes of a domain another CloudFront resource is
//!   already served at: with `allow` "CloudFront moves the domain to this distribution
//!   tenant when the domain's DNS points to this distribution tenant", and otherwise — which
//!   is what a call stating nothing asks for — "the request fails if the domain is
//!   associated with another resource";
//! * `ConnectionGroup$ViewerFallback` names "the distribution whose certificate CloudFront
//!   uses for viewer requests that arrive on the connection group's Anycast static IPs
//!   without Server Name Indication (SNI)", so it is resolved against the stored
//!   distributions, held as the id it names and reported by every read and listing of the
//!   group.
//!
//! The rows differ along what those rules generalise over: every value each enum admits
//! (read off the model's own catalogs) plus the absent member, both kinds of resource a
//! domain can be taken over from (another tenant, a distribution's alternate domain name),
//! whether the DNS record points at the new tenant's connection group, and whether the
//! connection group carries an Anycast static IP list at all.
//!
//! The wire harness (the router, the request helpers and the element scanner) is
//! [`crate::distribution_lifecycle`]'s, and the creates and reads are the sibling suites',
//! reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    ON_DOMAIN_TAKEOVER_VALUES, SERVE_ON_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DOMAIN_STATUS_ACTIVE, DOMAIN_STATUS_INACTIVE, IP_ADDRESS_TYPE_IPV4, ON_DOMAIN_TAKEOVER_ALLOW,
    ON_DOMAIN_TAKEOVER_REJECT, SERVE_ON_ANY, SERVE_ON_ANYCAST_IP_LIST_ONLY, STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, Create as AnycastCreate};
use crate::connection_groups::{create_connection_group, get_connection_group, list_connection_groups, list_body as connection_group_list_body};
use crate::distribution_lifecycle::{call, config, create_distribution, emulator, get_distribution, Answer, Node, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, domains_of, escape, multi_tenant_distribution,
};
use crate::distribution_tenants_list::{list_body as tenant_list_body, list_tenants, summaries};

// ---------------------------------------------------------------------------
// The requests under test
// ---------------------------------------------------------------------------

/// One domain a create or an update binds: the name, and the `ServeOn` the call states for
/// it — `None` leaves the optional member out entirely.
type Domain<'a> = (&'a str, Option<&'a str>);

/// The `Domains` element binding `domains`. `DomainList`'s member carries no `xmlName`
/// (`smithy_query(query="shape",shape="DomainList")`), so each entry travels in the restXml
/// default `<member>` element, and `ServeOn` is a member of that entry.
fn domains_element(domains: &[Domain<'_>]) -> String {
    let mut out = "<Domains>".to_string();
    for (domain, serve_on) in domains {
        out.push_str(&format!("<member><Domain>{}</Domain>", escape(domain)));
        if let Some(serve_on) = serve_on {
            out.push_str(&format!("<ServeOn>{}</ServeOn>", escape(serve_on)));
        }
        out.push_str("</member>");
    }
    out.push_str("</Domains>");
    out
}

/// A `CreateDistributionTenantRequest` document, in the order the shape declares its
/// members.
///
/// Every domain a row below binds already resolves to a CloudFront resource of this
/// account, so no create asks for a managed certificate: the proof the account controls the
/// name is the DNS record this service answers for it.
fn create_tenant_xml(
    distribution_id: &str,
    name: &str,
    domains: &[Domain<'_>],
    connection_group_id: &str,
    on_domain_takeover: Option<&str>,
) -> String {
    let mut out = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{}</Name>{}",
        escape(distribution_id),
        escape(name),
        domains_element(domains)
    );
    // The one parameter the multi-tenant distribution under test marks required.
    out.push_str("<Parameters><member><Name>tenantName</Name><Value>");
    out.push_str(&escape(name));
    out.push_str("</Value></member></Parameters>");
    out.push_str(&format!(
        "<ConnectionGroupId>{}</ConnectionGroupId>",
        escape(connection_group_id)
    ));
    if let Some(on_domain_takeover) = on_domain_takeover {
        out.push_str(&format!(
            "<OnDomainTakeover>{}</OnDomainTakeover>",
            escape(on_domain_takeover)
        ));
    }
    out.push_str("</CreateDistributionTenantRequest>");
    out
}

/// A `CreateConnectionGroupRequest` document naming whichever of the members under test the
/// case states.
///
/// `viewer_fallback` is `Some` when the request carries the element at all: a
/// `Some(None)` sends one with no `DistributionId` inside it, which names no distribution.
fn create_group_xml(
    name: &str,
    anycast_ip_list_id: Option<&str>,
    viewer_fallback: Option<Option<&str>>,
) -> String {
    let mut out = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{}</Name>",
        escape(name)
    );
    if let Some(anycast_ip_list_id) = anycast_ip_list_id {
        out.push_str(&format!(
            "<AnycastIpListId>{}</AnycastIpListId>",
            escape(anycast_ip_list_id)
        ));
    }
    out.push_str(&viewer_fallback_element(viewer_fallback));
    out.push_str("</CreateConnectionGroupRequest>");
    out
}

/// The `ViewerFallback` element a request carries, on the same convention
/// [`create_group_xml`]'s argument follows.
fn viewer_fallback_element(viewer_fallback: Option<Option<&str>>) -> String {
    match viewer_fallback {
        None => String::new(),
        Some(None) => "<ViewerFallback></ViewerFallback>".to_string(),
        Some(Some(distribution_id)) => format!(
            "<ViewerFallback><DistributionId>{}</DistributionId></ViewerFallback>",
            escape(distribution_id)
        ),
    }
}

/// `PUT /2020-05-31/connection-group/{Id}` stating the `ViewerFallback` the case asks for.
///
/// The sibling [`crate::connection_group_update`] suite's helper carries the three members
/// it is about and not this one, so the member under test here is sent from here rather than
/// by widening that suite's request for every one of its rows.
async fn update_group_fallback(
    router: &MultiServiceRouter,
    id: &str,
    if_match: &str,
    viewer_fallback: Option<Option<&str>>,
) -> Answer {
    let body = format!(
        "<UpdateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{}\
         </UpdateConnectionGroupRequest>",
        viewer_fallback_element(viewer_fallback)
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/connection-group/{id}"),
        Some(&body),
        Some(if_match),
    )
    .await
}

/// `PUT /2020-05-31/distribution-tenant/{Id}` stating `Domains` and nothing else.
async fn update_tenant_domains(
    router: &MultiServiceRouter,
    id: &str,
    if_match: &str,
    domains: &[Domain<'_>],
) -> Answer {
    let body = format!(
        "<UpdateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{}\
         </UpdateDistributionTenantRequest>",
        domains_element(domains)
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution-tenant/{id}"),
        Some(&body),
        Some(if_match),
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Id}`.
async fn get_tenant(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// The `ServeOn` of every entry of a tenant's `Domains` list, in document order — the member
/// under test, beside the `(Domain, Status)` pairs
/// [`crate::distribution_tenant_lifecycle::domains_of`] reads from the same list.
fn serve_on_of(tenant: &Node) -> Vec<String> {
    tenant
        .at("Domains")
        .map(|list| {
            list.children
                .iter()
                .map(|entry| entry.text_at("ServeOn").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create a connection group named `name` and answer `(Id, RoutingEndpoint)`.
///
/// The routing endpoint is "also known as the DNS name … such as
/// `d111111abcdef8.cloudfront.net`", and it is a name this service RESOLVES, so a tenant's
/// domain that IS this endpoint is a domain whose DNS points at this group.
async fn created_group(
    router: &MultiServiceRouter,
    name: &str,
    anycast_ip_list_id: Option<&str>,
) -> (String, String) {
    let answer =
        create_connection_group(router, &create_group_xml(name, anycast_ip_list_id, None)).await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected the connection group to be created, got {} ({})",
        answer.status, answer.raw
    );
    (
        answer.root().text_at("Id").unwrap_or_default().to_string(),
        answer
            .root()
            .text_at("RoutingEndpoint")
            .unwrap_or_default()
            .to_string(),
    )
}

/// Create one Anycast static IP list and answer its id — the association that gives a
/// connection group Anycast static IPs for a request to arrive on.
async fn created_anycast_ip_list(router: &MultiServiceRouter, name: &str) -> String {
    let answer = create_anycast_ip_list(
        router,
        &AnycastCreate {
            case: "the Anycast static IP list a connection group is associated with",
            name: name.to_string(),
            // `IpCount` is one of the counts this service admits, as its own create
            // refusal names them.
            ip_count: 3,
            ip_address_type: Some(IP_ADDRESS_TYPE_IPV4),
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        }
        .xml(),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "case {name}: expected the Anycast static IP list to be created, got {} ({})",
        answer.status, answer.raw
    );
    answer.root().text_at("Id").unwrap_or_default().to_string()
}

/// The `DistributionTenantSummary` a listing reports for the tenant `id` names.
async fn listed_tenant(router: &MultiServiceRouter, id: &str, case: &str) -> Vec<String> {
    let listing = list_tenants(router, &tenant_list_body(None, None, None)).await;
    assert_eq!(
        listing.status, 200,
        "case {case}: expected the listing to answer 200, got {} ({})",
        listing.status, listing.raw
    );
    let summary = summaries(&listing)
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(id))
        .unwrap_or_else(|| panic!("case {case}: the listing reports no tenant {id}: {}", listing.raw));
    serve_on_of(summary)
}

// ---------------------------------------------------------------------------
// The catalogs the rows are built from
// ---------------------------------------------------------------------------

/// The rows below are built from the values the MODEL's enums admit, so this suite is tied
/// to the catalog the service is generated from rather than to a list written out here.
#[test]
fn the_rows_cover_every_value_the_model_admits() {
    assert_eq!(
        SERVE_ON_VALUES,
        [SERVE_ON_ANY, SERVE_ON_ANYCAST_IP_LIST_ONLY],
        "ServeOn admits {SERVE_ON_VALUES:?}"
    );
    assert_eq!(
        ON_DOMAIN_TAKEOVER_VALUES,
        [ON_DOMAIN_TAKEOVER_ALLOW, ON_DOMAIN_TAKEOVER_REJECT],
        "OnDomainTakeover admits {ON_DOMAIN_TAKEOVER_VALUES:?}"
    );
}

// ---------------------------------------------------------------------------
// ServeOn
// ---------------------------------------------------------------------------

/// One row: the `ServeOn` the create states for the tenant's one domain, whether the
/// connection group it is routed through carries an Anycast static IP list, and the status
/// the domain is therefore reported under.
#[derive(Debug, Clone, Copy)]
struct ServeOnRow {
    case: &'static str,
    serve_on: Option<&'static str>,
    anycast_ip_list: bool,
    active: bool,
}

/// Every value `ServeOn` admits and the absent member, crossed with whether the group has
/// Anycast static IPs where that is what the value turns on.
fn serve_on_rows() -> Vec<ServeOnRow> {
    vec![
        ServeOnRow {
            case: "no ServeOn stated",
            serve_on: None,
            anycast_ip_list: false,
            active: true,
        },
        ServeOnRow {
            case: "served on any arrival address",
            serve_on: Some(SERVE_ON_ANY),
            anycast_ip_list: false,
            active: true,
        },
        ServeOnRow {
            case: "served on any arrival address, with Anycast static IPs too",
            serve_on: Some(SERVE_ON_ANY),
            anycast_ip_list: true,
            active: true,
        },
        ServeOnRow {
            case: "served only on Anycast static IPs the group does not have",
            serve_on: Some(SERVE_ON_ANYCAST_IP_LIST_ONLY),
            anycast_ip_list: false,
            active: false,
        },
        ServeOnRow {
            case: "served only on the Anycast static IPs the group has",
            serve_on: Some(SERVE_ON_ANYCAST_IP_LIST_ONLY),
            anycast_ip_list: true,
            active: true,
        },
    ]
}

#[tokio::test]
async fn serve_on_is_held_as_stated_and_decides_where_the_domain_is_reachable() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-serve-on").await;
    let anycast_ip_list_id = created_anycast_ip_list(&router, "serve-on-list").await;

    for (index, row) in serve_on_rows().into_iter().enumerate() {
        let case = format!("{} ({index})", row.case);
        let (group_id, routing_endpoint) = created_group(
            &router,
            &format!("serve-on-group-{index:02}"),
            row.anycast_ip_list
                .then_some(anycast_ip_list_id.as_str()),
        )
        .await;
        // The domain IS the routing endpoint of the group the tenant is routed through, so
        // the DNS record this service answers for it points at that group.
        let name = format!("serve-on-{index:02}");
        let created = create_distribution_tenant(
            &router,
            &create_tenant_xml(
                &distribution_id,
                &name,
                &[(routing_endpoint.as_str(), row.serve_on)],
                &group_id,
                None,
            ),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} ({})",
            created.status, created.raw
        );

        // The value the record holds: the one stated, or the documented default for a
        // domain that stated none.
        let expected = row.serve_on.unwrap_or(SERVE_ON_ANY).to_string();
        let id = created.root().text_at("Id").unwrap_or_default().to_string();
        assert_eq!(
            serve_on_of(created.root()),
            vec![expected.clone()],
            "case {case}: expected ServeOn {expected:?}, got {}",
            created.raw
        );

        // The status the domain is reachable under, which is what the member DECIDES: a
        // domain served only on Anycast static IPs is reached through none while the group
        // it is routed through has none.
        let expected_status = if row.active {
            DOMAIN_STATUS_ACTIVE
        } else {
            DOMAIN_STATUS_INACTIVE
        };
        assert_eq!(
            domains_of(created.root()),
            vec![(routing_endpoint.clone(), expected_status.to_string())],
            "case {case}: expected the domain {expected_status}, got {}",
            created.raw
        );

        // The round trip: a read and a listing answer from the one record the create wrote.
        let read = get_tenant(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read to answer 200, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            serve_on_of(read.root()),
            vec![expected.clone()],
            "case {case}: the read must answer ServeOn {expected:?}, got {}",
            read.raw
        );
        assert_eq!(
            domains_of(read.root()),
            vec![(routing_endpoint.clone(), expected_status.to_string())],
            "case {case}: the read must answer the domain {expected_status}, got {}",
            read.raw
        );
        assert_eq!(
            listed_tenant(&router, &id, &case).await,
            vec![expected.clone()],
            "case {case}: the listing must report ServeOn {expected:?}"
        );
    }
}

#[tokio::test]
async fn a_tenant_holds_one_serve_on_per_domain_and_an_update_replaces_the_ones_it_states() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-serve-on-per-domain").await;
    let (group_id, first) = created_group(&router, "per-domain-group", None).await;
    // A second group, whose routing endpoint is the tenant's other domain: another name
    // this service resolves, pointing at a group the tenant is NOT routed through.
    let (_, second) = created_group(&router, "per-domain-other", None).await;

    let created = create_distribution_tenant(
        &router,
        &create_tenant_xml(
            &distribution_id,
            "per-domain",
            &[
                (first.as_str(), Some(SERVE_ON_ANY)),
                (second.as_str(), Some(SERVE_ON_ANYCAST_IP_LIST_ONLY)),
            ],
            &group_id,
            None,
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case two domains, two values: expected HTTP 201, got {} ({})",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    assert_eq!(
        serve_on_of(created.root()),
        vec![
            SERVE_ON_ANY.to_string(),
            SERVE_ON_ANYCAST_IP_LIST_ONLY.to_string()
        ],
        "case two domains, two values: each domain keeps its own ServeOn, got {}",
        created.raw
    );

    // An update that states the domains REPLACES the value of each one it states — here
    // swapped, and the second domain's member left out entirely so the default applies.
    let etag = created.header("etag").to_string();
    let updated = update_tenant_domains(
        &router,
        &id,
        &etag,
        &[
            (first.as_str(), Some(SERVE_ON_ANYCAST_IP_LIST_ONLY)),
            (second.as_str(), None),
        ],
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case the update that swaps the values: expected HTTP 200, got {} ({})",
        updated.status, updated.raw
    );
    assert_eq!(
        serve_on_of(updated.root()),
        vec![
            SERVE_ON_ANYCAST_IP_LIST_ONLY.to_string(),
            SERVE_ON_ANY.to_string()
        ],
        "case the update that swaps the values: expected the stated values, got {}",
        updated.raw
    );
    // The first domain was active before the update — it points at the group the tenant is
    // routed through — and the update has taken it off every address that group answers on.
    assert_eq!(
        domains_of(updated.root()),
        vec![
            (first.clone(), DOMAIN_STATUS_INACTIVE.to_string()),
            (second.clone(), DOMAIN_STATUS_INACTIVE.to_string()),
        ],
        "case the update that swaps the values: expected both domains inactive, got {}",
        updated.raw
    );

    let read = get_tenant(&router, &id).await;
    assert_eq!(
        serve_on_of(read.root()),
        vec![
            SERVE_ON_ANYCAST_IP_LIST_ONLY.to_string(),
            SERVE_ON_ANY.to_string()
        ],
        "case the update that swaps the values: the read must answer from the written \
         record, got {}",
        read.raw
    );
}

// ---------------------------------------------------------------------------
// OnDomainTakeover
// ---------------------------------------------------------------------------

/// One row: what the claiming create states for `OnDomainTakeover`, whether the domain's DNS
/// points at the connection group THAT tenant is routed through, and whether the claim is
/// admitted.
#[derive(Debug, Clone, Copy)]
struct TakeoverRow {
    case: &'static str,
    on_domain_takeover: Option<&'static str>,
    dns_points_at_the_claimant: bool,
    moved: bool,
}

/// Every value `OnDomainTakeover` admits and the absent member, crossed with whether the
/// domain's DNS points at the claiming tenant — the condition `allow` is documented under.
fn takeover_rows() -> Vec<TakeoverRow> {
    vec![
        TakeoverRow {
            case: "no OnDomainTakeover stated",
            on_domain_takeover: None,
            dns_points_at_the_claimant: true,
            moved: false,
        },
        TakeoverRow {
            case: "the takeover rejected",
            on_domain_takeover: Some(ON_DOMAIN_TAKEOVER_REJECT),
            dns_points_at_the_claimant: true,
            moved: false,
        },
        TakeoverRow {
            case: "the takeover allowed, with the DNS pointing somewhere else",
            on_domain_takeover: Some(ON_DOMAIN_TAKEOVER_ALLOW),
            dns_points_at_the_claimant: false,
            moved: false,
        },
        TakeoverRow {
            case: "the takeover allowed, with the DNS pointing at the claimant",
            on_domain_takeover: Some(ON_DOMAIN_TAKEOVER_ALLOW),
            dns_points_at_the_claimant: true,
            moved: true,
        },
    ]
}

#[tokio::test]
async fn a_domain_another_tenant_serves_moves_only_when_the_call_asks_and_its_dns_points_here() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-takeover").await;

    for (index, row) in takeover_rows().into_iter().enumerate() {
        let case = format!("{} ({index})", row.case);
        // The holder: a tenant SERVED at the routing endpoint of the group it is routed
        // through, so the domain is taken and the DNS record points at that group.
        let (holder_group, domain) =
            created_group(&router, &format!("takeover-held-{index:02}"), None).await;
        let holder_name = format!("takeover-holder-{index:02}");
        let holder = create_distribution_tenant(
            &router,
            &create_tenant_xml(
                &distribution_id,
                &holder_name,
                &[(domain.as_str(), None)],
                &holder_group,
                None,
            ),
        )
        .await;
        assert_eq!(
            holder.status, 201,
            "case {case}: expected the holder to be created, got {} ({})",
            holder.status, holder.raw
        );
        let holder_id = holder.root().text_at("Id").unwrap_or_default().to_string();
        let holder_etag = holder.header("etag").to_string();

        // The claimant's connection group: the holder's where the row has the DNS pointing
        // at the claimant, and a group of its own where it does not.
        let claimant_group = if row.dns_points_at_the_claimant {
            holder_group.clone()
        } else {
            created_group(&router, &format!("takeover-other-{index:02}"), None)
                .await
                .0
        };
        let claimant_name = format!("takeover-claimant-{index:02}");
        let claim = create_distribution_tenant(
            &router,
            &create_tenant_xml(
                &distribution_id,
                &claimant_name,
                &[(domain.as_str(), None)],
                &claimant_group,
                row.on_domain_takeover,
            ),
        )
        .await;

        if !row.moved {
            assert_eq!(
                claim.status, 409,
                "case {case}: expected HTTP 409, got {} ({})",
                claim.status, claim.raw
            );
            assert_eq!(
                claim.error_code(),
                "CNAMEAlreadyExists",
                "case {case}: expected CNAMEAlreadyExists, got {:?} ({})",
                claim.error_code(),
                claim.raw
            );
            // A refused claim moves nothing: the holder keeps the domain under the version
            // token it was created with.
            let held = get_tenant(&router, &holder_id).await;
            assert_eq!(
                domains_of(held.root()),
                vec![(domain.clone(), DOMAIN_STATUS_ACTIVE.to_string())],
                "case {case}: the holder must keep the domain, got {}",
                held.raw
            );
            assert_eq!(
                held.header("etag"),
                holder_etag,
                "case {case}: a refused claim must spend no version token, got {}",
                held.raw
            );
            continue;
        }

        assert_eq!(
            claim.status, 201,
            "case {case}: expected HTTP 201, got {} ({})",
            claim.status, claim.raw
        );
        let claimant_id = claim.root().text_at("Id").unwrap_or_default().to_string();
        assert_eq!(
            domains_of(claim.root()),
            vec![(domain.clone(), DOMAIN_STATUS_ACTIVE.to_string())],
            "case {case}: the claimant must be served at the domain it took, got {}",
            claim.raw
        );

        // The move: the domain is released from the holder, whose record is versioned as
        // any other change to it is.
        let held = get_tenant(&router, &holder_id).await;
        assert_eq!(
            domains_of(held.root()),
            Vec::new(),
            "case {case}: the holder must have lost the domain, got {}",
            held.raw
        );
        assert_ne!(
            held.header("etag"),
            holder_etag,
            "case {case}: the move must version the holder, got {}",
            held.raw
        );
        assert_eq!(
            held.root().text_at("Status"),
            Some(STATUS_IN_PROGRESS),
            "case {case}: the holder's new configuration has to reach the edge locations, \
             got {}",
            held.raw
        );

        // And the claimant is the one the domain now resolves to.
        let claimed = get_tenant(&router, &claimant_id).await;
        assert_eq!(
            domains_of(claimed.root()),
            vec![(domain.clone(), DOMAIN_STATUS_ACTIVE.to_string())],
            "case {case}: the claimant must hold the domain, got {}",
            claimed.raw
        );
    }
}

#[tokio::test]
async fn a_domain_a_distributions_alternate_domain_name_holds_moves_the_same_way() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-takeover-alias").await;
    let (group_id, domain) = created_group(&router, "takeover-alias-group", None).await;

    // A standard distribution that declares the domain as an alternate domain name, which
    // is the other kind of resource a domain can already be served at.
    let mut alias_config = config("the distribution that declares the alias", "ref-alias-holder");
    alias_config.aliases = vec![domain.clone()];
    let holder = create_distribution(&router, &alias_config.xml()).await;
    assert_eq!(
        holder.status, 201,
        "case the alias holder: expected the distribution to be created, got {} ({})",
        holder.status, holder.raw
    );
    let holder_id = holder.root().text_at("Id").unwrap_or_default().to_string();
    let holder_etag = holder.header("etag").to_string();

    // Rejected first: the claim fails and the alias stands.
    let refused = create_distribution_tenant(
        &router,
        &create_tenant_xml(
            &distribution_id,
            "alias-rejected",
            &[(domain.as_str(), None)],
            &group_id,
            Some(ON_DOMAIN_TAKEOVER_REJECT),
        ),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case the rejected claim: expected HTTP 409, got {} ({})",
        refused.status, refused.raw
    );
    let standing = get_distribution(&router, &holder_id).await;
    assert_eq!(
        standing.header("etag"),
        holder_etag,
        "case the rejected claim: a refused claim must spend no version token, got {}",
        standing.raw
    );

    // Allowed, with the domain's DNS pointing at the group the tenant is routed through.
    let claim = create_distribution_tenant(
        &router,
        &create_tenant_xml(
            &distribution_id,
            "alias-claimant",
            &[(domain.as_str(), None)],
            &group_id,
            Some(ON_DOMAIN_TAKEOVER_ALLOW),
        ),
    )
    .await;
    assert_eq!(
        claim.status, 201,
        "case the allowed claim: expected HTTP 201, got {} ({})",
        claim.status, claim.raw
    );

    // The alias is released from the distribution, and the count it states is the count of
    // the names that remain.
    let released = get_distribution(&router, &holder_id).await;
    let aliases = released
        .root()
        .at("DistributionConfig/Aliases")
        .unwrap_or_else(|| {
            panic!(
                "case the allowed claim: the distribution reports no Aliases: {}",
                released.raw
            )
        });
    assert_eq!(
        aliases
            .at("Items")
            .map(|items| items.children_named("CNAME").len())
            .unwrap_or_default(),
        0,
        "case the allowed claim: the alias must be released, got {}",
        released.raw
    );
    assert_eq!(
        aliases.text_at("Quantity"),
        Some("0"),
        "case the allowed claim: Quantity must count the names that remain, got {}",
        released.raw
    );
    assert_ne!(
        released.header("etag"),
        holder_etag,
        "case the allowed claim: the move must version the distribution, got {}",
        released.raw
    );
}

// ---------------------------------------------------------------------------
// ViewerFallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_connection_groups_viewer_fallback_names_a_distribution_this_account_holds() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-viewer-fallback").await;

    // An id no distribution carries names no entity, whatever shape it takes — and nothing
    // is stored for it.
    for (case, candidate) in [
        ("an id in CloudFront's shape", "E1AAAAAAAAAAAA"),
        ("a value of no distribution shape at all", "not an id !!"),
    ] {
        let refused = create_connection_group(
            &router,
            &create_group_xml(
                &format!("fallback-refused-{}", candidate.len()),
                None,
                Some(Some(candidate)),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected HTTP 404, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?} ({})",
            refused.error_code(),
            refused.raw
        );
    }

    // An element naming NO distribution describes no fallback, so the group carries none.
    let bare = create_connection_group(
        &router,
        &create_group_xml("fallback-bare", None, Some(None)),
    )
    .await;
    assert_eq!(
        bare.status, 201,
        "case an element naming no distribution: expected HTTP 201, got {} ({})",
        bare.status, bare.raw
    );
    assert_eq!(
        bare.root().text_at("ViewerFallback/DistributionId"),
        None,
        "case an element naming no distribution: no fallback must be stored, got {}",
        bare.raw
    );

    // A distribution this account holds is resolved, held and reported back.
    let created = create_connection_group(
        &router,
        &create_group_xml("fallback-named", None, Some(Some(&distribution_id))),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case a resolvable distribution: expected HTTP 201, got {} ({})",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    assert_eq!(
        created.root().text_at("ViewerFallback/DistributionId"),
        Some(distribution_id.as_str()),
        "case a resolvable distribution: expected the submitted id, got {}",
        created.raw
    );

    // The round trip: the read and the listing answer from the one stored record.
    let read = get_connection_group(&router, &id).await;
    assert_eq!(
        read.root().text_at("ViewerFallback/DistributionId"),
        Some(distribution_id.as_str()),
        "case a resolvable distribution: the read must answer the stored fallback, got {}",
        read.raw
    );
    let listing =
        list_connection_groups(&router, &connection_group_list_body(None, None, None)).await;
    let summary = listing
        .root()
        .at("ConnectionGroups")
        .map(|wrapper| wrapper.children_named("ConnectionGroupSummary"))
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(id.as_str()))
        .unwrap_or_else(|| {
            panic!(
                "case a resolvable distribution: the listing reports no group {id}: {}",
                listing.raw
            )
        });
    assert_eq!(
        summary.text_at("ViewerFallback/DistributionId"),
        Some(distribution_id.as_str()),
        "case a resolvable distribution: the listing must report the stored fallback, got {}",
        listing.raw
    );
}

#[tokio::test]
async fn an_update_replaces_the_viewer_fallback_it_states_and_keeps_the_one_it_does_not() {
    let router = emulator();
    let first = multi_tenant_distribution(&router, "ref-fallback-first").await;
    let second = multi_tenant_distribution(&router, "ref-fallback-second").await;

    let created = create_connection_group(
        &router,
        &create_group_xml("fallback-updated", None, Some(Some(&first))),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the group under test: expected HTTP 201, got {} ({})",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let mut etag = created.header("etag").to_string();

    // An update that states no `ViewerFallback` leaves the one the group carries.
    let kept = update_group_fallback(&router, &id, &etag, None).await;
    assert_eq!(
        kept.status, 200,
        "case an update stating nothing: expected HTTP 200, got {} ({})",
        kept.status, kept.raw
    );
    assert_eq!(
        kept.root().text_at("ViewerFallback/DistributionId"),
        Some(first.as_str()),
        "case an update stating nothing: the fallback must stand, got {}",
        kept.raw
    );
    etag = kept.header("etag").to_string();

    // An id no distribution carries is no entity, and the refusal changes nothing.
    let refused = update_group_fallback(&router, &id, &etag, Some(Some("E1AAAAAAAAAAAA"))).await;
    assert_eq!(
        refused.status, 404,
        "case an update naming an absent distribution: expected HTTP 404, got {} ({})",
        refused.status, refused.raw
    );
    let unchanged = get_connection_group(&router, &id).await;
    assert_eq!(
        unchanged.root().text_at("ViewerFallback/DistributionId"),
        Some(first.as_str()),
        "case an update naming an absent distribution: the fallback must stand, got {}",
        unchanged.raw
    );
    assert_eq!(
        unchanged.header("etag"),
        etag,
        "case an update naming an absent distribution: no version token must be spent, got {}",
        unchanged.raw
    );

    // A stated fallback REPLACES the one the group carried.
    let replaced = update_group_fallback(&router, &id, &etag, Some(Some(&second))).await;
    assert_eq!(
        replaced.root().text_at("ViewerFallback/DistributionId"),
        Some(second.as_str()),
        "case an update naming another distribution: expected the stated id, got {}",
        replaced.raw
    );
    etag = replaced.header("etag").to_string();

    // And an element naming no distribution leaves the group with no fallback at all.
    let cleared = update_group_fallback(&router, &id, &etag, Some(None)).await;
    assert_eq!(
        cleared.root().text_at("ViewerFallback/DistributionId"),
        None,
        "case an update naming no distribution: the fallback must be released, got {}",
        cleared.raw
    );
    let read = get_connection_group(&router, &id).await;
    assert_eq!(
        read.root().text_at("ViewerFallback/DistributionId"),
        None,
        "case an update naming no distribution: the read must answer from the written \
         record, got {}",
        read.raw
    );
}
