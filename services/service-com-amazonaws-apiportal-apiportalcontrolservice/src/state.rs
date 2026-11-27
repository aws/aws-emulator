// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apiportal-apiportalcontrolservice service.
//!
//! One store per emulator, reached by every handler through [`service_state`]. The
//! store holds this service's control plane; the product resource — the primary
//! resource of API Portal — lives in [`products`], which also owns the rules the
//! product handlers share (its id and ARN forms, its API-selection validation and the
//! shape every response reports it in).

mod products;

pub use products::*;

use std::collections::BTreeMap;

/// The region an ARN carries when the request's SigV4 credential scope names none the
/// modelled ARN patterns would accept.
const DEFAULT_REGION: &str = "us-east-1";

/// Wall-clock now, as milliseconds since the Unix epoch.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The region this request was signed for, for the ARNs the service mints.
///
/// Read from the SigV4 credential scope — the only region a request carries — and
/// refused back to [`DEFAULT_REGION`] when it is not a region the modelled
/// `ProductArn` pattern (`[a-z0-9\-]+`) would accept, so a minted ARN always
/// satisfies its own pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    match ::emulator_server::sigv4_credential_scope(&parts.headers) {
        Some(scope) if region_is_well_formed(&scope.region) => scope.region,
        _ => DEFAULT_REGION.to_string(),
    }
}

/// Whether `region` matches the region segment of the modelled ARN patterns.
fn region_is_well_formed(region: &str) -> bool {
    !region.is_empty()
        && region.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// A `nextToken` resuming after the resource id `id`.
///
/// The token encodes a POSITION, not a secret and not a capability: it names the
/// resource the previous page stopped on, which the caller already received. A caller
/// that forges or edits one can reach nothing it could not already list, and
/// [`decode_page_token`]'s caller refuses a token that names no resource it holds. It
/// is opaque in the sense the API contract needs: clients must not parse or construct
/// it, and its form is free to change.
pub fn encode_page_token(id: &str) -> String {
    ::hex::encode(id)
}

/// The resource id a `nextToken` resumes after, or `None` when the token was not
/// minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// Per-service state for com-amazonaws-apiportal-apiportalcontrolservice. A handler reaches it through [`service_state`].
///
/// The store holds the resources of exactly one account — the emulator's single
/// `engine::emulator_account_id()` — so a resource id is a resource's complete
/// identity here: no record carries an owner discriminator, because there is no second
/// account whose resources could be reached. Growth is bounded by [`MAX_PRODUCTS`],
/// which `CreateProduct` enforces, and released by `DeleteProduct`.
#[derive(Debug, Default)]
pub struct ComAmazonawsApiportalApiportalcontrolserviceState {
    /// Products, keyed by `productId`. A `BTreeMap` keeps one deterministic order for
    /// every caller, so `ListProducts` pages reproducibly and a `nextToken` naming the
    /// product a page stopped on resumes at exactly the next one.
    pub products: BTreeMap<String, Product>,
    /// Monotonic counter behind every `productId` this service mints, so an id is
    /// unique for the life of the store and is never reused by a later create.
    pub next_product_seq: u64,
}

impl ComAmazonawsApiportalApiportalcontrolserviceState {
    /// The next value of the product id counter. Unique for the life of the store.
    pub fn next_product_sequence(&mut self) -> u64 {
        self.next_product_seq += 1;
        self.next_product_seq
    }

    /// Materialise every transition the clock has already made due at `now_millis`.
    ///
    /// The only write a reporting operation makes: a product whose rule evaluation
    /// came due is `ACTIVE` from that instant onward whether or not anybody called, so
    /// its `updatedAt` is the instant the transition fell due, not the instant it was
    /// observed.
    pub fn settle(&mut self, now_millis: i64) {
        for product in self.products.values_mut() {
            product.settle(now_millis);
        }
    }

    /// Persist a newly minted product.
    pub fn insert_product(&mut self, product: Product) {
        self.products.insert(product.product_id.clone(), product);
    }

    /// Remove a product. Answers whether it was there to remove.
    pub fn remove_product(&mut self, product_id: &str) -> bool {
        self.products.remove(product_id).is_some()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApiportalApiportalcontrolserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApiportalApiportalcontrolserviceState>)
}
