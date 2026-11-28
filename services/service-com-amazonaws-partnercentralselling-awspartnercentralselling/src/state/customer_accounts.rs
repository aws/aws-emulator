// SPDX-License-Identifier: CC0-1.0

//! Stored CustomerAccount records, and the helpers the CustomerAccount operations
//! share: minting ids and ARNs, settling the asynchronous matching step, resolving an
//! `Identifier` that may be an id or an ARN, and the modelled not-found error.
//!
//! They live beside the state struct rather than in one operation's file because six
//! operations read and write the same records.
//!
//! Nothing here can fail on what a caller sent. The two that take a caller's string
//! ([`ComAmazonawsPartnercentralsellingAwspartnercentralsellingState::customer_account_key`],
//! which resolves an `Identifier`, and
//! [`ComAmazonawsPartnercentralsellingAwspartnercentralsellingState::customer_account_by_token`],
//! which resolves a `ClientToken`) compare it for equality against a map key and against
//! stored values — no indexing, no slicing, no parsing, no arithmetic — and answer
//! `Option`. The rest work on stored values only: `settle_matching` walks the records
//! whose due instant has passed, `remove_customer_account` removes one and re-matches
//! what referred to it, and `mint_customer_account_id` formats a counter. Timestamps are
//! compared and advanced with saturating or `max` arithmetic ([`StoredCustomerAccount::touch`],
//! `super::millis_of`), so no value a request carries can overflow one.

use super::ComAmazonawsPartnercentralsellingAwspartnercentralsellingState;
use crate::ops::types::{AwsEnrichment, CustomerAccountData, ResourceNotFoundException, Tag};

/// `EnrichmentStatus` a freshly created or re-matched account carries. Source: the
/// `CreateCustomerAccount` doc — "The response returns the new customer account with a
/// status of `PENDING`" — and the `Status` member doc, "A new account starts as
/// PENDING".
pub const ENRICHMENT_STATUS_PENDING: &str = "PENDING";

/// `EnrichmentStatus` of an account matching found no customer for ("AWS couldn't
/// match the customer account to a customer").
pub const ENRICHMENT_STATUS_NO_MATCH_FOUND: &str = "NO_MATCH_FOUND";

/// `EnrichmentStatus` of an account whose matching outcome needs the caller's action
/// ("The customer account needs your action. See `ReasonCode` and `Message` for
/// details").
pub const ENRICHMENT_STATUS_ACTION_REQUIRED: &str = "ACTION_REQUIRED";

/// The `CustomerAccountFailureReason` for an account whose customer another account
/// already maps ("The customer is already mapped by another customer account.
/// `ExistingCustomerAccountArn` identifies that account").
pub const REASON_DUPLICATE_CUSTOMER_MATCH: &str = "DUPLICATE_CUSTOMER_MATCH";

/// How long after the call that starts it the matching step settles.
///
/// "Matching runs asynchronously. The response returns the new customer account with a
/// status of `PENDING`" — so `PENDING` is a transient status a caller must be able to
/// observe, and the account reaches its outcome a short span later. The span is a
/// stored due instant, never a sleep: nothing in this service waits.
pub const MATCHING_SPAN_MS: i64 = 3_000;

/// The most customer accounts this emulator holds at once.
///
/// A create past the ceiling is refused with the modelled
/// `ServiceQuotaExceededException` rather than accepted, so a caller cannot grow this
/// process's memory — or the cost of the scan every search runs — without limit. AWS
/// publishes its own customer-account quota and it is not this number; this is the
/// emulator's own ceiling, chosen well above what a client script needs.
pub const MAX_CUSTOMER_ACCOUNTS: usize = 200;

/// Default page size of `SearchCustomerAccounts`.
///
/// The model bounds `MaxResults` at `1..100` but documents no default, so this is the
/// emulator's choice: the same page size the service documents for its other paginated
/// read ([`super::DEFAULT_LIST_MAX_RESULTS`], "Default: 20").
pub const DEFAULT_SEARCH_MAX_RESULTS: i32 = 20;

/// Largest page `SearchCustomerAccounts` answers: the modelled maximum of its
/// `MaxResults` member (`range 1..100`). The handler clamps to it rather than trusting
/// the request, so one call cannot ask for the whole store.
pub const MAX_SEARCH_MAX_RESULTS: i32 = 100;

/// One stored CustomerAccount: everything a later read of it must be able to answer.
///
/// Timestamps are epoch milliseconds, the precision the `date-time` wire format
/// preserves, so the `LastModifiedDate` a caller echoes into `UpdateCustomerAccount`
/// compares equal to the stored one.
#[derive(Debug, Clone)]
pub struct StoredCustomerAccount {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    /// The customer details the caller provided, replaced whole by an update.
    pub account: CustomerAccountData,
    pub partner_account_identifier: Option<String>,
    pub tags: Vec<Tag>,
    /// The `ClientToken` the create that minted this account carried, and a fixed-size
    /// digest of the rest of that request.
    ///
    /// They live on the record rather than in a ledger of their own, so the tokens this
    /// service remembers are bounded by the same ceiling as the accounts
    /// ([`MAX_CUSTOMER_ACCOUNTS`]) and a deleted account forgets its token with the rest
    /// of itself. The digest is what makes the token's two documented outcomes
    /// separable: the same token with the same body answers the account the first call
    /// minted, the same token with a different body is a conflict. Only the digest is
    /// kept, so remembering a request costs a fixed 32 bytes however large it was, and
    /// neither value is ever returned to a caller.
    pub client_token: Option<String>,
    pub request_fingerprint: [u8; 32],
    /// One of [`ENRICHMENT_STATUS_PENDING`], [`ENRICHMENT_STATUS_NO_MATCH_FOUND`] or
    /// [`ENRICHMENT_STATUS_ACTION_REQUIRED`].
    pub enrichment_status: String,
    /// When the matching step this record is waiting on settles; `None` once it has.
    pub matching_due_ms: Option<i64>,
    /// What matching recorded about the customer it matched.
    ///
    /// Present only when the status is `ENRICHED`, and this emulator holds no
    /// directory of AWS customers to match an account against, so matching here ends
    /// in `NO_MATCH_FOUND` (or `ACTION_REQUIRED`) and this stays `None` rather than
    /// carrying values nothing computed.
    pub aws_enrichment: Option<AwsEnrichment>,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub existing_customer_account_arn: Option<String>,
    pub created_date_ms: i64,
    pub last_modified_date_ms: i64,
}

impl StoredCustomerAccount {
    /// The customer this account describes, as the value two accounts are the same
    /// customer when they share.
    ///
    /// The Data Universal Numbering System number identifies one company, so two
    /// accounts carrying the same `Duns` describe the same customer. An account that
    /// carries none describes a customer this emulator cannot identify, so it is never
    /// anyone's duplicate.
    pub fn customer_identity(&self) -> Option<&str> {
        self.account
            .duns
            .as_deref()
            .filter(|duns| !duns.is_empty())
    }

    /// Move `LastModifiedDate` to now, never backwards and never to the value it
    /// already held: the member doubles as `UpdateCustomerAccount`'s concurrency
    /// token, so two writes inside one millisecond must still be distinguishable.
    pub fn touch(&mut self) {
        self.last_modified_date_ms = (self.last_modified_date_ms + 1).max(super::now_ms());
    }

    /// Start matching over: the status goes back to `PENDING` and every value the
    /// previous outcome recorded is dropped.
    ///
    /// `UpdateCustomerAccount` — "AWS matches the account again with the updated
    /// information" — and a delete that removes the account another one was reported
    /// as duplicating both run matching again.
    pub fn start_matching(&mut self, now_ms: i64) {
        self.enrichment_status = ENRICHMENT_STATUS_PENDING.to_string();
        self.matching_due_ms = Some(now_ms + MATCHING_SPAN_MS);
        self.aws_enrichment = None;
        self.reason_code = None;
        self.message = None;
        self.existing_customer_account_arn = None;
    }
}

impl ComAmazonawsPartnercentralsellingAwspartnercentralsellingState {
    /// Mint the next `^cust-acc-[0-9a-z]{13}$` customer-account id. The 13 characters
    /// are the sequence in hexadecimal, so ids are unique and order lexicographically
    /// the way they were minted.
    pub fn mint_customer_account_id(&mut self) -> String {
        self.next_customer_account_seq += 1;
        format!("cust-acc-{:013x}", self.next_customer_account_seq)
    }

    /// The account `identifier` names in `catalog`, by id or by ARN ("Specifies the
    /// ARN or the identifier of the customer account").
    pub fn customer_account(
        &self,
        catalog: &str,
        identifier: &str,
    ) -> Option<&StoredCustomerAccount> {
        self.customer_account_key(catalog, identifier)
            .and_then(|key| self.customer_accounts.get(&key))
    }

    /// The same resolution, for a caller that writes to the record.
    pub fn customer_account_mut(
        &mut self,
        catalog: &str,
        identifier: &str,
    ) -> Option<&mut StoredCustomerAccount> {
        self.customer_account_key(catalog, identifier)
            .and_then(|key| self.customer_accounts.get_mut(&key))
    }

    /// The store key of the account `identifier` names, or `None` when the catalog
    /// holds no such account.
    pub fn customer_account_key(
        &self,
        catalog: &str,
        identifier: &str,
    ) -> Option<(String, String)> {
        let direct = (catalog.to_string(), identifier.to_string());
        if self.customer_accounts.contains_key(&direct) {
            return Some(direct);
        }
        self.customer_accounts
            .values()
            .find(|account| account.catalog == catalog && account.arn == identifier)
            .map(|account| (account.catalog.clone(), account.id.clone()))
    }

    /// The account a `ClientToken` already minted in `catalog`, if one did.
    pub fn customer_account_by_token(
        &self,
        catalog: &str,
        token: &str,
    ) -> Option<&StoredCustomerAccount> {
        self.customer_accounts.values().find(|account| {
            account.catalog == catalog && account.client_token.as_deref() == Some(token)
        })
    }

    /// Materialise every matching outcome the clock already owes.
    ///
    /// A record whose due instant has passed was owed its outcome at that instant even
    /// if nobody had called, so settling it here is what the clock did, not what this
    /// request did: every CustomerAccount operation, reporting ones included, calls
    /// this before it reads the store.
    pub fn settle_matching(&mut self, now_ms: i64) {
        let due: Vec<(String, String)> = self
            .customer_accounts
            .iter()
            .filter(|(_, account)| {
                account
                    .matching_due_ms
                    .is_some_and(|due_ms| due_ms <= now_ms)
            })
            .map(|(key, _)| key.clone())
            .collect();

        for key in due {
            let Some(account) = self.customer_accounts.get(&key) else {
                continue;
            };
            let mapped = self
                .existing_mapping(account)
                .map(|other| (other.id.clone(), other.arn.clone()));
            let Some(account) = self.customer_accounts.get_mut(&key) else {
                continue;
            };
            match mapped {
                // "The customer is already mapped by another customer account."
                Some((other_id, other_arn)) => {
                    account.enrichment_status = ENRICHMENT_STATUS_ACTION_REQUIRED.to_string();
                    account.reason_code = Some(REASON_DUPLICATE_CUSTOMER_MATCH.to_string());
                    account.existing_customer_account_arn = Some(other_arn);
                    account.message = Some(format!(
                        "The customer described by this account is already mapped by customer \
                         account {other_id}. Update this account with the details of another \
                         customer, or delete it and use {other_id}."
                    ));
                }
                // No directory of AWS customers to match against, so an account whose
                // customer no other account maps gets the outcome that says so.
                None => {
                    account.enrichment_status = ENRICHMENT_STATUS_NO_MATCH_FOUND.to_string();
                }
            }
            account.matching_due_ms = None;
        }
    }

    /// The account that already maps `account`'s customer: the first other account in
    /// the same catalog describing the same company. "First" is by creation, with the
    /// id as the tie-break, so the order is total and the same account keeps the
    /// mapping however often matching runs.
    fn existing_mapping(&self, account: &StoredCustomerAccount) -> Option<&StoredCustomerAccount> {
        let identity = account.customer_identity()?;
        self.customer_accounts
            .values()
            .filter(|other| other.catalog == account.catalog && other.id != account.id)
            .filter(|other| other.customer_identity() == Some(identity))
            .filter(|other| {
                (other.created_date_ms, other.id.as_str())
                    < (account.created_date_ms, account.id.as_str())
            })
            .min_by_key(|other| (other.created_date_ms, other.id.clone()))
    }

    /// Remove the account `identifier` names, and run matching again for every account
    /// that was reported as duplicating it — the account that mapped their customer is
    /// gone, so the outcome they carry no longer holds. Answers whether a record was
    /// removed.
    pub fn remove_customer_account(
        &mut self,
        catalog: &str,
        identifier: &str,
        now_ms: i64,
    ) -> bool {
        let Some(key) = self.customer_account_key(catalog, identifier) else {
            return false;
        };
        let Some(removed) = self.customer_accounts.remove(&key) else {
            return false;
        };
        for account in self.customer_accounts.values_mut() {
            if account.existing_customer_account_arn.as_deref() == Some(removed.arn.as_str()) {
                account.start_matching(now_ms);
            }
        }
        true
    }
}

/// The ARN of a customer account. The modelled `CustomerAccountArn` is
/// `^arn:[a-z\-]*:partnercentral:[a-z0-9\-]*:[0-9]{12}:catalog/[a-zA-Z]+/customer-account/cust-acc-[0-9a-z]{13}$`,
/// so the resource path carries the catalog the account belongs to.
pub fn customer_account_arn(region: &str, catalog: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "partnercentral",
        region,
        &format!("catalog/{catalog}/customer-account/{id}"),
    )
}

/// A modelled `ResourceNotFoundException` for a customer account that is not in the
/// requested catalog.
pub fn customer_account_not_found(catalog: &str, identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Customer account `{identifier}` was not found in catalog `{catalog}`."
        )),
    }
}
