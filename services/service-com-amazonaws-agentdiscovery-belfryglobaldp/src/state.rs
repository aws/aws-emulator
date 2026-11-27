// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-agentdiscovery-belfryglobaldp service.
//!
//! This service binds ONE operation, `SearchPublicRecords`, and it is `@readonly`: the
//! model declares no operation that publishes a public record, so the index a search
//! reports is written through [`ComAmazonawsAgentdiscoveryBelfryglobaldpState::insert_record`]
//! — the crawler/authoritative ingest seam a fixture or a sibling service writes through —
//! and read, never written, by the handler.

use crate::ops::types::{Identifier, InternalServerException, PublicRecord, ValidationException};

use ::service_common::page_tokens::PageTokenKey;

/// Per-service state for com-amazonaws-agentdiscovery-belfryglobaldp. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAgentdiscoveryBelfryglobaldpState {
    /// The public record index, in the order it was published: the stable order a
    /// search pages over, so two pages of one result set neither repeat nor skip a
    /// record.
    records: Vec<PublicRecord>,
    /// The secret this store's pagination tokens are authenticated under, so a
    /// position no search offered cannot be named by a value a caller assembled.
    page_token_key: PageTokenKey,
}

impl ComAmazonawsAgentdiscoveryBelfryglobaldpState {
    /// Publish `record` into the index, replacing the record its `identifier` already
    /// names and keeping that record's position.
    ///
    /// An identifier is what a public record is known by — the union carries either the
    /// domain it is published under or its URI — so re-publishing one updates the record
    /// rather than listing it twice; a record nothing names yet is appended, which is
    /// what makes the index's order the order it was published in.
    pub fn insert_record(&mut self, record: PublicRecord) {
        let identifier = identifier_value(&record.identifier).to_string();
        match self
            .records
            .iter_mut()
            .find(|held| identifier_value(&held.identifier) == identifier)
        {
            Some(held) => *held = record,
            None => self.records.push(record),
        }
    }

    /// The index, in publication order.
    pub fn records(&self) -> &[PublicRecord] {
        &self.records
    }

    /// The key this store's pagination tokens are minted and verified under.
    pub fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }
}

/// The string an `Identifier` carries, whichever member of the union it is: a public
/// record is identified by its domain or by its URI, and both are compared, prefixed
/// and reported as the text they are.
pub fn identifier_value(identifier: &Identifier) -> &str {
    match identifier {
        Identifier::Domain(domain) => domain.as_str(),
        Identifier::Uri(uri) => uri.as_str(),
    }
}

/// `ArdErrorCode$INVALID_ARGUMENT`, which the model makes the code of a refused
/// request on this surface. One of the model's own `ARD_ERROR_CODE_VALUES`.
pub const ERROR_CODE_INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";

/// `ArdErrorCode$INTERNAL_ERROR`, the code of a failure of the service itself. One of
/// the model's own `ARD_ERROR_CODE_VALUES`.
pub const ERROR_CODE_INTERNAL_ERROR: &str = "INTERNAL_ERROR";

/// The modelled `ValidationException` for a request this service refuses, carrying the
/// `errorCode` the ARD surface makes required of every error body.
///
/// Shared, so one refusal is worded one way however many places can raise it; each
/// operation lifts it into its own error channel with `.into()`.
pub fn invalid_argument(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(message.into()),
        error_code: Some(ERROR_CODE_INVALID_ARGUMENT.to_string()),
    }
}

/// The modelled `InternalServerException` for an engine carrying no store for this
/// service: a defect in how the emulator was assembled, which is exactly what the
/// model documents this error for.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some(
            "The request could not be completed: this service's record index is unavailable."
                .to_string(),
        ),
        error_code: Some(ERROR_CODE_INTERNAL_ERROR.to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAgentdiscoveryBelfryglobaldpState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAgentdiscoveryBelfryglobaldpState>)
}
