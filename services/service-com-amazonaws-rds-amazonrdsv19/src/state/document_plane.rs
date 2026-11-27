// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENT data plane of a DB CLUSTER whose engine is Amazon DocumentDB's, and of each of
//! its instances: the socket a client connects to and speaks the engine's own wire on.
//!
//! Amazon DocumentDB, Amazon Neptune and Amazon RDS are three views of ONE AWS front door
//! (`src/service.rs`): an AWS SDK DocumentDB client signs `rds`, so a `CreateDBCluster` /
//! `CreateDBInstance` with `Engine: docdb` is served HERE, and the resource it creates is an
//! Amazon DocumentDB cluster. `DBCluster$Endpoint` is "the connection endpoint for the primary
//! instance of the DB cluster" and an instance's `Endpoint{Address,Port}` "the connection
//! endpoint" — so, exactly as for the engines whose SQL wires this service serves
//! ([`super::data_plane`]), such a cluster and each of its instances is given a socket of the
//! EMULATOR's own and publishes the address that socket answers on: a name under Amazon
//! DocumentDB's own DNS zone resolves to nothing on the machine this emulator runs on and would
//! be no connection endpoint at all.
//!
//! What answers there is the ONE implementation of that wire this tree has, which the Amazon
//! DocumentDB control plane's own crate owns
//! (`service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane`), over the
//! documents that crate holds for the CLUSTER — keyed by the cluster's immutable
//! `DbClusterResourceId`, so a cluster created through either view of the door addresses one
//! set of documents, and the cluster's own endpoint and its members' are addresses onto one
//! volume. This module is only the Amazon RDS side of it: which engine is served that way,
//! where the facts a session is judged by are read from (THIS service's records), and the
//! handle the store holds so a port lives exactly as long as the resource it belongs to.

use std::sync::Arc;

use ::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane as document_wire;

use super::{ComAmazonawsRdsAmazonrdsv19State, DbInstanceRecord, RegionalKey, ServedEndpoint};

/// One instance's document-wire socket, as the store holds it. Dropping it closes the port and
/// severs every session on it.
pub type DocumentPlane = document_wire::ServedDataPlane;

/// The address a document-wire socket answers on, as THIS service publishes one: the host the
/// core bound the listener to and the port it assigned, read off the plane the DocumentDB
/// crate opened.
#[must_use]
pub fn published_address(plane: &DocumentPlane) -> ServedEndpoint {
    let endpoint = plane.endpoint();
    ServedEndpoint {
        host: endpoint.host.clone(),
        port: endpoint.port,
    }
}

/// Whether the data plane of `engine` is the document wire this module serves.
///
/// `CreateDBClusterMessage$Engine` accepts `docdb` for an Amazon DocumentDB cluster, whose
/// clients speak the document wire. Every other engine this door accepts is served by its own
/// wire or by none ([`super::engine_wire`], [`super::api_plane_address`]).
#[must_use]
pub fn engine_serves_the_document_wire(engine: &str) -> bool {
    document_wire::engine_speaks_the_document_wire(engine)
}

/// Whether the endpoints of the cluster `named` are ENCRYPTED.
///
/// The engine carries this as a DB CLUSTER parameter, so the answer is the effective value of
/// that parameter for this cluster: what the cluster's own group sets, and the parameter's
/// documented default (`enabled`) where the group sets nothing — which is also the answer for
/// a cluster carrying a default group AWS provides, since such a group sets no parameter of
/// its own. The same reading the DocumentDB view of this door takes.
#[must_use]
fn endpoints_are_encrypted(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    region: &str,
    named: &str,
) -> bool {
    let setting = store
        .resolve_db_cluster(region, named)
        .and_then(|cluster| cluster.db_cluster_parameter_group_name.as_ref())
        .and_then(|name| {
            store
                .db_cluster_parameter_groups
                .get(&(region.to_string(), name.to_ascii_lowercase()))
        })
        .and_then(|group| group.parameters.get(document_wire::TLS_PARAMETER))
        .map(|setting| setting.value.clone());
    !setting.is_some_and(|value| value.eq_ignore_ascii_case(document_wire::TLS_DISABLED))
}

/// The facts a session on the instance `key` names is judged against, read from THIS service's
/// records on every command — so a change the control plane makes while a session is open (a
/// failover moving the writer role, the instance deleted) is in force on its next command.
///
/// The owning cluster is resolved by the immutable `DbClusterResourceId` the plane was opened
/// for, not by the identifier the instance names it with: `ModifyDBCluster` renames a cluster
/// under the live sessions of its members' endpoints, and a lookup that missed would leave the
/// endpoint with no credential to check a login against.
fn instance_facts(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    documents: Option<&::engine::ServiceState<document_wire::ClusterDocuments>>,
    key: &RegionalKey,
    cluster_resource_id: &str,
) -> document_wire::commands::EndpointFacts {
    let store = state.lock();
    let Some(instance) = store.db_instances.get(key) else {
        return document_wire::commands::EndpointFacts {
            exists: false,
            serving: false,
            status: super::DB_INSTANCE_STATUS_DELETING.to_string(),
            is_writer: false,
            engine_version: String::new(),
            credentials: None,
        };
    };
    let cluster = store.cluster_by_resource_id(&key.0, cluster_resource_id);
    document_wire::commands::EndpointFacts {
        exists: true,
        // An instance still being created has no engine to answer out of yet; one that is
        // available serves.
        serving: instance.status == super::DB_INSTANCE_STATUS_AVAILABLE,
        status: instance.status.clone(),
        // Writes go through the member the cluster writes through — the first of its members,
        // which is the one `IsClusterWriter` reports and which `FailoverDBCluster` moves — so
        // every other member serves reads only.
        is_writer: cluster.is_some_and(|cluster| {
            store
                .cluster_members(cluster)
                .first()
                .is_some_and(|writer| writer.identifier == instance.identifier)
        }),
        engine_version: cluster
            .and_then(|cluster| cluster.engine_version.clone())
            .or_else(|| instance.engine_version.clone())
            .unwrap_or_default(),
        // The credential a client authenticates against is the CLUSTER's, held with the
        // cluster's documents under its immutable resource id, so both views of this door
        // check a login against the one credential.
        credentials: cluster.and_then(|cluster| {
            documents?.lock().credentials(&cluster.resource_id)
        }),
    }
}

/// Open the socket the instance `record` is served on, for an instance of an engine whose data
/// plane is the document wire, and report it.
///
/// `None` for every other engine, for an instance that names no cluster — a DocumentDB
/// instance is always a member of one — and for an engine that vends no listeners, in which
/// case the instance keeps the DNS-form endpoint this service has always reported.
///
/// Called with NO store guard held: opening a socket is asynchronous, and this service holds
/// its store's lock across no await. The caller publishes
/// [`DocumentPlane::endpoint`] as the instance's `Endpoint` and keeps the handle on the store,
/// which is what holds the port open.
pub async fn open_document_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    record: &DbInstanceRecord,
) -> Option<DocumentPlane> {
    if !engine_serves_the_document_wire(&record.engine) {
        return None;
    }
    let documents = document_wire::ClusterDocuments::of(engine);
    let named = record.db_cluster_identifier.clone()?;
    let (resource_id, volume, encrypted) = {
        let store = state.lock();
        let cluster = store.resolve_db_cluster(&record.region, &named)?;
        let resource_id = cluster.resource_id.clone();
        let encrypted = endpoints_are_encrypted(&store, &record.region, &named);
        // The cluster's storage is taken under this service's own store, the lock order the
        // shared register fixes.
        (
            resource_id.clone(),
            documents.as_ref()?.lock().volume(&resource_id),
            encrypted,
        )
    };
    let key: RegionalKey = (record.region.clone(), record.identifier.clone());
    let facts: document_wire::FactsSource = {
        let state = state.clone();
        let documents = documents.clone();
        Arc::new(move || instance_facts(&state, documents.as_ref(), &key, &resource_id))
    };
    document_wire::open_instance_data_plane(engine, facts, volume, encrypted).await
}

/// The facts a session on a CLUSTER's own endpoint is judged against, read from THIS service's
/// records on every command.
///
/// The cluster is found by its immutable `DbClusterResourceId`, because `ModifyDBCluster`
/// renames a cluster under the live sessions of its endpoints and the resource id is what does
/// not move.
///
/// `DBCluster$Endpoint` is "the connection endpoint for the primary instance of the DB
/// cluster", so what answers here answers as that primary: writes are accepted, and the
/// endpoint serves only while the cluster is available and the member it writes through is —
/// a cluster with no member yet has no engine behind its endpoint, exactly as one still being
/// created has none.
fn cluster_facts(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    documents: Option<&::engine::ServiceState<document_wire::ClusterDocuments>>,
    resource_id: &str,
) -> document_wire::commands::EndpointFacts {
    let store = state.lock();
    let Some(cluster) = store
        .db_clusters
        .values()
        .find(|cluster| cluster.resource_id == resource_id)
    else {
        return document_wire::commands::EndpointFacts {
            exists: false,
            serving: false,
            status: super::DB_CLUSTER_STATUS_DELETING.to_string(),
            is_writer: false,
            engine_version: String::new(),
            credentials: None,
        };
    };
    // The member the cluster writes through, which is the instance this endpoint leads to.
    let writer = store.cluster_members(cluster).into_iter().next();
    document_wire::commands::EndpointFacts {
        exists: true,
        serving: cluster.status == super::DB_CLUSTER_STATUS_AVAILABLE
            && writer.is_some_and(|writer| {
                writer.status == super::DB_INSTANCE_STATUS_AVAILABLE
            }),
        // Why a session is not served, when it is not: the primary's own status where the
        // cluster has one, and the cluster's own while it has no member to lead to.
        status: writer.map_or_else(
            || cluster.status.clone(),
            |writer| writer.status.clone(),
        ),
        is_writer: true,
        engine_version: cluster.engine_version.clone().unwrap_or_default(),
        // The credential a client authenticates against is the CLUSTER's, held with the
        // cluster's documents under its immutable resource id, so both views of this door
        // check a login against the one credential.
        credentials: documents.and_then(|documents| {
            documents.lock().credentials(&cluster.resource_id)
        }),
    }
}

/// Open the socket the CLUSTER `record` is served on, for a cluster of an engine whose data
/// plane is the document wire, and report it — `None` for every other engine and for an engine
/// that vends no listeners, in which case the cluster keeps the DNS-form endpoints this service
/// has always reported.
///
/// The cluster's two endpoint members carry no port of their own — `DBCluster` gives them one
/// shared `Port`, "the port that the database engine is listening on" — so the address a client
/// composes is the published host with THAT port, and it is recorded as a second address of
/// this same server ([`document_wire::open_endpoint_data_plane`]) rather than leaving the pair
/// the caller was handed answered by nothing.
///
/// Called with NO store guard held, as every socket of this service is opened. The caller
/// publishes [`published_cluster_address`] as the cluster's `Endpoint` and `ReaderEndpoint` and
/// keeps the handle on the store, which is what holds the port open.
pub async fn open_cluster_document_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    record: &super::DbClusterRecord,
) -> Option<DocumentPlane> {
    if !engine_serves_the_document_wire(&record.engine) {
        return None;
    }
    let documents = document_wire::ClusterDocuments::of(engine);
    let (volume, encrypted) = {
        let store = state.lock();
        let encrypted = endpoints_are_encrypted(&store, &record.region, &record.identifier);
        // The cluster's storage is taken under this service's own store, the lock order the
        // shared register fixes.
        (
            documents.as_ref()?.lock().volume(&record.resource_id),
            encrypted,
        )
    };
    let facts: document_wire::FactsSource = {
        let state = state.clone();
        let documents = documents.clone();
        let resource_id = record.resource_id.clone();
        Arc::new(move || cluster_facts(&state, documents.as_ref(), &resource_id))
    };
    document_wire::open_endpoint_data_plane(engine, facts, volume, encrypted, Some(record.port))
        .await
}

/// The HOST a CLUSTER whose data plane is the document wire PUBLISHES for both of its
/// endpoints: the host the core bound its listener on.
///
/// A host and nothing else, because those endpoint members carry no port of their own: the
/// cluster's one `Port` member is the port a client completes it with, and the pair the two
/// compose is recorded as an address this same server answers at
/// ([`open_cluster_document_plane`]).
#[must_use]
pub fn published_cluster_host(plane: &DocumentPlane) -> String {
    plane.endpoint().host.clone()
}

/// Record the credential a client of the cluster `resource_id`'s endpoints authenticates
/// against, derived from the master user and password its create or modify carried.
///
/// What is kept is the SCRAM credential a login is checked against, never the password or any
/// form it can be recovered from, and it is held with the cluster's documents rather than on
/// this service's record, because both belong to the CLUSTER rather than to the view of this
/// front door that created it. Does nothing for a create that named no master user, whose
/// endpoints then admit no session at all — the wire serves a command outside the handshake
/// only to a session that has authenticated — and nothing on an engine that vends no such
/// store.
pub fn record_master_credential<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_id: &str,
    username: Option<&str>,
    password: Option<&str>,
) {
    let (Some(username), Some(password)) = (username, password) else {
        return;
    };
    if let Some(documents) = document_wire::ClusterDocuments::of(engine) {
        documents.lock().set_credentials(
            resource_id,
            Some(document_wire::auth::ScramCredentials::derive(
                username, password,
            )),
        );
    }
}

/// Drop everything the cluster `resource_id` held in the shared document storage, which the
/// delete of the cluster does: the documents and the credential are the cluster's, so nothing
/// of them outlives it.
pub fn forget_cluster_documents<Eng: Send + Sync + 'static>(engine: &Eng, resource_id: &str) {
    if let Some(documents) = document_wire::ClusterDocuments::of(engine) {
        documents.lock().forget(resource_id);
    }
}
