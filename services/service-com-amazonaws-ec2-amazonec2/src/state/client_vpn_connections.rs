// SPDX-License-Identifier: CC0-1.0

//! The client connections of a Client VPN endpoint: the store they live in, the status codes
//! one moves through, the refusals for a connection or a user an endpoint does not have, and
//! the one writer of a termination.
//!
//! A submodule of [`crate::state`] beside [`super::client_vpn`], which holds the endpoints
//! themselves, their target networks and their routes: a connection is a resource of its own
//! — named by its own `ConnectionId`, carrying its own `ClientVpnConnectionStatus` and the
//! `terminating` -> `terminated` move the clock owes it — so its store, its refusals and its
//! writer live in their own file. Everything here is re-exported by `crate::state`, so a
//! handler reaches it as `crate::state::client_vpn_connection` exactly as it reaches the
//! helpers of the module root.
//!
//! The store itself is a field of [`ComAmazonawsEc2Amazonec2State`]
//! (`client_vpn_connections`), because the state struct is one struct; it is only the code
//! that reads and writes it that lives here.

use super::*;

/// The id prefix EC2 mints a Client VPN client connection with: `cvpn-connection-`, as the
/// AWS Client VPN documentation's own connection ids read (for example
/// `cvpn-connection-0e520c7b7c9b4d1e2`). The model puts no pattern on the member
/// (`smithy_query(query="operation",operation="TerminateClientVpnConnections")` shows
/// `ConnectionId` targeting a plain `String`), so the prefix is the one thing that says
/// which resource an id names.
///
/// As with [`CLIENT_VPN_ENDPOINT_ID_PREFIX`], the prefix is what an id of this kind READS
/// as rather than a constraint this surface enforces: a connection is resolved by LOOKUP, so
/// a value of any form that the endpoint holds no connection for earns the one refusal
/// [`invalid_client_vpn_connection_id_not_found`] writes.
pub const CLIENT_VPN_CONNECTION_ID_PREFIX: &str = "cvpn-connection";

/// "active" — "The state of the client connection" while the client is connected
/// (`ClientVpnConnectionStatusCode`, whose values the generated
/// `CLIENT_VPN_CONNECTION_STATUS_CODE_VALUES` catalog spells). It is the state a connection
/// is established in, and the only state [`ComAmazonawsEc2Amazonec2State::
/// terminate_client_vpn_connection`] moves one out of.
pub const CLIENT_VPN_CONNECTION_STATUS_ACTIVE: &str = "active";

/// "terminating" — the state `TerminateClientVpnConnections` answers with for a connection it
/// has begun terminating (`ClientVpnConnectionStatusCode`).
pub const CLIENT_VPN_CONNECTION_STATUS_TERMINATING: &str = "terminating";

/// "terminated" — the terminal state the clock owes a `terminating` connection
/// ([`TRANSITION_DELAY`] later, through
/// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const CLIENT_VPN_CONNECTION_STATUS_TERMINATED: &str = "terminated";

/// `InvalidClientVpnConnection.IdNotFound` (HTTP 400): the request named a connection id the
/// Client VPN endpoint holds no client connection for, which is the code AWS's EC2 error
/// catalogue documents for a Client VPN connection id it cannot resolve.
pub const INVALID_CLIENT_VPN_CONNECTION_ID_NOT_FOUND: &str =
    "InvalidClientVpnConnection.IdNotFound";

/// `InvalidClientVpnConnection.UserNotFound` (HTTP 400): the request named a user the Client
/// VPN endpoint holds no active client connection for, the catalogue's counterpart code for
/// the other way a connection is named.
pub const INVALID_CLIENT_VPN_CONNECTION_USER_NOT_FOUND: &str =
    "InvalidClientVpnConnection.UserNotFound";

/// The most characters of a refused connection id or user name its message repeats back.
///
/// Both are the request's own bytes — `ConnectionId` and `Username` are bare strings the
/// model puts no `@length` on, and this surface refuses a value of any form by LOOKUP rather
/// than by its form — so the echo is bounded here exactly as
/// [`MAX_ECHOED_CLIENT_VPN_ENDPOINT_ID_CHARS`] bounds a refused endpoint id's. Far above the
/// longest connection id this service mints ([`CLIENT_VPN_CONNECTION_ID_PREFIX`] plus 17
/// digits), so a real id is always named in full.
pub const MAX_ECHOED_CLIENT_VPN_CONNECTION_SELECTOR_CHARS: usize = 128;

/// Refuse a request that named a connection id the endpoint holds no client connection for:
/// HTTP 400 [`INVALID_CLIENT_VPN_CONNECTION_ID_NOT_FOUND`], worded the way this surface words
/// a Client VPN resource it cannot resolve — `<Kind> <value> does not exist`, the wording
/// [`invalid_client_vpn_endpoint_id_not_found`] answers an unresolvable endpoint id with — of
/// which the first [`MAX_ECHOED_CLIENT_VPN_CONNECTION_SELECTOR_CHARS`] characters are named.
pub fn invalid_client_vpn_connection_id_not_found<E>(connection_id: &str) -> Scaffolded<E> {
    let echoed: String = connection_id
        .chars()
        .take(MAX_ECHOED_CLIENT_VPN_CONNECTION_SELECTOR_CHARS)
        .collect();
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_CONNECTION_ID_NOT_FOUND,
        400,
        format!("Connection {echoed} does not exist"),
    )
}

/// Refuse a request that named a user the endpoint holds no active client connection for:
/// HTTP 400 [`INVALID_CLIENT_VPN_CONNECTION_USER_NOT_FOUND`], in that same wording.
pub fn invalid_client_vpn_connection_user_not_found<E>(username: &str) -> Scaffolded<E> {
    let echoed: String = username
        .chars()
        .take(MAX_ECHOED_CLIENT_VPN_CONNECTION_SELECTOR_CHARS)
        .collect();
    Scaffolded::service_error(
        INVALID_CLIENT_VPN_CONNECTION_USER_NOT_FOUND,
        400,
        format!("User {echoed} does not exist"),
    )
}

/// One client connection of a Client VPN endpoint: the modelled
/// [`crate::ops::types::ClientVpnConnection`] every operation that reports one answers with,
/// plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// of this subject holds one: a member a writer sets round-trips through
/// `DescribeClientVpnConnections` without a field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct ClientVpnConnectionRecord {
    /// The connection as the model shapes it, carrying the status reached so far.
    pub view: crate::ops::types::ClientVpnConnection,
    /// The `ClientVpnConnectionStatusCode` the clock owes this connection while it is still
    /// transient (`terminating` -> `terminated`); `None` once it has been applied.
    pub pending: Option<PendingStateMove>,
}

/// What one client connection is written with: the endpoint the client connected to, who
/// connected, the address the endpoint assigned the client, and when the connection was
/// established.
///
/// Every member the service itself owns — the connection's id and the status it starts in —
/// is derived by [`ComAmazonawsEc2Amazonec2State::insert_client_vpn_connection`] rather than
/// taken from a caller, so two writers cannot report one connection differently.
#[derive(Debug, Clone, Default)]
pub struct ClientVpnConnectionSpec {
    /// "The ID of the Client VPN endpoint to which the client is connected"
    /// (`ClientVpnConnection$ClientVpnEndpointId`).
    pub client_vpn_endpoint_id: String,
    /// "The username of the client who established the client connection. This information
    /// is only provided if Active Directory client authentication is used"
    /// (`ClientVpnConnection$Username`) — `None` for a connection authenticated by
    /// certificate, which names no user.
    pub username: Option<String>,
    /// "The common name associated with the client. This is either the name of the client
    /// certificate, or the Active Directory user name"
    /// (`ClientVpnConnection$CommonName`).
    pub common_name: Option<String>,
    /// "The IP address of the client" (`ClientVpnConnection$ClientIp`), assigned out of the
    /// endpoint's client CIDR range.
    pub client_ip: Option<String>,
    /// When the connection was established, which the record reports as
    /// `ClientVpnConnection$ConnectionEstablishedTime`.
    pub established_at: Option<SystemTime>,
}

/// `at` as EC2 spells the time members of a Client VPN client connection:
/// `YYYY-MM-DD HH:MM:SS` in UTC.
///
/// Those members are modelled as plain strings rather than as timestamps
/// (`smithy_query(query="shape",shape="ClientVpnConnection")` targets `String` for
/// `Timestamp`, `ConnectionEstablishedTime` and `ConnectionEndTime`), and that is the form
/// the Client VPN surface writes them in — a civil date and time separated by a space, with
/// no zone suffix — so it is spelled here once rather than per writer. The calendar
/// conversion is `chrono`'s, like every other civil-calendar rendering in this crate.
pub fn client_vpn_connection_time(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64;
    ::chrono::DateTime::from_timestamp(seconds, 0)
        .map(|at| at.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// The caller-chosen bytes one stored client connection holds: the endpoint it is connected
/// to, who connected, the address assigned to the client and the two times it carries, on
/// top of the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
///
/// The ONE definition of that charge, the way [`client_vpn_route_payload_bytes`] is for the
/// store beside it: the insert charges exactly it, and a removal path must give back the
/// same number.
pub fn client_vpn_connection_payload_bytes(view: &crate::ops::types::ClientVpnConnection) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(view.client_vpn_endpoint_id.as_deref().map_or(0, str::len))
        .saturating_add(view.username.as_deref().map_or(0, str::len))
        .saturating_add(view.common_name.as_deref().map_or(0, str::len))
        .saturating_add(view.client_ip.as_deref().map_or(0, str::len))
        .saturating_add(
            view.connection_established_time
                .as_deref()
                .map_or(0, str::len),
        )
        .saturating_add(view.connection_end_time.as_deref().map_or(0, str::len))
}

/// The status a client connection carries, by its code — the shape both the connection's own
/// `Status` and the `PreviousStatus`/`CurrentStatus` a termination reports are.
fn client_vpn_connection_status(code: &str) -> crate::ops::types::ClientVpnConnectionStatus {
    crate::ops::types::ClientVpnConnectionStatus {
        code: Some(code.to_string()),
        message: None,
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another client connection fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// writer asks this BEFORE inserting, because nothing here reclaims a stored connection.
    pub fn has_client_vpn_connection_capacity(&self) -> bool {
        self.client_vpn_connections.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Record one client connection to a Client VPN endpoint and answer it as a reader
    /// reports it — or `None` when one of the store's bounds would be crossed and NOTHING
    /// was stored.
    ///
    /// A client connection is established by a VPN client over the endpoint's own data plane
    /// rather than by any EC2 call, so this is the seam that data plane and
    /// `DescribeClientVpnConnections` share, and today only this crate's tests write through
    /// it; it is not dead code — it is the ONE way a connection enters the store, exactly as
    /// [`Self::insert_client_vpn_endpoint`] is for the endpoint it belongs to.
    ///
    /// The id and the `active` status are derived here rather than taken from `spec`, so no
    /// path can store a connection under an id this service could not have issued or in a
    /// state a client never connected in.
    pub fn insert_client_vpn_connection(
        &mut self,
        spec: ClientVpnConnectionSpec,
    ) -> Option<crate::ops::types::ClientVpnConnection> {
        let view = crate::ops::types::ClientVpnConnection {
            client_vpn_endpoint_id: Some(spec.client_vpn_endpoint_id),
            username: spec.username,
            common_name: spec.common_name,
            client_ip: spec.client_ip,
            connection_established_time: spec.established_at.map(client_vpn_connection_time),
            status: Some(client_vpn_connection_status(
                CLIENT_VPN_CONNECTION_STATUS_ACTIVE,
            )),
            ..crate::ops::types::ClientVpnConnection::default()
        };
        let charge = client_vpn_connection_payload_bytes(&view);
        if !self.has_client_vpn_connection_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let connection_id = self.next_id(CLIENT_VPN_CONNECTION_ID_PREFIX);
        let view = crate::ops::types::ClientVpnConnection {
            connection_id: Some(connection_id.clone()),
            ..view
        };
        self.add_stored_bytes(charge);
        let stored = view.clone();
        self.client_vpn_connections.insert(
            connection_id,
            ClientVpnConnectionRecord {
                view,
                pending: None,
            },
        );
        Some(stored)
    }

    /// The client connection `connection_id` names on `endpoint_id`, or `None` when that
    /// endpoint holds none under that id.
    ///
    /// Scoped to the endpoint the request named: a connection belongs to ONE endpoint
    /// ("The ID of the Client VPN endpoint to which the client is connected" —
    /// `ClientVpnConnection$ClientVpnEndpointId`), so an id that names a connection of
    /// another endpoint is not a connection of this one.
    pub fn client_vpn_connection(
        &self,
        endpoint_id: &str,
        connection_id: &str,
    ) -> Option<&ClientVpnConnectionRecord> {
        self.client_vpn_connections
            .get(connection_id)
            .filter(|record| {
                record.view.client_vpn_endpoint_id.as_deref() == Some(endpoint_id)
            })
    }

    /// The ACTIVE client connections `username` holds on `endpoint_id`, by connection id in
    /// the store's own order.
    ///
    /// "Use this option to terminate all active connections for the specified user"
    /// (`TerminateClientVpnConnectionsRequest$Username`): the user's connections that are
    /// still active are what naming a user selects, so a connection already on its way to
    /// `terminated` is not selected again.
    pub fn active_client_vpn_connections_for_user(
        &self,
        endpoint_id: &str,
        username: &str,
    ) -> Vec<String> {
        self.client_vpn_connections
            .iter()
            .filter(|(_, record)| {
                record.view.client_vpn_endpoint_id.as_deref() == Some(endpoint_id)
                    && record.view.username.as_deref() == Some(username)
                    && client_vpn_connection_status_code(record)
                        == CLIENT_VPN_CONNECTION_STATUS_ACTIVE
            })
            .map(|(connection_id, _)| connection_id.clone())
            .collect()
    }

    /// Begin terminating the client connection `connection_id` and answer the status it held
    /// before this call beside the one it holds after it — or `None` when the store holds no
    /// connection under that id.
    ///
    /// The ONE writer of a termination. A connection that is still `active` moves to
    /// `terminating`, is stamped with the instant it ended ("The date and time the client
    /// connection was terminated" — `ClientVpnConnection$ConnectionEndTime`) and is owed
    /// `terminated` one [`TRANSITION_DELAY`] later through
    /// [`Self::materialize_due_transitions`]: the span is never zero, so a caller reading the
    /// connection straight after the call that terminated it still sees `terminating`. A
    /// connection that is already terminating or terminated is reported UNCHANGED — both
    /// statuses answer the same code — so terminating one twice neither moves a terminal
    /// status back nor charges the store twice.
    pub fn terminate_client_vpn_connection(
        &mut self,
        connection_id: &str,
        at: SystemTime,
    ) -> Option<(
        crate::ops::types::ClientVpnConnectionStatus,
        crate::ops::types::ClientVpnConnectionStatus,
    )> {
        let record = self.client_vpn_connections.get_mut(connection_id)?;
        let previous = client_vpn_connection_status_code(record).to_string();
        if previous != CLIENT_VPN_CONNECTION_STATUS_ACTIVE {
            let held = client_vpn_connection_status(&previous);
            return Some((held.clone(), held));
        }
        let ended_at = client_vpn_connection_time(at);
        // The end time is written onto a record that is ALREADY inside the store's bounds,
        // at most once per connection and of a fixed width, so it is charged without a bound
        // of its own — exactly as [`Self::pin_client_vpn_endpoint_vpc`] charges the id it
        // writes onto a stored endpoint. The bounds belong to the writers that add a RECORD.
        let charge = ended_at.len();
        record.view.connection_end_time = Some(ended_at);
        record.view.status = Some(client_vpn_connection_status(
            CLIENT_VPN_CONNECTION_STATUS_TERMINATING,
        ));
        record.pending = Some(PendingStateMove::starting_now(
            CLIENT_VPN_CONNECTION_STATUS_TERMINATED,
        ));
        self.add_stored_bytes(charge);
        Some((
            client_vpn_connection_status(&previous),
            client_vpn_connection_status(CLIENT_VPN_CONNECTION_STATUS_TERMINATING),
        ))
    }

    /// Apply the `terminating` -> `terminated` move every client connection that has reached
    /// its due instant by `now` owes.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is what a reporting operation
    /// calls: the move was owed at its instant whether or not anybody asked, so applying it
    /// on a read changes nothing a later caller would not have seen anyway.
    pub(crate) fn materialize_due_client_vpn_connections(&mut self, now: Instant) {
        for record in self.client_vpn_connections.values_mut() {
            let due = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string);
            if let Some(code) = due {
                record.view.status = Some(client_vpn_connection_status(&code));
                record.pending = None;
            }
        }
    }
}

/// The `ClientVpnConnectionStatusCode` a stored connection is in.
///
/// A connection is stored `active` by the one writer that stores one, so a record whose
/// status member carries no code is read as the state it was established in rather than as
/// no state at all.
pub fn client_vpn_connection_status_code(record: &ClientVpnConnectionRecord) -> &str {
    record
        .view
        .status
        .as_ref()
        .and_then(|status| status.code.as_deref())
        .unwrap_or(CLIENT_VPN_CONNECTION_STATUS_ACTIVE)
}
