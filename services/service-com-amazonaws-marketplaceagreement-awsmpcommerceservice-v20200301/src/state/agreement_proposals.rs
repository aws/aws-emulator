// SPDX-License-Identifier: CC0-1.0

//! The agreement proposal a quote is created from, in the service that OWNS it.
//!
//! `CreateAgreementRequest$agreementProposalIdentifier` names "[t]he agreement
//! proposal signed by the proposer. The proposal includes the requested
//! resources and the terms that outline an agreement outcome" — a resource of
//! ANOTHER AWS Marketplace API: an offer's proposal, which the buyer-facing
//! catalog publishes as `GetOffer$agreementProposalId` ("[a]n encoded string to
//! be passed by the acceptor of the terms when creating an agreement").
//!
//! So the identifier a quote names is RESOLVED here against that catalog rather
//! than taken on trust: the proposal a request names exists only if the service
//! that owns offers reports an offer publishing it, which is what keeps an
//! agreement request from standing on a reference nothing backs. Nothing is
//! written: the catalog is read, and this service's own answer is the refusal or
//! the quote.
//!
//! LOCK ORDER: the catalog's guard is taken with NONE of this service's own held
//! — the proposal is resolved before the store guard is — and the catalog reads
//! nothing of this service, so the order cannot invert.

use super::*;

use ::service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state as catalog;

/// Whether `identifier` names an agreement proposal that is ACTIVE at `now` —
/// one the catalog publishes for an offer currently on sale.
///
/// `false` for an identifier the catalog publishes no offer for, for one whose
/// offer is no longer (or not yet) on sale, and on an engine holding no catalog
/// at all: in each case this service can resolve no active proposal to create a
/// quote from, which is the one condition its caller refuses on.
pub fn is_active<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
    now: SystemTime,
) -> bool {
    let Some(state) = catalog::service_state(engine) else {
        return false;
    };
    let now_secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    let active = state
        .lock()
        .holds_active_agreement_proposal(identifier, now_secs);
    active
}
