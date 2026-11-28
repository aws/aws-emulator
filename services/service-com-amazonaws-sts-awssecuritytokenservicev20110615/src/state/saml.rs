// SPDX-License-Identifier: CC0-1.0

//! Which SAML 2.0 identity a presented authentication response stands for, and the class a
//! response that stands for none is refused as.
//!
//! `AssumeRoleWithSAML` federates the identity an assertion names, so the first question it
//! has to answer is whether the assertion really is one the identity provider the account
//! registered issued. That answer is a VERIFICATION, in the order a SAML service provider
//! decides it (OASIS "Assertions and Protocols for the OASIS Security Assertion Markup
//! Language (SAML) V2.0", §3.2.2 and §2.5, and W3C "XML Signature Syntax and Processing"
//! §3.2):
//!
//! * the value is the base64 of an XML document, and that document is a SAML 2.0 `Response`;
//! * the document carries an XML signature whose transforms and algorithms a verifier
//!   accepts — nothing that selects the signed content by an expression of its own, which is
//!   how a signature is made to cover less than it appears to;
//! * that signature checks out against a key the REGISTERED provider publishes in its
//!   metadata, never against a key the document itself brings along: a response signed by
//!   anything else is a response the provider did not issue;
//! * the response names that provider as its `Issuer`;
//! * the provider reported success rather than an authentication failure;
//! * the response names the identity it speaks for; and
//! * the clock is inside the validity window the response states.
//!
//! Each class is refused with the fixed text AWS words for THAT class
//! ([`AssertionDefect::sentence`]), so a caller can tell a forged signature from an expired
//! window from a claim the provider itself rejected.
//!
//! ## Who refused
//!
//! This reading is not STS's own: a presented response is read by the internal discovery
//! service that holds the account's SAML manifests, and what a client is served is THAT
//! call's refusal, reported inside the envelope the relaying layer states around a failed
//! call — the service it was made to, the status it answered, the exception it named, that
//! call's own request id, and the proxy it went through ([`AssertionDefect::message`]). The
//! exception names which part of the relayed reading refused: looking the provider's
//! manifest up, or reading the presented response against the manifest it found. A class
//! decided here rather than there — the role the REQUEST asked for not being one the
//! response asserts, a validity window this service answers its own expiry error for, and
//! the provider's own report that the authentication failed — carries its sentence alone,
//! because no relayed call refused it.
//!
//! ## What the digest is computed over
//!
//! The reference's transforms are the enveloped-signature transform followed by a
//! canonicalization, and canonicalizing a document that is ALREADY in canonical form is the
//! identity — so the digest is computed over the signed element's own bytes with the
//! `Signature` subtree removed, which is exactly the transform output for such a document.
//! A document whose serialization is not canonical does not verify here, which is the
//! fail-closed answer: the response is refused as one whose signature did not check out
//! rather than federated on an unchecked signature.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The namespace of the SAML 2.0 protocol vocabulary, which the `Response` element is in.
const PROTOCOL_NAMESPACE: &str = "urn:oasis:names:tc:SAML:2.0:protocol";

/// The namespace of the SAML 2.0 assertion vocabulary: the `Assertion` and everything
/// inside it.
const ASSERTION_NAMESPACE: &str = "urn:oasis:names:tc:SAML:2.0:assertion";

/// The namespace of the XML signature vocabulary, the ONE reading of it this tree has.
const SIGNATURE_NAMESPACE: &str =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::XML_SIGNATURE_NAMESPACE;

/// The status a provider reports when the authentication it was asked for SUCCEEDED
/// ("Assertions and Protocols", §3.2.2.2). Any other top-level status is the provider
/// reporting that it did not.
const STATUS_SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";

/// The prefix the SAML 2.0 name-identifier formats share: "if the format includes the prefix
/// urn:oasis:names:tc:SAML:2.0:nameid-format, that prefix is removed"
/// (`AssumeRoleWithSAMLResponse$SubjectType`).
const NAME_ID_FORMAT_PREFIX: &str = "urn:oasis:names:tc:SAML:2.0:nameid-format";

/// The attribute an assertion names the role sessions it permits in, each value the role ARN
/// and the provider ARN of one permitted pair (AWS "Configuring SAML assertions for the
/// authentication response").
const ROLE_ATTRIBUTE: &str = "https://aws.amazon.com/SAML/Attributes/Role";

/// The attribute an assertion names the session by, which is the `RoleSessionName` of the
/// session this mint vends (same reference).
const ROLE_SESSION_NAME_ATTRIBUTE: &str = "https://aws.amazon.com/SAML/Attributes/RoleSessionName";

/// The attribute an assertion carries a source identity in, reported as `SourceIdentity`
/// ("The value in the SourceIdentity attribute in the SAML assertion",
/// `AssumeRoleWithSAMLResponse$SourceIdentity`).
const SOURCE_IDENTITY_ATTRIBUTE: &str = "https://aws.amazon.com/SAML/Attributes/SourceIdentity";

/// How long after it was issued a response may still be redeemed: AWS gives a SAML
/// authentication response five minutes from its `IssueInstant` ("Troubleshooting SAML 2.0
/// federation with AWS", which states the five-minute limit between the instant the response
/// was issued and the instant it is presented).
const REDEMPTION_WINDOW: Duration = Duration::from_secs(5 * 60);

/// The most signatures one authentication response may carry: SAML 2.0 places a signature in
/// two spots — over the `Response` and over the `Assertion` ("Assertions and Protocols",
/// §5) — and AWS accepts either or both, so two is every signature a response this service
/// federates can carry.
///
/// Checking one costs a digest over the element it covers, so the count is what multiplies
/// that cost: a presented document carrying more signatures than those two spots is refused
/// without any of them being checked ([`verify_signatures`]), which bounds the work one
/// request can ask for by the number of signatures AWS itself would read.
const MAX_SIGNATURES: usize = 2;

/// The transforms a reference may name: the enveloped-signature transform (so the signature
/// does not cover itself) and the canonicalizations, which are the identity on a document
/// already in canonical form. Anything else — an expression selecting the signed content, a
/// stylesheet, a decoding — makes the signature cover something other than the element it
/// appears to, so it is refused rather than evaluated.
const ACCEPTED_TRANSFORMS: &[&str] = &[
    "http://www.w3.org/2000/09/xmldsig#enveloped-signature",
    "http://www.w3.org/2001/10/xml-exc-c14n#",
    "http://www.w3.org/2001/10/xml-exc-c14n#WithComments",
    "http://www.w3.org/TR/2001/REC-xml-c14n-20010315",
    "http://www.w3.org/TR/2001/REC-xml-c14n-20010315#WithComments",
];

/// The canonicalizations a `SignedInfo` may be canonicalized by, the same set without the
/// enveloped-signature transform.
const ACCEPTED_CANONICALIZATIONS: &[&str] = &[
    "http://www.w3.org/2001/10/xml-exc-c14n#",
    "http://www.w3.org/2001/10/xml-exc-c14n#WithComments",
    "http://www.w3.org/TR/2001/REC-xml-c14n-20010315",
    "http://www.w3.org/TR/2001/REC-xml-c14n-20010315#WithComments",
];

/// How AWS words the refusal of a signature that did not check out against the keys the
/// provider publishes ("Troubleshooting SAML 2.0 federation with AWS"). Fixed text: a
/// verifier that rejected a signature says no more than that it did, and — unlike the other
/// readings of a presented response — this class's message carries no relayed envelope either
/// (see [`AssertionDefect::relayed_exception`]), so it is equal for every presentation.
const SIGNATURE_INVALID_MESSAGE: &str = "Response signature invalid";

/// How AWS words the refusal of a response whose `Issuer` is not the one the named provider
/// is registered under (same reference).
const ISSUER_NOT_IN_PROVIDER_MESSAGE: &str = "Issuer not present in specified provider";

/// How AWS words the refusal of a request naming a provider the account does not hold (same
/// reference).
const PROVIDER_NOT_FOUND_MESSAGE: &str = "Specified provider doesn't exist";

/// How AWS words the refusal of a value that is not the base64 a `SAMLAssertion` is.
const NOT_BASE64_MESSAGE: &str = "Invalid base64 SAMLResponse";

/// How AWS words the refusal of a response that asserts no role pair the request asked for:
/// the provider claims nothing about the session the request wants, so the attributes carry
/// neither the role asked for nor the provider's metadata beside it.
const ROLE_NOT_ASSERTED_MESSAGE: &str =
    "SAML Assertion doesn't contain the requested Role and Metadata in the attributes";

/// How AWS words the refusal of a response nothing signed at all: a response's own signature
/// is optional, and the assertion inside it carries a mandatory one — a different condition
/// from a signature that was checked and did not hold.
const NOT_SIGNED_MESSAGE: &str =
    "Response (with optional signature) must contain an assertion with a mandatory signature";

/// How AWS words the refusal of a response presented longer after it was issued than the
/// window it may be redeemed in: the five minutes AWS gives a SAML authentication response
/// between its `IssueInstant` and its redemption.
const NOT_REDEEMABLE_MESSAGE: &str = "Token must be redeemed within 5 minutes of issuance";

/// How AWS words the refusal of a response that names no session at all (same reference):
/// the service provider is AWS, which is what needs the name.
const SESSION_NAME_REQUIRED_MESSAGE: &str =
    "RoleSessionName is required in AuthnResponse (service provider)";

/// How AWS words the refusal of a session name outside the character class a role session
/// name is held to (same reference), which is `roleSessionNameType`'s own pattern and length
/// in this service's model.
const SESSION_NAME_PATTERN_MESSAGE: &str =
    "RoleSessionName in AuthnResponse must match [a-zA-Z_0-9+=,.@-]{2,64}";

/// How AWS words the refusal of a response whose validity window has passed, whether that
/// window is stated by the assertion's conditions or by its subject's confirmation data.
const RESPONSE_EXPIRED_MESSAGE: &str = "Response has expired";

/// How AWS words the refusal of a response whose validity window has not begun.
const RESPONSE_NOT_YET_VALID_MESSAGE: &str = "Response is not yet valid";

/// How AWS words the refusal of a claim the identity provider itself rejected: the sentence
/// `IDPRejectedClaimException` is documented with in this service's model.
const IDP_REJECTED_CLAIM_MESSAGE: &str =
    "The identity provider (IdP) reported that authentication failed.";

/// The shortest and longest a role session name may be, and the characters it admits:
/// `roleSessionNameType` (len 2..64, pattern `^[\w+=,.@-]*$`, this service's model), which
/// the name an assertion carries is held to just as the one a request names is.
const SESSION_NAME_MIN_CHARS: usize = 2;
const SESSION_NAME_MAX_CHARS: usize = 64;

/// Why a presented value is not a SAML 2.0 identity this service can federate.
///
/// One variant per class AWS tells apart, in the order a verifier decides them. The caller
/// maps each to the modelled error its class carries and states it with
/// [`AssertionDefect::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionDefect {
    /// The request names a SAML provider the account does not hold, so there is no metadata
    /// to read a key or an issuer out of.
    ProviderNotFound,
    /// Not the base64 of anything.
    NotBase64,
    /// Base64 of something that is not a well-formed XML document.
    NotWellFormed,
    /// A well-formed document that is not a SAML 2.0 authentication response.
    NotSamlResponse,
    /// Nothing signed the response: neither it nor the assertion inside it carries an XML
    /// signature.
    NotSigned,
    /// The signature names a transform or an algorithm a verifier does not accept, so what
    /// the signature covers is not the element it appears to.
    UnacceptedTransform,
    /// The signature was checked against the keys the provider publishes and did not check
    /// out — including a response signed by a key the provider does not publish at all.
    SignatureInvalid,
    /// The response's `Issuer` is not the entity the named provider is registered for.
    IssuerNotInProvider,
    /// The provider reported that the authentication failed: a claim it rejects itself.
    ClaimRejected,
    /// The provider asserted no role pair the request asked for, so it claims nothing about
    /// the session the request wants.
    RoleNotAsserted,
    /// The response names no session.
    SessionNameMissing,
    /// The session name the response names is outside the class a role session name admits.
    SessionNamePattern,
    /// The clock has passed the validity window the response states — its assertion's own
    /// conditions, or the confirmation data of its subject.
    Expired,
    /// The response was presented longer after it was issued than it may be redeemed in.
    NotRedeemable,
    /// The validity window the response states has not begun.
    NotYetValid,
}

/// The internal service a presented SAML authentication response is read by, as the layer
/// that relays the call names it: STS holds no SAML manifest of its own, so the reading — and
/// the refusal of it — belongs to that service.
const DISCOVERY_SERVICE: &str = "AWSOpenIdDiscoveryService";

/// The HTTP status that service answers a response it refused with, which is the status the
/// envelope states for the relayed call (not this operation's own).
const DISCOVERY_REFUSAL_STATUS: u16 = 400;

/// The exception that service names when the presented value is not a response of the
/// provider's it will read: the reading of the response itself refused.
const INVALID_RESPONSE_EXCEPTION: &str = "AuthSamlInvalidSamlResponseException";

/// The exception that service names when the manifest it would read the response against is
/// not there to be found: the manifest LOOKUP refused, before any response was read.
const MANIFEST_NOT_FOUND_EXCEPTION: &str = "AuthSamlManifestNotFoundException";

/// What a proxy field states when the relayed call went through no proxy, which is every call
/// made in this process.
const NO_PROXY: &str = "null";

/// The envelope the relaying layer states around a refused call, including the space that
/// separates it from the sentence it follows: the service the call was made to, the status it
/// answered, the exception it named, that call's OWN request id — minted per call, so two
/// refusals never report one id — and the proxy it went through, in that order.
#[must_use]
fn relayed_envelope(exception: &str) -> String {
    format!(
        " (Service: {DISCOVERY_SERVICE}; Status Code: {DISCOVERY_REFUSAL_STATUS}; \
         Error Code: {exception}; Request ID: {}; Proxy: {NO_PROXY})",
        ::service_common::query_error_envelope::new_request_id()
    )
}

impl AssertionDefect {
    /// The sentence AWS words for this class — fixed text per class, so no reader of the
    /// document puts its own diagnostics on the wire.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::ProviderNotFound => PROVIDER_NOT_FOUND_MESSAGE.to_string(),
            Self::NotBase64 => NOT_BASE64_MESSAGE.to_string(),
            Self::NotWellFormed => {
                "Invalid SAMLResponse: the value is not a well-formed XML document".to_string()
            }
            Self::NotSamlResponse => {
                "Invalid SAMLResponse: the document is not a SAML 2.0 Response".to_string()
            }
            // A response whose signature covers something other than the element it names,
            // and one whose signature does not check out, are one answer: no signature of
            // the provider's covers what this response says.
            Self::UnacceptedTransform | Self::SignatureInvalid => {
                SIGNATURE_INVALID_MESSAGE.to_string()
            }
            Self::NotSigned => NOT_SIGNED_MESSAGE.to_string(),
            Self::IssuerNotInProvider => ISSUER_NOT_IN_PROVIDER_MESSAGE.to_string(),
            Self::ClaimRejected => IDP_REJECTED_CLAIM_MESSAGE.to_string(),
            Self::RoleNotAsserted => ROLE_NOT_ASSERTED_MESSAGE.to_string(),
            Self::SessionNameMissing => SESSION_NAME_REQUIRED_MESSAGE.to_string(),
            Self::SessionNamePattern => SESSION_NAME_PATTERN_MESSAGE.to_string(),
            Self::Expired => RESPONSE_EXPIRED_MESSAGE.to_string(),
            Self::NotRedeemable => NOT_REDEEMABLE_MESSAGE.to_string(),
            Self::NotYetValid => RESPONSE_NOT_YET_VALID_MESSAGE.to_string(),
        }
    }

    /// The exception the relayed reading answered this class with, or `None` for a class whose
    /// refusal carries NO envelope.
    ///
    /// Reading the manifest the account registered and reading the presented response against
    /// it are two steps of that call, and the exception says which of them refused. Three
    /// kinds of class carry none:
    ///
    /// * the classes this service decides ITSELF — the role the request asked for against the
    ///   pairs a VERIFIED response asserts, the validity window it answers its own expiry
    ///   error for, and the provider's own report that the authentication failed — because no
    ///   relayed call refused them;
    /// * the SIGNATURE classes: a response whose signature does not check out, and one whose
    ///   declared transforms a verifier will not accept, are refused with the fixed sentence
    ///   ALONE. Unlike the other readings of the presented document, that refusal carries no
    ///   parenthesized envelope at all, so a client telling this class from the others matches
    ///   a message that is equal every time.
    #[must_use]
    pub fn relayed_exception(&self) -> Option<&'static str> {
        match self {
            // The lookup: there is no manifest to read a key or an issuer out of.
            Self::ProviderNotFound => Some(MANIFEST_NOT_FOUND_EXCEPTION),
            // The reading: what the presented document is held to, from its encoding to the
            // session it names.
            Self::NotBase64
            | Self::NotWellFormed
            | Self::NotSamlResponse
            | Self::NotSigned
            | Self::IssuerNotInProvider
            | Self::SessionNameMissing
            | Self::SessionNamePattern => Some(INVALID_RESPONSE_EXCEPTION),
            // The signature classes, and the classes this service decides itself.
            Self::UnacceptedTransform
            | Self::SignatureInvalid
            | Self::ClaimRejected
            | Self::RoleNotAsserted
            | Self::Expired
            | Self::NotRedeemable
            | Self::NotYetValid => None,
        }
    }

    /// The whole message this class is reported with: its sentence, and — for a class a
    /// relayed call refused — the envelope naming that call ([`relayed_envelope`]).
    #[must_use]
    pub fn message(&self) -> String {
        match self.relayed_exception() {
            Some(exception) => format!("{}{}", self.sentence(), relayed_envelope(exception)),
            None => self.sentence(),
        }
    }
}

/// The SAML provider the account holds under one ARN: the entity it is registered for and
/// the keys its metadata publishes.
#[derive(Debug, Clone)]
pub struct SamlProvider {
    /// The ARN the request named it by, which a role's trust policy names as its
    /// `Federated` principal.
    pub arn: String,
    /// The friendly name — the last part of the ARN — which the `NameQualifier` hash is
    /// built from.
    pub name: String,
    /// "The issuer's name" the metadata document states, which the response's own `Issuer`
    /// has to be.
    pub entity_id: String,
    /// The keys the metadata publishes, as the base64 of a DER X.509 certificate each.
    pub certificates: Vec<String>,
}

/// What a verified response stands for: the facts the exchange reports back, the roles the
/// provider asserted for this identity, and the session bounds it states.
#[derive(Debug, Clone)]
pub struct SamlAssertion {
    /// "The value of the Issuer element of the SAML assertion"
    /// (`AssumeRoleWithSAMLResponse$Issuer`).
    pub issuer: String,
    /// "The value of the NameID element in the Subject element"
    /// (`AssumeRoleWithSAMLResponse$Subject`).
    pub subject: String,
    /// "The format of the name ID", with the SAML 2.0 format prefix removed
    /// (`AssumeRoleWithSAMLResponse$SubjectType`).
    pub subject_type: String,
    /// "The value of the Recipient attribute of the SubjectConfirmationData element"
    /// (`AssumeRoleWithSAMLResponse$Audience`), which is also what the `saml:aud` condition
    /// key answers; the audience restriction the assertion states when it confirms the
    /// subject without naming a recipient.
    pub audience: String,
    /// The name the session is vended under: the `RoleSessionName` attribute, or the name
    /// identifier when the response names no such attribute.
    pub session_name: String,
    /// The (role ARN, provider ARN) pairs the `Role` attribute asserts.
    pub asserted_roles: Vec<(String, String)>,
    /// The `SourceIdentity` attribute, when the response carries one.
    pub source_identity: Option<String>,
    /// Every attribute the assertion carried, as (name, value) pairs — which is what a
    /// trust policy's conditions are decided against beside the `saml:` keys the provider
    /// publishes.
    pub attributes: Vec<(String, String)>,
    /// `SessionNotOnOrAfter` of the authentication statement, as seconds since the Unix
    /// epoch: the instant the session this response authorises may not outlast.
    pub session_not_on_or_after: Option<i64>,
    /// The digest of the KEY that signed the assertion ([`signing_key_digest`]), which is
    /// what the origin identity of a SAML session names its issuer by: "for
    /// `AssumeRoleWithSAML`, this is a hash of the key that signed the SAML assertion, not
    /// the `Issuer` element of the assertion" (`OriginIdentity$Issuer`, this service's
    /// model) — so it is taken from the key the signature really checked out against
    /// ([`verify_signatures`]) rather than from anything the document says about itself.
    pub signing_key_digest: String,
}

impl SamlAssertion {
    /// Whether the provider asserted the (role, provider) pair this request asks for. The
    /// attribute's values state the two ARNs in either order (AWS "Configuring SAML
    /// assertions for the authentication response" gives both), so a pair matches whichever
    /// way round it is written.
    #[must_use]
    pub fn asserts(&self, role_arn: &str, principal_arn: &str) -> bool {
        self.asserted_roles.iter().any(|(first, second)| {
            (first == role_arn && second == principal_arn)
                || (first == principal_arn && second == role_arn)
        })
    }
}

/// The SAML provider the account holds under `principal_arn`, read from IAM's own records in
/// the same engine, so registering a provider is what makes its responses federatable.
///
/// `None` when the account holds no such provider, or when its metadata document describes
/// no issuer: either way there is nothing to verify a response against.
#[must_use]
pub fn registered_provider<Eng: Send + Sync + 'static>(
    engine: &Eng,
    principal_arn: &str,
) -> Option<SamlProvider> {
    let iam = super::iam_state(engine)?;
    let document = {
        let iam = iam.lock();
        iam.saml_providers
            .get(principal_arn)
            .map(|provider| provider.metadata_document.clone())?
    };
    let metadata =
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::parse_saml_metadata(
            &document,
        )?;
    Some(SamlProvider {
        arn: principal_arn.to_string(),
        name: principal_arn
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string(),
        entity_id: metadata.entity_id,
        certificates: metadata.signing_certificates,
    })
}

/// The identity `presented` stands for at `now`, verified against `provider`, or the class
/// it is refused as.
///
/// The order is the one a service provider applies, and each step is decided from what the
/// step before it established; see this module's own documentation for the whole of it.
pub fn resolve(
    provider: &SamlProvider,
    presented: &str,
    now: SystemTime,
) -> Result<SamlAssertion, AssertionDefect> {
    let decoded = decode_base64(presented).ok_or(AssertionDefect::NotBase64)?;
    let text = String::from_utf8(decoded).map_err(|_| AssertionDefect::NotWellFormed)?;
    let document = ::roxmltree::Document::parse(&text).map_err(|_| AssertionDefect::NotWellFormed)?;
    let response = document.root_element();
    if !is_element(&response, PROTOCOL_NAMESPACE, "Response") {
        return Err(AssertionDefect::NotSamlResponse);
    }
    let assertion = response
        .children()
        .find(|node| is_element(node, ASSERTION_NAMESPACE, "Assertion"))
        .ok_or(AssertionDefect::NotSamlResponse)?;

    // Nothing below is read out of a document the provider did not sign.
    let signing_key = verify_signatures(&text, &document, provider, &assertion)?;

    let issuer = element_text(&response, ASSERTION_NAMESPACE, "Issuer")
        .or_else(|| element_text(&assertion, ASSERTION_NAMESPACE, "Issuer"))
        .unwrap_or_default();
    if issuer != provider.entity_id {
        return Err(AssertionDefect::IssuerNotInProvider);
    }

    // "The <StatusCode> element … specifies the status of the corresponding request": a
    // provider that did not authenticate the subject says so here, and that is the provider
    // rejecting the claim rather than a response that cannot be read.
    let status = response
        .children()
        .find(|node| is_element(node, PROTOCOL_NAMESPACE, "Status"))
        .and_then(|status| {
            status
                .children()
                .find(|node| is_element(node, PROTOCOL_NAMESPACE, "StatusCode"))
        })
        .and_then(|code| code.attribute("Value"))
        .unwrap_or(STATUS_SUCCESS)
        .to_string();
    if status != STATUS_SUCCESS {
        return Err(AssertionDefect::ClaimRejected);
    }

    let subject = assertion
        .children()
        .find(|node| is_element(node, ASSERTION_NAMESPACE, "Subject"));
    let name_id = subject.and_then(|subject| {
        subject
            .children()
            .find(|node| is_element(node, ASSERTION_NAMESPACE, "NameID"))
    });
    let subject_type = name_id
        .and_then(|name_id| name_id.attribute("Format"))
        .map(|format| {
            format
                .strip_prefix(NAME_ID_FORMAT_PREFIX)
                .map_or(format, |rest| rest.trim_start_matches(':'))
                .to_string()
        })
        .unwrap_or_default();
    let name_id_text = name_id
        .and_then(|name_id| name_id.text())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    let attributes = attribute_values(&assertion);
    let named = |name: &str| {
        attributes
            .iter()
            .find(|(attribute, _)| attribute == name)
            .map(|(_, value)| value.clone())
    };
    let session_name = named(ROLE_SESSION_NAME_ATTRIBUTE)
        .filter(|name| !name.is_empty())
        .unwrap_or(name_id_text.clone());
    if session_name.is_empty() {
        return Err(AssertionDefect::SessionNameMissing);
    }
    if !is_session_name(&session_name) {
        return Err(AssertionDefect::SessionNamePattern);
    }
    let source_identity = named(SOURCE_IDENTITY_ATTRIBUTE).filter(|value| !value.is_empty());

    // The window: the confirmation data of the response's subject, the conditions the
    // assertion itself states, and the instant the response was issued. A response outside
    // ANY of them is outside the lifetime the provider gave it, and each of the three is its
    // own answer to the caller.
    let at = unix_seconds(now);
    let confirmation_windows: Vec<::roxmltree::Node<'_, '_>> = subject
        .map(|subject| {
            subject
                .children()
                .filter(|node| is_element(node, ASSERTION_NAMESPACE, "SubjectConfirmation"))
                .flat_map(|confirmation| {
                    confirmation
                        .children()
                        .filter(|node| {
                            is_element(node, ASSERTION_NAMESPACE, "SubjectConfirmationData")
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    let condition_windows: Vec<::roxmltree::Node<'_, '_>> = assertion
        .children()
        .filter(|node| is_element(node, ASSERTION_NAMESPACE, "Conditions"))
        .collect();
    if let Some(issued_at) = response
        .attribute("IssueInstant")
        .and_then(parse_xs_date_time)
    {
        let redemption = i64::try_from(REDEMPTION_WINDOW.as_secs()).unwrap_or(i64::MAX);
        if at.saturating_sub(issued_at) > redemption {
            return Err(AssertionDefect::NotRedeemable);
        }
    }
    for windows in [&condition_windows, &confirmation_windows] {
        for window in windows {
            if let Some(not_on_or_after) = window
                .attribute("NotOnOrAfter")
                .and_then(parse_xs_date_time)
            {
                if at >= not_on_or_after {
                    return Err(AssertionDefect::Expired);
                }
            }
            if let Some(not_before) = window.attribute("NotBefore").and_then(parse_xs_date_time) {
                if at < not_before {
                    return Err(AssertionDefect::NotYetValid);
                }
            }
        }
    }

    // "The value of the Recipient attribute of the SubjectConfirmationData element", and the
    // audience the assertion restricts itself to when it names no recipient.
    let audience = subject
        .and_then(|subject| {
            subject
                .descendants()
                .filter(|node| is_element(node, ASSERTION_NAMESPACE, "SubjectConfirmationData"))
                .find_map(|data| data.attribute("Recipient").map(str::to_string))
        })
        .or_else(|| {
            assertion
                .descendants()
                .filter(|node| is_element(node, ASSERTION_NAMESPACE, "Audience"))
                .find_map(|node| node.text().map(|text| text.trim().to_string()))
        })
        .unwrap_or_default();

    let asserted_roles: Vec<(String, String)> = attributes
        .iter()
        .filter(|(attribute, _)| attribute == ROLE_ATTRIBUTE)
        .filter_map(|(_, value)| value.split_once(','))
        .map(|(first, second)| (first.trim().to_string(), second.trim().to_string()))
        .collect();

    let session_not_on_or_after = assertion
        .descendants()
        .filter(|node| is_element(node, ASSERTION_NAMESPACE, "AuthnStatement"))
        .find_map(|statement| {
            statement
                .attribute("SessionNotOnOrAfter")
                .and_then(parse_xs_date_time)
        });

    Ok(SamlAssertion {
        issuer,
        subject: name_id_text,
        subject_type,
        audience,
        session_name,
        asserted_roles,
        source_identity,
        attributes,
        session_not_on_or_after,
        signing_key_digest: signing_key_digest(&signing_key),
    })
}

/// The digest of a signing key, as the origin identity of a SAML session names its issuer
/// by ([`SamlAssertion::signing_key_digest`]): the SHA-256 of the key as the provider's
/// metadata publishes it, base64-coded — the same coding the other hash this exchange
/// reports is carried in ([`name_qualifier`]).
///
/// AWS states that the value IS a hash of that key and not what the hash is computed with,
/// so the digest and its coding are stated here. Both are a function of the key alone: two
/// exchanges of a response signed by one key report one value, and a provider that rotates
/// its key reports another.
#[must_use]
fn signing_key_digest(key: &[u8]) -> String {
    let digest = ::ring::digest::digest(&::ring::digest::SHA256, key);
    ::aws_smithy_types::base64::encode(digest.as_ref())
}

/// The condition keys a SAML identity provider publishes about one of its responses, for a
/// role's trust policy to be decided against.
///
/// AWS publishes the response's own values under the `saml:` prefix: `saml:iss` is "the
/// issuer", `saml:aud` "the endpoint URL to which the SAML assertion is sent", `saml:sub`
/// the subject of the claim, `saml:sub_type` its format, `saml:namequalifier` the hash the
/// exchange reports, `saml:doc` "the principal that was used to assume the role" written as
/// the account and the provider's friendly name, and every attribute the assertion carries
/// is published under its own name. A clause written over any one of them is therefore
/// decided against the value the provider signed.
#[must_use]
pub fn condition_context(
    assertion: &SamlAssertion,
    provider: &SamlProvider,
    account: &str,
) -> Vec<(String, String)> {
    let mut context = vec![
        ("saml:iss".to_string(), assertion.issuer.clone()),
        ("saml:aud".to_string(), assertion.audience.clone()),
        ("saml:sub".to_string(), assertion.subject.clone()),
        ("saml:sub_type".to_string(), assertion.subject_type.clone()),
        (
            "saml:namequalifier".to_string(),
            name_qualifier(&assertion.issuer, account, &provider.name),
        ),
        (
            "saml:doc".to_string(),
            format!("{account}/{}", provider.name),
        ),
    ];
    // Every attribute the provider asserted, under the name it asserted it by: a trust
    // policy written over a claim this emulator does not model is still decided against the
    // value the assertion carries.
    context.extend(assertion.attributes.clone());
    context
}

/// The keys [`condition_context`] publishes, as the policy evaluator needs them: the keys
/// whose absence from the context means the response carried no such value rather than this
/// emulator not knowing the key.
#[must_use]
pub fn published_condition_keys(
    assertion: &SamlAssertion,
    provider: &SamlProvider,
    account: &str,
) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (key, _) in condition_context(assertion, provider, account) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Every spelling of a SAML provider that a role's trust policy may name in its `Principal`
/// element: a SAML identity is admitted as a `Federated` principal, named by the ARN of the
/// IAM provider the account registered for it.
#[must_use]
pub fn principals(provider: &SamlProvider) -> Vec<String> {
    vec![provider.arn.clone(), provider.entity_id.clone()]
}

/// "A hash value based on the concatenation of the following: The Issuer response value. The
/// Amazon Web Services account ID. The friendly name … of the SAML provider in IAM", as
/// `AssumeRoleWithSAMLResponse$NameQualifier`'s own pseudocode states it:
/// `BASE64 ( SHA1 ( "https://example.com/saml" + "123456789012" + "/MySAMLIdP" ) )`.
#[must_use]
pub fn name_qualifier(issuer: &str, account: &str, provider_name: &str) -> String {
    let concatenated = format!("{issuer}{account}/{provider_name}");
    let digest = ::ring::digest::digest(
        &::ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        concatenated.as_bytes(),
    );
    ::aws_smithy_types::base64::encode(digest.as_ref())
}

/// Whether every signature the document carries checks out against a key `provider`
/// publishes, and that it carries one at all — and, when they do, the KEY that signed the
/// `assertion`, which is what the session's origin identity names its issuer by
/// ([`SamlAssertion::signing_key_digest`]).
///
/// A response may be signed at the `Response` level, at the `Assertion` level, or both; AWS
/// accepts either, and every signature present has to check out — a document carrying one
/// good and one bad signature is not one the provider issued. The key reported is the one
/// that signed the assertion itself where the assertion carries a signature, and otherwise
/// the one that signed the response the assertion travelled in: either way the key whose
/// holder vouched for the identity read out below.
fn verify_signatures(
    text: &str,
    document: &::roxmltree::Document<'_>,
    provider: &SamlProvider,
    assertion: &::roxmltree::Node<'_, '_>,
) -> Result<Vec<u8>, AssertionDefect> {
    let signatures: Vec<::roxmltree::Node<'_, '_>> = document
        .root_element()
        .descendants()
        .filter(|node| is_element(node, SIGNATURE_NAMESPACE, "Signature"))
        .take(MAX_SIGNATURES.saturating_add(1))
        .collect();
    if signatures.is_empty() {
        return Err(AssertionDefect::NotSigned);
    }
    // A response has at most the two signatures SAML 2.0 places in one: one over the
    // `Response` and one over the `Assertion`. Checking one costs a digest over the element it
    // covers, so a document carrying more is not read at all — it is refused as a signature
    // that does not hold, before any of them is checked.
    if signatures.len() > MAX_SIGNATURES {
        return Err(AssertionDefect::SignatureInvalid);
    }
    let keys: Vec<Vec<u8>> = provider
        .certificates
        .iter()
        .filter_map(|certificate| decode_base64(certificate))
        .collect();
    let mut over_the_assertion: Option<Vec<u8>> = None;
    let mut over_the_response: Option<Vec<u8>> = None;
    for signature in &signatures {
        let key = verify_signature(text, document, signature, &keys)?;
        if signature.ancestors().any(|node| node == *assertion) {
            over_the_assertion = Some(key);
        } else {
            over_the_response = Some(key);
        }
    }
    over_the_assertion
        .or(over_the_response)
        .ok_or(AssertionDefect::NotSigned)
}

/// Whether ONE signature checks out: its reference digests the element it names, and its
/// `SignatureValue` is that `SignedInfo` signed by one of `keys` — which is the key reported
/// back.
fn verify_signature(
    text: &str,
    document: &::roxmltree::Document<'_>,
    signature: &::roxmltree::Node<'_, '_>,
    keys: &[Vec<u8>],
) -> Result<Vec<u8>, AssertionDefect> {
    let signed_info = signature
        .children()
        .find(|node| is_element(node, SIGNATURE_NAMESPACE, "SignedInfo"))
        .ok_or(AssertionDefect::SignatureInvalid)?;
    let canonicalization = algorithm(&signed_info, "CanonicalizationMethod")
        .ok_or(AssertionDefect::SignatureInvalid)?;
    if !ACCEPTED_CANONICALIZATIONS.contains(&canonicalization.as_str()) {
        return Err(AssertionDefect::UnacceptedTransform);
    }
    let signature_method =
        algorithm(&signed_info, "SignatureMethod").ok_or(AssertionDefect::SignatureInvalid)?;
    let reference = signed_info
        .children()
        .find(|node| is_element(node, SIGNATURE_NAMESPACE, "Reference"))
        .ok_or(AssertionDefect::SignatureInvalid)?;
    for transform in reference
        .children()
        .filter(|node| is_element(node, SIGNATURE_NAMESPACE, "Transforms"))
        .flat_map(|transforms| {
            transforms
                .children()
                .filter(|node| is_element(node, SIGNATURE_NAMESPACE, "Transform"))
                .collect::<Vec<_>>()
        })
    {
        let named = transform.attribute("Algorithm").unwrap_or_default();
        if !ACCEPTED_TRANSFORMS.contains(&named) {
            return Err(AssertionDefect::UnacceptedTransform);
        }
    }
    let digest_method =
        algorithm(&reference, "DigestMethod").ok_or(AssertionDefect::SignatureInvalid)?;
    let digest_algorithm =
        digest_algorithm(&digest_method).ok_or(AssertionDefect::UnacceptedTransform)?;
    let digest_value = signed_info
        .descendants()
        .find(|node| is_element(node, SIGNATURE_NAMESPACE, "DigestValue"))
        .and_then(|node| node.text())
        .and_then(decode_base64)
        .ok_or(AssertionDefect::SignatureInvalid)?;

    // The element the reference names, which the digest is over: the whole document for an
    // empty reference, otherwise the element carrying that id.
    let referenced = match reference.attribute("URI").unwrap_or_default() {
        "" => document.root_element(),
        uri => {
            let id = uri.strip_prefix('#').ok_or(AssertionDefect::SignatureInvalid)?;
            document
                .root_element()
                .descendants()
                .find(|node| {
                    node.is_element()
                        && (node.attribute("ID") == Some(id) || node.attribute("Id") == Some(id))
                })
                .ok_or(AssertionDefect::SignatureInvalid)?
        }
    };
    let signed = transformed(text, &referenced, signature);
    if ::ring::digest::digest(digest_algorithm, signed.as_bytes()).as_ref() != digest_value.as_slice()
    {
        return Err(AssertionDefect::SignatureInvalid);
    }

    let signature_value = signature
        .children()
        .find(|node| is_element(node, SIGNATURE_NAMESPACE, "SignatureValue"))
        .and_then(|node| node.text())
        .and_then(decode_base64)
        .ok_or(AssertionDefect::SignatureInvalid)?;
    let signed_info_bytes = text.get(signed_info.range()).unwrap_or_default();
    let checks_out = keys.iter().find(|key| {
        signature_checks_out(
            &signature_method,
            key,
            signed_info_bytes.as_bytes(),
            &signature_value,
        )
    });
    match checks_out {
        Some(key) => Ok(key.clone()),
        None => Err(AssertionDefect::SignatureInvalid),
    }
}

/// The transform output for the reference: the element's own bytes with the enveloping
/// `Signature` removed, which is what canonicalizing an already-canonical document yields
/// (see this module's documentation).
///
/// Every range comes from the parser's own view of the document it parsed, so a range it
/// does not resolve reads as nothing rather than as a slice of something else: the digest
/// then does not hold and the response is refused.
fn transformed(
    text: &str,
    referenced: &::roxmltree::Node<'_, '_>,
    signature: &::roxmltree::Node<'_, '_>,
) -> String {
    let element = referenced.range();
    let enveloped = signature.range();
    if enveloped.start >= element.start && enveloped.end <= element.end {
        let mut output = String::with_capacity(element.end - element.start);
        output.push_str(text.get(element.start..enveloped.start).unwrap_or_default());
        output.push_str(text.get(enveloped.end..element.end).unwrap_or_default());
        output
    } else {
        text.get(element).unwrap_or_default().to_string()
    }
}

/// Whether `signature` is `signed` signed under `algorithm` by the key the X.509
/// certificate `certificate` certifies.
///
/// The algorithms are the ones W3C "XML Signature Syntax and Processing" and its `-more`
/// companion register for RSASSA-PKCS1-v1_5 and ECDSA at each digest width, checked through
/// the ONE cryptography library this workspace verifies signatures with. An algorithm
/// outside them, or a key whose own algorithm is not the one named, does not check out.
fn signature_checks_out(
    algorithm: &str,
    certificate: &[u8],
    signed: &[u8],
    signature: &[u8],
) -> bool {
    use ::x509_cert::der::Decode as _;

    let Ok(parsed) = ::x509_cert::Certificate::from_der(certificate) else {
        return false;
    };
    let key_info = parsed.tbs_certificate.subject_public_key_info;
    let Some(key) = key_info.subject_public_key.as_bytes() else {
        return false;
    };
    let rsa = key_info.algorithm.oid.to_string() == "1.2.840.113549.1.1.1";
    let elliptic_curve = key_info.algorithm.oid.to_string() == "1.2.840.10045.2.1";
    let verified = match algorithm {
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256" if rsa => ::ring::signature::
            UnparsedPublicKey::new(&::ring::signature::RSA_PKCS1_2048_8192_SHA256, key)
            .verify(signed, signature),
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384" if rsa => ::ring::signature::
            UnparsedPublicKey::new(&::ring::signature::RSA_PKCS1_2048_8192_SHA384, key)
            .verify(signed, signature),
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha512" if rsa => ::ring::signature::
            UnparsedPublicKey::new(&::ring::signature::RSA_PKCS1_2048_8192_SHA512, key)
            .verify(signed, signature),
        "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256" if elliptic_curve => {
            ::ring::signature::UnparsedPublicKey::new(
                &::ring::signature::ECDSA_P256_SHA256_FIXED,
                key,
            )
            .verify(signed, signature)
        }
        "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha384" if elliptic_curve => {
            ::ring::signature::UnparsedPublicKey::new(
                &::ring::signature::ECDSA_P384_SHA384_FIXED,
                key,
            )
            .verify(signed, signature)
        }
        _ => return false,
    };
    verified.is_ok()
}

/// The digest the identifier `named` registers, or `None` for one outside the registered
/// widths (W3C "XML Signature Syntax and Processing" §6.2 and its `-more` companion).
fn digest_algorithm(named: &str) -> Option<&'static ::ring::digest::Algorithm> {
    match named {
        "http://www.w3.org/2001/04/xmlenc#sha256" => Some(&::ring::digest::SHA256),
        "http://www.w3.org/2001/04/xmldsig-more#sha384" => Some(&::ring::digest::SHA384),
        "http://www.w3.org/2001/04/xmlenc#sha512" => Some(&::ring::digest::SHA512),
        _ => None,
    }
}

/// The `Algorithm` attribute of the `local` child of `parent`, in the signature namespace.
fn algorithm(parent: &::roxmltree::Node<'_, '_>, local: &str) -> Option<String> {
    parent
        .children()
        .find(|node| is_element(node, SIGNATURE_NAMESPACE, local))
        .and_then(|node| node.attribute("Algorithm"))
        .map(str::to_string)
}

/// Every `Attribute` the assertion carries, as (name, value) pairs — one pair per
/// `AttributeValue`, because an attribute may carry several (the `Role` attribute of a
/// subject permitted more than one role does).
fn attribute_values(assertion: &::roxmltree::Node<'_, '_>) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    for attribute in assertion
        .descendants()
        .filter(|node| is_element(node, ASSERTION_NAMESPACE, "Attribute"))
    {
        let Some(name) = attribute.attribute("Name") else {
            continue;
        };
        for value in attribute
            .children()
            .filter(|node| is_element(node, ASSERTION_NAMESPACE, "AttributeValue"))
        {
            values.push((
                name.to_string(),
                value.text().unwrap_or_default().trim().to_string(),
            ));
        }
    }
    values
}

/// Whether `node` is the element `local` of `namespace`.
fn is_element(node: &::roxmltree::Node<'_, '_>, namespace: &str, local: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace() == Some(namespace)
        && node.tag_name().name() == local
}

/// The text of the `local` child of `parent`, trimmed.
fn element_text(
    parent: &::roxmltree::Node<'_, '_>,
    namespace: &str,
    local: &str,
) -> Option<String> {
    parent
        .children()
        .find(|node| is_element(node, namespace, local))
        .and_then(|node| node.text())
        .map(|text| text.trim().to_string())
}

/// Whether `name` is a role session name: `roleSessionNameType`'s own length and character
/// class, which is what AWS holds the name an assertion carries to.
fn is_session_name(name: &str) -> bool {
    let characters = name.chars().count();
    (SESSION_NAME_MIN_CHARS..=SESSION_NAME_MAX_CHARS).contains(&characters)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_+=,.@-".contains(c))
}

/// `value` read as base64, with the whitespace a wrapped body carries ignored. The standard
/// alphabet with padding, which is what an XML signature's values and a SAML authentication
/// response are encoded in.
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let packed: String = value.split_whitespace().collect();
    if packed.is_empty() {
        return None;
    }
    ::aws_smithy_types::base64::decode(&packed).ok()
}

/// The instant an `xs:dateTime` attribute names, as seconds since the Unix epoch.
fn parse_xs_date_time(text: &str) -> Option<i64> {
    let (civil, offset_secs) =
        ::service_common::rfc3339::split_utc_offset(text).unwrap_or((text, 0));
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    parsed.secs().checked_sub(offset_secs)
}

/// `at` as whole seconds since the Unix epoch, the unit the window comparisons are made in.
fn unix_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
