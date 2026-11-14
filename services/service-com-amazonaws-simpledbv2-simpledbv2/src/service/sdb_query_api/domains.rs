// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN actions of the Amazon SimpleDB 2009-04-15 API: `CreateDomain`,
//! `DeleteDomain`, `ListDomains` and `DomainMetadata`.
//!
//! All four answer from the one domain registry
//! ([`crate::state::simpledb_domains`]) — the same registry the v2 export operations
//! resolve a `domainName` against — so a domain created here is a domain
//! `StartDomainExport` can export, and a domain deleted here stops being one.

use std::time::UNIX_EPOCH;

use aws_smithy_runtime_api::http::Response as RtResponse;

use super::envelope::{self, QueryApiError};
use super::parameters::QueryParameters;
use super::{required_domain_name, SimpleDbQueryApi};
use crate::state::simpledb_domains::{Domain, MAX_LIST_DOMAINS};

/// The parameter naming the page size `ListDomains` is asked for.
const MAX_NUMBER_OF_DOMAINS_PARAMETER: &str = "MaxNumberOfDomains";

/// The parameter carrying the position `ListDomains` continues from.
const NEXT_TOKEN_PARAMETER: &str = "NextToken";

impl<Eng: Send + Sync + 'static> SimpleDbQueryApi<Eng> {
    /// `CreateDomain`: create the domain `DomainName` names.
    ///
    /// "CreateDomain is an idempotent operation; running it multiple times using the same
    /// domain name will not result in an error response" — so a name the account already
    /// holds is answered exactly as a new one, and the items that name already holds stay
    /// where they are. A NEW name past the account's domain quota is the published
    /// `NumberDomainsExceeded`.
    pub(super) fn create_domain(
        &self,
        parameters: &QueryParameters,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let domain_name = required_domain_name(parameters)?;
        let store = self.store()?;
        if !store.lock().create_domain(domain_name) {
            return Err(envelope::number_domains_exceeded());
        }
        Ok(envelope::response("CreateDomain", None, request_id))
    }

    /// `DeleteDomain`: remove the domain `DomainName` names, and every item it holds.
    ///
    /// "Running DeleteDomain on a domain that does not exist or running the function
    /// multiple times using the same domain name will not result in an error response",
    /// so the answer is the same document whether anything was removed or not.
    pub(super) fn delete_domain(
        &self,
        parameters: &QueryParameters,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let domain_name = required_domain_name(parameters)?;
        let store = self.store()?;
        let _ = store.lock().delete_domain(domain_name);
        Ok(envelope::response("DeleteDomain", None, request_id))
    }

    /// `ListDomains`: the domain names this account holds, one page at a time.
    ///
    /// The page size is the request's `MaxNumberOfDomains` when it names one and the
    /// server's own maximum otherwise, and a page that stopped short of the end carries
    /// the `NextToken` the next call continues from. A token this store did not issue
    /// names no position and is refused rather than honoured.
    pub(super) fn list_domains(
        &self,
        parameters: &QueryParameters,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let page_size = page_size(parameters)?;
        let store = self.store()?;
        let store = store.lock();
        let after = match parameters.value(NEXT_TOKEN_PARAMETER) {
            None => None,
            Some(token) => Some(
                store
                    .domain_page_cursor(token)
                    .ok_or_else(envelope::invalid_next_token)?,
            ),
        };
        // One name past the page is asked for, so whether there IS a next page is read
        // off the store rather than guessed from a full page.
        let mut names = store.domain_names_after(after.as_deref(), page_size.saturating_add(1));
        let more = names.len() > page_size;
        names.truncate(page_size);
        let mut result: String = names
            .iter()
            .map(|name| envelope::element("DomainName", name))
            .collect();
        if let Some(last) = names.last().filter(|_| more) {
            result.push_str(&envelope::element(
                NEXT_TOKEN_PARAMETER,
                &store.domain_page_token(last),
            ));
        }
        Ok(envelope::response(
            "ListDomains",
            Some(result),
            request_id,
        ))
    }

    /// `DomainMetadata`: the counts and sizes of the domain `DomainName` names.
    ///
    /// Every figure is counted off that domain's own items, so the metadata of a domain
    /// nothing has written to is all zeroes and every figure grows only with what a
    /// caller actually stored. A name the account does not hold is `NoSuchDomain`.
    pub(super) fn domain_metadata(
        &self,
        parameters: &QueryParameters,
        request_id: &str,
    ) -> Result<RtResponse, QueryApiError> {
        let domain_name = required_domain_name(parameters)?;
        let store = self.store()?;
        let store = store.lock();
        let domain = store
            .domain(domain_name)
            .ok_or_else(envelope::no_such_domain)?;
        Ok(envelope::response(
            "DomainMetadata",
            Some(metadata_result(domain)),
            request_id,
        ))
    }
}

/// The `DomainMetadataResult` of one domain, in the order the API documents its members.
///
/// `Timestamp` is "the data and time when metadata was calculated, in Epoch (UNIX)
/// seconds": the metadata reported here was true from the instant the domain last
/// changed, which is the instant the record carries, so a domain nothing has written to
/// reports the same timestamp on every read.
fn metadata_result(domain: &Domain) -> String {
    let counted = |name: &str, count: usize| envelope::element(name, &count.to_string());
    let timestamp = domain
        .changed_at()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    [
        counted("ItemCount", domain.item_count()),
        counted("ItemNamesSizeBytes", domain.item_names_size_bytes()),
        counted("AttributeNameCount", domain.attribute_names().len()),
        counted(
            "AttributeNamesSizeBytes",
            domain.attribute_names_size_bytes(),
        ),
        counted("AttributeValueCount", domain.attribute_value_count()),
        counted(
            "AttributeValuesSizeBytes",
            domain.attribute_values_size_bytes(),
        ),
        envelope::element("Timestamp", &timestamp.to_string()),
    ]
    .concat()
}
/// The page size one `ListDomains` call is served at: the request's
/// `MaxNumberOfDomains`, or the server's maximum when the request names none.
///
/// Amazon SimpleDB documents `MaxNumberOfDomains` as 1 to 100 with a default of 100, and
/// a value outside that range is the published `InvalidParameterValue` rather than a page
/// of a size the service does not offer.
fn page_size(parameters: &QueryParameters) -> Result<usize, QueryApiError> {
    let Some(requested) = parameters.value(MAX_NUMBER_OF_DOMAINS_PARAMETER) else {
        return Ok(MAX_LIST_DOMAINS);
    };
    let refusal = || {
        envelope::invalid_parameter_value(
            MAX_NUMBER_OF_DOMAINS_PARAMETER,
            requested,
            &format!(
                "{MAX_NUMBER_OF_DOMAINS_PARAMETER} must be between 1 and {MAX_LIST_DOMAINS}."
            ),
        )
    };
    let size: usize = requested.trim().parse().map_err(|_| refusal())?;
    if !(1..=MAX_LIST_DOMAINS).contains(&size) {
        return Err(refusal());
    }
    Ok(size)
}
