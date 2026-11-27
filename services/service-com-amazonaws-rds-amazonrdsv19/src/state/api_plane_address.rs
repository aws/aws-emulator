// SPDX-License-Identifier: CC0-1.0

//! The address a DB cluster or DB instance publishes when its DATA PLANE is an AWS API this
//! emulator serves at its OWN endpoint, rather than a native engine wire on a listener of
//! its own.
//!
//! A cluster's `Endpoint` is "the connection endpoint for the primary instance of the DB
//! cluster" and its `Port` is "the port that the database engine is listening on". For the
//! engines whose native wire this emulator speaks the pair is the resource's own listener
//! ([`super::data_plane`]): the host it is bound to and the port it was assigned. An Amazon
//! Neptune cluster's data plane is not a native wire at all: it is the graph API (restJson1,
//! signing name `neptune-db`), which has no regional endpoint of its own and is addressed at
//! the cluster's connection endpoint and nowhere else — and this emulator serves that API on
//! its own endpoint. Such a cluster therefore publishes the HOST this process is reached at,
//! for every one of its endpoints — its two members and each custom endpoint in front of it —
//! because a name published under the engine's AWS DNS domain
//! ([`super::endpoint_domain`]) resolves to nothing here and would be no connection endpoint
//! at all.
//!
//! Its `Port` is NOT that address's. `Port` is "the port that the database engine is
//! listening on", a value the CREATE configures and a modification moves, so it is a stored
//! property of the resource: identical on every read of it and across restarts. The port this
//! process is bound to is a property of the PROCESS — it differs between two resources and
//! between two boots — so publishing it as `Port` would answer a client a value it never
//! configured and could not cache. The address this emulator answers the API at is therefore
//! published through the HOST alone, and the resource keeps its own configured port
//! ([`super::DbClusterRecord::published_port`]). Only the port of a listener opened for a
//! resource's own engine WIRE replaces it, because there the port is that engine's.
//!
//! WHICH address is this emulator's own is the tree's one reading of that question
//! (`service_common::data_plane_addressing::reached_emulator_host_and_port`), so this
//! service writes no second rule for it; what lives here is only which engines are served
//! that way.

use super::{ServedEndpoint, NEPTUNE_ENGINE};

/// Whether the data plane of `engine` is an AWS API this emulator serves at its own
/// endpoint instead of a native wire of its own.
///
/// Amazon Neptune's is: `CreateDBCluster$Engine` accepts `neptune`, and a Neptune cluster's
/// graph queries, bulk loader, statistics, ML endpoints and fast reset are all operations
/// of its data-plane API, which this emulator serves.
#[must_use]
pub fn engine_data_plane_is_an_api(engine: &str) -> bool {
    engine.eq_ignore_ascii_case(NEPTUNE_ENGINE)
}

/// The address a cluster or instance running `engine` publishes, for an engine whose data
/// plane this emulator serves as an API at its own endpoint: the HOST this request reached
/// this emulator at, which is the host that plane is served on and the one member such a
/// resource publishes it through — its `Port` stays the port the resource was configured on
/// ([`super::DbClusterRecord::published_port`]).
///
/// The port this process was reached on is carried along for the readers that need the whole
/// address of the plane rather than the resource's endpoint pair (a custom endpoint's single
/// `Endpoint` string, which has no port member of its own).
///
/// `None` for an engine whose data plane is a native wire (the resource's own listener is
/// its address), and for an emulator served on no port at all that no request addressed
/// directly — an in-process call with no socket behind it, where there is no address of this
/// process to publish, so the resource keeps its AWS DNS-form endpoint.
#[must_use]
pub fn api_plane_endpoint(
    engine: &str,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<ServedEndpoint> {
    if !engine_data_plane_is_an_api(engine) {
        return None;
    }
    let (host, port) = ::service_common::data_plane_addressing::reached_emulator_host_and_port(
        ctx.parts(),
        ctx.endpoint().as_str(),
    )?;
    Some(ServedEndpoint {
        host,
        port: i32::from(port),
    })
}
