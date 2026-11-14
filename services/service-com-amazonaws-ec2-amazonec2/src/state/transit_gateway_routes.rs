// SPDX-License-Identifier: CC0-1.0

//! The static routes of a transit gateway route table: what one is stored as, the states and
//! types its catalogs admit, what a create is refused with, the store's bounds and its one
//! writer.
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
    ROUTE_LIMIT_EXCEEDED, STORED_RECORD_OVERHEAD_BYTES,
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
/// of this crate writes.
pub const TRANSIT_GATEWAY_ROUTE_TYPE_STATIC: &str = "static";

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
}
