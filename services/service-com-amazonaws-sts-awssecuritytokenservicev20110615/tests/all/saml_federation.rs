// SPDX-License-Identifier: CC0-1.0

//! `AssumeRoleWithSAML` federates a SAML 2.0 identity only when the SAML provider the
//! account registered really signed the authentication response it is presented.
//!
//! RULE: a presented SAML authentication response is exchanged for a role session only when
//! it is the base64 of a SAML 2.0 `Response` whose XML signature — under transforms and
//! algorithms a verifier accepts — checks out against a key the REGISTERED provider
//! publishes in its metadata, which names that provider as its issuer, reports a successful
//! authentication, names the session and the role pair asked for, and is presented inside
//! the validity window it states; every other outcome is refused as the class it is, each
//! with the fixed text AWS words for that class — a response nothing signed, a signature
//! that did not check out (including one made with a key the provider does not publish), a
//! transform that selects the signed content by an expression of its own, an issuer that is
//! not the provider's, a window that has passed or not begun, and a claim the provider
//! itself rejected.
//!
//! Every row drives the real path end to end on ONE shared engine: the provider and the role
//! are created through IAM's own operations, the response is signed HERE with a key pair
//! minted for the row and verified by the service against the certificate that provider's
//! metadata publishes, and the call is made UNSIGNED, as a client holding no Amazon Web
//! Services credential makes it. The rows differ along what the rule generalises over: which
//! part of the response was altered (its encoding, its XML, its signature, its transforms,
//! its issuer, its status, its attributes, its window), whose key signed it, and which side
//! of the window the clock is on. The recorded cases — a non-base64 assertion, a forged one,
//! an expired one, an XPath-transform one, a claim the provider rejected and the successful
//! positive control — are rows of the table.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::sts_sessions::{
    element, emulator_account, member, query, LONG_TERM_KEY, STS_API_VERSION, STS_SIGNING_NAME,
};
use super::web_identity_federation::unsigned_call;

/// IAM's SigV4 signing name and awsQuery API version: the provider and the role are
/// registered through IAM's own operations, not poked into its store.
const IAM_SIGNING_NAME: &str = "iam";
const IAM_API_VERSION: &str = "2010-05-08";

/// The endpoint a SAML authentication response for AWS is addressed to, which is the
/// `Recipient` an assertion names and therefore the `saml:aud` a trust policy decides on
/// (AWS "Configuring SAML assertions for the authentication response").
const AWS_SAML_ENDPOINT: &str = "https://signin.aws.amazon.com/saml";

/// The session the responses below name.
const SESSION_NAME: &str = "saml-federated";

/// The namespaces and algorithm identifiers a SAML 2.0 authentication response and its XML
/// signature are written in.
const PROTOCOL_NAMESPACE: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
const ASSERTION_NAMESPACE: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const SIGNATURE_NAMESPACE: &str = "http://www.w3.org/2000/09/xmldsig#";
const STATUS_SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";
const STATUS_AUTHN_FAILED: &str = "urn:oasis:names:tc:SAML:2.0:status:AuthnFailed";
const ENVELOPED_SIGNATURE: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
const EXCLUSIVE_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const XPATH_TRANSFORM: &str = "http://www.w3.org/TR/1999/REC-xpath-19991116";
const ECDSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256";
const SHA256_DIGEST: &str = "http://www.w3.org/2001/04/xmlenc#sha256";

/// The attributes AWS reads out of an assertion.
const ROLE_ATTRIBUTE: &str = "https://aws.amazon.com/SAML/Attributes/Role";
const ROLE_SESSION_NAME_ATTRIBUTE: &str = "https://aws.amazon.com/SAML/Attributes/RoleSessionName";

/// The sentences AWS words each refusal class with.
const SIGNATURE_INVALID_MESSAGE: &str = "Response signature invalid";
const ISSUER_NOT_IN_PROVIDER_MESSAGE: &str = "Issuer not present in specified provider";
const PROVIDER_NOT_FOUND_MESSAGE: &str = "Specified provider doesn't exist";
const ROLE_NOT_ASSERTED_MESSAGE: &str = "Specified role could not be found in the SAML assertion";
const NOT_SIGNED_MESSAGE: &str = "Response is not signed";
const NOT_BASE64_MESSAGE: &str = "Invalid base64 SAMLResponse";
const NOT_REDEEMABLE_MESSAGE: &str = "Token must be redeemed within 5 minutes of issuance";
const SESSION_NAME_REQUIRED_MESSAGE: &str =
    "RoleSessionName is required in AuthnResponse (service provider)";
const SESSION_NAME_PATTERN_MESSAGE: &str =
    "RoleSessionName in AuthnResponse must match [a-zA-Z_0-9+=,.@-]{2,64}";
const RESPONSE_EXPIRED_MESSAGE: &str = "Response has expired";
const RESPONSE_NOT_YET_VALID_MESSAGE: &str = "Response is not yet valid";
const IDP_REJECTED_CLAIM_MESSAGE: &str =
    "The identity provider (IdP) reported that authentication failed.";
const NOT_AUTHORIZED_MESSAGE: &str = "Not authorized to perform sts:AssumeRoleWithSAML";

/// The emulator these rows drive: one engine carrying IAM's SAML providers and roles and STS
/// beside them, mounted as the router mounts them, so every record a decision reads is the
/// one its owning service wrote.
pub(crate) fn emulator_with_iam() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One IAM awsQuery call from the long-term caller, required to succeed.
async fn iam_call(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let (status, body) = query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
        action,
        params,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {action}: expected HTTP 200 from IAM, got {status} {body}"
    );
    body
}

/// A SAML identity provider as the test plays it: a key pair it signs with, and the
/// certificate its metadata publishes for that key.
pub(crate) struct IdentityProvider {
    pub(crate) entity_id: String,
    /// The signing key, as the PKCS#8 the signer is built from for each signature.
    pub(crate) pkcs8: Vec<u8>,
    /// The base64 of the DER X.509 certificate the metadata publishes.
    pub(crate) certificate: String,
}

impl IdentityProvider {
    /// A fresh provider identity: a key pair minted for it, and a self-signed certificate
    /// certifying that key — the same shape a real IdP publishes in its metadata.
    pub(crate) fn mint(entity_id: &str) -> Self {
        let key = rcgen::KeyPair::generate().expect("a key pair");
        let mut params =
            rcgen::CertificateParams::new(Vec::<String>::new()).expect("certificate parameters");
        let mut subject = rcgen::DistinguishedName::new();
        subject.push(rcgen::DnType::CommonName, entity_id.to_string());
        params.distinguished_name = subject;
        let certificate = params.self_signed(&key).expect("a self-signed certificate");
        Self {
            entity_id: entity_id.to_string(),
            pkcs8: key.serialize_der(),
            certificate: aws_smithy_types::base64::encode(certificate.der().as_ref()),
        }
    }

    /// `signed` signed with this provider's key, as the base64 an XML signature carries.
    fn sign(&self, signed: &[u8]) -> String {
        let random = ring::rand::SystemRandom::new();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &self.pkcs8,
            &random,
        )
        .expect("the generated key pair is readable");
        let signature = pair
            .sign(&random, signed)
            .expect("the key pair signs the SignedInfo");
        aws_smithy_types::base64::encode(signature.as_ref())
    }

    /// The SAML 2.0 metadata document this provider publishes: its entity id and the one key
    /// a reader checks its messages against, in the descriptor a real IdP publishes — the
    /// signing and encryption key descriptors, the name identifier formats it supports and
    /// its endpoints, which is what carries a real document past
    /// `SAMLMetadataDocumentType`'s 1000-character minimum.
    fn metadata(&self) -> String {
        let document = format!(
            "<EntityDescriptor xmlns=\"urn:oasis:names:tc:SAML:2.0:metadata\" \
             entityID=\"{entity}\">\
             <IDPSSODescriptor protocolSupportEnumeration=\"{PROTOCOL_NAMESPACE}\">\
             <KeyDescriptor use=\"signing\"><KeyInfo xmlns=\"{SIGNATURE_NAMESPACE}\">\
             <X509Data><X509Certificate>{certificate}</X509Certificate></X509Data>\
             </KeyInfo></KeyDescriptor>\
             <KeyDescriptor use=\"encryption\"><KeyInfo xmlns=\"{SIGNATURE_NAMESPACE}\">\
             <X509Data><X509Certificate>{certificate}</X509Certificate></X509Data>\
             </KeyInfo></KeyDescriptor>\
             <NameIDFormat>urn:oasis:names:tc:SAML:2.0:nameid-format:transient</NameIDFormat>\
             <NameIDFormat>urn:oasis:names:tc:SAML:2.0:nameid-format:persistent</NameIDFormat>\
             <NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress\
             </NameIDFormat>\
             <SingleLogoutService \
             Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect\" \
             Location=\"{entity}/logout\"/>\
             <SingleSignOnService \
             Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST\" \
             Location=\"{entity}/sso\"/>\
             <SingleSignOnService \
             Binding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect\" \
             Location=\"{entity}/sso\"/>\
             </IDPSSODescriptor>\
             <Organization><OrganizationName xml:lang=\"en\">Example</OrganizationName>\
             <OrganizationDisplayName xml:lang=\"en\">Example Corp</OrganizationDisplayName>\
             <OrganizationURL xml:lang=\"en\">{entity}</OrganizationURL></Organization>\
             </EntityDescriptor>",
            entity = self.entity_id,
            certificate = self.certificate,
        );
        assert!(
            document.chars().count() >= 1000,
            "the published metadata must satisfy SAMLMetadataDocumentType's 1000-character \
             minimum, got {} characters",
            document.chars().count()
        );
        document
    }
}

/// The parts of an authentication response a row varies. Everything not named here is the
/// same in every row, so each row differs from the positive control in exactly what it sets.
pub(crate) struct Response {
    /// Seconds from now the response was issued at.
    pub(crate) issued_at_offset: i64,
    /// Seconds from now the validity window opens and closes.
    pub(crate) not_before_offset: i64,
    /// `None` states no `NotOnOrAfter` at all.
    pub(crate) not_on_or_after_offset: Option<i64>,
    /// The instant the ASSERTION's own conditions close at, when the row states a different
    /// one from the window its subject confirmation states.
    pub(crate) conditions_not_on_or_after_offset: Option<i64>,
    /// Seconds from now the authentication statement bounds the SESSION at, when it bounds
    /// it at all.
    pub(crate) session_not_on_or_after_offset: Option<i64>,
    pub(crate) status: &'static str,
    pub(crate) issuer: String,
    pub(crate) name_id: String,
    /// The `(role, provider)` pairs the `Role` attribute asserts.
    pub(crate) asserted: Vec<(String, String)>,
    /// Whether the response carries a `RoleSessionName` attribute.
    pub(crate) names_session: bool,
    /// The transforms the reference names.
    pub(crate) transforms: Vec<&'static str>,
    /// Whether the response carries an XML signature at all.
    pub(crate) signed: bool,
    /// Flip one character of the signed assertion AFTER it was signed, so the digest no
    /// longer covers the document presented.
    pub(crate) altered_after_signing: bool,
}

impl Response {
    /// The response the positive control presents: issued just now, valid for ten minutes,
    /// successful, from the provider, naming the session and the role pair asked for.
    pub(crate) fn valid(provider: &IdentityProvider, role_arn: &str, provider_arn: &str) -> Self {
        Self {
            issued_at_offset: -30,
            not_before_offset: -30,
            not_on_or_after_offset: Some(600),
            conditions_not_on_or_after_offset: None,
            session_not_on_or_after_offset: None,
            status: STATUS_SUCCESS,
            issuer: provider.entity_id.clone(),
            name_id: SESSION_NAME.to_string(),
            asserted: vec![(role_arn.to_string(), provider_arn.to_string())],
            names_session: true,
            transforms: vec![ENVELOPED_SIGNATURE, EXCLUSIVE_C14N],
            signed: true,
            altered_after_signing: false,
        }
    }
}

/// An `xs:dateTime` `offset` seconds from now, as a SAML timestamp is written.
fn instant(offset: i64) -> String {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs(),
    )
    .expect("the current time fits in an i64");
    aws_smithy_types::DateTime::from_secs(now + offset)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an ISO-8601 instant")
}

/// The base64 `SAMLAssertion` a row presents: a SAML 2.0 `Response` carrying one assertion,
/// written in Exclusive-C14N form and — unless the row says otherwise — signed with the
/// provider's own key over exactly the bytes of that assertion with the signature removed,
/// which is what the enveloped-signature transform leaves of it.
pub(crate) fn saml_assertion(provider: &IdentityProvider, row: &Response) -> String {
    let assertion_id = "_assertion";
    let attributes = {
        let mut attributes = String::new();
        if row.names_session {
            attributes.push_str(&format!(
                "<saml:Attribute Name=\"{ROLE_SESSION_NAME_ATTRIBUTE}\">\
                 <saml:AttributeValue>{}</saml:AttributeValue></saml:Attribute>",
                row.name_id
            ));
        }
        for (role, saml_provider) in &row.asserted {
            attributes.push_str(&format!(
                "<saml:Attribute Name=\"{ROLE_ATTRIBUTE}\">\
                 <saml:AttributeValue>{role},{saml_provider}</saml:AttributeValue>\
                 </saml:Attribute>"
            ));
        }
        attributes
    };
    let window = match row.not_on_or_after_offset {
        Some(offset) => format!(
            " NotBefore=\"{}\" NotOnOrAfter=\"{}\"",
            instant(row.not_before_offset),
            instant(offset)
        ),
        None => format!(" NotBefore=\"{}\"", instant(row.not_before_offset)),
    };
    let conditions_window = match row
        .conditions_not_on_or_after_offset
        .or(row.not_on_or_after_offset)
    {
        Some(offset) => format!(
            " NotBefore=\"{}\" NotOnOrAfter=\"{}\"",
            instant(row.not_before_offset),
            instant(offset)
        ),
        None => format!(" NotBefore=\"{}\"", instant(row.not_before_offset)),
    };
    let session_bound = match row.session_not_on_or_after_offset {
        Some(offset) => format!(" SessionNotOnOrAfter=\"{}\"", instant(offset)),
        None => String::new(),
    };
    // The assertion, split where its signature belongs: a SAML signature follows the
    // `Issuer` ("Assertions and Protocols", §2.3.3), so removing it leaves the head and the
    // tail joined — which is exactly what the digest is taken over.
    let head = format!(
        "<saml:Assertion xmlns:saml=\"{ASSERTION_NAMESPACE}\" ID=\"{assertion_id}\" \
         IssueInstant=\"{issued}\" Version=\"2.0\">\
         <saml:Issuer>{issuer}</saml:Issuer>",
        issued = instant(row.issued_at_offset),
        issuer = row.issuer,
    );
    let tail = format!(
        "<saml:Subject>\
         <saml:NameID Format=\"urn:oasis:names:tc:SAML:2.0:nameid-format:transient\">\
         {name_id}</saml:NameID>\
         <saml:SubjectConfirmation Method=\"urn:oasis:names:tc:SAML:2.0:cm:bearer\">\
         <saml:SubjectConfirmationData Recipient=\"{AWS_SAML_ENDPOINT}\"{window}/>\
         </saml:SubjectConfirmation></saml:Subject>\
         <saml:Conditions{conditions_window}>\
         <saml:AudienceRestriction><saml:Audience>{AWS_SAML_ENDPOINT}</saml:Audience>\
         </saml:AudienceRestriction></saml:Conditions>\
         <saml:AuthnStatement AuthnInstant=\"{issued}\"{session_bound}>\
         <saml:AuthnContext><saml:AuthnContextClassRef>\
         urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport\
         </saml:AuthnContextClassRef></saml:AuthnContext></saml:AuthnStatement>\
         <saml:AttributeStatement>{attributes}</saml:AttributeStatement>\
         </saml:Assertion>",
        name_id = row.name_id,
        issued = instant(row.issued_at_offset),
    );

    let assertion = if row.signed {
        let digested = format!("{head}{tail}");
        let digest = ring::digest::digest(&ring::digest::SHA256, digested.as_bytes());
        let transforms: String = row
            .transforms
            .iter()
            .map(|algorithm| format!("<ds:Transform Algorithm=\"{algorithm}\"></ds:Transform>"))
            .collect();
        let signed_info = format!(
            "<ds:SignedInfo xmlns:ds=\"{SIGNATURE_NAMESPACE}\">\
             <ds:CanonicalizationMethod Algorithm=\"{EXCLUSIVE_C14N}\">\
             </ds:CanonicalizationMethod>\
             <ds:SignatureMethod Algorithm=\"{ECDSA_SHA256}\"></ds:SignatureMethod>\
             <ds:Reference URI=\"#{assertion_id}\"><ds:Transforms>{transforms}</ds:Transforms>\
             <ds:DigestMethod Algorithm=\"{SHA256_DIGEST}\"></ds:DigestMethod>\
             <ds:DigestValue>{digest}</ds:DigestValue></ds:Reference></ds:SignedInfo>",
            digest = aws_smithy_types::base64::encode(digest.as_ref()),
        );
        let signature = format!(
            "<ds:Signature xmlns:ds=\"{SIGNATURE_NAMESPACE}\">{signed_info}\
             <ds:SignatureValue>{value}</ds:SignatureValue>\
             <ds:KeyInfo><ds:X509Data><ds:X509Certificate>{certificate}</ds:X509Certificate>\
             </ds:X509Data></ds:KeyInfo></ds:Signature>",
            value = provider.sign(signed_info.as_bytes()),
            certificate = provider.certificate,
        );
        format!("{head}{signature}{tail}")
    } else {
        format!("{head}{tail}")
    };
    let assertion = if row.altered_after_signing {
        // One character of the SIGNED content changed, which is what a tampered response
        // is: the signature still verifies over the `SignedInfo`, and the digest in it no
        // longer matches the document presented.
        assertion.replace(&row.name_id, "another-identity")
    } else {
        assertion
    };
    let response = format!(
        "<samlp:Response xmlns:samlp=\"{PROTOCOL_NAMESPACE}\" \
         xmlns:saml=\"{ASSERTION_NAMESPACE}\" ID=\"_response\" IssueInstant=\"{issued}\" \
         Version=\"2.0\" Destination=\"{AWS_SAML_ENDPOINT}\">\
         <saml:Issuer>{issuer}</saml:Issuer>\
         <samlp:Status><samlp:StatusCode Value=\"{status}\"></samlp:StatusCode></samlp:Status>\
         {assertion}</samlp:Response>",
        issued = instant(row.issued_at_offset),
        issuer = row.issuer,
        status = row.status,
    );
    aws_smithy_types::base64::encode(response.as_bytes())
}

/// Register `provider` in IAM through its own operation, and answer the provider ARN a trust
/// policy names it by.
pub(crate) async fn create_saml_provider(
    router: &MultiServiceRouter,
    name: &str,
    provider: &IdentityProvider,
) -> String {
    let body = iam_call(
        router,
        "CreateSAMLProvider",
        &[
            ("Name", name.to_string()),
            ("SAMLMetadataDocument", provider.metadata()),
        ],
    )
    .await;
    member("create a SAML provider", &body, "SAMLProviderArn")
}

/// Create, through IAM's own operation, a role whose trust policy admits the SAML identities
/// `provider_arn` asserts, under `condition`.
pub(crate) async fn create_saml_role(
    router: &MultiServiceRouter,
    role_name: &str,
    provider_arn: &str,
    condition: Value,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Federated": provider_arn },
            "Action": ["sts:AssumeRoleWithSAML"],
            "Condition": condition,
        }],
    })
    .to_string();
    let body = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", "/".to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    member("create a SAML role", &body, "Arn")
}

/// The clause the suite's roles carry: the endpoint the assertion was addressed to, which is
/// the `saml:aud` the provider publishes about it.
pub(crate) fn audience_condition() -> Value {
    json!({ "StringEquals": { "SAML:aud": AWS_SAML_ENDPOINT } })
}

/// One `AssumeRoleWithSAML`, made the way a SAML client makes it: UNSIGNED, holding no
/// Amazon Web Services credential at all.
pub(crate) async fn exchange(
    router: &MultiServiceRouter,
    role_arn: &str,
    principal_arn: &str,
    assertion: &str,
) -> (u16, String) {
    unsigned_call(
        router,
        "AssumeRoleWithSAML",
        &[
            ("RoleArn", role_arn.to_string()),
            ("PrincipalArn", principal_arn.to_string()),
            ("SAMLAssertion", assertion.to_string()),
        ],
    )
    .await
}

/// What a row expects of the exchange.
enum Expected {
    /// A session of the role, reporting the facts the response carried.
    Federated,
    /// This status, this `<Code>` and exactly this message.
    Refused(u16, &'static str, String),
}

#[tokio::test]
async fn a_saml_identity_is_federated_only_when_its_registered_provider_signed_the_response() {
    let (router, _engine) = emulator_with_iam();
    let provider = IdentityProvider::mint("https://idp.example.test/saml");
    let provider_arn = create_saml_provider(&router, "corp-idp", &provider).await;
    // A SECOND provider, registered by the account but not the one the rows name: its key is
    // a key the named provider does not publish, which is what a forged response is signed
    // with.
    let other = IdentityProvider::mint("https://other.example.test/saml");
    create_saml_provider(&router, "other-idp", &other).await;
    let role_name = "saml-federated";
    let role_arn = create_saml_role(&router, role_name, &provider_arn, audience_condition()).await;
    // A role whose trust policy admits only the OTHER provider, so a response this provider
    // signed is verified and then not admitted.
    let unadmitted_role = create_saml_role(
        &router,
        "saml-unadmitted",
        &iam_provider_arn("other-idp"),
        audience_condition(),
    )
    .await;

    let valid = || Response::valid(&provider, &role_arn, &provider_arn);
    let signed = |row: Response| saml_assertion(&provider, &row);

    // One row: the case, the role the request names, the provider ARN it names, the
    // `SAMLAssertion` it presents, and what the rule answers for it.
    let rows: Vec<(&str, String, String, String, Expected)> = vec![
        (
            // The recorded positive control.
            "a freshly signed, self-contained response from the registered provider",
            role_arn.clone(),
            provider_arn.clone(),
            signed(valid()),
            Expected::Federated,
        ),
        (
            "a value that is not base64 at all",
            role_arn.clone(),
            provider_arn.clone(),
            "not-base64".to_string(),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                NOT_BASE64_MESSAGE.to_string(),
            ),
        ),
        (
            "base64 of something that is not XML",
            role_arn.clone(),
            provider_arn.clone(),
            aws_smithy_types::base64::encode("this is not an XML document"),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                "Invalid SAMLResponse: the value is not a well-formed XML document".to_string(),
            ),
        ),
        (
            "base64 of well-formed XML that is not a SAML Response",
            role_arn.clone(),
            provider_arn.clone(),
            aws_smithy_types::base64::encode("<html><body>signed in</body></html>"),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                "Invalid SAMLResponse: the document is not a SAML 2.0 Response".to_string(),
            ),
        ),
        (
            "a well-formed response nothing signed",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                signed: false,
                ..valid()
            }),
            Expected::Refused(400, "InvalidIdentityToken", NOT_SIGNED_MESSAGE.to_string()),
        ),
        (
            // Forged: correctly signed, by a key the named provider does not publish.
            "a response signed by another provider's key",
            role_arn.clone(),
            provider_arn.clone(),
            saml_assertion(&other, &Response::valid(&other, &role_arn, &provider_arn)),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                SIGNATURE_INVALID_MESSAGE.to_string(),
            ),
        ),
        (
            "a signed response altered after it was signed",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                altered_after_signing: true,
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                SIGNATURE_INVALID_MESSAGE.to_string(),
            ),
        ),
        (
            // A transform that selects the signed content by an expression of its own, which
            // is how a signature is made to cover less than it appears to.
            "a reference naming an XPath transform",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                transforms: vec![ENVELOPED_SIGNATURE, XPATH_TRANSFORM],
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                SIGNATURE_INVALID_MESSAGE.to_string(),
            ),
        ),
        (
            // The window the subject's confirmation data states.
            "a correctly signed response whose subject confirmation window has passed",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                issued_at_offset: -30,
                not_before_offset: -7200,
                not_on_or_after_offset: Some(-3600),
                conditions_not_on_or_after_offset: Some(600),
                ..valid()
            }),
            Expected::Refused(
                400,
                "ExpiredTokenException",
                RESPONSE_EXPIRED_MESSAGE.to_string(),
            ),
        ),
        (
            "a correctly signed response whose window closes this very second",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                not_on_or_after_offset: Some(0),
                conditions_not_on_or_after_offset: Some(600),
                ..valid()
            }),
            Expected::Refused(
                400,
                "ExpiredTokenException",
                RESPONSE_EXPIRED_MESSAGE.to_string(),
            ),
        ),
        (
            // The assertion's OWN conditions, which are the other window the same rule
            // covers: whichever of the two has passed, the response has expired.
            "a correctly signed response whose assertion conditions have passed",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                conditions_not_on_or_after_offset: Some(-60),
                ..valid()
            }),
            Expected::Refused(
                400,
                "ExpiredTokenException",
                RESPONSE_EXPIRED_MESSAGE.to_string(),
            ),
        ),
        (
            "a correctly signed response whose window has not begun",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                not_before_offset: 600,
                not_on_or_after_offset: Some(1200),
                ..valid()
            }),
            Expected::Refused(
                400,
                "ExpiredTokenException",
                RESPONSE_NOT_YET_VALID_MESSAGE.to_string(),
            ),
        ),
        (
            // The redemption window: a response inside its own validity window but issued
            // longer ago than it may be redeemed in.
            "a correctly signed response issued longer ago than it may be redeemed",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                issued_at_offset: -3600,
                ..valid()
            }),
            Expected::Refused(
                400,
                "ExpiredTokenException",
                NOT_REDEEMABLE_MESSAGE.to_string(),
            ),
        ),
        (
            "a response whose provider reported that authentication failed",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                status: STATUS_AUTHN_FAILED,
                ..valid()
            }),
            Expected::Refused(
                403,
                "IDPRejectedClaim",
                IDP_REJECTED_CLAIM_MESSAGE.to_string(),
            ),
        ),
        (
            "a response asserting only another role",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                asserted: vec![(
                    format!("arn:aws:iam::{}:role/other", emulator_account()),
                    provider_arn.clone(),
                )],
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                ROLE_NOT_ASSERTED_MESSAGE.to_string(),
            ),
        ),
        (
            "a response asserting no role at all",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                asserted: Vec::new(),
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                ROLE_NOT_ASSERTED_MESSAGE.to_string(),
            ),
        ),
        (
            "a correctly signed response from another issuer",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                issuer: "https://impostor.example.test/saml".to_string(),
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                ISSUER_NOT_IN_PROVIDER_MESSAGE.to_string(),
            ),
        ),
        (
            "a response naming no session at all",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                name_id: String::new(),
                names_session: false,
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                SESSION_NAME_REQUIRED_MESSAGE.to_string(),
            ),
        ),
        (
            "a response naming a session outside the character class",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                name_id: "not a session name".to_string(),
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                SESSION_NAME_PATTERN_MESSAGE.to_string(),
            ),
        ),
        (
            "a response naming a session of one character",
            role_arn.clone(),
            provider_arn.clone(),
            signed(Response {
                name_id: "a".to_string(),
                ..valid()
            }),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                SESSION_NAME_PATTERN_MESSAGE.to_string(),
            ),
        ),
        (
            // The provider the request names is one the account does not hold, so there is
            // no metadata to verify anything against.
            "a provider ARN the account does not hold",
            role_arn.clone(),
            iam_provider_arn("never-registered"),
            signed(valid()),
            Expected::Refused(
                400,
                "InvalidIdentityToken",
                PROVIDER_NOT_FOUND_MESSAGE.to_string(),
            ),
        ),
        (
            "a verified response against a role whose trust policy admits another provider",
            unadmitted_role.clone(),
            provider_arn.clone(),
            saml_assertion(
                &provider,
                &Response {
                    asserted: vec![(unadmitted_role.clone(), provider_arn.clone())],
                    ..valid()
                },
            ),
            Expected::Refused(403, "AccessDenied", NOT_AUTHORIZED_MESSAGE.to_string()),
        ),
        (
            "a verified response against a role the account does not hold",
            format!("arn:aws:iam::{}:role/no-such-role", emulator_account()),
            provider_arn.clone(),
            saml_assertion(
                &provider,
                &Response {
                    asserted: vec![(
                        format!("arn:aws:iam::{}:role/no-such-role", emulator_account()),
                        provider_arn.clone(),
                    )],
                    ..valid()
                },
            ),
            Expected::Refused(403, "AccessDenied", NOT_AUTHORIZED_MESSAGE.to_string()),
        ),
    ];

    for (case, role, principal, assertion, expected) in rows {
        let (status, body) = exchange(&router, &role, &principal, &assertion).await;
        match expected {
            Expected::Federated => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the exchange to be served, got {status} {body}"
                );
                let assumed_role_arn = format!(
                    "arn:aws:sts::{}:assumed-role/{role_name}/{SESSION_NAME}",
                    emulator_account()
                );
                assert_eq!(
                    element(&body, "Arn").as_deref(),
                    Some(assumed_role_arn.as_str()),
                    "case {case:?}: expected the assumed-role ARN {assumed_role_arn}, got {body}"
                );
                for (name, expected_value) in [
                    ("Subject", SESSION_NAME),
                    ("SubjectType", "transient"),
                    ("Issuer", provider.entity_id.as_str()),
                    ("Audience", AWS_SAML_ENDPOINT),
                ] {
                    let reported = member(case, &body, name).replace("&amp;", "&");
                    assert_eq!(
                        reported, expected_value,
                        "case {case:?}: expected {name} {expected_value:?}, got {reported:?}"
                    );
                }
                assert!(
                    !member(case, &body, "NameQualifier").is_empty(),
                    "case {case:?}: expected a NameQualifier hash, got {body}"
                );
                // The session is REAL: the credential it vended resolves to the same
                // assumed-role identity when it is read back.
                let access_key_id = member(case, &body, "AccessKeyId");
                let (status, read) = query(
                    &router,
                    &access_key_id,
                    STS_SIGNING_NAME,
                    STS_API_VERSION,
                    "GetCallerIdentity",
                    &[],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the vended credential must resolve, got {status} {read}"
                );
                assert_eq!(
                    element(&read, "Arn").as_deref(),
                    Some(assumed_role_arn.as_str()),
                    "case {case:?}: expected the session to report {assumed_role_arn}, got {read}"
                );
            }
            Expected::Refused(expected_status, code, message) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
                );
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some(code),
                    "case {case:?}: expected <Code>{code}</Code>, got {body}"
                );
                let reported = member(case, &body, "Message").replace("&amp;", "&");
                assert_eq!(
                    reported, message,
                    "case {case:?}: expected the message {message:?}, got {reported:?}"
                );
                assert!(
                    element(&body, "Credentials").is_none(),
                    "case {case:?}: a refused exchange must vend no credential, got {body}"
                );
                assert!(
                    body.contains("<ErrorResponse"),
                    "case {case:?}: expected an awsQuery XML ErrorResponse, got {body}"
                );
            }
        }
    }
}

/// A response the provider signed is federated while the provider is registered, and
/// refused once the account DELETES that provider: the verification reads IAM's own records,
/// so removing the provider removes the only thing its responses could be checked against.
#[tokio::test]
async fn deleting_the_saml_provider_blocks_the_responses_it_signed() {
    let (router, _engine) = emulator_with_iam();
    let provider = IdentityProvider::mint("https://idp.example.test/saml");
    let provider_arn = create_saml_provider(&router, "deletable-idp", &provider).await;
    let role_arn =
        create_saml_role(&router, "saml-deletable", &provider_arn, audience_condition()).await;

    let assertion = |case: &str| {
        let _ = case;
        saml_assertion(&provider, &Response::valid(&provider, &role_arn, &provider_arn))
    };

    let (status, body) = exchange(&router, &role_arn, &provider_arn, &assertion("before")).await;
    assert_eq!(
        status, 200,
        "case \"before the provider is deleted\": expected the exchange to be served, got \
         {status} {body}"
    );

    iam_call(
        &router,
        "DeleteSAMLProvider",
        &[("SAMLProviderArn", provider_arn.clone())],
    )
    .await;

    // Twice: the refusal is derived from the account's records and the response's own bytes,
    // so it is stable and nothing about it is consumed by being read.
    for attempt in ["first attempt after the delete", "repeated attempt"] {
        let (status, body) = exchange(&router, &role_arn, &provider_arn, &assertion(attempt)).await;
        assert_eq!(
            status, 400,
            "case {attempt:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidIdentityToken"),
            "case {attempt:?}: expected the provider to be unresolvable, got {body}"
        );
        assert_eq!(
            member(attempt, &body, "Message"),
            PROVIDER_NOT_FOUND_MESSAGE,
            "case {attempt:?}: expected {PROVIDER_NOT_FOUND_MESSAGE:?}, got {body}"
        );
        assert!(
            element(&body, "Credentials").is_none(),
            "case {attempt:?}: a refused exchange must vend no credential, got {body}"
        );
    }
}

/// The session a verified response vends is no longer than the response's own
/// `SessionNotOnOrAfter`: "your role session lasts for the duration that you specify, or
/// until the time specified in the SAML authentication response's SessionNotOnOrAfter value,
/// whichever is shorter" (this operation's model).
#[tokio::test]
async fn the_session_is_no_longer_than_the_response_allows() {
    let (router, _engine) = emulator_with_iam();
    let provider = IdentityProvider::mint("https://idp.example.test/saml");
    let provider_arn = create_saml_provider(&router, "bounded-idp", &provider).await;
    let role_arn =
        create_saml_role(&router, "saml-bounded", &provider_arn, audience_condition()).await;

    // One row: the case, the instant the response bounds the session at (seconds from now),
    // and the longest session it may therefore vend — `None` is the response that bounds
    // nothing, which gets this operation's own default hour.
    let rows: Vec<(&str, Option<i64>, i64)> = vec![
        ("a response bounding the session in ten minutes", Some(600), 600),
        ("a response bounding it in half an hour", Some(1800), 1800),
        ("a response bounding nothing at all", None, 3600),
    ];

    for (case, bound, longest) in rows {
        let assertion = saml_assertion(
            &provider,
            &Response {
                session_not_on_or_after_offset: bound,
                ..Response::valid(&provider, &role_arn, &provider_arn)
            },
        );
        let (status, body) = exchange(&router, &role_arn, &provider_arn, &assertion).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the exchange to be served, got {status} {body}"
        );
        let expiration = member(case, &body, "Expiration");
        let expires_at = super::sts_sessions::timestamp_secs(case, &expiration);
        let now = super::sts_sessions::now_secs();
        assert!(
            expires_at <= now + longest,
            "case {case:?}: expected a session no longer than {longest}s, got {expires_at} \
             at {now}: {body}"
        );
        assert!(
            expires_at > now,
            "case {case:?}: expected a session that has not already expired, got \
             {expires_at} at {now}: {body}"
        );
    }
}

/// The ARN IAM keys a SAML provider of this account by.
fn iam_provider_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:saml-provider/{name}", emulator_account())
}
