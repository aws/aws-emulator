// SPDX-License-Identifier: CC0-1.0

//! The registry this registrar speaks for: which names are already registered by
//! ANYBODY, and what the standard registration of a name under a TLD costs.
//!
//! Two things live here because both are properties of the registry rather than of the
//! calling account:
//!
//! * the names registered OUTSIDE this account. `CheckDomainAvailability`'s
//!   `Availability` is the registry's answer for a name — its `AVAILABLE` member is "the
//!   domain name is available" and `UNAVAILABLE` is "the domain name is not available",
//!   and the response's own note is "You can register only domains designated as
//!   AVAILABLE". A name a third party holds is therefore not available, even though this
//!   account's domain store knows nothing about it, so the registry needs a notion of
//!   third-party registrations beside
//!   [`crate::state::ComAmazonawsRoute53domainsRoute53domainsV20140515State::domains`];
//! * the standard registration price per TLD. `PriceWithCurrency` carries a required
//!   `Price` (`@range(min: 0)`), a required three-character `Currency` and a `PriceType`
//!   whose `STANDARD` member "means the domain is priced at the regular rate for its
//!   TLD". A standard rate belongs to the TLD, not to the request, so it comes from a
//!   stable table here: the same name always costs the same, and two names under one TLD
//!   cost the same.

use crate::ops::types::{Consent, DomainPrice, PriceWithCurrency};
use crate::state::request_problem;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Modelled enum members this slice writes
// ---------------------------------------------------------------------------

/// `AVAILABLE`, from the model's `DomainAvailability` enum
/// (`crate::ops::types::DOMAIN_AVAILABILITY_VALUES`): "The domain name is available."
pub const AVAILABILITY_AVAILABLE: &str = "AVAILABLE";
/// `UNAVAILABLE`, from the model's `DomainAvailability` enum: "The domain name is not
/// available."
pub const AVAILABILITY_UNAVAILABLE: &str = "UNAVAILABLE";

/// `UNTRANSFERABLE`, from the model's `Transferable` enum
/// (`crate::ops::types::TRANSFERABLE_VALUES`): "The domain name can't be transferred to
/// Route 53."
pub const TRANSFERABLE_UNTRANSFERABLE: &str = "UNTRANSFERABLE";
/// `DOMAIN_IN_OWN_ACCOUNT`, from the model's `Transferable` enum: "The domain already
/// exists in the current Amazon Web Services account."
pub const TRANSFERABLE_DOMAIN_IN_OWN_ACCOUNT: &str = "DOMAIN_IN_OWN_ACCOUNT";

/// `STANDARD`, from the model's `PriceType` enum
/// (`crate::ops::types::PRICE_TYPE_VALUES`): the domain "is priced at the regular rate
/// for its TLD", as opposed to `REGISTRY_PREMIUM`, which prices an individual name.
pub const PRICE_TYPE_STANDARD: &str = "STANDARD";

/// The `Currency` every price this registrar quotes is denominated in. Route 53 publishes
/// its domain registration prices in US dollars, and `Currency` is `@length(3, 3)`, so
/// the member carries the ISO 4217 alphabetic code for them.
pub const CURRENCY_USD: &str = "USD";

// ---------------------------------------------------------------------------
// What a standard registration costs
// ---------------------------------------------------------------------------

/// The standard yearly registration rate, in [`CURRENCY_USD`], of the TLDs this table
/// carries — the per-TLD prices Route 53 publishes in its domain registration price list
/// (the same rates `ListPrices` reports as `RegistrationPrice`).
///
/// A rate belongs to the TLD: it is the same for every name under it and the same on
/// every call, which is what makes it a table rather than a per-request value. A TLD the
/// table does not carry takes [`DEFAULT_STANDARD_REGISTRATION_RATE`].
pub const STANDARD_REGISTRATION_RATES: &[(&str, f64)] = &[
    ("ai", 110.00),
    ("app", 20.00),
    ("biz", 20.00),
    ("ca", 17.00),
    ("cloud", 24.00),
    ("co", 35.00),
    ("co.uk", 10.00),
    ("com", 16.00),
    ("com.au", 20.00),
    ("de", 10.00),
    ("dev", 15.00),
    ("es", 13.00),
    ("eu", 12.00),
    ("fr", 12.00),
    ("in", 14.00),
    ("info", 20.00),
    ("io", 47.00),
    ("it", 12.00),
    ("jp", 95.00),
    ("me", 25.00),
    ("net", 17.00),
    ("nl", 14.00),
    ("org", 16.00),
    ("org.uk", 10.00),
    ("us", 13.00),
    ("xyz", 14.00),
];

/// The standard yearly registration rate this registrar quotes for a TLD
/// [`STANDARD_REGISTRATION_RATES`] does not carry.
///
/// Route 53 offers hundreds of TLDs, and a price is only useful to a caller if it is
/// always there, so this is the rate this emulator states for the rest of them. It is a
/// constant, so the answer for one TLD never changes between calls.
pub const DEFAULT_STANDARD_REGISTRATION_RATE: f64 = 20.00;

/// The standard yearly registration rate for `tld`, in [`CURRENCY_USD`].
///
/// The table's keys are lower-cased; the comparison ignores case, because domain
/// names — and so the TLDs taken out of them — are not case sensitive.
pub fn standard_registration_rate(tld: &str) -> f64 {
    STANDARD_REGISTRATION_RATES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(tld))
        .map_or(DEFAULT_STANDARD_REGISTRATION_RATE, |(_, rate)| *rate)
}

/// What an owner change costs at this registrar, in [`CURRENCY_USD`].
///
/// A price record carries an "Owner change" price beside the registration, transfer,
/// renewal and restoration prices (`ListPrices`'s own documentation lists the five).
/// Handing a registration to a new owner buys no registration year from the registry, and
/// this registrar's published price list charges nothing for the change itself, so the
/// amount is the same for every TLD.
pub const CHANGE_OWNERSHIP_RATE: f64 = 0.00;

/// What restoring an expired registration costs, in [`CURRENCY_USD`].
///
/// The restoration price is the fee for pulling a registration back out of the registry's
/// redemption period, which is registrar work rather than another registration year: this
/// registrar's published price list states one flat fee for it, the same for every TLD.
pub const RESTORATION_RATE: f64 = 40.00;

/// `amount` as a modelled `PriceWithCurrency` at a TLD's regular rate.
///
/// The ONE place a price this registrar quotes is built: `Price` carries the amount,
/// `Currency` the code every quote is denominated in ([`CURRENCY_USD`]), and `PriceType`
/// is `STANDARD` — the regular rate for a TLD, not an individual name's registry premium.
fn standard_price(amount: f64) -> PriceWithCurrency {
    PriceWithCurrency {
        price: amount,
        currency: CURRENCY_USD.to_string(),
        price_type: Some(PRICE_TYPE_STANDARD.to_string()),
    }
}

/// What a standard registration under `tld` costs, as the modelled `PriceWithCurrency`.
///
/// `PriceType` is `STANDARD`: this is the regular rate for the TLD, not an individual
/// name's registry premium.
pub fn standard_registration_price(tld: &str) -> PriceWithCurrency {
    standard_price(standard_registration_rate(tld))
}

/// What a transfer of a name under `tld` in to this registrar costs, as the modelled
/// `PriceWithCurrency`.
///
/// A transfer in carries a year of registration with it, so the registrar charges the
/// TLD's one-year rate for it — the same amount as a registration under that TLD, which
/// is why both come from [`standard_registration_rate`]. `PriceType` is `STANDARD`: this
/// is the regular rate for the TLD, not an individual name's registry premium.
pub fn standard_transfer_price(tld: &str) -> PriceWithCurrency {
    standard_price(standard_registration_rate(tld))
}

/// What renewing a name under `tld` for a year costs, as the modelled
/// `PriceWithCurrency`.
///
/// A renewal buys exactly what a registration buys — one more year of the registration —
/// so it costs the TLD's standard yearly rate, the same amount as the registration did.
pub fn standard_renewal_price(tld: &str) -> PriceWithCurrency {
    standard_price(standard_registration_rate(tld))
}

/// What an owner change costs, as the modelled `PriceWithCurrency`.
///
/// [`CHANGE_OWNERSHIP_RATE`] belongs to the registrar rather than to the TLD, so this
/// takes no TLD.
pub fn change_ownership_price() -> PriceWithCurrency {
    standard_price(CHANGE_OWNERSHIP_RATE)
}

/// What restoring an expired registration costs, as the modelled `PriceWithCurrency`.
///
/// [`RESTORATION_RATE`] belongs to the registrar rather than to the TLD, so this takes no
/// TLD.
pub fn restoration_price() -> PriceWithCurrency {
    standard_price(RESTORATION_RATE)
}

/// Every price of a name under `tld`, reported under `name`, as the modelled
/// `DomainPrice`.
///
/// One rule for every surface that quotes prices: the five prices a `DomainPrice` carries
/// are derived from the TLD's standard yearly rate — registration, transfer and renewal
/// each buy one year at that rate — plus the registrar's own owner-change and restoration
/// fees. `name` is what `DomainPrice$Name` reports the prices apply to ("The TLD or domain
/// name that the prices apply to"): the TLD when a caller asks about a TLD, and the domain
/// name when it asks about one name.
pub fn price_record(name: &str, tld: &str) -> DomainPrice {
    DomainPrice {
        name: Some(name.to_string()),
        registration_price: Some(standard_registration_price(tld)),
        transfer_price: Some(standard_transfer_price(tld)),
        renewal_price: Some(standard_renewal_price(tld)),
        change_ownership_price: Some(change_ownership_price()),
        restoration_price: Some(restoration_price()),
    }
}

// ---------------------------------------------------------------------------
// What a caller agreed to pay
// ---------------------------------------------------------------------------

/// The problem to refuse a request with when its `Consent` does not cover `price`, or
/// `None` when it does.
///
/// `Consent` is "[t]he maximum amount that you agree to pay for an operation, and the
/// currency for that amount", and an operation that takes one states the rule it applies:
/// if the request includes it and the operation's price "is more than MaxPrice, the
/// request fails". So consent covers a price when it is denominated in the currency the
/// price is quoted in — every quote here is in [`CURRENCY_USD`] — and its `MaxPrice` is at
/// least that price. A currency code is not case sensitive, so the codes are compared
/// ignoring case.
///
/// The problem is stated in the envelope this service words a request-level problem in
/// ([`request_problem`]). It names the MEMBER the caller has to change and the currency
/// this registrar quotes in, and never the amount the request carried: the price and the
/// currency are this registrar's and the member name is the model's, while the submitted
/// amount is the caller's own value.
pub fn consent_shortfall(consent: &Consent, price: &PriceWithCurrency) -> Option<String> {
    if !consent.currency.eq_ignore_ascii_case(&price.currency) {
        return Some(request_problem(&format!(
            "The Currency of MaxPrice must be {}",
            price.currency
        )));
    }
    (consent.max_price < price.price).then(|| {
        request_problem("The price of this operation is more than the MaxPrice you agreed to pay")
    })
}

/// Every TLD this registrar publishes a price for, in ascending TLD order.
///
/// The keys of [`STANDARD_REGISTRATION_RATES`], which IS this registrar's published price
/// list: a caller that asks for the prices of all the TLDs the registrar supports is
/// asking for that list. A name under a TLD outside it is still registrable — it takes
/// [`DEFAULT_STANDARD_REGISTRATION_RATE`] — but the registrar publishes no price list row
/// for it.
pub fn priced_tlds() -> Vec<&'static str> {
    let mut tlds: Vec<&'static str> = STANDARD_REGISTRATION_RATES
        .iter()
        .map(|(tld, _)| *tld)
        .collect();
    tlds.sort_unstable();
    tlds
}

// ---------------------------------------------------------------------------
// Whether a name can be transferred in
// ---------------------------------------------------------------------------

/// What the registry's record of a name means for transferring that name IN to this
/// registrar — the question `CheckDomainTransferability` answers.
///
/// A transfer in moves an EXISTING registration: `TransferDomain` "[t]ransfers a domain
/// from another registrar to Amazon Route 53". So the answer turns on where the name's
/// registration lives rather than on whether the name could be registered, and each of
/// the three situations this registry can be in for a name has its own answer.
///
/// Three of the model's `Transferable` members cannot arise from what this registry
/// knows: `DONT_KNOW` and `PENDING` both describe a registry that did not answer in
/// time, which never happens to an in-process store; `PREMIUM_DOMAIN` describes a name
/// with registry premium pricing, and every name here is priced at its TLD's standard
/// rate; and `DOMAIN_IN_ANOTHER_ACCOUNT` describes a name held by another Amazon Web
/// Services account, which this emulator has no notion of — a name it holds for somebody
/// else is held OUTSIDE Route 53. Nor is a name ever `TRANSFERABLE`: the only
/// registrations this registry holds are this account's own, which are here already, and
/// those outside it, which the registry keeps transfer-locked, and this registrar cannot
/// clear a lock on a registration it does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transferability {
    /// The calling account already holds the name.
    DomainInOwnAccount,
    /// A registration outside this account holds the name, and the registry's transfer
    /// lock on it is not cleared, so there is nothing this registrar can move in.
    HeldElsewhere,
    /// No registry holds the name, so there is no registration to move at all.
    NotRegistered,
}

impl Transferability {
    /// The `Transferable` enum member this answer is reported as.
    pub fn member(self) -> &'static str {
        match self {
            Self::DomainInOwnAccount => TRANSFERABLE_DOMAIN_IN_OWN_ACCOUNT,
            Self::HeldElsewhere | Self::NotRegistered => TRANSFERABLE_UNTRANSFERABLE,
        }
    }

    /// The explanation `CheckDomainTransferabilityResponse`'s `Message` carries, which
    /// the member's own documentation returns "for when a domain can't be transferred" —
    /// so every answer here carries one, each naming the requested domain and the action
    /// that would unblock the transfer.
    ///
    /// A registration outside this account is blocked by the REGISTRY's transfer lock
    /// (`clientTransferProhibited`), not by who holds it: clearing that lock is what a
    /// transfer in actually requires, so that is the action the explanation names. The
    /// name is the requested `DomainName`, resolved (and so lower-cased) by
    /// [`crate::state::resolve_domain_name`], and is the sentence's only variable part.
    pub fn message(self, name: &str) -> String {
        match self {
            Self::DomainInOwnAccount => {
                format!("The domain {name} already exists in the current Amazon Web Services account.")
            }
            Self::HeldElsewhere => format!(
                "The domain {name} is locked at the registry. Unlock the domain at the registry \
                 and try again."
            ),
            Self::NotRegistered => format!(
                "The domain {name} isn't registered. Register the domain first and then try again."
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Names registered outside this account
// ---------------------------------------------------------------------------

/// Second-level labels that are registered under every suffix in
/// [`REGISTERED_SUFFIXES`]: names whose owners hold them in the real registries across
/// the common generic TLDs, so no registrar would sell one. `example` is among them
/// because `example.com`, `example.net` and `example.org` are reserved for documentation
/// and registered to IANA (RFC 2606 §3).
const REGISTERED_LABELS: &[&str] = &[
    "amazon",
    "apple",
    "aws",
    "cloudflare",
    "example",
    "facebook",
    "github",
    "google",
    "instagram",
    "linkedin",
    "microsoft",
    "netflix",
    "openai",
    "reddit",
    "twitter",
    "wikipedia",
    "yahoo",
    "youtube",
];

/// The suffixes each label in [`REGISTERED_LABELS`] is taken under.
const REGISTERED_SUFFIXES: &[&str] = &["com", "net", "org"];

/// Further names held in the real registries, listed in full because their label and
/// suffix do not pair up with the cross product above.
const REGISTERED_NAMES: &[&str] = &[
    "amazon.co.uk",
    "amazon.de",
    "bbc.co.uk",
    "google.co.uk",
    "google.de",
    "iana.org",
    "icann.org",
    "x.com",
];

/// Every name this registry holds for somebody other than the calling account.
///
/// Seeded once into
/// [`crate::state::ComAmazonawsRoute53domainsRoute53domainsV20140515State`], so the set
/// is the same for the whole life of the process and a caller cannot register a name out
/// of it. Keys are lower-cased, like the domain store's, because domain names are not
/// case sensitive.
pub fn seeded_registrations() -> BTreeSet<String> {
    let mut names: BTreeSet<String> = REGISTERED_LABELS
        .iter()
        .flat_map(|label| {
            REGISTERED_SUFFIXES
                .iter()
                .map(move |suffix| format!("{label}.{suffix}"))
        })
        .collect();
    names.extend(REGISTERED_NAMES.iter().map(|name| (*name).to_string()));
    names
}

/// The message the `InvalidInput` refusal of a registration of a name the registry
/// already holds carries.
///
/// `CheckDomainAvailability`'s `Availability` note is the rule being enforced: "You can
/// register only domains designated as AVAILABLE".
pub fn unavailable_domain_message(name: &str) -> String {
    format!("Domain {name} is not available for registration")
}
