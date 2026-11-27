// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-account-account service.
//!
//! The account record is this service's primary resource: `PutAccountName`,
//! `PutAlternateContact` and `PutContactInformation` write it,
//! `GetAccountInformation`, `GetAlternateContact` and `GetContactInformation`
//! report it, and `DeleteAlternateContact` removes one of its contacts. The
//! record below is the one copy every one of those handlers reads and writes.
//! It also carries the account's most recent primary email update, which
//! `StartPrimaryEmailUpdate` records, `AcceptPrimaryEmailUpdate` confirms and
//! `GetPrimaryEmailUpdateStatus` reports, the primary email address every
//! account carries and a confirmed update replaces once it completed, which
//! `GetPrimaryEmail` reports, and the phone-number
//! verification of its primary contact, which `SendPhoneNumberVerification`
//! starts, `VerifyPhoneNumber` completes and `GetContactInformation` reports as
//! `VerificationStatus` ([`PhoneNumberVerification`]).
//!
//! Most Account Management operations take an OPTIONAL `AccountId`: absent, the
//! call lands on "the Amazon Web Services account of the identity used to call
//! the operation" — which for this emulator is
//! [`::emulator_server::emulator_account_id`]. Present, it names a MEMBER
//! account of the caller's organization, which the caller's own account is not:
//! that is [`target_account_id`], the one resolution every operation taking an
//! account id goes through. An operation that acts on an account which must
//! ALREADY exist refuses an id this emulator holds no record of in one place
//! too ([`ComAmazonawsAccountAccountState::accessible_account`], and
//! [`ComAmazonawsAccountAccountState::accessible_account_mut`] where the record
//! is then changed), while a write that MAY bring the record into existence has
//! its own second half in one place
//! ([`ComAmazonawsAccountAccountState::creatable_account_mut`]), and the
//! COMPOSITION of the two —
//! [`ComAmazonawsAccountAccountState::target_account`] for a read,
//! [`ComAmazonawsAccountAccountState::target_account_mut`] for a write that
//! must not create a record,
//! [`ComAmazonawsAccountAccountState::target_account_or_create`] for a write
//! that creates one on first touch — is what those handlers call, keeping only
//! the mapping into their own operation's error enum. That decision is taken
//! WHOLE
//! and FIRST: a handler resolves its account target before it decides any
//! refusal of its own about another input member (which Region a `RegionName`
//! names, which countries an address is admitted for, which page a token
//! resumes), so a request that is both an account-target refusal and carries
//! such a member answers the account-target refusal. A refusal the request
//! BOUNDARY decides — an absent `@required` member, a value outside a modelled
//! constraint — still precedes it: that request never reaches a handler.
//!
//! The GovCloud slice hangs off that record too:
//! [`Account::gov_cloud_account`] holds the GovCloud account linked to the
//! standard account, which `GetGovCloudAccountInformation` reports. Creating a
//! GovCloud account is an Organizations operation and not one this service
//! binds, so the link is a fact the store HOLDS ([`GovCloudAccountLink`]),
//! never one a read derives and never one this emulator invents: no account
//! comes up linked, and a record carries a link only once something records
//! one through [`Account::link_gov_cloud_account`].
//!
//! The Region opt-in slice hangs off that same record:
//! [`Account::region_opt_status`] holds the opt status of every Region the
//! account has been opted into or out of, which `EnableRegion` and
//! `DisableRegion` write and `GetRegionOptStatus` and `ListRegions` report.
//! [`REGION_CATALOG`] is what decides whether a Region has an opt status at all
//! and what it reports before any opt call, and [`region_opt_target`] is the one
//! place that reads it to answer whether a Region can be the TARGET of an opt
//! call — the question `EnableRegion` and `DisableRegion` both decide by.
//!
//! The primary contact structure carries rules of the same kind, which the model
//! states in prose rather than in constraints: which two-letter strings are
//! country codes, and in which countries an address must carry a state or
//! region. [`validated_contact_information`] is the one place that decides both,
//! the question `PutContactInformation` admits a structure by.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use ::aws_smithy_schema_server::constraints::{validate, PatternTrait};

use crate::ops::types::{
    AccessDeniedException, AlternateContact, ConflictException, ContactInformation,
    ResourceNotFoundException, ValidationException, ValidationExceptionField,
    ALTERNATE_CONTACT_TYPE_VALUES,
};

/// The `AccountState` an account this emulator knows carries.
///
/// `AccountState` is modelled `ACTIVE | CLOSED | PENDING_ACTIVATION | SUSPENDED`
/// (see `ACCOUNT_STATE_VALUES`). Closing or suspending an account is an
/// Organizations operation, not one this service binds, so an account that
/// exists here is active.
pub const ACTIVE_ACCOUNT_STATE: &str = "ACTIVE";

/// The `ValidationExceptionReason` a refusal this service decides about the
/// value of one named input member reports: `fieldValidationFailed`, one of that
/// enum's two values (`fieldValidationFailed | invalidRegionOptTarget`,
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`).
///
/// Every refusal below that is detected ON an input member carries it —
/// whichever member, and whatever is wrong with the value — because that is the
/// reason the service reports for a member whose value failed validation. The
/// other value is not a reason any refusal here reports.
pub const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `PhoneNumberVerificationStatus` the phone number of a stored primary
/// contact carries before any one-time-passcode flow has run on THAT number:
/// "UNVERIFIED – The phone number has not been verified"
/// (`PHONE_NUMBER_VERIFICATION_STATUS_VALUES` is
/// `NOT_SUPPORTED | PENDING | UNVERIFIED | VERIFIED`).
///
/// A phone number becomes `PENDING` when `SendPhoneNumberVerification` sends a
/// one-time passcode to it and `VERIFIED` when `VerifyPhoneNumber` is handed
/// that passcode back; see [`PhoneNumberVerification`].
pub const UNVERIFIED_PHONE_NUMBER: &str = "UNVERIFIED";

/// The `PhoneNumberVerificationStatus` of a phone number a one-time passcode has
/// been sent to and not yet submitted for: "PENDING – A one-time passcode has
/// been sent and is waiting to be submitted".
///
/// `PENDING` is not a status the CLOCK moves on: nothing advances it but a
/// `VerifyPhoneNumber` call carrying the passcode, so no due instant is stored
/// for it and a number nobody verifies stays `PENDING`.
pub const PENDING_PHONE_NUMBER_VERIFICATION: &str = "PENDING";

/// The `PhoneNumberVerificationStatus` of a phone number the one-time passcode
/// was submitted for: "VERIFIED – The phone number has been verified".
pub const VERIFIED_PHONE_NUMBER: &str = "VERIFIED";

/// How many characters a one-time passcode has: `Otp` is modelled
/// `^[a-zA-Z0-9]{6}$` (`smithy_query(query="operation",
/// operation="VerifyPhoneNumber")`), so a passcode this service mints has to be
/// six alphanumerics for the caller to be able to submit it at all.
pub const OTP_LENGTH: usize = 6;

/// A fresh one-time passcode: [`OTP_LENGTH`] hex characters, which satisfies
/// `Otp`'s modelled `^[a-zA-Z0-9]{6}$`.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is a v4 UUID rendered
/// canonically), which this crate already depends on — nothing here is a
/// hand-rolled generator. A passcode is never derived from the request or from
/// the account, so two sends never produce the same one.
///
/// The passcode is `@sensitive` on the wire and is NEVER returned or logged:
/// AWS sends it to the phone number itself, and the only thing this service
/// reports is the resulting status.
pub fn new_otp() -> String {
    random_chars(OTP_LENGTH, char::is_ascii_alphanumeric)
}

/// `len` random characters, each one the uuid-backed randomness produced and
/// `admitted` accepts.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is a v4 UUID rendered
/// canonically), which this crate already depends on — the ONE generator behind
/// every value this store mints, so nothing here is hand-rolled and two mints
/// never agree by construction. A canonical UUID carries fewer than `len`
/// admitted characters for a long enough `len`, so ids are drawn until the
/// length is reached.
fn random_chars(len: usize, admitted: fn(&char) -> bool) -> String {
    let mut minted = String::with_capacity(len);
    while minted.len() < len {
        minted.extend(
            ::emulator_telemetry::BatchId::random()
                .as_str()
                .chars()
                .filter(admitted),
        );
    }
    minted.truncate(len);
    minted
}

/// What a handler reports when the engine it was built on carries no store for
/// this service — a server fault, never a bad request.
pub const STORE_UNAVAILABLE: &str = "the account store is unavailable";

/// The name an account record carries from the moment it exists, before any
/// `PutAccountName` renames it.
///
/// Every Amazon Web Services account HAS a name: the name is set when the
/// account is created, and `GetAccountInformation` is documented to retrieve
/// "information about the specified account including its account name, account
/// ID, account creation date and time, and account state" — so no account ever
/// reports that member absent. An account record this emulator holds therefore
/// gets a name when it is created, not when a caller first writes one.
///
/// The value names the account by its id, which keeps it distinct per record and
/// inside `AccountName`'s modelled `@length(min: 1, max: 50)` and
/// `^[ -;=?-~]+$`: 8 characters of prefix plus a 12-digit id is 20, and every
/// byte of it (the ASCII letters, the space and the digits) is in the admitted
/// range.
pub fn initial_account_name(account_id: &str) -> String {
    format!("Account {account_id}")
}

/// The first line of the postal address an account record carries before any
/// `PutContactInformation` replaces it. Inside `AddressLine`'s
/// `@length(min: 1, max: 60)`.
const INITIAL_ADDRESS_LINE_1: &str = "1 Example Street";

/// The city of that address, inside `City`'s `@length(min: 1, max: 50)`.
const INITIAL_CITY: &str = "Seattle";

/// The state of that address. `ContactInformation`'s own documentation of
/// `StateOrRegion` states it "is required in the following countries: US, CA,
/// GB, DE, JP, IN and BR", and [`INITIAL_COUNTRY_CODE`] is one of them, so the
/// seeded address carries one: the two-character state code of [`INITIAL_CITY`],
/// inside `StateOrRegion`'s `@length(min: 1, max: 50)`.
const INITIAL_STATE_OR_REGION: &str = "WA";

/// The postal code of that address, inside `PostalCode`'s
/// `@length(min: 1, max: 20)`.
const INITIAL_POSTAL_CODE: &str = "98101";

/// The country of that address: an ISO-3166 two-letter country code, which is
/// what `CountryCode` is documented to hold and what its `@length(min: 2,
/// max: 2)` admits.
const INITIAL_COUNTRY_CODE: &str = "US";

/// The phone number of that address: a number in the range reserved for
/// examples, written in the international form `ContactInformationPhoneNumber`'s
/// `^[+][\s0-9()-]+$` admits and inside its `@length(min: 1, max: 20)`.
const INITIAL_PHONE_NUMBER: &str = "+1 206-555-0100";

/// The PRIMARY contact information an account record carries from the moment it
/// exists, before any `PutContactInformation` replaces it.
///
/// Every Amazon Web Services account HAS primary contact information: it is
/// captured when the account is signed up for and inherited by an account
/// created inside an organization, and `ContactInformation` models `FullName`,
/// `AddressLine1`, `City`, `PostalCode`, `CountryCode` and `PhoneNumber` as
/// `@required` — a complete structure, not optional data. So a read of an
/// account this emulator holds reports a primary contact, exactly as it reports
/// a name (see [`initial_account_name`]), rather than refusing until a caller
/// has written one.
///
/// Every member satisfies its modelled constraint (see the constants above), and
/// `FullName` names the account by its id, which keeps the seeded structure
/// distinct per record: 8 characters of prefix, a 12-digit id and 16 of suffix
/// is 36, inside `FullName`'s `@length(min: 1, max: 50)`. The optional members
/// this emulator has not been told anything about stay absent — an address with
/// no second line, district or company is a complete address.
pub fn initial_contact_information(account_id: &str) -> ContactInformation {
    ContactInformation {
        full_name: format!("Account {account_id} Primary Contact"),
        address_line1: INITIAL_ADDRESS_LINE_1.to_string(),
        address_line2: None,
        address_line3: None,
        city: INITIAL_CITY.to_string(),
        state_or_region: Some(INITIAL_STATE_OR_REGION.to_string()),
        district_or_county: None,
        postal_code: INITIAL_POSTAL_CODE.to_string(),
        country_code: INITIAL_COUNTRY_CODE.to_string(),
        phone_number: INITIAL_PHONE_NUMBER.to_string(),
        company_name: None,
        website_url: None,
    }
}

/// The country codes a primary contact address must carry a `StateOrRegion`
/// for, from that member's OWN documentation: "This field is required in the
/// following countries: US, CA, GB, DE, JP, IN, and BR"
/// (`smithy_query(query="doc", shape="ContactInformation",
/// member="StateOrRegion")`).
///
/// The model leaves `StateOrRegion` optional, because whether it is required
/// depends on another member of the same structure — so the cross-member rule is
/// this service's own, exactly as WHICH strings name a Region is
/// ([`REGION_CATALOG`]).
pub const COUNTRIES_REQUIRING_STATE_OR_REGION: &[&str] =
    &["US", "CA", "GB", "DE", "JP", "IN", "BR"];

/// The `ContactInformation` member a refusal about the country of the address
/// names in its `fieldList`.
const COUNTRY_CODE_MEMBER: &str = "CountryCode";

/// The `ContactInformation` member a refusal about the state of the address
/// names in its `fieldList`.
const STATE_OR_REGION_MEMBER: &str = "StateOrRegion";

/// Whether `country_code` is an ISO 3166-1 alpha-2 country code, which is what
/// `CountryCode` is documented to hold ("The ISO-3166 two-letter country code
/// for the primary contact address").
///
/// The model constrains the member only `@length(min: 2, max: 2)`, so WHICH
/// two-letter strings are country codes is data this service decides by — the
/// `rust_iso3166` crate's published table, rather than a list restated here.
/// The codes are published as two UPPERCASE letters, so the lookup is
/// case-sensitive: a lower-case or digit-bearing pair is not one of them.
pub fn is_country_code(country_code: &str) -> bool {
    country_code.chars().all(|letter| letter.is_ascii_uppercase())
        && ::rust_iso3166::from_alpha2(country_code).is_some()
}

/// Whether `contact` is a primary contact structure this service accepts, and
/// the refusal it earns when it is not: the country must be an assigned
/// ISO 3166-1 alpha-2 code ([`is_country_code`]) and, in the countries that
/// documentation names, the address must carry a state or region
/// ([`COUNTRIES_REQUIRING_STATE_OR_REGION`]).
///
/// The ONE statement of the rules the model cannot express about a
/// `ContactInformation` — the counterpart of [`region_opt_target`] — so
/// `PutContactInformation` decides through it after the account-target rule,
/// which is one decision taken whole before a handler decides anything of its
/// own, and before the store is touched, so a refused put writes nothing.
pub fn validated_contact_information(
    contact: &ContactInformation,
) -> Result<(), ValidationException> {
    if !is_country_code(&contact.country_code) {
        return Err(unknown_country_code(&contact.country_code));
    }
    if contact.state_or_region.is_none()
        && COUNTRIES_REQUIRING_STATE_OR_REGION.contains(&contact.country_code.as_str())
    {
        return Err(member_required_for_country(
            STATE_OR_REGION_MEMBER,
            &contact.country_code,
        ));
    }
    Ok(())
}

/// The `ValidationException` payload for a `CountryCode` that is not an
/// ISO 3166-1 alpha-2 country code.
///
/// `reason` is `fieldValidationFailed` and `fieldList` names the member, so the
/// caller can see which input was rejected — the same shape as
/// [`unknown_region`], the other member of this service whose admitted values
/// are data rather than a modelled constraint.
pub fn unknown_country_code(country_code: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "{COUNTRY_CODE_MEMBER} {country_code:?} is not an ISO-3166 two-letter country code"
        ),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: COUNTRY_CODE_MEMBER.to_string(),
            message: format!("{country_code:?} is not an ISO-3166 two-letter country code"),
        }]),
    }
}

/// The `ValidationException` payload for a primary contact address that leaves
/// out a member the COUNTRY of that address makes required — a member the model
/// leaves optional, so the rule is about the pair of members rather than about
/// either one alone.
///
/// The sentence quotes the modelled (Pascal-case) name of the member that is
/// missing and the country code the request submitted, which together are what
/// tells the caller which member it has to supply and why; both are variable, so
/// the same sentence states the rule for whichever member and whichever country
/// the structure pairs. `reason` is `fieldValidationFailed` and `fieldList` names
/// the missing member with the same sentence, like every other refusal this
/// service decides about one member.
pub fn member_required_for_country(member: &str, country_code: &str) -> ValidationException {
    let rule = format!("'{member}' field is required for country '{country_code}'");
    ValidationException {
        message: rule.clone(),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: rule,
        }]),
    }
}

/// The local part of the primary email address an account record carries from
/// the moment it exists, before any primary email update has completed on it.
const INITIAL_PRIMARY_EMAIL_LOCAL_PART: &str = "root+";

/// The domain of that address: the domain RFC 2606 reserves for examples, so no
/// mailbox a real person could own is named.
const INITIAL_PRIMARY_EMAIL_DOMAIN: &str = "@example.com";

/// The primary email address (the root user email address) an account record
/// carries from the moment it exists, before a primary email update has
/// completed on it.
///
/// Every Amazon Web Services account HAS a primary email address: it is the root
/// user's email address, captured when the account is signed up for, and
/// `GetPrimaryEmail` is documented to retrieve "the primary email address
/// associated with the specified account" — so an account record this emulator
/// holds reports one from the moment it exists, exactly as it reports a name and
/// primary contact information (see [`initial_account_name`] and
/// [`initial_contact_information`]), rather than reporting that member absent
/// until a caller has run the update flow.
///
/// The value names the account by its id, which keeps it distinct per record and
/// inside `PrimaryEmailAddress`'s modelled `@length(min: 5, max: 64)`: 5
/// characters of local part, a 12-digit id and 12 of domain is 29, and the
/// shape carries no `@pattern` to satisfy beyond that.
///
/// Because the address is a function of the id ALONE, it is the address that id
/// will hold whenever its record is first touched — so it belongs to that
/// account before the record exists, which is what
/// [`seeded_primary_email_account_id`] reads back off an address and what keeps
/// one address from reaching two accounts.
pub fn initial_primary_email(account_id: &str) -> String {
    format!("{INITIAL_PRIMARY_EMAIL_LOCAL_PART}{account_id}{INITIAL_PRIMARY_EMAIL_DOMAIN}")
}

/// The account whose seeded root address `address` is, or `None` for an address
/// this emulator mints for no account — the inverse of
/// [`initial_primary_email`].
///
/// An address is one this emulator mints only when it is EXACTLY that form: the
/// local part, an account id of [`ACCOUNT_ID_LENGTH`] digits (`AccountId`'s own
/// `^\d{12}$`) and the domain. An address that merely resembles it — another
/// domain, a middle part that is not twelve digits — is nobody's seeded address
/// and is admitted like any other.
pub fn seeded_primary_email_account_id(address: &str) -> Option<&str> {
    let account_id = address
        .strip_prefix(INITIAL_PRIMARY_EMAIL_LOCAL_PART)?
        .strip_suffix(INITIAL_PRIMARY_EMAIL_DOMAIN)?;
    (account_id.len() == ACCOUNT_ID_LENGTH
        && account_id.bytes().all(|digit| digit.is_ascii_digit()))
    .then_some(account_id)
}

/// How many account records the store holds before a call that would create a
/// NEW one is refused.
///
/// This is the emulator's OWN bound on its store, not a claim about any limit
/// AWS publishes: `AccountId` is only constrained to `^\d{12}$`, which bounds
/// the shape of a key and not the number of them, so without this bound a
/// caller could mint records without end by varying `AccountId`. It is set far
/// above any workflow an emulator serves, and it is checked only when a request
/// would ADD a record: every account already in the store stays writable.
pub const MAX_ACCOUNTS: usize = 1_000;

/// The `PrimaryEmailUpdateStatus` an update carries from the moment
/// `StartPrimaryEmailUpdate` records it: `PENDING`, one of that enum's four
/// values (`PRIMARY_EMAIL_UPDATE_STATUS_VALUES` in `ops::types`).
///
/// `PENDING` is not a status the CLOCK moves on: Account Management sends a
/// one-time passcode to the proposed address and the update advances only when a
/// caller presents that passcode through `AcceptPrimaryEmailUpdate`. So no due
/// instant is stored for it — waiting changes nothing, and an update nobody
/// accepts stays `PENDING`.
pub const PENDING_PRIMARY_EMAIL_UPDATE_STATUS: &str = "PENDING";

/// The `PrimaryEmailUpdateStatus` an update carries from the moment
/// `AcceptPrimaryEmailUpdate` confirms it: `ACCEPTED`, another of that enum's
/// four values.
///
/// `ACCEPTED` is NOT terminal — the request has been accepted and the address
/// change still has to be applied — so the clock owes the update its
/// [`COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS`] after
/// [`PRIMARY_EMAIL_UPDATE_COMPLETION`].
pub const ACCEPTED_PRIMARY_EMAIL_UPDATE_STATUS: &str = "ACCEPTED";

/// The TERMINAL `PrimaryEmailUpdateStatus` of an accepted update: `COMPLETED`,
/// the status an update carries once the address it was started for is the
/// account's primary email.
pub const COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS: &str = "COMPLETED";

/// How long this emulator takes to carry an accepted primary email update from
/// `ACCEPTED` to `COMPLETED`.
///
/// AWS publishes no duration for it, so this span is the emulator's OWN choice.
/// It is deliberately non-zero: a caller that reads the status straight after
/// accepting still observes `ACCEPTED`, and the state errors that go with it,
/// exactly as it would against AWS.
pub const PRIMARY_EMAIL_UPDATE_COMPLETION: Duration = Duration::from_millis(500);

/// The `RegionOptStatus` a Region that Amazon Web Services turns on for every
/// account carries: `ENABLED_BY_DEFAULT`, one of `RegionOptStatus`'s five values
/// (`REGION_OPT_STATUS_VALUES` in `ops::types`). Such a Region cannot be
/// disabled, so no account ever reports anything else for it.
pub const ENABLED_BY_DEFAULT_REGION_STATUS: &str = "ENABLED_BY_DEFAULT";

/// The `RegionOptStatus` an opt-in Region carries for an account that has not
/// enabled it: `DISABLED`. `EnableRegion` is what moves such a Region to
/// `ENABLING` and then `ENABLED`.
pub const DISABLED_REGION_STATUS: &str = "DISABLED";

/// The Regions of the commercial (`aws`) partition Amazon Web Services turns on
/// for every account, which therefore need no opt-in at all.
///
/// SOURCE: the "Regions that are enabled by default" table of the Amazon Web
/// Services Account Management Reference Guide, "Specify which Amazon Web
/// Services Regions your account can use" — the guide this API's Region-opt
/// operations document themselves against.
///
/// The ENABLED set is the written list and the opt-in set is its COMPLEMENT over
/// the published Region catalog ([`REGION_CATALOG`]), rather than the other way
/// round, because that is the direction AWS's rule runs: a Region launched after
/// the guide's cut-off is disabled until the account opts in, so a Region the
/// catalog gains later is an opt-in Region without this list being touched.
pub const REGIONS_ENABLED_BY_DEFAULT: [&str; 17] = [
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

/// The Region-code prefixes of the partitions this API does NOT report Regions
/// of: the China partition, GovCloud and the isolated partitions, whose Region
/// codes all name their partition in their leading groups.
///
/// An account lives in exactly one partition and reaches only that partition's
/// Regions, and this emulator serves the commercial one — the partition its
/// endpoints and its signing scope name — so a Region of any other partition
/// carries no opt status for this account at all.
///
/// The two isolated-partition prefixes deliberately end WITHOUT the hyphen: the
/// codes they have to catch spell the partition letter inside that group
/// (`us-iso-east-1`, `us-isob-east-1`, `us-isof-south-1`, `eu-isoe-west-1`), so
/// completing them to `us-iso-` would readmit every `us-isob`/`us-isof` Region as
/// a commercial one.
const NON_COMMERCIAL_REGION_PREFIXES: [&str; 4] = ["cn-", "eu-iso", "us-gov-", "us-iso"];

/// Whether `region_name` is a Region of the commercial partition — the partition
/// whose Region codes name no partition at all
/// ([`NON_COMMERCIAL_REGION_PREFIXES`]).
fn is_commercial_region(region_name: &str) -> bool {
    !NON_COMMERCIAL_REGION_PREFIXES
        .iter()
        .any(|prefix| region_name.starts_with(prefix))
}

/// Every Region of the commercial (`aws`) partition this emulator reports, with
/// the `RegionOptStatus` an account that has neither enabled nor disabled it
/// reports for that Region.
///
/// MEMBERSHIP is service data, not a modelled list: `RegionName` is a free
/// string constrained only to `@length(min: 1, max: 50)`
/// (`smithy_query(query="shape",shape="RegionName")`), so there is no modelled
/// list to project. It is DERIVED from the catalog of Regions this emulator
/// publishes (`emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the
/// wire may carry", already sorted), narrowed to the commercial partition
/// ([`is_commercial_region`]) — the same source the Region catalog of the service
/// that publishes each Region's opt-in status derives from — rather than from a
/// second hand-written membership list that would drift from it. A Region the
/// emulator publishes but this catalog omitted would carry no opt status for the
/// account, and every endpoint gated on that status would serve a Region the
/// account cannot use.
///
/// The status is the catalog's DEFAULT, before any opt call: a Region of
/// [`REGIONS_ENABLED_BY_DEFAULT`] is `ENABLED_BY_DEFAULT`, and every other Region
/// of the partition requires an opt-in, so it is `DISABLED` until `EnableRegion`
/// says otherwise.
///
/// SORTED by Region code and free of duplicates: `ListRegions` reports it in this
/// order and pages through it by Region code, so a resume point is a position in
/// this sequence, and [`catalog_region_status`] finds a Region by binary search.
/// The order is established HERE rather than assumed of the source catalog, so a
/// source that ever carried an out-of-order entry cannot turn that lookup into a
/// silent miss.
pub static REGION_CATALOG: LazyLock<Vec<(&'static str, &'static str)>> = LazyLock::new(|| {
    let mut catalog: Vec<(&'static str, &'static str)> = ::emulator_telemetry::policy::KNOWN_REGIONS
        .iter()
        .copied()
        .filter(|region| is_commercial_region(region))
        .map(|region| {
            let status = if REGIONS_ENABLED_BY_DEFAULT.contains(&region) {
                ENABLED_BY_DEFAULT_REGION_STATUS
            } else {
                DISABLED_REGION_STATUS
            };
            (region, status)
        })
        .collect();
    catalog.sort_unstable();
    catalog.dedup();
    catalog
});

/// The status [`REGION_CATALOG`] gives `region_name` before any account-specific
/// opt-in is recorded, or `None` for a Region code the catalog does not hold.
///
/// The catalog is sorted by Region code, so the lookup is a binary search over it.
pub fn catalog_region_status(region_name: &str) -> Option<&'static str> {
    REGION_CATALOG
        .binary_search_by(|(name, _)| (*name).cmp(region_name))
        .ok()
        .map(|index| REGION_CATALOG[index].1)
}

/// Which alternate contact a record is: the `AlternateContactType` enum, whose
/// wire values are `BILLING | OPERATIONS | SECURITY`
/// (`ALTERNATE_CONTACT_TYPE_VALUES` in `ops::types`).
///
/// `Ord` is what keys the per-account [`BTreeMap`], so the contacts of one
/// account iterate in a stable order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlternateContactType {
    Billing,
    Operations,
    Security,
}

impl AlternateContactType {
    /// The type a wire value names, or `None` for a value the enum does not
    /// admit.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "BILLING" => Some(Self::Billing),
            "OPERATIONS" => Some(Self::Operations),
            "SECURITY" => Some(Self::Security),
            _ => None,
        }
    }

    /// The wire value of this type — what the `AlternateContact` structure
    /// reports back.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Billing => "BILLING",
            Self::Operations => "OPERATIONS",
            Self::Security => "SECURITY",
        }
    }
}

/// One alternate contact, exactly as `PutAlternateContact` stored it.
#[derive(Debug, Clone)]
pub struct AlternateContactRecord {
    pub name: String,
    pub title: String,
    pub email_address: String,
    pub phone_number: String,
    pub contact_type: AlternateContactType,
}

impl AlternateContactRecord {
    /// The `AlternateContact` structure `GetAlternateContact` returns: every
    /// member is the stored one.
    pub fn to_alternate_contact(&self) -> AlternateContact {
        AlternateContact {
            name: Some(self.name.clone()),
            title: Some(self.title.clone()),
            email_address: Some(self.email_address.clone()),
            phone_number: Some(self.phone_number.clone()),
            alternate_contact_type: Some(self.contact_type.as_wire().to_string()),
        }
    }
}

/// Which column of the Region catalog a Region code is in: whether the Region
/// is a Region-opt target at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Available without opting in, so neither `EnableRegion` nor
    /// `DisableRegion` can act on it.
    EnabledByDefault,
    /// Disabled until the account opts in.
    OptIn,
}

/// Which kind of Region `region_name` is, read off [`REGION_CATALOG`], or `None`
/// for a value the catalog does not hold.
pub fn region_kind(region_name: &str) -> Option<RegionKind> {
    catalog_region_status(region_name).map(|status| {
        if status == ENABLED_BY_DEFAULT_REGION_STATUS {
            RegionKind::EnabledByDefault
        } else {
            RegionKind::OptIn
        }
    })
}

/// Whether `region_name` can be the target of a Region-opt call, and the
/// refusal it earns when it cannot: `Ok(())` for an opt-in Region, the
/// [`unknown_region`] payload for a value [`REGION_CATALOG`] does not hold, and
/// the [`region_not_opt_target`] payload for a Region the catalog turns on by
/// default, which neither opt call can act on.
///
/// The ONE statement of that rule. `EnableRegion` and `DisableRegion` both
/// decide through it — after the account-target rule, which is one decision
/// taken whole before a handler decides anything of its own, and before the
/// store is touched, so a refused call writes nothing — so which Regions count
/// as opt targets, and which refusal every other case earns, changes here alone
/// and cannot drift between the two operations.
pub fn region_opt_target(region_name: &str) -> Result<(), ValidationException> {
    match region_kind(region_name) {
        None => Err(unknown_region(region_name)),
        Some(RegionKind::EnabledByDefault) => Err(region_not_opt_target(region_name)),
        Some(RegionKind::OptIn) => Ok(()),
    }
}

/// How long this emulator takes to carry a Region from a transient opt status
/// (`ENABLING`, `DISABLING`) to its terminal one (`ENABLED`, `DISABLED`).
///
/// AWS publishes no duration for it — `EnableRegion`'s `RegionName`
/// documentation says only that the process "takes a few minutes for most
/// accounts, but it can take several hours" — so this span is the emulator's
/// OWN choice. It is deliberately non-zero: a caller that reads the status
/// straight after the call still observes the transient status, and the state
/// errors that go with it, exactly as it would against AWS.
pub const REGION_OPT_TRANSITION: Duration = Duration::from_millis(500);

/// A Region's opt status for one account: the `RegionOptStatus` enum
/// (`DISABLED | DISABLING | ENABLED | ENABLED_BY_DEFAULT | ENABLING`).
///
/// A transient status carries the instant its terminal status is DUE, so the
/// transition is owed by the CLOCK rather than by whoever happens to call next:
/// [`settled`](Self::settled) reports the terminal status from that instant on,
/// whether or not anybody read or wrote in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOptState {
    /// Available without opting in: the status of every Region
    /// [`REGION_CATALOG`] marks [`ENABLED_BY_DEFAULT_REGION_STATUS`].
    EnabledByDefault,
    /// Opted in, with the opt-in not yet complete.
    Enabling { ready_at: SystemTime },
    /// Opted in and usable.
    Enabled,
    /// Opted out, with the opt-out not yet complete.
    Disabling { ready_at: SystemTime },
    /// Not opted in: the status of an opt-in Region the account has never
    /// enabled, and the one an opt-out settles to.
    Disabled,
}

impl RegionOptState {
    /// The wire value of this status, one of `REGION_OPT_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::EnabledByDefault => ENABLED_BY_DEFAULT_REGION_STATUS,
            Self::Enabling { .. } => "ENABLING",
            Self::Enabled => "ENABLED",
            Self::Disabling { .. } => "DISABLING",
            Self::Disabled => DISABLED_REGION_STATUS,
        }
    }

    /// The status a wire `RegionOptStatus` value names, with a transient value
    /// settling at `ready_at`. `None` for a value the enum does not admit.
    pub fn from_wire(value: &str, ready_at: SystemTime) -> Option<Self> {
        match value {
            ENABLED_BY_DEFAULT_REGION_STATUS => Some(Self::EnabledByDefault),
            "ENABLING" => Some(Self::Enabling { ready_at }),
            "ENABLED" => Some(Self::Enabled),
            "DISABLING" => Some(Self::Disabling { ready_at }),
            DISABLED_REGION_STATUS => Some(Self::Disabled),
            _ => None,
        }
    }

    /// This status as of `now`: a transient status whose due instant has passed
    /// IS its terminal status from that instant on, and every other status is
    /// itself. A pure function of the record and the clock, so a reader reports
    /// the settled status without writing anything.
    pub fn settled(self, now: SystemTime) -> Self {
        match self {
            Self::Enabling { ready_at } if now >= ready_at => Self::Enabled,
            Self::Disabling { ready_at } if now >= ready_at => Self::Disabled,
            other => other,
        }
    }
}

/// The most recent primary email update of one account, as
/// `StartPrimaryEmailUpdate` recorded it and `GetPrimaryEmailUpdateStatus`
/// reports it.
#[derive(Debug, Clone)]
pub struct PrimaryEmailUpdate {
    /// The address the update was started for (`PrimaryEmailAddress`,
    /// `@length(min: 5, max: 64)` and `@sensitive`). Held because the update is
    /// about an address, and `AcceptPrimaryEmailUpdate` has to check the address
    /// a caller confirms against the one that was requested; it is never put in
    /// a response or a log by this service.
    pub requested_email: String,
    /// The one-time passcode this update was started with, while it is still
    /// outstanding, and `None` once the call that confirmed the update consumed
    /// it.
    ///
    /// Held for the same reason the phone flow holds one (see
    /// [`PhoneNumberVerificationState::Pending`]): the passcode goes out of
    /// band, so the only way a caller's submission can be checked against the
    /// one that was sent is for the store to hold it. It is `@sensitive` and
    /// never leaves the store — no response and no log carries it.
    pub otp: Option<String>,
    /// One of `PrimaryEmailUpdateStatus`'s values; see
    /// [`PENDING_PRIMARY_EMAIL_UPDATE_STATUS`].
    pub status: String,
    /// When this status was last set — what `GetPrimaryEmailUpdateStatus`
    /// reports as `UpdatedAt`.
    pub updated_at: SystemTime,
    /// The instant this update's `COMPLETED` status is DUE, set when
    /// `AcceptPrimaryEmailUpdate` moves it to `ACCEPTED`, and `None` for a
    /// status the clock does not move on (`PENDING`, and `COMPLETED` once it
    /// has been materialised).
    ///
    /// Held so the transition is owed by the CLOCK rather than by whoever
    /// happens to call next: [`settled_status`](Self::settled_status) reports
    /// the terminal status from that instant on, whether or not anybody read or
    /// wrote in between. When the completion it names counts as OWED is decided
    /// in one place, by this type's own `due_completion`, which every reader and
    /// every writer of the status goes through.
    pub completes_at: Option<SystemTime>,
}

impl PrimaryEmailUpdate {
    /// The update `StartPrimaryEmailUpdate` records at `now`: `PENDING`, because
    /// the address still has to be confirmed with the one-time passcode `otp`,
    /// which is outstanding from this moment on.
    pub fn started(requested_email: String, otp: String, now: SystemTime) -> Self {
        Self {
            requested_email,
            otp: Some(otp),
            status: PENDING_PRIMARY_EMAIL_UPDATE_STATUS.to_string(),
            updated_at: now,
            completes_at: None,
        }
    }

    /// The passcode outstanding for this update, or `None` once it has been
    /// submitted: a one-time passcode is consumed by the call that confirms the
    /// update. The counterpart of
    /// [`PhoneNumberVerification::outstanding_otp`].
    pub fn outstanding_otp(&self) -> Option<&str> {
        self.otp.as_deref()
    }

    /// Whether this update is still waiting for the one-time passcode, which is
    /// the one status `AcceptPrimaryEmailUpdate` can act on.
    pub fn is_pending(&self) -> bool {
        self.status == PENDING_PRIMARY_EMAIL_UPDATE_STATUS
    }

    /// Whether this update has been confirmed and its completion is owed: the
    /// address it names is on its way to being an account's primary email, so
    /// it already counts as in use.
    pub fn is_accepted(&self) -> bool {
        self.status == ACCEPTED_PRIMARY_EMAIL_UPDATE_STATUS
    }

    /// Confirm this update at `now`: it becomes `ACCEPTED`, its one-time
    /// passcode is consumed, and its `COMPLETED` status comes due
    /// [`PRIMARY_EMAIL_UPDATE_COMPLETION`] later.
    pub fn accept(&mut self, now: SystemTime) {
        self.otp = None;
        self.status = ACCEPTED_PRIMARY_EMAIL_UPDATE_STATUS.to_string();
        self.updated_at = now;
        self.completes_at = Some(now + PRIMARY_EMAIL_UPDATE_COMPLETION);
    }

    /// The instant this update's completion came DUE, once it is owed as of
    /// `now`, and `None` while nothing is owed — either because no completion
    /// is pending ([`completes_at`](Self::completes_at) is unset) or because
    /// its due instant is still ahead of `now`.
    ///
    /// The ONE statement of that rule. The status read, the address read and
    /// the two mutating primary-email operations all decide through it — via
    /// [`settled_status`](Self::settled_status),
    /// [`settled_address`](Self::settled_address),
    /// [`settled_updated_at`](Self::settled_updated_at) and
    /// [`settle`](Self::settle) — so when a completion counts as owed changes
    /// here alone and cannot drift between the answers those operations give
    /// about one stored instant.
    fn due_completion(&self, now: SystemTime) -> Option<SystemTime> {
        self.completes_at.filter(|due| now >= *due)
    }

    /// This update's status as of `now`: an accepted update whose due instant
    /// has passed IS `COMPLETED` from that instant on, and every other status
    /// is the stored one. A pure function of the record and the clock, so a
    /// reader reports the settled status without writing anything.
    pub fn settled_status(&self, now: SystemTime) -> &str {
        if self.due_completion(now).is_some() {
            COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS
        } else {
            &self.status
        }
    }

    /// The address this update has already PUT ON the account as of `now`: the
    /// address it was started for, once the completion the clock owes it has
    /// come due, and `None` while the update has not completed. A pure function
    /// of the record and the clock, so a reader answers the settled address
    /// without writing anything — the counterpart of
    /// [`settled_status`](Self::settled_status).
    pub fn settled_address(&self, now: SystemTime) -> Option<&str> {
        self.due_completion(now)
            .is_some()
            .then_some(self.requested_email.as_str())
    }

    /// When the status [`settled_status`](Self::settled_status) reports was set:
    /// the due instant for a completion the clock already owes, else the instant
    /// the stored status was written.
    pub fn settled_updated_at(&self, now: SystemTime) -> SystemTime {
        self.due_completion(now).unwrap_or(self.updated_at)
    }

    /// Materialise the completion the clock already owes: an `ACCEPTED` update
    /// whose due instant is at or before `now` becomes `COMPLETED` in the
    /// store, and the address it was started for is answered back as the one
    /// the account's primary email becomes.
    ///
    /// Owed by the CLOCK, so this changes nothing a reader could not already
    /// see ([`settled_status`](Self::settled_status) reports the same status
    /// from the same instant without writing), and it is idempotent.
    pub fn settle(&mut self, now: SystemTime) -> Option<String> {
        let due = self.due_completion(now)?;
        self.status = COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS.to_string();
        self.updated_at = due;
        self.completes_at = None;
        Some(self.requested_email.clone())
    }
}

/// Where one phone number got to in the one-time-passcode flow.
///
/// There is no third state: a number this emulator holds no record for is
/// `UNVERIFIED`, which is what [`Account::phone_number_verification_status`]
/// reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneNumberVerificationState {
    /// `SendPhoneNumberVerification` sent a passcode and it has not been
    /// submitted yet. The passcode is held so `VerifyPhoneNumber` can check what
    /// the caller submits against it; it is `@sensitive` and never leaves the
    /// store.
    Pending { otp: String },
    /// The passcode was submitted and matched.
    Verified,
}

impl PhoneNumberVerificationState {
    /// The wire `PhoneNumberVerificationStatus` of this state, one of
    /// `PHONE_NUMBER_VERIFICATION_STATUS_VALUES`.
    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Pending { .. } => PENDING_PHONE_NUMBER_VERIFICATION,
            Self::Verified => VERIFIED_PHONE_NUMBER,
        }
    }
}

/// The phone-number verification of one account, as
/// `SendPhoneNumberVerification` recorded it and `VerifyPhoneNumber` advanced
/// it.
///
/// A verification is ABOUT a phone number, not about an account: AWS sends the
/// passcode to "the phone number in the primary contact information", so the
/// number it was sent to is stored with it and a record only speaks for the
/// number it names. Replace the primary contact's phone number and the stored
/// record no longer applies to it — see
/// [`Account::phone_number_verification_status`].
#[derive(Debug, Clone)]
pub struct PhoneNumberVerification {
    /// The primary-contact phone number the passcode was sent to.
    pub phone_number: String,
    /// How far that number got.
    pub state: PhoneNumberVerificationState,
    /// When this state was last set.
    pub updated_at: SystemTime,
}

impl PhoneNumberVerification {
    /// The record `SendPhoneNumberVerification` writes at `now`: a passcode is
    /// outstanding for `phone_number`, so the number is `PENDING`.
    pub fn sent(phone_number: String, otp: String, now: SystemTime) -> Self {
        Self {
            phone_number,
            state: PhoneNumberVerificationState::Pending { otp },
            updated_at: now,
        }
    }

    /// The passcode outstanding for this number, or `None` once it has been
    /// submitted: a one-time passcode is consumed by the call that verifies it.
    pub fn outstanding_otp(&self) -> Option<&str> {
        match &self.state {
            PhoneNumberVerificationState::Pending { otp } => Some(otp.as_str()),
            PhoneNumberVerificationState::Verified => None,
        }
    }
}

/// How many digits an account id has: `AccountId` is modelled `^\d{12}$`
/// (`smithy_query(query="shape",shape="AccountId")`), so an id this emulator
/// mints has to be twelve digits for a caller to be able to send it back.
pub const ACCOUNT_ID_LENGTH: usize = 12;

/// A fresh account id for a GovCloud account: [`ACCOUNT_ID_LENGTH`] digits,
/// which satisfies `AccountId`'s modelled `^\d{12}$`.
///
/// Minted through [`random_chars`], the one generator this store draws from, so
/// the id is never derived from the standard account's id and no two links name
/// one GovCloud account. It is minted ONCE and stored, so every later read
/// reports the same id.
pub fn new_gov_cloud_account_id() -> String {
    random_chars(ACCOUNT_ID_LENGTH, char::is_ascii_digit)
}

/// The GovCloud account linked to one standard account, as this emulator
/// persists it.
///
/// A GovCloud account is brought into existence by an Organizations operation,
/// never by anything Account Management binds, so this record is a fact the
/// store HOLDS rather than one a read computes: an account record carrying one
/// has a GovCloud account to report, and an account record without one has
/// none. [`Account::link_gov_cloud_account`] is the seam whatever creates a
/// GovCloud account records it through.
#[derive(Debug, Clone)]
pub struct GovCloudAccountLink {
    /// The 12-digit account id of the linked GovCloud account (`^\d{12}$`).
    pub gov_cloud_account_id: String,
    /// The linked GovCloud account's OWN state: one of `AwsAccountState`'s
    /// values (`AWS_ACCOUNT_STATE_VALUES` in `ops::types`), which is the
    /// GovCloud account's state and not the standard account's.
    pub state: String,
}

impl GovCloudAccountLink {
    /// A link to a freshly minted, ACTIVE GovCloud account of the standard
    /// account `standard_account_id`.
    ///
    /// A GovCloud account is a SEPARATE account from the standard one it is
    /// linked to, so the id is minted until it differs from the standard
    /// account's — the one thing a caller can check about a minted id without
    /// having been told it.
    pub fn active_for(standard_account_id: &str) -> Self {
        let mut gov_cloud_account_id = new_gov_cloud_account_id();
        while gov_cloud_account_id == standard_account_id {
            gov_cloud_account_id = new_gov_cloud_account_id();
        }
        Self {
            gov_cloud_account_id,
            state: ACTIVE_ACCOUNT_STATE.to_string(),
        }
    }
}

/// One Amazon Web Services account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Account {
    /// The 12-digit account id this record is keyed by (`^\d{12}$`).
    pub account_id: String,
    /// The account's name: [`initial_account_name`] at creation, and whatever
    /// `PutAccountName` last stored after that. Never absent — an account
    /// always has a name (see [`initial_account_name`]).
    pub account_name: String,
    /// Set ONCE, when the record is first created, and never recomputed — so
    /// every later `GetAccountInformation` reports the same instant.
    pub created_date: SystemTime,
    /// One of `AccountState`'s values; see [`ACTIVE_ACCOUNT_STATE`].
    pub state: String,
    /// At most one contact per `AlternateContactType`, which is what makes
    /// `PutAlternateContact` an upsert rather than an append.
    pub alternate_contacts: BTreeMap<AlternateContactType, AlternateContactRecord>,
    /// The account's PRIMARY contact information:
    /// [`initial_contact_information`] at creation, and exactly what the last
    /// `PutContactInformation` stored after that. Never absent — every account
    /// has primary contact information (see [`initial_contact_information`]).
    /// There is at most one primary contact per account, so a put replaces the
    /// whole structure rather than merging into it.
    pub contact_information: ContactInformation,
    /// The `RegionOptStatus` this account carries for a Region it has been opted
    /// into or out of, keyed by Region code. A Region ABSENT from this map still
    /// reports its [`REGION_CATALOG`] status, so the map holds only what an
    /// opt-in call changed: `EnableRegion` and `DisableRegion` are the
    /// operations that write it, and `ListRegions` and `GetRegionOptStatus`
    /// report it. A transient entry carries the instant its terminal status is
    /// due, so a reader settles it from the clock rather than from a write.
    pub region_opt_status: BTreeMap<String, RegionOptState>,
    /// The MOST RECENT primary email update of this account, or `None` while
    /// none has ever been started — which is what makes a status read of an
    /// account with no update a refusal rather than a reported status.
    pub primary_email_update: Option<PrimaryEmailUpdate>,
    /// The account's primary email address (the root user email address):
    /// [`initial_primary_email`] from the moment the record exists, and the
    /// address of the last primary email update that COMPLETED on it after
    /// that. Never absent — every account has a root user email address (see
    /// [`initial_primary_email`]), which is what `GetPrimaryEmail` reports.
    ///
    /// Written by [`settle_primary_email_update`](Self::settle_primary_email_update)
    /// when an accepted update completes, and read to decide whether an
    /// address is already in use — which is the conflict Account Management
    /// documents for a root user email change.
    pub primary_email: String,
    /// The MOST RECENT phone-number verification of this account, or `None`
    /// while no `SendPhoneNumberVerification` has ever sent a passcode. The
    /// record names the number the passcode went to, so it stops applying when
    /// the primary contact's phone number changes.
    pub phone_verification: Option<PhoneNumberVerification>,
    /// The GovCloud account linked to this standard account, and `None` while
    /// this emulator holds no record of one: a GovCloud account is created by
    /// an Organizations operation, so this emulator learns of a link only by
    /// being told of it, and never invents one for an account whose id it was
    /// merely handed.
    pub gov_cloud_account: Option<GovCloudAccountLink>,
}

impl Account {
    /// A new account record, created at `now`.
    pub fn new(account_id: String, now: SystemTime) -> Self {
        Self {
            account_name: initial_account_name(&account_id),
            contact_information: initial_contact_information(&account_id),
            primary_email: initial_primary_email(&account_id),
            account_id,
            created_date: now,
            state: ACTIVE_ACCOUNT_STATE.to_string(),
            alternate_contacts: BTreeMap::new(),
            region_opt_status: BTreeMap::new(),
            primary_email_update: None,
            phone_verification: None,
            gov_cloud_account: None,
        }
    }

    /// Record the GovCloud account linked to this standard account.
    ///
    /// The write seam for whatever CREATES a GovCloud account; an account has
    /// at most one, so this replaces any link the record already carried rather
    /// than appending a second.
    pub fn link_gov_cloud_account(&mut self, link: GovCloudAccountLink) {
        self.gov_cloud_account = Some(link);
    }

    /// Materialise the primary email update completion the clock already owes:
    /// an `ACCEPTED` update whose due instant is at or before `now` becomes
    /// `COMPLETED`, and the address it was started for becomes this account's
    /// [`primary_email`](Self::primary_email).
    ///
    /// Owed by the CLOCK and idempotent, exactly like
    /// [`settle_region_opts`](Self::settle_region_opts). The mutating
    /// primary-email operations call it before deciding, so the record they
    /// write over is the settled one.
    pub fn settle_primary_email_update(&mut self, now: SystemTime) {
        let completed = self
            .primary_email_update
            .as_mut()
            .and_then(|update| update.settle(now));
        if let Some(address) = completed {
            self.primary_email = address;
        }
    }

    /// This account's primary email address as of `now`: the address of an
    /// accepted update whose completion the clock already owes, else the stored
    /// one.
    ///
    /// A pure function of the record and the clock, exactly like
    /// [`PrimaryEmailUpdate::settled_status`], so a REPORTING operation answers
    /// the settled address without writing anything.
    pub fn settled_primary_email(&self, now: SystemTime) -> &str {
        self.primary_email_update
            .as_ref()
            .and_then(|update| update.settled_address(now))
            .unwrap_or(self.primary_email.as_str())
    }

    /// The phone number in this account's primary contact information.
    /// `PhoneNumber` is `@required` on `ContactInformation` and every account
    /// carries one of those, so there is always a number to report.
    pub fn primary_phone_number(&self) -> &str {
        self.contact_information.phone_number.as_str()
    }

    /// The `PhoneNumberVerificationStatus` this account reports for the phone
    /// number currently in its primary contact information.
    ///
    /// A verification speaks only for the number it was sent to, so a stored
    /// record is honoured only while that number is still the primary contact's
    /// — otherwise, and with no record at all, the number is
    /// [`UNVERIFIED_PHONE_NUMBER`]. A pure function of the record: a reporting
    /// operation answers from it without writing anything.
    ///
    /// `NOT_SUPPORTED` ("phone number verification isn't available for this
    /// account") is never reported: every account this emulator holds can run
    /// the passcode flow.
    pub fn phone_number_verification_status(&self) -> &'static str {
        let phone_number = self.primary_phone_number();
        match &self.phone_verification {
            Some(verification) if verification.phone_number == phone_number => {
                verification.state.as_wire()
            }
            _ => UNVERIFIED_PHONE_NUMBER,
        }
    }

    /// This account's verification record, but only while it speaks for the
    /// phone number currently in the primary contact information — what
    /// `VerifyPhoneNumber` checks a submitted passcode against.
    pub fn current_phone_verification_mut(&mut self) -> Option<&mut PhoneNumberVerification> {
        let phone_number = self.primary_phone_number().to_string();
        self.phone_verification
            .as_mut()
            .filter(|verification| verification.phone_number == phone_number)
    }

    /// Materialise every Region opt transition the clock already owes: an
    /// `ENABLING` or `DISABLING` Region whose due instant is at or before `now`
    /// becomes `ENABLED` or `DISABLED` in the store.
    ///
    /// Owed by the CLOCK, so this changes nothing a reader could not already
    /// see — [`RegionOptState::settled`] reports the same status from the same
    /// instant without writing — and it is idempotent. The mutating opt
    /// operations call it before deciding, so the record they write over is the
    /// settled one.
    pub fn settle_region_opts(&mut self, now: SystemTime) {
        for status in self.region_opt_status.values_mut() {
            *status = status.settled(now);
        }
    }

    /// This account's opt status for the Region `region_name` of kind `kind`,
    /// as of `now`.
    ///
    /// A default-enabled Region reports `ENABLED_BY_DEFAULT` whatever the store
    /// holds, and an opt-in Region the account never acted on reports
    /// `DISABLED`. A transient stored status settles from the clock
    /// ([`RegionOptState::settled`]), so the answer never depends on whether
    /// anybody wrote the terminal status back.
    pub fn region_opt_state(
        &self,
        region_name: &str,
        kind: RegionKind,
        now: SystemTime,
    ) -> RegionOptState {
        match kind {
            RegionKind::EnabledByDefault => RegionOptState::EnabledByDefault,
            RegionKind::OptIn => self
                .region_opt_status
                .get(region_name)
                .copied()
                .unwrap_or(RegionOptState::Disabled)
                .settled(now),
        }
    }

    /// The `RegionOptStatus` this account reports for `region_name` as of `now`:
    /// the status an opt-in call stored for it, settled, else its
    /// [`REGION_CATALOG`] status. `None` for a Region code the catalog does not
    /// hold.
    ///
    /// A Region AWS turns on by default is never opted out of, so a stored
    /// status is honoured only where the catalog admits one — which keeps
    /// `ENABLED_BY_DEFAULT` the one status such a Region can report.
    pub fn reported_region_status(
        &self,
        region_name: &str,
        now: SystemTime,
    ) -> Option<&'static str> {
        let kind = region_kind(region_name)?;
        Some(self.region_opt_state(region_name, kind, now).as_wire())
    }

    /// Every Region in [`REGION_CATALOG`], in catalog order, with the status this
    /// account reports for it as of `now` — what `ListRegions` projects.
    pub fn reported_regions(&self, now: SystemTime) -> Vec<(&'static str, &'static str)> {
        REGION_CATALOG
            .iter()
            .filter_map(|(name, _)| {
                self.reported_region_status(name, now)
                    .map(|status| (*name, status))
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-account-account. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAccountAccountState {
    /// Every account this emulator knows, keyed by its 12-digit id. The
    /// caller's own account ([`default_account_id`]) is always present.
    pub accounts: BTreeMap<String, Account>,
}

/// The store starts holding the caller's OWN account: that account exists
/// before any call is made, so a reporting operation never has to create it.
/// Its `created_date` is fixed here, at the one instant the store comes up.
///
/// No account comes up GovCloud-LINKED. Creating a GovCloud account is an
/// Organizations operation and not one this service binds, so this emulator
/// invents no such pair: a record gains a link only when something records one
/// ([`Account::link_gov_cloud_account`]), and until then
/// `GetGovCloudAccountInformation` reports that there is no GovCloud account to
/// report.
impl Default for ComAmazonawsAccountAccountState {
    fn default() -> Self {
        let own = default_account_id().to_string();
        let mut accounts = BTreeMap::new();
        accounts.insert(own.clone(), Account::new(own, SystemTime::now()));
        Self { accounts }
    }
}

impl ComAmazonawsAccountAccountState {
    /// The account `account_id` names, for a REPORTING operation: no record is
    /// created, so a read leaves the store exactly as it found it.
    pub fn account(&self, account_id: &str) -> Option<&Account> {
        self.accounts.get(account_id)
    }

    /// The account `account_id` names, for a MUTATING operation, creating the
    /// record on first touch.
    ///
    /// An account id reaching here satisfies `^\d{12}$` (the boundary refuses
    /// anything else), and writing to a member account is what the optional
    /// `AccountId` member is for — so the accounts this emulator knows are
    /// exactly the ones it has been told about, up to [`MAX_ACCOUNTS`].
    ///
    /// `None` when the id names no record and the store is already at that
    /// bound: no record is created and nothing is written.
    ///
    /// The record comes up seeded from the id ([`Account::new`]), its primary
    /// email address included, and that address is never one another account
    /// holds: an address this emulator owes an account whose record it has not
    /// materialised yet is in use by that account from the start
    /// ([`seeded_primary_email_owed`](Self::seeded_primary_email_owed)), so no
    /// change can take it away before the record exists.
    pub fn account_mut(&mut self, account_id: &str, now: SystemTime) -> Option<&mut Account> {
        if !self.accounts.contains_key(account_id) && self.accounts.len() >= MAX_ACCOUNTS {
            return None;
        }
        Some(
            self.accounts
                .entry(account_id.to_string())
                .or_insert_with(|| Account::new(account_id.to_string(), now)),
        )
    }

    /// Materialise every primary email update completion the clock already owes,
    /// across every account — see
    /// [`Account::settle_primary_email_update`].
    pub fn settle_primary_email_updates(&mut self, now: SystemTime) {
        for account in self.accounts.values_mut() {
            account.settle_primary_email_update(now);
        }
    }

    /// Whether `address` is already in use by an account OTHER than
    /// `account_id`: another account holds it, another account's update is
    /// ACCEPTED for it, so its completion is already owed, or it is the root
    /// address this emulator owes an account whose record it has not
    /// materialised yet
    /// ([`seeded_primary_email_owed`](Self::seeded_primary_email_owed)).
    ///
    /// The question is asked ON BEHALF of one account, and it is asked about
    /// every OTHER account symmetrically — every half of the test skips
    /// `account_id`. Giving an account the address it already holds, the one its
    /// own accepted update is already owed, or the root address its own id
    /// names, takes that address from nobody and changes nothing, so it is not
    /// the conflict this service documents ("you try to change an account's root
    /// user email to an email address which is already in use").
    ///
    /// Settle first ([`settle_primary_email_updates`](Self::settle_primary_email_updates)):
    /// an address a completed update put on an account is in use from the
    /// instant that completion came due. An address a PENDING update merely
    /// asked for is NOT in use — nothing has been changed for it yet, and the
    /// passcode it waits for may never arrive.
    pub fn primary_email_in_use_by_other(&self, address: &str, account_id: &str) -> bool {
        if self
            .seeded_primary_email_owed(address)
            .is_some_and(|owed_to| owed_to != account_id)
        {
            return true;
        }
        self.accounts
            .values()
            .filter(|account| account.account_id != account_id)
            .any(|account| {
                account.primary_email == address
                    || account.primary_email_update.as_ref().is_some_and(|update| {
                        update.is_accepted() && update.requested_email == address
                    })
            })
    }

    /// The account `address` is the seeded root address of while this emulator
    /// holds no record of it — the account that address is OWED.
    ///
    /// An account's first primary email address is a function of its id alone
    /// ([`initial_primary_email`]), so the address belongs to that account from
    /// the start: materialising the record is this emulator's storage detail
    /// ([`account_mut`](Self::account_mut) creates one on first touch), not the
    /// instant the account comes to have a root user email address. Treating
    /// such an address as free would hand it to another account and then seed
    /// its own account with it on the next touch, leaving two records holding
    /// one address — and leaving the account that was handed it unable to even
    /// re-affirm the address it holds.
    ///
    /// An id whose record DOES exist is answered by that record instead: it
    /// either still holds the address, which the holder half of
    /// [`primary_email_in_use_by_other`](Self::primary_email_in_use_by_other)
    /// reports, or a completed update moved it off, which frees the address
    /// exactly as a completed update frees any other address, because nothing
    /// seeds an existing record again.
    fn seeded_primary_email_owed<'a>(&self, address: &'a str) -> Option<&'a str> {
        seeded_primary_email_account_id(address)
            .filter(|owed_to| !self.accounts.contains_key(*owed_to))
    }

    /// The account `account_id` names, for a mutating operation that must NOT
    /// bring a record into existence: `None` when this emulator holds no record
    /// of that account, and nothing is written.
    ///
    /// What the phone-number passcode operations need: they act on the primary
    /// contact information of an account that already exists, so an id naming no
    /// record is a refusal rather than a new account.
    pub fn existing_account_mut(&mut self, account_id: &str) -> Option<&mut Account> {
        self.accounts.get_mut(account_id)
    }

    /// The account `account_id` names, or the refusal for an id this emulator
    /// holds no record of — the second half of the account-target composition,
    /// stated once.
    ///
    /// Naming another account requires the caller to be an identity in the
    /// organization's management account with the named account a member of the
    /// same organization, which an account this emulator holds no record of is
    /// not: the modelled refusal is [`account_not_accessible`], never a report
    /// about an account the caller may not read. Nothing is written, so a read
    /// leaves the store exactly as it found it.
    pub fn accessible_account(
        &self,
        account_id: &str,
    ) -> Result<&Account, AccessDeniedException> {
        self.account(account_id)
            .ok_or_else(|| account_not_accessible(account_id))
    }

    /// The same half as [`accessible_account`](Self::accessible_account), for a
    /// MUTATING call that must not bring a record into existence: the record is
    /// handed back mutably, and an id naming no record is that same refusal
    /// rather than a new account.
    pub fn accessible_account_mut(
        &mut self,
        account_id: &str,
    ) -> Result<&mut Account, AccessDeniedException> {
        self.existing_account_mut(account_id)
            .ok_or_else(|| account_not_accessible(account_id))
    }

    /// The account a call TARGETS: the account-id member resolved by
    /// [`target_account_id`], then
    /// [`accessible_account`](Self::accessible_account) — the COMPOSITION every
    /// operation acting on an account that must already exist goes through.
    ///
    /// `Err` carries this service's two account-target refusals, both of them
    /// the modelled `AccessDeniedException`: the member named the caller's own
    /// account ([`cannot_target_own_account`]), or it named an account this
    /// emulator holds no record of ([`account_not_accessible`]). The refusal is
    /// decided HERE, before anything about that account's sub-resources is
    /// looked at, and a caller keeps only the mapping into its own operation's
    /// error enum.
    ///
    /// This is the REPORTING form: the record is handed back for reading, and
    /// nothing is written.
    pub fn target_account(
        &self,
        requested: Option<&str>,
    ) -> Result<&Account, AccessDeniedException> {
        self.accessible_account(&target_account_id(requested)?)
    }

    /// The same composition as [`target_account`](Self::target_account), built
    /// on [`accessible_account_mut`](Self::accessible_account_mut): the record
    /// is handed back mutably, for a call that CHANGES an account which must
    /// already exist and so must not bring one into existence (unlike
    /// [`account_mut`](Self::account_mut), which creates on first touch).
    pub fn target_account_mut(
        &mut self,
        requested: Option<&str>,
    ) -> Result<&mut Account, AccessDeniedException> {
        let account_id = target_account_id(requested)?;
        self.accessible_account_mut(&account_id)
    }

    /// The same half as [`accessible_account_mut`](Self::accessible_account_mut)
    /// for a write that MAY bring the record into existence: the record is
    /// created on first touch ([`account_mut`](Self::account_mut)), so the only
    /// id refused here is one this emulator holds no record of while the store
    /// already sits at [`MAX_ACCOUNTS`] ([`account_store_full`]) — that arm
    /// creates nothing and writes nothing.
    pub fn creatable_account_mut(
        &mut self,
        account_id: &str,
        now: SystemTime,
    ) -> Result<&mut Account, AccessDeniedException> {
        self.account_mut(account_id, now)
            .ok_or_else(|| account_store_full(account_id))
    }

    /// The account a WRITE that may bring the record into existence TARGETS:
    /// the account-id member resolved by [`target_account_id`], then
    /// [`creatable_account_mut`](Self::creatable_account_mut) — the third
    /// composition of the account-target rule, beside
    /// [`target_account`](Self::target_account) for a read and
    /// [`target_account_mut`](Self::target_account_mut) for a write that must
    /// not create a record.
    ///
    /// `Err` carries the modelled `AccessDeniedException` of either half: the
    /// member named the caller's own account
    /// ([`cannot_target_own_account`]), or the write would add a record past
    /// [`MAX_ACCOUNTS`] ([`account_store_full`]). A caller keeps only the
    /// mapping into its own operation's error enum.
    ///
    /// A handler that must decide a refusal of its OWN about another input
    /// member in between — after the account target, which is decided whole and
    /// first, and before any record is created, so a refused call writes
    /// nothing — calls the two halves in that order instead, exactly as the
    /// no-create handlers that read the store between them do. Both paths
    /// decide the same rule with the same two refusals.
    pub fn target_account_or_create(
        &mut self,
        requested: Option<&str>,
        now: SystemTime,
    ) -> Result<&mut Account, AccessDeniedException> {
        let account_id = target_account_id(requested)?;
        self.creatable_account_mut(&account_id, now)
    }
}

/// The account a call lands on when its `AccountId` member is absent: "the
/// Amazon Web Services account of the identity used to call the operation".
pub fn default_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The account id a request operates on: the member is absent, so the call is
/// the standalone one on the caller's own account ([`default_account_id`]), or
/// it names a member account of the caller's organization.
///
/// Every operation taking an account id resolves through here, whether that
/// member is optional or required: a required member always names an account,
/// so it is passed as `Some(..)` and the standalone arm simply never applies to
/// it.
///
/// `Err` when the member names the caller's OWN account: every account-id member
/// of this service documents that "the management account can't specify its own
/// `AccountId`; it must call the operation in standalone context by not
/// including the `AccountId` parameter", so that request is refused
/// ([`cannot_target_own_account`]) rather than answered as the standalone call
/// it is not.
pub fn target_account_id(requested: Option<&str>) -> Result<String, AccessDeniedException> {
    match requested {
        None => Ok(default_account_id().to_string()),
        Some(named) if named == default_account_id() => Err(cannot_target_own_account()),
        Some(named) => Ok(named.to_string()),
    }
}

/// The `AccessDeniedException` payload for an account the caller may not read.
///
/// Naming another account requires the caller to be an identity in the
/// organization's management account, with the named account a member of the
/// same organization — which an account this emulator holds no record of is not.
pub fn account_not_accessible(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "The operation failed because the calling identity doesn't have the minimum \
             required permissions for account {account_id}"
        ),
        error_type: None,
    }
}

/// The `AccessDeniedException` payload for a write that would add an account
/// past [`MAX_ACCOUNTS`]: the emulator admits no further records, so the write
/// is refused rather than stored.
pub fn account_store_full(account_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "The operation failed because this emulator already holds the maximum of \
             {MAX_ACCOUNTS} account records and cannot add account {account_id}"
        ),
        error_type: None,
    }
}

/// The one sentence Account Management states for an alternate contact that is
/// not set.
///
/// INVARIANT OF THE REQUEST: it names neither the `AlternateContactType` asked
/// for nor the account the call resolved to, so every contact type and both the
/// `GetAlternateContact` and the `DeleteAlternateContact` path state the
/// identical literal — and no account identifier is surfaced in an error body.
pub const ALTERNATE_CONTACT_NOT_FOUND_MESSAGE: &str =
    "No contact of the inputted alternate contact type found.";

/// The `ResourceNotFoundException` payload for an alternate contact that is not
/// set — "the operation failed because it specified a resource that can't be
/// found".
///
/// Takes no argument on purpose: see [`ALTERNATE_CONTACT_NOT_FOUND_MESSAGE`].
pub fn alternate_contact_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: ALTERNATE_CONTACT_NOT_FOUND_MESSAGE.to_string(),
        error_type: None,
    }
}

/// The `ValidationException` payload for a primary-email call on an account that
/// has never had an update started: there is no update to report or to accept.
///
/// `GetPrimaryEmailUpdateStatus` reports "the status of the MOST RECENT primary
/// email update" and models `Status` as REQUIRED, so an account with no update
/// has no status to report and the call is refused rather than answered with an
/// invented status; `AcceptPrimaryEmailUpdate` accepts "the request that
/// originated from StartPrimaryEmailUpdate", so with no such request there is
/// nothing it can accept. Account Management refuses that condition as a request
/// it cannot act on — the modelled 400 `ValidationException`, "the operation
/// failed because one of the input parameters was invalid" — rather than as a
/// resource that is missing: the update is not a sub-resource a caller can
/// address, it is the in-flight request the call's own parameters assume, so the
/// refusal is terminal for this request instead of the retry-or-create signal a
/// 404 reads as. Neither modelled `reason` covers it (the enum admits only an
/// invalid Region opt target and a failed field validation, and no single input
/// member is at fault), so `reason` and `fieldList` stay unset.
///
/// INVARIANT OF THE REQUEST: the message names no account id, so it is the same
/// sentence whichever account the call resolved to, as it is on the status read
/// (where the account id can be absent from the request entirely) and on the
/// acceptance.
pub fn no_primary_email_update() -> ValidationException {
    ValidationException {
        message: NO_PRIMARY_EMAIL_UPDATE_MESSAGE.to_string(),
        reason: None,
        field_list: None,
    }
}

/// The one sentence Account Management states for a primary email update that is
/// not there to be reported or accepted.
pub const NO_PRIMARY_EMAIL_UPDATE_MESSAGE: &str = "No email update found for account";

/// The `ConflictException` payload for an `AcceptPrimaryEmailUpdate` whose
/// update is not waiting for its one-time passcode: only a `PENDING` update can
/// be accepted, and "the request could not be processed because of a conflict in
/// the current status of the resource".
///
/// The message names the status the update IS in, which is what a caller needs
/// to tell "already accepted" from "nothing in flight"; it names no address,
/// because `PrimaryEmailAddress` is `@sensitive`.
pub fn primary_email_update_not_pending(status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The primary email update is in status {status}; only a \
             {PENDING_PRIMARY_EMAIL_UPDATE_STATUS} update can be accepted"
        ),
        error_type: None,
    }
}

/// The `ConflictException` payload for a `StartPrimaryEmailUpdate` on an account
/// whose previous update is still in flight: it has been accepted and the
/// address change the clock owes it has not come due yet, so there is no
/// replacing its record — "the request could not be processed because of a
/// conflict in the current status of the resource".
///
/// The counterpart of [`region_opt_in_progress`]: a change the clock still owes
/// is refused rather than discarded, so no accepted address change is lost. The
/// message names the status the update IS in and no address, because
/// `PrimaryEmailAddress` is `@sensitive`.
pub fn primary_email_update_in_progress(status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The primary email update is in status {status}; wait for it to complete before \
             starting another update"
        ),
        error_type: None,
    }
}

/// The `ValidationException` payload for an `AcceptPrimaryEmailUpdate` whose
/// `PrimaryEmail` is not the address the update was started for.
///
/// The rule is the model's own `PrimaryEmail` documentation: "This must match
/// the PrimaryEmail from the StartPrimaryEmailUpdate API call." `reason` is
/// `fieldValidationFailed` and `fieldList` names the member, so a caller can see
/// which input was refused — and neither the presented nor the requested address
/// is echoed, because `PrimaryEmailAddress` is `@sensitive`.
pub fn primary_email_does_not_match_request() -> ValidationException {
    const RULE: &str =
        "PrimaryEmail must match the PrimaryEmail from the StartPrimaryEmailUpdate API call";
    ValidationException {
        message: RULE.to_string(),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: PRIMARY_EMAIL_MEMBER.to_string(),
            message: RULE.to_string(),
        }]),
    }
}

/// The Smithy member name the primary-email operations take their address in,
/// which a refusal about that address names in its `fieldList`.
pub const PRIMARY_EMAIL_MEMBER: &str = "PrimaryEmail";

/// The Smithy member whose `@pattern` is this service's own expression of an
/// email address: `PutAlternateContactRequest$EmailAddress`, whose target
/// (`EmailAddress`) is the one address shape in this model that carries one.
const EMAIL_ADDRESS_MEMBER: &str = "EmailAddress";

/// That expression, as the model declares it, or `None` when the model declares
/// none — read off the member's own schema rather than restated here, so the
/// syntax this service enforces and the syntax its model declares cannot drift
/// apart.
fn modelled_email_expression() -> Option<&'static str> {
    let member = crate::ops::types::PUT_ALTERNATE_CONTACT_INPUT_SCHEMA
        .members()
        .iter()
        .find(|candidate| candidate.member_name() == Some(EMAIL_ADDRESS_MEMBER))?;
    let declared = member.traits()?.get(&PatternTrait::TRAIT_ID)?;
    Some(declared.as_any().downcast_ref::<PatternTrait>()?.regex)
}

/// The email-address expression this service enforces, as the model declares it,
/// or `None` when the model declares none at all.
///
/// Public because `None` would admit every value: a test asserts the model still
/// declares an expression and that it is the one this service enforces, so a
/// renamed member or an unreadable expression fails a test rather than silently
/// stopping the refusal below.
pub fn email_address_expression() -> Option<&'static str> {
    *EMAIL_ADDRESS_EXPRESSION
}

/// [`modelled_email_expression`] resolved once, so the schema below and every
/// caller read the same answer.
static EMAIL_ADDRESS_EXPRESSION: LazyLock<Option<&'static str>> =
    LazyLock::new(modelled_email_expression);

/// That expression in the framework's own form, hung on the member schema the
/// walk below is driven over.
///
/// NOTHING about pattern checking is reimplemented here: the expression is the
/// model's, carried by the framework's [`PatternTrait`], and the check is
/// [`validate`] — the same walk the request boundary runs — so a refusal decided
/// here reads exactly like one decided there.
static EMAIL_ADDRESS_TRAITS: LazyLock<::aws_smithy_schema::TraitMap> = LazyLock::new(|| {
    let mut traits = ::aws_smithy_schema::TraitMap::new();
    if let Some(declared) = *EMAIL_ADDRESS_EXPRESSION {
        traits.insert(Box::new(PatternTrait::new(declared)));
    }
    traits
});

/// The member schema the walk reads: a string carrying the email-address
/// expression. Its shape id is deliberately one the model does not declare, so
/// it cannot collide with a modelled shape, and the member name is the one both
/// address members of the primary-email operations are called.
static M_PRIMARY_EMAIL: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
    ::aws_smithy_schema::ShapeId::from_static(
        "com.amazonaws.account#CheckedAddress$PrimaryEmail",
        "com.amazonaws.account",
        "PrimaryEmail",
    ),
    ::aws_smithy_schema::ShapeType::String,
    "PrimaryEmail",
    0,
)
.with_traits(&EMAIL_ADDRESS_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is
/// empty on purpose: the walk visits what [`CheckedAddress`] WRITES, and a root
/// struct schema carries no member name of its own.
static CHECKED_ADDRESS_SCHEMA: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.account#CheckedAddress",
            "com.amazonaws.account",
            "CheckedAddress",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );

/// One address written for the validating walk the way the codec writes a string
/// member, against the member schema whose expression the walk is to apply.
struct CheckedAddress<'a> {
    value: &'a str,
}

impl ::aws_smithy_schema::serde::SerializableStruct for CheckedAddress<'_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        serializer.write_string(&M_PRIMARY_EMAIL, self.value)
    }
}

/// Refuse `value` for the member named `member` unless it is an email address by
/// that expression, or pass it through.
///
/// `PrimaryEmailAddress` carries only `@length(min: 5, max: 64)`, so WHICH
/// strings of that size name a mailbox is a rule this service enforces itself:
/// the address becomes the root user email address of an account. With no
/// expression to apply there is nothing to refuse on, so the value passes.
///
/// `Err` is the modelled 400 `ValidationException`, and its message is the one
/// this service words for the violations the walk found
/// ([`crate::service::constraint_refusal_message`]) — the message the request
/// boundary answers with for the member the expression is DECLARED on, so one
/// expression produces one wording whichever address member was refused, and the
/// refused value is echoed there exactly as the boundary echoes it. `reason` is
/// `fieldValidationFailed` and `fieldList` names the member, which a refusal the
/// boundary raises carries no room for.
pub fn validated_email_address(member: &str, value: &str) -> Result<(), ValidationException> {
    let violations = validate(&CheckedAddress { value }, &CHECKED_ADDRESS_SCHEMA);
    let Some(stated) = crate::service::constraint_refusal_message(&violations) else {
        return Ok(());
    };
    Err(ValidationException {
        message: stated.clone(),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: stated,
        }]),
    })
}

/// The `ConflictException` payload for a primary email change to an address that
/// is already in use — the conflict this service's own `ConflictException`
/// documentation gives as its example: "you try to change an account's root user
/// email to an email address which is already in use".
///
/// It names no address: `PrimaryEmailAddress` is `@sensitive`, and the address
/// in use belongs to whichever account already holds it.
pub fn primary_email_already_in_use() -> ConflictException {
    ConflictException {
        message: "The request could not be processed because the email address is already in \
                  use as the primary email address of an account"
            .to_string(),
        error_type: None,
    }
}

/// The `AccessDeniedException` payload for a request whose account-id member
/// names the caller's own account.
///
/// The model's own documentation of every account-id member states the rule: the
/// member names "a member account in the same organization", and "the management
/// account can't specify its own `AccountId`; it must call the operation in
/// standalone context by not including the `AccountId` parameter". The message
/// names no account id, so it is the same sentence whichever account the caller
/// is, and it names neither the operation nor whether that operation reads or
/// writes — the rule is the same for a read of a member account as for a write
/// of one, and the same for a member that is required as for one that is
/// optional ([`target_account_id`]).
pub fn cannot_target_own_account() -> AccessDeniedException {
    AccessDeniedException {
        message: "The operation failed because the management account can't specify its own \
                  AccountId; this operation acts on a member account of the organization"
            .to_string(),
        error_type: None,
    }
}

/// The `ResourceNotFoundException` payload for a standard account this emulator
/// holds no GovCloud link for — "the operation failed because it specified a
/// resource that can't be found". There is no GovCloud account to report, which
/// is a different fact from the account itself being unreachable
/// ([`account_not_accessible`]).
pub fn gov_cloud_account_not_linked(standard_account_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("No GovCloud account is linked to account {standard_account_id}"),
        error_type: None,
    }
}

/// The `ValidationException` payload for an `AlternateContactType` value the
/// modelled enum does not admit.
///
/// `reason` is `fieldValidationFailed` (one of `ValidationExceptionReason`'s two
/// values) and `fieldList` names the member, so a client can see which input was
/// invalid and why.
pub fn invalid_alternate_contact_type(value: &str) -> ValidationException {
    let admitted = ALTERNATE_CONTACT_TYPE_VALUES.join(" | ");
    ValidationException {
        message: format!("AlternateContactType must be one of {admitted}"),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: "AlternateContactType".to_string(),
            message: format!("{value:?} is not one of {admitted}"),
        }]),
    }
}

/// The Smithy member the Region operations take their Region code in, which a
/// refusal about that code names in its `fieldList`.
pub const REGION_NAME_MEMBER: &str = "RegionName";

/// The `ValidationException` payload for a `RegionName` that is not a Region
/// code of this partition.
///
/// `RegionName` is modelled only `@length(min: 1, max: 50)`, so the boundary
/// admits any string of that length: WHICH strings name a Region is a rule this
/// service enforces itself. `reason` is `fieldValidationFailed` and `fieldList`
/// names the member, so the caller can see which input was rejected.
pub fn unknown_region(region_name: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "{REGION_NAME_MEMBER} {region_name:?} is not an Amazon Web Services Region code"
        ),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: REGION_NAME_MEMBER.to_string(),
            message: format!("{region_name:?} is not an Amazon Web Services Region code"),
        }]),
    }
}

/// The `ValidationException` payload for a Region that cannot be opted into or
/// out of: it is enabled by default, so it is not a Region-opt target.
///
/// The refusal is about the `RegionName` the request named, and says exactly
/// that: the Region code, then that it is not a valid region for opt-in or
/// opt-out. The code is taken from the request, so the sentence is the same
/// statement whichever Region is named.
///
/// `reason` is [`FIELD_VALIDATION_FAILED`] and `fieldList` names the member the
/// refusal is ABOUT, which is "the field where the invalid entry was detected":
/// the value this request carries in `RegionName` is one the member does not
/// admit, so the request is turned away on that member alone, before anything
/// else about it is looked at — exactly as the sibling refusal of the same
/// member is ([`unknown_region`]). The two refusals of one member are told apart
/// by their sentence, not by the reason they report.
pub fn region_not_opt_target(region_name: &str) -> ValidationException {
    let stated = format!("{region_name} is not a valid region for opt-in or opt-out.");
    ValidationException {
        message: stated.clone(),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: REGION_NAME_MEMBER.to_string(),
            message: stated,
        }]),
    }
}

/// The `ConflictException` payload for a Region-opt call that conflicts with
/// the Region's current status — enabling one that is `DISABLING`, or disabling
/// one that is `ENABLING`, neither of which can be reversed before the status
/// in flight has settled.
pub fn region_opt_in_progress(region_name: &str, status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "Region {region_name} is in status {status}; wait for that operation to complete \
             before changing its opt status again"
        ),
        error_type: None,
    }
}

/// The `ConflictException` payload for a `VerifyPhoneNumber` on a phone number
/// that has no outstanding one-time passcode: the request conflicts with the
/// current status of the resource, which is what that error is modelled for.
///
/// `status` is the `PhoneNumberVerificationStatus` the number carries instead of
/// `PENDING` — `UNVERIFIED` when no passcode was ever sent to it (or the primary
/// contact's number changed since), `VERIFIED` when the passcode was already
/// submitted and consumed. No passcode is named: `Otp` is `@sensitive`.
pub fn no_outstanding_passcode(status: &str) -> ConflictException {
    ConflictException {
        message: format!(
            "The phone number in the primary contact information is in status {status}; no \
             one-time passcode is waiting to be submitted. Call SendPhoneNumberVerification \
             to send one"
        ),
        error_type: None,
    }
}

/// The Smithy member the one-time-passcode operations take their passcode in,
/// which a refusal about that passcode names in its `fieldList`.
pub const OTP_MEMBER: &str = "Otp";

/// The `ValidationException` payload for a submitted `Otp` that is not the
/// passcode this service sent, whatever the passcode was sent TO: `destination`
/// names that — the phone number for `VerifyPhoneNumber`, the proposed address
/// for `AcceptPrimaryEmailUpdate`.
///
/// WHICH six alphanumerics are the passcode is not something the model can
/// express, so the boundary admits any `^[a-zA-Z0-9]{6}$` and this service
/// refuses the wrong one itself: `reason` is `fieldValidationFailed` and
/// `fieldList` names the member, so a client can see which input was invalid.
/// Neither the submitted nor the outstanding passcode is echoed — `Otp` is
/// `@sensitive`.
fn incorrect_passcode_for(destination: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "The one-time passcode submitted in {OTP_MEMBER} is not the passcode sent to \
             {destination}"
        ),
        reason: Some(FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(vec![ValidationExceptionField {
            name: OTP_MEMBER.to_string(),
            message: "the submitted one-time passcode is incorrect".to_string(),
        }]),
    }
}

/// The `ValidationException` payload for a `VerifyPhoneNumber` whose `Otp` is
/// not the outstanding passcode: AWS sends that passcode to "the phone number in
/// the primary contact information", which is what the message names.
pub fn incorrect_passcode() -> ValidationException {
    incorrect_passcode_for("the phone number in the primary contact information")
}

/// The `ValidationException` payload for an `AcceptPrimaryEmailUpdate` whose
/// `Otp` is not the passcode the update was started with.
///
/// The model's own `Otp` documentation is the rule: it is "the OTP code sent to
/// the PrimaryEmail specified on the StartPrimaryEmailUpdate API call", which is
/// what the message names — the address itself is not echoed, because
/// `PrimaryEmailAddress` is `@sensitive`.
pub fn incorrect_primary_email_passcode() -> ValidationException {
    incorrect_passcode_for(
        "the PrimaryEmail specified on the StartPrimaryEmailUpdate API call",
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAccountAccountState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAccountAccountState>)
}
