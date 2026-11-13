// SPDX-License-Identifier: CC0-1.0

//! @op PutTaxRegistration
//!
//! Stores the submitted TRN as this account's tax registration and reports the
//! status it was stored with. Writing again replaces the account's record and
//! moves its revision on — past every revision this account's documents have
//! already been minted under, including those of a registration that was
//! deleted — so the document access tokens an earlier write minted no longer
//! name anything.
//!
//! The documents the entry submits are kept, not just named: the content of each
//! `VerificationDetails$taxRegistrationDocuments` entry — an uploaded file's
//! bytes, or the object an `s3Location` references — is held as this service's own
//! copy where a read of that document reports it
//! ([`crate::state::documents`]), which is what gives
//! `GetTaxRegistrationDocument` a document to download. The copies of the
//! revision this write retires go with its access tokens.
//!
//! The status is the outcome of validating the submitted TRN, which the model
//! documents as `Verified`, `Pending` or `Rejected`. Validating takes time, so a
//! write stores the registration `Pending` and the validation falls due one
//! [`crate::state::TRN_VALIDATION_DELAY`] later; every read then reports what
//! that validation reached ([`crate::state::trn_validation_status`]). Accepting
//! the entry and validating the NUMBER are separate facts: this handler refuses
//! everything about the entry it will not stand behind, and the validation that
//! follows judges the submitted number against the structure its jurisdiction
//! publishes, which a stored registration can fail.
//!
//! The refusals this handler adds beyond the constraints the boundary
//! validator enforces, spelled out here so a later fixer does not read them as
//! model-derived:
//!
//! * a `legalAddress` submitted for a registration type AWS derives the address
//!   from the tax number for (`crate::state::TYPES_WITH_DERIVED_LEGAL_ADDRESS`)
//!   is refused: the address is not the caller's to supply. India's `PAN` is such
//!   a type, so a `PAN` entry is stored with its address filled in rather than
//!   submitted. Refused BEFORE every rule that reads the address's own value,
//!   including the jurisdiction rule below: a member the submitted type does not
//!   admit is a field violation, and a value the caller may not send is not
//!   evidence for another refusal.
//! * a registration type the service ties to ONE country
//!   (`crate::state::TYPE_JURISDICTIONS`) set for a registration held somewhere
//!   else is refused with the `errorCode` the model publishes for that type
//!   (`crate::state::TYPE_OUTSIDE_ITS_COUNTRY_CODES`): the country a
//!   registration is held in is its `legalAddress.countryCode`, so an entry
//!   naming another country names a customer that may not set the type. Reached
//!   only for a type whose address the request supplies — see that constant.
//! * a registration type the account's STORED registration blocks
//!   (`crate::state::TYPES_BLOCKED_BY_A_STORED_TYPE`) is refused with the
//!   `errorCode` the model publishes for that pair, and the stored record is
//!   left as it was — see
//!   [`crate::state::ComAmazonawsTaxsettingsTaxsettingsState::check_registration_type_against_stored`],
//!   the one place both writers judge an entry against what the account holds.
//! * `legalName` and `legalAddress` are optional on `TaxRegistrationEntry` and
//!   `@required` on every shape that REPORTS a registration (`TaxRegistration`,
//!   `TaxRegistrationWithJurisdiction`), so an entry that omits one is refused
//!   UNLESS the jurisdiction it identifies is one the service fills that member
//!   in for: Brazil for the legal name, Brazil's `CNPJ` type and Mexico for both
//!   (`crate::state::COUNTRIES_WITH_DERIVED_LEGAL_NAME`,
//!   `crate::state::COUNTRIES_WITH_DERIVED_LEGAL_ADDRESS`). Filling one in means
//!   reading the account's registration back from the tax authority's registry,
//!   which this emulator has none of, so the stored record reports the
//!   jurisdiction the entry identified and
//!   `crate::state::DERIVED_MEMBER_UNKNOWN` for what only that registry holds —
//!   the write is accepted, because the service accepts it.
//!   An entry that identifies no jurisdiction at all (no `legalAddress`, and a
//!   type the model ties to no one country) is refused for the missing address:
//!   neither carve-out can be read without a country, and the `countryCode` every
//!   read reports could be neither supplied nor derived.
//! * the model's non-blank pattern `^(?!\s*$)[\s\S]+$` is a negative lookahead
//!   the boundary validator's regex engine cannot compile, so this handler
//!   applies it to `registrationId`, `legalName` and every address line.
//! * an address member the model documents the API as accepting only for certain
//!   jurisdictions (`crate::state::ADDRESS_MEMBER_JURISDICTIONS`) is refused as a
//!   field violation when the address that carries it is held anywhere else: the
//!   restriction is stated per country, which no constraint on the shared
//!   `Address` shape can express. Applied by
//!   [`crate::state::check_address`], so every operation that takes an address
//!   judges it the same way.
//! * a write aimed at an account whose stored TRN is still `Pending` is refused
//!   with the modelled `ConflictException`: the model documents "You can't set a
//!   TRN if there's a pending TRN. You'll need to delete the pending TRN first",
//!   so the outstanding record is left exactly as it was and the caller deletes
//!   it (or waits for its validation to run) before writing again — see
//!   [`crate::state::pending_registration_conflict`].
//! * a write that would take the resident store past
//!   [`crate::state::MAX_STORED_REGISTRATIONS`] is refused with the modelled
//!   `ConflictException`; see that constant for why the store is bounded.
//! * an `additionalTaxInformation` object that is not exactly ONE country block
//!   for the entry's own jurisdiction is refused. The shape states this about
//!   itself rather than about any country — "You can only specify one of the
//!   following parameters and the value can't be empty", and "The parameter that
//!   you specify must match the country for the TRN" — so no block, several
//!   blocks, or a block belonging to another jurisdiction is a field violation;
//!   the blocks the shape declares and the country each belongs to are
//!   `crate::state::ADDITIONAL_INFO_BLOCKS`, applied by
//!   [`crate::state::check_additional_info_blocks`].
//! * the `additionalTaxInformation` and the other members the entry's country
//!   requires, which are the per-country business rules
//!   [`crate::state::COUNTRY_RULES`] carries and
//!   [`crate::state::check_country_requirements`] applies.
//!
//! Every one of these is decided in `state.rs` — the entry's own refusals by
//! [`crate::state::check_tax_registration_entry`] and the one that depends on
//! what the account already holds by
//! [`crate::state::ComAmazonawsTaxsettingsTaxsettingsState::check_registration_type_against_stored`]
//! — so this operation and `BatchPutTaxRegistration` judge one entry, and one
//! account's state, the same way.
//!
//! The per-country rules are the sentences the operation's own model
//! documentation states for `taxRegistrationEntry` — "Kenya You must specify the
//! `personType` in the `kenyaAdditionalInfo` field of the
//! `additionalTaxInformation` object", and the equivalent sentence for every
//! other country the documentation names — together with the per-country
//! requirements the model states on the entry's own MEMBERS instead, which are
//! rules of the same kind: the state, region or province an address held in
//! certain jurisdictions must name, and the resale a Canadian provincial sales
//! tax claim must confirm. They live in
//! [`crate::state::COUNTRY_RULES`], one row per documented sentence, and are
//! applied BEFORE the state write, so an entry AWS refuses is refused here with
//! the modelled `ValidationException` and nothing is persisted. They are in
//! `state.rs` rather than in this file so every writer of a
//! `TaxRegistrationEntry` — `BatchPutTaxRegistration`, the per-account fan-out of
//! this operation — applies the same set.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `PutTaxRegistration::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutTaxRegistration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-taxsettings-taxsettings/smithy-model/com-amazonaws-taxsettings-taxsettings.json")
//! ```
//!
//! Adds or updates tax registration for a single account. You can't set a TRN if there's a
//! pending TRN. You'll need to delete the pending TRN first. To call this API operation for
//! specific countries, see the following country-specific requirements. Bangladesh You must
//! specify the tax registration certificate document in the taxRegistrationDocuments field
//! of the VerificationDetails object. Brazil You must complete the tax registration process
//! in the Payment preferences page in the Billing and Cost Management console. After your
//! TRN and billing address are verified, you can call this API operation. For Amazon Web
//! Services accounts created through Organizations, you can call this API operation when
//! you don't have a billing address. Georgia The valid personType values are Physical
//! Person and Business. Indonesia PutTaxRegistration: The use of this operation to submit
//! tax information is subject to the Amazon Web Services service terms. By submitting,
//! you’re providing consent for Amazon Web Services to validate NIK, NPWP, and NITKU data,
//! provided by you with the Directorate General of Taxes of Indonesia in accordance with
//! the Minister of Finance Regulation (PMK) Number 112/PMK.03/2022.
//! BatchPutTaxRegistration: The use of this operation to submit tax information is subject
//! to the Amazon Web Services service terms. By submitting, you’re providing consent for
//! Amazon Web Services to validate NIK, NPWP, and NITKU data, provided by you with the
//! Directorate General of Taxes of Indonesia in accordance with the Minister of Finance
//! Regulation (PMK) Number 112/PMK.03/2022, through our third-party partner PT Achilles
//! Advanced Management (OnlinePajak). You must specify the taxRegistrationNumberType in the
//! indonesiaAdditionalInfo field of the additionalTaxInformation object. If you specify
//! decisionNumber, you must specify the ppnExceptionDesignationCode in the
//! indonesiaAdditionalInfo field of the additionalTaxInformation object. If the
//! taxRegistrationNumberType is set to NPWP or NITKU, valid values for
//! ppnExceptionDesignationCode are either 01, 02, 03, 07, or 08. For other
//! taxRegistrationNumberType values, ppnExceptionDesignationCode must be either 01, 07, or
//! 08. If ppnExceptionDesignationCode is 07 or 08, you must specify the decisionNumber in
//! the indonesiaAdditionalInfo field of the additionalTaxInformation object. Kenya You must
//! specify the personType in the kenyaAdditionalInfo field of the additionalTaxInformation
//! object. If the personType is Physical Person, you must specify the tax registration
//! certificate document in the taxRegistrationDocuments field of the VerificationDetails
//! object. Malaysia The sector valid values are Business and Individual. RegistrationType
//! valid values are NRIC for individual, and TIN and sales and service tax (SST) for
//! Business. For individual, you can specify the taxInformationNumber in
//! MalaysiaAdditionalInfo with NRIC type, and a valid MyKad or NRIC number. For business,
//! you must specify a businessRegistrationNumber in …
//!
//! ## Input
//!
//! * `accountId` (AccountId, len 12..12, pattern ^\d+$) — Your unique account identifier.
//! * `taxRegistrationEntry` (TaxRegistrationEntry) **required** — Your TRN information that
//!   will be stored to the account mentioned in accountId.
//!
//! ## Output
//!
//! * `status` (TaxRegistrationStatus, one of: Deleted | Pending | Rejected | Verified) — The
//!   status of your TRN stored in the system after processing. Based on the validation
//!   occurring on the TRN, the status can be Verified, Pending or Rejected.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The exception when the input is creating conflict with
//!   the given state.
//! * `InternalServerException` (HTTP 500) — The exception thrown when an unexpected error
//!   occurs when processing a request.
//! * `ValidationException` (HTTP 400) — The exception when the input doesn't pass validation
//!   for at least one of the input parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutTaxRegistrationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-taxsettings-taxsettings.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutTaxRegistrationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.taxsettings#PutTaxRegistration")]
#[derive(Debug)]
pub enum PutTaxRegistrationErrorModeled {
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_tax_registration::PutTaxRegistrationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutTaxRegistrationError = crate::scaffolded::Scaffolded<PutTaxRegistrationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutTaxRegistrationErrorModeled> for PutTaxRegistrationError {
    fn from(modeled: PutTaxRegistrationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutTaxRegistration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutTaxRegistration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutTaxRegistration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutTaxRegistration<Eng>
{
    type Input = PutTaxRegistrationInput;
    type Output = PutTaxRegistrationOutput;
    type Error = PutTaxRegistrationError;

    const NAME: &'static str = "PutTaxRegistration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_TAX_REGISTRATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_TAX_REGISTRATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            PutTaxRegistrationErrorModeled::InternalServerException(
                crate::state::state_unavailable(),
            )
        })?;
        let account_id = crate::state::resolve_account_id(input.account_id);
        let entry = &input.tax_registration_entry;

        // Whether this entry is storable at all is decided in ONE place, which
        // `BatchPutTaxRegistration` uses too, so the two writers cannot disagree
        // about one entry: the refusals the module header lists (a legal address
        // submitted for a type AWS derives it from, a legal name or address the
        // entry's own jurisdiction requires and it omits, a blank value the
        // boundary validator's regex engine could not refuse) and then the
        // per-country business rules the model documents, keyed off the
        // jurisdiction the entry identifies — all of it BEFORE the write, so an
        // entry AWS refuses persists nothing. Everything else on this input (the
        // 12-digit `accountId`, the 1..=200 lengths, the closed
        // `TaxRegistrationType` / `Sector` value sets, the `certifiedEmailId`
        // pattern, the 1..=5 document list) the boundary validator already
        // refused before this handler ran.
        crate::state::check_tax_registration_entry(entry)
            .map_err(PutTaxRegistrationErrorModeled::ValidationException)?;

        // And the submitted NUMBER, judged last of the entry's facts by the same
        // one judgement the batch write makes: a number its jurisdiction's
        // authority does not issue is refused here with the modelled
        // `ValidationException` naming the member that carried it, rather than
        // stored and failed afterwards.
        crate::state::check_registration_number(entry)
            .map_err(|refusal| {
                PutTaxRegistrationErrorModeled::ValidationException(refusal.into_validation())
            })?;

        // The CONTENT of the documents this entry submits, read before this
        // service's store is locked, because a document that REFERENCES an object
        // is read from the object store (the lock order
        // [`crate::state::documents`] states). Read whether or not the write goes
        // through: nothing is held until it has.
        let submitted = crate::state::documents::submitted_documents(&*self.engine, entry);

        // A write replaces the account's record and bumps its revision, which the
        // document access tokens derived from it carry — unless the record it
        // would replace is a TRN whose validation is still outstanding, which the
        // model says must be deleted first, or the store is at its bound. The
        // write itself lives in `state.rs` so this operation and
        // `BatchPutTaxRegistration` cannot store one entry two different ways,
        // nor judge one account's state two different ways; both refusals are
        // this operation's modelled `ConflictException`.
        let mut store = state.lock();

        // What the account already holds can itself refuse the type the request
        // sets, which is a fact about the stored record rather than about the
        // entry, so it is judged here rather than in the entry checker — still
        // in `state.rs`, so the batch writer judges it the same way.
        store
            .check_registration_type_against_stored(&account_id, entry)
            .map_err(PutTaxRegistrationErrorModeled::ValidationException)?;

        // Where the documents of the record this write REPLACES are kept, read
        // before it is replaced: the write retires that revision's access tokens,
        // so the copies those tokens named are retired with them.
        let retired = store.held_document_coordinates(&account_id);
        let status = store
            .store_registration(&account_id, entry, ::std::time::Instant::now())
            .map_err(PutTaxRegistrationErrorModeled::ConflictException)?;
        let revision = store.held_document_coordinates(&account_id).map(|(revision, _)| revision);
        drop(store);

        // The documents themselves, held where a read of one reports it — and the
        // copies of the revision this write retired, dropped. Both with the store
        // above unlocked.
        if let Some((revision, documents)) = retired {
            crate::state::documents::release_documents(
                &*self.engine,
                &account_id,
                revision,
                documents,
            );
        }
        if let Some(revision) = revision {
            let _held = crate::state::documents::hold_documents(
                &*self.engine,
                &account_id,
                revision,
                &submitted,
            );
        }

        Ok(PutTaxRegistrationOutput {
            status: Some(status),
        })
    }
}
