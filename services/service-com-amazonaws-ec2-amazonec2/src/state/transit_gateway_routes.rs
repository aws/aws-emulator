// SPDX-License-Identifier: CC0-1.0

//! The routes of a transit gateway route table: what a static one is stored as, the states and
//! types its catalogs admit, what a create is refused with, the store's bounds and its one
//! writer — and, beside them, the PROPAGATED routes the table's propagations put in it, which
//! nothing stores because they are derived from those propagations
//! ([`ComAmazonawsEc2Amazonec2State::transit_gateway_route_set`] is the one answer to what a
//! table holds).
//!
//! The tables themselves — their id form, their own states and the refusal for an id that
//! names none — live in the transit gateway route table store, beside the other transit
//! gateway stores; this module holds only what a ROUTE of one adds to them. A
//! route has no id of its own: it is named by the destination it matches inside one table
//! ("The CIDR range for the route. This must match the CIDR for the route exactly",
//! `DeleteTransitGatewayRouteRequest$DestinationCidrBlock`), so the store is keyed by
//! (`TransitGatewayRouteTableId`, destination) exactly as the prefix list references of a
//! table are keyed by (table, prefix list).
//!
//! Each route is held as the modelled [`TransitGatewayRoute`] shape itself, so every member a
//! writer sets round-trips through `SearchTransitGatewayRoutes` without a field-by-field copy
//! that can silently drop one.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use std::collections::BTreeMap;

use super::{
    ComAmazonawsEc2Amazonec2State, MAX_STORED_BYTES, MAX_STORED_RECORDS_PER_STORE,
    PROPAGATION_STATE_ENABLED, ROUTE_LIMIT_EXCEEDED, STORED_RECORD_OVERHEAD_BYTES,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TransitGatewayAttachmentView,
    canonical_cidr_block, held_ipv4_cidr_blocks, held_ipv6_cidr_blocks,
};
use crate::ops::types::{TransitGatewayRoute, TransitGatewayRouteAttachment};
use crate::scaffolded::Scaffolded;

/// A route that is carrying traffic to its attachment: the `active` member of
/// `TransitGatewayRouteState`
/// ([`crate::ops::types::TRANSIT_GATEWAY_ROUTE_STATE_VALUES`] — `active`, `blackhole`,
/// `deleted`, `deleting`, `pending`).
///
/// The state a created route that names an attachment is reported in straight away: AWS's own
/// `create-transit-gateway-route` example answers the route it just made `"State": "active"`
/// with the attachment it forwards to, so the route is in force at once and the clock owes it
/// no later move.
pub const TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE: &str = "active";

/// A route that DROPS the traffic matching it: that same catalog's `blackhole`, and the state
/// AWS's own `create-transit-gateway-route --blackhole` example answers the created route in.
///
/// "Indicates whether to drop traffic that matches this route"
/// (`CreateTransitGatewayRouteRequest$Blackhole`): a blackhole route forwards to nothing, so
/// its state says so rather than reporting it `active` beside an attachment it does not have.
pub const TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE: &str = "blackhole";

/// A route a caller created: the `static` member of `TransitGatewayRouteType`
/// ([`crate::ops::types::TRANSIT_GATEWAY_ROUTE_TYPE_VALUES`] — `propagated`, `static`).
///
/// "Creates a STATIC route for the specified transit gateway route table"
/// (`CreateTransitGatewayRoute`) fixes the type of every route this store writes; a
/// `propagated` route is one an attachment's own prefixes put in the table, which no operation
/// writes at all — it is DERIVED from the propagations of the table
/// ([`ComAmazonawsEc2Amazonec2State::transit_gateway_route_set`]).
pub const TRANSIT_GATEWAY_ROUTE_TYPE_STATIC: &str = "static";

/// A route an attachment's own prefixes put in the table: that same catalog's `propagated`
/// member.
///
/// "Enables the specified attachment to propagate routes to the specified propagation route
/// table" (`EnableTransitGatewayRouteTablePropagation`) is what puts one there, so a route of
/// this type is a fact of a propagation and of the address space the propagating attachment's
/// resource holds — never of a request that named a destination.
pub const TRANSIT_GATEWAY_ROUTE_TYPE_PROPAGATED: &str = "propagated";

/// How many routes one transit gateway route table holds in this emulator before a create must
/// refuse — CHOSEN, for the reason [`super::MAX_LOCAL_GATEWAY_ROUTES_PER_TABLE`] is: the routes
/// of a table are records in this process's memory, and nothing reclaims them, so a write path
/// that did not ask would let one caller grow the store without limit. It is not AWS's own
/// per-table route quota, which this emulator does not model.
pub const MAX_TRANSIT_GATEWAY_ROUTES_PER_TABLE: usize = 1000;

/// The bytes one stored transit gateway route costs: the fixed [`STORED_RECORD_OVERHEAD_BYTES`]
/// every stored record costs plus the caller-chosen text it holds — the destination that names
/// it and the ids of the attachment it forwards to. Its type and state are members of their
/// catalogs, so they are fixed-size.
///
/// The ONE definition of that charge: `CreateTransitGatewayRoute` asks the byte budget about
/// exactly this figure, [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_route`] charges
/// it, and every path that removes a route gives back the same number.
pub fn transit_gateway_route_payload_bytes(route: &TransitGatewayRoute) -> usize {
    let text = |value: Option<&str>| value.map_or(0, str::len);
    let attachments = route
        .transit_gateway_attachments
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|attachment| {
            text(attachment.transit_gateway_attachment_id.as_deref())
                .saturating_add(text(attachment.resource_id.as_deref()))
                .saturating_add(text(attachment.resource_type.as_deref()))
        })
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(text(route.destination_cidr_block.as_deref()))
        .saturating_add(attachments)
}

/// Refuse a route this store cannot take: HTTP 400 [`ROUTE_LIMIT_EXCEEDED`], naming the two
/// bounds a route is held against here ([`MAX_TRANSIT_GATEWAY_ROUTES_PER_TABLE`] and
/// [`MAX_STORED_BYTES`]), the way the local gateway route store names its own.
pub fn transit_gateway_route_limit_exceeded<E>(route_table_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        ROUTE_LIMIT_EXCEEDED,
        400,
        format!(
            "The transit gateway route table '{route_table_id}' holds at most \
             {MAX_TRANSIT_GATEWAY_ROUTES_PER_TABLE} routes, and this emulator at most \
             {MAX_STORED_BYTES} bytes of caller-supplied data; this request would take it past \
             one of the two"
        ),
    )
}

/// What one transit gateway route is written with: the table it goes in, the destination that
/// names it there, and the attachment it forwards to — `None` for a route that drops the
/// traffic matching it.
///
/// Everything else the stored route reports — its type and its state — is decided by
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_route`] rather than taken from a
/// caller, so no two writers can report one route differently.
#[derive(Debug, Clone)]
pub struct TransitGatewayRouteSpec {
    /// "The ID of the transit gateway route table" the route is created in.
    pub transit_gateway_route_table_id: String,
    /// "The CIDR range used for destination matches", in the canonical form
    /// [`super::canonical_cidr_block`] gives it, which is what names the route in its table.
    pub destination_cidr_block: String,
    /// The attachment the route forwards matching traffic to, as a report of the route names
    /// it; `None` for a blackhole route, which forwards to nothing.
    pub attachment: Option<TransitGatewayRouteAttachment>,
}

/// The values the EC2 filter `name` compares against the transit gateway route `route`, for the
/// filters that compare an ATTRIBUTE of the route — or of one of its attachments — as text.
///
/// The ONE such reader for this crate: every operation that narrows the routes of a transit
/// gateway route table (`SearchTransitGatewayRoutes` and `ExportTransitGatewayRoutes`) publishes
/// the same attribute names out of one union, so what a name compares against belongs here
/// rather than once per operation, where two copies could answer one name differently. What
/// stays with the operation is which of these names it DOCUMENTS at all: a name its own
/// `Filters` doc does not list is refused by that operation before anything is compared.
///
/// A filter naming an attribute this route does not carry compares against NO values, so it
/// selects nothing — the same answer AWS gives for any attribute a resource holds no value for.
/// The `attachment.*` names compare against EVERY attachment of the route, because one filter
/// value matching any of them is what "the id of the transit gateway attachment" asks. The
/// `route-search.*` names are not here: they relate CIDR BLOCKS rather than text, and are
/// answered by the crate's shared [`super::RouteSearch`].
pub fn transit_gateway_route_filter_values<'a>(
    route: &'a TransitGatewayRoute,
    name: &str,
) -> Vec<::std::borrow::Cow<'a, str>> {
    use ::std::borrow::Cow;

    let text = |value: Option<&'a str>| -> Vec<Cow<'a, str>> {
        value.map(Cow::Borrowed).into_iter().collect()
    };
    let attachments = || {
        route
            .transit_gateway_attachments
            .as_deref()
            .unwrap_or_default()
    };
    let of_each = |member: fn(&'a TransitGatewayRouteAttachment) -> Option<&'a str>| {
        attachments()
            .iter()
            .filter_map(member)
            .map(Cow::Borrowed)
            .collect()
    };
    match name {
        // "attachment.transit-gateway-attachment-id - The id of the transit gateway attachment."
        "attachment.transit-gateway-attachment-id" => {
            of_each(|attachment| attachment.transit_gateway_attachment_id.as_deref())
        }
        // "attachment.resource-id - The resource id of the transit gateway attachment."
        "attachment.resource-id" => of_each(|attachment| attachment.resource_id.as_deref()),
        // "attachment.resource-type - The attachment resource type."
        "attachment.resource-type" => of_each(|attachment| attachment.resource_type.as_deref()),
        // "prefix-list-id - The ID of the prefix list": the prefix list a route that came from
        // a prefix list reference names (`TransitGatewayRoute$PrefixListId`).
        "prefix-list-id" => text(route.prefix_list_id.as_deref()),
        // "state - The state of the route (active | blackhole)."
        "state" => text(route.state.as_deref()),
        // "transit-gateway-route-destination-cidr-block - The CIDR range": the destination the
        // route matches, which is also what names it inside its table.
        "transit-gateway-route-destination-cidr-block" => {
            text(route.destination_cidr_block.as_deref())
        }
        // "type - The type of route (propagated | static)."
        "type" => text(route.r#type.as_deref()),
        // A name no operation documents compares against nothing, so it can never widen a
        // result: each caller has already refused it before reaching here.
        _ => Vec::new(),
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The route the transit gateway route table `route_table_id` holds for `destination`, if
    /// it holds one.
    ///
    /// The ONE lookup of a transit gateway route, so every reader of one asks the same
    /// question: the destination is matched exactly, in the canonical form it is stored under,
    /// so two spellings of one address prefix reach the one route it names.
    pub fn transit_gateway_route(
        &self,
        route_table_id: &str,
        destination: &str,
    ) -> Option<&TransitGatewayRoute> {
        self.transit_gateway_routes
            .get(&(route_table_id.to_string(), destination.to_string()))
    }

    /// How many routes the transit gateway route table `route_table_id` holds.
    pub fn transit_gateway_route_count(&self, route_table_id: &str) -> usize {
        self.transit_gateway_routes_of(route_table_id).count()
    }

    /// Store one static route in the transit gateway route table its spec names, and answer it
    /// as the store now holds it — or `None`, with NOTHING written, when the account holds no
    /// such table, that table already holds a route for the destination, or the route does not
    /// fit inside the store's bounds.
    ///
    /// The ONE place a transit gateway route is built, so the create and every later reader of
    /// one report the same member set. What the request does not decide is decided here:
    ///
    /// * the type, which "Creates a static route for the specified transit gateway route table"
    ///   (`CreateTransitGatewayRoute`) fixes at [`TRANSIT_GATEWAY_ROUTE_TYPE_STATIC`];
    /// * the state, which is [`TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE`] for a route that drops
    ///   the traffic it matches and [`TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE`] for one that
    ///   forwards it — both terminal, so the clock owes a created route no later move;
    /// * `PrefixListId` and `TransitGatewayRouteTableAnnouncementId`, which name the OTHER two
    ///   ways a route reaches a table (a prefix list reference and a route table announcement)
    ///   and so are left unset on a route a caller named a CIDR range for.
    ///
    /// Like every other insert of this crate, the caller-chosen bytes of the record are charged
    /// against the store's byte budget here, so no path can store a route uncharged.
    pub fn insert_transit_gateway_route(
        &mut self,
        spec: TransitGatewayRouteSpec,
    ) -> Option<TransitGatewayRoute> {
        if !self
            .transit_gateway_route_tables
            .contains_key(&spec.transit_gateway_route_table_id)
        {
            return None;
        }
        let key = (
            spec.transit_gateway_route_table_id.clone(),
            spec.destination_cidr_block.clone(),
        );
        if self.transit_gateway_routes.contains_key(&key)
            || self.transit_gateway_routes.len() >= MAX_STORED_RECORDS_PER_STORE
            // Counted only as far as the ceiling, so a write into a full table costs the
            // ceiling rather than the whole range of that table's keys.
            || self
                .transit_gateway_routes_of(&spec.transit_gateway_route_table_id)
                .take(MAX_TRANSIT_GATEWAY_ROUTES_PER_TABLE)
                .count()
                >= MAX_TRANSIT_GATEWAY_ROUTES_PER_TABLE
        {
            return None;
        }
        let blackhole = spec.attachment.is_none();
        let route = TransitGatewayRoute {
            destination_cidr_block: Some(spec.destination_cidr_block),
            prefix_list_id: None,
            transit_gateway_route_table_announcement_id: None,
            // "The attachments" (`TransitGatewayRoute$TransitGatewayAttachments`): the one the
            // route forwards to, and nothing at all for a blackhole route — a dropped packet
            // reaches no attachment.
            transit_gateway_attachments: spec.attachment.map(|attachment| vec![attachment]),
            r#type: Some(TRANSIT_GATEWAY_ROUTE_TYPE_STATIC.to_string()),
            state: Some(
                if blackhole {
                    TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE
                } else {
                    TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE
                }
                .to_string(),
            ),
        };
        let payload_bytes = transit_gateway_route_payload_bytes(&route);
        if self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        // The write FIRST and the charge after it, so the budget can never be charged for a
        // route that was not stored.
        self.transit_gateway_routes.insert(key, route.clone());
        self.add_stored_bytes(payload_bytes);
        Some(route)
    }

    /// The routes of one transit gateway route table, in destination order — the stable order
    /// `SearchTransitGatewayRoutes` reports and pages over — each with the destination it is
    /// named by in that table.
    ///
    /// The store is keyed by (route table, destination), so this is a RANGE over the keys of
    /// one table rather than a scan of every route stored, exactly as
    /// [`Self::transit_gateway_prefix_list_references_of`] is.
    ///
    /// A read, so it writes nothing.
    pub fn transit_gateway_routes_of<'a>(
        &'a self,
        route_table_id: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a TransitGatewayRoute)> {
        self.transit_gateway_routes
            .range((route_table_id.to_string(), String::new())..)
            .take_while(move |((table, _), _)| table == route_table_id)
            .map(|((_, destination), route)| (destination.as_str(), route))
    }

    /// Take every route of the transit gateway route table `route_table_id` out of the store.
    ///
    /// A route is a resource OF its table ("Creates a static route for the specified transit
    /// gateway route table"), so it cannot outlive the table it is in: a table a delete removed
    /// holds no routes, and leaving them would keep their bytes charged against the store's
    /// budget for ever and let a route table id EC2 later re-issued inherit them. The bytes
    /// given back are exactly what [`Self::insert_transit_gateway_route`] charged.
    ///
    /// Called by every path that removes a transit gateway route table — the delete of one
    /// table, and the delete of the gateway that takes its tables with it — so a route can
    /// never be orphaned by one of them.
    pub fn remove_transit_gateway_routes_of(&mut self, route_table_id: &str) {
        let held: BTreeMap<(String, String), TransitGatewayRoute> = self
            .transit_gateway_routes
            .range((route_table_id.to_string(), String::new())..)
            .take_while(|((table, _), _)| table == route_table_id)
            .map(|(key, route)| (key.clone(), route.clone()))
            .collect();
        for (key, route) in held {
            if self.transit_gateway_routes.remove(&key).is_some() {
                self.release_stored_bytes(transit_gateway_route_payload_bytes(&route));
            }
        }
    }

    /// The address prefixes the resource of the transit gateway attachment `attachment_id`
    /// announces into every route table it propagates into.
    ///
    /// "Enables the specified attachment to PROPAGATE ROUTES to the specified propagation route
    /// table" (`EnableTransitGatewayRouteTablePropagation`): what propagates is the address
    /// space of the resource the attachment attaches, so this reads that resource out of the
    /// store it lives in rather than taking prefixes from any request.
    ///
    /// An attachment carries traffic only while it is in service, so one that is not
    /// `available` announces nothing — a propagation left behind by an attachment that is
    /// being or has been deleted puts no route in a table.
    ///
    /// A kind whose announced prefixes this emulator does not hold announces NONE rather than
    /// a guess: a peering attachment's prefixes are the ones the peer's route table
    /// announcement advertises, a Connect attachment's are the ones its Connect peers announce
    /// over BGP, and a VPN concentrator's are the ones its connections announce — none of
    /// which is recorded against the attachment here.
    fn propagated_prefixes(&self, attachment_id: &str) -> Vec<String> {
        let Some(record) = self.transit_gateway_attachments.get(attachment_id) else {
            return Vec::new();
        };
        if record.view.state() != Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE) {
            return Vec::new();
        }
        match &record.view {
            // A VPC attachment announces the CIDR blocks of the VPC it attaches, in the order
            // that VPC reports them and only while it still HOLDS each one (the reading
            // [`held_ipv4_cidr_blocks`] takes of an association on its way out).
            TransitGatewayAttachmentView::Vpc(attachment) => {
                let Some(vpc) = attachment
                    .vpc_id
                    .as_deref()
                    .and_then(|vpc_id| self.vpcs.get(vpc_id))
                else {
                    return Vec::new();
                };
                let mut blocks = held_ipv4_cidr_blocks(vpc);
                blocks.extend(held_ipv6_cidr_blocks(vpc));
                blocks
            }
            // "The IPv4 address range, in CIDR notation, from which client IP addresses are
            // assigned" (`ClientVpnEndpoint$ClientCidrBlock`): the one range a Client VPN
            // endpoint holds, and so the one a Client VPN attachment announces.
            TransitGatewayAttachmentView::ClientVpn(attachment) => attachment
                .client_vpn_endpoint_id
                .as_deref()
                .and_then(|endpoint_id| self.client_vpn_endpoints.get(endpoint_id))
                .and_then(|endpoint| endpoint.client_cidr_block.clone())
                .into_iter()
                .collect(),
            TransitGatewayAttachmentView::Peering(_)
            | TransitGatewayAttachmentView::Connect(_)
            | TransitGatewayAttachmentView::VpnConcentrator(_) => Vec::new(),
        }
    }

    /// The routes the transit gateway route table `route_table_id` holds NOW, in destination
    /// order: the static routes a caller created in it, and the `propagated` routes its
    /// propagations put there.
    ///
    /// The ONE answer to "what is in this route table", so `SearchTransitGatewayRoutes` reports
    /// a propagated prefix in the tables that propagate it and in no other:
    ///
    /// * a propagation contributes routes only once it is in force
    ///   ([`super::PROPAGATION_STATE_ENABLED`]) — one that is still `enabling`, or finishing a
    ///   disable, has not put its routes in the table — and each route it contributes names the
    ///   propagating attachment, that attachment's own resource and resource type, exactly as
    ///   the propagation reports them;
    /// * one prefix announced by SEVERAL attachments is ONE route of the table naming each of
    ///   them, because a route is named by the destination it matches inside one table and
    ///   "the attachments" (`TransitGatewayRoute$TransitGatewayAttachments`) is a list — in the
    ///   attachment-id order the propagations come out in, which is also what decides the one
    ///   route table announcement such a route reports: the FIRST announcement of that order
    ///   which announced the prefix, rather than whichever propagation was read first;
    /// * a destination the table holds a STATIC route for is reported as that static route: a
    ///   route a caller created is the table's own entry for its destination, which is why a
    ///   create is refused for a destination already stored while a propagated prefix cannot
    ///   stand in the way of one.
    ///
    /// A read, so it writes nothing and stores nothing: a propagated route is a fact of the
    /// propagation and of the address space its resource holds, so it follows both without a
    /// second copy that could drift from either. The caller applies the clock-owed moves
    /// ([`Self::materialize_due_transitions`]) before asking, exactly as every describe of this
    /// crate does.
    pub fn transit_gateway_route_set(
        &self,
        route_table_id: &str,
    ) -> Vec<(String, TransitGatewayRoute)> {
        let mut routes: BTreeMap<String, TransitGatewayRoute> = BTreeMap::new();
        for propagation in self
            .transit_gateway_route_table_propagations(route_table_id)
            .filter(|propagation| propagation.state == PROPAGATION_STATE_ENABLED)
        {
            let attachment = TransitGatewayRouteAttachment {
                transit_gateway_attachment_id: Some(
                    propagation.transit_gateway_attachment_id.clone(),
                ),
                resource_id: propagation.resource_id.clone(),
                resource_type: Some(propagation.resource_type.clone()),
            };
            for prefix in self.propagated_prefixes(&propagation.transit_gateway_attachment_id) {
                // Named by the destination in the SAME canonical form a static route of the
                // table is keyed by, so the two kinds of route cannot name one prefix
                // differently and a static route for it is recognised as the table's entry.
                let destination = canonical_cidr_block(&prefix).unwrap_or(prefix);
                let route = routes.entry(destination.clone()).or_insert_with(|| {
                    TransitGatewayRoute {
                        destination_cidr_block: Some(destination),
                        prefix_list_id: None,
                        transit_gateway_route_table_announcement_id: None,
                        transit_gateway_attachments: Some(Vec::new()),
                        r#type: Some(TRANSIT_GATEWAY_ROUTE_TYPE_PROPAGATED.to_string()),
                        // A propagated prefix is being carried to its attachment, which is
                        // what `active` says; nothing about a propagation drops traffic, so
                        // no propagated route is a blackhole.
                        state: Some(TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE.to_string()),
                    }
                });
                // How this route reached the table, for a propagation asked for through a
                // route table announcement
                // (`TransitGatewayRoute$TransitGatewayRouteTableAnnouncementId`): the first
                // announcement, in the attachment-id order above, that announced this prefix.
                // A propagation naming none leaves what an earlier one recorded, so the member
                // reports an announcement the route really came through rather than the
                // absence of one on another propagation of the same prefix.
                if route.transit_gateway_route_table_announcement_id.is_none() {
                    route.transit_gateway_route_table_announcement_id = propagation
                        .transit_gateway_route_table_announcement_id
                        .clone();
                }
                route
                    .transit_gateway_attachments
                    .get_or_insert_with(Vec::new)
                    .push(attachment.clone());
            }
        }
        for (destination, route) in self.transit_gateway_routes_of(route_table_id) {
            routes.insert(destination.to_string(), route.clone());
        }
        routes.into_iter().collect()
    }
}
