// SPDX-License-Identifier: CC0-1.0

//! The DNS side of a public certificate's domain validation: whether the CNAME this
//! service handed out for a covered name is actually published, and what that fact
//! does to the certificate's pending outcome.
//!
//! `DomainValidation$ResourceRecord` "contains the CNAME record that you add to your
//! DNS database for domain validation", and `DomainValidation$ValidationStatus` is
//! `PENDING_VALIDATION` until that record proves control of the name. ACM resolves it
//! itself, over and over, for the whole documented [`VALIDATION_WINDOW`] — so a
//! certificate whose records are all published is one ACM has proven and issues, and
//! only a window that elapses with a record still missing is
//! [`STATUS_VALIDATION_TIMED_OUT`]. An unpublished record is never a request FAILURE:
//! `FailureReason` "exists only when the certificate status is FAILED"
//! (`CertificateDetail$FailureReason` doc) and no value that enum declares names a
//! challenge nobody published yet.
//!
//! The same store answers the other DNS question a public request turns on: whether the
//! names it covers are in the public DNS at all ([`any_name_is_not_in_public_dns_in`]).
//! A name the registries reserve is held by nobody UNLESS the account is itself
//! authoritative for it here — a public hosted zone is a delegation the account publishes
//! the name from — and only a name with no holder at all is the up-front request failure.
//!
//! Whether the record is published is the DNS service's fact, not this one's, so it is
//! READ from the Route 53 store in the shared engine: the record set has to carry the
//! name, the type and the value this service handed out, in a PUBLIC hosted zone of the
//! account (a private zone resolves nowhere the public DNS can see it, so it proves
//! nothing). Nothing is copied into a certificate's record, so a zone that stops
//! publishing the CNAME stops proving the name.
//!
//! The transition itself is OWED, never slept on, exactly as every other transition
//! this store carries: the pass that first observes the published records replaces the
//! owed outcome with issuance [`VALIDATION_SETTLE_DELAY`] out, and whichever call next
//! touches the store materialises it. That span is never zero, so a caller that
//! publishes the CNAME and reads straight afterwards still sees `PENDING_VALIDATION`
//! and the evidence it is waiting on.

use std::time::SystemTime;

use ::service_com_amazonaws_route53_awsdnsv20130401::state as dns_store;

use super::*;

impl ComAmazonawsAcmCertificatemanagerState {
    /// Apply everything the clock already owes at `now`, resolving each pending DNS
    /// validation against the DNS in `engine` first.
    ///
    /// This is the pass a handler that reads or acts on a CERTIFICATE makes: the
    /// published-CNAME lookup can only change a certificate record, so the ACME
    /// resources beside them need nothing from the DNS and go through
    /// [`Self::materialize_due`] as they did.
    ///
    /// Resolving BEFORE materialising is what lets one call both observe the proof and
    /// report an issuance that had already fallen due: the proof fixes the instant, and
    /// the pass behind it applies whatever that instant has already made due.
    pub fn materialize_due_in<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        self.resolve_dns_validations(engine, now);
        self.materialize_due(now);
    }

    /// Resolve every pending DNS validation against the account's hosted zones.
    ///
    /// An engine carrying no DNS store resolves nothing: no zone exists there, so no
    /// name is proven and every pending request keeps the outcome it already owed.
    ///
    /// LOCK ORDER: this store is already held by the handler that called in, and the DNS
    /// store is taken under it — so this service locks ACM then Route 53, never the
    /// other way round. A cross-read added in the other direction has to use the same
    /// order.
    fn resolve_dns_validations<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        let Some(zones) = dns_store::service_state(engine) else {
            return;
        };
        let zones = zones.lock();
        for certificate in self.certificates.values_mut() {
            resolve_dns_validation(certificate, &zones, now);
        }
    }
}

/// Whether any of `domain_names` names no domain in the public DNS at all, so that
/// NOBODY could publish evidence of control over it.
///
/// Two facts decide it, and a name needs both to be unholdable:
///
/// * the registries reserve the name rather than delegating it
///   ([`domain_is_not_in_public_dns`]), and
/// * no PUBLIC hosted zone of the account is authoritative for it
///   ([`zone_delegates`]) — a zone the account holds is a delegation it publishes the
///   name from, so the name has a holder, and that holder is the caller.
///
/// Which names are delegated is the DNS service's fact, not this one's, exactly as
/// whether a challenge is published is: it is READ from the Route 53 store in the shared
/// engine, so a zone created or deleted there changes this answer with it and nothing is
/// copied into a certificate's record. Every name is judged under ONE acquisition of
/// that store, and a name the registries delegate needs no lookup at all.
///
/// This is what a certificate request is refused up front for ("unless it fails for any
/// of the reasons given in the troubleshooting topic Certificate request fails",
/// `CertificateDetail$Status` doc) — and one unholdable name is enough, since "each
/// domain name that you specify must be validated" (`RequestCertificate` doc). Every
/// other public request names domains whose holder COULD publish the evidence, so it
/// enters `PENDING_VALIDATION` and waits the documented [`VALIDATION_WINDOW`] out — or
/// reaches issuance once the challenge is published ([`resolve_dns_validation`]).
///
/// LOCK ORDER: the ACM store is held by the handler that calls in and the DNS store is
/// taken under it, the same order [`ComAmazonawsAcmCertificatemanagerState::materialize_due_in`]
/// uses.
pub fn any_name_is_not_in_public_dns_in<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    domain_names: &[String],
) -> bool {
    // The name ACM proves control of, for each name the registries reserve — the only
    // ones a delegation of the account's could still answer for.
    let reserved: Vec<&str> = domain_names
        .iter()
        .filter(|name| domain_is_not_in_public_dns(name))
        .map(|name| validation_target(name))
        .collect();
    if reserved.is_empty() {
        return false;
    }
    // An engine carrying no DNS store holds no zone, so it delegates nothing.
    let Some(zones) = dns_store::service_state(engine) else {
        return true;
    };
    let zones = zones.lock();
    reserved
        .iter()
        .any(|name| !zones.zones.values().any(|zone| zone_delegates(zone, name)))
}

/// Whether `zone` is a PUBLIC hosted zone authoritative for `domain_name`: its apex is
/// that name or a parent of it.
///
/// A private zone resolves nowhere the public DNS can see it, so it delegates no public
/// name — the same reason it proves no challenge ([`record_is_published`]). Which names
/// one zone answers for is the DNS service's own rule (`name_is_in_zone`), which
/// compares label by label, so the trailing dot and the letter case a zone was created
/// with do not decide it and a sibling name is not inside the zone.
fn zone_delegates(zone: &dns_store::StoredZone, domain_name: &str) -> bool {
    !zone.private_zone && dns_store::name_is_in_zone(domain_name, &zone.name)
}

/// Resolve one certificate's pending DNS validation against the published DNS.
///
/// Only a PUBLIC request still waiting on proof of domain control has anything to
/// resolve: a request naming a private certificate authority needs no proof ("if you
/// are requesting a private certificate, domain validation is not required",
/// `RequestCertificate` doc), and one that has reached a terminal status is done.
///
/// The outcome moves in both directions, because the proof is read rather than
/// remembered: every covered name proven takes the request to issuance, and evidence
/// withdrawn before that issuance falls due puts the request back to waiting the
/// documented [`VALIDATION_WINDOW`] out from the instant it was made.
fn resolve_dns_validation(
    certificate: &mut Certificate,
    zones: &dns_store::ComAmazonawsRoute53Awsdnsv20130401State,
    now: SystemTime,
) {
    if certificate.status != STATUS_PENDING_VALIDATION
        || certificate.certificate_authority_arn.is_some()
    {
        return;
    }
    let Some(owed) = certificate
        .pending_validation
        .as_ref()
        .map(|pending| pending.outcome)
    else {
        return;
    };
    let proven = certificate_is_proven(certificate, zones);
    match (proven, owed) {
        (true, ValidationOutcome::TimedOut) => {
            certificate.pending_validation = Some(PendingValidation {
                due_at: now + VALIDATION_SETTLE_DELAY,
                outcome: ValidationOutcome::Issued,
            });
        }
        (false, ValidationOutcome::Issued) => {
            certificate.pending_validation = Some(PendingValidation {
                due_at: certificate.created_at + VALIDATION_WINDOW,
                outcome: ValidationOutcome::TimedOut,
            });
        }
        // A request ACM refuses up front keeps that refusal whatever is published — the
        // reasons it fails are the modelled `FailureReason` values, which a DNS record
        // does not answer — and a proof already recorded needs no second recording.
        _ => {}
    }
}

/// Whether every name this certificate covers is proven by a published DNS record.
///
/// "Each domain name that you specify must be validated" (`RequestCertificate` doc), so
/// one unproven name leaves the whole request waiting. A name validated by any other
/// method is not proven HERE: email validation's evidence is a mailbox and HTTP
/// validation's is a fetched URL, neither of which the DNS answers.
fn certificate_is_proven(
    certificate: &Certificate,
    zones: &dns_store::ComAmazonawsRoute53Awsdnsv20130401State,
) -> bool {
    !certificate.domain_validations.is_empty()
        && certificate.domain_validations.iter().all(|validation| {
            validation.validation_method == VALIDATION_METHOD_DNS
                && validation
                    .resource_record
                    .as_ref()
                    .is_some_and(|record| record_is_published(record, zones))
        })
}

/// Whether `record` — the name, type and value this service handed out — is published
/// in one of the account's PUBLIC hosted zones.
///
/// Which record sets answer for a name and type is the DNS service's own lookup
/// (`StoredZone::records_at`), so the key order it rests on stays single-homed in the
/// crate that owns it, and its name identity decides which spellings of the name match
/// (the trailing dot, the letter case and the escaping a publisher wrote it with do
/// not). A CNAME's value is itself a domain name, so it is compared in that same
/// canonical form. An alias record set answers no value of its own, so it proves
/// nothing.
fn record_is_published(
    record: &ResourceRecord,
    zones: &dns_store::ComAmazonawsRoute53Awsdnsv20130401State,
) -> bool {
    let value = dns_store::canonical_dns_name(&record.value);
    zones.zones.values().any(|zone| {
        !zone.private_zone
            && zone
                .records_at(&record.name, &record.r#type)
                .iter()
                .any(|published| {
                    published
                        .resource_records
                        .iter()
                        .flatten()
                        .any(|data| dns_store::canonical_dns_name(&data.value) == value)
                })
    })
}
