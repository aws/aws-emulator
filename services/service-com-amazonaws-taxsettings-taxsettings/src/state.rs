// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-taxsettings-taxsettings service.
//!
//! It holds the tax-registration (TRN) record of each account and the account's
//! tax-inheritance election, plus the projections every reader of a stored
//! record needs. The projections live here, beside the stored record, so
//! `GetTaxRegistration` and `ListTaxRegistrations` report one record the same
//! way instead of each rebuilding it from the raw fields.
//!
//! The structures the jurisdictions publish for a tax registration number — what
//! the TRN validation of a stored record judges it by — live in
//! [`registration_number`], so the state struct and its projections stay about
//! records rather than about number formats.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, Instant, SystemTime};

use ::aws_smithy_types::base64;

pub mod documents;
pub mod organization;
pub mod registration_number;
pub mod support_case;

use crate::ops::types::{
    AccountMetaData, AdditionalInfoRequest, AdditionalInfoResponse, Address, Authority,
    CanadaAdditionalInfo, CaseCreationLimitExceededException, ConflictException,
    IndonesiaAdditionalInfo, MalaysiaAdditionalInfo,
    InternalServerException, Jurisdiction, ResourceNotFoundException, SupplementalTaxRegistration,
    SupplementalTaxRegistrationEntry, TaxDocumentMetadata, TaxExemption, TaxExemptionDetails,
    TaxExemptionType, TaxRegistration, TaxRegistrationEntry, TaxRegistrationWithJurisdiction,
    ValidationException, ValidationExceptionField,
};

/// The `TaxRegistrationStatus` values, from the model's enum
/// (`Verified | Pending | Deleted | Rejected`). The model documents the value as
/// the outcome of validating the submitted TRN: `PutTaxRegistration`'s `status`
/// is "the status of your TRN stored in the system after processing. Based on the
/// validation occurring on the TRN, the status can be Verified, Pending or
/// Rejected."
pub const STATUS_PENDING: &str = "Pending";
/// See [`STATUS_PENDING`]: the validation of the submitted TRN found no fault,
/// which is what a stored registration whose number its jurisdiction's authority
/// CONFIRMS reaches once that validation falls due — see
/// [`trn_validation_status`].
pub const STATUS_VERIFIED: &str = "Verified";
/// See [`STATUS_PENDING`]: the validation of the submitted TRN failed, which is
/// what a stored registration whose number that authority's own rule does NOT
/// confirm reaches — see [`trn_validation_status`]. Being WRITTEN the way such a
/// number is written and being CONFIRMED by the authority's rule are separate
/// facts, answered at separate times: the write refuses a value that is not
/// written that way at all ([`check_registration_number`]) and the validation
/// that follows judges whether the number it stored is one the register
/// confirms.
pub const STATUS_REJECTED: &str = "Rejected";
/// See [`STATUS_PENDING`]. Never reached: `DeleteTaxRegistration` REMOVES the
/// account's record, which is what the real service does too — a read of a
/// deleted registration answers `ResourceNotFoundException`, not a registration
/// in this status.
pub const STATUS_DELETED: &str = "Deleted";

/// The status the validation of a stored TRN reaches: what the submitted number
/// is judged to be once the validation the write started falls due.
///
/// The model documents the status as the outcome of "the validation occurring on
/// the TRN", and what is left for that validation to establish is whether the
/// number the write stored is one its jurisdiction's authority confirms
/// ([`registration_number::confirmed_by_its_authority`]): a number of the
/// published layout whose check digit that authority's own rule does not
/// compute is a well-formed number the register does not know, so its validation
/// fails ([`STATUS_REJECTED`]), while one the rule confirms — or one no published
/// rule judges — is contradicted by nothing this service holds and its
/// validation succeeds ([`STATUS_VERIFIED`]). How the number is WRITTEN is not
/// this question: the write already refused a value that is not written the way
/// such a number is ([`check_registration_number`]).
///
/// A function of what the write STORED — the number, its registration type and
/// the jurisdiction the registration is held in — and of nothing else, so every
/// reader of one record settles it the same way and no read can move a record
/// the previous read did not.
pub fn trn_validation_status(
    country_code: &str,
    registration_type: &str,
    registration_id: &str,
) -> &'static str {
    if registration_number::confirmed_by_its_authority(
        country_code,
        registration_type,
        registration_id,
    ) {
        STATUS_VERIFIED
    } else {
        STATUS_REJECTED
    }
}


/// How long after a write the TRN's validation is due.
///
/// AWS documents no span for it, so this is the emulator's own, inside the
/// project's 250-750 ms band for a transition the clock owes: it is never zero, so
/// a caller that reads the registration straight after writing it still sees the
/// `Pending` the write reported — and still meets the refusals that status
/// carries — and it is short enough that a caller polling for a terminal status
/// gets one.
pub const TRN_VALIDATION_DELAY: Duration = Duration::from_millis(500);

/// The status and due instant a write stores a registration with.
///
/// The write starts the validation of the submitted TRN, so the registration is
/// stored in [`STATUS_PENDING`] while that validation is outstanding and its
/// validation falls due [`TRN_VALIDATION_DELAY`] after the write. One pairing
/// for every kind of stored registration, so the status a write reports and the
/// instant the validation behind it falls due cannot drift apart.
fn outstanding_trn_validation(written_at: Instant) -> (String, Option<Instant>) {
    (
        STATUS_PENDING.to_string(),
        Some(written_at + TRN_VALIDATION_DELAY),
    )
}

/// The clock-owed TRN validation a stored registration carries.
///
/// The account's own TRN and a supplemental one both have that transition, for
/// the same reason — a write records the instant the validation of the submitted
/// number falls due ([`outstanding_trn_validation`]) and the status the record
/// reports moves to what that validation reached once the clock has made it due —
/// so the rule lives here once instead of on each stored record: a change to it
/// (another status that must not move, a further field to clear, a second
/// clock-owed transition) is made in one place and every kind of registration
/// moves with it.
///
/// An implementor supplies only what the rule reads and writes on its own
/// record; [`Self::settle`] is the rule itself, and
/// [`ComAmazonawsTaxsettingsTaxsettingsState::settle_due`] drives it for every
/// stored registration.
pub trait TrnValidation {
    /// The status the validation of this registration's stored number reaches —
    /// see [`trn_validation_status`].
    fn validation_outcome(&self) -> &'static str;

    /// When this registration's validation falls due, `None` once it has
    /// settled.
    fn validation_due_at(&self) -> Option<Instant>;

    /// The same instant, to clear once the validation has settled.
    fn validation_due_at_mut(&mut self) -> &mut Option<Instant>;

    /// The registration's `TaxRegistrationStatus`.
    fn status_mut(&mut self) -> &mut String;

    /// Move this registration to the status its validation reached, if that
    /// validation was already due at `now`. Answers whether it moved.
    ///
    /// Clearing the due instant is what makes settling idempotent: a record that
    /// has settled never moves again, so a read cannot change what a later
    /// caller sees.
    fn settle(&mut self, now: Instant) -> bool {
        match self.validation_due_at() {
            Some(due) if due <= now => {
                let outcome = self.validation_outcome();
                *self.status_mut() = outcome.to_string();
                *self.validation_due_at_mut() = None;
                true
            }
            _ => false,
        }
    }
}

/// The `EntityExemptionAccountStatus` values, from the model's enum
/// (`None | Valid | Expired | Pending`), which is what `TaxExemption$status`
/// reports for one exemption of an account.
pub const EXEMPTION_STATUS_NONE: &str = "None";
/// See [`EXEMPTION_STATUS_NONE`]: the exemption is in force.
pub const EXEMPTION_STATUS_VALID: &str = "Valid";
/// See [`EXEMPTION_STATUS_NONE`]: the exemption's expiration date has passed.
pub const EXEMPTION_STATUS_EXPIRED: &str = "Expired";
/// See [`EXEMPTION_STATUS_NONE`]: the submitted exemption certificate has not
/// been reviewed yet.
pub const EXEMPTION_STATUS_PENDING: &str = "Pending";

/// The `HeritageStatus` values, from the model's enum (`OptIn | OptOut`).
pub const HERITAGE_STATUS_OPT_IN: &str = "OptIn";
/// See [`HERITAGE_STATUS_OPT_IN`].
pub const HERITAGE_STATUS_OPT_OUT: &str = "OptOut";

/// The election `GetTaxInheritance` reports for an account that has none
/// recorded.
///
/// `HeritageStatus` is a two-valued enum — an account either inherits its
/// parent's tax settings (`OptIn`) or does not (`OptOut`) — and the member is
/// modelled on the output of a `@readonly` operation that takes no input, so
/// there is no "unset" to report: an account that has not opted in is opted out.
pub const DEFAULT_HERITAGE_STATUS: &str = HERITAGE_STATUS_OPT_OUT;

/// The role the legal address stored with an account's tax registration plays
/// for that account: one of the model's `AddressRoleType` values
/// (`TaxAddress | BillingAddress | ContactAddress`).
///
/// The address a `TaxRegistrationEntry` carries is the address its TRN is
/// registered at, so the role it plays is the account's tax address — the role
/// the reported account metadata files it under. The other two values are a
/// billing and a contact address, which no operation this service models
/// accepts, so no stored address plays either role.
pub const LEGAL_ADDRESS_ROLE: &str = "TaxAddress";

/// The `TaxRegistrationType` values whose legal address AWS derives from the tax
/// number itself rather than from the request.
///
/// A request that submits a `legalAddress` for one of them is refused: the
/// address is not the caller's to supply. `PAN` is India's Permanent Account
/// Number, and the model's own `ValidationExceptionErrorCode` enum carries
/// `NonIndiaCustomerCanNotSetPAN`, so the type is India-only — the country it
/// belongs to is in [`TYPE_JURISDICTIONS`]. An entry of such a type that submits
/// no address is stored with the address derived
/// ([`derived_legal_address`]), because the member is one the service fills in.
///
/// Country rules that make a member REQUIRED (Saudi Arabia's `addressLine3`,
/// Turkey's `districtOrCounty`, Kenya's `personType`, …) are a different class:
/// they are the per-country business rules [`COUNTRY_RULES`] carries and
/// [`check_country_requirements`] applies. A member the API accepts only for
/// certain jurisdictions is a third class, in
/// [`ADDRESS_MEMBER_JURISDICTIONS`].
pub const TYPES_WITH_DERIVED_LEGAL_ADDRESS: &[&str] = &["PAN"];

/// The country each country-specific `TaxRegistrationType` value belongs to: the
/// `Address$countryCode` of that country, and the country's name as a refusal
/// states it.
///
/// A `TaxRegistrationEntry` carries no country member: the jurisdiction a
/// registration is held in is its `legalAddress.countryCode`. An entry that
/// carries no legal address therefore identifies its jurisdiction only through
/// its registration type, and only for the types the model ties to ONE country:
/// Brazil's `CNPJ` and `CPF` ("If you're setting a TRN in Brazil for the CNPJ tax
/// type …", "for CPF tax types Brazil, you must specify the legal address"),
/// India's `PAN` (the `ValidationExceptionErrorCode` value
/// `NonIndiaCustomerCanNotSetPAN`), and Malaysia's `NRIC` and `SST`
/// ("`RegistrationType` valid values are `NRIC` for individual, and TIN and sales
/// and service tax (SST) for Business"). `VAT`, `GST`, `TIN` and `NIP` are tied
/// to no single country by any sentence the model states, so an entry of one of
/// those carrying no address identifies no jurisdiction at all.
pub const TYPE_JURISDICTIONS: &[(&str, &str, &str)] = &[
    ("CNPJ", "BR", "Brazil"),
    ("CPF", "BR", "Brazil"),
    ("PAN", "IN", "India"),
    ("NRIC", "MY", "Malaysia"),
    ("SST", "MY", "Malaysia"),
];

/// The jurisdictions whose registration legal NAME the service fills in from the
/// tax registration number, so an entry held in one need not carry it.
///
/// `TaxRegistrationEntry$legalName` is not `@required`, and the model states
/// where it may be omitted: "If you're setting a TRN in Brazil, you don't need to
/// specify the legal name. For TRNs in other countries, you must specify the
/// legal name", plus the operation's own Mexico note — "You do not need to
/// provide address and legal name. These will be populated based on your tax
/// registration number."
pub const COUNTRIES_WITH_DERIVED_LEGAL_NAME: &[&str] = &["BR", "MX"];

/// The jurisdictions whose registration legal ADDRESS the service fills in from
/// the tax registration number, each with the registration types the carve-out is
/// documented for — an EMPTY type list meaning every type of that country.
///
/// `TaxRegistrationEntry$legalAddress` is not `@required`, and the model states
/// where it may be omitted: "If you're setting a TRN in Brazil for the CNPJ tax
/// type, you don't need to specify the legal address. For TRNs in other countries
/// and for CPF tax types Brazil, you must specify the legal address", plus the
/// operation's Mexico note, which carves out every Mexican registration. The
/// per-TYPE carve-out is [`TYPES_WITH_DERIVED_LEGAL_ADDRESS`].
///
/// The Mexican row is the carve-out as the documentation states it, and it is
/// REACHED only by an entry that identifies Mexico some other way: the country an
/// entry with no legal address identifies comes from [`TYPE_JURISDICTIONS`], and
/// no `TaxRegistrationType` belongs to Mexico, so today a Mexican entry that
/// carries no address at all names no country and is refused for the jurisdiction
/// ([`MISSING_LEGAL_ADDRESS_JURISDICTION`]) rather than admitted by this row. The
/// row stays because it is what the service documents, and a later fixer whose
/// request carries the account's country has the rule already written down.
pub const COUNTRIES_WITH_DERIVED_LEGAL_ADDRESS: &[(&str, &[&str])] =
    &[("BR", &["CNPJ"]), ("MX", &[])];

/// The value `Address$addressLine1` and `Address$city` declare as their default
/// in the model
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#Address")` ->
/// both carry `{"default": "Unknown", "addedDefault": true}`).
///
/// `@addedDefault` keeps each member nullable on the wire, so a request may omit
/// it; the value the service substitutes for the omission is this one. See
/// [`address_with_declared_defaults`], which applies it.
pub const ADDRESS_MEMBER_DEFAULT: &str = "Unknown";

/// What a stored registration reports for a member the real service fills in
/// from the tax registration number, where the entry left it out.
///
/// NOT a derived value, and not the model's rule for this member: filling one in
/// means reading the account's registration back from the tax authority's
/// registry, which this emulator has none of, so the LOOKUP is knowingly not
/// emulated and
/// the member says "not known" instead of carrying a legal name or a postcode
/// nothing produced. A later fixer holding a source for the real value fills it
/// in here, and nothing else changes.
///
/// The value itself is [`ADDRESS_MEMBER_DEFAULT`], the one the model declares for
/// an address member whose value is unknown, extended — as this emulator's own
/// choice — to the members with no modelled default (`postalCode`, and
/// `legalName`), which cannot be left empty: both are `@required` where they are
/// reported and both forbid a blank value.
pub const DERIVED_MEMBER_UNKNOWN: &str = ADDRESS_MEMBER_DEFAULT;

/// The refusal of an entry whose jurisdiction requires a legal name and that
/// carries none, stating the rule the way the model does ("For TRNs in other
/// countries, you must specify the legal name").
pub const MISSING_LEGAL_NAME: &str = "You must specify the legalName for TRNs in this country.";

/// The refusal of an entry that carries no legal address and whose jurisdiction
/// is one the service does not fill the address in for.
pub const MISSING_LEGAL_ADDRESS: &str =
    "You must specify the legalAddress for TRNs in this country.";

/// The refusal of an entry that identifies no jurisdiction at all: it carries no
/// legal address, and its registration type belongs to no one country, so the
/// country the address would be filled in for can be neither read from the
/// request nor derived from the type. A different sentence from
/// [`MISSING_LEGAL_ADDRESS`] because a different thing is wrong: there is no
/// country whose rule could be stated.
pub const MISSING_LEGAL_ADDRESS_JURISDICTION: &str =
    "You must specify the legalAddress, which states the country your TRN is registered in.";

/// The jurisdictions this emulator stores a SUPPLEMENTAL TRN for.
///
/// `PutSupplementalTaxRegistration` accepts an entry only for a country in this
/// set and refuses every other one with the modelled `ValidationException`
/// ([`SUPPLEMENTAL_COUNTRY_NOT_SUPPORTED`]), because the real service does:
/// supplemental registration is supported in a bounded set of jurisdictions and
/// the rest take a 400.
///
/// WHERE THIS LIST COMES FROM: OBSERVED BEHAVIOUR, not the model. The model is
/// silent — it constrains `SupplementalTaxRegistrationEntry$address$countryCode`
/// to any two letters, makes `VAT` the one `SupplementalTaxRegistrationType`,
/// and the operation's whole documentation is "Stores supplemental tax
/// registration for a single account" — so model silence is the reason the
/// boundary cannot be read off the model, NOT evidence that there is none. The
/// evidence that there is one, and what it is, is the recorded unit
/// `taxsettings.supplemental-tax-registration-unsupported-country-rejected`: an
/// entry for France and one for the United Kingdom are each refused 400
/// `ValidationException` with that message, while a Swiss entry clears country
/// validation and fails only on account state (a `ConflictException`). So this
/// set holds every jurisdiction with positive evidence of being admitted —
/// Switzerland, whose VAT/MWST number is held with the Swiss authority
/// alongside the account's primary TRN — and a country with neither kind of
/// evidence takes the refusal the observed ones take.
///
/// That asymmetry is deliberate: a refusal is a modelled outcome a client
/// handles, while reporting a stored registration for a jurisdiction the service
/// refuses is a fabricated success, and the recording shows the refusal reaches
/// countries (the United Kingdom among them) that no model constraint excludes.
/// A later fixer holding a recording of another jurisdiction ADMITTED adds its
/// row here, and nothing else changes.
pub const SUPPLEMENTAL_TRN_COUNTRIES: &[(&str, &str)] = &[("CH", "Switzerland")];

/// How a supplemental TRN for a jurisdiction outside
/// [`SUPPLEMENTAL_TRN_COUNTRIES`] is refused, worded as the observed refusal of
/// that class words it. The sentence names no country and no registration, so
/// nothing of one request leaks into another's answer.
pub const SUPPLEMENTAL_COUNTRY_NOT_SUPPORTED: &str =
    "Tax registration for country is not supported.";

/// The jurisdictions whose stored tax registration this API cannot remove,
/// paired with the country's name as the refusal states it.
///
/// Both delete operations carve the same jurisdiction out in their own
/// documentation: "This API operation can't be used to delete your tax
/// registration in Brazil. Use the Payment preferences page in the Billing and
/// Cost Management console instead."
/// (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#DeleteTaxRegistration")`,
/// and the same note on `BatchDeleteTaxRegistration`). A registration held in
/// one of these jurisdictions is therefore removable only outside the API, so a
/// delete aimed at one is refused and the record stays readable.
///
/// The jurisdiction is the one the registration is HELD in — its stored
/// `legalAddress.countryCode`, the same key [`COUNTRY_RULES`] is applied by —
/// not the `TaxRegistrationType` the entry was submitted with: the carve-out is
/// about the country, and a country admits more than one type.
///
/// A record no API call can remove still costs a slot of the
/// [`MAX_STORED_REGISTRATIONS`] the store keeps resident, which is what bounds
/// the store; the bound is unchanged by this list.
pub const COUNTRIES_WITH_NON_API_REGISTRATION_DELETE: &[(&str, &str)] = &[("BR", "Brazil")];

/// How a write of the account's tax-inheritance election is refused when the
/// caller is not the management account of its organization, worded as the
/// observed refusal of that class words it.
///
/// The sentence is fixed text: it names no account, no election and no limit, so
/// nothing of one request leaks into another's answer and every refused caller
/// is told the same thing.
pub const TAX_INHERITANCE_MANAGEMENT_ACCOUNT_ONLY: &str =
    "Only AWS orgs management account is allowed to set tax inheritance.";

/// `ValidationException.errorCode` values, from the model's
/// `ValidationExceptionErrorCode` enum
/// (`MalformedToken | ExpiredToken | InvalidToken | FieldValidationFailed |
/// MissingInput | NonIndiaCustomerCanNotSetPAN | GSTExistenceBlockSetPAN`).
pub const ERROR_CODE_MISSING_INPUT: &str = "MissingInput";
/// See [`ERROR_CODE_MISSING_INPUT`].
pub const ERROR_CODE_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
/// See [`ERROR_CODE_MISSING_INPUT`].
pub const ERROR_CODE_MALFORMED_TOKEN: &str = "MalformedToken";
/// See [`ERROR_CODE_MISSING_INPUT`].
pub const ERROR_CODE_INVALID_TOKEN: &str = "InvalidToken";
/// See [`ERROR_CODE_MISSING_INPUT`]. The code a token of this service's own is
/// refused with once the validity window the model publishes for it has passed —
/// see [`TAX_DOCUMENT_TOKEN_VALIDITY`].
pub const ERROR_CODE_EXPIRED_TOKEN: &str = "ExpiredToken";
/// See [`ERROR_CODE_MISSING_INPUT`]. The code a registration type that belongs
/// to one country is refused with when the registration is held somewhere else
/// — see [`TYPE_OUTSIDE_ITS_COUNTRY_CODES`].
pub const ERROR_CODE_NON_INDIA_CUSTOMER_CAN_NOT_SET_PAN: &str = "NonIndiaCustomerCanNotSetPAN";
/// See [`ERROR_CODE_MISSING_INPUT`]. The code a registration type is refused
/// with when the account already holds a registration whose type blocks it —
/// see [`TYPES_BLOCKED_BY_A_STORED_TYPE`].
pub const ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN: &str = "GSTExistenceBlockSetPAN";

/// The `ConflictException$errorCode` this service publishes: the code every
/// refusal of the conflict class carries.
///
/// `ConflictException` is the one refusal this service answers when "the input is
/// creating conflict with the given state"
/// (`smithy_query(query="shape", shape="ConflictException")`), and that class has
/// one published code, which every conflict of this service carries whichever
/// state it conflicted with.
///
/// The `message` is NOT one per class: a refusal carries the fixed text the
/// service words for the condition it is refusing — the general state conflict
/// ([`STATE_CONFLICT_MESSAGE`]) or the pending-TRN rule
/// ([`PENDING_REGISTRATION_CONFLICT_MESSAGE`]) — so `errorCode` is what a caller
/// branches on and the message is what tells it WHICH state conflicted.
pub const ERROR_CODE_GENERAL_CONFLICT: &str = "GeneralConflict";

/// The `message` a `ConflictException` of this service carries when the state it
/// conflicted with is not one the service words its own sentence for — the fixed
/// text of the general conflict condition, as a recorded 409 of this service
/// words it. See [`ERROR_CODE_GENERAL_CONFLICT`] and [`state_conflict`].
pub const STATE_CONFLICT_MESSAGE: &str = "Operation cannot be performed due to conflict in current state.";

/// The `message` a write of an account's tax registration is refused with while
/// that account's stored TRN is still awaiting validation — the fixed text this
/// service words for the pending-TRN condition, as a recorded 409 of
/// `PutTaxRegistration` words it. See [`pending_registration_conflict`].
///
/// This is the condition the write's own documentation states — "You can't set a
/// TRN if there's a pending TRN. You'll need to delete the pending TRN first"
/// (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`)
/// — and the sentence the refusal carries is the service's own wording of it,
/// not that documentation sentence. Entirely fixed text: it names no account, no
/// registration and no jurisdiction, because the request it refused already
/// names them.
pub const PENDING_REGISTRATION_CONFLICT_MESSAGE: &str =
    "There is a pending TRN for the account. Please delete that first and retry the request.";

/// The `message` the per-account row of `BatchDeleteTaxRegistration` carries for
/// an account that holds no tax registration: the token naming the class of the
/// refusal, as a recorded row of that class words it.
///
/// The single-account 404 carries that exception's own sentence instead
/// ([`no_tax_registration`]): the two channels word this class differently, and
/// each is recorded.
pub const TAX_REGISTRATION_NOT_FOUND: &str = "TaxRegistrationNotFound";

/// The `message` this service answers with for a submitted tax registration
/// number that is not one its jurisdiction's authority issues: the token naming
/// the class of the refusal, as a recorded per-account row of
/// `BatchPutTaxRegistration` words it. See [`check_registration_number`].
pub const INVALID_TAX_REGISTRATION_NUMBER: &str = "TCS_INVALID_TAX_REGISTRATION_NUMBER";

/// The `message` `PutTaxExemption` refuses a submission with when the
/// jurisdiction it names is not one the exemption type it names applies in — the
/// fixed text of that refusal class. See [`check_exemption_authority`].
pub const EXEMPTION_AUTHORITY_NOT_SUPPORTED: &str =
    "Provided tax exemption authority is not supported for the tax exemption type.";

/// The `ValidationException$errorCode` this service answers with when a request
/// sets a `TaxRegistrationType` that belongs to ONE country
/// ([`TYPE_JURISDICTIONS`]) for a registration held in a different country, per
/// type.
///
/// `errorCode` is the discriminator a caller branches on instead of parsing the
/// message, and the model's `ValidationExceptionErrorCode` enum publishes one
/// value per refusal this service answers; `NonIndiaCustomerCanNotSetPAN` is the
/// value published for the type India's Permanent Account Number is, so a
/// request that sets that type for a registration held outside that country is
/// the refusal it names.
///
/// A type whose jurisdiction row exists but that has NO row here is not refused
/// for its jurisdiction: the published enum is the evidence that the service
/// enforces the rule for a type, and it publishes a value for this one only. A
/// later fixer holding a recording of another type refused the same way adds its
/// row, and nothing else changes.
///
/// WHAT DECIDES THE COUNTRY: the customer's own country, which this rule reads
/// off the legal address the entry carries. A type whose address the service
/// derives from the tax number ([`TYPES_WITH_DERIVED_LEGAL_ADDRESS`]) admits no
/// submitted address, so an entry of such a type carries no address to read and
/// the refusal below is not reached for it: submitting one is the field
/// violation [`check_registration_type_members`] answers, and omitting one
/// names only the type's own country. Every type this constant names is such a
/// type today, so the rule waits for a jurisdiction-bound type whose address the
/// request supplies, or for a later fixer holding the customer's own country
/// from somewhere other than the entry.
pub const TYPE_OUTSIDE_ITS_COUNTRY_CODES: &[(&str, &str)] =
    &[("PAN", ERROR_CODE_NON_INDIA_CUSTOMER_CAN_NOT_SET_PAN)];

/// The `TaxRegistrationType` a request may not set while the account's stored
/// registration is of a given type: the stored type, the submitted type it
/// blocks, and the `ValidationException$errorCode` the refusal carries.
///
/// Read the same way as [`TYPE_OUTSIDE_ITS_COUNTRY_CODES`]: the model's
/// `ValidationExceptionErrorCode` enum publishes `GSTExistenceBlockSetPAN`, so
/// the existence of a stored registration of the goods-and-services-tax type
/// blocks a write that sets the Permanent Account Number type for that account,
/// and the account's stored record decides it. A pair the enum does not publish
/// is not blocked, so writing any other type over any other stored type stays a
/// plain replacement.
pub const TYPES_BLOCKED_BY_A_STORED_TYPE: &[(&str, &str, &str)] =
    &[("GST", "PAN", ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN)];

/// How a refusal of a registration type that cannot be set for this customer
/// states the rule: the type, and the one country whose customers may set it.
///
/// AWS publishes no message for the refusal, only its `errorCode`, so the
/// sentence is this emulator's own; the code is the requirement and the message
/// states the same rule in words.
pub fn type_outside_its_country_message(registration_type: &str, country: &str) -> String {
    format!("Only a customer in {country} can set a {registration_type} tax registration.")
}

/// How a refusal of a registration type the account's stored registration blocks
/// states the rule: the type the request set, and the stored type that blocks
/// it. The wording is this emulator's own, for the reason
/// [`type_outside_its_country_message`] gives.
pub fn type_blocked_by_stored_type_message(registration_type: &str, stored_type: &str) -> String {
    format!(
        "You can't set a {registration_type} tax registration while there's a {stored_type} tax \
         registration for this account. Delete the {stored_type} tax registration first."
    )
}

/// The page a listing builds when a request names no `maxResults`, for an
/// operation whose own documentation states no default.
///
/// The bounds are per operation (see [`PageBounds`]): where the operation's
/// documentation states them, they are the service's, and where it states none
/// this is the emulator's own, so a request that names no page size still gets a
/// bounded page and a cursor to continue from rather than the whole store in one
/// response.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The largest page a listing builds whose own documentation states no cap,
/// whatever `maxResults` asks for.
///
/// Equal to the ceiling of the shared `MaxResults` range (1..=1000), so it never
/// truncates a page size the model admits; it pins that the SERVER, not the
/// request, owns the upper bound on one response's size and on the work done
/// while the state lock is held.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// The most tax registrations this emulator keeps resident.
///
/// `accountId` is caller-supplied (any 12 digits), so without a bound a caller
/// could grow the store by one record per request and there would be no release
/// path but `DeleteTaxRegistration`. A write that would add a record past this
/// bound is refused with `PutTaxRegistration`'s modelled `ConflictException`;
/// rewriting an account that already has a record is always allowed, because it
/// adds nothing.
pub const MAX_STORED_REGISTRATIONS: usize = 1_000;

/// The most supplemental tax registrations this emulator keeps resident.
///
/// `SupplementalTaxRegistrationEntry$registrationId` is caller-supplied, so
/// without a bound a caller could grow the store by one record per request. A
/// write that would ADD a record past this bound is refused with
/// `PutSupplementalTaxRegistration`'s modelled `ConflictException`; rewriting a
/// supplemental TRN the account already has is always allowed, because it adds
/// nothing.
pub const MAX_STORED_SUPPLEMENTAL_REGISTRATIONS: usize = 1_000;

/// The page-size bounds of ONE listing operation: the page it builds when the
/// request names no `maxResults`, and the largest page it builds whatever
/// `maxResults` asks for.
///
/// Bounds belong to the operation, not to the service: the listings of one
/// service need not share them, and the range of the `MaxResults` shape they
/// share (1..=1000) is what the WIRE admits, not what any one of them serves in
/// a single page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBounds {
    /// The page size used when the request names no `maxResults`.
    pub default_size: usize,
    /// The largest page built, whatever `maxResults` asks for.
    pub max_size: usize,
}

impl PageBounds {
    /// Bounds with this default page size and this ceiling.
    pub const fn new(default_size: usize, max_size: usize) -> Self {
        Self {
            default_size,
            max_size,
        }
    }

    /// The page size a request selects under these bounds: what it asked for,
    /// else [`Self::default_size`], never more than [`Self::max_size`] and never
    /// zero.
    ///
    /// A `maxResults` outside the model's 1..=1000 range never reaches here: the
    /// boundary validator refuses it with a `ValidationException` before the
    /// handler runs, as `tests/all/tax_exemptions.rs` pins on each bound and one
    /// past it. The clamp is therefore the SERVER's ceiling on a page, which a
    /// larger `maxResults` does not raise, not a coercion of a value a client
    /// could not have sent.
    pub fn page_size(self, max_results: Option<i32>) -> usize {
        max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(self.default_size)
            .clamp(1, self.max_size)
    }
}

/// `ListTaxRegistrations`' page bounds, which its own documentation states: the
/// operation "can be used to retrieve up to 100 accounts' tax registrations in
/// one call (default 50)". So a request naming no `maxResults` is served 50
/// entries and a cursor, and one naming more than 100 — up to the 1000 the
/// shared `MaxResults` shape admits on the wire — is still served at most 100
/// entries and a cursor.
pub const TAX_REGISTRATIONS_PAGE_BOUNDS: PageBounds = PageBounds::new(50, 100);

/// `ListTaxExemptions`' page bounds. Its documentation states neither a default
/// nor a cap, so these are the emulator's own ([`DEFAULT_PAGE_SIZE`] /
/// [`MAX_PAGE_SIZE`]).
pub const TAX_EXEMPTIONS_PAGE_BOUNDS: PageBounds =
    PageBounds::new(DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE);

/// `ListSupplementalTaxRegistrations`' page bounds. Documented like
/// [`TAX_EXEMPTIONS_PAGE_BOUNDS`]: the operation states neither a default nor a
/// cap, so these are the emulator's own.
pub const SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS: PageBounds =
    PageBounds::new(DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE);

/// Per-service state for com-amazonaws-taxsettings-taxsettings. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsTaxsettingsTaxsettingsState {
    /// Every account's tax registration, keyed by its 12-digit account id.
    ///
    /// A `BTreeMap` so `ListTaxRegistrations` enumerates in one stable order and
    /// a page token can name the key the previous page stopped after.
    pub registrations: BTreeMap<String, StoredTaxRegistration>,
    /// The tax-inheritance election (`OptIn` / `OptOut`) recorded for the
    /// account. One value, because neither `PutTaxInheritance` nor
    /// `GetTaxInheritance` takes an account: both address the caller's own, and
    /// this emulator presents one account.
    ///
    /// `None` until `PutTaxInheritance` records one, for as long as which
    /// `GetTaxInheritance` reports [`DEFAULT_HERITAGE_STATUS`]; a write replaces
    /// the value, because the account has exactly one election.
    ///
    /// The election is an organization-level one — whether an organization's
    /// member accounts inherit the management account's tax settings — so only
    /// the management account of the organization may record it: the standing
    /// [`organization::caller_is_management_account`] reads from the control
    /// plane that owns it, which the write consults before it touches this
    /// field.
    pub tax_inheritance: Option<String>,
    /// The account's supplemental tax registrations, keyed by the `authorityId`
    /// this service minted for each when it was first stored.
    ///
    /// A `BTreeMap` so `ListSupplementalTaxRegistrations` enumerates in one
    /// stable order and a page token can name the key the previous page stopped
    /// after. One map for the whole store, because neither
    /// `PutSupplementalTaxRegistration` nor `ListSupplementalTaxRegistrations`
    /// takes an account: both address the caller's own, and this emulator
    /// presents one account.
    pub supplemental_registrations: BTreeMap<String, StoredSupplementalRegistration>,
    /// How many `authorityId`s this service has minted. The next one is derived
    /// from it, so every supplemental TRN gets its own and a deleted one's id is
    /// never handed out again.
    pub minted_authority_ids: u64,
    /// The tax exemptions recorded for each account, keyed by its 12-digit
    /// account id.
    ///
    /// A `BTreeMap` so a listing enumerates in one stable order and a page token
    /// can name the key the previous page stopped after, the same way
    /// [`Self::registrations`] does. An account with no exemption has no entry
    /// here — never an empty vector — so "this service holds exemptions for the
    /// account" is one question with one answer. Within an account the records
    /// keep the order they were first recorded in, so every reader reports one
    /// account's exemptions one way.
    ///
    /// `PutTaxExemption` is the writer AWS models for this map (it submits an
    /// exemption certificate for review) and writes it through
    /// [`Self::store_exemptions`].
    pub exemptions: BTreeMap<String, Vec<StoredTaxExemption>>,
    /// How many support cases `PutTaxExemption` has opened. The cases
    /// themselves live in the AWS Support store, which owns them and mints their
    /// ids ([`support_case::open_exemption_case`]); this is this service's tally
    /// of the submissions it applied, written by
    /// [`Self::count_opened_exemption_case`].
    pub minted_exemption_cases: u64,
    /// The highest registration revision this service has already minted
    /// document access tokens under for a registration it no longer holds.
    ///
    /// A revision is the coordinate a `taxDocumentAccessToken` locates ONE write
    /// of ONE account's registration by, so no revision may be handed out twice:
    /// a token the service stopped honouring must never start naming a document
    /// again. A record carries its own revision while the store holds it, but
    /// removing the record would take that count with it, so
    /// [`Self::delete_registration`] keeps its high-water mark here and the next
    /// write for an account the store holds no record for starts past it.
    ///
    /// One number for the whole store rather than a tally per account, so
    /// remembering what was retired costs nothing per account removed — a
    /// revision past every retired one is past the retired ones of the account
    /// being written, which is what the rule needs.
    pub retired_registration_revision: u64,
}

impl ComAmazonawsTaxsettingsTaxsettingsState {
    /// Materialise every TRN validation the clock has already made due at `now`.
    ///
    /// Each stored registration carries the instant its validation falls due,
    /// which was fixed by the write that started it, so the status a read reports
    /// is the one the registration had at that instant whether or not anybody
    /// called: this settles what is owed, it never starts anything. Every read of
    /// a registration goes through here first, so `GetTaxRegistration` and
    /// `ListTaxRegistrations` cannot report one record in two different states.
    ///
    /// Every kind of stored registration settles by the one rule
    /// ([`TrnValidation::settle`]), so an account's own TRN and a supplemental
    /// one are materialised the same way.
    pub fn settle_due(&mut self, now: Instant) {
        for stored in self.registrations.values_mut() {
            stored.settle(now);
        }
        for stored in self.supplemental_registrations.values_mut() {
            stored.settle(now);
        }
    }

    /// Store `entry` as `account_id`'s tax registration, answering the status it
    /// was stored with.
    ///
    /// The one place a `TaxRegistrationEntry` becomes a stored record, so
    /// `PutTaxRegistration` and the per-account loop of
    /// `BatchPutTaxRegistration` cannot write one entry two different ways: the
    /// write replaces the account's record and moves its revision on, past every
    /// revision this account's documents have already been minted under — the
    /// record's own, or, when the store holds none, the retired high-water mark
    /// [`Self::delete_registration`] keeps.
    ///
    /// Two refusals, both `ConflictException`s — each caller renders one on its
    /// own channel, a `ConflictException` response or a per-account row of the
    /// batch's `errors` list:
    ///
    /// * the account's stored TRN is still outstanding: both writers document
    ///   "You can't set a TRN if there's a pending TRN. You'll need to delete the
    ///   pending TRN first", so a record in [`STATUS_PENDING`] is NOT replaced —
    ///   it is left exactly as it was and the write is refused, carrying the
    ///   sentence this service words for that condition
    ///   ([`pending_registration_conflict`]). The validation the clock had
    ///   already made due at `written_at` is materialised first, because it was
    ///   owed at that instant whether or not anybody wrote: a TRN whose
    ///   validation has run is not an outstanding one, so it may be rewritten.
    /// * `accountId` is caller-supplied, so a write that would ADD a record is
    ///   refused once the store holds as many as this emulator keeps resident
    ///   ([`MAX_STORED_REGISTRATIONS`]) while rewriting an account that already
    ///   has one adds nothing. That is a conflict with the store's own state,
    ///   which this service words no sentence of its own for, so it carries the
    ///   general one ([`state_conflict`]).
    pub fn store_registration(
        &mut self,
        account_id: &str,
        entry: &TaxRegistrationEntry,
        written_at: Instant,
    ) -> Result<String, ConflictException> {
        let previous_revision = match self.registrations.get_mut(account_id) {
            Some(stored) => {
                // Settle what the clock owed at this instant before judging the
                // record's status, so "pending" means a validation that is
                // genuinely still outstanding rather than one nobody has read
                // since it fell due.
                stored.settle(written_at);
                if stored.status == STATUS_PENDING {
                    return Err(pending_registration_conflict());
                }
                Some(stored.revision)
            }
            None => None,
        };
        if previous_revision.is_none() && self.registrations.len() >= MAX_STORED_REGISTRATIONS {
            return Err(state_conflict());
        }
        // Every write gets a revision past every one this account's documents
        // have already been minted under — the record's own while the store
        // holds it, and the retired high-water mark once it does not, because a
        // removed record's revisions are gone from the store but their tokens
        // are not gone from the callers that hold them.
        let revision = previous_revision
            .unwrap_or(self.retired_registration_revision)
            .saturating_add(1);
        let stored = StoredTaxRegistration::from_entry(entry, account_id, revision, written_at);
        let status = stored.status.clone();
        self.registrations.insert(account_id.to_string(), stored);
        Ok(status)
    }

    /// Remove `account_id`'s tax registration, or answer the refusal a delete of
    /// that one account is refused with.
    ///
    /// The one place a stored record STOPS being one, so
    /// `DeleteTaxRegistration` and the per-account loop of
    /// `BatchDeleteTaxRegistration` cannot decide one record's deletion two
    /// different ways — the delete-side counterpart of
    /// [`Self::store_registration`]. The whole decision lives here: which
    /// refusal applies, and the removal itself.
    ///
    /// Two refusals, each caller rendering one on its own channel — a modelled
    /// error response, or a per-account row of the batch's `errors` list (see
    /// [`RegistrationDeleteRefusal`]):
    ///
    /// * the account holds no record at all, so there is nothing to delete
    ///   whatever jurisdiction the carve-out is about
    ///   ([`no_tax_registration`], the 404 class);
    /// * the record is held in a jurisdiction this API cannot remove one from
    ///   ([`check_registration_deletable`], the 400 class).
    ///
    /// A refused record is left exactly as it was, so every later read still
    /// reports it.
    ///
    /// A removed record's revision is retired into
    /// [`Self::retired_registration_revision`] rather than discarded with it, so
    /// the document access tokens its documents had are never minted again.
    pub fn delete_registration(
        &mut self,
        account_id: &str,
    ) -> Result<(), RegistrationDeleteRefusal> {
        let Some(stored) = self.registrations.get(account_id) else {
            return Err(RegistrationDeleteRefusal::NotFound(no_tax_registration()));
        };
        check_registration_deletable(stored).map_err(RegistrationDeleteRefusal::NotDeletable)?;
        if let Some(removed) = self.registrations.remove(account_id) {
            self.retired_registration_revision =
                self.retired_registration_revision.max(removed.revision);
        }
        Ok(())
    }

    /// The tax document the submitted `TaxDocumentMetadata` names, or the
    /// `ValidationException` that metadata is refused with.
    ///
    /// The metadata must name a document this service HOLDS, and the token that
    /// names it must still be inside the validity window the model publishes for
    /// it — the two ways the model says a `taxDocumentAccessToken` stops being
    /// valid:
    ///
    /// * it locates one document of ONE revision of ONE account's registration,
    ///   so a token minted for a registration that has since been rewritten (a
    ///   later revision) or deleted names nothing ([`unknown_tax_document`]).
    ///   The stored token is compared whole, so a metadata whose
    ///   `taxDocumentName` disagrees with the document its token locates names
    ///   nothing either.
    /// * it stops being valid [`TAX_DOCUMENT_TOKEN_VALIDITY`] after the write
    ///   that minted it, whatever has happened to the registration since, so a
    ///   token presented past that window is refused
    ///   ([`expired_tax_document_token`]) even though the document it names is
    ///   still held.
    ///
    /// The window is judged only for a token that still locates a document: a
    /// retired token names nothing whatever its age, so it is refused as the
    /// token of a registration that is gone rather than as a stale one.
    ///
    /// A reporting call: resolving a token reads the registration documents a
    /// write recorded, and the window is computed from `now` rather than
    /// recorded, so a caller that asks for the same document twice inside the
    /// window is answered the same way twice.
    pub fn held_tax_document(
        &self,
        metadata: &TaxDocumentMetadata,
        now: Instant,
    ) -> Result<HeldTaxDocument, ValidationException> {
        let named = decode_document_access_token(&metadata.tax_document_access_token)?;
        let Some(stored) = self
            .registrations
            .get(&named.account_id)
            .filter(|stored| stored.revision == named.revision)
            .filter(|stored| {
                stored
                    .tax_document_metadatas
                    .get(named.index)
                    .is_some_and(|document| {
                        document.tax_document_access_token == metadata.tax_document_access_token
                            && document.tax_document_name == metadata.tax_document_name
                    })
            })
        else {
            return Err(unknown_tax_document());
        };
        if stored.document_tokens_valid_at(now) {
            Ok(named)
        } else {
            Err(expired_tax_document_token())
        }
    }

    /// Where the documents of the registration this store holds for `account_id`
    /// are kept: that registration's revision, and how many documents it carries.
    ///
    /// The coordinates [`documents::document_object_key`] derives a document's
    /// object key from, so a caller that is about to replace or remove the record
    /// can retire the copies of the documents it is retiring the access tokens of
    /// ([`documents::release_documents`]) — read HERE, from the record itself,
    /// rather than recomputed from anything a request carried.
    ///
    /// `None` when the store holds no registration for the account, so there is
    /// nothing of its to retire.
    ///
    /// A pure read: a record's revision and its document list are fixed by the
    /// write that stored it and no validation falling due changes either.
    pub fn held_document_coordinates(&self, account_id: &str) -> Option<(u64, usize)> {
        self.registrations
            .get(account_id)
            .map(|stored| (stored.revision, stored.tax_document_metadatas.len()))
    }

    /// Refuse a write whose registration type the account's STORED registration
    /// blocks — see [`TYPES_BLOCKED_BY_A_STORED_TYPE`].
    ///
    /// The one place both writers judge an entry against what the account
    /// already holds, so `PutTaxRegistration` and the per-account loop of
    /// `BatchPutTaxRegistration` cannot judge one account's state two different
    /// ways. It answers the modelled `ValidationException`, carrying the
    /// `errorCode` the model publishes for the pair, and is applied BEFORE
    /// [`Self::store_registration`]: an account whose stored type blocks the
    /// submitted one keeps its record whatever status that record is in, so the
    /// caller is told which type blocks the write rather than that the record is
    /// outstanding.
    ///
    /// A reporting call, despite taking `&self` on a store whose records settle:
    /// the type of a stored registration is fixed when it is written and no
    /// validation falling due changes it, so nothing has to be materialised to
    /// read it.
    pub fn check_registration_type_against_stored(
        &self,
        account_id: &str,
        entry: &TaxRegistrationEntry,
    ) -> Result<(), ValidationException> {
        let Some(stored) = self.registrations.get(account_id) else {
            return Ok(());
        };
        let Some(error_code) =
            type_blocked_by_stored_type(&stored.registration_type, &entry.registration_type)
        else {
            return Ok(());
        };
        Err(validation(
            error_code,
            "taxRegistrationEntry.registrationType",
            type_blocked_by_stored_type_message(
                &entry.registration_type,
                &stored.registration_type,
            ),
        ))
    }

    /// Whether this service reports tax exemptions for `account_id` at all —
    /// that is, whether the account is one of those a caller of this service may
    /// read exemptions for.
    ///
    /// The exemption reads address "accounts listed in a consolidated billing
    /// family" (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#ListTaxExemptions")`),
    /// so the accounts they can answer for are the caller's own account and the
    /// accounts whose tax settings this service holds — the only accounts it is
    /// ever told about. Any other 12-digit id names an account outside that
    /// family, which this service holds no tax-settings resource for.
    ///
    /// Comparing a REQUEST-SUPPLIED id against [`::engine::emulator_account_id`]
    /// is a real discriminator (an id naming another account fails it), unlike
    /// comparing the CALLER against it, which every caller passes.
    ///
    /// A read that names such an account reports it on its own per-account
    /// channel — the batch read's `failedAccounts` — so the accounts named
    /// beside it are still reported.
    ///
    /// Decided by MEMBERSHIP of the one set those accounts are enumerated as
    /// ([`Self::exemption_report_accounts`]), so the listing that reports the
    /// family and the batch read that screens a named account against it cannot
    /// disagree about who is in it.
    pub fn reports_exemptions_for(&self, account_id: &str) -> bool {
        self.exemption_report_accounts().contains_key(account_id)
    }

    /// The `TaxExemptionDetails` the exemption reads report for `account_id` at
    /// `now`.
    ///
    /// The ONE projection both read paths use, so `ListTaxExemptions` and
    /// `BatchGetTaxExemptions` cannot describe one account two different ways.
    /// Every account these reads answer for ([`Self::reports_exemptions_for`])
    /// gets one, whether or not this service holds an exemption for it: the
    /// details say what is known about the account's exemptions, and "it holds
    /// none, and none is inherited" is such an answer.
    ///
    /// `taxExemptions` therefore carries the records this service holds and is
    /// EMPTY for an account holding none — the member is this read's payload, so
    /// an empty list says "it holds none" where an absent one would leave a
    /// client unable to tell that from a response that did not answer. The same
    /// reason reports `heritageObtainedDetails` ALWAYS: the model declares a
    /// boolean, documented as "the indicator if the tax exemption is inherited
    /// from the consolidated billing family management account", and a client
    /// branching on inheritance must be able to tell "not inherited" from "the
    /// service did not say". It is false for every account: no operation this
    /// service models records an exemption inherited from a family management
    /// account — the same reason `ListTaxRegistrations` reports no
    /// `taxInheritanceDetails` — so nothing is inherited, and the parent entity
    /// and the reason such an inheritance would be described by stay absent
    /// because there is no inheritance to describe.
    pub fn exemption_details(&self, account_id: &str, now: SystemTime) -> TaxExemptionDetails {
        TaxExemptionDetails {
            tax_exemptions: Some(
                self.exemptions
                    .get(account_id)
                    .map(|stored| {
                        stored
                            .iter()
                            .map(|exemption| exemption.to_tax_exemption(now))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            heritage_obtained_details: Some(false),
            heritage_obtained_parent_entity: None,
            heritage_obtained_reason: None,
        }
    }

    /// The accounts the exemption reads answer for, in account-id order: the
    /// caller's own account — which is in its own consolidated billing family
    /// whatever it holds — and every account whose tax settings this service
    /// holds, which are the only accounts it is ever told about.
    ///
    /// The ONE statement of that membership rule: `ListTaxExemptions`
    /// enumerates this set and [`Self::reports_exemptions_for`] asks it whether
    /// one account is in it, so the listing and the batch read cannot describe
    /// two different families.
    ///
    /// A map rather than a set so the listing pages over it through the one
    /// shared page builder ([`build_page`]), whose cursor names the account the
    /// previous page stopped after; the entries carry no value of their own
    /// because the details are projected from the store by
    /// [`Self::exemption_details`].
    pub fn exemption_report_accounts(&self) -> BTreeMap<String, ()> {
        let mut accounts: BTreeMap<String, ()> = BTreeMap::new();
        accounts.insert(::engine::emulator_account_id().to_string(), ());
        for account_id in self.registrations.keys().chain(self.exemptions.keys()) {
            accounts.insert(account_id.clone(), ());
        }
        accounts
    }

    /// The `authorityId` of the stored supplemental TRN with this type and
    /// registration id, if the account already has one.
    ///
    /// `SupplementalTaxRegistrationEntry` carries no `authorityId`, so the TRN
    /// itself — its type and its registration id — is the identity a write
    /// addresses: that is what makes `PutSupplementalTaxRegistration` "adding or
    /// updating" one registration rather than appending a record per call.
    ///
    /// Which means the number is compared as the NUMBER it is, not as the string
    /// one submission spelled it with: a registration number is one number
    /// however its holder punctuates and pads it, so the separators the
    /// authorities print it with and the surrounding whitespace the member's
    /// non-blank pattern admits are removed and its letters folded before the
    /// comparison — the form [`registration_number::normalised`] states, which is
    /// already the form every judgement this service makes about a submitted
    /// number is made in. Case and padding in the registration TYPE make no
    /// second TRN either, exactly as they make no second exemption
    /// ([`is_same_exemption`]), so what this store treats as one number of one
    /// type it treats as one record.
    pub fn find_supplemental_authority(
        &self,
        registration_type: &str,
        registration_id: &str,
    ) -> Option<String> {
        let submitted_number = registration_number::normalised(registration_id);
        self.supplemental_registrations
            .iter()
            .find(|(_, stored)| {
                stored
                    .registration_type
                    .trim()
                    .eq_ignore_ascii_case(registration_type.trim())
                    && registration_number::normalised(&stored.registration_id)
                        == submitted_number
            })
            .map(|(authority_id, _)| authority_id.clone())
    }

    /// Mint the `authorityId` of a supplemental TRN this account did not have
    /// yet.
    ///
    /// `authorityId` is SERVER-generated (no request member carries one) and the
    /// model constrains it only as a `GenericString` of 1..=200 characters, so
    /// AWS publishes no format for it: this mints a short opaque id from the
    /// service's own counter, unique per record and stable for the life of that
    /// record, which is every invariant the member has.
    pub fn mint_supplemental_authority_id(&mut self) -> String {
        self.minted_authority_ids = self.minted_authority_ids.saturating_add(1);
        format!("supplemental-trn-{:08}", self.minted_authority_ids)
    }

    /// Store `entry` as a supplemental tax registration of the account,
    /// answering the `authorityId` it is held under and the status it was stored
    /// with.
    ///
    /// The one place a `SupplementalTaxRegistrationEntry` becomes a stored
    /// record, beside [`Self::store_registration`] and [`Self::store_exemptions`]:
    /// the add-vs-replace question and the resident bound are decided here, with
    /// the insert that acts on them, so no writer of a supplemental TRN can
    /// decide them its own way.
    ///
    /// The entry carries no `authorityId`, so the TRN itself — its
    /// `registrationType` and `registrationId` — is the identity the write
    /// addresses ([`Self::find_supplemental_authority`]): writing a TRN the store
    /// already holds REPLACES that record under the id it was first stored with —
    /// held means the same number, whichever of its separator and padding forms
    /// the write spells it in — and a TRN the store does not hold yet gets a
    /// freshly minted one ([`Self::mint_supplemental_authority_id`]). The record
    /// then carries the members the LAST write submitted, that number included,
    /// as submitted.
    ///
    /// Three refusals, each the modelled `ConflictException` ([`state_conflict`]),
    /// and a refused write stores nothing and mints no id. All three carry the
    /// class's general sentence ([`STATE_CONFLICT_MESSAGE`]): the sentence this
    /// service words for the pending-TRN condition belongs to the write of an
    /// ACCOUNT's own registration ([`pending_registration_conflict`]), and a
    /// recorded 409 of this operation words its conflict with the general one.
    ///
    /// * a supplemental tax registration SUPPLEMENTS the account's own: the
    ///   account must therefore hold a tax registration whose validation has
    ///   already run, and a write for an account that holds none — or whose own
    ///   TRN is still outstanding — conflicts with the account's current state.
    ///   The validation the clock had already made due at `written_at` is
    ///   materialised first, because it was owed at that instant whether or not
    ///   anybody wrote.
    /// * the supplemental record the write addresses has a validation still
    ///   outstanding: the service documents the rule for an account's TRN — "You
    ///   can't set a TRN if there's a pending TRN. You'll need to delete the
    ///   pending TRN first"
    ///   (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`)
    ///   — and a supplemental TRN is a tax registration of the same account, held
    ///   in the same store and validated by the same clock-owed rule
    ///   ([`TrnValidation`]), so a record in [`STATUS_PENDING`] is NOT replaced:
    ///   it is left exactly as it was.
    /// * `registrationId` is caller-supplied, so a write that would ADD a record
    ///   is refused once the store holds as many supplemental registrations as
    ///   this emulator keeps resident ([`MAX_STORED_SUPPLEMENTAL_REGISTRATIONS`])
    ///   while rewriting a TRN the store already holds adds nothing.
    pub fn store_supplemental_registration(
        &mut self,
        entry: &SupplementalTaxRegistrationEntry,
        written_at: Instant,
    ) -> Result<(String, String), ConflictException> {
        // A supplemental registration supplements the account's own, so the
        // account's state must carry one whose validation has already run.
        let own_account = ::engine::emulator_account_id().to_string();
        match self.registrations.get_mut(&own_account) {
            Some(stored) => {
                stored.settle(written_at);
                if stored.status != STATUS_VERIFIED {
                    return Err(state_conflict());
                }
            }
            None => return Err(state_conflict()),
        }
        let authority_id = match self
            .find_supplemental_authority(&entry.registration_type, &entry.registration_id)
        {
            Some(authority_id) => {
                if let Some(held) = self.supplemental_registrations.get_mut(&authority_id) {
                    // Settle what the clock owed at this instant before judging
                    // the record's status, so "outstanding" means a validation
                    // that is genuinely still running rather than one nobody has
                    // read since it fell due.
                    held.settle(written_at);
                    if held.status == STATUS_PENDING {
                        return Err(state_conflict());
                    }
                }
                authority_id
            }
            None => {
                if self.supplemental_registrations.len()
                    >= MAX_STORED_SUPPLEMENTAL_REGISTRATIONS
                {
                    return Err(state_conflict());
                }
                self.mint_supplemental_authority_id()
            }
        };
        let stored = StoredSupplementalRegistration::from_entry(entry, written_at);
        let status = stored.status.clone();
        self.supplemental_registrations
            .insert(authority_id.clone(), stored);
        Ok((authority_id, status))
    }

    /// Remove the supplemental tax registration `authority_id` names, or answer
    /// the refusal that delete is refused with.
    ///
    /// The one place a stored supplemental record STOPS being one — the
    /// delete-side counterpart of [`Self::store_supplemental_registration`], as
    /// [`Self::delete_registration`] is of [`Self::store_registration`]. The
    /// whole decision lives here: an `authorityId` the store does not hold names
    /// nothing to delete and is refused ([`invalid_authority_id`]), whatever the
    /// shape of the id, and nothing else is refused.
    ///
    /// A refused delete leaves the store exactly as it was, so a second delete of
    /// an id already removed meets that same refusal.
    pub fn delete_supplemental_registration(
        &mut self,
        authority_id: &str,
    ) -> Result<(), ValidationException> {
        if self
            .supplemental_registrations
            .remove(authority_id)
            .is_none()
        {
            return Err(invalid_authority_id());
        }
        Ok(())
    }

    /// Count one support case opened for an applied submission.
    ///
    /// The case itself is opened in the AWS Support store, which owns support
    /// cases and mints their ids ([`support_case::open_exemption_case`]); this
    /// is only this service's tally of the submissions it applied, so one
    /// applied submission answers for exactly one case and a refused one for
    /// none.
    pub fn count_opened_exemption_case(&mut self) {
        self.minted_exemption_cases = self.minted_exemption_cases.saturating_add(1);
    }

    /// How many exemption records the store holds, across every account.
    pub fn stored_exemption_count(&self) -> usize {
        self.exemptions.values().map(Vec::len).sum()
    }

    /// Whether the store has room for the submission `account_ids`,
    /// `exemption_type` and `authority` describe, or the refusal it is refused
    /// with.
    ///
    /// Asked BEFORE the submission's support case is opened, so a submission the
    /// store has no room for opens no case: the room a write needs is decided
    /// from the same records [`Self::store_exemptions`] then writes, which asks
    /// this first itself — a submission is applied whole or not at all.
    pub fn exemption_room(
        &self,
        account_ids: &[String],
        exemption_type: &str,
        authority: &Authority,
    ) -> Result<(), CaseCreationLimitExceededException> {
        let recorded_type = submitted_exemption_type(exemption_type, authority);
        let description = recorded_type.description.clone().unwrap_or_default();
        // Only the accounts that do not already hold this exemption ADD a
        // record; replacing one an account holds is always allowed, because it
        // adds nothing.
        let additional = distinct_account_ids(account_ids)
            .iter()
            .filter(|account_id| {
                self.exemptions.get(**account_id).is_none_or(|held| {
                    !held
                        .iter()
                        .any(|stored| is_same_exemption(stored, &description, authority))
                })
            })
            .count();
        if self.stored_exemption_count() + additional > MAX_STORED_EXEMPTIONS {
            return Err(exemption_store_full());
        }
        Ok(())
    }

    /// Record the exemption `exemption_type` and `authority` identify for every
    /// account of `account_ids`, as submitted at `recorded_at`, answering how
    /// many accounts the write addressed.
    ///
    /// The one place an exemption becomes a stored record. `exemption_type` is
    /// the token the submission named its type by, recorded as
    /// [`submitted_exemption_type`] describes. The exemption's identity is that
    /// type and the jurisdiction it applies in — the request carries no exemption
    /// id — so submitting the same pair for an account again REPLACES that
    /// account's record in place, which is what makes this a `Put` rather than an
    /// append; case and padding in either do not make a second exemption
    /// ([`is_same_exemption`]).
    ///
    /// The record is stored in [`EXEMPTION_STATUS_PENDING`], the status the model
    /// gives an exemption whose certificate has not been reviewed: a submission
    /// attaches the certificate to a support case for review, and granting the
    /// exemption is that reviewer's decision, not something the clock makes due
    /// (see [`StoredTaxExemption::reported_status`]). It carries no effective or
    /// expiration date either: both are facts about the submitted certificate,
    /// which nothing here reads.
    ///
    /// The accounts the submission addresses are the ids it names, each once
    /// ([`distinct_account_ids`]): one record per addressed account, whatever
    /// number of times the list repeats its id. That count — how many accounts
    /// the write addressed, not how many entries the list carried — is what an
    /// applied write ANSWERS, so a caller reporting on the write it just
    /// performed reports the store's own number rather than recomputing one from
    /// the request.
    ///
    /// `accountIds` is caller-supplied (up to 550 ids per call), so a submission
    /// that would ADD records past [`MAX_STORED_EXEMPTIONS`] is refused whole,
    /// before any record is written: a partially applied submission would report
    /// one support case covering accounts it did not cover. Replacing exemptions
    /// the accounts already hold is always allowed, because it adds nothing.
    ///
    /// Both passes walk only the accounts the request names plus the records they
    /// hold, and the count they are checked against is bounded by
    /// [`MAX_STORED_EXEMPTIONS`]; a larger bound would want that count kept
    /// incrementally rather than summed per submission.
    pub fn store_exemptions(
        &mut self,
        account_ids: &[String],
        exemption_type: &str,
        authority: &Authority,
        recorded_at: SystemTime,
    ) -> Result<usize, CaseCreationLimitExceededException> {
        // The type the record is held under — the published entry the token
        // names, or the token as submitted ([`submitted_exemption_type`]) — and
        // its `description`, which with the jurisdiction is the exemption's
        // identity.
        let recorded_type = submitted_exemption_type(exemption_type, authority);
        let description = recorded_type.description.clone().unwrap_or_default();
        // The bound comes first, from the same records this then writes, so a
        // submission past it is refused having written nothing
        // ([`Self::exemption_room`]).
        self.exemption_room(account_ids, exemption_type, authority)?;
        let distinct = distinct_account_ids(account_ids);

        let submitted = StoredTaxExemption {
            authority: normalized_authority(authority),
            exemption_type: recorded_type,
            effective_date: None,
            expiration_date: None,
            system_effective_date: Some(recorded_at),
            status: EXEMPTION_STATUS_PENDING.to_string(),
        };
        let addressed = distinct.len();
        for account_id in distinct {
            let held = self.exemptions.entry(account_id.to_string()).or_default();
            match held
                .iter()
                .position(|stored| is_same_exemption(stored, &description, authority))
            {
                Some(index) => held[index] = submitted.clone(),
                None => held.push(submitted.clone()),
            }
        }
        Ok(addressed)
    }
}

/// One supplemental tax registration (a supplemental TRN) of the account.
///
/// Every member of the `SupplementalTaxRegistration` shape the reads report is
/// `@required`, and the write's entry carries all of them but `authorityId` and
/// `status`, which the service owns: the id is minted when the record is first
/// stored and the status is the outcome of validating the submitted TRN.
#[derive(Debug, Clone, Default)]
pub struct StoredSupplementalRegistration {
    /// `SupplementalTaxRegistrationEntry$registrationId`, as submitted.
    pub registration_id: String,
    /// `SupplementalTaxRegistrationEntry$registrationType`, one of the model's
    /// `SupplementalTaxRegistrationType` values (today only `VAT`).
    pub registration_type: String,
    /// `SupplementalTaxRegistrationEntry$legalName`, as submitted.
    pub legal_name: String,
    /// `SupplementalTaxRegistrationEntry$address`, as submitted, with the
    /// defaults the model declares for its members materialised
    /// ([`address_with_declared_defaults`]).
    pub address: Address,
    /// The registration's `TaxRegistrationStatus`.
    pub status: String,
    /// When the validation of this TRN falls due, after which
    /// [`TrnValidation::settle`] moves [`Self::status`] to what that validation
    /// reached ([`TrnValidation::validation_outcome`]) — the same clock-owed
    /// transition a main TRN has, for the same reason, served by the same rule.
    /// `None` once it has been settled.
    pub validation_due_at: Option<Instant>,
}

impl StoredSupplementalRegistration {
    /// The record `PutSupplementalTaxRegistration` stores for `entry`.
    pub fn from_entry(entry: &SupplementalTaxRegistrationEntry, written_at: Instant) -> Self {
        // The write starts the validation of the submitted TRN and reports the
        // status the registration is stored with while that validation is
        // outstanding.
        let (status, validation_due_at) = outstanding_trn_validation(written_at);
        Self {
            registration_id: entry.registration_id.clone(),
            registration_type: entry.registration_type.clone(),
            legal_name: entry.legal_name.clone(),
            // The address is stored with the defaults the model declares for its
            // members materialised, so every read of this record reports them.
            address: address_with_declared_defaults(entry.address.clone()),
            status,
            validation_due_at,
        }
    }

    /// The `SupplementalTaxRegistration` shape the reads report, under the
    /// `authorityId` this record is stored by.
    pub fn to_supplemental_tax_registration(
        &self,
        authority_id: &str,
    ) -> SupplementalTaxRegistration {
        SupplementalTaxRegistration {
            registration_id: self.registration_id.clone(),
            registration_type: self.registration_type.clone(),
            legal_name: self.legal_name.clone(),
            address: self.address.clone(),
            authority_id: authority_id.to_string(),
            status: self.status.clone(),
        }
    }
}

impl TrnValidation for StoredSupplementalRegistration {
    /// What the validation of this supplemental TRN reaches: the number it was
    /// stored with, judged against the rule the jurisdiction it is held in
    /// publishes for confirming one — see [`trn_validation_status`].
    fn validation_outcome(&self) -> &'static str {
        trn_validation_status(
            &self.address.country_code,
            &self.registration_type,
            &self.registration_id,
        )
    }

    fn validation_due_at(&self) -> Option<Instant> {
        self.validation_due_at
    }

    fn validation_due_at_mut(&mut self) -> &mut Option<Instant> {
        &mut self.validation_due_at
    }

    fn status_mut(&mut self) -> &mut String {
        &mut self.status
    }
}

/// One account's stored tax registration.
///
/// `legalName` and `legalAddress` are stored unconditionally because every shape
/// that reports a registration (`TaxRegistration`,
/// `TaxRegistrationWithJurisdiction`) marks both `@required`. The entry they come
/// from may leave either out where the service fills it in from the tax
/// registration number, and the record then carries the value a read of the tax
/// authority's registry would have filled in — see [`DERIVED_MEMBER_UNKNOWN`].
#[derive(Debug, Clone, Default)]
pub struct StoredTaxRegistration {
    /// `TaxRegistrationEntry$registrationId`, as submitted.
    pub registration_id: String,
    /// `TaxRegistrationEntry$registrationType`, one of the model's
    /// `TaxRegistrationType` values.
    pub registration_type: String,
    /// `TaxRegistrationEntry$legalName`, as submitted, or the value the service
    /// filled in where the entry's jurisdiction let it be omitted.
    pub legal_name: String,
    /// `TaxRegistrationEntry$legalAddress`, as submitted, or the address the
    /// service filled in where the entry's jurisdiction let it be omitted —
    /// either way with the defaults the model declares for its members
    /// materialised ([`address_with_declared_defaults`]). Also the source of the
    /// `Jurisdiction` that `ListTaxRegistrations` reports.
    pub legal_address: Address,
    /// `TaxRegistrationEntry$sector`, one of the model's `Sector` values.
    pub sector: Option<String>,
    /// `TaxRegistrationEntry$additionalTaxInformation`, projected onto the
    /// response shape the reads report.
    pub additional_tax_information: Option<AdditionalInfoResponse>,
    /// `TaxRegistrationEntry$certifiedEmailId`, as submitted.
    pub certified_email_id: Option<String>,
    /// The registration's `TaxRegistrationStatus`.
    pub status: String,
    /// When the validation of this TRN falls due, after which
    /// [`TrnValidation::settle`] moves [`Self::status`] to what that validation
    /// reached ([`TrnValidation::validation_outcome`]). `None` once it has been
    /// settled, so settling is idempotent and a settled record never moves
    /// again.
    pub validation_due_at: Option<Instant>,
    /// Which write of this account's TRN stored this record, counted past every
    /// revision the store retired when a registration was removed
    /// ([`ComAmazonawsTaxsettingsTaxsettingsState::retired_registration_revision`]).
    /// The document access tokens below are derived from it, because the model
    /// documents that updating a registration invalidates the previous tokens —
    /// and so does removing it, which is why a deleted record's count is not
    /// handed out again.
    pub revision: u64,
    /// The metadata of the documents submitted with this registration, derived
    /// from `VerificationDetails$taxRegistrationDocuments` when it was stored.
    /// Empty when the registration carried no document.
    pub tax_document_metadatas: Vec<TaxDocumentMetadata>,
    /// When the access tokens of the documents above stop being valid: the
    /// instant the write that minted them ran, plus the window the model
    /// publishes for a `taxDocumentAccessToken`
    /// ([`TAX_DOCUMENT_TOKEN_VALIDITY`]). Every token of one record is minted by
    /// the one write, so one instant is the window of all of them.
    ///
    /// `None` only on a record no write produced — the derived [`Default`],
    /// which a test uses to fill the store, cannot name an instant. Such a
    /// record carries no document, so there is no token of its whose window
    /// could be asked about.
    pub document_tokens_expire_at: Option<Instant>,
}

impl StoredTaxRegistration {
    /// The record `PutTaxRegistration` stores for `entry`.
    ///
    /// `revision` is the account's new revision number (the one
    /// [`ComAmazonawsTaxsettingsTaxsettingsState::store_registration`] moved it
    /// on to), which the derived document access tokens carry.
    ///
    /// `legalName` and `legalAddress` are optional on `TaxRegistrationEntry` and
    /// `@required` on every shape that reports a registration: where the entry
    /// omits one, the caller has already established that this is a registration
    /// whose member the service fills in from the tax registration number
    /// ([`check_tax_registration_entry`]), and the record carries the filled-in
    /// value — the jurisdiction the entry identified, and
    /// [`DERIVED_MEMBER_UNKNOWN`] for what only the tax authority's registry
    /// holds.
    pub fn from_entry(
        entry: &TaxRegistrationEntry,
        account_id: &str,
        revision: u64,
        written_at: Instant,
    ) -> Self {
        let legal_name = entry
            .legal_name
            .clone()
            .unwrap_or_else(|| DERIVED_MEMBER_UNKNOWN.to_string());
        let given_address = entry.legal_address.clone().unwrap_or_else(|| {
            // The judge admits an entry carrying no address only when the entry
            // identifies the jurisdiction the address is filled in for, so a
            // record stored here always reports a country.
            let country_code = entry_country_code(entry).unwrap_or_else(|| {
                debug_assert!(
                    false,
                    "an entry stored without a legal address must identify the jurisdiction the \
                     address is filled in for"
                );
                ""
            });
            derived_legal_address(country_code)
        });
        // The address is stored with the defaults the model declares for its
        // members materialised, so every read of this record reports them.
        let legal_address = address_with_declared_defaults(given_address);
        // The write starts the validation of the submitted TRN and reports the
        // status the registration is stored with while that validation is
        // outstanding.
        let (status, validation_due_at) = outstanding_trn_validation(written_at);
        Self {
            registration_id: entry.registration_id.clone(),
            registration_type: entry.registration_type.clone(),
            legal_name,
            legal_address,
            sector: entry.sector.clone(),
            additional_tax_information: entry
                .additional_tax_information
                .as_ref()
                .map(additional_info_response),
            certified_email_id: entry.certified_email_id.clone(),
            status,
            validation_due_at,
            revision,
            tax_document_metadatas: document_metadatas(entry, account_id, revision),
            // The write is what mints this record's document access tokens, so
            // the window the model publishes for them runs from it.
            document_tokens_expire_at: Some(written_at + TAX_DOCUMENT_TOKEN_VALIDITY),
        }
    }

    /// Whether the access tokens this record's documents carry are still inside
    /// the validity window the model publishes for a `taxDocumentAccessToken` at
    /// `now` — see [`Self::document_tokens_expire_at`].
    ///
    /// A record whose window this store does not hold vouches for no token, so
    /// no token of it is inside one.
    pub fn document_tokens_valid_at(&self, now: Instant) -> bool {
        self.document_tokens_expire_at
            .is_some_and(|expires| now < expires)
    }

    /// The `TaxRegistration` shape `GetTaxRegistration` reports, carrying the
    /// full legal address.
    pub fn to_tax_registration(&self) -> TaxRegistration {
        TaxRegistration {
            registration_id: self.registration_id.clone(),
            registration_type: self.registration_type.clone(),
            legal_name: self.legal_name.clone(),
            status: self.status.clone(),
            sector: self.sector.clone(),
            tax_document_metadatas: self.reported_document_metadatas(),
            certified_email_id: self.certified_email_id.clone(),
            additional_tax_information: self.additional_tax_information.clone(),
            legal_address: self.legal_address.clone(),
        }
    }

    /// The `TaxRegistrationWithJurisdiction` shape `ListTaxRegistrations`
    /// reports: the same record, with the legal address reduced to the
    /// jurisdiction (country code, and state or region where there is one) the
    /// model says this shape carries instead of the full address.
    pub fn to_tax_registration_with_jurisdiction(&self) -> TaxRegistrationWithJurisdiction {
        TaxRegistrationWithJurisdiction {
            registration_id: self.registration_id.clone(),
            registration_type: self.registration_type.clone(),
            legal_name: self.legal_name.clone(),
            status: self.status.clone(),
            sector: self.sector.clone(),
            tax_document_metadatas: self.reported_document_metadatas(),
            certified_email_id: self.certified_email_id.clone(),
            additional_tax_information: self.additional_tax_information.clone(),
            jurisdiction: self.jurisdiction(),
        }
    }

    /// This registration's `Jurisdiction`: the stored legal address reduced to
    /// its country code and, where the address has one, its state or region.
    ///
    /// One projection for every shape that reports where the registration is
    /// held, so the jurisdiction beside the TRN and the one in the account's
    /// address-role map cannot disagree.
    pub fn jurisdiction(&self) -> Jurisdiction {
        Jurisdiction {
            state_or_region: self.legal_address.state_or_region.clone(),
            country_code: self.legal_address.country_code.clone(),
        }
    }

    /// The `AccountMetaData` a listing reports beside the account's TRN: the
    /// legal profile this service holds for the account.
    ///
    /// Every member is projected from the stored record. `accountName` is the
    /// legal name the registration was stored with — the only name any operation
    /// this service models persists for an account; `address` is the stored
    /// legal address in full (the listing's `taxRegistration` carries only its
    /// jurisdiction, so this is where a caller reads the address); `addressType`
    /// and `addressRoleMap` report the role that address plays
    /// ([`LEGAL_ADDRESS_ROLE`]) and, under that role, the jurisdiction of the
    /// address held in it.
    ///
    /// `seller` is the Amazon Web Services seller of record for the account: no
    /// operation this service models carries one and nothing in the store records
    /// one, so it stays absent rather than being invented.
    pub fn to_account_meta_data(&self) -> AccountMetaData {
        AccountMetaData {
            account_name: Some(self.legal_name.clone()),
            seller: None,
            address: Some(self.legal_address.clone()),
            address_type: Some(LEGAL_ADDRESS_ROLE.to_string()),
            address_role_map: Some(HashMap::from([(
                LEGAL_ADDRESS_ROLE.to_string(),
                self.jurisdiction(),
            )])),
        }
    }

    /// `taxDocumentMetadatas` as the reads report it: absent rather than an
    /// empty list, because the model constrains `TaxDocumentMetadatas` to
    /// 1..=5 entries.
    fn reported_document_metadatas(&self) -> Option<Vec<TaxDocumentMetadata>> {
        if self.tax_document_metadatas.is_empty() {
            None
        } else {
            Some(self.tax_document_metadatas.clone())
        }
    }
}

impl TrnValidation for StoredTaxRegistration {
    /// What the validation of this TRN reaches: the number it was stored with,
    /// judged against the rule the jurisdiction it is held in publishes for
    /// confirming one — see [`trn_validation_status`].
    ///
    /// Read from the stored record, so the outcome was fixed by the write and
    /// every reader of this record settles it identically.
    fn validation_outcome(&self) -> &'static str {
        trn_validation_status(
            &self.legal_address.country_code,
            &self.registration_type,
            &self.registration_id,
        )
    }

    fn validation_due_at(&self) -> Option<Instant> {
        self.validation_due_at
    }

    fn validation_due_at_mut(&mut self) -> &mut Option<Instant> {
        &mut self.validation_due_at
    }

    fn status_mut(&mut self) -> &mut String {
        &mut self.status
    }
}

/// One tax exemption recorded for an account.
///
/// The dates are wall-clock instants ([`SystemTime`]) rather than [`Instant`]s
/// because the model reports them as `Timestamp`s to the caller, and because
/// whether an exemption has expired is a fact about the calendar.
#[derive(Debug, Clone)]
pub struct StoredTaxExemption {
    /// `TaxExemption$authority`: the address domain (country, and state where
    /// the exemption is a state one) the exemption applies in.
    pub authority: Authority,
    /// `TaxExemption$taxExemptionType`: the type the exemption was submitted
    /// under, as [`submitted_exemption_type`] records it.
    pub exemption_type: TaxExemptionType,
    /// When the exemption takes effect. `None` while that is not decided (an
    /// exemption still under review has no effective date yet).
    pub effective_date: Option<SystemTime>,
    /// When the exemption lapses. `None` for one that does not expire.
    pub expiration_date: Option<SystemTime>,
    /// When this service recorded the exemption —
    /// `TaxExemption$systemEffectiveDate`, "the tax exemption recording time in
    /// the `TaxSettings` system".
    pub system_effective_date: Option<SystemTime>,
    /// The exemption's recorded `EntityExemptionAccountStatus`
    /// ([`EXEMPTION_STATUS_VALID`] and friends).
    pub status: String,
}

impl StoredTaxExemption {
    /// The status a read reports at `now`.
    ///
    /// Lapsing is a transition the CLOCK owes: an exemption whose expiration
    /// date has passed is expired at that instant whether or not anybody called,
    /// so a read reports [`EXEMPTION_STATUS_EXPIRED`] for it rather than the
    /// status it was recorded with. Every other status is reported as recorded —
    /// a `Pending` certificate becomes valid only when a reviewer decides it
    /// does, which is not something the clock can make due.
    pub fn reported_status(&self, now: SystemTime) -> &str {
        if self.status == EXEMPTION_STATUS_VALID
            && self.expiration_date.is_some_and(|expires| expires <= now)
        {
            EXEMPTION_STATUS_EXPIRED
        } else {
            &self.status
        }
    }

    /// The `TaxExemption` shape the exemption reads report for this record at
    /// `now`.
    pub fn to_tax_exemption(&self, now: SystemTime) -> TaxExemption {
        TaxExemption {
            authority: self.authority.clone(),
            tax_exemption_type: self.exemption_type.clone(),
            effective_date: self.effective_date.map(::aws_smithy_types::DateTime::from),
            expiration_date: self.expiration_date.map(::aws_smithy_types::DateTime::from),
            system_effective_date: self
                .system_effective_date
                .map(::aws_smithy_types::DateTime::from),
            status: Some(self.reported_status(now).to_string()),
        }
    }
}

/// Project an `AdditionalInfoRequest` (what a client submits) onto the
/// `AdditionalInfoResponse` (what the reads report), country block by country
/// block.
///
/// `brazilAdditionalInfo` and `indiaAdditionalInfo` exist only on the response
/// shape — there is no request member to carry them — so they stay absent.
///
/// `malaysiaAdditionalInfo` is the one block with a member that declares a
/// default, which the projection materialises
/// ([`malaysia_info_with_declared_defaults`]); every other block is reported as
/// it was submitted.
fn additional_info_response(request: &AdditionalInfoRequest) -> AdditionalInfoResponse {
    AdditionalInfoResponse {
        malaysia_additional_info: request
            .malaysia_additional_info
            .clone()
            .map(malaysia_info_with_declared_defaults),
        israel_additional_info: request.israel_additional_info.clone(),
        estonia_additional_info: request.estonia_additional_info.clone(),
        canada_additional_info: request.canada_additional_info.clone(),
        brazil_additional_info: None,
        spain_additional_info: request.spain_additional_info.clone(),
        kenya_additional_info: request.kenya_additional_info.clone(),
        south_korea_additional_info: request.south_korea_additional_info.clone(),
        turkey_additional_info: request.turkey_additional_info.clone(),
        georgia_additional_info: request.georgia_additional_info.clone(),
        italy_additional_info: request.italy_additional_info.clone(),
        romania_additional_info: request.romania_additional_info.clone(),
        ukraine_additional_info: request.ukraine_additional_info.clone(),
        poland_additional_info: request.poland_additional_info.clone(),
        saudi_arabia_additional_info: request.saudi_arabia_additional_info.clone(),
        india_additional_info: None,
        indonesia_additional_info: request.indonesia_additional_info.clone(),
        vietnam_additional_info: request.vietnam_additional_info.clone(),
        egypt_additional_info: request.egypt_additional_info.clone(),
        greece_additional_info: request.greece_additional_info.clone(),
        uzbekistan_additional_info: request.uzbekistan_additional_info.clone(),
        philippines_additional_info: request.philippines_additional_info.clone(),
        belgium_additional_info: request.belgium_additional_info.clone(),
        chile_additional_info: request.chile_additional_info.clone(),
        france_additional_info: request.france_additional_info.clone(),
        monaco_additional_info: request.monaco_additional_info.clone(),
    }
}

/// The `TaxDocumentMetadata` of each document submitted in
/// `VerificationDetails$taxRegistrationDocuments`.
///
/// A document is named by its uploaded file name, or by its S3 key when it was
/// referenced in S3. The access token locates the document within this account's
/// registration revision, so writing the registration again mints new tokens and
/// the previous ones no longer name anything — one of the two invalidations the
/// model documents for `taxDocumentAccessToken`. The other is the window the
/// minting write starts ([`TAX_DOCUMENT_TOKEN_VALIDITY`]), which
/// [`StoredTaxRegistration::document_tokens_expire_at`] records for the tokens
/// minted here.
fn document_metadatas(
    entry: &TaxRegistrationEntry,
    account_id: &str,
    revision: u64,
) -> Vec<TaxDocumentMetadata> {
    let Some(documents) = entry
        .verification_details
        .as_ref()
        .and_then(|details| details.tax_registration_documents.as_ref())
    else {
        return Vec::new();
    };
    documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let name = document
                .file
                .as_ref()
                .map(|file| file.file_name.clone())
                .or_else(|| document.s3_location.as_ref().map(|loc| loc.key.clone()))
                .unwrap_or_else(|| format!("document-{}", index + 1));
            TaxDocumentMetadata {
                tax_document_access_token: document_access_token(
                    account_id, revision, index, &name,
                ),
                tax_document_name: name,
            }
        })
        .collect()
}

/// The access token minted for ONE document of a stored registration: the
/// coordinates that locate it — the account, the registration revision the
/// document was submitted with, the document's position in that submission and
/// its name — as one opaque string.
///
/// Paired with [`decode_document_access_token`], its only reader, so what a
/// write mints and what a later request is resolved against cannot drift apart.
/// Encoded with `aws_smithy_types::base64`, the codec this service's other
/// opaque token carries ([`encode_page_token`]).
fn document_access_token(account_id: &str, revision: u64, index: usize, name: &str) -> String {
    base64::encode(format!("{account_id}/{revision}/{index}/{name}"))
}

/// A tax document this service holds: which registration's documents it is one
/// of, and what it is called.
///
/// These are exactly the coordinates [`document_access_token`] carries, so
/// resolving a token answers this and every reader of a held document — the
/// download URL, the destination path — is built from the same four values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTaxDocument {
    /// The account whose tax registration carries the document.
    pub account_id: String,
    /// The revision of that registration the document was submitted with.
    pub revision: u64,
    /// The document's position in that submission, from zero.
    pub index: usize,
    /// The document's name — its uploaded file name, or its S3 object key where
    /// it was referenced in S3.
    pub name: String,
}

/// How this service words the refusal of a `taxDocumentAccessToken` that cannot
/// be resolved to a tax document it holds, whichever way the token failed.
///
/// AWS answers ONE sentence for the whole class — the token is "either
/// malformed or incorrect" — so every refusal of the class states it and the
/// published `errorCode` is what tells them apart. It names no value from the
/// request: the refusal is about the member, not about the text submitted for
/// it.
pub const TAX_DOCUMENT_TOKEN_REFUSED: &str = "The specified token in the request is either \
                                             malformed or incorrect. Check your request and try \
                                             again.";

/// How long a `taxDocumentAccessToken` this service mints stays valid, counted
/// from the write that minted it.
///
/// AWS's own duration, not a span this emulator chose: the model states it on
/// the member that carries the token — "This token is valid for 24 hours."
/// (`smithy_query(query="shape",shape="TaxDocumentMetadata")`, member
/// `taxDocumentAccessToken`).
///
/// The window is published for THIS token only. The opaque cursor this service
/// also mints ([`encode_page_token`]) carries no published window, so it is not
/// aged.
pub const TAX_DOCUMENT_TOKEN_VALIDITY: Duration = Duration::from_secs(24 * 60 * 60);

/// The name a refusal of a tax document access token reports in
/// `ValidationExceptionField$name`.
///
/// OBSERVED, and NOT the CONSTANT_CASE of the member that carries the token
/// (`TaxDocumentMetadata$taxDocumentAccessToken` would give
/// `TAX_DOCUMENT_ACCESS_TOKEN`): the field list names the service's own
/// PARAMETER, and for this one the service abbreviates the document. Recorded
/// unit `taxsettings.get-tax-registration-document-invalid-token-rejected`. See
/// [`parameter_name`] for the rule the parameters with no recording of their own
/// follow, and [`validation_naming`] for how a parameter like this one is named.
pub const TAX_DOCUMENT_ACCESS_TOKEN_PARAMETER: &str = "TAX_DOC_ACCESS_TOKEN";

/// The coordinates a `taxDocumentAccessToken` carries, or the refusal of a text
/// that is not a token this service minted.
///
/// The name is the REST of the encoded text rather than one more separated
/// field, because a document named by its S3 object key carries the separator
/// itself.
fn decode_document_access_token(token: &str) -> Result<HeldTaxDocument, ValidationException> {
    let malformed = || {
        validation_naming(
            ERROR_CODE_MALFORMED_TOKEN,
            TAX_DOCUMENT_ACCESS_TOKEN_PARAMETER,
            TAX_DOCUMENT_TOKEN_REFUSED,
        )
    };
    let decoded = base64::decode(token).map_err(|_| malformed())?;
    let decoded = String::from_utf8(decoded).map_err(|_| malformed())?;
    let mut fields = decoded.splitn(4, '/');
    let (Some(account_id), Some(revision), Some(index), Some(name)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(malformed());
    };
    Ok(HeldTaxDocument {
        account_id: account_id.to_string(),
        revision: revision.parse().map_err(|_| malformed())?,
        index: index.parse().map_err(|_| malformed())?,
        name: name.to_string(),
    })
}

/// The refusal of a `TaxDocumentMetadata` that names no tax document this
/// service holds.
///
/// `InvalidToken` rather than `MalformedToken`: the token IS one of this
/// service's, it just no longer locates a document — the registration it was
/// minted for has since been rewritten or deleted, which is one of the two
/// invalidations the model documents for `taxDocumentAccessToken` (the other
/// being age — see [`expired_tax_document_token`]).
pub fn unknown_tax_document() -> ValidationException {
    validation_naming(
        ERROR_CODE_INVALID_TOKEN,
        TAX_DOCUMENT_ACCESS_TOKEN_PARAMETER,
        TAX_DOCUMENT_TOKEN_REFUSED,
    )
}

/// The refusal of a `taxDocumentAccessToken` presented past the validity window
/// the model publishes for it ([`TAX_DOCUMENT_TOKEN_VALIDITY`]).
///
/// `ExpiredToken` rather than `InvalidToken`: the token IS one of this service's
/// and it still locates a document, it has only outlived the window — the second
/// invalidation the model documents for `taxDocumentAccessToken`, beside the
/// rewrite [`unknown_tax_document`] answers. The message is the one sentence the
/// class is refused with ([`TAX_DOCUMENT_TOKEN_REFUSED`]), so the code is what
/// tells a caller which of the two it hit.
pub fn expired_tax_document_token() -> ValidationException {
    validation_naming(
        ERROR_CODE_EXPIRED_TOKEN,
        TAX_DOCUMENT_ACCESS_TOKEN_PARAMETER,
        TAX_DOCUMENT_TOKEN_REFUSED,
    )
}

/// The Amazon S3 bucket a reader reports an account's tax documents from.
///
/// The documents are the SERVICE's copies of what was submitted with the
/// account's tax registration, so they live in a bucket of its own named from
/// the account they belong to — never in the caller's bucket, which a request
/// names only to be downloaded INTO. The form satisfies `S3BucketName`'s
/// published constraints (3..=63 characters of lowercase label text) for every
/// `accountId` the model admits (12 digits).
pub fn document_bucket(account_id: &str) -> String {
    format!("aws-tax-documents-{account_id}")
}

/// The URL a reader reports for a held tax document.
///
/// It names the document's own object in the account's document bucket by the
/// coordinates its access token carries — the registration revision and the
/// document's position in it ([`documents::document_object_key`]) — so every read
/// of one document reports one URL, two documents never share one, and rewriting
/// the registration retires the previous URL exactly as it retires the previous
/// access token. The object it names is the copy this service holds there
/// ([`documents::hold_documents`]), so the location resolves to the document's
/// own content.
///
/// It carries no query signature: the model caps `Url` at 200 characters, which
/// no SigV4 query-signed form fits (its signature parameters alone are longer
/// than that). What the URL does carry is where the document is, derived from
/// this service's state.
pub fn document_download_url(document: &HeldTaxDocument) -> String {
    format!(
        "https://{}.s3.amazonaws.com/{}",
        document_bucket(&document.account_id),
        documents::document_object_key(document.revision, document.index)
    )
}

/// The account a request addresses: the `accountId` it carries, or the caller's
/// own account when it omits one, as the model documents for every `accountId`
/// member of this service.
pub fn resolve_account_id(account_id: Option<String>) -> String {
    account_id.unwrap_or_else(|| ::engine::emulator_account_id().to_string())
}

/// The accounts an `accountIds` list addresses: each id once, in the order it
/// first appears in the request.
///
/// Every `accountIds` member of this service is documented "List of unique
/// account identifiers", so an id named more than once names the SAME account
/// and the SAME single action on it: the caller walking this list acts on each
/// addressed account once rather than a second time against the record this very
/// call just wrote or removed — which, for a batch, would report the action it
/// performed as having failed in the response that performed it.
///
/// First-seen order, so a response built while walking the list (the per-account
/// rows of a batch's `errors` list) reports the accounts in the order the request
/// named them.
///
/// The one place that rule is stated, beside [`resolve_account_id`] — the one
/// place an addressed account is normalised: every operation whose input carries
/// an `accountIds` list addresses its accounts through here, as does
/// [`ComAmazonawsTaxsettingsTaxsettingsState::store_exemptions`], the write one
/// of them performs, so none of them can disagree about what a repeated id
/// means.
pub fn distinct_account_ids(account_ids: &[String]) -> Vec<&str> {
    let mut addressed: BTreeSet<&str> = BTreeSet::new();
    let mut distinct: Vec<&str> = Vec::with_capacity(account_ids.len());
    for account_id in account_ids {
        if addressed.insert(account_id.as_str()) {
            distinct.push(account_id.as_str());
        }
    }
    distinct
}

/// This service's modelled `ValidationException` (HTTP 400) naming the member it
/// refused.
///
/// `fieldList` carries the parameter the refusal is about, named the way this
/// service names one on the wire — see [`parameter_name`].
pub fn validation(
    error_code: &str,
    member: &str,
    message: impl Into<String>,
) -> ValidationException {
    validation_naming(error_code, &parameter_name(member), message)
}

/// The same refusal as [`validation`], naming the refused PARAMETER outright.
///
/// For a parameter whose name is not the CONSTANT_CASE of the member that
/// carries it, which is the one thing [`parameter_name`] cannot derive — see
/// [`TAX_DOCUMENT_ACCESS_TOKEN_PARAMETER`], the parameter a recording pins that
/// way. Every other refusal names its member and goes through [`validation`].
pub fn validation_naming(
    error_code: &str,
    parameter: &str,
    message: impl Into<String>,
) -> ValidationException {
    ValidationException {
        message: message.into(),
        error_code: error_code.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: parameter.to_string(),
        }]),
    }
}

/// The same refusal as [`validation`] about the CALL rather than one of its
/// parameters, so `fieldList` is absent.
///
/// For a refusal no member of the request is at fault for — the caller's
/// standing, which no parameter carries — where naming a parameter would tell
/// the client to repair a value that is not what was refused.
pub fn validation_of_the_call(
    error_code: &str,
    message: impl Into<String>,
) -> ValidationException {
    ValidationException {
        message: message.into(),
        error_code: error_code.to_string(),
        field_list: None,
    }
}

/// The name this service reports in `ValidationExceptionField$name`: the
/// offending member's own name in CONSTANT_CASE, which is the form AWS answers
/// with (`taxRegistrationEntry.legalAddress` -> `LEGAL_ADDRESS`).
///
/// The member path a message states is dotted lowerCamel, as the boundary
/// validator spells it; the field list names the PARAMETER, so only the last
/// segment appears and it is re-cased.
///
/// The CONSTANT_CASE form is OBSERVED for one parameter (`LEGAL_ADDRESS`, in the
/// refusal of a legal address submitted for an India PAN registration) and
/// INFERRED for the rest: one casing rule for every parameter this service names
/// is more likely than a per-member one, but no recording pins `NEXT_TOKEN` or
/// `HERITAGE_STATUS`. A later fixer with a recording that disagrees should follow
/// the recording.
pub fn parameter_name(member_path: &str) -> String {
    let member = member_path.rsplit('.').next().unwrap_or(member_path);
    let mut name = String::with_capacity(member.len() + 2);
    for character in member.chars() {
        if character.is_ascii_uppercase() && !name.is_empty() {
            name.push('_');
        }
        name.extend(character.to_uppercase());
    }
    name
}

/// Refuse an entry that submits a member its registration type does not admit.
///
/// Today that is the legal address of a type AWS derives it from the tax number
/// for ([`type_with_derived_legal_address`]): the address is not the caller's to
/// supply, so submitting one is a field violation naming that parameter.
pub fn check_registration_type_members(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    let registration_type = &entry.registration_type;
    let Some((_, country)) = type_with_derived_legal_address(registration_type) else {
        return Ok(());
    };
    if entry.legal_address.is_some() {
        return Err(validation(
            ERROR_CODE_FIELD_VALIDATION_FAILED,
            "taxRegistrationEntry.legalAddress",
            format!(
                "Legal address should not be provided for {country} {registration_type} Tax \
                 Registration."
            ),
        ));
    }
    Ok(())
}

/// Refuse an entry that sets a registration type the service ties to one country
/// for a registration held in a different country.
///
/// The country the registration is held in is the `countryCode` of the legal
/// address the entry carries, and only where that address is the caller's to
/// supply: a type whose address the service derives from the tax number
/// ([`type_with_derived_legal_address`]) admits no submitted address at all, so
/// an entry of such a type carries no value this rule may read — a submitted one
/// is the field violation [`check_registration_type_members`] refuses, and it is
/// not evidence of where the customer is. An entry that carries no address
/// identifies no country but the one its own type belongs to
/// ([`entry_country_code`]), so there is no other jurisdiction for the rule to
/// refuse it for either. The refusal names the `registrationType` parameter,
/// because the type is what cannot be set here, and carries the `errorCode`
/// [`TYPE_OUTSIDE_ITS_COUNTRY_CODES`] publishes for that type.
pub fn check_registration_type_jurisdiction(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    let registration_type = &entry.registration_type;
    let Some(error_code) = type_outside_its_country_code(registration_type) else {
        return Ok(());
    };
    if type_with_derived_legal_address(registration_type).is_some() {
        return Ok(());
    }
    let Some((country_code, country)) = type_jurisdiction(registration_type) else {
        return Ok(());
    };
    let Some(address) = entry.legal_address.as_ref() else {
        return Ok(());
    };
    if address.country_code.eq_ignore_ascii_case(country_code) {
        return Ok(());
    }
    Err(validation(
        error_code,
        "taxRegistrationEntry.registrationType",
        type_outside_its_country_message(registration_type, country),
    ))
}

/// The `ValidationException$errorCode` a registration type held outside the one
/// country it belongs to is refused with, or `None` for a type whose
/// jurisdiction this service does not refuse a registration for — see
/// [`TYPE_OUTSIDE_ITS_COUNTRY_CODES`].
pub fn type_outside_its_country_code(registration_type: &str) -> Option<&'static str> {
    TYPE_OUTSIDE_ITS_COUNTRY_CODES
        .iter()
        .find(|(declared, _)| *declared == registration_type)
        .map(|(_, error_code)| *error_code)
}

/// The `ValidationException$errorCode` a write of `registration_type` is refused
/// with for an account whose stored registration is of `stored_type`, or `None`
/// for a pair this service does not block — see
/// [`TYPES_BLOCKED_BY_A_STORED_TYPE`].
pub fn type_blocked_by_stored_type(
    stored_type: &str,
    registration_type: &str,
) -> Option<&'static str> {
    TYPES_BLOCKED_BY_A_STORED_TYPE
        .iter()
        .find(|(stored, submitted, _)| *stored == stored_type && *submitted == registration_type)
        .map(|(_, _, error_code)| *error_code)
}

/// The jurisdiction of a registration type AWS fills the legal address in for —
/// its `Address$countryCode` and the country's name — or `None` for a type whose
/// address comes from the request.
///
/// The carve-out is a fact about ONE country's tax number, so it is read off the
/// rows [`TYPE_JURISDICTIONS`] ties to a country rather than off the type name
/// alone: a type named in [`TYPES_WITH_DERIVED_LEGAL_ADDRESS`] with no
/// jurisdiction row has no country whose number the address could come from, and
/// carries no carve-out.
pub fn type_with_derived_legal_address(
    registration_type: &str,
) -> Option<(&'static str, &'static str)> {
    if !TYPES_WITH_DERIVED_LEGAL_ADDRESS.contains(&registration_type) {
        return None;
    }
    type_jurisdiction(registration_type)
}

/// The country a country-specific `TaxRegistrationType` belongs to — its
/// `Address$countryCode` and the country's name — or `None` for a type the model
/// ties to no single country. See [`TYPE_JURISDICTIONS`].
pub fn type_jurisdiction(registration_type: &str) -> Option<(&'static str, &'static str)> {
    TYPE_JURISDICTIONS
        .iter()
        .find(|(declared, _, _)| *declared == registration_type)
        .map(|(_, country_code, country)| (*country_code, *country))
}

/// The jurisdiction `entry` identifies: the country of the legal address it
/// carries, else the country its registration type belongs to.
///
/// `None` for an entry that carries no address and whose type belongs to no one
/// country: nothing in such a request says where the registration is held, so no
/// per-country rule of this service can be applied to it.
pub fn entry_country_code(entry: &TaxRegistrationEntry) -> Option<&str> {
    match entry.legal_address.as_ref() {
        Some(address) => Some(address.country_code.as_str()),
        None => type_jurisdiction(&entry.registration_type).map(|(country_code, _)| country_code),
    }
}

/// Whether the service fills in the legal NAME of a registration held in
/// `country_code` from the tax registration number — see
/// [`COUNTRIES_WITH_DERIVED_LEGAL_NAME`].
pub fn legal_name_is_derived(country_code: &str) -> bool {
    COUNTRIES_WITH_DERIVED_LEGAL_NAME
        .iter()
        .any(|code| code.eq_ignore_ascii_case(country_code))
}

/// Whether the service fills in the legal ADDRESS of a `registration_type`
/// registration held in `country_code` from the tax registration number — see
/// [`COUNTRIES_WITH_DERIVED_LEGAL_ADDRESS`] and
/// [`TYPES_WITH_DERIVED_LEGAL_ADDRESS`].
pub fn legal_address_is_derived(country_code: &str, registration_type: &str) -> bool {
    if type_with_derived_legal_address(registration_type).is_some() {
        return true;
    }
    COUNTRIES_WITH_DERIVED_LEGAL_ADDRESS
        .iter()
        .any(|(code, types)| {
            code.eq_ignore_ascii_case(country_code)
                && (types.is_empty() || types.contains(&registration_type))
        })
}

/// The legal address stored for an entry that carried none, held in the
/// jurisdiction `country_code`.
///
/// The country is what the entry identified ([`entry_country_code`]), so the
/// jurisdiction every reader reports is the registration's own. The rest of the
/// address is what the service would have read from the tax authority's registry,
/// which this emulator has none of, so `postalCode` carries
/// [`DERIVED_MEMBER_UNKNOWN`], and `addressLine1` and `city` are left to the
/// defaults the model declares for them ([`address_with_declared_defaults`]).
pub fn derived_legal_address(country_code: &str) -> Address {
    address_with_declared_defaults(Address {
        postal_code: DERIVED_MEMBER_UNKNOWN.to_string(),
        country_code: country_code.to_string(),
        ..Address::default()
    })
}

/// Materialise the defaults the model declares for the members of an `Address`.
///
/// `addressLine1` and `city` are the two members of the shape that declare one,
/// and both declare [`ADDRESS_MEMBER_DEFAULT`]; every other member declares none
/// and stays as it was submitted — absent where the request omitted it.
///
/// Applied where an address is STORED ([`StoredTaxRegistration::from_entry`],
/// [`StoredSupplementalRegistration::from_entry`]), so the record holds the
/// address the service accepted and every reader of it reports the same members
/// without knowing the rule.
pub fn address_with_declared_defaults(mut address: Address) -> Address {
    if address.address_line1.is_none() {
        address.address_line1 = Some(ADDRESS_MEMBER_DEFAULT.to_string());
    }
    if address.city.is_none() {
        address.city = Some(ADDRESS_MEMBER_DEFAULT.to_string());
    }
    address
}

/// Materialise the default the model declares for the members of
/// `MalaysiaAdditionalInfo`: `serviceTaxCodes` declares `{"default": [],
/// "addedDefault": true}`
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#MalaysiaAdditionalInfo")`),
/// so a stored registration whose country block omitted it reports the declared
/// empty list. The block's other members declare no default.
///
/// The same rule as [`address_with_declared_defaults`], applied to the other
/// shape of this service whose members declare one.
pub fn malaysia_info_with_declared_defaults(
    mut info: MalaysiaAdditionalInfo,
) -> MalaysiaAdditionalInfo {
    if info.service_tax_codes.is_none() {
        info.service_tax_codes = Some(Vec::new());
    }
    info
}

/// Refuse an all-whitespace value for a member the model constrains with its
/// non-blank pattern `^(?!\s*$)[\s\S]+$`.
///
/// That pattern is a negative lookahead, which the boundary validator's regex
/// engine cannot compile, so a blank value reaches the handler having satisfied
/// every constraint the boundary could check. The handler applies the one it
/// could not.
pub fn check_not_blank(member: &str, value: &str) -> Result<(), ValidationException> {
    if !value.trim().is_empty() {
        return Ok(());
    }
    Err(validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        member,
        format!("{member} must not be blank."),
    ))
}

/// Apply [`check_not_blank`] to every `Address` member the model constrains with
/// the non-blank pattern, then the jurisdictions each member is accepted for
/// ([`check_address_member_jurisdictions`]). `countryCode`'s own pattern
/// (`^[a-zA-Z]+$`) is lookahead-free, so the boundary validator already enforces
/// it.
///
/// The one place an `Address` this service is asked to store is judged, so every
/// operation that takes one — the legal address of a `TaxRegistrationEntry` and
/// the address of a `SupplementalTaxRegistrationEntry` — judges it the same way.
pub fn check_address(member_path: &str, address: &Address) -> Result<(), ValidationException> {
    let members: [(&str, Option<&String>); 7] = [
        ("addressLine1", address.address_line1.as_ref()),
        ("addressLine2", address.address_line2.as_ref()),
        ("addressLine3", address.address_line3.as_ref()),
        ("districtOrCounty", address.district_or_county.as_ref()),
        ("city", address.city.as_ref()),
        ("stateOrRegion", address.state_or_region.as_ref()),
        ("postalCode", Some(&address.postal_code)),
    ];
    for (name, value) in members {
        if let Some(value) = value {
            check_not_blank(&format!("{member_path}.{name}"), value)?;
        }
    }
    check_address_member_jurisdictions(member_path, address)
}

/// One `Address` member the model documents the API as accepting only for
/// certain jurisdictions: the member as the wire spells it, the
/// `Address$countryCode`s of the countries whose address accepts it, those
/// countries as a refusal names them, and whether a submitted address carries
/// the member.
///
/// A restriction of this kind is not one the SHAPE can carry, because every
/// country's address is the same shape, so it is a per-country rule of this
/// service like the rows of [`COUNTRY_RULES`] — and, where one sentence states
/// both halves, it is the other half of a rule that table already carries: a
/// member only one country's address accepts is also the member that country's
/// registration must specify.
#[derive(Debug, Clone, Copy)]
pub struct AddressMemberJurisdiction {
    /// The member the rule is about, as the wire spells it. Appended to the
    /// address's own member path, so the refusal names the parameter the request
    /// carried it in — see [`parameter_name`].
    pub member: &'static str,
    /// The `Address$countryCode`s whose address the API accepts the member for.
    pub country_codes: &'static [&'static str],
    /// Those countries' names, as the refusal states them.
    pub countries: &'static str,
    /// Whether a submitted address carries the member.
    pub submitted: fn(&Address) -> bool,
}

impl AddressMemberJurisdiction {
    /// Whether an address held in `country_code` accepts this member. Compared
    /// case-insensitively, as every reader of an `Address$countryCode` here is:
    /// the member's own pattern (`^[a-zA-Z]+$`) admits either case.
    pub fn accepts(&self, country_code: &str) -> bool {
        self.country_codes
            .iter()
            .any(|code| code.eq_ignore_ascii_case(country_code))
    }

    /// The refusal's message: the rule as the member's own documentation states
    /// it, naming the member and the jurisdictions whose address accepts it.
    pub fn message(&self) -> String {
        format!(
            "The Tax Settings API accepts the {} parameter only for {}.",
            self.member, self.countries
        )
    }
}

/// Every `Address` member the model documents the API as accepting only for
/// certain jurisdictions.
///
/// `addressLine3` is the only member of the shape whose documentation states
/// such a restriction
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#Address")`):
/// "Currently, the Tax Settings API accepts the `addressLine3` parameter only
/// for Saudi Arabia. When you specify a TRN in Saudi Arabia, you must enter the
/// `addressLine3` …". The requiring half of that sentence is the Saudi row of
/// [`COUNTRY_RULES`]; this is the accepting half. `districtOrCounty` names a
/// country too ("For addresses in Brazil, this parameter uses the name of the
/// neighborhood"), but that sentence says what the member MEANS there, not which
/// countries may send it, so it is no row here.
pub const ADDRESS_MEMBER_JURISDICTIONS: &[AddressMemberJurisdiction] =
    &[AddressMemberJurisdiction {
        member: "addressLine3",
        country_codes: &["SA"],
        countries: "Saudi Arabia",
        submitted: |address: &Address| address.address_line3.is_some(),
    }];

/// Refuse an address that carries a member the API accepts only for
/// jurisdictions other than the one that address names
/// ([`ADDRESS_MEMBER_JURISDICTIONS`]).
///
/// The jurisdiction is the address's OWN `countryCode`, because the rule is
/// about the address the member belongs to: a member carried at all where the
/// API does not accept it is a value that member may not take there, hence
/// [`ERROR_CODE_FIELD_VALIDATION_FAILED`], naming the parameter the request
/// carried it in — the same refusal class as the other rules of this service
/// about a member a jurisdiction does not admit.
pub fn check_address_member_jurisdictions(
    member_path: &str,
    address: &Address,
) -> Result<(), ValidationException> {
    for rule in ADDRESS_MEMBER_JURISDICTIONS {
        if !(rule.submitted)(address) || rule.accepts(&address.country_code) {
            continue;
        }
        return Err(validation(
            ERROR_CODE_FIELD_VALIDATION_FAILED,
            &format!("{member_path}.{}", rule.member),
            rule.message(),
        ));
    }
    Ok(())
}

/// What AWS answers when the `additionalTaxInformation` object a request carries
/// does not satisfy the country's rule.
///
/// Observed for the Kenya rule (an entry whose `kenyaAdditionalInfo` carries no
/// `personType`) and used for every rule about that object: one message per
/// class of refusal, with the offending parameter in `fieldList`, rather than a
/// per-member sentence no recording pins.
pub const ADDITIONAL_INFO_INVALID: &str = "Your request has an AdditionalInfo object that isn't \
                                           valid or is missing information. Check your request \
                                           and try again.";

/// The `Sector` values the model documents as valid for a country whose rule
/// reads "The sector valid values are `Business` and `Individual`" (Malaysia,
/// Nepal, Ukraine) — a subset of the `Sector` enum (`Business | Individual |
/// Government`).
pub const SECTORS_BUSINESS_OR_INDIVIDUAL: &[&str] = &["Business", "Individual"];

/// The `PersonType` values the model documents as valid for Georgia ("The valid
/// `personType` values are `Physical Person` and `Business`") — a subset of the
/// `PersonType` enum (`Legal Person | Physical Person | Business`).
pub const GEORGIA_PERSON_TYPES: &[&str] = &["Physical Person", "Business"];

/// The `TaxRegistrationType` values the model documents as valid for Malaysia
/// ("`RegistrationType` valid values are `NRIC` for individual, and TIN and
/// sales and service tax (SST) for Business") — the VALUE SET that sentence
/// states. Which of those values each `sector` admits is the other half of it,
/// and lives in [`SECTOR_REGISTRATION_TYPES`].
pub const MALAYSIA_REGISTRATION_TYPES: &[&str] = &["NRIC", "TIN", "SST"];

/// The Malaysian registration types the model documents as the business ones,
/// for which "you must specify a `businessRegistrationNumber` in
/// `MalaysiaAdditionalInfo`".
pub const MALAYSIA_BUSINESS_REGISTRATION_TYPES: &[&str] = &["TIN", "SST"];

/// The Malaysian registration type the model documents as the individual one
/// ("`RegistrationType` valid values are `NRIC` for individual"), and for which
/// "for individual, you can specify the `taxInformationNumber` in
/// `MalaysiaAdditionalInfo` with NRIC type".
pub const MALAYSIA_INDIVIDUAL_REGISTRATION_TYPES: &[&str] = &["NRIC"];

/// The `TaxRegistrationType` values a country's documentation pairs with ONE
/// `Sector`: the `Address$countryCode`, the sector, and the registration types
/// an entry declaring that sector may choose from.
///
/// A country whose documentation states which type goes with which sector
/// constrains the two members TOGETHER, so the country's value set alone
/// ([`MALAYSIA_REGISTRATION_TYPES`], the union of its rows here) is only half the
/// rule: the other half is which of those values the declared sector admits. One
/// row per sector the documentation names a set for; a sector with no row is one
/// the documentation pairs no types with, which this table therefore says nothing
/// about.
///
/// Malaysia is the only country `PutTaxRegistration`'s documentation states such
/// a pairing for
/// (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`):
/// "`RegistrationType` valid values are `NRIC` for individual, and TIN and sales
/// and service tax (SST) for Business."
pub const SECTOR_REGISTRATION_TYPES: &[(&str, &str, &[&str])] = &[
    ("MY", "Individual", MALAYSIA_INDIVIDUAL_REGISTRATION_TYPES),
    ("MY", "Business", MALAYSIA_BUSINESS_REGISTRATION_TYPES),
];

/// The registration types a registration held in `country_code` whose entry
/// declares `sector` may carry, or `None` where the documentation pairs the two
/// members for neither — see [`SECTOR_REGISTRATION_TYPES`].
///
/// `country_code` is compared case-insensitively, as every reader of an
/// `Address$countryCode` here is; `sector` is an enum value the boundary
/// validator has already matched exactly.
pub fn sector_registration_types(
    country_code: &str,
    sector: &str,
) -> Option<&'static [&'static str]> {
    SECTOR_REGISTRATION_TYPES
        .iter()
        .find(|(code, documented, _)| {
            code.eq_ignore_ascii_case(country_code) && *documented == sector
        })
        .map(|(_, _, types)| *types)
}

/// The Malaysian registration type of a business reseller, for which "you must
/// specify a `businessRegistrationNumber` and `taxInformationNumber` in
/// `MalaysiaAdditionalInfo`".
pub const MALAYSIA_RESELLER_REGISTRATION_TYPE: &str = "SST";

/// The `IndonesiaTaxRegistrationNumberType` values whose
/// `ppnExceptionDesignationCode` set is the wider one: "If the
/// `taxRegistrationNumberType` is set to NPWP or NITKU, valid values for
/// `ppnExceptionDesignationCode` are either 01, 02, 03, 07, or 08."
pub const INDONESIA_WIDE_PPN_TYPES: &[&str] = &["NPWP", "NITKU"];

/// See [`INDONESIA_WIDE_PPN_TYPES`].
pub const INDONESIA_PPN_CODES_WIDE: &[&str] = &["01", "02", "03", "07", "08"];

/// "For other `taxRegistrationNumberType` values, `ppnExceptionDesignationCode`
/// must be either 01, 07, or 08."
pub const INDONESIA_PPN_CODES_NARROW: &[&str] = &["01", "07", "08"];

/// "If `ppnExceptionDesignationCode` is 07 or 08, you must specify the
/// `decisionNumber`."
pub const INDONESIA_PPN_CODES_NEEDING_DECISION_NUMBER: &[&str] = &["07", "08"];

/// The Kenyan `personType` for which "you must specify the tax registration
/// certificate document in the `taxRegistrationDocuments` field".
pub const KENYA_PERSON_TYPE_NEEDING_DOCUMENT: &str = "Physical Person";

/// The Spanish `registrationType` for which "you must specify the tax
/// registration certificate document in the `taxRegistrationDocuments` field".
pub const SPAIN_REGISTRATION_TYPE_NEEDING_DOCUMENT: &str = "Local";

/// The Turkish `sector` for which the model documents "If your `sector` is
/// `Individual`, don't enter this value" of `taxOffice` and `kepEmailId`, and
/// for which `taxOffice` is therefore not required either.
pub const TURKEY_SECTOR_WITHOUT_TAX_OFFICE: &str = "Individual";

/// The Brazilian `TaxRegistrationType` whose address must name a state, region or
/// province — "Brazil (CPF)" in the documentation of `Address$stateOrRegion`
/// (see [`COUNTRY_RULES`]). A Brazilian registration of any other type carries
/// the member only if the caller chooses to.
pub const BRAZIL_STATE_OR_REGION_TYPE: &str = "CPF";

/// The Canadian provinces for whose TRN a provincial sales tax ID is a reseller
/// claim, each as its subdivision code and as the province's name.
///
/// `CanadaAdditionalInfo$isResellerAccount` is documented as "The value for this
/// parameter must be `true` if the `provincialSalesTaxId` value is provided for a
/// TRN in British Columbia, Saskatchewan, or Manitoba provinces"
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#CanadaAdditionalInfo")`),
/// so these three provinces are the ones that rule turns on — Quebec, the fourth
/// province whose number `provincialSalesTaxId` may carry, is not one of them.
/// Each row carries the subdivision code beside the name because the province is
/// read off free text — see [`canada_reseller_province`].
pub const CANADA_RESELLER_PROVINCES: &[(&str, &str)] = &[
    ("BC", "British Columbia"),
    ("SK", "Saskatchewan"),
    ("MB", "Manitoba"),
];

/// The province of [`CANADA_RESELLER_PROVINCES`] that `state_or_region` names, by
/// its subdivision code or by its name, or `None` for any other value.
///
/// `Address$stateOrRegion` is free text the model constrains only in length and
/// non-blankness ("use the same name as shown on the Tax Settings page"), so the
/// match is case-insensitive and ignores surrounding space and admits either
/// spelling. A value this cannot place names no province of the set, so a rule
/// keyed off it has nothing to say about the entry: the match is what makes a
/// rule APPLY, never what satisfies one, so an unfamiliar spelling cannot turn
/// into a refusal of an entry the service accepts.
pub fn canada_reseller_province(state_or_region: &str) -> Option<&'static str> {
    let value = state_or_region.trim();
    CANADA_RESELLER_PROVINCES
        .iter()
        .find(|(code, province)| {
            value.eq_ignore_ascii_case(code) || value.eq_ignore_ascii_case(province)
        })
        .map(|(_, province)| *province)
}

/// One per-country business rule `PutTaxRegistration` documents for
/// `taxRegistrationEntry`.
///
/// The operation's model documentation states, country by country, which members
/// of the entry a request for that country must specify and which values those
/// members may take ("Kenya: You must specify the `personType` in the
/// `kenyaAdditionalInfo` field of the `additionalTaxInformation` object"). Each
/// row below is one of those sentences, so the rule set is read off the model
/// rather than written per request shape, and one evaluator
/// ([`check_country_requirements`]) applies all of them.
#[derive(Debug, Clone, Copy)]
pub struct CountryRule {
    /// The `Address$countryCode` whose rule this is. Compared case-insensitively,
    /// because the member's pattern (`^[a-zA-Z]+$`) admits either case.
    pub country_code: &'static str,
    /// The member the rule is about, as the wire spells it. Its last segment is
    /// the parameter the refusal names — see [`parameter_name`].
    pub member_path: &'static str,
    /// The `ValidationExceptionErrorCode` the refusal carries:
    /// [`ERROR_CODE_MISSING_INPUT`] for a member the country requires and the
    /// entry omits, [`ERROR_CODE_FIELD_VALIDATION_FAILED`] for one the entry
    /// carries with a value (or at all) the country does not admit.
    pub error_code: &'static str,
    /// The refusal's message.
    pub message: &'static str,
    /// Whether this rule has anything to say about `entry` — some rules are
    /// documented as conditional on another member of the same entry.
    pub applies: fn(&TaxRegistrationEntry) -> bool,
    /// Whether `entry` satisfies the rule.
    pub satisfied: fn(&TaxRegistrationEntry) -> bool,
}

/// A rule that applies to every entry for its country.
fn always(_entry: &TaxRegistrationEntry) -> bool {
    true
}

/// The `additionalTaxInformation` object of `entry`, if it carries one.
fn additional_info(entry: &TaxRegistrationEntry) -> Option<&AdditionalInfoRequest> {
    entry.additional_tax_information.as_ref()
}

/// Whether `entry` carries at least one tax registration certificate document in
/// `verificationDetails.taxRegistrationDocuments`.
fn has_registration_document(entry: &TaxRegistrationEntry) -> bool {
    entry
        .verification_details
        .as_ref()
        .and_then(|details| details.tax_registration_documents.as_ref())
        .is_some_and(|documents| !documents.is_empty())
}

/// `taxRegistrationEntry.sector`, if the entry carries one.
fn sector(entry: &TaxRegistrationEntry) -> Option<&str> {
    entry.sector.as_deref()
}

/// The registration types the entry's own jurisdiction pairs with the `sector` it
/// declares — [`sector_registration_types`] for the entry's country and sector.
///
/// `None` for an entry that declares no sector, identifies no jurisdiction, or is
/// held where the documentation pairs the two members for no sector: nothing
/// about such an entry states which types its sector admits.
fn entry_sector_registration_types(
    entry: &TaxRegistrationEntry,
) -> Option<&'static [&'static str]> {
    sector_registration_types(entry_country_code(entry)?, sector(entry)?)
}

/// What makes the pairing rule apply: the entry declares a sector its
/// jurisdiction pairs registration types with.
fn sector_pairs_registration_types(entry: &TaxRegistrationEntry) -> bool {
    entry_sector_registration_types(entry).is_some()
}

/// Whether the entry's `registrationType` is one its jurisdiction pairs with the
/// `sector` it declares. An entry the pairing says nothing about satisfies it.
fn registration_type_fits_sector(entry: &TaxRegistrationEntry) -> bool {
    entry_sector_registration_types(entry)
        .is_none_or(|types| types.contains(&entry.registration_type.as_str()))
}

/// Whether the entry's `sector` is one of `SECTORS_BUSINESS_OR_INDIVIDUAL`, for
/// a country that admits only those two. An entry carrying no sector is left to
/// the rule that requires one, where the country has one.
fn sector_is_business_or_individual(entry: &TaxRegistrationEntry) -> bool {
    sector(entry).is_none_or(|value| SECTORS_BUSINESS_OR_INDIVIDUAL.contains(&value))
}

/// `additionalTaxInformation.kenyaAdditionalInfo.personType`, if the entry
/// carries the Kenyan block. `personType` is `@required` on
/// `KenyaAdditionalInfo`, so the boundary validator has already refused a block
/// without one: carrying the block is carrying the member.
fn kenya_person_type(entry: &TaxRegistrationEntry) -> Option<&str> {
    additional_info(entry)
        .and_then(|info| info.kenya_additional_info.as_ref())
        .map(|kenya| kenya.person_type.as_str())
}

/// `additionalTaxInformation.spainAdditionalInfo.registrationType`, if the entry
/// carries the Spanish block (`registrationType` is `@required` on it).
fn spain_registration_type(entry: &TaxRegistrationEntry) -> Option<&str> {
    additional_info(entry)
        .and_then(|info| info.spain_additional_info.as_ref())
        .map(|spain| spain.registration_type.as_str())
}

/// The entry's `indonesiaAdditionalInfo` block, if it carries one.
fn indonesia_info(entry: &TaxRegistrationEntry) -> Option<&IndonesiaAdditionalInfo> {
    additional_info(entry).and_then(|info| info.indonesia_additional_info.as_ref())
}

/// `indonesiaAdditionalInfo.ppnExceptionDesignationCode`, if the entry carries
/// one.
fn indonesia_ppn_code(entry: &TaxRegistrationEntry) -> Option<&str> {
    indonesia_info(entry).and_then(|indonesia| indonesia.ppn_exception_designation_code.as_deref())
}

/// Whether the `ppnExceptionDesignationCode` the entry carries is one the
/// documented set for its `taxRegistrationNumberType` admits.
fn indonesia_ppn_code_is_admitted(entry: &TaxRegistrationEntry) -> bool {
    let Some(indonesia) = indonesia_info(entry) else {
        return true;
    };
    let Some(code) = indonesia.ppn_exception_designation_code.as_deref() else {
        return true;
    };
    let admitted = match indonesia.tax_registration_number_type.as_deref() {
        Some(number_type) if INDONESIA_WIDE_PPN_TYPES.contains(&number_type) => {
            INDONESIA_PPN_CODES_WIDE
        }
        _ => INDONESIA_PPN_CODES_NARROW,
    };
    admitted.contains(&code)
}

/// The entry's `turkeyAdditionalInfo` member `select`ed, or `None` when the entry
/// carries no Turkish block.
fn turkey_member(
    entry: &TaxRegistrationEntry,
    select: fn(&crate::ops::types::TurkeyAdditionalInfo) -> Option<&String>,
) -> Option<String> {
    additional_info(entry)
        .and_then(|info| info.turkey_additional_info.as_ref())
        .and_then(|turkey| select(turkey).cloned())
}

/// Whether the entry's `sector` is the Turkish one that does not take a
/// `taxOffice` or a `kepEmailId`.
fn turkey_sector_is_individual(entry: &TaxRegistrationEntry) -> bool {
    sector(entry).is_some_and(|value| value == TURKEY_SECTOR_WITHOUT_TAX_OFFICE)
}

/// Whether `entry` submitted a legal address of its own.
///
/// What makes every rule about an ADDRESS member apply: an entry that carries no
/// address either had it refused already (the member its jurisdiction requires
/// and it omits) or has it filled in from the tax number
/// ([`derived_legal_address`]), and in neither case is an address member the
/// caller's to supply.
fn submitted_legal_address(entry: &TaxRegistrationEntry) -> bool {
    entry.legal_address.is_some()
}

/// Whether the legal address `entry` submitted names a state, region or
/// province.
fn legal_address_has_state_or_region(entry: &TaxRegistrationEntry) -> bool {
    entry
        .legal_address
        .as_ref()
        .is_some_and(|address| address.state_or_region.is_some())
}

/// The entry's `canadaAdditionalInfo` block, if it carries one.
fn canada_info(entry: &TaxRegistrationEntry) -> Option<&CanadaAdditionalInfo> {
    additional_info(entry).and_then(|info| info.canada_additional_info.as_ref())
}

/// Whether `entry` claims a provincial sales tax ID for a TRN held in one of the
/// provinces where that claim is a reseller claim
/// ([`CANADA_RESELLER_PROVINCES`]).
fn canada_claims_provincial_sales_tax_id(entry: &TaxRegistrationEntry) -> bool {
    canada_info(entry).is_some_and(|canada| canada.provincial_sales_tax_id.is_some())
        && entry
            .legal_address
            .as_ref()
            .and_then(|address| address.state_or_region.as_deref())
            .and_then(canada_reseller_province)
            .is_some()
}

/// Whether `entry` confirms that purchases made under it are for resale.
fn canada_is_reseller_account(entry: &TaxRegistrationEntry) -> bool {
    canada_info(entry).is_some_and(|canada| canada.is_reseller_account == Some(true))
}

/// Every per-country business rule `PutTaxRegistration`'s model documentation
/// states for `taxRegistrationEntry`, in the order that documentation lists the
/// countries
/// (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`),
/// followed by the per-country requirements the model states on the entry's own
/// MEMBERS rather than in that list (`Address$stateOrRegion`,
/// `CanadaAdditionalInfo$isResellerAccount`): a member a shape cannot constrain,
/// because every country shares the shape, is a per-country rule of this table
/// like any other.
///
/// Two documented rules are deliberately NOT rows here, because the fact they
/// turn on is not in the request and this service models no API that supplies
/// one: Brazil's "complete the registration process in the console first", whose
/// completion no member of the entry states, and Turkey's "if your `sector` is
/// `Business` and you're subject to KDV tax, you must specify your industry",
/// whose KDV liability no member states either. The Brazil and Mexico carve-outs
/// that make `legalName` and `legalAddress` omittable are not rules a rule row
/// can carry — they REMOVE a requirement rather than add one — and live in
/// [`COUNTRIES_WITH_DERIVED_LEGAL_NAME`] /
/// [`COUNTRIES_WITH_DERIVED_LEGAL_ADDRESS`], which
/// [`check_tax_registration_entry`] applies.
pub const COUNTRY_RULES: &[CountryRule] = &[
    // Bangladesh: "You must specify the tax registration certificate document in
    // the taxRegistrationDocuments field of the VerificationDetails object."
    CountryRule {
        country_code: "BD",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Bangladesh must specify the tax registration certificate \
                  document in verificationDetails.taxRegistrationDocuments.",
        applies: always,
        satisfied: has_registration_document,
    },
    // Georgia: "The valid personType values are Physical Person and Business."
    CountryRule {
        country_code: "GE",
        member_path: "additionalTaxInformation.georgiaAdditionalInfo.personType",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.georgia_additional_info.is_some())
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry)
                .and_then(|info| info.georgia_additional_info.as_ref())
                .is_none_or(|georgia| {
                    GEORGIA_PERSON_TYPES.contains(&georgia.person_type.as_str())
                })
        },
    },
    // Indonesia: "You must specify the taxRegistrationNumberType in the
    // indonesiaAdditionalInfo field of the additionalTaxInformation object."
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.taxRegistrationNumberType",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            indonesia_info(entry)
                .is_some_and(|indonesia| indonesia.tax_registration_number_type.is_some())
        },
    },
    // Indonesia: "If you specify decisionNumber, you must specify the
    // ppnExceptionDesignationCode in the indonesiaAdditionalInfo field of the
    // additionalTaxInformation object."
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.ppnExceptionDesignationCode",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            indonesia_info(entry).is_some_and(|indonesia| indonesia.decision_number.is_some())
        },
        satisfied: |entry: &TaxRegistrationEntry| indonesia_ppn_code(entry).is_some(),
    },
    // Indonesia: the ppnExceptionDesignationCode value set, which depends on the
    // taxRegistrationNumberType — see `indonesia_ppn_code_is_admitted`.
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.ppnExceptionDesignationCode",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| indonesia_ppn_code(entry).is_some(),
        satisfied: indonesia_ppn_code_is_admitted,
    },
    // Indonesia: "If ppnExceptionDesignationCode is 07 or 08, you must specify
    // the decisionNumber in the indonesiaAdditionalInfo field of the
    // additionalTaxInformation object."
    CountryRule {
        country_code: "ID",
        member_path: "additionalTaxInformation.indonesiaAdditionalInfo.decisionNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            indonesia_ppn_code(entry)
                .is_some_and(|code| INDONESIA_PPN_CODES_NEEDING_DECISION_NUMBER.contains(&code))
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            indonesia_info(entry).is_some_and(|indonesia| indonesia.decision_number.is_some())
        },
    },
    // Kenya: "You must specify the personType in the kenyaAdditionalInfo field of
    // the additionalTaxInformation object."
    CountryRule {
        country_code: "KE",
        member_path: "additionalTaxInformation.kenyaAdditionalInfo.personType",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| kenya_person_type(entry).is_some(),
    },
    // Kenya: "If the personType is Physical Person, you must specify the tax
    // registration certificate document in the taxRegistrationDocuments field of
    // the VerificationDetails object."
    CountryRule {
        country_code: "KE",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Kenya for a Physical Person must specify the tax \
                  registration certificate document in \
                  verificationDetails.taxRegistrationDocuments.",
        applies: |entry: &TaxRegistrationEntry| {
            kenya_person_type(entry).is_some_and(|person| person == KENYA_PERSON_TYPE_NEEDING_DOCUMENT)
        },
        satisfied: has_registration_document,
    },
    // Malaysia: "The sector valid values are Business and Individual."
    CountryRule {
        country_code: "MY",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid sector values for a tax registration in Malaysia are Business and \
                  Individual.",
        applies: always,
        satisfied: sector_is_business_or_individual,
    },
    // Malaysia: "RegistrationType valid values are NRIC for individual, and TIN
    // and sales and service tax (SST) for Business."
    CountryRule {
        country_code: "MY",
        member_path: "taxRegistrationEntry.registrationType",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid registrationType values for a tax registration in Malaysia are NRIC, \
                  TIN and SST.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            MALAYSIA_REGISTRATION_TYPES.contains(&entry.registration_type.as_str())
        },
    },
    // Malaysia: the same sentence's OTHER half — which of those values goes with
    // which sector, rather than the union of them — see
    // [`SECTOR_REGISTRATION_TYPES`].
    CountryRule {
        country_code: "MY",
        member_path: "taxRegistrationEntry.registrationType",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid registrationType values for a tax registration in Malaysia are NRIC \
                  for Individual, and TIN and SST for Business.",
        applies: sector_pairs_registration_types,
        satisfied: registration_type_fits_sector,
    },
    // Malaysia: "For business, you must specify a businessRegistrationNumber in
    // MalaysiaAdditionalInfo with a TIN type and tax identification number."
    CountryRule {
        country_code: "MY",
        member_path: "additionalTaxInformation.malaysiaAdditionalInfo.businessRegistrationNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            MALAYSIA_BUSINESS_REGISTRATION_TYPES.contains(&entry.registration_type.as_str())
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry)
                .and_then(|info| info.malaysia_additional_info.as_ref())
                .is_some_and(|malaysia| malaysia.business_registration_number.is_some())
        },
    },
    // Malaysia: "For business resellers, you must specify a
    // businessRegistrationNumber and taxInformationNumber in
    // MalaysiaAdditionalInfo with a sales and service tax (SST) type and a valid
    // SST number."
    CountryRule {
        country_code: "MY",
        member_path: "additionalTaxInformation.malaysiaAdditionalInfo.taxInformationNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            entry.registration_type == MALAYSIA_RESELLER_REGISTRATION_TYPE
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry)
                .and_then(|info| info.malaysia_additional_info.as_ref())
                .is_some_and(|malaysia| malaysia.tax_information_number.is_some())
        },
    },
    // Mexico: "You must provide a Constancia de Situación fiscal (CSF) document
    // in the verificationDetails field."
    CountryRule {
        country_code: "MX",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Mexico must provide a Constancia de Situacion Fiscal \
                  (CSF) document in verificationDetails.taxRegistrationDocuments.",
        applies: always,
        satisfied: has_registration_document,
    },
    // Nepal: "The sector valid values are Business and Individual."
    CountryRule {
        country_code: "NP",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid sector values for a tax registration in Nepal are Business and \
                  Individual.",
        applies: always,
        satisfied: sector_is_business_or_individual,
    },
    // Saudi Arabia: "For address, you must specify addressLine3." The other
    // half of the member's rule — that the API accepts it ONLY there — is
    // [`ADDRESS_MEMBER_JURISDICTIONS`], because it is about the address rather
    // than about this country's entry.
    CountryRule {
        country_code: "SA",
        member_path: "taxRegistrationEntry.legalAddress.addressLine3",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Saudi Arabia must specify legalAddress.addressLine3.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            entry
                .legal_address
                .as_ref()
                .is_some_and(|address| address.address_line3.is_some())
        },
    },
    // South Korea: "You must specify the certifiedEmailId and legalName in the
    // TaxRegistrationEntry object." (`legalName` is required for every country
    // here — see `ops/put_tax_registration.rs`.)
    CountryRule {
        country_code: "KR",
        member_path: "taxRegistrationEntry.certifiedEmailId",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in South Korea must specify certifiedEmailId.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| entry.certified_email_id.is_some(),
    },
    // South Korea: "You must specify the businessRepresentativeName,
    // itemOfBusiness, and lineOfBusiness in the southKoreaAdditionalInfo field of
    // the additionalTaxInformation object." All three are `@required` on
    // `SouthKoreaAdditionalInfo`, so the boundary validator refuses a block
    // missing any one: what is left to check is that the block is there.
    CountryRule {
        country_code: "KR",
        member_path: "additionalTaxInformation.southKoreaAdditionalInfo",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.south_korea_additional_info.is_some())
        },
    },
    // South Korea: "You must specify the tax registration certificate document in
    // the taxRegistrationDocuments field of the VerificationDetails object."
    CountryRule {
        country_code: "KR",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in South Korea must specify the tax registration certificate \
                  document in verificationDetails.taxRegistrationDocuments.",
        applies: always,
        satisfied: has_registration_document,
    },
    // Spain: "You must specify the registrationType in the spainAdditionalInfo
    // field of the additionalTaxInformation object."
    CountryRule {
        country_code: "ES",
        member_path: "additionalTaxInformation.spainAdditionalInfo.registrationType",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| spain_registration_type(entry).is_some(),
    },
    // Spain: "If the registrationType is Local, you must specify the tax
    // registration certificate document in the taxRegistrationDocuments field of
    // the VerificationDetails object."
    CountryRule {
        country_code: "ES",
        member_path: "taxRegistrationEntry.verificationDetails.taxRegistrationDocuments",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A Local tax registration in Spain must specify the tax registration certificate \
                  document in verificationDetails.taxRegistrationDocuments.",
        applies: |entry: &TaxRegistrationEntry| {
            spain_registration_type(entry)
                .is_some_and(|value| value == SPAIN_REGISTRATION_TYPE_NEEDING_DOCUMENT)
        },
        satisfied: has_registration_document,
    },
    // Turkey: "You must specify the sector in the taxRegistrationEntry object."
    CountryRule {
        country_code: "TR",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Turkey must specify sector.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| sector(entry).is_some(),
    },
    // Turkey: "For address, you must specify districtOrCounty."
    CountryRule {
        country_code: "TR",
        member_path: "taxRegistrationEntry.legalAddress.districtOrCounty",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Turkey must specify legalAddress.districtOrCounty.",
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            entry
                .legal_address
                .as_ref()
                .is_some_and(|address| address.district_or_county.is_some())
        },
    },
    // Turkey: "Specify the taxOffice. If your sector is Individual, don't enter
    // this value." — required for the other sectors, refused for Individual.
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.taxOffice",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: |entry: &TaxRegistrationEntry| {
            sector(entry).is_some() && !turkey_sector_is_individual(entry)
        },
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.tax_office.as_ref()).is_some()
        },
    },
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.taxOffice",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: turkey_sector_is_individual,
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.tax_office.as_ref()).is_none()
        },
    },
    // Turkey: "(Optional) Specify the kepEmailId. If your sector is Individual,
    // don't enter this value."
    CountryRule {
        country_code: "TR",
        member_path: "additionalTaxInformation.turkeyAdditionalInfo.kepEmailId",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: turkey_sector_is_individual,
        satisfied: |entry: &TaxRegistrationEntry| {
            turkey_member(entry, |turkey| turkey.kep_email_id.as_ref()).is_none()
        },
    },
    // Ukraine: "The sector valid values are Business and Individual."
    CountryRule {
        country_code: "UA",
        member_path: "taxRegistrationEntry.sector",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: "The valid sector values for a tax registration in Ukraine are Business and \
                  Individual.",
        applies: always,
        satisfied: sector_is_business_or_individual,
    },
    // France: "You must specify the sirenNumber in the franceAdditionalInfo field
    // of the additionalTaxInformation object." (`sirenNumber` is `@required` on
    // `FranceAdditionalInfo`, so carrying the block is carrying the member.)
    CountryRule {
        country_code: "FR",
        member_path: "additionalTaxInformation.franceAdditionalInfo.sirenNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.france_additional_info.is_some())
        },
    },
    // Monaco: "You must specify the businessNumber in the monacoAdditionalInfo
    // field of the additionalTaxInformation object." (`businessNumber` is
    // `@required` on `MonacoAdditionalInfo`.)
    CountryRule {
        country_code: "MC",
        member_path: "additionalTaxInformation.monacoAdditionalInfo.businessNumber",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: ADDITIONAL_INFO_INVALID,
        applies: always,
        satisfied: |entry: &TaxRegistrationEntry| {
            additional_info(entry).is_some_and(|info| info.monaco_additional_info.is_some())
        },
    },
    // The rows above are the operation's own country-by-country list. The rows
    // below state the per-country requirements the model documents on the
    // MEMBERS of the entry instead, which are rules of exactly the same kind:
    // a member one country's registration must carry, or must carry a given
    // value, which no constraint on the shape can express because the shape is
    // shared by every country.
    //
    // `Address$stateOrRegion`: "This field is only required for Canada, India,
    // United Arab Emirates, Romania, and Brazil (CPF). It is optional for all
    // other countries."
    // (`smithy_query(query="doc",shape="com.amazonaws.taxsettings#Address",member="stateOrRegion")`)
    // One row per country of that sentence, because each states its own
    // jurisdiction in its refusal.
    CountryRule {
        country_code: "CA",
        member_path: "taxRegistrationEntry.legalAddress.stateOrRegion",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Canada must specify legalAddress.stateOrRegion.",
        applies: submitted_legal_address,
        satisfied: legal_address_has_state_or_region,
    },
    CountryRule {
        country_code: "IN",
        member_path: "taxRegistrationEntry.legalAddress.stateOrRegion",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in India must specify legalAddress.stateOrRegion.",
        applies: submitted_legal_address,
        satisfied: legal_address_has_state_or_region,
    },
    CountryRule {
        country_code: "AE",
        member_path: "taxRegistrationEntry.legalAddress.stateOrRegion",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in the United Arab Emirates must specify \
                  legalAddress.stateOrRegion.",
        applies: submitted_legal_address,
        satisfied: legal_address_has_state_or_region,
    },
    CountryRule {
        country_code: "RO",
        member_path: "taxRegistrationEntry.legalAddress.stateOrRegion",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A tax registration in Romania must specify legalAddress.stateOrRegion.",
        applies: submitted_legal_address,
        satisfied: legal_address_has_state_or_region,
    },
    // Brazil is in that sentence for ONE registration type — see
    // [`BRAZIL_STATE_OR_REGION_TYPE`] — so the row is conditional on the type,
    // and a Brazilian registration of any other type is left alone.
    CountryRule {
        country_code: "BR",
        member_path: "taxRegistrationEntry.legalAddress.stateOrRegion",
        error_code: ERROR_CODE_MISSING_INPUT,
        message: "A CPF tax registration in Brazil must specify legalAddress.stateOrRegion.",
        applies: |entry: &TaxRegistrationEntry| {
            submitted_legal_address(entry) && entry.registration_type == BRAZIL_STATE_OR_REGION_TYPE
        },
        satisfied: legal_address_has_state_or_region,
    },
    // `CanadaAdditionalInfo$isResellerAccount`: "The value for this parameter
    // must be true if the provincialSalesTaxId value is provided for a TRN in
    // British Columbia, Saskatchewan, or Manitoba provinces." The province the
    // TRN is held in is the address's own `stateOrRegion` — see
    // [`CANADA_RESELLER_PROVINCES`] — which the Canadian row above has already
    // required, so this rule is reached with the province stated.
    CountryRule {
        country_code: "CA",
        member_path: "additionalTaxInformation.canadaAdditionalInfo.isResellerAccount",
        error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
        message: ADDITIONAL_INFO_INVALID,
        applies: canada_claims_provincial_sales_tax_id,
        satisfied: canada_is_reseller_account,
    },
];

/// One country block of an `additionalTaxInformation` object: the member the wire
/// spells it with, the jurisdiction whose TRN it carries additional information
/// for, and whether a submitted object carries it.
///
/// The jurisdiction of a block is the country its own documentation names
/// ("Additional tax information associated with your TRN in Poland"), as the
/// `Address$countryCode` of that country — the ISO 3166-1 alpha-2 code that
/// member carries.
#[derive(Debug, Clone, Copy)]
pub struct AdditionalInfoBlock {
    /// The member of `AdditionalInfoRequest` that carries the block, as the wire
    /// spells it. The parameter a refusal about the block names — see
    /// [`parameter_name`].
    pub member: &'static str,
    /// The `Address$countryCode` of the country the block belongs to.
    pub country_code: &'static str,
    /// Whether a submitted object carries this block.
    pub submitted: fn(&AdditionalInfoRequest) -> bool,
}

/// Every country block `AdditionalInfoRequest` declares, in the order the model
/// declares them
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#AdditionalInfoRequest")`).
///
/// One row per member of that shape, so counting the blocks an object carries and
/// placing the one it carries are read off the model rather than off the countries
/// some rule happens to mention. `brazilAdditionalInfo` and `indiaAdditionalInfo`
/// are deliberately NOT rows: they exist only on the RESPONSE shape, so no request
/// can carry either (see [`additional_info_response`]).
pub const ADDITIONAL_INFO_BLOCKS: &[AdditionalInfoBlock] = &[
    AdditionalInfoBlock {
        member: "malaysiaAdditionalInfo",
        country_code: "MY",
        submitted: |info: &AdditionalInfoRequest| info.malaysia_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "israelAdditionalInfo",
        country_code: "IL",
        submitted: |info: &AdditionalInfoRequest| info.israel_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "estoniaAdditionalInfo",
        country_code: "EE",
        submitted: |info: &AdditionalInfoRequest| info.estonia_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "canadaAdditionalInfo",
        country_code: "CA",
        submitted: |info: &AdditionalInfoRequest| info.canada_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "spainAdditionalInfo",
        country_code: "ES",
        submitted: |info: &AdditionalInfoRequest| info.spain_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "kenyaAdditionalInfo",
        country_code: "KE",
        submitted: |info: &AdditionalInfoRequest| info.kenya_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "southKoreaAdditionalInfo",
        country_code: "KR",
        submitted: |info: &AdditionalInfoRequest| info.south_korea_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "turkeyAdditionalInfo",
        country_code: "TR",
        submitted: |info: &AdditionalInfoRequest| info.turkey_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "georgiaAdditionalInfo",
        country_code: "GE",
        submitted: |info: &AdditionalInfoRequest| info.georgia_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "italyAdditionalInfo",
        country_code: "IT",
        submitted: |info: &AdditionalInfoRequest| info.italy_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "romaniaAdditionalInfo",
        country_code: "RO",
        submitted: |info: &AdditionalInfoRequest| info.romania_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "ukraineAdditionalInfo",
        country_code: "UA",
        submitted: |info: &AdditionalInfoRequest| info.ukraine_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "polandAdditionalInfo",
        country_code: "PL",
        submitted: |info: &AdditionalInfoRequest| info.poland_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "saudiArabiaAdditionalInfo",
        country_code: "SA",
        submitted: |info: &AdditionalInfoRequest| info.saudi_arabia_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "indonesiaAdditionalInfo",
        country_code: "ID",
        submitted: |info: &AdditionalInfoRequest| info.indonesia_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "vietnamAdditionalInfo",
        country_code: "VN",
        submitted: |info: &AdditionalInfoRequest| info.vietnam_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "egyptAdditionalInfo",
        country_code: "EG",
        submitted: |info: &AdditionalInfoRequest| info.egypt_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "greeceAdditionalInfo",
        country_code: "GR",
        submitted: |info: &AdditionalInfoRequest| info.greece_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "uzbekistanAdditionalInfo",
        country_code: "UZ",
        submitted: |info: &AdditionalInfoRequest| info.uzbekistan_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "philippinesAdditionalInfo",
        country_code: "PH",
        submitted: |info: &AdditionalInfoRequest| info.philippines_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "belgiumAdditionalInfo",
        country_code: "BE",
        submitted: |info: &AdditionalInfoRequest| info.belgium_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "chileAdditionalInfo",
        country_code: "CL",
        submitted: |info: &AdditionalInfoRequest| info.chile_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "franceAdditionalInfo",
        country_code: "FR",
        submitted: |info: &AdditionalInfoRequest| info.france_additional_info.is_some(),
    },
    AdditionalInfoBlock {
        member: "monacoAdditionalInfo",
        country_code: "MC",
        submitted: |info: &AdditionalInfoRequest| info.monaco_additional_info.is_some(),
    },
];

/// The country blocks `info` carries, in the order [`ADDITIONAL_INFO_BLOCKS`]
/// declares them.
pub fn submitted_additional_info_blocks(
    info: &AdditionalInfoRequest,
) -> Vec<&'static AdditionalInfoBlock> {
    ADDITIONAL_INFO_BLOCKS
        .iter()
        .filter(|block| (block.submitted)(info))
        .collect()
}

/// Refuse an `additionalTaxInformation` object that is not exactly ONE country
/// block, for the jurisdiction the entry itself identifies.
///
/// The shape states this about the object rather than about any one country —
/// "You can only specify one of the following parameters and the value can't be
/// empty", and "The parameter that you specify must match the country for the
/// TRN"
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#AdditionalInfoRequest")`)
/// — so it is checked here rather than as a row of [`COUNTRY_RULES`], whose rows
/// each say something about their own country alone and therefore cannot see a
/// block belonging to another one.
///
/// Every one of the three ways an object can break the rule — no block, more than
/// one block, a block of another jurisdiction — is a value the shape does not
/// admit rather than a member the caller left out, hence
/// [`ERROR_CODE_FIELD_VALIDATION_FAILED`] and [`ADDITIONAL_INFO_INVALID`], the
/// refusal every other rule about this object carries. The parameter named is the
/// object itself where the fault is HOW MANY blocks it carries, and the offending
/// block where the fault is which country that block belongs to.
///
/// An entry whose jurisdiction nothing states ([`entry_country_code`]) is already
/// refused for the address its country would have come from, so only the count is
/// judged for it.
pub fn check_additional_info_blocks(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    let Some(info) = entry.additional_tax_information.as_ref() else {
        return Ok(());
    };
    let submitted = submitted_additional_info_blocks(info);
    let [block] = submitted.as_slice() else {
        return Err(validation(
            ERROR_CODE_FIELD_VALIDATION_FAILED,
            "additionalTaxInformation",
            ADDITIONAL_INFO_INVALID,
        ));
    };
    let Some(country_code) = entry_country_code(entry) else {
        return Ok(());
    };
    if block.country_code.eq_ignore_ascii_case(country_code) {
        return Ok(());
    }
    Err(validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        &format!("additionalTaxInformation.{}", block.member),
        ADDITIONAL_INFO_INVALID,
    ))
}

/// Refuse an entry that does not satisfy the per-country business rules
/// `PutTaxRegistration` documents for the country of its legal address.
///
/// The country is the jurisdiction the entry identifies ([`entry_country_code`]);
/// an entry that identifies none has no country whose rule could be applied. The
/// first unmet rule in [`COUNTRY_RULES`] is the refusal, so the rule set is shared
/// by every writer of a `TaxRegistrationEntry` rather than re-stated per
/// operation.
pub fn check_country_requirements(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    let Some(country_code) = entry_country_code(entry) else {
        return Ok(());
    };
    for rule in COUNTRY_RULES {
        if !rule.country_code.eq_ignore_ascii_case(country_code) {
            continue;
        }
        if !(rule.applies)(entry) || (rule.satisfied)(entry) {
            continue;
        }
        return Err(validation(rule.error_code, rule.member_path, rule.message));
    }
    Ok(())
}

/// Refuse a `TaxRegistrationEntry` no write of this service may store.
///
/// The one place both writers — `PutTaxRegistration` and the per-account loop of
/// `BatchPutTaxRegistration` — decide whether an entry is storable, so the two
/// operations cannot disagree about one entry: the structural refusals (a legal
/// address submitted for a type AWS derives it from, a registration type set for
/// a jurisdiction it does not belong to, a legal name or address the
/// entry's own jurisdiction requires and it omits, a blank value the boundary
/// validator's regex engine could not refuse, an `additionalTaxInformation`
/// object that is not one country block for the entry's own jurisdiction) and
/// then the per-country business rules [`COUNTRY_RULES`] carries. Why each is not
/// left to the boundary validator is spelled out on the helpers this calls and in
/// `ops/put_tax_registration.rs`.
///
/// Everything judged here is a fact about the ENTRY alone, bar the submitted
/// NUMBER, which [`check_registration_number`] judges on its own because the two
/// writers report that refusal on different channels. The refusals that depend
/// on what the account already holds are
/// [`ComAmazonawsTaxsettingsTaxsettingsState::check_registration_type_against_stored`],
/// which each writer applies per account.
pub fn check_tax_registration_entry(
    entry: &TaxRegistrationEntry,
) -> Result<(), ValidationException> {
    // A registration type whose legal address AWS derives from the tax number
    // itself does not take one from the request. Decided FIRST: a member the
    // submitted type does not admit is refused as a field violation before any
    // rule that would be read off that member's value, so a value the caller was
    // not allowed to send is never evidence for another refusal.
    check_registration_type_members(entry)?;

    // A registration type the service ties to one country may be set only for a
    // registration held in that country — the country being the one the entry's
    // own legal address names, which is reached only for a type that accepts a
    // submitted address.
    check_registration_type_jurisdiction(entry)?;

    // Which of the two optional members the entry MUST carry is the jurisdiction's
    // rule, so the jurisdiction is resolved first.
    let country_code = entry_country_code(entry);

    // The legal address is required except where the service fills it in from the
    // tax registration number, and an entry that identifies no jurisdiction at all
    // names no country for either rule to be read from — it is refused for the
    // address, the member its country would have come from.
    if entry.legal_address.is_none() {
        match country_code {
            None => {
                return Err(validation(
                    ERROR_CODE_MISSING_INPUT,
                    "taxRegistrationEntry.legalAddress",
                    MISSING_LEGAL_ADDRESS_JURISDICTION,
                ));
            }
            Some(code) if !legal_address_is_derived(code, &entry.registration_type) => {
                return Err(validation(
                    ERROR_CODE_MISSING_INPUT,
                    "taxRegistrationEntry.legalAddress",
                    MISSING_LEGAL_ADDRESS,
                ));
            }
            Some(_) => {}
        }
    }

    // The legal name is required except where the service fills it in from the tax
    // registration number.
    if entry.legal_name.is_none() && country_code.is_none_or(|code| !legal_name_is_derived(code)) {
        return Err(validation(
            ERROR_CODE_MISSING_INPUT,
            "taxRegistrationEntry.legalName",
            MISSING_LEGAL_NAME,
        ));
    }

    check_not_blank(
        "taxRegistrationEntry.registrationId",
        &entry.registration_id,
    )?;
    if let Some(legal_name) = entry.legal_name.as_deref() {
        check_not_blank("taxRegistrationEntry.legalName", legal_name)?;
    }
    if let Some(legal_address) = entry.legal_address.as_ref() {
        check_address("taxRegistrationEntry.legalAddress", legal_address)?;
    }

    // The `additionalTaxInformation` object's OWN rule — one country block, and
    // the one belonging to this entry's jurisdiction — which the shape states for
    // every country rather than for any one of them, so it is judged before the
    // per-country set whose rows each speak only for their own country.
    check_additional_info_blocks(entry)?;

    // The per-country business rules the model documents for a
    // `TaxRegistrationEntry`, keyed off the jurisdiction it identifies. They run
    // last because they are about an entry that is otherwise storable.
    check_country_requirements(entry)
}

/// Refuse a submitted tax registration NUMBER that is not WRITTEN the way its
/// jurisdiction's authority writes one.
///
/// The authority publishes how a number of that kind is written — its length and
/// its character layout
/// ([`registration_number::satisfies_published_layout`]) — so a value that is
/// not written that way is not a registration number of that type at all, and
/// the write refuses it as it reads the request rather than storing it. Whether
/// a number of that layout is CONFIRMED by the authority's own rule (its check
/// digit) is the separate question the validation a write starts answers
/// ([`trn_validation_status`]): such a number is stored, and its validation
/// reaches its own outcome. A jurisdiction and type this service publishes no
/// layout for is not judged, so such an entry is admitted.
///
/// Its own step, beside [`check_tax_registration_entry`], because the two
/// writers report it on DIFFERENT channels: `PutTaxRegistration` answers the
/// modelled `ValidationException` ([`invalid_registration_number`]), while
/// `BatchPutTaxRegistration` reports it per account in its `errors` list — the
/// channel that names the accounts the TRN information could not be added to —
/// and still answers 200. Judged LAST of the entry's facts, after
/// [`check_tax_registration_entry`], so a request that also breaks a rule about
/// the entry's members is told about that member rather than about the number.
pub fn check_registration_number(
    entry: &TaxRegistrationEntry,
) -> Result<(), InvalidRegistrationNumber> {
    let Some(country_code) = entry_country_code(entry) else {
        return Ok(());
    };
    check_number(country_code, &entry.registration_type, &entry.registration_id)
}

/// The same refusal as [`check_registration_number`] for a SUPPLEMENTAL TRN:
/// one number is judged one way whichever kind of registration holds it, the
/// jurisdiction being the country of the address the entry carries.
pub fn check_supplemental_registration_number(
    entry: &SupplementalTaxRegistrationEntry,
) -> Result<(), InvalidRegistrationNumber> {
    check_number(
        &entry.address.country_code,
        &entry.registration_type,
        &entry.registration_id,
    )
}

/// The one judgement both number checks make.
fn check_number(
    country_code: &str,
    registration_type: &str,
    registration_id: &str,
) -> Result<(), InvalidRegistrationNumber> {
    if registration_number::satisfies_published_layout(
        country_code,
        registration_type,
        registration_id,
    ) {
        Ok(())
    } else {
        Err(InvalidRegistrationNumber)
    }
}

/// Why a write of a tax registration was refused: the submitted number is not
/// one its jurisdiction's authority issues — see [`check_registration_number`].
///
/// Its own refusal class, rather than a bare `ValidationException`, because what
/// each writer reports for it differs: the message is the class's own token
/// ([`INVALID_TAX_REGISTRATION_NUMBER`]) whichever path renders it, and a
/// per-account row of this class carries NO `code` (see [`BatchRowErrorCode`]).
#[derive(Debug, Clone, Copy)]
pub struct InvalidRegistrationNumber;

impl InvalidRegistrationNumber {
    /// The refusal's message: the token this service words the class with, one
    /// wording whichever path renders it.
    pub fn message(self) -> String {
        INVALID_TAX_REGISTRATION_NUMBER.to_string()
    }

    /// The modelled `ValidationException` (HTTP 400) a single-record write
    /// answers this refusal with.
    pub fn into_validation(self) -> ValidationException {
        invalid_registration_number()
    }
}

/// This service publishes no code for an invalid tax registration number: the
/// class's token is the row's `message`, and a recorded row of this class
/// carries no `code` at all — so the member is left off the wire rather than
/// filled with the `errorCode` of the exception the single-record write renders.
impl BatchRowErrorCode for InvalidRegistrationNumber {
    fn row_error_code(&self) -> Option<String> {
        None
    }
}

/// Whether this emulator stores a supplemental TRN for `country_code`, with the
/// country's name — see [`SUPPLEMENTAL_TRN_COUNTRIES`].
pub fn supplemental_country(country_code: &str) -> Option<&'static str> {
    SUPPLEMENTAL_TRN_COUNTRIES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country_code))
        .map(|(_, country)| *country)
}

/// Refuse a `SupplementalTaxRegistrationEntry` for a jurisdiction this service
/// does not store a supplemental TRN for ([`SUPPLEMENTAL_TRN_COUNTRIES`]).
///
/// The members the boundary validator's regex engine could not refuse (the
/// non-blank pattern on `registrationId`, `legalName` and the address lines) are
/// checked by the handler through [`check_not_blank`] / [`check_address`]; this
/// is the jurisdiction boundary alone.
pub fn check_supplemental_jurisdiction(
    entry: &SupplementalTaxRegistrationEntry,
) -> Result<(), ValidationException> {
    if supplemental_country(&entry.address.country_code).is_some() {
        return Ok(());
    }
    Err(validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        "taxRegistrationEntry.address.countryCode",
        SUPPLEMENTAL_COUNTRY_NOT_SUPPORTED,
    ))
}

/// This service's modelled `ValidationException` for an `authorityId` that names
/// no live supplemental registration of the caller's account.
///
/// `DeleteSupplementalTaxRegistration` also models `ResourceNotFoundException`,
/// but the real service answers an unknown `authorityId` — a junk string and a
/// well-formed UUID alike — with the 400 class, so this is the refusal both get:
/// an `authorityId` is valid only while it names a registration. The model's
/// two doc strings do not choose between the errors; the recorded units
/// `taxsettings.delete-supplemental-tax-registration-not-found` and
/// `taxsettings.concurrent-delete-supplemental-tax-registration-consistent` do,
/// and `tests/all/missing_resource_errors.rs` pins their outcome against the
/// 404 the `accountId`-addressed operations answer, so the two classes cannot be
/// swapped by re-reading the docs. A later fixer holding a recording that
/// disagrees should follow the recording.
pub fn invalid_authority_id() -> ValidationException {
    validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        "authorityId",
        "Provided authorityId is not valid.",
    )
}

/// Whether a tax registration held in `country_code` is one this API cannot
/// remove, with the country's name — see
/// [`COUNTRIES_WITH_NON_API_REGISTRATION_DELETE`].
pub fn non_api_delete_country(country_code: &str) -> Option<&'static str> {
    COUNTRIES_WITH_NON_API_REGISTRATION_DELETE
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country_code))
        .map(|(_, country)| *country)
}

/// Refuse the deletion of a stored registration held in a jurisdiction this API
/// cannot remove one from ([`COUNTRIES_WITH_NON_API_REGISTRATION_DELETE`]).
///
/// The jurisdiction step of the one delete decision
/// ([`ComAmazonawsTaxsettingsTaxsettingsState::delete_registration`]), which
/// both delete paths take, so the two operations cannot disagree about one
/// record. The 400 class and the `ValidationException` shape are the contract
/// both declare; the wording states the documented carve-out, naming the
/// jurisdiction of the record the call addressed, and `accountId` is the
/// parameter that selected it.
pub fn check_registration_deletable(
    stored: &StoredTaxRegistration,
) -> Result<(), ValidationException> {
    match non_api_delete_country(&stored.legal_address.country_code) {
        Some(country) => Err(validation(
            ERROR_CODE_FIELD_VALIDATION_FAILED,
            "accountId",
            format!(
                "This API operation can't be used to delete your tax registration in {country}. \
                 Use the Payment preferences page in the Billing and Cost Management console \
                 instead."
            ),
        )),
        None => Ok(()),
    }
}

/// Why a delete of ONE account's tax registration was refused, as the one delete
/// decision ([`ComAmazonawsTaxsettingsTaxsettingsState::delete_registration`])
/// answers it.
///
/// It names the modelled error `DeleteTaxRegistration` raises, which is what each
/// caller renders on its own channel: the single-account delete returns that
/// error, and the batch reports a per-account row of its `errors` list carrying
/// the same message and the code that error publishes (see
/// [`BatchRowErrorCode`]). So a refusal added to the decision reaches both
/// paths, and neither path decides on its own what a delete of one record
/// answers.
#[derive(Debug)]
pub enum RegistrationDeleteRefusal {
    /// The account holds no tax registration — [`no_tax_registration`].
    NotFound(ResourceNotFoundException),
    /// The record is held in a jurisdiction this API cannot remove one from —
    /// [`check_registration_deletable`].
    NotDeletable(ValidationException),
}

impl RegistrationDeleteRefusal {
    /// The message the refusal is reported with on the channel that renders it.
    ///
    /// A refusal the API answers as an EXCEPTION carries that exception's own
    /// sentence; the per-account row of the batch delete carries the token this
    /// service words the class with ([`TAX_REGISTRATION_NOT_FOUND`]), which is
    /// what a recorded row of the missing-registration class carries. The
    /// jurisdiction carve-out has no such token, so its row carries the
    /// exception's sentence.
    pub fn into_row_message(self) -> String {
        match self {
            Self::NotFound(_) => TAX_REGISTRATION_NOT_FOUND.to_string(),
            Self::NotDeletable(error) => error.message,
        }
    }
}

/// The `code` a batch operation reports in the per-account `errors[]` row that
/// renders this refusal.
///
/// Both batch error shapes target their `code` at the model's free-form
/// `ErrorCode` — the very shape this service's exceptions target their
/// `errorCode` at — so the member carries a SERVICE-DEFINED code and never an
/// exception's own shape name: the wire already carries that in
/// `x-amzn-errortype` / `__type`, and a client reading `code` to classify a
/// failure would take a shape name for a service code. A refusal class this
/// service publishes no code for therefore leaves the member off the wire,
/// exactly as those exceptions leave `errorCode` off it (see
/// `ops/types/shared_types.rs`); `code` is optional in both shapes, so there is
/// nothing to invent.
pub trait BatchRowErrorCode {
    /// The code to report, or `None` when this refusal class publishes none.
    fn row_error_code(&self) -> Option<String>;
}

/// A validation refusal publishes its own code, the discriminator this service
/// reports for the class of refusal it is (see [`validation`]).
impl BatchRowErrorCode for ValidationException {
    fn row_error_code(&self) -> Option<String> {
        (!self.error_code.is_empty()).then(|| self.error_code.clone())
    }
}

/// A state conflict publishes one code for the whole class
/// ([`ERROR_CODE_GENERAL_CONFLICT`]), so a row rendering one reports it, exactly
/// as the 409 response does.
impl BatchRowErrorCode for ConflictException {
    fn row_error_code(&self) -> Option<String> {
        (!self.error_code.is_empty()).then(|| self.error_code.clone())
    }
}

/// This service publishes no code for a missing resource, for the reason
/// [`no_tax_registration`] states.
impl BatchRowErrorCode for ResourceNotFoundException {
    fn row_error_code(&self) -> Option<String> {
        None
    }
}

/// A per-account delete refusal reports the code of the exception it renders, so
/// the batch row and the single-account delete publish the same discriminator.
impl BatchRowErrorCode for RegistrationDeleteRefusal {
    fn row_error_code(&self) -> Option<String> {
        match self {
            Self::NotFound(error) => error.row_error_code(),
            Self::NotDeletable(error) => error.row_error_code(),
        }
    }
}

/// This service's modelled `ResourceNotFoundException` (HTTP 404) for an account
/// that has no stored tax registration.
///
/// It carries its `message` and nothing else. `errorCode` on this service's
/// exceptions is a SERVICE-DEFINED code, never the exception's own shape name —
/// the wire already carries that in `x-amzn-errortype` — and this service
/// publishes no code for a missing resource (`ErrorCode` is a free-form string,
/// unlike the `ValidationExceptionErrorCode` enum), so the member is left off the
/// wire rather than filled with the shape name; see `ops/types/shared_types.rs`.
/// The message names no account: the request that asked already names it. It is
/// the sentence the EXCEPTION carries; the per-account row of the batch delete
/// carries the class's own token instead
/// ([`RegistrationDeleteRefusal::into_row_message`]), which is how a recording
/// of each channel words this class.
pub fn no_tax_registration() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "No tax registration found".to_string(),
    }
}

/// This service's modelled `ConflictException` (HTTP 409) for a state conflict
/// the service words no sentence of its own for.
///
/// The class publishes one code ([`ERROR_CODE_GENERAL_CONFLICT`]) and this
/// condition's message is the class's general sentence
/// ([`STATE_CONFLICT_MESSAGE`]), neither of which interpolates anything from the
/// request or from state. WHICH state conflicts is the business of the call
/// sites — the resident bounds [`MAX_STORED_REGISTRATIONS`] /
/// [`MAX_STORED_SUPPLEMENTAL_REGISTRATIONS`], and the account state a
/// supplemental registration supplements — each of which documents its own rule
/// where it applies it. A write refused because the account's own TRN is still
/// awaiting validation carries the sentence this service words for THAT
/// condition instead ([`pending_registration_conflict`]).
pub fn state_conflict() -> ConflictException {
    ConflictException {
        message: STATE_CONFLICT_MESSAGE.to_string(),
        error_code: ERROR_CODE_GENERAL_CONFLICT.to_string(),
    }
}

/// This service's modelled `ConflictException` (HTTP 409) for a write of an
/// account's tax registration aimed at an account whose stored TRN is still
/// awaiting validation — the refusal [`ComAmazonawsTaxsettingsTaxsettingsState::store_registration`]
/// makes, rendered by `PutTaxRegistration` on its own error channel and by the
/// per-account rows of `BatchPutTaxRegistration`.
///
/// Same class and therefore the same published code as [`state_conflict`], and
/// the message this service words for the pending-TRN condition
/// ([`PENDING_REGISTRATION_CONFLICT_MESSAGE`]): a caller is told which state
/// conflicted by the message, because the code is the whole class's.
pub fn pending_registration_conflict() -> ConflictException {
    ConflictException {
        message: PENDING_REGISTRATION_CONFLICT_MESSAGE.to_string(),
        error_code: ERROR_CODE_GENERAL_CONFLICT.to_string(),
    }
}

/// This service's modelled `ValidationException` (HTTP 400) for a submitted tax
/// registration number that is not written the way its jurisdiction's authority
/// writes one — see [`check_registration_number`], which is where the judgement
/// is made.
///
/// The message is the class's own token, which a recorded per-account row of
/// this class carries; what AWS words the SINGLE-account 400 with is not
/// recorded, so carrying the same token here is this emulator's inference from
/// the row — a later fixer holding a recording of this path should follow it.
pub fn invalid_registration_number() -> ValidationException {
    validation(
        ERROR_CODE_FIELD_VALIDATION_FAILED,
        "taxRegistrationEntry.registrationId",
        INVALID_TAX_REGISTRATION_NUMBER,
    )
}

/// This service's modelled `InternalServerException` (HTTP 500) for a handler
/// mounted on an engine that holds no state for this service. A handler answers
/// this rather than building an engine of its own, which would give that one
/// request a private store. It carries its `message` only, for the reason
/// [`no_tax_registration`] gives.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "This service's state store is unavailable.".to_string(),
    }
}

/// The opaque page cursor naming the last store key a page emitted — an account
/// id for `ListTaxRegistrations`, an `authorityId` for
/// `ListSupplementalTaxRegistrations`.
///
/// Resuming from it means "the first key strictly greater than this one", which
/// is well defined because both
/// [`ComAmazonawsTaxsettingsTaxsettingsState::registrations`] and
/// [`ComAmazonawsTaxsettingsTaxsettingsState::supplemental_registrations`] are
/// ordered.
///
/// Encoded with `aws_smithy_types::base64`, the codec the wire protocols already
/// use for this service's `blob` members. Its standard alphabet (`A-Za-z0-9+/`)
/// and `=` padding both lie inside the model's `PaginationTokenString` pattern
/// `^[-A-Za-z0-9_+\=\/]+$`, so every token issued here satisfies the constraint
/// the next request's `nextToken` is validated against.
pub fn encode_page_token(last_key: &str) -> String {
    base64::encode(last_key)
}

/// The account id a page cursor names, or a `ValidationException` when the token
/// was not minted by [`encode_page_token`].
pub fn decode_page_token(token: &str) -> Result<String, ValidationException> {
    let account_id = decode_page_token_key(token)?;
    if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(validation(
            ERROR_CODE_INVALID_TOKEN,
            "nextToken",
            "nextToken does not name an account this service can resume from.",
        ));
    }
    Ok(account_id)
}

/// One page of a listing: the entries it reports, in store order, and the cursor
/// a caller continues from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The entries this page reports, at most the page size asked of
    /// [`build_page`] of them, in the order the store enumerates them.
    pub items: Vec<T>,
    /// The cursor naming the last key this page emitted — issued only when a
    /// further entry remains, so the last page carries none.
    pub next_token: Option<String>,
}

/// Build one page of a listing over an ordered store: resume past the key the
/// cursor names, report up to `page_size` entries, and issue a cursor naming the
/// last key of the page only when a further entry remains.
///
/// The ONE implementation of this service's page-build rule, so every listing
/// pages the same way and a change to paging — a new listing, or a change to how
/// a cursor is minted ([`encode_page_token`], beside this) — has a single place
/// to touch.
///
/// `project` turns a stored entry into the shape its listing reports, and
/// answers `None` for one with nothing to report: such an entry is skipped
/// rather than reported empty, and its key never becomes a cursor.
///
/// Resuming uses the ordered map's own `range`, so a cursor's page costs a
/// lookup rather than a scan of everything before it, and the work done here is
/// bounded by the page — `page_size` reported entries, plus the entries
/// `project` skips and the one peeked at to decide the cursor — not by the size
/// of the store. What a CALLER of this does under the state lock besides
/// building the page (settling what the clock owes, say) is its own cost.
///
/// `page_size` must be at least one: a page of none would report nothing and
/// carry no cursor, leaving a caller unable to continue. Every caller takes it
/// from [`PageBounds::page_size`], which never answers zero, and the assertion
/// below keeps that where the rule lives rather than at each listing.
pub fn build_page<V, T, F>(
    source: &BTreeMap<String, V>,
    resume_after: Option<&str>,
    page_size: usize,
    mut project: F,
) -> Page<T>
where
    F: FnMut(&str, &V) -> Option<T>,
{
    debug_assert!(page_size > 0, "a page carries at least one entry");
    let lower = match resume_after {
        Some(after) => Bound::Excluded(after),
        None => Bound::Unbounded,
    };
    let mut remaining = source
        .range::<str, _>((lower, Bound::Unbounded))
        .filter_map(|(key, stored)| project(key, stored).map(|item| (key.as_str(), item)))
        .peekable();
    let mut items = Vec::new();
    let mut last_key: Option<&str> = None;
    for (key, item) in remaining.by_ref().take(page_size) {
        last_key = Some(key);
        items.push(item);
    }
    let next_token = if remaining.peek().is_some() {
        last_key.map(encode_page_token)
    } else {
        None
    };
    Page { items, next_token }
}

// ---------------------------------------------------------------------------
// The tax exemption type catalog
//
// `GetTaxExemptionTypes` answers "the supported types of tax exemptions": a
// collection the SERVICE publishes, the same for every account, that no
// operation of this service creates, updates or deletes. So the catalog below IS
// that collection — the same way the API Gateway emulator's `SdkType` collection
// is a published table rather than a stored record — and the handler projects it
// into the modelled shapes instead of reading an account's records.
// ---------------------------------------------------------------------------

/// One jurisdiction an exemption type applies in, as the catalog spells it:
/// `(country, state)` — the two members of the model's `Authority` shape
/// (`smithy_query(query="shape", shape="Authority")`: `country` is a required
/// two-letter code, `state` an optional name). `None` means the type applies
/// across the whole country rather than in one state of it.
pub type JurisdictionRow = (&'static str, Option<&'static str>);

/// One entry of the published catalog:
/// `(displayName, description, applicableJurisdictions)` — the three members of
/// the model's `TaxExemptionType` shape.
pub type TaxExemptionTypeRow = (&'static str, &'static str, &'static [JurisdictionRow]);

/// The tax exemption types this service supports, each with the jurisdictions it
/// applies in.
///
/// Hand-written because the model declares no shape enumerating them: `TaxExemptionType`
/// is a free-form structure, and the only thing the model says about which values are
/// legal is `PutTaxExemptionRequest$exemptionType`'s "Use the supported tax exemption type
/// description" (`smithy_query(query="doc", shape="PutTaxExemptionRequest",
/// member="exemptionType")`) — i.e. the `description` of an entry of THIS catalog is the
/// token a caller names an exemption type by, so the descriptions below are the catalog's
/// identifying values and are kept distinct from one another.
///
/// What is claimed to match AWS:
///
/// * the FIRST entry outright. The service's own catalog token for the charitable
///   category is `501C`, carried by both `displayName` and `description`, and it applies
///   in the states [`CHARITABLE_ORGANIZATION_JURISDICTIONS`] names — one `Authority` per
///   state, as a recorded `GetTaxExemptionTypes` response publishes it.
/// * the SHAPE of every entry: `applicableJurisdictions` enumerates one `Authority` per
///   STATE the type applies in, because a US sales-and-use-tax exemption is granted by a
///   state's own taxing authority and the category is not offered in every state — which
///   is what the authority check of a submission reads
///   ([`check_exemption_authority`]).
/// * the categories themselves: those AWS accepts a certificate for (AWS Billing and Cost
///   Management User Guide, "Tax exemption" / "Managing your tax exemptions": tax
///   exemptions are offered for United States sales tax, under the certificate categories
///   a reseller, a direct-pay permit holder, a federal, state, local or tribal government
///   body, an educational institution or a charitable organization holds).
///
/// What is NOT claimed: the `displayName` and `description` WORDING of every entry but the
/// first, the per-state list of those entries, and the number and order of the entries.
/// The model states no sentence for either member, so each is this emulator's own phrasing
/// of a documented category, sorted by `displayName` so the collection is enumerated one
/// stable way; the states those categories carry are the jurisdictions that levy the tax
/// at all ([`US_SALES_TAX_JURISDICTIONS`]), which is the widest list a category can have
/// and the only one that follows from a published fact. A client that keys off the prose
/// rather than off a type it already read from this catalog is reading wording the real
/// service words differently.
///
/// A submission is still admitted on the constraints the model states for `exemptionType`
/// whether or not this table spells its token the same way (see
/// [`submitted_exemption_type`]); what the table decides about a write is where a type it
/// DOES publish applies ([`check_exemption_authority`]).
pub const TAX_EXEMPTION_TYPE_CATALOG: &[TaxExemptionTypeRow] = &[
    ("501C", "501C", CHARITABLE_ORGANIZATION_JURISDICTIONS),
    (
        "Direct Pay Permit",
        "Holder of a direct pay permit that remits sales and use tax directly to the taxing \
         authority",
        US_SALES_TAX_JURISDICTIONS,
    ),
    (
        "Educational Institution",
        "Accredited school, college or university exempt from sales and use tax",
        US_SALES_TAX_JURISDICTIONS,
    ),
    (
        "Federal Government",
        "US federal government body exempt from sales and use tax",
        US_SALES_TAX_JURISDICTIONS,
    ),
    (
        "Reseller",
        "Purchases for resale, held under a resale certificate",
        US_SALES_TAX_JURISDICTIONS,
    ),
    (
        "State or Local Government",
        "US state, county or municipal government body exempt from sales and use tax",
        US_SALES_TAX_JURISDICTIONS,
    ),
    (
        "Tribal Government",
        "Federally recognized tribal government body exempt from sales and use tax",
        US_SALES_TAX_JURISDICTIONS,
    ),
];

/// The jurisdictions the charitable-organization entry of
/// [`TAX_EXEMPTION_TYPE_CATALOG`] applies in — one `Authority` per state, as a
/// recorded `GetTaxExemptionTypes` response publishes them for that type.
pub const CHARITABLE_ORGANIZATION_JURISDICTIONS: &[JurisdictionRow] = &[
    ("US", Some("AZ")),
    ("US", Some("CO")),
    ("US", Some("CT")),
    ("US", Some("DC")),
    ("US", Some("FL")),
    ("US", Some("GA")),
    ("US", Some("ID")),
    ("US", Some("IL")),
    ("US", Some("IN")),
    ("US", Some("KS")),
    ("US", Some("KY")),
    ("US", Some("LA")),
    ("US", Some("MA")),
    ("US", Some("MD")),
    ("US", Some("MI")),
    ("US", Some("MN")),
    ("US", Some("MO")),
    ("US", Some("MS")),
    ("US", Some("ND")),
    ("US", Some("NE")),
    ("US", Some("NJ")),
    ("US", Some("NV")),
    ("US", Some("NY")),
    ("US", Some("OH")),
    ("US", Some("OK")),
    ("US", Some("PA")),
    ("US", Some("RI")),
    ("US", Some("SC")),
    ("US", Some("SD")),
    ("US", Some("TN")),
    ("US", Some("TX")),
    ("US", Some("UT")),
    ("US", Some("VA")),
    ("US", Some("VT")),
    ("US", Some("WI")),
    ("US", Some("WV")),
    ("US", Some("WY")),
];

/// The jurisdictions every other entry of [`TAX_EXEMPTION_TYPE_CATALOG`] applies
/// in: the US jurisdictions that levy a general sales tax at all, by their
/// two-letter postal codes — the fifty states and the District of Columbia less
/// Alaska, Delaware, Montana, New Hampshire and Oregon, which impose no general
/// sales tax, so no exemption from one is granted there.
pub const US_SALES_TAX_JURISDICTIONS: &[JurisdictionRow] = &[
    ("US", Some("AL")),
    ("US", Some("AR")),
    ("US", Some("AZ")),
    ("US", Some("CA")),
    ("US", Some("CO")),
    ("US", Some("CT")),
    ("US", Some("DC")),
    ("US", Some("FL")),
    ("US", Some("GA")),
    ("US", Some("HI")),
    ("US", Some("IA")),
    ("US", Some("ID")),
    ("US", Some("IL")),
    ("US", Some("IN")),
    ("US", Some("KS")),
    ("US", Some("KY")),
    ("US", Some("LA")),
    ("US", Some("MA")),
    ("US", Some("MD")),
    ("US", Some("ME")),
    ("US", Some("MI")),
    ("US", Some("MN")),
    ("US", Some("MO")),
    ("US", Some("MS")),
    ("US", Some("NC")),
    ("US", Some("ND")),
    ("US", Some("NE")),
    ("US", Some("NJ")),
    ("US", Some("NM")),
    ("US", Some("NV")),
    ("US", Some("NY")),
    ("US", Some("OH")),
    ("US", Some("OK")),
    ("US", Some("PA")),
    ("US", Some("RI")),
    ("US", Some("SC")),
    ("US", Some("SD")),
    ("US", Some("TN")),
    ("US", Some("TX")),
    ("US", Some("UT")),
    ("US", Some("VA")),
    ("US", Some("VT")),
    ("US", Some("WA")),
    ("US", Some("WI")),
    ("US", Some("WV")),
    ("US", Some("WY")),
];

/// The catalog as the modelled `TaxExemptionType` list `GetTaxExemptionTypes`
/// reports, in [`TAX_EXEMPTION_TYPE_CATALOG`]'s order.
///
/// Every member is reported: the collection exists to tell a caller which types
/// it may name and where each one applies, and an omitted member would leave that
/// unanswerable.
pub fn tax_exemption_type_catalog() -> Vec<TaxExemptionType> {
    TAX_EXEMPTION_TYPE_CATALOG
        .iter()
        .map(exemption_type_of)
        .collect()
}

/// One published row as the modelled `TaxExemptionType` shape, which is how both
/// the catalog listing and a stored exemption report the type it was granted
/// under: one projection, so the two cannot describe one type differently.
pub fn exemption_type_of(
    (display_name, description, jurisdictions): &TaxExemptionTypeRow,
) -> TaxExemptionType {
    TaxExemptionType {
        display_name: Some((*display_name).to_string()),
        description: Some((*description).to_string()),
        applicable_jurisdictions: Some(
            jurisdictions
                .iter()
                .map(|(country, state)| Authority {
                    country: (*country).to_string(),
                    state: state.map(std::string::ToString::to_string),
                })
                .collect(),
        ),
    }
}

/// The published row a caller names by `exemption_type`, or `None` for a token
/// that names no entry of the collection this service publishes.
///
/// `PutTaxExemptionRequest$exemptionType` documents "Use the supported tax
/// exemption type description", so the `description` of a
/// [`TAX_EXEMPTION_TYPE_CATALOG`] entry is the token a type is named by; the
/// `displayName` is accepted for the same entry, because it is the catalog's
/// other identifying member. Matching ignores case and surrounding whitespace,
/// neither of which distinguishes one published type from another.
///
/// This PLACES a token in the published collection; it never decides whether a
/// submission is admitted — see [`submitted_exemption_type`].
pub fn find_exemption_type(exemption_type: &str) -> Option<&'static TaxExemptionTypeRow> {
    let wanted = exemption_type.trim();
    TAX_EXEMPTION_TYPE_CATALOG
        .iter()
        .find(|(display_name, description, _)| {
            description.eq_ignore_ascii_case(wanted) || display_name.eq_ignore_ascii_case(wanted)
        })
}

/// The `TaxExemptionType` a submission of `exemption_type` in `authority`
/// records, and that every reader of the record then reports.
///
/// `exemptionType` is a free-form string the model constrains only in length
/// (1..=200) and by a pattern that admits every character
/// (`smithy_query(query="operation", operation="PutTaxExemption")`), and
/// `TaxExemptionType` is a structure of free text rather than an enumeration
/// (`smithy_query(query="shape", shape="TaxExemptionType")`): the model
/// publishes no set of legal values, so a submitted token is admitted on the
/// constraints the model states and recorded as the caller spelled it.
///
/// A token that names an entry of the published collection is recorded AS that
/// entry, so the type an exemption is held under reads back the same whichever
/// of the entry's two identifying members the caller named it by, and with the
/// jurisdictions the collection publishes for it. Any other token is recorded as
/// submitted, applicable in the jurisdiction this submission names — the only
/// jurisdiction this service is told the type applies in. The token is its
/// `displayName` too where the member's 0..50 bound admits it; a longer token
/// has none, because a truncation would report a name no caller sent.
pub fn submitted_exemption_type(exemption_type: &str, authority: &Authority) -> TaxExemptionType {
    if let Some(published) = find_exemption_type(exemption_type) {
        return exemption_type_of(published);
    }
    TaxExemptionType {
        display_name: (exemption_type.chars().count() <= MAX_TYPE_DISPLAY_NAME)
            .then(|| exemption_type.to_string()),
        description: Some(exemption_type.to_string()),
        applicable_jurisdictions: Some(vec![normalized_authority(authority)]),
    }
}

/// The longest `TaxExemptionType$displayName` the model admits: `@length` 0..50
/// (`smithy_query(query="shape", shape="TaxExemptionType")`), against the
/// 1..=200 of the `exemptionType` a submission names a type by.
const MAX_TYPE_DISPLAY_NAME: usize = 50;

/// Refuse a submission whose `authority` is not a jurisdiction the exemption
/// type it names applies in.
///
/// A US sales-and-use-tax exemption is granted by the taxing authority of the
/// jurisdiction it is claimed in, and the published catalog says for each type
/// which jurisdictions those are
/// ([`TAX_EXEMPTION_TYPE_CATALOG`]`$applicableJurisdictions`), so a submission
/// that claims a type in a jurisdiction the type does not apply in has nothing
/// to be granted under: it is refused with the modelled `ValidationException`
/// (HTTP 400), the `errorCode` for a field that failed validation, the fixed
/// text of that refusal class ([`EXEMPTION_AUTHORITY_NOT_SUPPORTED`]) and
/// `fieldList` naming the offending member — the `authority`.
///
/// A jurisdiction is one the type applies in when the catalog lists it for the
/// type's country: the whole country where the entry names no state, that state
/// where the submission names one, and the country where the submission names no
/// state at all — such a submission claims the exemption for the country, which
/// the type does apply in. Case and padding distinguish no jurisdiction, so they
/// are ignored, exactly as they are where a stored exemption's jurisdiction is
/// compared ([`is_same_exemption`]).
///
/// The catalog publishes a state as the two-letter postal code its taxing
/// authority is named by, and a recorded refusal of this class names the state
/// the same way, so that is the form a submission is compared in: a value
/// naming the state some other way names no jurisdiction the published list
/// carries. A recording showing a state NAME admitted would widen this
/// comparison, and nothing else.
///
/// A token that names no published entry is not judged here: the collection says
/// nothing about where a type it does not publish applies, and the submission
/// itself is the only thing that does ([`submitted_exemption_type`]).
pub fn check_exemption_authority(
    exemption_type: &str,
    authority: &Authority,
) -> Result<(), ValidationException> {
    let Some((_, _, jurisdictions)) = find_exemption_type(exemption_type) else {
        return Ok(());
    };
    let country = authority.country.trim();
    let claimed_state = authority.state.as_deref().map(str::trim);
    let applies = jurisdictions.iter().any(|(published_country, published_state)| {
        published_country.eq_ignore_ascii_case(country)
            && match (published_state, claimed_state) {
                (None, _) | (Some(_), None) => true,
                (Some(published), Some(claimed)) => published.eq_ignore_ascii_case(claimed),
            }
    });
    if applies {
        Ok(())
    } else {
        Err(validation(
            ERROR_CODE_FIELD_VALIDATION_FAILED,
            "authority",
            EXEMPTION_AUTHORITY_NOT_SUPPORTED,
        ))
    }
}

// ---------------------------------------------------------------------------
// Writing a tax exemption
//
// `PutTaxExemption` is the one writer of
// [`ComAmazonawsTaxsettingsTaxsettingsState::exemptions`]; the record it stores
// and the projection every reader of one reports are [`StoredTaxExemption`],
// above. What lives here is what a submission needs on top of that record: the
// bound the store keeps, and the identity two submissions share.
// ---------------------------------------------------------------------------

/// The most tax exemption records this emulator keeps resident, across every
/// account.
///
/// `accountIds` is caller-supplied (up to 550 ids per call), so without a bound a
/// caller could grow the store by hundreds of records per request, and this
/// service models no operation that deletes an exemption. A submission that would
/// ADD records past this bound is refused with `PutTaxExemption`'s modelled
/// `CaseCreationLimitExceededException` — the refusal AWS gives when no further
/// support case may be opened, and each submission opens one — while replacing
/// exemptions the accounts already hold is always allowed, because it adds
/// nothing.
pub const MAX_STORED_EXEMPTIONS: usize = 1_000;

/// `authority` as a record stores it: the country upper-cased, because
/// `Authority$country` is a two-letter code whose case carries no meaning, and
/// the state trimmed but otherwise as submitted, being a name.
pub fn normalized_authority(authority: &Authority) -> Authority {
    Authority {
        country: authority.country.trim().to_uppercase(),
        state: authority
            .state
            .as_deref()
            .map(|state| state.trim().to_string()),
    }
}

/// Whether `stored` is the same exemption as the one `description` and
/// `authority` name.
///
/// The type and the jurisdiction are the identity a submission addresses — the
/// request carries no exemption id — so a second submission of the same pair
/// replaces the account's record rather than adding one beside it. `description`
/// is the recorded type's own ([`submitted_exemption_type`]), so naming a
/// published entry by either of its identifying members addresses one record.
/// Case and padding make no second exemption either, in the type any more than
/// in the jurisdiction: every member of the identity is compared the way
/// [`find_exemption_type`] places a token and [`normalized_authority`] stores a
/// jurisdiction — ignoring case and surrounding whitespace — so what the store
/// treats as one type in one jurisdiction it treats as one record.
pub fn is_same_exemption(
    stored: &StoredTaxExemption,
    description: &str,
    authority: &Authority,
) -> bool {
    let same_state = match (stored.authority.state.as_deref(), authority.state.as_deref()) {
        (Some(held), Some(submitted)) => held.trim().eq_ignore_ascii_case(submitted.trim()),
        (None, None) => true,
        _ => false,
    };
    stored
        .exemption_type
        .description
        .as_deref()
        .is_some_and(|held| held.trim().eq_ignore_ascii_case(description.trim()))
        && stored
            .authority
            .country
            .trim()
            .eq_ignore_ascii_case(authority.country.trim())
        && same_state
}

/// This service's modelled `CaseCreationLimitExceededException` (HTTP 413) for a
/// submission that would take the store past [`MAX_STORED_EXEMPTIONS`]. It
/// carries its `message` only, for the reason [`no_tax_registration`] gives.
pub fn exemption_store_full() -> CaseCreationLimitExceededException {
    CaseCreationLimitExceededException {
        message: format!(
            "This emulator keeps at most {MAX_STORED_EXEMPTIONS} tax exemptions resident, so no \
             further exemption case can be created."
        ),
    }
}

/// The `authorityId` a supplemental-listing page cursor names, or a
/// `ValidationException` when the token was not minted by
/// [`encode_page_token`].
///
/// The key a supplemental page resumes from is an `authorityId`, which the model
/// constrains as a `GenericString` of 1..=200 characters, so a token carrying
/// anything outside that bound names nothing this listing could resume from.
pub fn decode_supplemental_page_token(token: &str) -> Result<String, ValidationException> {
    let authority_id = decode_page_token_key(token)?;
    if authority_id.is_empty() || authority_id.chars().count() > 200 {
        return Err(validation(
            ERROR_CODE_INVALID_TOKEN,
            "nextToken",
            "nextToken does not name a supplemental tax registration this service can resume \
             from.",
        ));
    }
    Ok(authority_id)
}

/// The store key a page cursor carries, whatever kind of key that listing pages
/// by: the text [`encode_page_token`] encoded, or a `ValidationException` when
/// the token does not decode at all.
fn decode_page_token_key(token: &str) -> Result<String, ValidationException> {
    let malformed = || {
        validation(
            ERROR_CODE_MALFORMED_TOKEN,
            "nextToken",
            "nextToken is not a pagination token this service issued.",
        )
    };
    let decoded = base64::decode(token).map_err(|_| malformed())?;
    String::from_utf8(decoded).map_err(|_| malformed())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsTaxsettingsTaxsettingsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsTaxsettingsTaxsettingsState>)
}
