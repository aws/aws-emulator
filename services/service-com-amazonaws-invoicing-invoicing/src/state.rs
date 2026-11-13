// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-invoicing-invoicing service.
//!
//! The invoice-unit lifecycle (`CreateInvoiceUnit` / `GetInvoiceUnit` /
//! `ListInvoiceUnits` / `UpdateInvoiceUnit` / `DeleteInvoiceUnit`) and the tag plane
//! (`TagResource` / `UntagResource` / `ListTagsForResource`) are served entirely from
//! the maps below: every response those operations return is computed from what an
//! earlier request stored here.
//!
//! The procurement-portal-preference plane
//! (`CreateProcurementPortalPreference` / `ListProcurementPortalPreferences`) is served
//! the same way: a create persists one [`StoredProcurementPortalPreference`] and the
//! list reports exactly what is persisted, one summary per stored preference.
//!
//! An invoice unit keeps its superseded configurations as well as its current one,
//! because `GetInvoiceUnit` and `ListInvoiceUnits` both accept `AsOf` and the model
//! documents `LastModified` as "the version of invoice unit configuration used to
//! create the invoices" — so a read at an instant must answer with the configuration
//! that was in effect then, not with the latest one.
//!
//! For the same reason a DELETE RETIRES an invoice unit rather than erasing it: the
//! model documents `AsOf` as "the state of an invoice unit at a specified time. You can
//! see legacy invoice units that are currently deleted if the `AsOf` time is set to
//! before it was deleted", so the instant a unit was deleted at is part of its history
//! and a read at an earlier instant still has to answer with it. A retired unit carries
//! a [`StoredInvoiceUnit::deleted_at`] tombstone, is reported by a point-in-time read
//! whose instant falls between its creation and its deletion, and is absent from every
//! read of the state NOW — so the live surface (a create of the same `Name`, a tag
//! write, an update, a second delete) sees it as gone.
//!
//! The procurement-portal plane keeps its preferences in
//! [`ComAmazonawsInvoicingInvoicingState::procurement_portal_preferences`]: a preference is
//! created by `CreateProcurementPortalPreference`, read back by
//! `GetProcurementPortalPreference`, rewritten by `PutProcurementPortalPreference`,
//! moved between verification statuses by `UpdateProcurementPortalPreferenceStatus`,
//! erased by `DeleteProcurementPortalPreference` and validated through
//! `SendProcurementPortalValidation` / `VerifyProcurementPortalValidation`, each one stored
//! as the modelled `ProcurementPortalPreference` a reader reports, so its `Version`,
//! `CreateDate` and `LastUpdateDate` are what the last write left rather than anything
//! computed per read. There is no point-in-time read on this plane, so a DELETE erases
//! the entry rather than tombstoning it as an invoice unit's delete does.
//!
//! `Version` and `LastUpdateDate` are advanced by the calls that SUCCESSFULLY WRITE a
//! preference — the create, the configuration update, the status update, and the
//! validation calls that record a code or a submission against it — and by nothing else
//! ([`StoredProcurementPortalPreference::record_write`]): one version per write the
//! service applied, so a call that answered an error before writing anything leaves both
//! exactly as the last write left them. What the validation calls write is the code
//! `SendProcurementPortalValidation` issued ([`IssuedPortalValidation`]) and how many
//! submissions have been refused since it was issued
//! ([`StoredProcurementPortalPreference::failed_verification_attempts`], which
//! [`StoredProcurementPortalPreference::issue_validation`] starts again with each code,
//! because the budget a refusal reports is one code's). Neither is a modelled member, so a
//! reader reports the WRITE those calls performed rather than what they wrote.
//!
//! The PORTALS themselves are not stored beside the preferences, because no operation of
//! this service creates one: they are the catalog [`procurement_portals`] derives from
//! the modelled `ProcurementPortalName` values, which `ListProcurementPortals` reports
//! and which a `PortalIdentifier` on any other operation of this plane resolves against.
//!
//! `ListProcurementPortalSuppliers` is served from those same preferences rather than
//! from a registry of its own: the suppliers CONFIGURED for a portal are the supplier
//! identities the stored preferences name against that portal, which
//! [`ComAmazonawsInvoicingInvoicingState::procurement_portal_suppliers`] computes — so a
//! portal no preference configures reports no suppliers, and nothing is invented for one.
//!
//! Beside the invoice units sits the INVOICE registry [`ComAmazonawsInvoicingInvoicingState::invoices`],
//! the one copy of the invoices this emulator holds, which `ListInvoiceSummaries`
//! reports. An invoice is ISSUED by AWS out of a billing period's activity: no
//! operation of this service's API creates one (`smithy_query(query="list_operations")`
//! lists one create, `CreateInvoiceUnit`), and this emulator runs no billing, so
//! nothing mints an invoice here either. The registry therefore starts out EMPTY —
//! which is the answer a reader owes for an account that has no invoices — and a
//! reader reports exactly what it holds rather than synthesising amounts for charges
//! nothing incurred.
//!
//! A registry entry is one [`StoredInvoice`]: the invoice as a reader reports it, plus
//! the DOCUMENTS issued with it, which `GetInvoicePDF` hands out pre-signed download URLs
//! for and `crate::service`'s document plane serves. Where those URLs lead and what is
//! served there lives in [`invoice_documents`], beside the registry the documents are
//! rendered from.

mod invoice_documents;

pub use invoice_documents::*;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;
use ::rand::Rng;

use crate::ops::types::{
    AccessDeniedException, Contact, EinvoiceDeliveryPreference, FeatureConfigurations,
    InternalServerException, InvoiceConfiguration, InvoiceProfile, InvoiceSummary, InvoiceUnit,
    InvoiceUnitRule, MarketplacePunchOutPreference, MessageKey, ProcurementPortal,
    ProcurementPortalPreference, ProcurementPortalPreferenceSelector,
    ProcurementPortalPreferenceSummary, ProcurementPortalSupplier, ResourceNotFoundException,
    ResourceTag, TestEnvPreferenceInput, ValidationException,
    EINVOICE_DELIVERY_ATTACHMENT_TYPE_VALUES, EINVOICE_DELIVERY_DOCUMENT_TYPE_VALUES,
    PROCUREMENT_PORTAL_NAME_VALUES,
};

/// Per-service state for com-amazonaws-invoicing-invoicing. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsInvoicingInvoicingState {
    /// Every invoice unit this emulator holds, LIVE AND RETIRED, keyed by its
    /// `InvoiceUnitArn`. A `BTreeMap` so `ListInvoiceUnits` walks a stable,
    /// deterministic order and a pagination token (a position in that order) stays
    /// meaningful across calls.
    ///
    /// A retired unit (one [`StoredInvoiceUnit::deleted_at`] is set on) is history, not
    /// a resource: reach the live surface through [`Self::live_invoice_unit`],
    /// [`Self::live_invoice_unit_mut`] and [`Self::live_invoice_units`] rather than
    /// indexing this map, and read history through
    /// [`StoredInvoiceUnit::version_as_of`], which answers `None` for an instant the
    /// unit did not exist at — before it was created or at or after it was deleted.
    pub invoice_units: BTreeMap<String, StoredInvoiceUnit>,
    /// Every invoice this emulator holds, keyed by its `InvoiceId`: the modelled
    /// `InvoiceSummary` a reader reports it with, and the documents issued with it
    /// ([`StoredInvoice`]) — one copy, so two readers of one invoice cannot disagree. A
    /// `BTreeMap` so `ListInvoiceSummaries` walks a stable, deterministic order and a
    /// pagination token (a position in that order) stays meaningful across calls.
    ///
    /// Empty until something puts an invoice here, because nothing in this service's
    /// API issues one — see the module docs.
    pub invoices: BTreeMap<String, StoredInvoice>,
    /// `ResourceTags` per resource ARN, as a create or `TagResource` persisted them and
    /// `ListTagsForResource` reports them, for every resource kind this service mints.
    /// One tag per key: a later write of a key already set replaces that key's value.
    pub resource_tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The result an `@idempotent` operation already produced for a `ClientToken`,
    /// keyed by `(operation, target, request digest, token)`: one token replayed against
    /// another operation, against another invoice unit, or carrying any other request
    /// than the recorded one is a DIFFERENT request and must not replay this one's
    /// result. Bounded by [`MAX_IDEMPOTENT_RECORDS`].
    idempotent_results: BTreeMap<IdempotencyKey, String>,
    /// The keys of [`Self::idempotent_results`] in the order they were recorded, so the
    /// oldest record is the one evicted when the bound is reached.
    idempotent_record_order: VecDeque<IdempotencyKey>,
    /// Every procurement portal preference this emulator holds, keyed by its
    /// `ProcurementPortalPreferenceArn` — the one copy of a preference, so the
    /// operations of the portal plane cannot disagree about whether one exists. A
    /// `BTreeMap` so a listing walks a stable, deterministic order and a pagination token
    /// (a position in that order) stays meaningful across calls.
    ///
    /// Empty until a create puts a preference here:
    /// `CreateProcurementPortalPreference` is the operation that does so,
    /// `GetProcurementPortalPreference` reads one back,
    /// `PutProcurementPortalPreference` rewrites one,
    /// `DeleteProcurementPortalPreference` removes one and
    /// `ListProcurementPortalPreferences` reports exactly what it stored. An ARN naming
    /// no preference resolves to nothing — which is what every reader of this plane
    /// reports as the modelled `ResourceNotFoundException`, rather than claiming a
    /// configuration or a validation state for a preference that does not exist.
    pub procurement_portal_preferences: BTreeMap<String, StoredProcurementPortalPreference>,
    /// Source of invoice-unit ids for ARN minting.
    next_invoice_unit_id: u64,
    /// Source of procurement-portal-preference ids for ARN minting.
    next_procurement_portal_preference_id: u64,
    /// The validation codes this store has already issued, most recent last, so a fresh
    /// draw is not handed a code another validation of this run is still identified by
    /// ([`Self::issue_portal_validation_code`]). Bounded by
    /// [`MAX_REMEMBERED_PORTAL_VALIDATION_CODES`]: sends are caller-driven, so past that
    /// many the code issued longest ago is forgotten and may be drawn again.
    ///
    /// It decides nothing a caller can observe about the NEXT code — the draw is the
    /// system's randomness either way — and no reader reports it.
    recent_portal_validation_codes: VecDeque<String>,
    /// The secret this store authenticates the DOCUMENT ADDRESSES it hands out under, so an
    /// address `GetInvoicePDF` minted is one no caller could have assembled and no other
    /// process can produce (see [`invoice_documents`] and [`Self::document_token`]). Drawn
    /// from the system's randomness when the store is created, held only in memory, never
    /// reported and never logged.
    document_address_key: ::service_common::page_tokens::PageTokenKey,
}

/// What an `@idempotent` replay record is keyed by: the operation, the request's target
/// (the resource it names, the name it creates, or the preference identity it creates),
/// a digest of the rest of the request ([`idempotent_request_digest`]) and the caller's
/// `ClientToken`.
///
/// The digest is part of the key because a `ClientToken` identifies a REPEAT OF ONE
/// REQUEST: a record may answer only a request carrying the same members the recorded
/// call carried, so a token reused with any other member values falls through to the
/// handler and is served on its own merits.
type IdempotencyKey = (String, String, String, String);

/// How many invoice units this emulator holds. The model's `ValidationExceptionReason`
/// declares `maxInvoiceUnitsExceeded`, so the real service refuses a create once its own
/// (account-adjustable) maximum is reached; this constant is the emulator's maximum. It
/// bounds the units that EXIST (the registry also holds the tombstones a delete leaves,
/// bounded by [`MAX_RETIRED_INVOICE_UNITS`]) and the per-request walks over them.
pub const MAX_INVOICE_UNITS: usize = 1000;

/// How many tags ONE RESOURCE holds — a quota on the resource, independent of how many
/// tags one call may carry.
///
/// `ResourceTagList` is modelled `length 0..200` as `TagResource`'s input
/// (`smithy_query(query="operation", operation="TagResource,ListTagsForResource")`), but
/// that is the bound on ONE CALL's array: a per-resource quota equal to it could never
/// be reached by any legal sequence of calls, because no single call may carry more than
/// the quota either. The per-resource quota is therefore the AWS-wide tagging quota —
/// "Maximum number of tags per resource – 50" (AWS General Reference, "Tagging your
/// resources") — strictly below the per-call maximum, so a caller tagging in batches
/// reaches it. A write that would push a resource past it is refused ATOMICALLY with the
/// modelled `ServiceQuotaExceededException` (HTTP 402); nothing of the refused batch
/// persists.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The sentence the per-resource tag quota is refused with: a CONSTANT, the service's
/// own canned wording as the recorded 402 of this service states it — it names no ARN
/// and no count, so a client matching it matches every one of these refusals.
pub const TAG_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request exceeds the maximum number of tags allowed per resource.";

/// How many RETIRED invoice units this emulator keeps for point-in-time reads. Deletes
/// are caller-driven, so the tombstones are bounded the same way the configuration
/// history is: past this many the unit deleted longest ago is dropped, and an `AsOf`
/// read can no longer answer with it.
const MAX_RETIRED_INVOICE_UNITS: usize = 1000;

/// How many procurement portal preferences this emulator holds. AWS publishes no
/// number for this preview API, so this is the emulator's own maximum; a create past it
/// is the modelled `ServiceQuotaExceededException` (HTTP 402), which
/// `CreateProcurementPortalPreference` declares and whose documentation is "the request
/// was rejected because it attempted to create resources beyond the current Amazon Web
/// Services account limits". It also bounds the registry and the per-request walks over
/// it.
pub const MAX_PROCUREMENT_PORTAL_PREFERENCES: usize = 100;

/// How many `@idempotent` replay records this service keeps, across every operation and
/// `ClientToken`. The key is caller-supplied, so without a bound a long-lived emulator
/// would accumulate one entry per distinct token for the life of the process; past this
/// many the oldest record is dropped, and a replay of a token that old mints a new
/// result instead of replaying.
const MAX_IDEMPOTENT_RECORDS: usize = 1024;

/// How many superseded configurations one invoice unit keeps for `AsOf` reads. Updates
/// are caller-driven, so the history is bounded the same way: past this many the oldest
/// configuration is dropped, and an `AsOf` older than the oldest retained one reads
/// that oldest retained configuration.
const MAX_RETAINED_VERSIONS: usize = 32;

/// How many decimal digits a procurement-portal validation code carries: the six-digit
/// form the portal-validation flow's `Code` is quoted in.
pub const PORTAL_VALIDATION_CODE_DIGITS: usize = 6;

/// How many codes [`PORTAL_VALIDATION_CODE_DIGITS`] digits span — the value space one
/// code is drawn uniformly from.
const PORTAL_VALIDATION_CODE_SPACE: u32 = 1_000_000;

/// How many issued codes a store remembers for the sake of not drawing one of them again.
/// Far below [`PORTAL_VALIDATION_CODE_SPACE`], so a draw is almost never discarded for
/// repeating a remembered code and the codes left to draw stay all but the whole space.
const MAX_REMEMBERED_PORTAL_VALIDATION_CODES: usize = 1024;

/// How many times [`ComAmazonawsInvoicingInvoicingState::issue_portal_validation_code`]
/// re-draws when the draw repeats a code it still remembers. A repeat has probability at
/// most `MAX_REMEMBERED_PORTAL_VALIDATION_CODES / PORTAL_VALIDATION_CODE_SPACE`, so this
/// many independent repeats cannot be reached in practice; the bound is here so the mint
/// cannot spin on its own bookkeeping.
const PORTAL_VALIDATION_CODE_DRAWS: usize = 8;

/// One procurement-portal validation code, drawn uniformly from the whole
/// [`PORTAL_VALIDATION_CODE_DIGITS`]-digit space, zero-padded to that width.
///
/// The draw is the maintained generator's own uniform range draw over a cryptographically
/// secure generator seeded from the operating system's randomness, rather than sampling
/// written here: a code the service issues out of band and then gates a verification on is
/// only worth as much as the generator it came from.
fn draw_portal_validation_code() -> String {
    let code = ::rand::rng().random_range(0..PORTAL_VALIDATION_CODE_SPACE);
    format!("{code:0width$}", width = PORTAL_VALIDATION_CODE_DIGITS)
}

impl ComAmazonawsInvoicingInvoicingState {
    /// Mint the ARN of a new invoice unit: `arn:aws:invoicing::<account>:invoice-unit/<id>`.
    ///
    /// The id is zero-padded hex of a monotonic counter, so an ARN is never reused and
    /// the `BTreeMap` order of `invoice_units` is creation order.
    pub fn mint_invoice_unit_arn(&mut self) -> String {
        self.next_invoice_unit_id = self.next_invoice_unit_id.wrapping_add(1);
        let id = self.next_invoice_unit_id;
        ::engine::emulator_arn("invoicing", "", &format!("invoice-unit/{id:012x}"))
    }

    /// The invoice unit `arn` names RIGHT NOW: the stored unit, unless it has been
    /// retired, in which case there is no such resource any more and a reader of the
    /// current state answers `ResourceNotFoundException`.
    ///
    /// Every operation that acts on the live resource \u2014 a read with no `AsOf`, an
    /// update, a tag write, a second delete \u2014 resolves the ARN through here, so one
    /// retired unit cannot look present to one operation and absent to another.
    pub fn live_invoice_unit(&self, arn: &str) -> Option<&StoredInvoiceUnit> {
        self.invoice_units.get(arn).filter(|unit| unit.is_live())
    }

    /// [`Self::live_invoice_unit`], for a caller that goes on to change the unit.
    pub fn live_invoice_unit_mut(&mut self, arn: &str) -> Option<&mut StoredInvoiceUnit> {
        self.invoice_units
            .get_mut(arn)
            .filter(|unit| unit.is_live())
    }

    /// Every invoice unit that exists right now, in `InvoiceUnitArn` order. The retired
    /// ones are history and are reached only through a point-in-time read.
    pub fn live_invoice_units(&self) -> impl Iterator<Item = &StoredInvoiceUnit> {
        self.invoice_units.values().filter(|unit| unit.is_live())
    }

    /// How many invoice units exist right now \u2014 what [`MAX_INVOICE_UNITS`] bounds. The
    /// tombstones a delete leaves behind are history and do not consume the account's
    /// invoice-unit allowance.
    pub fn live_invoice_unit_count(&self) -> usize {
        self.live_invoice_units().count()
    }

    /// Retire the invoice unit `arn` names at `at`, keeping it as history: it stops
    /// being a resource (every live read now reports it missing) while a point-in-time
    /// read before `at` still answers with the configuration that was in effect then.
    ///
    /// `false` when `arn` names no LIVE invoice unit, which a delete reports as
    /// `ResourceNotFoundException` \u2014 an already-retired unit included, because the
    /// resource is already gone.
    pub fn retire_invoice_unit(&mut self, arn: &str, at: DateTime) -> bool {
        match self.live_invoice_unit_mut(arn) {
            Some(unit) => unit.deleted_at = Some(at),
            None => return false,
        }
        // Bounded history: deletes are caller-driven, so keep only the most recently
        // retired units rather than growing this map without limit.
        while self.retired_invoice_unit_count() > MAX_RETIRED_INVOICE_UNITS {
            let oldest = self
                .invoice_units
                .values()
                .filter_map(|unit| {
                    unit.deleted_at
                        .map(|deleted| (instant_key(&deleted), unit.arn.clone()))
                })
                .min();
            match oldest {
                Some((_, arn)) => self.invoice_units.remove(&arn),
                None => break,
            };
        }
        true
    }

    /// How many retired invoice units this emulator is holding as history.
    fn retired_invoice_unit_count(&self) -> usize {
        self.invoice_units
            .values()
            .filter(|unit| !unit.is_live())
            .count()
    }

    /// Mint the ARN of a new procurement portal preference:
    /// `arn:aws:invoicing::<account>:procurement-portal-preference/<id>`, the form
    /// `ProcurementPortalPreferenceArnString`'s `@pattern` spells
    /// (`smithy_query(query="shape", shape="ProcurementPortalPreference")`).
    ///
    /// The id is zero-padded hex of a monotonic counter, so it is inside that pattern's
    /// character class, an ARN is never reused, and the `BTreeMap` order of
    /// `procurement_portal_preferences` is creation order.
    pub fn mint_procurement_portal_preference_arn(&mut self) -> String {
        self.next_procurement_portal_preference_id =
            self.next_procurement_portal_preference_id.wrapping_add(1);
        let id = self.next_procurement_portal_preference_id;
        ::engine::emulator_arn(
            "invoicing",
            "",
            &format!("procurement-portal-preference/{id:012x}"),
        )
    }

    /// Whether this service holds a resource named by `arn`.
    ///
    /// The tag plane (`TagResource` / `UntagResource` / `ListTagsForResource`) is keyed
    /// by ARN alone and accepts the ARN of any resource of this service (`TagrisArn`'s
    /// `@pattern` constrains the service and the account, not the resource kind), so
    /// "does this resource exist" has to be asked of every kind this service mints — an
    /// invoice unit and a procurement portal preference. An invoice unit a delete has
    /// retired is history rather than a resource, so it is not one of them.
    pub fn resource_exists(&self, arn: &str) -> bool {
        self.live_invoice_unit(arn).is_some()
            || self.procurement_portal_preferences.contains_key(arn)
    }

    /// The preference that already carries `identity`, if this account holds one. The
    /// identity is the portal and the buyer/supplier pair the preference routes between
    /// (see [`procurement_portal_preference_identity`]), which is what makes a second
    /// create of the same preference a conflict rather than a second resource.
    pub fn procurement_portal_preference_by_identity(
        &self,
        identity: &str,
    ) -> Option<&StoredProcurementPortalPreference> {
        self.procurement_portal_preferences
            .values()
            .find(|preference| preference.identity() == identity)
    }

    /// The suppliers CONFIGURED for the procurement portal `portal_name`, keyed by
    /// [`procurement_portal_supplier_key`] so the walk is in one stable order and a
    /// pagination token (a position in that order) stays meaningful across calls.
    ///
    /// A supplier of a portal is not a resource of its own: no operation of this service
    /// creates one (`smithy_query(query="list_operations")` lists one portal-plane create,
    /// `CreateProcurementPortalPreference`), so the suppliers configured for a portal are
    /// exactly the supplier identities the preferences stored AGAINST THAT PORTAL name
    /// configure, and nothing is reported for a portal no preference names. Each
    /// configured identity is reported once however many preferences carry it, because
    /// the answer is a list of SUPPLIERS rather than of preferences.
    ///
    /// One preference configures one supplier identity per ENVIRONMENT it describes: its
    /// own members describe the production environment and its `TestEnvPreference` — "the
    /// test environment of the procurement portal" — the test one, which is what
    /// `ProcurementPortalSupplier$Environment`'s `PROD`/`TEST`
    /// ([`crate::ops::types::PROCUREMENT_PORTAL_ENV_VALUES`]) distinguishes. A preference
    /// that configures no test environment therefore contributes no `TEST` supplier.
    ///
    /// `SellerOfRecord` is singular on a supplier while a preference's
    /// `Selector$SellerOfRecords` is "the list of seller of record IDs to which this
    /// preference applies", so a preference that selects seller-of-record ids configures
    /// its supplier identity once per selected id; one that selects none applies to every
    /// seller of record and so configures it with no `SellerOfRecord` at all.
    ///
    /// `CountryCode` is left absent: no member of a stored preference carries a country
    /// (`smithy_query(query="shape", shape="ProcurementPortalPreference")`), so this
    /// reader reports what state holds rather than inventing one.
    ///
    /// The derived set is BOUNDED by the registry and the model rather than by the
    /// request: at most [`MAX_PROCUREMENT_PORTAL_PREFERENCES`] preferences exist (a
    /// create past that is refused), each contributing at most the two environments
    /// above times the `@length(min: 0, max: 100)` of
    /// `ProcurementPortalPreferenceSelector$SellerOfRecords` (`smithy_query(query="shape",
    /// shape="ProcurementPortalPreferenceSelector")`, which a write outside that length
    /// is refused against) — so this walk and the map it builds hold at most
    /// `MAX_PROCUREMENT_PORTAL_PREFERENCES * 2 * 100` entries, whatever a reader asks
    /// for. It is derived per call rather than cached, which is what keeps this read
    /// from writing anything, and the response itself is bounded far lower by
    /// `MaxResults`.
    pub fn procurement_portal_suppliers(
        &self,
        portal_name: &str,
    ) -> BTreeMap<String, ProcurementPortalSupplier> {
        let mut suppliers = BTreeMap::new();
        for stored in self.procurement_portal_preferences.values() {
            let preference = &stored.preference;
            if preference.procurement_portal_name != portal_name {
                continue;
            }
            // The seller-of-record ids this preference applies to, or one `None` for a
            // preference that selects none and so applies to all of them.
            let selected_sellers: Vec<Option<&str>> = match preference
                .selector
                .as_ref()
                .and_then(|selector| selector.seller_of_records.as_deref())
            {
                Some(ids) if !ids.is_empty() => ids.iter().map(|id| Some(id.as_str())).collect(),
                _ => vec![None],
            };
            let environments = [(PORTAL_ENV_PROD, preference.supplier_identifier.as_str())]
                .into_iter()
                .chain(
                    preference
                        .test_env_preference
                        .as_ref()
                        .map(|test| (PORTAL_ENV_TEST, test.supplier_identifier.as_str())),
                );
            for (environment, supplier_identifier) in environments {
                for seller_of_record in &selected_sellers {
                    let supplier = ProcurementPortalSupplier {
                        supplier_identifier: supplier_identifier.to_string(),
                        seller_of_record: seller_of_record.map(str::to_string),
                        country_code: None,
                        environment: Some(environment.to_string()),
                    };
                    suppliers.insert(procurement_portal_supplier_key(&supplier), supplier);
                }
            }
        }
        suppliers
    }

    /// A procurement-portal validation code: six decimal digits, the form the
    /// portal-validation flow's `Code` is quoted in.
    ///
    /// DRAWN, per issue, from a cryptographically secure random generator
    /// ([`draw_portal_validation_code`]) — uniformly over the whole six-digit space, and
    /// from nothing a caller supplied, nothing a reader is told and nothing this process
    /// has already done. It is the code a submission has to MATCH before the validation of
    /// a portal connection completes, and the real service delivers it out of band, inside
    /// the test e-invoice it posts to the configured portal: a caller that never received
    /// it cannot complete the validation, which is exactly what a code derived from a
    /// counter, the clock or the request would hand it.
    ///
    /// The drawn code is not returned on the wire — the modelled response carries only the
    /// preference's ARN — so the code the service issued is kept here (see
    /// [`IssuedPortalValidation`]) for `VerifyProcurementPortalValidation` to check a
    /// submitted code against.
    ///
    /// Two validations issued in one run do not share a code: a draw that repeats one of
    /// the codes this store still remembers
    /// ([`Self::recent_portal_validation_codes`]) is discarded and drawn again.
    ///
    /// `SendProcurementPortalValidation` is the operation that issues one: the model
    /// documents it as initiating the validation by issuing a validation code, so this is
    /// where the code it records against a preference comes from.
    pub fn issue_portal_validation_code(&mut self) -> String {
        let mut code = draw_portal_validation_code();
        for _ in 1..PORTAL_VALIDATION_CODE_DRAWS {
            if !self.recent_portal_validation_codes.contains(&code) {
                break;
            }
            code = draw_portal_validation_code();
        }
        self.recent_portal_validation_codes.push_back(code.clone());
        // Bounded memory: sends are caller-driven, so only the most recently issued codes
        // are remembered rather than every code this process ever drew.
        while self.recent_portal_validation_codes.len() > MAX_REMEMBERED_PORTAL_VALIDATION_CODES {
            self.recent_portal_validation_codes.pop_front();
        }
        code
    }

    /// The ARN an earlier call of `operation` already returned for this `target`,
    /// `request_digest` and `client_token`, if this is a replay of that request. `None`
    /// when the request carries no token, or carries one this operation has not seen
    /// against this target AND this request — a token repeated with a different request
    /// is not a replay of the recorded one, so the caller's request runs.
    pub fn replayed_result(
        &self,
        operation: &str,
        target: &str,
        request_digest: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let token = client_token?;
        self.idempotent_results
            .get(&(
                operation.to_string(),
                target.to_string(),
                request_digest.to_string(),
                token.to_string(),
            ))
            .cloned()
    }

    /// Record the ARN `operation` returned for this `target`, `request_digest` and
    /// `client_token`, so a replay of that same request answers with this same ARN
    /// instead of changing state a second time.
    pub fn record_result(
        &mut self,
        operation: &str,
        target: &str,
        request_digest: &str,
        client_token: Option<&str>,
        arn: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        let key = (
            operation.to_string(),
            target.to_string(),
            request_digest.to_string(),
            token.to_string(),
        );
        if self
            .idempotent_results
            .insert(key.clone(), arn.to_string())
            .is_none()
        {
            self.idempotent_record_order.push_back(key);
        }
        // Bounded store: evict the oldest records once the cap is reached, so a stream
        // of distinct caller-supplied tokens cannot grow this map without limit.
        while self.idempotent_record_order.len() > MAX_IDEMPOTENT_RECORDS {
            if let Some(oldest) = self.idempotent_record_order.pop_front() {
                self.idempotent_results.remove(&oldest);
            }
        }
    }

    /// Every LINKED account this service's state names under the requester's
    /// management-account organization.
    ///
    /// An operation documented over "linked accounts under the requester management
    /// account organization" (`BatchGetInvoiceProfile`) has to decide membership from
    /// what the emulator actually holds, and this service's state names a linked account
    /// of that organization in exactly two places: an invoice unit's `InvoiceReceiver`
    /// ("the Amazon Web Services account ID chosen to be the receiver of an invoice
    /// unit") and an invoice-unit rule's `LinkedAccounts` ("the list of
    /// `LINKED_ACCOUNT` IDs where charges are included within the invoice unit") —
    /// `smithy_query(query="shape", shape="InvoiceUnitRule")` and
    /// `smithy_query(query="operation", operation="CreateInvoiceUnit")`.
    ///
    /// Membership is derived from those configurations and from NOTHING else. The
    /// requester's own account ([`::engine::emulator_account_id`], the identity this
    /// emulator presents to a caller) is the MANAGEMENT account of this organization, so
    /// it is a LINKED account only when a stored configuration names it as one — which is
    /// a different question from whether the requester may READ it:
    /// [`Self::accounts_the_requester_may_read`] is what an authorization check consults.
    ///
    /// `BillSourceAccounts` is deliberately NOT a source: the model documents those as
    /// accounts that "can be payer accounts from other organizations that have
    /// authorized billing transfer to this account", so naming one does not place it in
    /// this organization.
    ///
    /// Every RETAINED configuration of a unit counts, not only its current one: an
    /// account an update dropped from a rule left that invoice unit, not the
    /// organization. A deleted invoice unit takes its accounts with it — state is the
    /// only record either way.
    pub fn linked_accounts(&self) -> BTreeSet<String> {
        let mut accounts = BTreeSet::new();
        for unit in self.live_invoice_units() {
            accounts.insert(unit.invoice_receiver.clone());
            for version in std::iter::once(&unit.current).chain(unit.superseded.iter()) {
                if let Some(linked) = version.rule.linked_accounts.as_ref() {
                    accounts.extend(linked.iter().cloned());
                }
            }
        }
        accounts
    }

    /// Every account the REQUESTER may be answered for: the accounts of its organization
    /// ([`Self::linked_accounts`]) and its own account.
    ///
    /// An account a requester holds no access to is what an operation over "linked
    /// accounts under the requester management account organization"
    /// (`BatchGetInvoiceProfile`) refuses, and a principal always has access to the
    /// resources of the account it authenticates as: a requester asking about ITS OWN
    /// account is asking about its own resources, so that account is readable by it
    /// whatever this service's state holds beside it — recorded behaviour, which answers a
    /// request naming only the caller's account with that account's profile. Every OTHER
    /// account id is readable exactly when a stored configuration places it in the
    /// organization, and refused when none does.
    ///
    /// It is the requester's identity that admits its own account here, so this is the set
    /// an AUTHORIZATION check reads, while [`Self::linked_accounts`] stays the
    /// organization's membership — the two differ by exactly that one account.
    pub fn accounts_the_requester_may_read(&self) -> BTreeSet<String> {
        let mut accounts = self.linked_accounts();
        accounts.insert(::engine::emulator_account_id().to_string());
        accounts
    }

    /// The rule a WRITE of an invoice unit holds its `Rule` to: an account is linked into
    /// at most ONE invoice unit, and is named at most once in the one it is linked into.
    ///
    /// The model documents the resource as "a set of mutually exclusive accounts that
    /// correspond to your business entity" (`smithy_query(query="shape",
    /// shape="InvoiceUnit")`), and `LinkedAccounts` as "the list of `LINKED_ACCOUNT` IDs
    /// where charges are included within the invoice unit"
    /// (`smithy_query(query="shape", shape="InvoiceUnitRule")`): one account's charges
    /// belong to one business entity, so a second unit linking an account the first one
    /// already links would place those charges in two entities at once, and an account
    /// repeated inside one rule is not a SET either. The model names the refusal reason
    /// for exactly this class, `ValidationExceptionReason$mutualExclusionError`
    /// ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]), which is what both
    /// faces are refused with.
    ///
    /// The condition is a combination of the request and the STORED units, which no
    /// modelled trait can express — `RuleAccountIdList` carries no `@uniqueItems` and
    /// knows nothing of state — so it is refused here rather than by the boundary
    /// validator, the way [`resolve_marketplace_punch_out`] refuses the combination a
    /// single request cannot express.
    ///
    /// It lives beside the registry rather than in one operation's module because it is
    /// one rule about the RESOURCE which every write of an invoice unit applies whole —
    /// the create and the update — so neither path can drift from the other. `unit_arn` is
    /// the unit the write is CHANGING, if any: the accounts a unit already links are its
    /// own, so keeping or re-stating them is not an overlap, while a create (`None`) is
    /// held against every live unit.
    ///
    /// Only each live unit's CURRENT rule holds accounts: a superseded configuration is
    /// history kept for point-in-time reads, so an account an update dropped from a rule
    /// is linkable again, and a retired unit takes its accounts with it. `BillSourceAccounts`
    /// is deliberately not part of the rule: the model documents those as accounts that
    /// "can be payer accounts from other organizations that have authorized billing
    /// transfer to this account", which no part of the model says is exclusive.
    ///
    /// The message states the rule over the offending account, which is enclosed in angle
    /// brackets the way this plane's other request-derived refusal spells the value it
    /// names, and the unit the account is already linked into travels in the shape's own
    /// `resourceName` member so a client can read it mechanically.
    pub fn check_linked_account_exclusivity(
        &self,
        rule: &InvoiceUnitRule,
        unit_arn: Option<&str>,
    ) -> Result<(), ValidationException> {
        let Some(requested) = rule.linked_accounts.as_deref() else {
            return Ok(());
        };
        // The request's own accounts, by the position each one holds in its list, so a
        // repeat is caught as the list is walked and an overlap is reported for the FIRST
        // offending account of the REQUEST rather than of the registry. Keyed by the
        // request's list, which the model bounds (`RuleAccountIdList`, `@length(0..1000)`),
        // rather than by the whole registry.
        let mut positions: BTreeMap<&str, usize> = BTreeMap::new();
        for (position, account) in requested.iter().enumerate() {
            if positions.insert(account.as_str(), position).is_some() {
                return Err(mutual_exclusion(
                    format!("Account <{account}> is listed more than once in the invoice unit rule"),
                    None,
                ));
            }
        }

        // The earliest account of the request that another live unit's current rule links,
        // and the unit that links it: the first such unit in `InvoiceUnitArn` order, so
        // one request is always refused the same way.
        let mut overlap: Option<(usize, &str, &str)> = None;
        for unit in self.live_invoice_units() {
            if Some(unit.arn.as_str()) == unit_arn {
                continue;
            }
            for account in unit.current.rule.linked_accounts.iter().flatten() {
                let Some(&position) = positions.get(account.as_str()) else {
                    continue;
                };
                if overlap.is_none_or(|(earliest, _, _)| position < earliest) {
                    overlap = Some((position, account.as_str(), unit.arn.as_str()));
                }
            }
        }
        if let Some((_, account, other_unit)) = overlap {
            return Err(mutual_exclusion(
                format!("Account <{account}> is already linked to another invoice unit"),
                Some(other_unit),
            ));
        }
        Ok(())
    }
}

/// The 400 a write of an invoice unit is refused with when its `Rule` breaks the
/// mutual-exclusion rule [`ComAmazonawsInvoicingInvoicingState::check_linked_account_exclusivity`]
/// applies: the model's own `mutualExclusionError` reason, so a client branches on the
/// reason rather than on the prose.
///
/// `message` is held to the bound the model gives the member it travels in
/// ([`crate::service::validation::bounded_message`]), as every message this service words
/// from a request is, and `other_unit` — the invoice unit the account is already linked
/// into — is reported in `resourceName` under the same rule every ARN this service reports
/// is held to ([`crate::service::validation::reportable_resource_arn`]). The text is keyed
/// as the invoice-unit plane keys it ([`MessageKey::ModelMemberName`]).
fn mutual_exclusion(message: String, other_unit: Option<&str>) -> ValidationException {
    ValidationException {
        message: Some(crate::service::validation::bounded_message(message)),
        resource_name: other_unit.and_then(crate::service::validation::reportable_resource_arn),
        reason: Some("mutualExclusionError".to_string()),
        message_key: MessageKey::ModelMemberName,
        ..Default::default()
    }
}

/// The REQUEST half of an `@idempotent` replay key ([`IdempotencyKey`]): a stable digest
/// of the request members the rest of the key does not already name.
///
/// A `ClientToken` identifies a REPEAT OF ONE REQUEST, so a recorded result answers only
/// a request carrying the same members the recorded call carried. A token repeated with
/// any other member values digests differently, finds no record and is served on its own
/// merits — including whatever refusal that request earns, because the checks a handler
/// owes a request are not something an earlier, different request can stand in for.
///
/// Every `@idempotent` operation of this service digests its own members through here:
/// the ones its `target` does not already state, written out member by member at the call
/// site so the digest is a CONTRACT — a member the model adds to one of those requests is
/// a line to add there, or two requests differing only in it would share one record.
///
/// The parts are length-prefixed, so `["a", "bc"]` and `["ab", "c"]` do not collide, and
/// the mixing is SHA-256 from the maintained `sha2` crate rather than anything written
/// here. Digesting rather than keeping the parts bounds the key, whose members are
/// modelled up to 1024 characters each in lists the model does not bound, and keeps a
/// member the model marks `@sensitive` from being held a second time.
pub fn idempotent_request_digest<S: AsRef<str>>(parts: &[S]) -> String {
    use sha2::Digest;

    let mut digest = sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// The digest of a request that is ONLY its target: an `@idempotent` operation whose
/// input carries nothing beside the resource it names and the `ClientToken`, so the
/// `target` in the key already states the whole request and there is no further member to
/// mix in.
pub fn empty_request_digest() -> String {
    idempotent_request_digest::<&str>(&[])
}

/// The digest part an ABSENT optional member contributes, so a request that omits a
/// member is not the same request as one that sends it empty.
///
/// A PRESENT member always contributes its marker and then its value, so the part a value
/// occupies is never read as this marker however the value is spelled: the parts of one
/// request decode left to right, member by member, from the markers the pushes below
/// write.
const DIGEST_UNSET: &str = "\u{0}unset";

/// Push one optional string member's digest parts.
pub fn push_optional_digest(parts: &mut Vec<String>, value: Option<&str>) {
    match value {
        Some(value) => {
            parts.push("set".to_string());
            parts.push(value.to_string());
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push one optional boolean member's digest parts: the three values a modelled
/// `Boolean` member a request may omit can carry.
pub fn push_flag_digest(parts: &mut Vec<String>, value: Option<bool>) {
    parts.push(match value {
        Some(true) => "true".to_string(),
        Some(false) => "false".to_string(),
        None => DIGEST_UNSET.to_string(),
    });
}

/// Push one optional string-list member's digest parts, ORDER AND COUNT INCLUDED: the
/// list a request states is the list it states, so neither a reordering nor a repeat is
/// the same request.
pub fn push_strings_digest(parts: &mut Vec<String>, values: Option<&[String]>) {
    match values {
        Some(values) => {
            parts.push(values.len().to_string());
            parts.extend(values.iter().cloned());
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push the digest parts of an `InvoiceUnitRule`, the shape both writes of an invoice
/// unit carry.
pub fn push_invoice_unit_rule_digest(parts: &mut Vec<String>, rule: Option<&InvoiceUnitRule>) {
    match rule {
        Some(rule) => {
            push_strings_digest(parts, rule.linked_accounts.as_deref());
            push_strings_digest(parts, rule.bill_source_accounts.as_deref());
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push the digest parts of a `ResourceTagList`, the shape every create of this service
/// carries, as the tags the create would STORE: one entry per key, the last value stated
/// for a key, in key order — the normalisation [`resource_tags_for_new_resource`]
/// performs. Two repeats that differ only in the order of their tag list, in a key they
/// state twice, or in stating an empty list rather than none at all, leave the resource
/// holding the same tags and so are one request here too.
pub fn push_resource_tags_digest(parts: &mut Vec<String>, tags: Option<&[ResourceTag]>) {
    let stored: BTreeMap<&str, &str> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    parts.push(stored.len().to_string());
    for (key, value) in stored {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
}

/// Push the digest parts of a `ProcurementPortalPreferenceSelector`.
pub fn push_selector_digest(
    parts: &mut Vec<String>,
    selector: Option<&ProcurementPortalPreferenceSelector>,
) {
    match selector {
        Some(selector) => {
            push_strings_digest(parts, selector.invoice_unit_arns.as_deref());
            push_strings_digest(parts, selector.seller_of_records.as_deref());
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push the digest parts of a `TestEnvPreferenceInput`.
pub fn push_test_env_preference_digest(
    parts: &mut Vec<String>,
    test_env: Option<&TestEnvPreferenceInput>,
) {
    match test_env {
        Some(test_env) => {
            parts.push(test_env.buyer_domain.clone());
            parts.push(test_env.buyer_identifier.clone());
            parts.push(test_env.supplier_domain.clone());
            parts.push(test_env.supplier_identifier.clone());
            push_optional_digest(
                parts,
                test_env.procurement_portal_shared_secret.as_deref(),
            );
            push_optional_digest(
                parts,
                test_env.procurement_portal_instance_endpoint.as_deref(),
            );
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push the digest parts of an `EinvoiceDeliveryPreference`.
pub fn push_einvoice_delivery_preference_digest(
    parts: &mut Vec<String>,
    preference: Option<&EinvoiceDeliveryPreference>,
) {
    match preference {
        Some(preference) => {
            push_strings_digest(parts, Some(&preference.einvoice_delivery_document_types));
            push_strings_digest(
                parts,
                preference.einvoice_delivery_attachment_types.as_deref(),
            );
            parts.push(preference.protocol.clone());
            parts.push(preference.purchase_order_data_sources.len().to_string());
            for source in &preference.purchase_order_data_sources {
                push_optional_digest(parts, source.einvoice_delivery_document_type.as_deref());
                push_optional_digest(parts, source.purchase_order_data_source_type.as_deref());
            }
            parts.push(preference.connection_testing_method.clone());
            parts.push(
                instant_key(&preference.einvoice_delivery_activation_date).to_string(),
            );
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push the digest parts of a `MarketplacePunchOutPreference`.
pub fn push_marketplace_punch_out_preference_digest(
    parts: &mut Vec<String>,
    preference: Option<&MarketplacePunchOutPreference>,
) {
    match preference {
        Some(preference) => {
            push_optional_digest(parts, preference.approval_request_redirect_url.as_deref())
        }
        None => parts.push(DIGEST_UNSET.to_string()),
    }
}

/// Push the digest parts of a `Contacts` list.
pub fn push_contacts_digest(parts: &mut Vec<String>, contacts: &[Contact]) {
    parts.push(contacts.len().to_string());
    for contact in contacts {
        push_optional_digest(parts, contact.name.as_deref());
        push_optional_digest(parts, contact.email.as_deref());
    }
}

/// One procurement portal preference as it is stored: the modelled structure a reader
/// reports, plus the portal-validation state the validation flow keeps about it.
#[derive(Debug, Clone)]
pub struct StoredProcurementPortalPreference {
    /// The preference itself, as the modelled `ProcurementPortalPreference` a reader
    /// reports it with — one copy, so two readers of one preference cannot disagree.
    /// `Version`, `CreateDate` and `LastUpdateDate` are members of it, so a read answers
    /// with exactly what the last write left.
    pub preference: ProcurementPortalPreference,
    /// The validation `SendProcurementPortalValidation` last issued for this
    /// preference, if any. `None` means no code is outstanding, so there is
    /// no code a `VerifyProcurementPortalValidation` could match.
    pub validation: Option<IssuedPortalValidation>,
    /// How many submissions of a validation code for this preference have been REFUSED
    /// SINCE THE SERVICE LAST ISSUED A CODE for it: the failed attempts of
    /// `VerifyProcurementPortalValidation`, which its refusal names ("attempt N of the
    /// budget").
    ///
    /// The count belongs to the code the service last issued, because the budget it is
    /// reported against is the budget of ONE code: the refusal's own remedy is to ask for
    /// a new code, so issuing one starts the count again
    /// ([`Self::issue_validation`]) and the attempt a refusal names stays inside the
    /// budget it is stated against.
    ///
    /// It is state the service keeps ABOUT the preference and no modelled member reports
    /// it, so a reader sees the COUNT itself nowhere; what a reader does see is that the
    /// submission was recorded, because recording it is a write of the preference
    /// ([`Self::record_write`]).
    pub failed_verification_attempts: u32,
}

/// One issued procurement-portal validation: the code the service issued, when it
/// issued it, and when (if ever) a caller submitted that same code back.
#[derive(Debug, Clone)]
pub struct IssuedPortalValidation {
    /// The code `SendProcurementPortalValidation` issued. Kept in state and never put
    /// on the wire: the modelled response carries only the preference's ARN, because
    /// the real service delivers the code out of band through the portal.
    pub code: String,
    /// When the code was issued.
    pub issued_at: DateTime,
    /// When a `VerifyProcurementPortalValidation` submitted this code back, which is
    /// what completes the validation. `None` while it is still outstanding.
    pub verified_at: Option<DateTime>,
}

impl StoredProcurementPortalPreference {
    /// Record that this preference was WRITTEN at `at`: the next `Version` and the instant
    /// the write happened at.
    ///
    /// `Version` is "the version number of the procurement portal preference
    /// configuration" and `LastUpdateDate` is "the date and time when the procurement
    /// portal preference was last updated" (`smithy_query(query="shape",
    /// shape="ProcurementPortalPreference")`). That documented wording of `Version` names
    /// the CONFIGURATION, while the recorded lifecycle counts more than the configuration
    /// writes: it reports version 4 for a preference after a create, one configuration
    /// write, a refused send and a refused code submission. So what the pair reports is
    /// the LAST WRITE this preference took — one version per write the service applied,
    /// whatever that write touched, and nothing for a call that wrote nothing. RECORDED
    /// behaviour is the grounding for counting the two validation writes; the model's
    /// wording does not license them on its own.
    ///
    /// Every write of a stored preference goes through here — the configuration writes (the
    /// create's first version aside, which has no previous one to advance), the status
    /// update, and the validation writes that record a code or a submission against it —
    /// so no path can take half of the pair or advance it twice for one write, and a caller
    /// that compares the version it wrote against the version it reads back is comparing
    /// one counter. A call the service REFUSES before it writes anything never reaches
    /// here, which is what keeps a refusal invisible to a later reader.
    pub fn record_write(&mut self, at: DateTime) {
        self.preference.version = self.preference.version.saturating_add(1);
        self.preference.last_update_date = at;
    }

    /// Record `code` as the validation outstanding for this preference, issued at
    /// `issued_at` — the write the call that ISSUES a code performs.
    ///
    /// The code it records becomes the one a submission has to match, retiring whatever
    /// an earlier issue left, and the failed-attempt count
    /// ([`Self::failed_verification_attempts`]) starts again with it: the attempt count a
    /// refusal reports is the count against the code currently outstanding, so a code
    /// nothing has been submitted against yet has its whole budget. Every call that
    /// issues a code goes through here, so the fresh code and the fresh count cannot come
    /// apart.
    ///
    /// Recording the code is a WRITE of the preference, so it advances the pair
    /// [`Self::record_write`] keeps, exactly as the writes that state a configuration or a
    /// status do.
    pub fn issue_validation(&mut self, code: String, issued_at: DateTime) {
        self.validation = Some(IssuedPortalValidation {
            code,
            issued_at,
            verified_at: None,
        });
        self.failed_verification_attempts = 0;
        self.record_write(issued_at);
    }

    /// What identifies this preference apart from its ARN: the portal it talks to and
    /// the buyer/supplier pair it routes between.
    pub fn identity(&self) -> String {
        procurement_portal_preference_identity(
            &self.preference.procurement_portal_name,
            &self.preference.buyer_domain,
            &self.preference.buyer_identifier,
            &self.preference.supplier_domain,
            &self.preference.supplier_identifier,
        )
    }

    /// This preference as the modelled `ProcurementPortalPreferenceSummary`: the
    /// members of the stored `ProcurementPortalPreference` that summary shape declares,
    /// and nothing else — the `@sensitive` shared secret and contacts are not among
    /// them, so a listing does not carry them.
    pub fn to_summary(&self) -> ProcurementPortalPreferenceSummary {
        let preference = &self.preference;
        ProcurementPortalPreferenceSummary {
            aws_account_id: preference.aws_account_id.clone(),
            procurement_portal_preference_arn: preference.procurement_portal_preference_arn.clone(),
            procurement_portal_name: preference.procurement_portal_name.clone(),
            buyer_domain: preference.buyer_domain.clone(),
            buyer_identifier: preference.buyer_identifier.clone(),
            supplier_domain: preference.supplier_domain.clone(),
            supplier_identifier: preference.supplier_identifier.clone(),
            selector: preference.selector.clone(),
            einvoice_delivery_enabled: preference.einvoice_delivery_enabled,
            purchase_order_retrieval_enabled: preference.purchase_order_retrieval_enabled,
            marketplace_punch_out_enabled: preference.marketplace_punch_out_enabled,
            einvoice_delivery_preference_status: preference
                .einvoice_delivery_preference_status
                .clone(),
            einvoice_delivery_preference_status_reason: preference
                .einvoice_delivery_preference_status_reason
                .clone(),
            purchase_order_retrieval_preference_status: preference
                .purchase_order_retrieval_preference_status
                .clone(),
            purchase_order_retrieval_preference_status_reason: preference
                .purchase_order_retrieval_preference_status_reason
                .clone(),
            version: preference.version,
            create_date: preference.create_date,
            last_update_date: preference.last_update_date,
        }
    }
}

/// `ProcurementPortalPreferenceStatus$PENDING_VERIFICATION` — the status a preference
/// plane holds while the portal connection has not been verified yet. One of the values
/// the modelled enum admits
/// ([`crate::ops::types::PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES`]), and the one a
/// freshly created preference starts at: the service models
/// `SendProcurementPortalValidation` and `VerifyProcurementPortalValidation` as the
/// calls that carry a preference past it, so nothing but a caller's verification (or an
/// explicit `UpdateProcurementPortalPreferenceStatus`) moves it.
pub const STATUS_PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// The verification state each plane of a procurement portal preference reports after a
/// write of that preference's configuration, in ONE place.
///
/// A plane's status is "the current status of the e-invoice delivery preference" / of the
/// purchase order retrieval preference, and its reason is "the reason for the current"
/// status (`smithy_query(query="shape", shape="ProcurementPortalPreference")`), so the
/// pair describes the portal connection an ENABLED plane has: a plane the configuration
/// enables and nothing has verified yet reports [`STATUS_PENDING_VERIFICATION`], and a
/// plane the configuration leaves DISABLED reports no status and no reason at all,
/// because there is no connection for either to be about. A status an enabled plane
/// already holds is left exactly as it is: the validation calls
/// (`SendProcurementPortalValidation` / `VerifyProcurementPortalValidation`) and
/// `UpdateProcurementPortalPreferenceStatus` are what move a status, and a configuration
/// write is none of them.
///
/// It lives beside the preference registry, for the reason [`PORTAL_COUPA`] states rather
/// than in one operation's module: it is one rule about the RESOURCE which every write of
/// a preference's configuration — the create and the update — applies whole, so two
/// preferences holding the same configuration cannot report different state because they
/// reached it by different paths. The one write that states a status without stating a
/// configuration — `UpdateProcurementPortalPreferenceStatus` — holds to the same rule
/// through [`check_plane_enabled_for_status`], so no write can leave a preference
/// reporting the state this function says cannot exist. The Marketplace PunchOut flag and
/// its configuration are the same shape of dependent pair, held by
/// [`resolve_marketplace_punch_out`] because a write resolves that flag before it builds
/// the preference.
pub fn apply_plane_verification_statuses(preference: &mut ProcurementPortalPreference) {
    // Each plane as the triple the rule acts on: whether the configuration enables it,
    // and the status and reason it reports. The fields are disjoint, so both planes are
    // borrowed at once and the rule below is written once for the two of them.
    let planes: [(bool, &mut Option<String>, &mut Option<String>); 2] = [
        (
            preference.einvoice_delivery_enabled,
            &mut preference.einvoice_delivery_preference_status,
            &mut preference.einvoice_delivery_preference_status_reason,
        ),
        (
            preference.purchase_order_retrieval_enabled,
            &mut preference.purchase_order_retrieval_preference_status,
            &mut preference.purchase_order_retrieval_preference_status_reason,
        ),
    ];
    for (enabled, status, reason) in planes {
        if enabled {
            if status.is_none() {
                *status = Some(STATUS_PENDING_VERIFICATION.to_string());
            }
        } else {
            *status = None;
            *reason = None;
        }
    }
}

/// `ProcurementPortalPreferenceStatus$VALIDATED` — the status a plane reports once the
/// portal connection it describes HAS been verified. One of the values the modelled enum
/// admits ([`crate::ops::types::PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES`]), and the
/// one a plane waiting at [`STATUS_PENDING_VERIFICATION`] reaches when a caller completes
/// the validation of the preference.
pub const STATUS_VALIDATED: &str = "VALIDATED";

/// What a COMPLETED portal validation leaves on the preference it validated, in one place:
/// every plane still waiting for the connection to be verified reports
/// [`STATUS_VALIDATED`].
///
/// A plane's status is "the current status of the e-invoice delivery preference" / of the
/// purchase order retrieval preference (`smithy_query(query="shape",
/// shape="ProcurementPortalPreference")`), so completing the verification of the portal
/// connection is what carries a plane off the status it holds WHILE that connection is
/// unverified — and only that plane: a plane the configuration leaves disabled has no
/// status for the connection to be about ([`apply_plane_verification_statuses`]), and a
/// plane an explicit status update already carried further along keeps the status it
/// holds, because a completed verification does not undo a later state. The status reason
/// is "the reason for the current" status, so a plane that moves reports no reason until
/// one is stated for the status it now holds.
///
/// A plane that moved reports the status it moved to and no reason. Whether any plane
/// moved decides nothing else: a validation that completes with no plane waiting still
/// COMPLETED, and the write that records it is the caller's
/// ([`StoredProcurementPortalPreference::record_write`]) — one version of the resource
/// either way — so this answers nothing back.
///
/// It lives beside the preference registry rather than in one operation's module for the
/// reason [`PORTAL_COUPA`] states: it is one rule about the RESOURCE's verification state,
/// the third face of the rule [`apply_plane_verification_statuses`] and
/// [`check_plane_enabled_for_status`] hold between them.
pub fn apply_completed_verification(preference: &mut ProcurementPortalPreference) {
    // Each plane as the pair the rule acts on: the status it reports and the reason stated
    // for that status. The fields are disjoint, so both planes are borrowed at once and the
    // rule below is written once for the two of them.
    let planes: [(&mut Option<String>, &mut Option<String>); 2] = [
        (
            &mut preference.einvoice_delivery_preference_status,
            &mut preference.einvoice_delivery_preference_status_reason,
        ),
        (
            &mut preference.purchase_order_retrieval_preference_status,
            &mut preference.purchase_order_retrieval_preference_status_reason,
        ),
    ];
    for (status, reason) in planes {
        if status.as_deref() == Some(STATUS_PENDING_VERIFICATION) {
            *status = Some(STATUS_VALIDATED.to_string());
            *reason = None;
        }
    }
}

/// `ProcurementPortalPreferenceStatus$TEST_INITIALIZED` — the status a plane reports once
/// the connection test its configuration names has been started. One of the values the
/// modelled enum admits
/// ([`crate::ops::types::PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES`]).
pub const STATUS_TEST_INITIALIZED: &str = "TEST_INITIALIZED";

/// `ProcurementPortalPreferenceStatus$TEST_INITIALIZATION_FAILED` — the status a plane
/// reports when the service could not START that connection test.
pub const STATUS_TEST_INITIALIZATION_FAILED: &str = "TEST_INITIALIZATION_FAILED";

/// `ProcurementPortalPreferenceStatus$TEST_FAILED` — the status a plane reports when the
/// connection test ran and did not pass.
pub const STATUS_TEST_FAILED: &str = "TEST_FAILED";

/// `ProcurementPortalPreferenceStatus$ACTIVE` — the status a plane reports once the
/// feature it describes is activated.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `ProcurementPortalPreferenceStatus$SUSPENDED` — the status a plane reports once the
/// service has suspended the feature it describes.
pub const STATUS_SUSPENDED: &str = "SUSPENDED";

/// The statuses a plane takes because the SERVICE put it there, so a caller cannot ask for
/// one of them.
///
/// `UpdateProcurementPortalPreferenceStatus` is documented as updating the status of a
/// preference "including the activation state of e-invoice delivery and purchase order
/// retrieval features" (`smithy_query(query="doc",
/// shape="UpdateProcurementPortalPreferenceStatus")`), so what a CALLER states through it
/// is the activation of a feature — asking for the connection test
/// ([`STATUS_TEST_INITIALIZED`]) and activating what passed it ([`STATUS_ACTIVE`]). Every
/// other value of the enum reports an outcome the service itself reached: the connection
/// being unverified or verified ([`STATUS_PENDING_VERIFICATION`], [`STATUS_VALIDATED`],
/// which this service's own validation flow writes), the test not starting or not passing
/// ([`STATUS_TEST_INITIALIZATION_FAILED`], [`STATUS_TEST_FAILED`]) and the feature being
/// suspended ([`STATUS_SUSPENDED`]).
///
/// Recorded behaviour: a caller-requested move into a status of this set is refused with
/// the sentence [`transition_is_awss_to_make`] words, which names the service as the only
/// one that makes it.
const SERVICE_ASSIGNED_STATUSES: &[&str] = &[
    STATUS_PENDING_VERIFICATION,
    STATUS_VALIDATED,
    STATUS_TEST_INITIALIZATION_FAILED,
    STATUS_TEST_FAILED,
    STATUS_SUSPENDED,
];

/// The moves a CALLER may ask one plane of a preference to make, as `(from, to)` pairs: the
/// lifecycle `ProcurementPortalPreferenceStatus` describes, walked by the only two statuses
/// a caller states ([`SERVICE_ASSIGNED_STATUSES`] names the rest).
///
/// A connection test is asked for once the portal connection has been verified, and again
/// after a test that did not start or did not pass; what passed the test is then activated.
/// A move this table does not carry is refused with the sentence
/// [`transition_is_not_allowed`] words.
const CALLER_REQUESTED_TRANSITIONS: &[(&str, &str)] = &[
    (STATUS_VALIDATED, STATUS_TEST_INITIALIZED),
    (STATUS_TEST_INITIALIZATION_FAILED, STATUS_TEST_INITIALIZED),
    (STATUS_TEST_FAILED, STATUS_TEST_INITIALIZED),
    (STATUS_TEST_INITIALIZED, STATUS_ACTIVE),
];

/// The rule sentence a move only the service makes violates: it names the member being
/// written, the status the plane holds and the status the request asked for.
fn transition_is_awss_to_make(status_member: &str, current: &str, requested: &str) -> String {
    format!("Transition of {status_member} from {current} to {requested} can only be done by AWS")
}

/// The rule sentence a move the lifecycle does not provide violates, worded from the same
/// three parts.
fn transition_is_not_allowed(status_member: &str, current: &str, requested: &str) -> String {
    format!("Transition of {status_member} from {current} to {requested} is not allowed")
}

/// Which rules a caller-requested move of ONE plane violates, in the order a refusal
/// states them — empty for a move the caller may ask for.
///
/// A plane's status is "the current status of the e-invoice delivery preference" / of the
/// purchase order retrieval preference (`smithy_query(query="shape",
/// shape="ProcurementPortalPreference")`), so the statuses are the states of one portal
/// connection and a request that states one asks for a MOVE between them, out of the status
/// the preference holds now. Two rules decide whether that move is a caller's to ask for:
/// the status asked for has to be one a caller states at all
/// ([`SERVICE_ASSIGNED_STATUSES`]), and the pair has to be a move the lifecycle provides
/// ([`CALLER_REQUESTED_TRANSITIONS`]). Both are reported when both are violated, which is
/// how the recorded refusal of a move into a service-assigned status reads.
///
/// A request that states the status the plane ALREADY holds asks for no move, so there is
/// no transition for either rule to be about and it violates neither — it states a reason
/// for the status the plane holds.
///
/// The rule lives beside the preference registry, for the reason [`PORTAL_COUPA`] states:
/// it is one rule about the RESOURCE's verification state, the fourth face of the rule
/// [`apply_plane_verification_statuses`], [`apply_completed_verification`] and
/// [`check_plane_enabled_for_status`] hold between them — the statuses this service's own
/// validation flow writes are exactly the ones a caller cannot ask for.
pub fn caller_requested_transition_violations(
    status_member: &str,
    current: Option<&str>,
    requested: &str,
) -> Vec<String> {
    // A plane the configuration enables always holds a status
    // ([`apply_plane_verification_statuses`]); a plane that holds none has had nothing
    // verified, which is the status a plane starts at.
    let current = current.unwrap_or(STATUS_PENDING_VERIFICATION);
    if current == requested {
        return Vec::new();
    }
    let mut violations = Vec::new();
    if SERVICE_ASSIGNED_STATUSES.contains(&requested) {
        violations.push(transition_is_awss_to_make(status_member, current, requested));
    }
    if !CALLER_REQUESTED_TRANSITIONS.contains(&(current, requested)) {
        violations.push(transition_is_not_allowed(status_member, current, requested));
    }
    violations
}

/// The plane a status update may move: the other face of the rule
/// [`apply_plane_verification_statuses`] holds, for the write that states a status without
/// stating a configuration.
///
/// A plane's status is the state of the portal connection the configuration gives that
/// plane, so only a plane the configuration ENABLES has a status to move: a request
/// stating the status of a plane the stored configuration leaves DISABLED names no status
/// that exists, and is refused with the modelled `ValidationException`. The condition is a
/// combination of an input member and the STORED resource, which the model cannot express
/// — every status member of that input is optional on its own — so it is refused here
/// rather than by the boundary validator, the way
/// [`resolve_marketplace_punch_out`] refuses the combination a single request
/// cannot express.
///
/// Both faces of the rule live beside the preference registry, for the reason
/// [`PORTAL_COUPA`] states: they are one rule about the RESOURCE, so no write can leave a
/// preference reporting a status for a plane that has no connection for one to be about,
/// and no status a write persisted can be erased by a later configuration write that
/// restates the configuration it was written under.
///
/// The message names the two members whose combination was rejected, so it is built from
/// what the request is acting on rather than from a constant, and it is keyed the way this
/// plane's bodies key their text ([`MessageKey::Capitalised`]).
pub fn check_plane_enabled_for_status(
    enabled: bool,
    status_member: &str,
    enabled_member: &str,
) -> Result<(), ValidationException> {
    if enabled {
        return Ok(());
    }
    Err(ValidationException {
        message: Some(format!(
            "{status_member} can only be updated when {enabled_member} is true"
        )),
        // `invalidInput` is one of the model's `ValidationExceptionReason` values
        // ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]).
        reason: Some("invalidInput".to_string()),
        message_key: MessageKey::Capitalised,
        ..Default::default()
    })
}

/// `ProcurementPortalName$COUPA` — the portal the CREATE's own member states its
/// Marketplace PunchOut configuration requirement for
/// (`CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`: "Required
/// for Coupa when MarketplacePunchOutEnabled is true"), which is the narrower of the two
/// scopes [`PunchOutConfigRequirement`] carries. One of the values the modelled enum
/// admits ([`crate::ops::types::PROCUREMENT_PORTAL_NAME_VALUES`]).
///
/// It lives beside the preference registry rather than inside one operation's module
/// because the combination it gates is a rule about the RESOURCE: every write that sets
/// a preference's Marketplace PunchOut configuration — the create and the update —
/// applies the one rule [`resolve_marketplace_punch_out`] holds, each with the scope its
/// own input member states.
pub const PORTAL_COUPA: &str = "COUPA";

/// The Marketplace PunchOut pair a write of a procurement portal preference stores: the
/// `MarketplacePunchOutEnabled` flag and the `MarketplacePunchOutPreference` that belongs
/// with it, resolved together by [`resolve_marketplace_punch_out`] so no write can take
/// half of the pair.
#[derive(Debug, Clone)]
pub struct MarketplacePunchOut {
    /// What the write stores in `MarketplacePunchOutEnabled`.
    pub enabled: bool,
    /// What the write stores in `MarketplacePunchOutPreference`: the configuration an
    /// ENABLED feature has, and nothing at all for a disabled one.
    pub preference: Option<MarketplacePunchOutPreference>,
}

/// Which portals a write's own `MarketplacePunchOutPreference` member states the
/// configuration requirement for: the one part of the Marketplace PunchOut rule the two
/// writes of a preference state differently, so the rule itself stays in one place
/// ([`resolve_marketplace_punch_out`]) and each write hands it the scope its own input
/// member documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PunchOutConfigRequirement {
    /// Every portal, with no qualification —
    /// `PutProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`:
    /// "Configuration for Marketplace PunchOut. Required when MarketplacePunchOutEnabled
    /// is true."
    EveryPortal,
    /// Only [`PORTAL_COUPA`] —
    /// `CreateProcurementPortalPreferenceRequest$MarketplacePunchOutPreference`:
    /// "Required for Coupa when MarketplacePunchOutEnabled is true."
    CoupaOnly,
}

impl PunchOutConfigRequirement {
    /// Whether a write to `procurement_portal_name` has to state the configuration to
    /// enable Marketplace PunchOut.
    fn applies_to(self, procurement_portal_name: &str) -> bool {
        match self {
            Self::EveryPortal => true,
            Self::CoupaOnly => procurement_portal_name == PORTAL_COUPA,
        }
    }

    /// The refusal's text, worded as the member that states the requirement words it: the
    /// scope that names no portal states none, and the scope stated FOR one portal names
    /// the portal the write is acting on.
    fn missing_config_message(self, procurement_portal_name: &str) -> String {
        match self {
            Self::EveryPortal => "MarketplacePunchOutPreference is required when \
                                  MarketplacePunchOutEnabled is true"
                .to_string(),
            Self::CoupaOnly => format!(
                "MarketplacePunchOutPreference is required when MarketplacePunchOutEnabled is \
                 true and ProcurementPortalName is {procurement_portal_name}"
            ),
        }
    }
}

/// The Marketplace PunchOut rule a write of a procurement portal preference applies, in
/// one place: the flag and the configuration the write stores, or the modelled 400 it is
/// refused with when it enables Marketplace PunchOut without stating the configuration
/// its own input member documents as required for that.
///
/// `MarketplacePunchOutEnabled` "defaults to false if not provided" — so an omitted
/// member is `false` here rather than anything that was stored — and
/// `MarketplacePunchOutPreference` is REQUIRED while that flag is true, which is a
/// combination of members the model cannot express: the boundary validator passes it and
/// it is refused here as the modelled `ValidationException`. How far that requirement
/// reaches is `requirement`, the scope the write's own member states
/// ([`PunchOutConfigRequirement`]) — the one part of the rule the two writes differ on.
/// The message is worded from what the write is acting on rather than from a constant, and
/// it is keyed the way this plane's recorded bodies key it ([`MessageKey::Capitalised`]) —
/// every write that reaches this rule is a write of a procurement portal preference.
///
/// The CONFIGURATION is part of the same rule, the way a disabled plane's status and
/// reason are part of [`apply_plane_verification_statuses`]:
/// `MarketplacePunchOutPreference` is "the Marketplace PunchOut configuration for this
/// procurement portal preference. This is present when `MarketplacePunchOutEnabled` is
/// `true`" (`smithy_query(query="shape", shape="ProcurementPortalPreference")`), so it
/// describes the feature an ENABLED flag has and a write that leaves the feature DISABLED
/// stores none of it — whether the request stated one or an earlier write left one behind,
/// because there is no enabled feature for it to configure. A write that leaves the
/// feature enabled stores the configuration it STATES: it is the write that enables the
/// feature which has to carry it, so within the scope the requirement reaches there is no
/// earlier configuration for an enabling write to fall back on.
///
/// Both the DEFAULT and the combination that is refused live here, beside the preference
/// registry, rather than in one operation's module, for the reason [`PORTAL_COUPA`]
/// states: they are one rule about the RESOURCE, which every write that sets a
/// preference's Marketplace PunchOut configuration — the create and the update — applies
/// whole, so neither path can drift from the other. The portal is the only part a caller
/// can reach differently: a create states it on the input, an update takes it off the
/// stored preference, whose portal no update can change.
pub fn resolve_marketplace_punch_out(
    requirement: PunchOutConfigRequirement,
    procurement_portal_name: &str,
    marketplace_punch_out_enabled: Option<bool>,
    requested_preference: Option<MarketplacePunchOutPreference>,
) -> Result<MarketplacePunchOut, ValidationException> {
    let enabled = marketplace_punch_out_enabled.unwrap_or(false);
    if enabled && requirement.applies_to(procurement_portal_name) && requested_preference.is_none()
    {
        return Err(ValidationException {
            message: Some(requirement.missing_config_message(procurement_portal_name)),
            // `invalidInput` is one of the model's `ValidationExceptionReason` values
            // ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]).
            reason: Some("invalidInput".to_string()),
            message_key: MessageKey::Capitalised,
            ..Default::default()
        });
    }
    let preference = if enabled { requested_preference } else { None };
    Ok(MarketplacePunchOut { enabled, preference })
}

/// The host part of an endpoint a caller configured, lowercased so two spellings of one
/// host are one host — the model gives these members only `^\S+$`, so the parse is what
/// decides whether a value names a host at all.
///
/// The parse is [`http::Uri`]'s, the same crate the request this write arrived on is
/// carried in, rather than a hand-rolled scan for `//` and `/`.
fn endpoint_host(endpoint: &str) -> Option<String> {
    endpoint
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_ascii_lowercase))
}

/// The scheme an endpoint member the model documents as a secure URL has to name:
/// `CreateProcurementPortalPreferenceRequest$ProcurementPortalInstanceEndpoint` is "the
/// endpoint URL where e-invoices will be delivered to the procurement portal. Must be a
/// valid HTTPS URL" (`smithy_query(query="shape",
/// shape="CreateProcurementPortalPreferenceRequest")`), and the update's own member states
/// the same requirement for the value it writes.
const SECURE_ENDPOINT_SCHEME: &str = "https";

/// Whether `endpoint` is a valid URL on [`SECURE_ENDPOINT_SCHEME`]: it has to PARSE as a
/// URL that names that scheme and a host, so a bare host (`portal.example.com`), a value
/// that is no URL at all, and a URL on any other scheme are each not one.
///
/// A URL's scheme is case-insensitive, so it is compared that way. The parse is
/// [`http::Uri`]'s, the crate the request this write arrived on is carried in, rather than
/// a hand-rolled scan — the same parse [`endpoint_host`] reads a host with.
fn is_secure_endpoint_url(endpoint: &str) -> bool {
    let Ok(uri) = endpoint.parse::<http::Uri>() else {
        return false;
    };
    uri.scheme_str()
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SECURE_ENDPOINT_SCHEME))
        && uri.host().is_some_and(|host| !host.is_empty())
}

/// The URL rule a write of a procurement portal preference applies to an endpoint member
/// its own input documents as having to be a valid HTTPS URL: a value that is not one is
/// refused with the modelled `ValidationException` (HTTP 400) before the write stores
/// anything, so no preference can report a delivery endpoint its member says cannot be
/// stored.
///
/// The members carrying that requirement are the two writes' own
/// `ProcurementPortalInstanceEndpoint` ("Must be a valid HTTPS URL"); each is optional, so
/// a write that states none states nothing for this rule to be about. The requirement is a
/// property of the VALUE that no modelled constraint can express — the member's target
/// gives it only `@length(0..1024)` and `^\S+$`, which any non-space string satisfies — so
/// the boundary validator passes it and it is refused here, the way
/// [`resolve_marketplace_punch_out`] refuses the combination the model cannot express.
///
/// The refusal names the member it is about, worded as that member words its own
/// requirement, and is keyed the way this plane's bodies key their text
/// ([`MessageKey::Capitalised`]). It lives beside the preference registry, for the reason
/// [`PORTAL_COUPA`] states: both writes that set such a member apply it.
pub fn check_secure_endpoint_url(
    member: &str,
    endpoint: Option<&str>,
) -> Result<(), ValidationException> {
    let Some(endpoint) = endpoint else {
        return Ok(());
    };
    if is_secure_endpoint_url(endpoint) {
        return Ok(());
    }
    Err(ValidationException {
        message: Some(format!("{member} must be a valid HTTPS URL")),
        // `invalidInput` is one of the model's `ValidationExceptionReason` values
        // ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]).
        reason: Some("invalidInput".to_string()),
        message_key: MessageKey::Capitalised,
        ..Default::default()
    })
}

/// The member the two writes of a preference state the delivery endpoint on, as a refusal
/// about it names it (`ProcurementPortalInstanceEndpoint`).
pub const PROCUREMENT_PORTAL_INSTANCE_ENDPOINT_MEMBER: &str = "ProcurementPortalInstanceEndpoint";

/// The host rule a write of a procurement portal preference applies to the pair of
/// endpoints it leaves on that preference:
/// `MarketplacePunchOutPreference$ApprovalRequestRedirectUrl` is documented as "when
/// provided together with the procurement portal instance endpoint, its host must match
/// the host of that endpoint" (`smithy_query(query="doc",
/// shape="MarketplacePunchOutPreference", member="ApprovalRequestRedirectUrl")`), so a
/// write that would leave the preference holding a redirect URL on one host and an
/// instance endpoint on another is refused with the modelled `ValidationException`.
///
/// The two values are taken as the write RESOLVED them — the redirect URL of the
/// configuration the write stores (so a write that leaves Marketplace PunchOut disabled,
/// and therefore stores no configuration at all, has no redirect URL for the rule to be
/// about) and the instance endpoint the preference holds after it. Only the HOST is
/// compared: the scheme, the port, the path and the query are each endpoint's own.
///
/// A value neither member's `^\S+$` pattern makes a URL is left alone here: the rule is
/// about two hosts, and a value that names no host is not something this rule has an
/// opinion on. The rule lives beside the preference registry, for the reason
/// [`PORTAL_COUPA`] states: both writes that set either member apply it.
pub fn check_marketplace_punch_out_redirect_host(
    marketplace_punch_out_preference: Option<&MarketplacePunchOutPreference>,
    procurement_portal_instance_endpoint: Option<&str>,
) -> Result<(), ValidationException> {
    let Some(redirect_url) = marketplace_punch_out_preference
        .and_then(|preference| preference.approval_request_redirect_url.as_deref())
    else {
        return Ok(());
    };
    let Some(endpoint) = procurement_portal_instance_endpoint else {
        return Ok(());
    };
    let (Some(redirect_host), Some(endpoint_host)) =
        (endpoint_host(redirect_url), endpoint_host(endpoint))
    else {
        return Ok(());
    };
    if redirect_host == endpoint_host {
        return Ok(());
    }
    Err(ValidationException {
        message: Some(
            "ApprovalRequestRedirectUrl must have the same host as \
             ProcurementPortalInstanceEndpoint"
                .to_string(),
        ),
        // `invalidInput` is one of the model's `ValidationExceptionReason` values
        // ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]).
        reason: Some("invalidInput".to_string()),
        message_key: MessageKey::Capitalised,
        ..Default::default()
    })
}

/// The identity of a procurement portal preference: its portal, buyer domain and
/// identifier, and supplier domain and identifier.
///
/// Each part is LENGTH-PREFIXED before it is joined, because the identifiers are
/// caller-supplied `^\S+$` strings that could otherwise contain whatever separator was
/// chosen and so forge another preference's identity.
pub fn procurement_portal_preference_identity(
    procurement_portal_name: &str,
    buyer_domain: &str,
    buyer_identifier: &str,
    supplier_domain: &str,
    supplier_identifier: &str,
) -> String {
    [
        procurement_portal_name,
        buyer_domain,
        buyer_identifier,
        supplier_domain,
        supplier_identifier,
    ]
    .iter()
    .map(|part| format!("{}:{part}", part.len()))
    .collect::<Vec<String>>()
    .join("|")
}

/// `ProcurementPortalEnv$PROD` — the environment a preference's own members configure,
/// one of [`crate::ops::types::PROCUREMENT_PORTAL_ENV_VALUES`].
pub const PORTAL_ENV_PROD: &str = "PROD";

/// `ProcurementPortalEnv$TEST` — the environment a preference's `TestEnvPreference`
/// ("the test environment of the procurement portal") configures, one of
/// [`crate::ops::types::PROCUREMENT_PORTAL_ENV_VALUES`].
pub const PORTAL_ENV_TEST: &str = "TEST";

/// What distinguishes one configured portal supplier from another, and the order a
/// paginated walk of them reports and resumes in: its identifier, the environment it is
/// configured in and the seller of record it applies to (empty when it applies to all of
/// them).
///
/// It is the walk's ORDER, not its token: the identifier and the seller of record are
/// caller-supplied `BasicStringWithoutSpace` members (`@length(max: 1024)` each), so a
/// token naming a position by this key would run past `NextToken`'s own modelled maximum
/// — the walk pages on the ordinal in this order instead (see [`encode_page_offset`]).
///
/// The parts are joined with a SPACE, which none of them can contain: a supplier
/// identifier and a seller-of-record id both come from a preference's
/// `BasicStringWithoutSpace` members (`@pattern ^\S+$`, which the boundary validator
/// refuses a space-bearing value against before any write can store one) and an
/// environment is one of [`crate::ops::types::PROCUREMENT_PORTAL_ENV_VALUES`]. A space
/// also sorts below every character those parts admit, so this key orders the walk by
/// identifier, then environment, then seller of record.
pub fn procurement_portal_supplier_key(supplier: &ProcurementPortalSupplier) -> String {
    format!(
        "{} {} {}",
        supplier.supplier_identifier,
        supplier.environment.as_deref().unwrap_or_default(),
        supplier.seller_of_record.as_deref().unwrap_or_default(),
    )
}

/// The `PortalIdentifier` of the portal `portal_name`: the portal name lowercased with
/// its underscores hyphenated, a `ProcurementPortalIdString` (`@length min 1`, no
/// pattern).
///
/// The model publishes no identifier for a portal, only the portal NAMES
/// (`smithy_query(query="shape", shape="ProcurementPortalName")`, as
/// [`crate::ops::types::PROCUREMENT_PORTAL_NAME_VALUES`]), so this emulator derives one
/// identifier per name: stable across calls and across runs, and unique per portal
/// because the names are.
pub fn procurement_portal_identifier(portal_name: &str) -> String {
    portal_name.to_ascii_lowercase().replace('_', "-")
}

/// The `ProcurementPortalName` the `PortalIdentifier` `identifier` names, or `None` when
/// it names no portal this service serves — which every reader that takes a
/// `PortalIdentifier` answers as the modelled `ResourceNotFoundException`
/// ([`no_such_procurement_portal`]) rather than as an empty listing.
///
/// The portals are exactly the values of `ProcurementPortalName`
/// ([`crate::ops::types::PROCUREMENT_PORTAL_NAME_VALUES`]): a preference can name no
/// portal outside that enum, so neither can a portal this service reports.
pub fn procurement_portal_name_for_identifier(identifier: &str) -> Option<&'static str> {
    PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .copied()
        .find(|name| procurement_portal_identifier(name) == identifier)
}

/// Every procurement portal this service serves, keyed by its `PortalIdentifier` — the
/// catalog `ListProcurementPortals` reports and the one
/// [`procurement_portal_name_for_identifier`] resolves an identifier against, so a
/// portal a caller can name a supplier walk or a preference for is exactly a portal this
/// catalog holds.
///
/// The portals are NOT caller-created resources: no operation of this service creates
/// one (`smithy_query(query="list_operations")` lists two creates, `CreateInvoiceUnit`
/// and `CreateProcurementPortalPreference`), and the operation is documented as
/// returning "the Amazon Web Services-supported procurement portals" — a catalog of what
/// the SERVICE supports rather than of what an account configured. The portals the
/// service supports are exactly the values of `ProcurementPortalName`
/// ([`crate::ops::types::PROCUREMENT_PORTAL_NAME_VALUES`]), because that enum is what a
/// preference may name a portal by, so nothing a caller stores can add one to this
/// catalog or take one out of it.
///
/// Each entry carries what the operation documents it includes — "the portal identifier,
/// name, and default feature configurations" — and nothing else:
///
/// * `PortalIdentifier` is the identifier this emulator derives per portal
///   ([`procurement_portal_identifier`]), stable across calls and runs.
/// * `PortalName` is the enum value itself.
/// * `DefaultFeatureConfigurations` carries the DEFAULT document and attachment types,
///   which the operation documents as "the supported document and attachment types":
///   the model publishes no per-portal subset, so the default for every portal is every
///   type its member's list admits — `EinvoiceDeliveryDocumentTypes` of
///   `EinvoiceDeliveryDocumentType` ([`crate::ops::types::EINVOICE_DELIVERY_DOCUMENT_TYPE_VALUES`],
///   five values, inside that list's `@length(min: 0, max: 10)`) and
///   `EinvoiceDeliveryAttachmentTypes` of `EinvoiceDeliveryAttachmentType`
///   ([`crate::ops::types::EINVOICE_DELIVERY_ATTACHMENT_TYPE_VALUES`]), which is also
///   exactly what a preference's `EinvoiceDeliveryPreference` may ask for.
/// * `PortalDisplayName` is left ABSENT: it is display prose AWS publishes for a portal,
///   the model carries none for any portal
///   (`smithy_query(query="shape", shape="ProcurementPortal")`) and the operation's own
///   documentation leaves it out of what an entry includes, so this reader reports no
///   display name rather than inventing one — the same reason a reported supplier carries
///   no `CountryCode` (see [`ComAmazonawsInvoicingInvoicingState::procurement_portal_suppliers`]).
///
/// A `BTreeMap` keyed by identifier, so `ListProcurementPortals` walks one stable,
/// deterministic order and a pagination token (a position in that order) stays
/// meaningful across calls. It is derived per call rather than cached, which is what
/// keeps that read from writing anything, and it is bounded by the enum: one entry per
/// value, whatever a caller asks for.
pub fn procurement_portals() -> BTreeMap<String, ProcurementPortal> {
    PROCUREMENT_PORTAL_NAME_VALUES
        .iter()
        .map(|portal_name| {
            let portal_identifier = procurement_portal_identifier(portal_name);
            let portal = ProcurementPortal {
                portal_identifier: portal_identifier.clone(),
                portal_name: (*portal_name).to_string(),
                portal_display_name: None,
                default_feature_configurations: Some(FeatureConfigurations {
                    invoice_configuration: Some(InvoiceConfiguration {
                        document_types: Some(
                            EINVOICE_DELIVERY_DOCUMENT_TYPE_VALUES
                                .iter()
                                .map(|document_type| (*document_type).to_string())
                                .collect(),
                        ),
                        attachment_types: Some(
                            EINVOICE_DELIVERY_ATTACHMENT_TYPE_VALUES
                                .iter()
                                .map(|attachment_type| (*attachment_type).to_string())
                                .collect(),
                        ),
                    }),
                }),
            };
            (portal_identifier, portal)
        })
        .collect()
}

/// The kind of resource a `PortalIdentifier` names, as the prose form of the portals
/// this service serves.
const PROCUREMENT_PORTAL_KIND: &str = "Procurement portal";

/// The 404 a reader answers for a `PortalIdentifier` that names no portal this service
/// serves, worded from the REQUEST the way the rest of the procurement-portal plane
/// words its refusals (see [`no_such_procurement_portal_preference`]) and keyed the way
/// that plane keys them ([`MessageKey::Capitalised`]).
///
/// `resourceName` is left absent: the model types it as an `InvoiceUnitArnString`
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so a portal
/// identifier — which is not an ARN — cannot be reported in it.
///
/// The sentence names the identifier the request supplied, and `ProcurementPortalIdString`
/// is the one input shape of this model with NO maximum length
/// (`smithy_query(query="operation", operation="ListProcurementPortalSuppliers")`), so the
/// sentence is held to the bound the model gives the member it travels in
/// ([`crate::service::validation::bounded_message`]) — a sentence naming a short
/// identifier is unchanged by it.
pub fn no_such_procurement_portal(portal_identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(crate::service::validation::bounded_message(format!(
            "{PROCUREMENT_PORTAL_KIND} with identifier {portal_identifier} does not exist"
        ))),
        resource_id: None,
        message_key: MessageKey::Capitalised,
    }
}

/// One invoice unit as it is stored: the members that can never change, plus the
/// configuration history that `AsOf` reads.
#[derive(Debug, Clone)]
pub struct StoredInvoiceUnit {
    /// The minted `InvoiceUnitArn`. Immutable.
    pub arn: String,
    /// `Name`, which the model documents as unchangeable once set.
    pub name: String,
    /// `InvoiceReceiver`, set at creation and not updatable.
    pub invoice_receiver: String,
    /// When the unit was created. An `AsOf` before this instant reads no configuration
    /// at all, because the unit did not exist then.
    pub created_at: DateTime,
    /// The configuration in effect now.
    pub current: InvoiceUnitVersion,
    /// Configurations an update replaced, oldest first, at most
    /// [`MAX_RETAINED_VERSIONS`] of them.
    pub superseded: Vec<InvoiceUnitVersion>,
    /// When the unit was DELETED, for a unit a delete has retired. The unit is no
    /// longer a resource from this instant on, while a point-in-time read before it
    /// still answers with the configuration that was in effect then — the model
    /// documents `AsOf` as showing "legacy invoice units that are currently deleted if
    /// the AsOf time is set to before it was deleted". `None` for a unit that still
    /// exists.
    pub deleted_at: Option<DateTime>,
}

impl StoredInvoiceUnit {
    /// Whether this unit still exists: a delete retires a unit rather than erasing it,
    /// and a retired one is history rather than a resource.
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }

    /// The configuration that was in effect at `as_of`, or the current one when the
    /// caller asked for no particular instant. `None` when the unit did not exist at
    /// `as_of`, which a reader reports as `ResourceNotFoundException`.
    ///
    /// "Did not exist" is both ends of the unit's life: an instant before it was
    /// created, and an instant at or after it was deleted. An instant in between
    /// answers with the configuration from then even for a unit that has since been
    /// deleted, and a caller that asked for no instant at all is asking about NOW, so a
    /// deleted unit is not part of that answer.
    ///
    /// A unit that existed at `as_of` but whose configuration from then has been evicted
    /// by [`MAX_RETAINED_VERSIONS`] reads the oldest configuration still retained.
    pub fn version_as_of(&self, as_of: Option<&DateTime>) -> Option<&InvoiceUnitVersion> {
        let Some(as_of) = as_of else {
            return self.is_live().then_some(&self.current);
        };
        let wanted = instant_key(as_of);
        if wanted < instant_key(&self.created_at) {
            return None;
        }
        if self
            .deleted_at
            .as_ref()
            .is_some_and(|deleted| instant_key(deleted) <= wanted)
        {
            return None;
        }
        std::iter::once(&self.current)
            .chain(self.superseded.iter().rev())
            .find(|version| instant_key(&version.last_modified) <= wanted)
            .or_else(|| self.superseded.first())
    }

    /// Make `next` the current configuration, keeping the one it replaces as history.
    pub fn apply(&mut self, next: InvoiceUnitVersion) {
        let previous = std::mem::replace(&mut self.current, next);
        self.superseded.push(previous);
        // Bounded history: updates are caller-driven, so keep only the most recent
        // configurations rather than growing this vector without limit.
        if self.superseded.len() > MAX_RETAINED_VERSIONS {
            let excess = self.superseded.len() - MAX_RETAINED_VERSIONS;
            self.superseded.drain(..excess);
        }
    }

    /// This unit as the modelled `InvoiceUnit` structure, reporting `version`.
    pub fn to_invoice_unit(&self, version: &InvoiceUnitVersion) -> InvoiceUnit {
        InvoiceUnit {
            invoice_unit_arn: Some(self.arn.clone()),
            invoice_receiver: Some(self.invoice_receiver.clone()),
            name: Some(self.name.clone()),
            description: version.description.clone(),
            tax_inheritance_disabled: Some(version.tax_inheritance_disabled),
            rule: Some(version.rule.clone()),
            last_modified: Some(version.last_modified),
        }
    }
}

/// The updatable half of an invoice unit, as of the `LastModified` instant it was
/// written at.
#[derive(Debug, Clone)]
pub struct InvoiceUnitVersion {
    pub description: Option<String>,
    pub tax_inheritance_disabled: bool,
    pub rule: InvoiceUnitRule,
    pub last_modified: DateTime,
}

/// A total order on timestamps, so a stored instant can be compared with a requested
/// one — a `LastModified` against an `AsOf`, an `IssuedDate` against a `DateInterval`'s
/// bounds — without assuming `DateTime` is itself `Ord`.
///
/// The order is at MILLISECOND granularity, rounded to the nearest millisecond, because
/// that is the precision this service's instants survive a round trip through the wire
/// at: a timestamp travels on the awsJson protocols as epoch seconds in a JSON NUMBER,
/// which a client (and this emulator's own deserializer) reads as a double, and a double
/// cannot hold a nanosecond-precise instant of the current epoch — the value comes back
/// a few hundred nanoseconds off, in either direction. Comparing at a finer granularity
/// than the wire carries would make a read at an instant the SERVICE ITSELF reported —
/// `AsOf` set to a unit's own `LastModified`, which is how a caller asks for "the state
/// as of that write" — miss the very state it reported, which is not a difference any
/// caller can account for. Milliseconds is also the granularity [`now`] stamps with, so
/// a stored instant is exactly on a boundary and the rounding only absorbs the wire's
/// error.
///
/// The arithmetic is SATURATING because the instant can come straight off the wire: a
/// timestamp arrives as a JSON number and is built with `DateTime::from_secs_f64`, whose
/// cast saturates, so a caller can name a second count whose millisecond count no `i64`
/// holds. Saturating keeps the key total and order-preserving at those extremes — an
/// absurdly large instant is "at or after everything stored", an absurdly small one is
/// "before everything" — instead of panicking under overflow checks or wrapping to a
/// negative key and mis-ordering the comparison.
pub fn instant_key(at: &DateTime) -> i64 {
    const NANOS_PER_MILLI: i64 = 1_000_000;
    at.secs()
        .saturating_mul(1_000)
        .saturating_add((i64::from(at.subsec_nanos()) + NANOS_PER_MILLI / 2) / NANOS_PER_MILLI)
}

/// Now, as the timestamp an invoice unit records in `LastModified`.
///
/// Built from whole seconds and MILLISECONDS rather than through an `f64`, so two writes
/// a few milliseconds apart are ordered exactly by [`instant_key`] and a stamp this
/// service hands out survives the round trip back through an `AsOf` — see
/// [`instant_key`] for why a finer stamp would not.
pub fn now() -> DateTime {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_millis() * 1_000_000,
    )
}

/// What the tag plane would hold for a resource once one call's `ResourceTags` array is
/// written over the tags it holds now (`held`, `None` for a resource that holds none) —
/// one entry per key, the array's value winning over the held one and a key the array
/// names more than once keeping the last value it carried — or `Err(count)` with the
/// number of distinct keys that write would leave on the resource when that is more than
/// [`MAX_RESOURCE_TAGS`].
///
/// A resource holds at most that many tags however the tags got there, so EVERY write of
/// tags decides this BEFORE it persists anything, and decides it on the tag SET the write
/// would leave on the resource rather than on how many entries the call's array carries:
/// an entry whose key that set already accounts for — one the resource holds already, or
/// one an earlier entry of the same array named — replaces a value instead of adding a
/// key, so it is never a step towards the quota. Two writes that would leave the resource
/// holding the same tags are therefore answered the same way, whichever call shape they
/// arrived in. The refusal is the quota error the ANSWERING OPERATION models —
/// `ServiceQuotaExceededException` where it declares one, otherwise the
/// `ValidationException` it does declare — carried by [`TAG_QUOTA_EXCEEDED_MESSAGE`], and
/// nothing of the refused write persists.
pub fn resource_tags_after_write(
    held: Option<&BTreeMap<String, String>>,
    tags: Option<Vec<ResourceTag>>,
) -> Result<BTreeMap<String, String>, usize> {
    let mut stored = held.cloned().unwrap_or_default();
    stored.extend(
        tags.unwrap_or_default()
            .into_iter()
            .map(|tag| (tag.key, tag.value)),
    );
    if stored.len() > MAX_RESOURCE_TAGS {
        return Err(stored.len());
    }
    Ok(stored)
}

/// [`resource_tags_after_write`] for a resource THIS CALL CREATES: it holds no tags yet,
/// so the write leaves it holding exactly the distinct keys the call's array names.
pub fn resource_tags_for_new_resource(
    tags: Option<Vec<ResourceTag>>,
) -> Result<BTreeMap<String, String>, usize> {
    resource_tags_after_write(None, tags)
}

/// A tag map as the modelled `ResourceTagList`, in a deterministic key order.
pub fn resource_tag_list(tags: &BTreeMap<String, String>) -> Vec<ResourceTag> {
    tags.iter()
        .map(|(key, value)| ResourceTag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The invoice profile this service holds for a linked account of its organization.
///
/// Every other member of `InvoiceProfile` describes the invoice RECEIVER itself —
/// `ReceiverName`, `ReceiverAddress`, `ReceiverEmail`, `Issuer` and
/// `TaxRegistrationNumber` (`smithy_query(query="shape", shape="InvoiceProfile")`) —
/// and all five are set through the billing/tax-settings plane that this service does
/// not own: no operation of THIS model writes any of them, so there is nothing in state
/// to report them from. Each is an optional member, so they are reported ABSENT —
/// "not on file here" — rather than invented. `AccountId`, the member that says whose
/// profile this is, is the one this service can answer, and it is answered only for an
/// account [`ComAmazonawsInvoicingInvoicingState::accounts_the_requester_may_read`] holds.
pub fn invoice_profile(account_id: &str) -> InvoiceProfile {
    InvoiceProfile {
        account_id: Some(account_id.to_string()),
        ..Default::default()
    }
}

/// The message a `ResourceNotFoundException` of the INVOICE-UNIT and TAG planes carries:
/// a CONSTANT, the same sentence whatever was asked for.
///
/// The shape's own documentation gives it (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")` -> "The resource could not be found."), and those
/// planes' recorded 404s answer with exactly that sentence — the resource that was not
/// found travels in its own member instead of being spliced into the prose, so a client
/// can read it mechanically and a client matching the documented sentence still matches.
///
/// The procurement-portal-preference plane words its own 404 from the REQUEST instead;
/// see [`no_such_procurement_portal_preference`].
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The resource could not be found.";

/// The 404 an operation of the INVOICE-UNIT or TAG plane answers for an ARN that is not
/// in state: the constant message, plus `arn` in the shape's own ARN member, keyed the
/// way those planes' recorded bodies key it ([`MessageKey::ModelMemberName`]).
///
/// The ARN is reported only when it is one that member can hold
/// ([`crate::service::validation::reportable_resource_arn`]), as every value this service
/// reports is held to the bound the model gives the member it travels in: that member is
/// modelled `InvoiceUnitArnString`, `@length(max: 256)`, while the tag plane accepts a
/// `TagrisArn` of up to 2048 characters, so a legal request can name a resource no
/// reporting member of this model holds. Such an ARN leaves the member ABSENT rather than
/// being cut down to the bound — a truncated ARN names a different resource — which is the
/// same answer this service gives for an identifier that member's `@pattern` does not
/// admit ([`no_such_resource_unnamed`]). An ARN from a member the model already caps at
/// 256, as every `InvoiceUnitArn` input of the invoice-unit plane is, is reported whole.
pub fn no_such_resource(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        resource_id: crate::service::validation::reportable_resource_arn(arn),
        message_key: MessageKey::ModelMemberName,
    }
}

/// The kind of resource the procurement-portal plane's refusals name, as its recorded
/// bodies spell it — the prose form of the `procurement-portal-preference` resource this
/// plane's ARNs name (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceArnString")`: `@pattern
/// ^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$`).
const PROCUREMENT_PORTAL_PREFERENCE_KIND: &str = "Procurement portal preference";

/// The 404 an operation of the PROCUREMENT-PORTAL-PREFERENCE plane answers for an ARN
/// that names no stored preference.
///
/// Recorded behaviour: this plane's message is BUILT FROM THE REQUEST — it names the kind
/// of resource that was asked about and the exact ARN the request supplied — rather than
/// the constant sentence the shape's documentation gives, and it travels under the
/// capitalised key this plane's recorded bodies carry ([`MessageKey::Capitalised`]). The
/// ARN is reported in the shape's own ARN member as well, so a client can still read it
/// mechanically instead of parsing it out of the prose.
///
/// Held to the bound the model gives the member it travels in
/// ([`crate::service::validation::bounded_message`]), as every message this service words
/// from a request is, so no wording of this service rests on a maximum that lives in
/// another shape — here `ProcurementPortalPreferenceArnString`'s own `@length(max: 256)`,
/// which already leaves the sentence inside the bound. The ARN member is held to its own
/// modelled bound the same way ([`crate::service::validation::reportable_resource_arn`],
/// see [`no_such_resource`]); that same input maximum of 256 already leaves this plane's
/// ARN inside it, so this refusal reports it whole.
pub fn no_such_procurement_portal_preference(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(crate::service::validation::bounded_message(format!(
            "{PROCUREMENT_PORTAL_PREFERENCE_KIND} with arn {arn} does not exist"
        ))),
        resource_id: crate::service::validation::reportable_resource_arn(arn),
        message_key: MessageKey::Capitalised,
    }
}

/// The 404 a reader answers for a resource named by something that is NOT an ARN — an
/// invoice id, say: the same constant message, with the ARN member left unset.
///
/// That member is modelled `InvoiceUnitArnString`, whose `@pattern` admits only an ARN
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so a
/// non-ARN identifier cannot be reported in it without answering a value the model
/// says that member never holds.
pub fn no_such_resource_unnamed() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        resource_id: None,
        message_key: MessageKey::ModelMemberName,
    }
}

/// The 403 an operation of this service answers when the requester may not read what it
/// asked about: the account it asked about is neither its own nor a linked account the
/// requester's organization contains
/// ([`ComAmazonawsInvoicingInvoicingState::accounts_the_requester_may_read`]), so the
/// requester is not authorized for that account's resources.
///
/// The message is BUILT FROM THE REQUEST, as the recorded refusals of this service state
/// it: it names the requester ([`::engine::emulator_account_id`], the account this
/// emulator presents to every caller) and the account the requester was refused.
///
/// `resourceName` is left absent: the model types it as an `InvoiceUnitArnString`
/// (`smithy_query(query="shape", shape="AccessDeniedException")`), so it names an
/// invoice unit rather than an account.
///
/// Held to the bound the model gives the member it travels in
/// ([`crate::service::validation::bounded_message`]), as every message this service words
/// from a request is; `AccountIdString`'s own `@pattern ^\d{12}$` already leaves this
/// sentence inside it.
pub fn not_authorized_for_account(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(crate::service::validation::bounded_message(format!(
            "Account {} is not authorized to access resources for account {account_id}",
            ::engine::emulator_account_id()
        ))),
        resource_name: None,
    }
}

// The 400 `ValidationException` that states a member constraint the request failed
// lives in `crate::service::validation`, with the rest of how this service WORDS a
// refusal: the `message` envelope, each `fieldList` entry's own clause and the `reason`
// they are stated under are one rendering, reached by every refusal this service
// answers.

/// A `NextToken` this service did not mint, or one minted for another operation's walk.
#[derive(Debug)]
pub struct InvalidPageToken;

/// The `NextToken` that resumes a paginated walk at `position`, under the walk's own
/// `prefix`.
///
/// The token is opaque to the caller (hex over the prefixed position) and the prefix is
/// what makes a token this service did not mint — or one minted for another operation's
/// walk — recognisable rather than silently resuming from arbitrary bytes.
///
/// A `NextToken` carries the model's constraints on BOTH sides of a paginated operation
/// (the output member a page hands back and the input member the next call hands in), so
/// a token has to stay inside them to be one this service will accept back. Hex doubles
/// the byte count, so `position` must be text whose length THIS SERVICE bounds — an
/// identifier it mints, an enum value, or the ordinal
/// [`encode_page_offset`] encodes — and never a caller-supplied value whose own modelled
/// length can carry the token past the `NextToken` member's maximum. Where a walk's
/// order is keyed by caller-supplied text, page it with [`encode_page_offset`].
pub fn encode_page_token(prefix: &str, position: &str) -> String {
    hex::encode(format!("{prefix}{position}"))
}

/// The `NextToken` that resumes a walk at the `offset`-th entry of its order, under the
/// walk's own `prefix`.
///
/// An ORDINAL is what a walk keyed by caller-supplied text pages on: the position it
/// names is a number bounded by how many entries the walk can hold, so the token stays
/// inside the `NextToken` member's modelled maximum whatever a caller stored, while a
/// key built from those members would not (see [`encode_page_token`]).
pub fn encode_page_offset(prefix: &str, offset: usize) -> String {
    encode_page_token(prefix, &offset.to_string())
}

/// The ordinal a caller's `NextToken` resumes at, or `Ok(None)` when it asked for the
/// first page. A token that is not one this `prefix`'s walk minted — including one whose
/// position is not an ordinal — is [`InvalidPageToken`], answered as the operation's own
/// modelled `ValidationException` ([`invalid_next_token`]).
pub fn decode_page_offset(
    prefix: &str,
    token: Option<&str>,
) -> Result<Option<usize>, InvalidPageToken> {
    match decode_page_token(prefix, token)? {
        Some(position) => position
            .parse::<usize>()
            .map(Some)
            .map_err(|_| InvalidPageToken),
        None => Ok(None),
    }
}

/// The position a caller's `NextToken` resumes at, or `Ok(None)` when it asked for the
/// first page. A token that is not one this `prefix`'s walk minted is
/// [`InvalidPageToken`], which the operation answers as its own modelled
/// `ValidationException` ([`invalid_next_token`]).
pub fn decode_page_token(
    prefix: &str,
    token: Option<&str>,
) -> Result<Option<String>, InvalidPageToken> {
    let Some(token) = token else {
        return Ok(None);
    };
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|plain| plain.strip_prefix(prefix).map(str::to_string))
        .map(Some)
        .ok_or(InvalidPageToken)
}

/// What one page of a paginated listing holds, and where the walk resumes after it.
///
/// Every paginated listing of this service answers ONE rule, which is why it is stated
/// here beside the token codecs rather than in each listing's own module: a page holds at
/// most the page size the request resolved to, taken in the walk's own order from the
/// position the caller's `NextToken` named, and it carries a token naming the first entry
/// that did NOT fit — so the next call resumes exactly there and the last page of a walk
/// carries none. A listing differs only in the registry it walks and what it reports per
/// entry, so neither the page's cut nor the position its token names can drift between
/// them.
#[derive(Debug)]
pub struct Page<Item> {
    /// The entries this page reports, in the walk's order, at most `page_size` of them.
    pub items: Vec<Item>,
    /// The `NextToken` that resumes the walk at the first entry this page did not hold,
    /// or `None` when it held the last of them.
    pub next_token: Option<String>,
}

/// How many entries one page of a walk holds: what the request's `MaxResults` asked for,
/// bounded by that walk's own maximum.
///
/// `requested` is `None` for a request that omitted a `MaxResults` the model gives no
/// `@default` — which asks for a full page — and the boundary validator has already
/// refused a value outside the member's `@range`, so this only guards the conversion and
/// keeps a page inside `max_page_size` whatever arrived.
pub fn resolve_page_size(requested: Option<i32>, max_page_size: usize) -> usize {
    requested
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(max_page_size)
        .clamp(1, max_page_size)
}

/// The [`Page`] a walk of `entries` reports, resuming at the key `resume_at` names and
/// reporting each admitted entry under `token_prefix`'s own token.
///
/// `entries` is one of this service's `BTreeMap` registries, so the walk is in one
/// stable, deterministic order and a token — the KEY the next page starts at — stays
/// meaningful across calls: a resumed page seeks straight to that key rather than
/// re-walking what the earlier pages already reported. `admit` answers `None` for an
/// entry this listing does not report (one a filter excludes, or one that did not exist
/// at a point-in-time read's instant), which takes no room in the page and is not what a
/// token names.
pub fn page_by_key<Entry, Item>(
    entries: &BTreeMap<String, Entry>,
    resume_at: Option<&str>,
    page_size: usize,
    token_prefix: &str,
    mut admit: impl FnMut(&Entry) -> Option<Item>,
) -> Page<Item> {
    let walk = match resume_at {
        Some(from) => entries.range(from.to_string()..),
        None => entries.range::<String, _>(..),
    };
    let mut items = Vec::new();
    let mut next_token = None;
    for (key, entry) in walk {
        let Some(item) = admit(entry) else {
            continue;
        };
        if items.len() == page_size {
            next_token = Some(encode_page_token(token_prefix, key));
            break;
        }
        items.push(item);
    }
    Page { items, next_token }
}

/// The [`Page`] a walk whose position is an ORDINAL reports, resuming at the entry
/// `resume_at` names and minting its token under `token_prefix`.
///
/// Same rule as [`page_by_key`], for a walk whose order is keyed by caller-supplied text:
/// there the position a token names is the ordinal of the entry the next page starts at
/// rather than that key, so the token stays inside the `NextToken` member's modelled
/// maximum whatever a caller stored (see [`encode_page_offset`]).
pub fn page_by_offset<Entry: Clone>(
    entries: &BTreeMap<String, Entry>,
    resume_at: Option<usize>,
    page_size: usize,
    token_prefix: &str,
) -> Page<Entry> {
    let resume_at = resume_at.unwrap_or(0);
    let items: Vec<Entry> = entries
        .values()
        .skip(resume_at)
        .take(page_size)
        .cloned()
        .collect();
    // Where the next page resumes: past everything reported through this one. A token is
    // minted only while entries remain, so the last page of a walk carries none and a
    // position past the end of the listing is an empty last page.
    let reported_through = resume_at.saturating_add(items.len());
    let next_token = (reported_through < entries.len())
        .then(|| encode_page_offset(token_prefix, reported_through));
    Page { items, next_token }
}

/// The 400 any paginated operation of this service answers for a `NextToken` it did not
/// mint: the model's own `invalidNextToken` reason
/// ([`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`]), so a client branches on
/// the reason rather than on the prose.
///
/// `message_key` is the walking operation's own plane: a paginated walk of invoice units
/// or invoice summaries keys the text as those planes do, a walk of procurement portal
/// preferences as that plane does (see [`MessageKey`]).
pub fn invalid_next_token(message_key: MessageKey) -> ValidationException {
    ValidationException {
        message: Some("The NextToken provided is not valid".to_string()),
        reason: Some("invalidNextToken".to_string()),
        message_key,
        ..Default::default()
    }
}

/// The 500 a handler answers when the engine it was constructed with is not the shared
/// in-memory engine, so this service has no store to read. Never construct an engine in
/// a handler instead: that store is private to the request and dropped when it returns.
/// The 500 a handler answers when the engine it was constructed with is not the shared
/// in-memory engine, so this service has no store to read. Never construct an engine in
/// a handler instead: that store is private to the request and dropped when it returns.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: Some("the invoicing emulator's state store is unavailable".to_string()),
        retry_after_seconds: None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsInvoicingInvoicingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsInvoicingInvoicingState>)
}
