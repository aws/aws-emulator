// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS one collection's data plane is published on, and what keeps it open.
//!
//! `CollectionDetail$collectionEndpoint` is the "Collection-specific endpoint used to
//! submit index, search, and data upload requests to an OpenSearch Serverless collection":
//! not a label a caller reads back, but the address an OpenSearch client is CONFIGURED
//! with. Such a client takes the endpoint's host and port and sends `PUT /{index}`,
//! `POST /{index}/_search` and the rest at the ROOT of it — that is what AWS's own
//! documented `OpenSearch(hosts=[{'host': <collectionEndpoint host>, 'port': 443}], …)`
//! client does — so the published address has to be one where this emulator answers the
//! OpenSearch REST API at the root path, with no prefix a client would have to be told
//! about.
//!
//! So a collection publishes a listener of ITS OWN, opened through the core
//! (`engine.listeners().http(..)`, which owns the socket, the bind interface and the TLS):
//! ONE listener per collection, so the ADDRESS identifies the collection and the whole path
//! of a request stays the client's. AWS's own `<id>.<region>.aoss.amazonaws.com` host
//! resolves nowhere on the machine this emulator runs on, and the emulator's own front door
//! can only serve a request that carries the collection in its path
//! ([`crate::service::collection_rest`]), which a stock client does not.
//!
//! The handle lives in
//! [`super::ComAmazonawsOpensearchserverlessOpensearchserverlessState::collection_planes`]
//! under the collection's id, beside the records that say which collections exist: dropping
//! it closes the port, so a purged collection stops answering
//! ([`super::ComAmazonawsOpensearchserverlessOpensearchserverlessState::remove_collection`]).
//! How many ports one store can hold open is bounded by the resources a caller may create:
//! one per collection, and a store holds at most [`super::MAX_COLLECTIONS`] of those.
//!
//! What the address answers is exactly what the front door's own path-style addressing
//! answers, out of the same records, because both go through
//! [`crate::service::collection_rest::serve_collection`]: a collection that is not ACTIVE
//! has no cluster behind its endpoint yet, and everything else is computed from the indexes
//! and documents that collection holds.
//!
//! An emulator that can open no listener at all (a host driving it on a runtime with no IO)
//! publishes the addresses [`super::announced_endpoints`] announces instead, so a
//! collection is never left reporting an address nothing serves.
//!
//! # Who answers at that address
//!
//! The address is an ADDRESS, so nothing can be read off its host: a client inside this
//! emulator that is handed one (a running function that read `collectionEndpoint` out of the
//! control plane and dials it) cannot tell which service answers there the way it can for an
//! endpoint host. So the authority is recorded with this service's signing name where every
//! such reader looks ([`::service_common::published_data_planes`]) — for as long as the port
//! is open, which is why the handle that keeps it open is what withdraws the record.

use std::future::Future;
use std::pin::Pin;

use super::ComAmazonawsOpensearchserverlessOpensearchserverlessState as ServiceState;

/// One collection's data-plane listener: the address it answers on, and the handle that
/// keeps it open.
pub struct CollectionPlane {
    /// `https://<host:port>`, the base every endpoint of this collection is built on.
    base_url: String,
    /// The `host:port` of that base, as it was recorded where a reader resolves which
    /// service answers at an address, so the drop below can withdraw the very key the
    /// publish wrote.
    authority: String,
    /// Where that record lives, for as long as this handle does. `None` on an engine that
    /// vends no such records, where nothing was recorded either.
    published: Option<::service_common::published_data_planes::PublishedPlanes>,
    /// Held only to keep the port open; the core owns the socket and its TLS. Dropping it
    /// closes the port and severs what is in flight.
    _listener: ::emulator_server::ResourceListener,
}

impl CollectionPlane {
    /// The base URL a client dials this collection at.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// The record of WHO answers at this address lasts exactly as long as the port does: the
/// listener closes as this handle drops ([`super::ServiceState::remove_collection`] drops it
/// when the collection is purged), so the address stops resolving to this service at the
/// same instant rather than naming a service for a port nothing listens on.
impl Drop for CollectionPlane {
    fn drop(&mut self) {
        if let Some(published) = &self.published {
            published.withdraw(&self.authority);
        }
    }
}

/// Written by hand rather than derived: a [`CollectionPlane`] holds a core
/// `ResourceListener`, which is a live socket and carries no `Debug`. What is diagnosable
/// is the address it announced.
impl std::fmt::Debug for CollectionPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CollectionPlane")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Open the listener the collection `collection_id` is published on and PUBLISH its address
/// on the stored record, so every reader of that collection reports the address this
/// emulator answers it at.
///
/// Called by the create that stored the collection, after the store's lock is released: the
/// response of a create carries no endpoint (`CreateCollectionDetail` models none), so the
/// first read that can report one is a later call, and this has completed by then.
///
/// IDEMPOTENT, and checked BEFORE a socket is bound: a collection already published at an
/// address keeps that address, because a client may be dialling it. Nothing is published
/// either when no listener can be opened, or when the collection is already gone: the
/// record keeps the addresses the create wrote, which are the ones
/// [`super::announced_endpoints`] announces.
pub async fn publish<Eng: Send + Sync + 'static>(
    engine: ::std::sync::Arc<Eng>,
    state: &::engine::ServiceState<ServiceState>,
    collection_id: &str,
) {
    if state.lock().collection_planes.contains_key(collection_id) {
        return;
    }
    let Some(plane) = open(engine, state, collection_id).await else {
        return;
    };
    let base = plane.base_url().to_string();
    let mut store = state.lock();
    // The collection may have been deleted and purged while the port was being bound; its
    // address is then nothing to publish, and the listener closes as this returns.
    let Some(record) = store.collections.get_mut(collection_id) else {
        return;
    };
    record.collection_endpoint = base.clone();
    record.dashboard_endpoint = format!("{base}{}", super::DASHBOARDS_PATH);
    // ONE address per collection: this emulator terminates one TLS endpoint for a
    // collection, so the FIPS pair names that same address rather than a second one that
    // nothing would answer. What a published endpoint has to be is an address a client
    // reaches the collection at, and this is that address.
    record.fips_collection_endpoint = record.collection_endpoint.clone();
    record.fips_dashboard_endpoint = record.dashboard_endpoint.clone();
    store
        .collection_planes
        .insert(collection_id.to_string(), plane);
}

/// Open one collection's listener, or `None` when this engine vends none, when there is no
/// runtime to bind a socket on, and when the socket cannot be bound at all. Each of those
/// leaves the collection reporting the addresses the create wrote rather than taking the
/// request down with it.
async fn open<Eng: Send + Sync + 'static>(
    engine: ::std::sync::Arc<Eng>,
    state: &::engine::ServiceState<ServiceState>,
    collection_id: &str,
) -> Option<CollectionPlane> {
    let listeners = shared_engine(&*engine)?.listeners();
    // Where the address this listener is opened at is recorded, read before the engine is
    // handed to the handler below. `None` on an engine that vends no such records, which
    // records nothing and withdraws nothing.
    let published = ::service_common::published_data_planes::PublishedPlanes::of(&*engine);
    let handler = CollectionRestServer {
        engine,
        state: state.clone(),
        collection_id: collection_id.to_string(),
    };
    // The runtime is asked for rather than assumed, and the bind runs on a task of its OWN:
    // a host driving this emulator in process may poll a request on another executor, or on
    // a runtime built with no IO driver, and neither can open a socket — so each leaves the
    // collection with the addresses the create wrote instead of panicking inside the
    // request.
    let runtime = ::tokio::runtime::Handle::try_current().ok()?;
    let opened = runtime
        .spawn(async move {
            listeners
                .http(handler, ::emulator_server::HttpPolicy::default())
                .await
        })
        .await;
    let listener = match opened {
        Ok(Ok(listener)) => listener,
        // Reported rather than discarded, so a port range or a descriptor table that is
        // exhausted is told apart from a runtime that opens no sockets at all.
        Ok(Err(error)) => {
            ::tracing::warn!(
                %error,
                collection_id,
                "a collection's data-plane address could not be opened"
            );
            return None;
        }
        Err(_cancelled) => return None,
    };
    // The core's OWN reading of the address a client dials, which names the loopback
    // interface for a wildcard bind — where the bound address would be `0.0.0.0`, an address
    // no client dials — and brackets an IPv6 one, so the URL parses wherever the operator
    // bound it.
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);
    // Recorded as the port opens, under this service's own signing name — which is the
    // credential scope a client signs a request to this address with — so a client inside
    // this emulator that was handed the address can be routed to this service (see the
    // module docs). Withdrawn when the handle below is dropped.
    if let Some(published) = &published {
        published.publish(&authority, super::ARN_NAMESPACE);
    }
    Some(CollectionPlane {
        base_url: format!("https://{authority}"),
        authority,
        published,
        _listener: listener,
    })
}

/// The shared engine behind a handler's `Eng`, which is what vends the core's listeners.
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}

/// Serves ONE collection's published address. The core runs the browser checks and owns
/// CORS; this answers the OpenSearch REST request out of the collection's own records.
///
/// The shared engine is held beside the store because the collection's own policies decide
/// what this address answers: WHO a request speaks for is a question about the account's
/// identity records, which live in that engine beside this service's own store
/// (`crate::service::collection_rest::serve_published`).
struct CollectionRestServer<Eng> {
    engine: ::std::sync::Arc<Eng>,
    state: ::engine::ServiceState<ServiceState>,
    collection_id: String,
}

impl<Eng: Send + Sync + 'static> ::emulator_server::HttpHandler for CollectionRestServer<Eng> {
    fn handle(
        &self,
        req: ::http::Request<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = ::http::Response<Vec<u8>>> + Send>> {
        let engine = ::std::sync::Arc::clone(&self.engine);
        let state = self.state.clone();
        let collection_id = self.collection_id.clone();
        Box::pin(async move {
            crate::service::collection_rest::serve_published(
                &*engine,
                &state,
                &collection_id,
                &req,
            )
        })
    }
}
