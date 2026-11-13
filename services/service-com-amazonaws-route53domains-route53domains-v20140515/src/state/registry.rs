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

use crate::ops::types::PriceWithCurrency;
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

/// What a standard registration under `tld` costs, as the modelled `PriceWithCurrency`.
///
/// `PriceType` is `STANDARD`: this is the regular rate for the TLD, not an individual
/// name's registry premium.
pub fn standard_registration_price(tld: &str) -> PriceWithCurrency {
    PriceWithCurrency {
        price: standard_registration_rate(tld),
        currency: CURRENCY_USD.to_string(),
        price_type: Some(PRICE_TYPE_STANDARD.to_string()),
    }
}

/// What a transfer of a name under `tld` in to this registrar costs, as the modelled
/// `PriceWithCurrency`.
///
/// A transfer in carries a year of registration with it, so the registrar charges the
/// TLD's one-year rate for it — the same amount as a registration under that TLD, which
/// is why both come from [`standard_registration_rate`]. `PriceType` is `STANDARD`: this
/// is the regular rate for the TLD, not an individual name's registry premium.
pub fn standard_transfer_price(tld: &str) -> PriceWithCurrency {
    PriceWithCurrency {
        price: standard_registration_rate(tld),
        currency: CURRENCY_USD.to_string(),
        price_type: Some(PRICE_TYPE_STANDARD.to_string()),
    }
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
/// third parties', which move only on the authorization of the registrant who holds them
/// (the `AuthCode` the losing registrar issues), and this registrar has no way to obtain
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transferability {
    /// The calling account already holds the name.
    DomainInOwnAccount,
    /// Another registrant holds it, outside this registrar.
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
    /// so every answer here carries one, each naming the requested domain and what the
    /// caller would have to do about it.
    pub fn message(self, name: &str) -> String {
        match self {
            Self::DomainInOwnAccount => {
                format!("The domain {name} already exists in the current Amazon Web Services account.")
            }
            Self::HeldElsewhere => format!(
                "The domain {name} is registered with another registrant. Ask the current \
                 registrant to authorize the transfer and then try again."
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
