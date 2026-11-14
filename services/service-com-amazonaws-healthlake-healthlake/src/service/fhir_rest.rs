// SPDX-License-Identifier: CC0-1.0

//! The data store's FHIR REST data plane: the wire
//! `DatastoreProperties$DatastoreEndpoint` advertises.
//!
//! # Not modelled operations
//!
//! `smithy_query(query="service_info")` reports this service as `awsJson1_0`, and its
//! 26 modelled operations are all control plane: they create, describe, tag and
//! delete data stores. The FHIR interactions a caller then performs on the endpoint a
//! data store reports — `POST <DatastoreEndpoint>Patient`,
//! `GET <DatastoreEndpoint>Patient/{id}` — are not modelled operations and carry no
//! `X-Amz-Target`: they are the FHIR REST API, addressed by path and HTTP method, on
//! the same signed `healthlake` wire. Nothing in the generated routing table can name
//! them, so they are reached through
//! [`crate::service::EmulatedService::override_operation`], the one seam a service
//! has for a request the model's routing key cannot name.
//!
//! # What is served
//!
//! The interactions the FHIR RESTful API defines over a resource's lifetime start:
//!
//! * `create` — `POST [base]/{type}`, which assigns the logical id (FHIR requires the
//!   server to assign one and to IGNORE any `id` the submitted resource carries), and
//!   answers `201` with the stored resource and its `Location`;
//! * `read` — `GET [base]/{type}/{id}`, which answers `200` with the stored resource,
//!   byte-for-byte what the create stored, and `404` for an id the data store holds no
//!   resource under;
//! * `update`, including `update-as-create` — `PUT [base]/{type}/{id}`, which stores the
//!   submitted resource AT the caller-chosen id: `200` when it replaces the version
//!   held there, `201` when it creates the resource at that id, and the resource is then
//!   readable through `read`;
//! * `search` at the TYPE level — `GET [base]/{type}?<param>=<value>`, which answers
//!   `200` with a `searchset` Bundle of the resources of that type satisfying every
//!   parameter the query carries ([`SearchCriteria`]), an empty result included. The
//!   parameters implemented are `_id`, `identifier` and the `_count` page size; any other
//!   is refused rather than ignored, because a dropped filter would answer resources the
//!   caller never asked for.
//!
//! Every other interaction on this path space (`delete`, `vread`, the
//! batch/transaction endpoint) is answered `501` with an `OperationOutcome` of
//! `not-supported`, which is this emulator's "not implemented yet" for a FHIR wire —
//! never an empty `200` that would look like a successful interaction nothing performed.
//! Each is its own gap, to be implemented against its own recorded behaviour.
//!
//! # The error envelope
//!
//! A FHIR endpoint states its failures as an `OperationOutcome` resource, not as the
//! control plane's awsJson error shape: a FHIR client reads the `issue[].code` to
//! decide what to do. [`outcome`] is the one builder of that body here.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};
use std::sync::Arc;
use std::time::SystemTime;

use super::non_rpc::{bounded, request_path};
use crate::state::FhirEndpointPath;

/// The media type a FHIR JSON representation travels as, which is what a FHIR client
/// sends and expects back (FHIR R4 §2.2 "Content Types").
pub const FHIR_JSON_CONTENT_TYPE: &str = "application/fhir+json";

/// The `OperationOutcome` issue code for a resource, or a resource type, the data
/// store does not hold.
const NOT_FOUND: &str = "not-found";

/// The issue code for a submitted resource whose content breaks a rule of the
/// interaction — the `resourceType` not matching the URL's type, say.
const INVALID: &str = "invalid";

/// The issue code for a body that could not be read as a FHIR JSON resource at all.
const STRUCTURE: &str = "structure";

/// The issue code for an interaction this endpoint does not implement.
const NOT_SUPPORTED: &str = "not-supported";

/// The issue code for an interaction refused because a bound on what this emulator
/// RETAINS was reached. FHIR's `too-costly` is "the operation was stopped to protect
/// server resources", which is what a retention ceiling is.
const TOO_COSTLY: &str = "too-costly";

/// The issue code for a submitted resource larger than this endpoint stores. FHIR's
/// `too-long` is "provided content is too long".
const TOO_LONG: &str = "too-long";

/// The issue code for a failure of the server itself.
const EXCEPTION: &str = "exception";

/// The path segment the FHIR capability statement is addressed at
/// (`GET [base]/metadata`, FHIR R4 §3.1.0.1): an interaction of its own, not a resource
/// type — the name is lower-case, which no resource type is — and not implemented here.
const CAPABILITY_STATEMENT_SEGMENT: &str = "metadata";

/// The longest resource type name this endpoint accepts in a path. FHIR resource type
/// names are short UpperCamelCase tokens; the bound keeps a long path segment out of
/// the store's keys and out of a message.
const MAX_RESOURCE_TYPE_LEN: usize = 64;

/// The longest FHIR logical id this endpoint accepts from a caller.
///
/// FHIR R4's `id` primitive type is `[A-Za-z0-9\-\.]{1,64}` ("Any combination of upper-
/// or lower-case ASCII letters, numerals, '-' and '.', with a length limit of 64
/// characters"), which is what [`is_logical_id`] enforces for the id an `update` places
/// a resource at.
const MAX_LOGICAL_ID_LEN: usize = 64;

/// How many logical ids a create offers before it gives up.
///
/// The mint advances a counter on every call, so each candidate differs and one
/// attempt is enough in practice; the cap is what BOUNDS the walk, because it runs
/// while the per-service store lock is held and no single request may pin that lock.
const MAX_ID_MINT_ATTEMPTS: usize = 8;

/// The data store FHIR REST data plane, as the one [`ErasedOperation`] the service
/// holds for it.
///
/// Owns the shared engine exactly as a generated operation does, so the resources it
/// writes live in the SAME per-service store the control plane's handlers read: a
/// data store deleted through `DeleteFHIRDatastore` takes its FHIR resources with it,
/// because they are held on the data store's own record.
pub struct FhirRestDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> FhirRestDataPlane<Eng> {
    /// Build the data plane from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for FhirRestDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req) })
    }
}

impl<Eng: Send + Sync + 'static> FhirRestDataPlane<Eng> {
    /// Serve one request on a data store's FHIR endpoint.
    ///
    /// The method and the shape of the interaction path together name the FHIR
    /// interaction, which is how the FHIR RESTful API is addressed — there is no
    /// operation name on this wire.
    fn serve(&self, req: &RtRequest) -> RtResponse {
        let path = request_path(req.uri());
        let Some(route) = crate::state::parse_datastore_fhir_path(path) else {
            // `override_operation` only routes a path this reader accepts, so this is
            // unreachable; answering the FHIR envelope rather than panicking keeps the
            // handler total.
            return outcome(
                404,
                NOT_FOUND,
                "The request path is not a data store FHIR endpoint path",
            );
        };
        let interaction: Vec<&str> = route.interaction.iter().map(String::as_str).collect();
        match (req.method(), interaction.as_slice()) {
            ("POST", [resource_type]) => match req.body().bytes() {
                Some(body) => self.create(&route, resource_type, body),
                // The router buffers every request before dispatch, so a body it
                // cannot hand back in memory is a server-side failure, never a
                // caller's malformed resource: answering 400 here would report a
                // document this handler never read.
                None => outcome(
                    500,
                    EXCEPTION,
                    "The server could not read the submitted resource from the request body",
                ),
            },
            ("GET", [resource_type, id]) => self.read(&route, resource_type, id),
            // The capability statement is its own interaction, not a search for a type
            // named `metadata`, so it keeps the "not implemented yet" answer every other
            // unimplemented interaction on this path space gets rather than being read as
            // a search of a type that cannot exist.
            ("GET", [segment]) if *segment == CAPABILITY_STATEMENT_SEGMENT => outcome(
                501,
                NOT_SUPPORTED,
                &format!(
                    "This data store endpoint does not implement the FHIR capability \
                     statement interaction (GET [base]/{CAPABILITY_STATEMENT_SEGMENT}) yet"
                ),
            ),
            ("GET", [resource_type]) => self.search(&route, resource_type, req.uri()),
            ("PUT", [resource_type, id]) => match req.body().bytes() {
                Some(body) => self.update(&route, resource_type, id, body),
                // As for the create: the router buffers every request before dispatch,
                // so a body it cannot hand back in memory is a server-side failure.
                None => outcome(
                    500,
                    EXCEPTION,
                    "The server could not read the submitted resource from the request body",
                ),
            },
            (method, _) => outcome(
                501,
                NOT_SUPPORTED,
                &format!(
                    "This data store endpoint implements the FHIR create (POST [base]/{{type}}), \
                     read (GET [base]/{{type}}/{{id}}), update (PUT [base]/{{type}}/{{id}}) and \
                     type-level search (GET [base]/{{type}}?<param>=<value>) interactions; {} {} \
                     is not implemented yet",
                    bounded(method),
                    bounded(&format!("[base]/{}", route.interaction.join("/")))
                ),
            ),
        }
    }

    /// The FHIR `create` interaction: store the submitted resource under a logical id
    /// this server assigns, and answer `201` with what was stored.
    fn create(&self, route: &FhirEndpointPath, resource_type: &str, body: &[u8]) -> RtResponse {
        if !is_resource_type_name(resource_type) {
            return unknown_resource_type(resource_type);
        }
        // The BYTES are bounded before the body is parsed, let alone stored: a count
        // ceiling alone would admit the count times whatever one body the transport
        // accepts, so the size of one resource is refused here and the retained totals
        // below.
        if let Err(failure) = submitted_within_size(body) {
            return failure.response();
        }
        let submitted = match submitted_resource(body, resource_type) {
            Ok(submitted) => submitted,
            Err(failure) => return failure.response(),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Writing to a data store means first letting the registry catch up with the
        // clock: a lifecycle transition that was already due is materialised here, so
        // a store that has become ACTIVE serves, and one that has become DELETED does
        // not.
        store.settle_lifecycle();
        if let Err(failure) = serving(&store, &route.datastore_id) {
            return failure.response();
        }
        let held = store
            .datastores
            .get(&route.datastore_id)
            .map_or(0, |datastore| datastore.fhir_resources.len());
        if held >= crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE {
            return outcome(
                507,
                TOO_COSTLY,
                &format!(
                    "This data store already holds the maximum number of FHIR resources this \
                     emulator retains ({})",
                    crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE
                ),
            );
        }

        let now = SystemTime::now();
        let stamp = ::aws_smithy_types::DateTime::from(now);
        let Ok(last_updated) = stamp.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
            return outcome(
                500,
                EXCEPTION,
                "The server could not render the resource's last-updated instant",
            );
        };
        // A minted id is checked against the data store before it is used, so a
        // create can never overwrite a stored resource. Each call advances the mint
        // counter, so a retry always offers a DIFFERENT candidate; the attempt cap
        // bounds the walk regardless, because this runs while the store lock is held
        // and no request may pin that lock.
        let mut minted = None;
        for _ in 0..MAX_ID_MINT_ATTEMPTS {
            let candidate = store.next_fhir_resource_id(now);
            let taken = store
                .datastores
                .get(&route.datastore_id)
                .is_some_and(|datastore| {
                    datastore
                        .fhir_resource(resource_type, &candidate)
                        .is_some()
                });
            if !taken {
                minted = Some(candidate);
                break;
            }
        }
        let Some(id) = minted else {
            return outcome(
                500,
                EXCEPTION,
                "The server could not assign an unused logical id for the resource",
            );
        };

        let version = 1;
        let content = stored_content(submitted, &id, version, &last_updated);
        // What this resource will COST the store, measured on the bytes actually
        // retained (the stored content, not the request), and charged against both
        // budgets with `saturating_add` before anything is written.
        let serialized = content.to_string();
        let bytes = serialized.len();
        // The pre-parse check above refused an over-large REQUEST; this one makes the
        // per-resource ceiling exact, because what is retained is the stored
        // serialisation (the submitted content with `id` and `meta` written on), which
        // is a little larger than the request.
        if bytes > crate::state::MAX_FHIR_RESOURCE_BYTES {
            return outcome(
                413,
                TOO_LONG,
                &format!(
                    "A FHIR resource this endpoint stores is at most {} bytes; the one this \
                     create would store is {bytes} bytes",
                    crate::state::MAX_FHIR_RESOURCE_BYTES
                ),
            );
        }
        // The retention budgets, charged the same way for a create and for an update:
        // a create retains one more resource and releases none.
        if let Err(failure) = admits(&mut store, &route.datastore_id, bytes, 0, true) {
            return failure.response();
        }
        let resource = crate::state::FhirResource {
            resource_type: resource_type.to_string(),
            id: id.clone(),
            version,
            last_updated: stamp,
            content,
            bytes,
            // A created resource has superseded nothing: its first version is
            // the only one there is.
            superseded: Vec::new(),
        };
        let Some(datastore) = store.datastores.get_mut(&route.datastore_id) else {
            // `serving` resolved the record under this same lock.
            return store_unavailable();
        };
        let location = format!(
            "{}{resource_type}/{id}/_history/{version}",
            datastore.endpoint
        );
        datastore.fhir_resource_bytes = datastore.fhir_resource_bytes.saturating_add(bytes);
        datastore
            .fhir_resources
            .insert((resource_type.to_string(), id), resource);
        let mut response = fhir_response(201, serialized);
        response.headers_mut().insert("location", location);
        response
            .headers_mut()
            .insert("etag", format!("W/\"{version}\""));
        response
    }

    /// The FHIR `update` interaction, and with it `update-as-create`: store the
    /// submitted resource AT the logical id the URL names.
    ///
    /// FHIR R4 §3.1.0.5 ("update"): the id is the CALLER's here, not the server's — the
    /// opposite of `create` — and a server that supports update-as-create stores the
    /// resource at that id when it holds none, answering `201`; replacing the version it
    /// does hold answers `200`. Either way the stored version is the one a later `read`
    /// answers, and `meta.versionId` advances with every write.
    ///
    /// A write ENDS the span the version it replaces was current for rather than
    /// erasing that version: the superseded version is retained on the record
    /// ([`crate::state::FhirResource::supersede`]), bounded by
    /// [`crate::state::MAX_FHIR_RESOURCE_VERSIONS`], so a reader asking what this
    /// resource was at an earlier instant — the point-in-time restore
    /// `RestoreFHIRDatastore` performs — is answered the version that was current
    /// then.
    fn update(
        &self,
        route: &FhirEndpointPath,
        resource_type: &str,
        id: &str,
        body: &[u8],
    ) -> RtResponse {
        if !is_resource_type_name(resource_type) {
            return unknown_resource_type(resource_type);
        }
        // The id is the caller's, so unlike a minted one it has to be checked against
        // the FHIR `id` type before it becomes a key of the store.
        if !is_logical_id(id) {
            return outcome(
                400,
                INVALID,
                &format!(
                    "'{}' is not a FHIR logical id: an id is 1 to {MAX_LOGICAL_ID_LEN} \
                     characters of ASCII letters, digits, '-' and '.'",
                    bounded(id)
                ),
            );
        }
        // The BYTES first, as the create does: a body larger than this endpoint stores
        // is refused before it is parsed, let alone stored.
        if let Err(failure) = submitted_within_size(body) {
            return failure.response();
        }
        let submitted = match submitted_resource(body, resource_type) {
            Ok(submitted) => submitted,
            Err(failure) => return failure.response(),
        };
        // "If the request body includes an id, it SHALL match the id in the URL": the
        // two naming different resources is a refusal, not a silent re-filing.
        if let Some(carried) = submitted.get("id").and_then(::serde_json::Value::as_str) {
            if carried != id {
                return outcome(
                    400,
                    INVALID,
                    &format!(
                        "The submitted resource's id is '{}', but the request URL names '{}'",
                        bounded(carried),
                        bounded(id)
                    ),
                );
            }
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Writing to a data store means first letting the registry catch up with the
        // clock, exactly as the create does.
        store.settle_lifecycle();
        if let Err(failure) = serving(&store, &route.datastore_id) {
            return failure.response();
        }

        // What is held at this id already decides everything that differs between the
        // two outcomes: the version the write lands on, and the status.
        let held_version = store
            .datastores
            .get(&route.datastore_id)
            .and_then(|datastore| datastore.fhir_resource(resource_type, id))
            .map(|resource| resource.version);

        let now = SystemTime::now();
        let stamp = ::aws_smithy_types::DateTime::from(now);
        let Ok(last_updated) = stamp.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
            return outcome(
                500,
                EXCEPTION,
                "The server could not render the resource's last-updated instant",
            );
        };
        let version = held_version.map_or(1, |held| held.saturating_add(1));
        let content = stored_content(submitted, id, version, &last_updated);
        let serialized = content.to_string();
        let bytes = serialized.len();
        // Exact per-resource ceiling on what is RETAINED, as the create applies it.
        if bytes > crate::state::MAX_FHIR_RESOURCE_BYTES {
            return outcome(
                413,
                TOO_LONG,
                &format!(
                    "A FHIR resource this endpoint stores is at most {} bytes; the one this \
                     update would store is {bytes} bytes",
                    crate::state::MAX_FHIR_RESOURCE_BYTES
                ),
            );
        }
        // The version this write supersedes is RETAINED beside the new one, so the
        // write gives nothing back unless versions have to be DROPPED: the
        // retained-version ceiling drops the oldest, and the retention budgets drop
        // further ones, oldest first, rather than refusing a write that a data store
        // keeping no history would have admitted. How many go is settled HERE, before
        // anything is written, so the write either fits every budget or is refused.
        //
        // What each candidate number of dropped versions would RELEASE is read out
        // of the store FIRST: charging the budgets needs the store mutably (the
        // store-wide ceiling reaps terminal records before refusing), so the held
        // resource is not borrowed across that walk. Bounded by
        // [`crate::state::MAX_FHIR_RESOURCE_VERSIONS`].
        let candidates: Option<Vec<(usize, usize)>> = store
            .datastores
            .get(&route.datastore_id)
            .and_then(|datastore| datastore.fhir_resource(resource_type, id))
            .map(|held| {
                (held.forced_drops_on_supersede()..=held.max_drops_on_supersede())
                    .map(|drop| (drop, held.release_dropping(drop)))
                    .collect()
            });
        let dropped = match candidates {
            Some(candidates) => {
                let mut admitted = None;
                let mut refused = None;
                for (drop, released) in candidates {
                    match admits(&mut store, &route.datastore_id, bytes, released, false) {
                        Ok(()) => {
                            admitted = Some(drop);
                            break;
                        }
                        Err(failure) => refused = Some(failure),
                    }
                }
                match (admitted, refused) {
                    (Some(drop), _) => drop,
                    // Not even retaining no earlier version at all fits the budgets,
                    // so the refusal the last attempt earned is this write's answer.
                    (None, Some(failure)) => return failure.response(),
                    // The walk above runs at least once, so one of the two is always
                    // set; answering a server fault keeps the handler total.
                    (None, None) => return store_unavailable(),
                }
            }
            // `update-as-create` retains one more resource and releases nothing.
            None => {
                if let Err(failure) = admits(&mut store, &route.datastore_id, bytes, 0, true) {
                    return failure.response();
                }
                0
            }
        };

        let next = crate::state::FhirResourceVersion {
            version,
            last_updated: stamp,
            content,
            bytes,
        };
        let Some(datastore) = store.datastores.get_mut(&route.datastore_id) else {
            // `serving` resolved the record under this same lock.
            return store_unavailable();
        };
        let location = format!(
            "{}{resource_type}/{id}/_history/{version}",
            datastore.endpoint
        );
        let key = (resource_type.to_string(), id.to_string());
        let released = match datastore.fhir_resources.get_mut(&key) {
            // The same resource, one version on: the version it held moves into its
            // history, where a read as of an earlier instant is still answered from,
            // and this one takes its place.
            Some(held) => held.supersede_dropping(next, dropped),
            // `update-as-create`: nothing is held at this id, so this write mints the
            // resource there at its first version, superseding nothing.
            None => {
                datastore.fhir_resources.insert(
                    key,
                    crate::state::FhirResource {
                        resource_type: resource_type.to_string(),
                        id: id.to_string(),
                        version: next.version,
                        last_updated: next.last_updated,
                        content: next.content,
                        bytes: next.bytes,
                        superseded: Vec::new(),
                    },
                );
                0
            }
        };
        // The bytes this write retains are charged, and the bytes of the versions it
        // dropped are given back: a superseded version stays held, so repeatedly
        // updating one resource accumulates only up to the retained-version ceiling
        // and whatever the retention budgets admit.
        datastore.fhir_resource_bytes = datastore
            .fhir_resource_bytes
            .saturating_sub(released)
            .saturating_add(bytes);
        let status = if held_version.is_some() { 200 } else { 201 };
        let mut response = fhir_response(status, serialized);
        response.headers_mut().insert("location", location);
        response
            .headers_mut()
            .insert("etag", format!("W/\"{version}\""));
        response
    }

    /// The FHIR `read` interaction: answer the stored resource, exactly as the create
    /// stored it.
    fn read(&self, route: &FhirEndpointPath, resource_type: &str, id: &str) -> RtResponse {
        if !is_resource_type_name(resource_type) {
            return unknown_resource_type(resource_type);
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Reporting a resource means reporting it from a data store whose own status
        // is up to date with the clock; the transition was owed at this instant
        // whether or not anybody read.
        store.settle_lifecycle();
        if let Err(failure) = serving(&store, &route.datastore_id) {
            return failure.response();
        }
        let found = store
            .datastores
            .get(&route.datastore_id)
            .and_then(|datastore| datastore.fhir_resource(resource_type, id));
        let Some(resource) = found else {
            return outcome(
                404,
                NOT_FOUND,
                &format!(
                    "This data store holds no {} with id '{}'",
                    bounded(resource_type),
                    bounded(id)
                ),
            );
        };
        let mut response = fhir_response(200, resource.content.to_string());
        response
            .headers_mut()
            .insert("etag", format!("W/\"{}\"", resource.version));
        response
    }

    /// The FHIR `search` interaction at the TYPE level: answer the resources of
    /// `resource_type` this data store holds that satisfy every search parameter the
    /// query carries, as a `searchset` Bundle.
    ///
    /// FHIR R4 §3.1.1 ("search"): `GET [base]/{type}?<parameters>` answers `200` with a
    /// Bundle of type `searchset` whose `entry` carries one match per resource, each with
    /// its `fullUrl` and `search.mode` — and an empty result is that same `200` with no
    /// entries, never a `404`. Repeated parameters are AND-ed, and the comma-separated
    /// values of ONE parameter are OR-ed, which is the whole of how a criterion composes.
    ///
    /// A REPORT: it reads the stored resources and answers them verbatim, so a search
    /// changes nothing a later caller sees. The one write is the lifecycle settle every
    /// interaction on this endpoint performs, which the clock already owed.
    fn search(&self, route: &FhirEndpointPath, resource_type: &str, uri: &str) -> RtResponse {
        if !is_resource_type_name(resource_type) {
            return unknown_resource_type(resource_type);
        }
        let criteria = match SearchCriteria::of(uri) {
            Ok(criteria) => criteria,
            Err(failure) => return failure.response(),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Reporting resources means reporting them from a data store whose own status is
        // up to date with the clock, exactly as the read does.
        store.settle_lifecycle();
        if let Err(failure) = serving(&store, &route.datastore_id) {
            return failure.response();
        }
        let Some(datastore) = store.datastores.get(&route.datastore_id) else {
            // `serving` resolved the record under this same lock.
            return store_unavailable();
        };
        // The data store's resources are held under `(resourceType, id)`, so the walk is
        // over the resources of THIS type, in id order — a stable order, so two searches
        // answering the same matches answer them in the same sequence. Bounded by
        // `crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE`, the ceiling on what one data
        // store retains.
        let matched: Vec<&crate::state::FhirResource> = datastore
            .fhir_resources
            .values()
            .filter(|resource| resource.resource_type == resource_type)
            .filter(|resource| criteria.matches(&resource.content, &resource.id))
            .collect();
        let entries: Vec<::serde_json::Value> = matched
            .iter()
            .take(criteria.page_size)
            .map(|resource| {
                ::serde_json::json!({
                    "fullUrl": format!(
                        "{}{}/{}",
                        datastore.endpoint, resource.resource_type, resource.id
                    ),
                    // The stored resource, byte for byte what a read of it answers.
                    "resource": resource.content.clone(),
                    // FHIR `SearchEntryMode`: this entry is here because it matched the
                    // criterion, not because something included it.
                    "search": { "mode": "match" },
                })
            })
            .collect();
        // `total` is "the total number of matches", which is every resource the criterion
        // selected rather than the number this page carries (FHIR R4 Bundle.total on a
        // `searchset`). `entry` is left out entirely when nothing matched, as an empty
        // search result is written. No `self` link: a Bundle link is optional, and the
        // one thing it could carry is the request's own query text, which this endpoint
        // does not echo back into a response.
        let mut bundle = ::serde_json::json!({
            "resourceType": "Bundle",
            "type": "searchset",
            "total": matched.len(),
        });
        if !entries.is_empty() {
            bundle["entry"] = ::serde_json::Value::Array(entries);
        }
        fhir_response(200, bundle.to_string())
    }
}

/// How many matches one search page carries when the request asks for no size.
///
/// CHOSEN by this emulator: FHIR leaves the page size to the server (`_count` is "the
/// number of resources that should be returned in a single page", and "the server is not
/// bound to return the number requested"), and nothing in HealthLake's documentation
/// fixes one. It is well above the number of resources any one test seeds and below the
/// per-data-store retention ceiling, so an ordinary search answers every match in one
/// page.
const DEFAULT_SEARCH_PAGE_SIZE: usize = 100;

/// The most matches one page carries, whatever `_count` asks for: the whole of what one
/// data store retains ([`crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE`]), so a caller
/// may ask for every resource it could possibly have stored and no more.
const MAX_SEARCH_PAGE_SIZE: usize = crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE;

/// The most search parameters one query may carry, and the most comma-separated
/// alternatives one parameter may carry.
///
/// A criterion is evaluated against every resource the data store holds, so the work one
/// request can ask for is the product of the two counts and the retained resources: these
/// bound it. Both are far above what a search of this endpoint expresses in practice.
const MAX_SEARCH_PARAMETERS: usize = 8;

/// See [`MAX_SEARCH_PARAMETERS`].
const MAX_SEARCH_ALTERNATIVES: usize = 16;

/// The search parameter that selects a resource by its logical id (FHIR R4's `_id`, "the
/// logical id of the resource").
const ID_PARAMETER: &str = "_id";

/// The search parameter that selects a resource by a business identifier it carries — the
/// `identifier` element, which is a token-type search parameter on every resource type
/// that has one.
const IDENTIFIER_PARAMETER: &str = "identifier";

/// The result parameter that asks for a page size (FHIR R4 §3.1.1.4 `_count`).
const COUNT_PARAMETER: &str = "_count";

/// One FHIR token-search value, as `[system]|[code]` splits it (FHIR R4 §3.1.1.5.4
/// "token"): the four forms a token parameter can take, which differ in whether the
/// system and the code are each constrained.
#[derive(Debug, Clone)]
enum TokenMatch {
    /// `code` — the value matches in ANY system, which is the common form.
    Code(String),
    /// `system|code` — both the system and the value are named.
    SystemAndCode(String, String),
    /// `|code` — the value, in a record carrying NO system.
    CodeWithoutSystem(String),
    /// `system|` — any value in that system.
    System(String),
}

impl TokenMatch {
    /// The token this parameter value states.
    fn of(raw: &str) -> Self {
        match raw.split_once('|') {
            Some(("", code)) => Self::CodeWithoutSystem(code.to_string()),
            Some((system, "")) => Self::System(system.to_string()),
            Some((system, code)) => Self::SystemAndCode(system.to_string(), code.to_string()),
            None => Self::Code(raw.to_string()),
        }
    }

    /// Whether one `Identifier` element (a JSON object carrying `system` and `value`)
    /// satisfies this token.
    fn matches_identifier(&self, identifier: &::serde_json::Value) -> bool {
        let system = identifier
            .get("system")
            .and_then(::serde_json::Value::as_str);
        let value = identifier.get("value").and_then(::serde_json::Value::as_str);
        match self {
            Self::Code(code) => value == Some(code.as_str()),
            Self::SystemAndCode(wanted_system, code) => {
                system == Some(wanted_system.as_str()) && value == Some(code.as_str())
            }
            Self::CodeWithoutSystem(code) => system.is_none() && value == Some(code.as_str()),
            Self::System(wanted_system) => system == Some(wanted_system.as_str()),
        }
    }
}

/// One search parameter of a query, with the alternatives its comma-separated value
/// states — any of which satisfies it.
#[derive(Debug, Clone)]
enum SearchFilter {
    /// `_id`: the resource's logical id.
    Id(Vec<String>),
    /// `identifier`: a business identifier the resource carries.
    Identifier(Vec<TokenMatch>),
}

impl SearchFilter {
    /// Whether `content` (the stored resource) stored under logical id `id` satisfies
    /// this parameter: any one alternative is enough, which is how the comma-separated
    /// values of one parameter compose.
    fn matches(&self, content: &::serde_json::Value, id: &str) -> bool {
        match self {
            Self::Id(wanted) => wanted.iter().any(|candidate| candidate == id),
            Self::Identifier(tokens) => {
                let element = content.get(IDENTIFIER_PARAMETER);
                let held: Vec<&::serde_json::Value> = match element {
                    // `Patient.identifier` and its siblings are `0..*`, so the element is
                    // normally an array — but a resource carrying a single object is read
                    // the same way rather than ignored.
                    Some(::serde_json::Value::Array(list)) => list.iter().collect(),
                    Some(single) => vec![single],
                    None => Vec::new(),
                };
                tokens
                    .iter()
                    .any(|token| held.iter().any(|identifier| token.matches_identifier(identifier)))
            }
        }
    }
}

/// What one search request asks for: the parameters a resource has to satisfy, and how
/// many matches the page carries.
#[derive(Debug, Clone)]
struct SearchCriteria {
    /// Every parameter the query carried, ALL of which a match satisfies (FHIR R4
    /// §3.1.1: repeated and distinct parameters are AND-ed).
    filters: Vec<SearchFilter>,
    /// How many matches this page carries, from `_count` or
    /// [`DEFAULT_SEARCH_PAGE_SIZE`].
    page_size: usize,
}

impl SearchCriteria {
    /// Read the criterion out of the request URI, or the refusal it earned.
    ///
    /// A parameter this endpoint does not implement is REFUSED rather than ignored: a
    /// search answers the resources that satisfy the criterion the caller stated, so
    /// dropping a parameter would answer resources the caller never asked for and call
    /// them matches. FHIR R4 §3.1.1.1 provides for exactly this ("the server … return[s]
    /// an error … for any parameter it does not recognise"), and each unimplemented
    /// parameter is its own gap, to be implemented against its own recorded behaviour.
    fn of(uri: &str) -> Result<Self, Failure> {
        let pairs = super::non_rpc::query_pairs(uri);
        if pairs.len() > MAX_SEARCH_PARAMETERS {
            return Err(Failure::new(
                400,
                TOO_COSTLY,
                format!(
                    "A search of this endpoint carries at most {MAX_SEARCH_PARAMETERS} \
                     parameters; this one carries {}",
                    pairs.len()
                ),
            ));
        }
        let mut filters = Vec::new();
        let mut page_size = DEFAULT_SEARCH_PAGE_SIZE;
        for (name, value) in &pairs {
            let alternatives: Vec<&str> = value.split(',').collect();
            if alternatives.len() > MAX_SEARCH_ALTERNATIVES {
                return Err(Failure::new(
                    400,
                    TOO_COSTLY,
                    format!(
                        "A search parameter of this endpoint carries at most \
                         {MAX_SEARCH_ALTERNATIVES} values; '{}' carries {}",
                        bounded(name),
                        alternatives.len()
                    ),
                ));
            }
            match name.as_str() {
                ID_PARAMETER => filters.push(SearchFilter::Id(
                    alternatives.iter().map(|one| (*one).to_string()).collect(),
                )),
                IDENTIFIER_PARAMETER => filters.push(SearchFilter::Identifier(
                    alternatives.iter().map(|one| TokenMatch::of(one)).collect(),
                )),
                COUNT_PARAMETER => {
                    let Ok(asked) = value.parse::<usize>() else {
                        return Err(Failure::new(
                            400,
                            INVALID,
                            format!(
                                "'{}' is not a page size: {COUNT_PARAMETER} is a whole \
                                 number of resources",
                                bounded(value)
                            ),
                        ));
                    };
                    // "The server is not bound to return the number requested, but can
                    // return less": the ceiling is this endpoint's own.
                    page_size = asked.min(MAX_SEARCH_PAGE_SIZE);
                }
                _ => {
                    return Err(Failure::new(
                        400,
                        NOT_SUPPORTED,
                        format!(
                            "This data store endpoint implements the search parameters \
                             {ID_PARAMETER}, {IDENTIFIER_PARAMETER} and \
                             {COUNT_PARAMETER}; '{}' is not implemented yet",
                            bounded(name)
                        ),
                    ))
                }
            }
        }
        Ok(Self {
            filters,
            page_size,
        })
    }

    /// Whether the resource stored as `content` under logical id `id` satisfies EVERY
    /// parameter of this criterion. A query carrying no parameter at all selects every
    /// resource of the type, which is what `GET [base]/{type}` means.
    fn matches(&self, content: &::serde_json::Value, id: &str) -> bool {
        self.filters
            .iter()
            .all(|filter| filter.matches(content, id))
    }
}

/// The submitted resource, read from the request body, or the refusal it earned.
///
/// Two rules of the FHIR create interaction, in the order a server can apply them: the
/// body must be a JSON object (a FHIR resource is one), and its `resourceType` must be
/// the type the URL names — a mismatch is a refusal rather than a silent re-filing,
/// because the URL and the body would otherwise disagree about what was created.
fn submitted_resource(body: &[u8], resource_type: &str) -> Result<::serde_json::Value, Failure> {
    if body.is_empty() {
        return Err(Failure::new(
            400,
            STRUCTURE,
            "The create interaction requires a FHIR resource in the request body".to_string(),
        ));
    }
    let parsed: ::serde_json::Value = ::serde_json::from_slice(body).map_err(|error| {
        Failure::new(
            400,
            STRUCTURE,
            format!(
                "The request body is not a FHIR JSON resource: {}",
                bounded(&error.to_string())
            ),
        )
    })?;
    if !parsed.is_object() {
        return Err(Failure::new(
            400,
            STRUCTURE,
            "A FHIR resource is a JSON object, and the request body is not one".to_string(),
        ));
    }
    match parsed
        .get("resourceType")
        .and_then(::serde_json::Value::as_str)
    {
        Some(submitted) if submitted == resource_type => Ok(parsed),
        Some(submitted) => Err(Failure::new(
            400,
            INVALID,
            format!(
                "The submitted resource's resourceType is '{}', but the request URL names \
                 '{}'",
                bounded(submitted),
                bounded(resource_type)
            ),
        )),
        None => Err(Failure::new(
            400,
            STRUCTURE,
            "A FHIR resource carries a resourceType, and the submitted one does not".to_string(),
        )),
    }
}

/// The submitted resource as it is STORED: its own content, with the two values the
/// server owns written onto it.
///
/// The server owns the version and the last-updated instant, so any `meta.versionId` or
/// `meta.lastUpdated` the request carried is replaced rather than honoured, and `id` is
/// written to the id the resource is stored under — the one the server assigned on a
/// `create`, the one the URL named on an `update`. The REST of `meta` (a profile claim,
/// a security label, a tag) is the caller's and is kept.
///
/// `pub(crate)` because it is the ONE builder of a stored resource's bytes: the
/// Synthea preload ([`super::synthea`]) and a FHIR import job's delivery
/// ([`crate::state::ComAmazonawsHealthlakeHealthlakeState::deliver_import_run`])
/// write their records through it too, so a preloaded, an imported and a created
/// resource are stored, and read back, identically.
pub(crate) fn stored_content(
    submitted: ::serde_json::Value,
    id: &str,
    version: u64,
    last_updated: &str,
) -> ::serde_json::Value {
    let mut content = submitted;
    let mut meta = content
        .get("meta")
        .filter(|meta| meta.is_object())
        .cloned()
        .unwrap_or_else(|| ::serde_json::json!({}));
    meta["versionId"] = ::serde_json::json!(version.to_string());
    meta["lastUpdated"] = ::serde_json::json!(last_updated);
    let object = content
        .as_object_mut()
        .expect("the caller checked the submitted resource is a JSON object");
    object.insert("id".to_string(), ::serde_json::json!(id));
    object.insert("meta".to_string(), meta);
    content
}

/// `Ok` when the data store `datastore_id` names is serving its FHIR endpoint.
///
/// A data store serves once it has been created and until it is destroyed: while it
/// is still `CREATING`, and once it is `DELETING` or `DELETED`, its endpoint holds
/// nothing a caller may read or write, and an id no data store carries is the same
/// answer — so a caller cannot tell a data store that is not ready from one that does
/// not exist by probing the data plane. A store applying a configuration change
/// (`UPDATING`) is neither: the resources it holds are untouched by that change, so
/// it keeps serving them rather than taking the endpoint down for the span of an
/// update.
fn serving(
    store: &crate::state::ComAmazonawsHealthlakeHealthlakeState,
    datastore_id: &str,
) -> Result<(), Failure> {
    let serving = store.datastores.get(datastore_id).is_some_and(|datastore| {
        matches!(
            datastore.status,
            crate::state::DatastoreStatus::Active | crate::state::DatastoreStatus::Updating
        )
    });
    if serving {
        return Ok(());
    }
    Err(Failure::new(
        404,
        NOT_FOUND,
        format!(
            "No ACTIVE data store serves the FHIR endpoint '{}'",
            bounded(datastore_id)
        ),
    ))
}

/// A FHIR failure a helper found, before it is rendered.
///
/// Carried instead of a built response so a helper that refuses a request answers in
/// the same words [`outcome`] writes for every other refusal, and so a `Result`'s
/// error variant stays small.
struct Failure {
    /// The HTTP status the refusal carries.
    status: u16,
    /// The FHIR `IssueType` a client branches on.
    code: &'static str,
    /// The sentence for a human.
    diagnostics: String,
}

impl Failure {
    /// One failure, from the three facts it states.
    fn new(status: u16, code: &'static str, diagnostics: String) -> Self {
        Self {
            status,
            code,
            diagnostics,
        }
    }

    /// This failure as the `OperationOutcome` response it answers with.
    fn response(self) -> RtResponse {
        outcome(self.status, self.code, &self.diagnostics)
    }
}

/// The refusal of a path segment that is not a FHIR resource type name.
fn unknown_resource_type(resource_type: &str) -> RtResponse {
    outcome(
        404,
        NOT_FOUND,
        &format!(
            "'{}' is not a FHIR resource type",
            bounded(resource_type)
        ),
    )
}

/// The answer when this service's store cannot be reached: a server fault, never a
/// fresh store.
fn store_unavailable() -> RtResponse {
    outcome(
        500,
        EXCEPTION,
        "The HealthLake data store registry is unavailable",
    )
}

/// `Ok` when a submitted body is small enough to store, before it is parsed.
///
/// Stated once for both writing interactions: a count ceiling alone would admit the
/// count times whatever one body the transport accepts, so the size of ONE resource is
/// refused here and the retained totals in [`admits`].
fn submitted_within_size(body: &[u8]) -> Result<(), Failure> {
    if body.len() <= crate::state::MAX_FHIR_RESOURCE_BYTES {
        return Ok(());
    }
    Err(Failure::new(
        413,
        TOO_LONG,
        format!(
            "A FHIR resource this endpoint stores is at most {} bytes; the submitted one is \
             {} bytes",
            crate::state::MAX_FHIR_RESOURCE_BYTES,
            body.len()
        ),
    ))
}

/// `Ok` when the data store `datastore_id` can RETAIN a resource of `bytes` that
/// releases `released` bytes of versions this write drops, and whether that write adds a
/// resource (`retains_one_more`) or writes a further version of one it holds.
///
/// WHICH ceilings a write is held to is decided once, in
/// [`crate::state::ComAmazonawsHealthlakeHealthlakeState::admits_fhir_resource`], and is
/// shared with every other writer of a data store's resources — an import job's run
/// delivers its resources through the same decision — so a resource one of them would
/// not retain is not retained by the other either. What stays HERE is only how this
/// surface states the refusal: the FHIR `OperationOutcome` a FHIR client reads, one
/// sentence per ceiling.
fn admits(
    store: &mut crate::state::ComAmazonawsHealthlakeHealthlakeState,
    datastore_id: &str,
    bytes: usize,
    released: usize,
    retains_one_more: bool,
) -> Result<(), Failure> {
    let Err(ceiling) = store.admits_fhir_resource(datastore_id, bytes, released, retains_one_more)
    else {
        return Ok(());
    };
    Err(match ceiling {
        crate::state::FhirRetentionCeiling::ResourceCount => Failure::new(
            507,
            TOO_COSTLY,
            format!(
                "This data store already holds the maximum number of FHIR resources this \
                 emulator retains ({})",
                crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE
            ),
        ),
        crate::state::FhirRetentionCeiling::DatastoreBytes => Failure::new(
            507,
            TOO_COSTLY,
            format!(
                "This data store already retains as many FHIR resource bytes as this \
                 emulator holds for one data store ({})",
                crate::state::MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE
            ),
        ),
        crate::state::FhirRetentionCeiling::StoreBytes => Failure::new(
            507,
            TOO_COSTLY,
            format!(
                "This emulator already retains as many FHIR resource bytes as it holds \
                 across every data store ({})",
                crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
            ),
        ),
    })
}

/// Whether `segment` is a FHIR logical id.
///
/// FHIR R4's `id` primitive type: "Any combination of upper- or lower-case ASCII
/// letters, numerals, '-' and '.', with a length limit of 64 characters" — so exactly
/// `[A-Za-z0-9\-\.]{1,64}`. An `update` places a resource at a CALLER-chosen id, so this
/// is the rule that id has to satisfy before it becomes a key of the store.
///
/// `pub(crate)` because every writer of a data store's resources holds a
/// caller-supplied id to the same rule: an imported resource is stored under the
/// id its own JSON carries, so that id is checked here too.
pub(crate) fn is_logical_id(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_LOGICAL_ID_LEN
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Whether `segment` names a FHIR resource type.
///
/// FHIR resource type names are UpperCamelCase ASCII tokens (`Patient`,
/// `Observation`, `MedicationRequest`), so a segment that is not one names no type at
/// all. This emulator does not carry the R4 resource-type catalogue, so a WELL-FORMED
/// type name is served and the resource stored under it; which types a release
/// defines is its own gap, and a catalogue invented here would reject resource types
/// HealthLake accepts.
///
/// `pub(crate)` for the reason [`is_logical_id`] is: an imported resource is
/// stored under the type its own `resourceType` names, which is held to the same
/// rule as the type segment of a FHIR URL.
pub(crate) fn is_resource_type_name(segment: &str) -> bool {
    segment.len() <= MAX_RESOURCE_TYPE_LEN
        && segment.starts_with(|first: char| first.is_ascii_uppercase())
        && segment.chars().all(|c| c.is_ascii_alphanumeric())
}

/// One FHIR response: the status, the FHIR JSON media type, and the body.
fn fhir_response(status: u16, body: String) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    response
        .headers_mut()
        .insert("content-type", FHIR_JSON_CONTENT_TYPE);
    response
}

/// A FHIR failure, as the `OperationOutcome` resource a FHIR endpoint answers with.
///
/// `severity` follows the status: a server fault is `fatal`, everything a caller can
/// act on is `error` (FHIR R4 `IssueSeverity`). The `code` is the FHIR `IssueType` a
/// client branches on; the `diagnostics` sentence is for a human and carries only
/// bounded, control-character-free text.
fn outcome(status: u16, code: &str, diagnostics: &str) -> RtResponse {
    let severity = if status >= 500 { "fatal" } else { "error" };
    let body = ::serde_json::json!({
        "resourceType": "OperationOutcome",
        "issue": [{
            "severity": severity,
            "code": code,
            "diagnostics": diagnostics,
        }],
    });
    fhir_response(status, body.to_string())
}

