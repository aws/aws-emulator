// SPDX-License-Identifier: CC0-1.0

//! The SERVING MATERIAL of a Certificate Manager certificate: the leaf, its chain and the
//! private key that matches it, as the service that TERMINATES a connection with that
//! certificate has to see them.
//!
//! A certificate is held by Certificate Manager but PRESENTED by another service: a load
//! balancer listener presents one on the wire, a custom domain name presents one. The
//! presenting service cannot see Certificate Manager's own store, and the handshake it has
//! to run needs more than the certificate's description — it needs the certificate bytes
//! and the key that signs with them. This is that record: ONE directory in the shared
//! engine, written by Certificate Manager when a caller brings key material into it (its
//! `ImportCertificate`, "the private key that matches the public key in the certificate")
//! and read by the service that serves a connection under it.
//!
//! The key NEVER leaves this process: it is held in memory for as long as the certificate
//! is, written to no file and named in no log — [`CertificateKeyMaterial`]'s own `Debug`
//! reports the names and the sizes and redacts the key, so a record that reaches a
//! formatter cannot carry it.
//!
//! The directory is BOUNDED in the one dimension a caller can grow — how many certificates
//! it holds material for ([`MAX_CERTIFICATES`]) — so no sequence of imports grows it
//! without end: past the ceiling the OLDEST entry is dropped, which keeps the certificate
//! a running workflow just imported serving.
//!
//! WHICH certificate a client's handshake selects is decided by the name it asked for, and
//! the names a certificate covers are the certificate's own: they are recorded beside the
//! material ([`CertificateKeyMaterial::covers`]), so the service that selects one reads the
//! names Certificate Manager parsed out of the certificate rather than parsing X.509 again.
//!
//! LOCK ORDER: a service takes its OWN store first and this directory second, and nothing
//! holds this directory while taking another store — so two services cannot invert the
//! order between them.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// How many certificates the directory holds serving material for at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so without a bound a caller importing certificates in a loop would grow
/// it without end. It is set at the number of certificates Certificate Manager's own store
/// holds, so material is kept for every certificate that store can hold and no more; past
/// it the OLDEST entry is dropped, never a newer one.
pub const MAX_CERTIFICATES: usize = 1_000;

/// What one certificate is SERVED from: the leaf, the chain it is sent with, the private
/// key that matches it, and the names it covers.
///
/// Every member is the caller's own material, held as it was brought in — PEM text, the
/// encoding `ImportCertificate` takes and `GetCertificate` reports — so the certificate a
/// client is presented is byte for byte the one the account imported.
pub struct CertificateKeyMaterial {
    /// The PEM-encoded leaf certificate.
    pub certificate_pem: String,
    /// The PEM-encoded chain sent with the leaf, and `None` for material brought in
    /// without one.
    pub certificate_chain_pem: Option<String>,
    /// The PEM-encoded, unencrypted private key that matches the leaf's public key.
    ///
    /// SECRET. It is held here so a handshake can be run with it and for nothing else:
    /// never written to a file, never put in a response, never logged.
    pub private_key_pem: String,
    /// Every name the certificate covers, as the service that read the certificate parsed
    /// them out of it: the subject common name and the DNS subject alternative names, a
    /// wildcard name included.
    pub domain_names: Vec<String>,
}

impl std::fmt::Debug for CertificateKeyMaterial {
    /// Everything about this record except the key: the names it covers and the sizes of
    /// the material, so a record that reaches a formatter — a log line, a state dump —
    /// cannot carry the private key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateKeyMaterial")
            .field("domain_names", &self.domain_names)
            .field("certificate_pem_len", &self.certificate_pem.len())
            .field(
                "certificate_chain_pem_len",
                &self.certificate_chain_pem.as_ref().map(String::len),
            )
            .field("private_key_pem", &"[redacted]")
            .finish()
    }
}

impl CertificateKeyMaterial {
    /// Whether this certificate covers `server_name` — the host a client asked for.
    ///
    /// The match is the one a TLS client applies to a certificate's names (RFC 6125): a
    /// name is matched case-insensitively, and a name whose LEFTMOST label is `*` matches
    /// any single label in that position, so `*.example.com` covers `a.example.com` and
    /// neither `example.com` nor `a.b.example.com`.
    #[must_use]
    pub fn covers(&self, server_name: &str) -> bool {
        self.domain_names
            .iter()
            .any(|name| name_covers(name, server_name))
    }
}

/// Whether the certificate name `name` covers the host `asked_for`: the ONE statement of
/// that match, so every service that decides whether a certificate serves a hostname —
/// the one running the handshake ([`CertificateKeyMaterial::covers`]) and one judging a
/// configuration that names a certificate for a host before any connection is made —
/// decides alike.
///
/// The match is the one a TLS client applies to a certificate's names (RFC 6125): a name is
/// matched case-insensitively, a trailing root dot on either side names the same host, and
/// a name whose LEFTMOST label is `*` matches any single label in that position, so
/// `*.example.com` covers `a.example.com` and neither `example.com` nor `a.b.example.com`.
#[must_use]
pub fn name_covers(name: &str, asked_for: &str) -> bool {
    let name = name.trim().trim_end_matches('.');
    let asked_for = asked_for.trim().trim_end_matches('.');
    if name.is_empty() || asked_for.is_empty() {
        return false;
    }
    if let Some(suffix) = name.strip_prefix("*.") {
        let Some((_label, rest)) = asked_for.split_once('.') else {
            return false;
        };
        return !rest.is_empty() && rest.eq_ignore_ascii_case(suffix);
    }
    name.eq_ignore_ascii_case(asked_for)
}

/// The serving material of each certificate the account has brought key material into
/// Certificate Manager for.
///
/// Registered in the shared engine under THIS type, so the service that holds the
/// certificate and the service that presents it read one directory instead of each keeping
/// its own copy.
#[derive(Debug, Default)]
pub struct CertificateKeyMaterialDirectory {
    /// The material of each certificate, by certificate ARN. Behind an `Arc`, so a reader
    /// takes the material off the directory and releases the lock before running a
    /// handshake with it.
    by_arn: BTreeMap<String, Arc<CertificateKeyMaterial>>,
    /// Import order, so the ceiling drops the oldest certificate's material first.
    order: VecDeque<String>,
}

impl CertificateKeyMaterialDirectory {
    /// Record that the certificate `certificate_arn` is served from `material`, replacing
    /// what was recorded for it before — which is what a re-import of the same certificate
    /// resource does.
    ///
    /// A blank ARN names no certificate and records nothing. Past [`MAX_CERTIFICATES`] the
    /// OLDEST entry is dropped, so no sequence of imports grows the directory without end.
    pub fn record(&mut self, certificate_arn: &str, material: CertificateKeyMaterial) {
        let certificate_arn = certificate_arn.trim();
        if certificate_arn.is_empty() {
            return;
        }
        let fresh = !self.by_arn.contains_key(certificate_arn);
        self.by_arn
            .insert(certificate_arn.to_string(), Arc::new(material));
        if fresh {
            self.order.push_back(certificate_arn.to_string());
            while self.order.len() > MAX_CERTIFICATES {
                if let Some(oldest) = self.order.pop_front() {
                    self.by_arn.remove(&oldest);
                }
            }
        }
    }

    /// The material the certificate `certificate_arn` is served from, or `None` for a
    /// certificate this directory holds none for — one issued rather than imported, or one
    /// whose material the ceiling dropped.
    #[must_use]
    pub fn material(&self, certificate_arn: &str) -> Option<Arc<CertificateKeyMaterial>> {
        self.by_arn.get(certificate_arn.trim()).map(Arc::clone)
    }

    /// Forget the material of `certificate_arn`: called by the service that holds the
    /// certificate when the certificate itself is gone, so no key outlives the resource it
    /// belonged to. A no-op for a certificate the directory holds nothing for.
    pub fn forget(&mut self, certificate_arn: &str) {
        let certificate_arn = certificate_arn.trim();
        if self.by_arn.remove(certificate_arn).is_some() {
            self.order.retain(|held| held != certificate_arn);
        }
    }

    /// How many certificates the directory holds material for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_arn.len()
    }

    /// Whether the directory holds no material at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_arn.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ACM certificate ARN, numbered.
    fn certificate(index: usize) -> String {
        format!("arn:aws:acm:us-west-2:000000000000:certificate/cert-{index:06}")
    }

    /// Material for a certificate covering `names`. The PEM text stands for the material:
    /// nothing here parses it.
    fn material(index: usize, names: &[&str]) -> CertificateKeyMaterial {
        CertificateKeyMaterial {
            certificate_pem: format!("-----BEGIN CERTIFICATE-----\n{index}\n-----END CERTIFICATE-----\n"),
            certificate_chain_pem: None,
            private_key_pem: format!("-----BEGIN PRIVATE KEY-----\n{index}\n-----END PRIVATE KEY-----\n"),
            domain_names: names.iter().map(|name| (*name).to_string()).collect(),
        }
    }

    #[test]
    fn recorded_material_is_read_back_by_certificate_arn() {
        let mut directory = CertificateKeyMaterialDirectory::default();
        directory.record(&certificate(1), material(1, &["a.example.com"]));

        let held = directory.material(&certificate(1));
        assert!(
            held.as_ref()
                .is_some_and(|held| held.certificate_pem.contains('1')),
            "case a recorded certificate: expected its material, got {held:?}"
        );
        assert!(
            directory.material(&certificate(2)).is_none(),
            "case a certificate nothing was recorded for: expected no material"
        );

        // A re-import of the same certificate resource replaces the material.
        directory.record(&certificate(1), material(7, &["b.example.com"]));
        let replaced = directory.material(&certificate(1));
        assert!(
            replaced
                .as_ref()
                .is_some_and(|held| held.certificate_pem.contains('7')),
            "case a re-imported certificate: expected the new material, got {replaced:?}"
        );
        assert_eq!(
            directory.len(),
            1,
            "case a re-imported certificate: expected one entry, got {}",
            directory.len()
        );

        directory.forget(&certificate(1));
        assert!(
            directory.is_empty() && directory.material(&certificate(1)).is_none(),
            "case a forgotten certificate: expected no material left, got {} entries",
            directory.len()
        );
        // Forgetting what is not held changes nothing.
        directory.forget(&certificate(1));
        directory.forget("");
        assert!(
            directory.is_empty(),
            "case forgetting an absent certificate: expected the directory still empty"
        );
    }

    #[test]
    fn a_blank_arn_records_nothing() {
        let mut directory = CertificateKeyMaterialDirectory::default();
        for case in ["", "   "] {
            directory.record(case, material(0, &["a.example.com"]));
            assert!(
                directory.is_empty(),
                "case {case:?}: expected no material on record, got {} entries",
                directory.len()
            );
        }
    }

    #[test]
    fn the_ceiling_drops_the_oldest_certificates_material_first() {
        let mut directory = CertificateKeyMaterialDirectory::default();
        for index in 0..MAX_CERTIFICATES {
            directory.record(&certificate(index), material(index, &["a.example.com"]));
        }
        assert_eq!(
            directory.len(),
            MAX_CERTIFICATES,
            "case a directory filled to the ceiling: expected {MAX_CERTIFICATES} entries, got {}",
            directory.len()
        );

        directory.record(
            &certificate(MAX_CERTIFICATES),
            material(MAX_CERTIFICATES, &["a.example.com"]),
        );
        assert_eq!(
            directory.len(),
            MAX_CERTIFICATES,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert!(
            directory.material(&certificate(0)).is_none(),
            "case one past the ceiling: expected the oldest material dropped"
        );
        assert!(
            directory.material(&certificate(1)).is_some()
                && directory.material(&certificate(MAX_CERTIFICATES)).is_some(),
            "case one past the ceiling: expected the newest material kept"
        );
    }

    #[test]
    fn a_certificate_covers_the_names_it_carries() {
        // The rows span what the match generalises over: an exact name, its case, a
        // trailing dot on either side, a wildcard's single label, a wildcard against the
        // bare domain and against two labels, and a name the certificate does not carry.
        let held = material(
            0,
            &["www.example.com", "*.apps.example.com", "other.example.net"],
        );
        let cases: &[(&str, bool)] = &[
            ("www.example.com", true),
            ("WWW.Example.COM", true),
            ("www.example.com.", true),
            ("api.apps.example.com", true),
            ("apps.example.com", false),
            ("a.b.apps.example.com", false),
            ("other.example.net", true),
            ("example.com", false),
            ("", false),
        ];
        for (asked_for, expected) in cases {
            let covered = held.covers(asked_for);
            assert_eq!(
                covered, *expected,
                "case {asked_for:?}: expected covers {expected}, got {covered}"
            );
        }
    }

    #[test]
    fn the_key_is_redacted_wherever_a_record_is_formatted() {
        let held = material(0, &["a.example.com"]);
        let formatted = format!("{held:?}");
        assert!(
            !formatted.contains("BEGIN PRIVATE KEY") && formatted.contains("[redacted]"),
            "case a formatted record: expected the key redacted, got {formatted}"
        );
    }
}
