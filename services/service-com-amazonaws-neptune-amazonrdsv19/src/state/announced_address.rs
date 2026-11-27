// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a DB cluster and its instances ANNOUNCE for a client to connect to.
//!
//! `DBCluster$Endpoint` is "the connection endpoint for the primary instance of the DB
//! cluster" and `Endpoint$Address` is "the DNS address of the DB instance": the HOST a client
//! reaches the graph at, because the Neptune data plane is addressed at the cluster's own
//! endpoint and nowhere else. A host this process does not answer on makes the whole data
//! plane unreachable, so the host announced is the one this emulator is REACHED at whenever
//! it has one, and the AWS-shaped DNS name only on an emulator that is served on no port at
//! all — an in-process call with no socket behind it, where there is no address of this
//! process to announce.
//!
//! `DBCluster$Port` and `Endpoint$Port` are "the port that the database engine is listening
//! on", and that is a value the CREATE configures: `CreateDBCluster$Port` is "the port number
//! on which the instances in the DB cluster accept connections" with "Default: 8182", so the
//! port announced is the one the request asked for, or that default when it asked for none.
//! It is a stored property of the resource — the same on every read of it and across
//! restarts — where the port this process happens to be bound to is a property of the
//! PROCESS, differing between two clusters and between two boots, which a client could
//! neither reconcile against the value it sent nor cache.
//!
//! WHICH address is this emulator's own is the tree's one reading of that question
//! (`service_common::data_plane_addressing::reached_emulator_host_and_port`), so this
//! service writes no second rule for it.

/// The hosts and the port one cluster announces: `DBCluster$Endpoint`,
/// `DBCluster$ReaderEndpoint` and `DBCluster$Port`, resolved once when the cluster is
/// created and stored on its record, so every later read reports the same values.
#[derive(Debug, Clone)]
pub struct ClusterAddress {
    pub endpoint: String,
    pub reader_endpoint: String,
    pub port: i32,
}

/// The `host` and `port` this emulator is reached at, or `None` when no request addressed
/// it directly and it is served on no port at all — the one case where there is no address
/// of this process to announce.
pub fn served_address(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<(String, i32)> {
    ::service_common::data_plane_addressing::reached_emulator_host_and_port(
        ctx.parts(),
        ctx.endpoint().as_str(),
    )
    .map(|(host, port)| (host, i32::from(port)))
}

/// The address a CLUSTER announces.
///
/// One process serves every cluster's data plane, so on a served emulator the writer and
/// the reader endpoint are the same host — it is the only address a client can reach. The
/// port is the cluster's own configured one either way: the value the request named, else the
/// engine default this control plane publishes ([`super::DEFAULT_PORT`]).
pub fn announced_cluster_address(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    identifier: &str,
    endpoint_token: &str,
    region: &str,
    requested_port: Option<i32>,
) -> ClusterAddress {
    let port = requested_port.unwrap_or(super::DEFAULT_PORT);
    match served_address(ctx) {
        Some((host, _)) => ClusterAddress {
            endpoint: host.clone(),
            reader_endpoint: host,
            port,
        },
        None => ClusterAddress {
            endpoint: cluster_dns_name(identifier, endpoint_token, region, "cluster-"),
            reader_endpoint: cluster_dns_name(identifier, endpoint_token, region, "cluster-ro-"),
            port,
        },
    }
}

/// The address an INSTANCE of a cluster announces: the host its cluster is reached at, since
/// one process serves both, and its CLUSTER's port.
///
/// Every instance of this engine belongs to a cluster, and `CreateDBInstance$Port` says of
/// such an instance "Not applicable. The port is managed by the DB cluster", so the port a
/// client reaches a member at is the cluster's and never one of the member's own.
pub fn announced_instance_address(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    identifier: &str,
    endpoint_token: &str,
    region: &str,
    cluster_port: i32,
) -> (String, i32) {
    match served_address(ctx) {
        Some((host, _)) => (host, cluster_port),
        None => (
            // An instance in a cluster inherits the cluster's endpoint shard label; only
            // its own DNS label differs.
            format!(
                "{identifier}.{endpoint_token}.{region}.{}",
                super::ENDPOINT_DOMAIN
            ),
            cluster_port,
        ),
    }
}

/// One of a cluster's AWS-shaped endpoint host names: the identifier, the shard label the
/// cluster minted behind `role` (`cluster-` for the writer, `cluster-ro-` for the reader),
/// and the region's DNS suffix under this engine's endpoint domain.
fn cluster_dns_name(identifier: &str, endpoint_token: &str, region: &str, role: &str) -> String {
    format!(
        "{identifier}.{role}{endpoint_token}.{region}.{}",
        super::ENDPOINT_DOMAIN
    )
}
