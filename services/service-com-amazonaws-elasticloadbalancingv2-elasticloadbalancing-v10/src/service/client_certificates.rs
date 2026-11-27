// SPDX-License-Identifier: CC0-1.0

//! The CLIENT CERTIFICATE an HTTPS listener asks its clients for, and what the TARGET it
//! forwards to is told about the one a client presented.
//!
//! `MutualAuthenticationAttributes$Mode` is "the client certificate handling method", and
//! `passthrough` is the value where the load balancer takes whatever certificate the client
//! presented and HANDS IT TO THE TARGET — the target is then the only thing that can
//! authenticate the caller from it, so a request that reaches it carrying none of the
//! certificate is a request it cannot authenticate at all. So a listener at that mode ASKS
//! for a certificate during the handshake ([`PresentedWithoutValidation`]) and states what
//! the client presented on the request it forwards, under the names Elastic Load Balancing
//! publishes for them: the whole chain, the leaf alone, and the four facts of the leaf a
//! target reads without parsing it ([`FAMILY`]).
//!
//! Each of those names is one an Application Load Balancer listener's own attribute
//! collection may RENAME (`routing.http.request.x_amzn_mtls_clientcert*.header_name`,
//! `crate::state::LISTENER_ATTRIBUTES`), so the name a member travels under is the listener's
//! own where a caller has written one.
//!
//! Whatever a CLIENT sent under one of those names is REMOVED first — on every listener,
//! whatever its mode, and before anything a member of the family is added under: the family
//! states what the load balancer OBSERVED of the connection, so a value a client wrote must
//! never reach a target as one. It is the discipline the facts of the TLS connection already
//! travel under ([`super::data_plane::TLS_HEADERS`]). A listener that asks for no client
//! certificate, and a connection that presented none, therefore hand the target no header of
//! this family at all rather than one a client forged.
//!
//! What a `verify` listener does — refuse the handshake of a client whose chain the trust
//! store its `TrustStoreArn` names does not vouch for — is not here: this module is the
//! PASSTHROUGH hand-off, which checks nothing about the chain it passes on, exactly as that
//! mode's own name says. Nothing here writes: observing a connection moves no status.
//!
//! # Bounds
//!
//! What a client presents is caller-sized, in the number of certificates and in their length,
//! so both are bounded where the chain is first read ([`presented_chain`]:
//! [`MAX_PRESENTED_CERTIFICATES`] and [`MAX_PRESENTED_BYTES`]) — a chain past either is carried
//! not at all rather than held. Every later read of it is bounded by the same byte ceiling,
//! checked before anything is decoded ([`Presented::of`]), so the work one forwarded request
//! costs, and what the family adds to it, are bounded by those two constants whatever a client
//! presented.

use ::std::sync::Arc;
use ::std::time::{Duration, SystemTime};

use ::percent_encoding::{percent_decode_str, utf8_percent_encode};
use ::rustls::pki_types::CertificateDer;
// The `rustls` module whose NAME warns that implementing its trait replaces a verification the
// library would otherwise do, imported under a name that says which side of the handshake it
// belongs to — the same way the shared TLS termination seam names it
// ([`::service_common::acm_certificate_termination`]). What this module implements is the
// SERVER side asking a client for a certificate: it verifies the handshake's own signature with
// the process's provider, and leaves the chain to the TARGET to judge, which is what the
// `passthrough` mode means.
use ::rustls::server::{danger as presented_certificates};
// The signature VALIDITY the two checks below answer with, which the library declares beside
// the client-side verifier; imported for the same reason as the module above.
use ::rustls::client::{danger as handshake_signatures};
use ::x509_cert::der::{Decode as _, EncodePem as _};
use ::x509_cert::Certificate;

use crate::state::ListenerRecord;

/// The `MutualAuthenticationAttributes$Mode` whose listener hands the presented chain to the
/// target: "[Application Load Balancers] The possible values are `off`, `passthrough`, and
/// `verify`."
pub(super) const PASSTHROUGH_MODE: &str = "passthrough";

/// Which fact of the presented chain one member of the family carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fact {
    /// Every certificate the client presented, in the order it presented them.
    Chain,
    /// The first of them, which is the client's own certificate.
    Leaf,
    /// Who that certificate was issued TO.
    Subject,
    /// Who issued it.
    Issuer,
    /// The two instants it is valid between.
    Validity,
    /// The serial number its issuer gave it.
    SerialNumber,
}

/// One member of the family: the name Elastic Load Balancing publishes it under, the listener
/// attribute that renames it, and which fact of the presented chain it carries.
#[derive(Debug)]
struct Member {
    name: &'static str,
    header_name_attribute: &'static str,
    carries: Fact,
}

/// The whole family, in the order a target receives it.
///
/// The names are the ones Elastic Load Balancing documents an Application Load Balancer
/// sending a client certificate to its target in, and each attribute is the one of
/// `crate::state::LISTENER_ATTRIBUTES` that renames that very header.
const FAMILY: &[Member] = &[
    Member {
        name: "x-amzn-mtls-clientcert",
        header_name_attribute: "routing.http.request.x_amzn_mtls_clientcert.header_name",
        carries: Fact::Chain,
    },
    Member {
        name: "x-amzn-mtls-clientcert-leaf",
        header_name_attribute: "routing.http.request.x_amzn_mtls_clientcert_leaf.header_name",
        carries: Fact::Leaf,
    },
    Member {
        name: "x-amzn-mtls-clientcert-subject",
        header_name_attribute: "routing.http.request.x_amzn_mtls_clientcert_subject.header_name",
        carries: Fact::Subject,
    },
    Member {
        name: "x-amzn-mtls-clientcert-issuer",
        header_name_attribute: "routing.http.request.x_amzn_mtls_clientcert_issuer.header_name",
        carries: Fact::Issuer,
    },
    Member {
        name: "x-amzn-mtls-clientcert-validity",
        header_name_attribute: "routing.http.request.x_amzn_mtls_clientcert_validity.header_name",
        carries: Fact::Validity,
    },
    Member {
        name: "x-amzn-mtls-clientcert-serial-number",
        header_name_attribute:
            "routing.http.request.x_amzn_mtls_clientcert_serial_number.header_name",
        carries: Fact::SerialNumber,
    },
];

/// What a URL-ENCODED value may carry unescaped: the unreserved characters of RFC 3986, so a
/// certificate's PEM — whose own `+`, `/`, `=` and line endings are none of them — travels as
/// one header value.
const URL_ENCODED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// How one member of the family writes an instant: the calendar date and the time of day in
/// UTC, to the second, as a certificate's own validity is stated.
const INSTANT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// How many certificates of a presented chain are carried at all.
///
/// The emulator's own bound, not an AWS one: what a client presents is caller-sized, so what
/// one connection can make this process hold is bounded here. Far above the leaf and the one or
/// two intermediates a real chain carries.
const MAX_PRESENTED_CERTIFICATES: usize = 8;

/// How many bytes the ENCODED chain may run to — the value held for the connection, read back
/// per forwarded request and stated on the request the target receives.
///
/// The emulator's own bound as well, and the one that caps every piece of work this module
/// does: the encode stops here, so nothing longer is ever held, nothing longer is ever decoded
/// again, and what the family adds to a forwarded request is bounded by it. A chain of
/// [`MAX_PRESENTED_CERTIFICATES`] ordinary certificates encodes well inside it.
const MAX_PRESENTED_BYTES: usize = 64 * 1024;

/// What ONE listener does with a client certificate: whether it hands what the client
/// presented to its target, and the name each member of the family travels under on it.
///
/// Read off the listener the request reached, under the store's own guard, so what a target is
/// told is exactly what a create or a modify persisted.
#[derive(Debug, Clone)]
pub(super) struct ClientCertificates {
    /// Whether this listener's mode is the one that hands the chain on
    /// ([`PASSTHROUGH_MODE`]).
    passes_through: bool,
    /// The name each member of [`FAMILY`] travels under on this listener, in that order: the
    /// member's own name, or the one this listener's attribute renames it to.
    names: Vec<String>,
}

impl ClientCertificates {
    /// Whether `name` is a name a member of the family travels under on this listener —
    /// which is what a request is cleared of before anything is added to it.
    ///
    /// Both the published name and the one an attribute renames it to: a target that reads
    /// the family under the name its listener was configured with must not be handed a
    /// client's value under it, and one still reading the published name must not either.
    fn carries(&self, name: &str) -> bool {
        FAMILY.iter().any(|member| member.name == name)
            || self.names.iter().any(|renamed| renamed == name)
    }
}

/// What the listener `listener` does with a client certificate: its own
/// `MutualAuthentication$Mode` and its own attribute collection, and nothing of the request.
pub(super) fn handling(listener: &ListenerRecord) -> ClientCertificates {
    let mode = listener
        .effective_mutual_authentication()
        .and_then(|attributes| attributes.mode);
    read(mode.as_deref(), |attribute| {
        listener.attributes.get(attribute).map(String::as_str)
    })
}

/// [`handling`] over the two facts it reads: the listener's mutual-authentication `mode`, and
/// what its attribute collection `renamed` each member of the family to.
fn read<'a>(
    mode: Option<&str>,
    renamed: impl Fn(&str) -> Option<&'a str>,
) -> ClientCertificates {
    let names = FAMILY
        .iter()
        .map(|member| {
            renamed(member.header_name_attribute)
                .map(|renamed| renamed.trim().to_ascii_lowercase())
                .filter(|renamed| !renamed.is_empty())
                .unwrap_or_else(|| member.name.to_string())
        })
        .collect();
    ClientCertificates {
        passes_through: mode == Some(PASSTHROUGH_MODE),
        names,
    }
}

/// Clear `headers` of every name a member of the family travels under, and — for a listener
/// that hands a client certificate on, over a connection that presented one — state the
/// family on the request the load balancer forwards.
///
/// `presented` is what the handshake observed ([`presented_chain`]). Called where the request
/// LEAVES the load balancer for a target, as the `X-Forwarded-For` processing is
/// ([`super::forwarded_for`]), so the rules the listener evaluated were matched against the
/// request as the client sent it.
pub(super) fn handed_to_target(
    headers: &mut Vec<(String, String)>,
    handling: &ClientCertificates,
    presented: Option<&str>,
) {
    headers.retain(|(name, _)| !handling.carries(name));
    let Some(presented) = presented.filter(|_| handling.passes_through) else {
        return;
    };
    let Some(chain) = Presented::of(presented) else {
        ::tracing::warn!(
            "the chain the client presented holds no certificate this emulator can read, so \
             the target was told nothing of it"
        );
        return;
    };
    for (member, name) in FAMILY.iter().zip(handling.names.iter()) {
        if let Some(value) = chain.fact(member.carries) {
            headers.push((name.clone(), value));
        }
    }
}

/// The chain one handshake was presented, as the family states it: the whole chain
/// URL-encoded, and the leaf it opens with.
#[derive(Debug)]
struct Presented {
    /// Every certificate presented, URL-encoded exactly as the member that carries the whole
    /// chain states it.
    chain: String,
    /// The leaf alone, URL-encoded the same way.
    leaf: String,
    /// The leaf as a certificate, which the four facts below are read off.
    certificate: Certificate,
}

impl Presented {
    /// Read what the handshake observed ([`presented_chain`]) back into the facts the family
    /// carries, or `None` when it holds no certificate that decodes as one — or runs past
    /// [`MAX_PRESENTED_BYTES`], which is read BEFORE anything is decoded, so the work one
    /// request costs is bounded by that constant and not by what a client presented.
    fn of(presented: &str) -> Option<Self> {
        if presented.len() > MAX_PRESENTED_BYTES {
            ::tracing::warn!(
                bytes = presented.len(),
                "the chain the client presented runs past the bound this emulator carries one \
                 for, so the target was told nothing of it"
            );
            return None;
        }
        let pem = percent_decode_str(presented).decode_utf8().ok()?;
        let mut reader = ::std::io::BufReader::new(pem.as_bytes());
        let leaf_der = ::rustls_pemfile::certs(&mut reader)
            .find_map(::std::result::Result::ok)
            .map(|der| der.as_ref().to_vec())?;
        let certificate = Certificate::from_der(&leaf_der).ok()?;
        Some(Self {
            chain: presented.to_string(),
            leaf: url_encoded(&certificate)?,
            certificate,
        })
    }

    /// The value the member carrying `fact` states, and `None` for a fact the leaf does not
    /// carry a readable value for — nothing is invented for one.
    fn fact(&self, fact: Fact) -> Option<String> {
        let leaf = &self.certificate.tbs_certificate;
        match fact {
            Fact::Chain => Some(self.chain.clone()),
            Fact::Leaf => Some(self.leaf.clone()),
            Fact::Subject => Some(leaf.subject.to_string()),
            Fact::Issuer => Some(leaf.issuer.to_string()),
            Fact::Validity => Some(format!(
                "NotBefore={};NotAfter={}",
                instant(leaf.validity.not_before.to_unix_duration()),
                instant(leaf.validity.not_after.to_unix_duration())
            )),
            Fact::SerialNumber => {
                Some(::hex::encode_upper(leaf.serial_number.as_bytes())).filter(|serial| {
                    // A serial number is an integer every certificate carries; one whose
                    // bytes are missing is a fact this leaf does not state.
                    !serial.is_empty()
                })
            }
        }
    }
}

/// What the HANDSHAKE observed of the certificate the client presented: every certificate of
/// the chain, as PEM, URL-encoded into one header value — the form the member carrying the
/// whole chain states it in.
///
/// Each certificate is DECODED as an X.509 certificate and re-encoded from what decoded, so
/// what travels is a chain of certificates rather than whatever bytes a client offered;
/// `None` for a handshake that presented nothing readable, which is a connection the target is
/// told nothing about.
///
/// What a client presents is caller-sized, so it is BOUNDED here, which is the only place it
/// can be: at most [`MAX_PRESENTED_CERTIFICATES`] certificates are encoded and the result is
/// kept only while it is within [`MAX_PRESENTED_BYTES`] — so the value one connection holds,
/// and every later read of it, are bounded by those two constants.
pub(super) fn presented_chain(presented: &[CertificateDer<'_>]) -> Option<String> {
    let pem: String = presented
        .iter()
        .take(MAX_PRESENTED_CERTIFICATES)
        .filter_map(|der| Certificate::from_der(der.as_ref()).ok())
        .filter_map(|certificate| certificate.to_pem(::x509_cert::der::pem::LineEnding::LF).ok())
        .collect();
    if pem.is_empty() {
        return None;
    }
    let encoded = utf8_percent_encode(&pem, URL_ENCODED).to_string();
    if encoded.len() > MAX_PRESENTED_BYTES {
        ::tracing::warn!(
            bytes = encoded.len(),
            "the chain the client presented runs past the bound this emulator carries one for, \
             so nothing of it is held for the connection"
        );
        return None;
    }
    Some(encoded)
}

/// One certificate as the family carries it: its own PEM, URL-encoded.
fn url_encoded(certificate: &Certificate) -> Option<String> {
    let pem = certificate
        .to_pem(::x509_cert::der::pem::LineEnding::LF)
        .ok()?;
    Some(utf8_percent_encode(&pem, URL_ENCODED).to_string())
}

/// One instant of a certificate's validity as the family writes it ([`INSTANT`]).
fn instant(since_epoch: Duration) -> String {
    let at = SystemTime::UNIX_EPOCH
        .checked_add(since_epoch)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .format(INSTANT)
        .to_string()
}

/// The client-certificate verifier of a listener at [`PASSTHROUGH_MODE`]: it ASKS every
/// client for a certificate and accepts the connection whatever it answers with, because
/// passthrough is the mode in which the load balancer "passes" the certificate on instead of
/// deciding anything about it — the target is what authenticates the caller, and a client that
/// presents nothing is served as any other client of an HTTPS listener is.
///
/// A chain is vouched for by the trust store a listener's `TrustStoreArn` names only in the
/// OTHER mode (`verify`), which no listener served here is configured into: this verifier is
/// reached for `passthrough` alone, and a load balancer that validated there would refuse
/// exactly the connections the mode exists to carry.
#[derive(Debug)]
pub(super) struct PresentedWithoutValidation {
    /// The provider whose algorithms the handshake's own signature is checked with — the
    /// one thing about the connection that IS verified here, because it is what makes the
    /// presented certificate the one the peer holds the key for.
    provider: Arc<::rustls::crypto::CryptoProvider>,
}

impl PresentedWithoutValidation {
    /// The verifier a listener at [`PASSTHROUGH_MODE`] asks its clients for a certificate
    /// with, over the provider every handshake this service terminates runs with.
    pub(super) fn asking() -> Arc<dyn presented_certificates::ClientCertVerifier> {
        Arc::new(Self {
            provider: ::service_common::acm_certificate_termination::provider(),
        })
    }
}

impl presented_certificates::ClientCertVerifier for PresentedWithoutValidation {
    /// The listener ASKS for a certificate: that request is what gives a client the chance to
    /// present one at all.
    fn offer_client_auth(&self) -> bool {
        true
    }

    /// And serves the client that presents none: an HTTPS listener at this mode answers an
    /// ordinary client's request as well, with no member of the family on it.
    fn client_auth_mandatory(&self) -> bool {
        false
    }

    /// No issuer is named to the client: this listener accepts a certificate from any issuer,
    /// so there is no list for a client to choose from.
    fn root_hint_subjects(&self) -> &[::rustls::DistinguishedName] {
        &[]
    }

    /// Whatever the client presented is passed on (see this type's own documentation).
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: ::rustls::pki_types::UnixTime,
    ) -> Result<presented_certificates::ClientCertVerified, ::rustls::Error> {
        Ok(presented_certificates::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &::rustls::DigitallySignedStruct,
    ) -> Result<handshake_signatures::HandshakeSignatureValid, ::rustls::Error> {
        ::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &::rustls::DigitallySignedStruct,
    ) -> Result<handshake_signatures::HandshakeSignatureValid, ::rustls::Error> {
        ::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<::rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    //! The hand-off itself. That a listener really ASKS a client for a certificate, and that
    //! the family reaches the target over a real handshake, is pinned end to end in
    //! `tests/all/alb_mutual_authentication_passthrough.rs`.

    use super::*;

    /// The name the certificate a case mints is issued to, which is what the subject and the
    /// issuer of a self-signed leaf both state.
    const CLIENT_NAME: &str = "mtls-client.example.com";

    /// The name a case renames the member carrying the whole chain to, as a caller writes one
    /// into `routing.http.request.x_amzn_mtls_clientcert.header_name`.
    const RENAMED: &str = "x-client-chain";

    /// A real client certificate, as the handshake hands it on: URL-encoded PEM
    /// ([`presented_chain`]).
    fn presented() -> String {
        presented_chain(&[::rustls::pki_types::CertificateDer::from(minted_der())])
            .expect("a minted certificate is one this reads")
    }

    /// One real certificate's DER, minted for a case.
    fn minted_der() -> Vec<u8> {
        let key = ::rcgen::KeyPair::generate().expect("a key pair");
        let mut params =
            ::rcgen::CertificateParams::new(vec![CLIENT_NAME.to_string()]).expect("parameters");
        let mut subject = ::rcgen::DistinguishedName::new();
        subject.push(::rcgen::DnType::CommonName, CLIENT_NAME);
        params.distinguished_name = subject;
        let certificate = params.self_signed(&key).expect("a self-signed certificate");
        certificate.der().to_vec()
    }

    /// The value one header of `headers` carries, and `None` for a name nothing carries.
    fn carried(headers: &[(String, String)], name: &str) -> Option<String> {
        let values: Vec<&String> = headers
            .iter()
            .filter(|(held, _)| held == name)
            .map(|(_, value)| value)
            .collect();
        assert!(
            values.len() <= 1,
            "case {name:?}: a target is handed one value, got {values:?}"
        );
        values.first().map(|value| (*value).to_string())
    }

    /// One row: the case name, the listener's own mutual-authentication mode, what the
    /// handshake presented, and the name the listener renames the member carrying the chain
    /// to.
    struct Row<'a> {
        case: &'a str,
        mode: Option<&'a str>,
        presented: Option<&'a str>,
        rename: Option<&'a str>,
    }

    /// RULE: a listener that hands a client certificate to its target states the chain the
    /// client presented and the facts of its leaf on the request it forwards, under the names
    /// its own attribute collection gives them; every other listener, and every connection
    /// that presented nothing, hands the target no header of that family — and a value the
    /// CLIENT sent under one of those names never reaches the target either way.
    #[test]
    fn the_family_states_what_the_handshake_observed_and_nothing_a_client_wrote() {
        let chain = presented();
        // The rows span what the rule generalises over: the listener's own mode, whether the
        // connection presented a certificate at all, and whether the listener renames the
        // member that carries the chain.
        let rows: &[Row<'_>] = &[
            Row {
                case: "a passthrough listener over a connection that presented a certificate",
                mode: Some(PASSTHROUGH_MODE),
                presented: Some(chain.as_str()),
                rename: None,
            },
            Row {
                case: "a passthrough listener whose client presented nothing",
                mode: Some(PASSTHROUGH_MODE),
                presented: None,
                rename: None,
            },
            Row {
                case: "a listener whose mutual authentication is off",
                mode: Some("off"),
                presented: Some(chain.as_str()),
                rename: None,
            },
            Row {
                case: "a listener that handles no client certificate at all",
                mode: None,
                presented: Some(chain.as_str()),
                rename: None,
            },
            Row {
                case: "a passthrough listener that renames the member carrying the chain",
                mode: Some(PASSTHROUGH_MODE),
                presented: Some(chain.as_str()),
                rename: Some(RENAMED),
            },
        ];
        for Row {
            case,
            mode,
            presented,
            rename,
        } in rows
        {
            let handling = read(*mode, |attribute| {
                rename.filter(|_| attribute == FAMILY[0].header_name_attribute)
            });
            // Every name of the family, forged by the client, beside a header of its own that
            // has to survive.
            let mut headers: Vec<(String, String)> = FAMILY
                .iter()
                .map(|member| (member.name.to_string(), "forged-by-the-client".to_string()))
                .chain([(RENAMED.to_string(), "forged-by-the-client".to_string())])
                .chain([("accept".to_string(), "*/*".to_string())])
                .collect();
            handed_to_target(&mut headers, &handling, *presented);

            assert_eq!(
                carried(&headers, "accept").as_deref(),
                Some("*/*"),
                "case {case:?}: the client's own headers travel, got {headers:?}"
            );
            let handed = mode == &Some(PASSTHROUGH_MODE) && presented.is_some();
            let chain_name = rename.unwrap_or(FAMILY[0].name);
            assert_eq!(
                carried(&headers, chain_name),
                handed.then(|| chain.clone()),
                "case {case:?}: expected the chain under {chain_name:?}, got {headers:?}"
            );
            if rename.is_some() {
                assert_eq!(
                    carried(&headers, FAMILY[0].name),
                    None,
                    "case {case:?}: a renamed member travels under its own name alone, got \
                     {headers:?}"
                );
            }
            // The facts of the leaf: stated for the hand-off, and absent for every row that
            // is not one.
            let expected: &[(Fact, Option<String>)] = &[
                (Fact::Leaf, handed.then(|| chain.clone())),
                (
                    Fact::Subject,
                    handed.then(|| format!("CN={CLIENT_NAME}")),
                ),
                (Fact::Issuer, handed.then(|| format!("CN={CLIENT_NAME}"))),
            ];
            for (fact, expected) in expected {
                let name = FAMILY
                    .iter()
                    .find(|member| member.carries == *fact)
                    .expect("every fact has a member")
                    .name;
                assert_eq!(
                    carried(&headers, name),
                    *expected,
                    "case {case:?}: expected {name} to carry {expected:?}, got {headers:?}"
                );
            }
            let validity = carried(&headers, "x-amzn-mtls-clientcert-validity");
            assert_eq!(
                validity.is_some(),
                handed,
                "case {case:?}: expected the validity to be stated {handed}, got {validity:?}"
            );
            if let Some(validity) = validity {
                assert!(
                    validity.starts_with("NotBefore=") && validity.contains(";NotAfter="),
                    "case {case:?}: expected the two instants of the leaf, got {validity:?}"
                );
            }
            let serial = carried(&headers, "x-amzn-mtls-clientcert-serial-number");
            assert_eq!(
                serial.is_some(),
                handed,
                "case {case:?}: expected the serial number to be stated {handed}, got {serial:?}"
            );
            if let Some(serial) = serial {
                assert!(
                    !serial.is_empty()
                        && serial
                            .chars()
                            .all(|digit| digit.is_ascii_hexdigit() && !digit.is_lowercase()),
                    "case {case:?}: expected the leaf's own serial number, got {serial:?}"
                );
            }
        }
    }

    /// RULE: nothing is handed to a target for bytes that are no certificate — neither out of
    /// a handshake nor out of a leg — and nothing past the bounds this emulator carries a
    /// presented chain within.
    #[test]
    fn bytes_that_are_no_certificate_state_nothing() {
        let der = ::rustls::pki_types::CertificateDer::from(b"not a certificate".to_vec());
        assert_eq!(
            presented_chain(&[der]),
            None,
            "a handshake that presented no readable certificate observes nothing"
        );
        assert_eq!(
            presented_chain(&[]),
            None,
            "a handshake that presented nothing observes nothing"
        );
        // A chain longer than the bound: only the certificates within it are carried.
        let minted: Vec<::rustls::pki_types::CertificateDer<'static>> =
            (0..MAX_PRESENTED_CERTIFICATES + 3)
                .map(|_| ::rustls::pki_types::CertificateDer::from(minted_der()))
                .collect();
        let carried = presented_chain(&minted).expect("a minted chain is one this reads");
        let within = carried.matches("BEGIN%20CERTIFICATE").count();
        assert_eq!(
            within, MAX_PRESENTED_CERTIFICATES,
            "case \"a chain past the bound\": expected {MAX_PRESENTED_CERTIFICATES} \
             certificates carried, got {within}"
        );
        // A value past the byte bound states nothing at all, and is not decoded.
        let past_the_bound = "-".repeat(MAX_PRESENTED_BYTES + 1);
        for case in [
            "",
            "not-a-pem",
            "-----BEGIN%20CERTIFICATE-----%0Azz%0A",
            past_the_bound.as_str(),
        ] {
            let mut headers: Vec<(String, String)> = Vec::new();
            handed_to_target(
                &mut headers,
                &read(Some(PASSTHROUGH_MODE), |_| None),
                Some(case),
            );
            assert!(
                headers.is_empty(),
                "case {:?}: expected no header of the family, got {headers:?}",
                &case[..case.len().min(32)]
            );
        }
    }
}
