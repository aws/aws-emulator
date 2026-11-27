// SPDX-License-Identifier: CC0-1.0

//! FIELD-LEVEL ENCRYPTION on the REQUEST path: what an origin behind a cache behaviour that
//! names a field-level encryption configuration actually receives — end to end over HTTP across
//! the services on one engine.
//!
//! The topology is [`crate::edge_lambda_url_origin`]'s, reused rather than copied: a running
//! Lambda function whose function URL is the origin's `DomainName`, a distribution in front of
//! it, and a viewer that posts a form to the `DomainName` the control plane published with no
//! AWS credential at all. The origin here ECHOES the body it was handed, so what reached the
//! origin is read off the origin's own answer rather than inferred, and the KEY PAIR is the
//! CLIENT's: the private half never leaves this suite, so a field the edge encrypted is proved
//! encrypted by decrypting it here.
//!
//! The rules the rows span:
//!
//! * a behaviour naming a field-level encryption configuration forwards the viewer's body with
//!   the value of EVERY field the selected profile's `FieldPatterns` designate replaced by the
//!   base64 of that value encrypted under the designating entity's public key, and every other
//!   field — and every field name — exactly as the viewer sent it. A pattern is "the full field
//!   name, or any beginning characters followed by a wildcard (*)", case-sensitive, so a field
//!   whose name merely resembles a designated one travels in the clear;
//! * `QueryArgProfileConfig` maps the profile the viewer NAMES in the `fle-profile` query
//!   argument, and `ContentTypeProfileConfig` holds the "profiles to use as by default in a
//!   request if a query argument doesn't specify a profile to use" — so the profile the request
//!   names takes precedence, and which fields are encrypted and under which key follow from the
//!   profile that was selected;
//! * a request NO profile is selected for is decided by the flag of the mapping that did not
//!   resolve: `ForwardWhenContentTypeIsUnknown` / `ForwardWhenQueryArgProfileIsUnknown` true
//!   forwards the body as the viewer sent it, and false means "an error is returned" — `400`
//!   from CloudFront itself, with `X-Cache: Error from cloudfront` and no origin request, so the
//!   origin never sees the fields at all. Each flag is read as the control plane STORED it,
//!   which for a query arg-profile mapping is whether that mapping maps NO query argument to a
//!   profile (`field_level_encryption_query_arg_forwarding.rs` states that rule).
//!
//! The rows differ along what those rules generalise over: the form of the pattern that
//! designates a field (the whole name, a prefix wildcard, a name no pattern covers, a name that
//! shares a designated prefix but is not covered, a name differing only in case), the value
//! being encrypted (plain text, characters the body's own format encodes, the empty value,
//! non-ASCII text, a value exactly as long as the key encrypts and one byte past it), how many
//! fields of one body are designated (one, two, none), which mapping selected the profile (the
//! content type, the query argument the viewer named), the profile selected (two profiles over
//! two different key pairs and different patterns), whether the mapping that did not resolve
//! forwards or refuses, and how the viewer reached the distribution (dispatched at the name it
//! published, and dialled at that published address with a stock TLS client).

use std::collections::BTreeMap;

use crate::distribution_lifecycle::API_VERSION;
use crate::edge_lambda_url_origin::{dial_over_tls, element, Topology};
use crate::edge_signed_access::CACHING_DISABLED;
use crate::field_level_encryption_profile_lifecycle::{Entity, Profile};

/// The handler behind the function URL: it reports the BODY it was handed, whether that body
/// was handed over base64-encoded, and the two headers that describe it — so the viewer's answer
/// says exactly what reached the origin. It imports only the standard library, so the report is
/// the handler's own work.
const FORM_ECHO_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    headers = event.get("headers") or {}
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json"},
        "body": json.dumps(
            {
                "marker": "FLE-ECHO-OK",
                "body": event.get("body", ""),
                "base64": bool(event.get("isBase64Encoded")),
                "content_type": headers.get("content-type", ""),
                "content_length": headers.get("content-length", ""),
            }
        ),
    }
"#;

/// The marker [`FORM_ECHO_SOURCE`] puts in every answer of its own, so an answer that carries it
/// came from the origin.
const ECHO_MARKER: &str = "FLE-ECHO-OK";

/// The media type a field-level encryption content type-profile mapping is written for, and the
/// one `ContentTypeProfile$Format` `URLEncoded` reads a body in.
const FORM_MEDIA_TYPE: &str = "application/x-www-form-urlencoded";

/// A media type the configurations below map NO profile for, which is what makes a request under
/// it the unknown content type the forwarding flag decides.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The one `Format` the model's `Format` enum admits
/// (`smithy_query(query="shape",shape="Format")` lists `URLEncoded` and nothing else).
const URL_ENCODED: &str = "URLEncoded";

/// The query argument a viewer names the profile to encrypt its fields under in, as
/// `ForwardWhenQueryArgProfileIsUnknown` states it: "the profile specified by the field-level
/// encryption query argument, `fle-profile`".
const PROFILE_QUERY_ARG: &str = "fle-profile";

/// `X-Cache` for an answer the origin produced for this request, and for one CloudFront
/// produced itself.
const MISS: &str = "Miss from cloudfront";
const ERROR: &str = "Error from cloudfront";

/// How many octets the modulus of a 2048-bit RSA key is, which is how long every ciphertext one
/// of its encryptions produces is — so the base64 the origin receives for a designated field is
/// `ceil(256 / 3) * 4` = 344 characters, far longer than any field value below.
const MODULUS_OCTETS: usize = 256;

/// The longest plaintext RSA-OAEP with SHA-256 encrypts under a 2048-bit key, from the bound
/// RFC 8017 section 7.1.1 states for the scheme: `k - 2 * hLen - 2`, with `k` the modulus
/// octets ([`MODULUS_OCTETS`]) and `hLen` the 32 octets of a SHA-256 digest.
const MAX_ENCRYPTED_PLAINTEXT: usize = MODULUS_OCTETS - 2 * 32 - 2;

// ---------------------------------------------------------------------------
// The client's own key pair
// ---------------------------------------------------------------------------

/// One RSA key pair, held the way the client that configures field-level encryption holds one:
/// the public half goes to CloudFront, and the private half — the only thing that can read a
/// field the edge encrypted — stays here.
struct ClientKeyPair {
    /// The public half as the PEM `SubjectPublicKeyInfo` `CreatePublicKey` takes.
    public_key_pem: String,
    /// The private half as PKCS#8, which this suite decrypts a field with.
    pkcs8: Vec<u8>,
}

impl ClientKeyPair {
    /// A fresh 2048-bit RSA pair, the width a CloudFront public key carries.
    fn generate() -> Self {
        use ::aws_lc_rs::encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der};
        use ::aws_lc_rs::signature::KeyPair as _;

        let key = ::aws_lc_rs::signature::RsaKeyPair::generate(::aws_lc_rs::rsa::KeySize::Rsa2048)
            .expect("the provider generates a 2048-bit RSA pair");
        let public: PublicKeyX509Der<'_> =
            AsDer::as_der(key.public_key()).expect("the public half encodes as an SPKI");
        let private: Pkcs8V1Der<'_> =
            AsDer::as_der(&key).expect("the private half encodes as a PKCS#8");
        Self {
            public_key_pem: ::pem::encode(&::pem::Pem::new(
                "PUBLIC KEY".to_string(),
                public.as_ref().to_vec(),
            )),
            pkcs8: private.as_ref().to_vec(),
        }
    }

    /// The plaintext `ciphertext` encrypts under this pair's private half, read as the client's
    /// origin reads a field the edge encrypted: RSA-OAEP with SHA-256 as the digest and MGF1
    /// hash.
    fn decrypt(&self, case: &str, ciphertext: &[u8]) -> Vec<u8> {
        use ::aws_lc_rs::rsa::{
            OaepPrivateDecryptingKey, PrivateDecryptingKey, OAEP_SHA256_MGF1SHA256,
        };

        let key = PrivateDecryptingKey::from_pkcs8(&self.pkcs8)
            .expect("the generated private half is a PKCS#8 RSA key");
        let key =
            OaepPrivateDecryptingKey::new(key).expect("an RSA private key decrypts under OAEP");
        let mut plaintext = vec![0u8; ciphertext.len()];
        let read = key
            .decrypt(&OAEP_SHA256_MGF1SHA256, ciphertext, &mut plaintext, None)
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the field the edge encrypted does not decrypt under the \
                     profile's own key pair ({error})"
                )
            });
        read.to_vec()
    }
}

// ---------------------------------------------------------------------------
// Staging the control plane
// ---------------------------------------------------------------------------

/// Upload `client`'s public half through `CreatePublicKey` and report the id it minted.
async fn created_public_key(topology: &Topology, case: &str, tag: &str, client: &ClientKeyPair) -> String {
    let body = format!(
        "<PublicKeyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{tag}</CallerReference><Name>{tag}</Name>\
         <EncodedKey>{}</EncodedKey></PublicKeyConfig>",
        client.public_key_pem
    );
    let (status, raw) = topology
        .cloudfront(case, "POST", &format!("/{API_VERSION}/public-key"), Some(&body))
        .await;
    assert_eq!(
        status, 201,
        "setup {case}: CreatePublicKey must be accepted, got {status} with {raw}"
    );
    let id = element(&raw, "Id");
    assert!(
        !id.is_empty(),
        "setup {case}: the created public key reports no id: {raw}"
    );
    id
}

/// Create the field-level encryption PROFILE designating `patterns` against `public_key_id`, and
/// report the id it minted.
///
/// The document is the profile suite's own builder ([`Profile`]), so there is one spelling of a
/// `FieldLevelEncryptionProfileConfig` in this crate's suites.
async fn created_profile(
    topology: &Topology,
    case: &str,
    tag: &str,
    public_key_id: &str,
    patterns: &[&str],
) -> String {
    let profile = Profile::new(
        case,
        tag,
        None,
        vec![Entity::new(public_key_id, "ClientProvider", patterns)],
    );
    let (status, raw) = topology
        .cloudfront(
            case,
            "POST",
            &format!("/{API_VERSION}/field-level-encryption-profile"),
            Some(&profile.xml()),
        )
        .await;
    assert_eq!(
        status, 201,
        "setup {case}: CreateFieldLevelEncryptionProfile must be accepted, got {status} with {raw}"
    );
    let id = element(&raw, "Id");
    assert!(
        !id.is_empty(),
        "setup {case}: the created profile reports no id: {raw}"
    );
    id
}

/// One `FieldLevelEncryptionConfig` a test stages: which content types and which `fle-profile`
/// values map to which profile, and what each mapping does with a request it cannot resolve.
struct FleConfig<'a> {
    reference: &'a str,
    /// `(ContentType, ProfileId)` for each content type-profile mapping.
    content_types: &'a [(&'a str, &'a str)],
    forward_unknown_content_type: bool,
    /// `(QueryArg, ProfileId)` for each query arg-profile mapping, and the flag of that
    /// section, or `None` for a configuration carrying no query arg-profile mapping at all.
    query_args: Option<(&'a [(&'a str, &'a str)], bool)>,
}

impl FleConfig<'_> {
    /// The `FieldLevelEncryptionConfig` XML a client sends, in the model's member declaration
    /// order (`CallerReference`, `Comment`, `QueryArgProfileConfig`,
    /// `ContentTypeProfileConfig`).
    fn xml(&self) -> String {
        let mut out = format!(
            "<FieldLevelEncryptionConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>",
            self.reference
        );
        if let Some((mappings, forward_unknown)) = self.query_args {
            out.push_str(&format!(
                "<QueryArgProfileConfig>\
                 <ForwardWhenQueryArgProfileIsUnknown>{forward_unknown}\
                 </ForwardWhenQueryArgProfileIsUnknown>\
                 <QueryArgProfiles><Quantity>{}</Quantity><Items>",
                mappings.len()
            ));
            for (query_arg, profile_id) in mappings {
                out.push_str(&format!(
                    "<QueryArgProfile><QueryArg>{query_arg}</QueryArg>\
                     <ProfileId>{profile_id}</ProfileId></QueryArgProfile>"
                ));
            }
            out.push_str("</Items></QueryArgProfiles></QueryArgProfileConfig>");
        }
        out.push_str(&format!(
            "<ContentTypeProfileConfig>\
             <ForwardWhenContentTypeIsUnknown>{}</ForwardWhenContentTypeIsUnknown>\
             <ContentTypeProfiles><Quantity>{}</Quantity><Items>",
            self.forward_unknown_content_type,
            self.content_types.len()
        ));
        for (content_type, profile_id) in self.content_types {
            out.push_str(&format!(
                "<ContentTypeProfile><Format>{URL_ENCODED}</Format>\
                 <ProfileId>{profile_id}</ProfileId>\
                 <ContentType>{content_type}</ContentType></ContentTypeProfile>"
            ));
        }
        out.push_str(
            "</Items></ContentTypeProfiles></ContentTypeProfileConfig>\
             </FieldLevelEncryptionConfig>",
        );
        out
    }
}

/// Create the field-level encryption CONFIGURATION `config` describes and report the id it
/// minted.
async fn created_config(topology: &Topology, case: &str, config: &FleConfig<'_>) -> String {
    let (status, raw) = topology
        .cloudfront(
            case,
            "POST",
            &format!("/{API_VERSION}/field-level-encryption"),
            Some(&config.xml()),
        )
        .await;
    assert_eq!(
        status, 201,
        "setup {case}: CreateFieldLevelEncryptionConfig must be accepted, got {status} with {raw}"
    );
    let id = element(&raw, "Id");
    assert!(
        !id.is_empty(),
        "setup {case}: the created configuration reports no id: {raw}"
    );
    id
}

/// The `AllowedMethods` every behaviour below carries: all seven methods, so a viewer POST is a
/// request the behaviour serves at all, and only `GET`/`HEAD` are cached — a POST therefore
/// reaches the origin every time.
const EVERY_METHOD: &str = "<AllowedMethods><Quantity>7</Quantity><Items>\
     <Method>GET</Method><Method>HEAD</Method><Method>OPTIONS</Method><Method>PUT</Method>\
     <Method>PATCH</Method><Method>POST</Method><Method>DELETE</Method></Items>\
     <CachedMethods><Quantity>2</Quantity>\
     <Items><Method>GET</Method><Method>HEAD</Method></Items></CachedMethods>\
     </AllowedMethods>";

/// One `DistributionConfig` over the function URL `origin` whose DEFAULT behaviour names the
/// field-level encryption configuration `default_config`, plus one ordered behaviour per
/// `(PathPattern, FieldLevelEncryptionId)` of `ordered`.
fn distribution_xml(
    caller_reference: &str,
    origin: &str,
    default_config: &str,
    ordered: &[(&str, &str)],
) -> String {
    let behaviors = if ordered.is_empty() {
        String::new()
    } else {
        let items: String = ordered
            .iter()
            .map(|(pattern, config)| {
                format!(
                    "<CacheBehavior><PathPattern>{pattern}</PathPattern>\
                     <TargetOriginId>function-url</TargetOriginId>\
                     <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{EVERY_METHOD}\
                     <FieldLevelEncryptionId>{config}</FieldLevelEncryptionId>\
                     <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
                     </CacheBehavior>"
                )
            })
            .collect();
        format!(
            "<CacheBehaviors><Quantity>{}</Quantity><Items>{items}</Items></CacheBehaviors>",
            ordered.len()
        )
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>function-url</Id><DomainName>{origin}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         <OriginSslProtocols><Quantity>1</Quantity>\
         <Items><SslProtocol>TLSv1.2</SslProtocol></Items></OriginSslProtocols>\
         </CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>function-url</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{EVERY_METHOD}\
         <FieldLevelEncryptionId>{default_config}</FieldLevelEncryptionId>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>{behaviors}\
         <Comment>a form post behind field-level encryption</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

// ---------------------------------------------------------------------------
// Posting a form through the edge, and reading what the origin received
// ---------------------------------------------------------------------------

/// What the edge answered one viewer POST with.
struct Posted {
    status: u16,
    x_cache: String,
    body: String,
}

/// POST `body` under `content_type` to `path_and_query` at the name the distribution published,
/// carrying no credential — which is what a viewer submitting a form is.
async fn post_form(
    topology: &Topology,
    domain: &str,
    path_and_query: &str,
    content_type: &str,
    body: &str,
) -> Posted {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(path_and_query)
        .header("host", domain)
        .header("content-type", content_type)
        .header("content-length", body.len().to_string())
        .body(::bytes::Bytes::from(body.as_bytes().to_vec()))
        .expect("the viewer request is well formed");
    let response = topology.router.dispatch(request).await;
    let status = response.status().as_u16();
    let x_cache = response
        .headers()
        .get("x-cache")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Posted {
        status,
        x_cache,
        body: String::from_utf8_lossy(&response.into_body()).to_string(),
    }
}

/// The BODY the origin received, out of the origin's own report of it (`answered`, the bytes
/// the edge served the viewer).
fn received_body(answered: &str, case: &str) -> String {
    assert!(
        answered.contains(ECHO_MARKER),
        "case {case:?}: expected the origin's own report of the body it received, got {answered:?}"
    );
    let report: ::serde_json::Value = ::serde_json::from_str(answered)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the origin's report is no document ({error}): {answered:?}")
        });
    let body = report
        .get("body")
        .and_then(::serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let base64 = report
        .get("base64")
        .and_then(::serde_json::Value::as_bool)
        .unwrap_or_default();
    let bytes = if base64 {
        // A form body is handed to a function URL base64-encoded, the format carrying only
        // text media types as themselves.
        use ::base64::Engine as _;
        ::base64::engine::general_purpose::STANDARD
            .decode(&body)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the origin's report is not base64 ({error}): {body:?}")
            })
    } else {
        body.into_bytes()
    };
    let received = String::from_utf8_lossy(&bytes).to_string();
    // The origin is told how many bytes of body travelled on its own hop, which for a body the
    // edge rewrote is that body's length and not the viewer's.
    let stated = report
        .get("content_length")
        .and_then(::serde_json::Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        stated,
        received.len().to_string(),
        "case {case:?}: expected the origin to be told the length of the body it received \
         ({}), got {stated:?}",
        received.len()
    );
    received
}

/// The fields of a `application/x-www-form-urlencoded` body, by name.
fn fields(body: &str) -> BTreeMap<String, String> {
    ::form_urlencoded::parse(body.as_bytes())
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// The `application/x-www-form-urlencoded` body a viewer sends for `pairs`.
fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut serializer = ::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

/// Assert that the field `name` reached the origin as the base64 of `plaintext` encrypted under
/// `client`'s key pair, and that nothing of the plaintext travelled with it.
fn assert_encrypted(
    received: &BTreeMap<String, String>,
    raw: &str,
    case: &str,
    name: &str,
    plaintext: &str,
    client: &ClientKeyPair,
) {
    use ::base64::Engine as _;

    let actual = received.get(name).unwrap_or_else(|| {
        panic!("case {case:?}: the origin received no field {name:?}, only {received:?}")
    });
    assert_ne!(
        actual.as_str(), plaintext,
        "case {case:?}: the designated field {name:?} reached the origin as the plaintext the \
         viewer sent"
    );
    let ciphertext = ::base64::engine::general_purpose::STANDARD
        .decode(actual)
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the designated field {name:?} is no base64 value ({error}), got \
                 {actual:?}"
            )
        });
    assert_eq!(
        ciphertext.len(),
        MODULUS_OCTETS,
        "case {case:?}: expected a ciphertext as long as the key's modulus for {name:?}, got {} \
         octets",
        ciphertext.len()
    );
    let decrypted = client.decrypt(case, &ciphertext);
    assert_eq!(
        String::from_utf8_lossy(&decrypted),
        plaintext,
        "case {case:?}: the designated field {name:?} must decrypt to the value the viewer sent, \
         got {:?}",
        String::from_utf8_lossy(&decrypted)
    );
    if !plaintext.is_empty() {
        assert!(
            !raw.contains(plaintext),
            "case {case:?}: the body the origin received carries the plaintext of {name:?}: \
             {raw:?}"
        );
    }
}

/// Assert that the field `name` reached the origin exactly as the viewer sent it.
fn assert_verbatim(received: &BTreeMap<String, String>, case: &str, name: &str, value: &str) {
    let actual = received.get(name).unwrap_or_else(|| {
        panic!("case {case:?}: the origin received no field {name:?}, only {received:?}")
    });
    assert_eq!(
        actual.as_str(), value,
        "case {case:?}: expected the field {name:?} to reach the origin as the viewer sent it \
         ({value:?}), got {actual:?}"
    );
}

/// Assert that CloudFront refused this request itself: its own `400`, reported as an answer no
/// origin produced, and carrying nothing of the body the viewer submitted.
fn assert_refused(answered: &Posted, case: &str, plaintext: &str) {
    assert_eq!(
        (answered.status, answered.x_cache.as_str()),
        (400, ERROR),
        "case {case:?}: expected CloudFront's own 400 with X-Cache {ERROR:?}, got {} with {:?} \
         (body {:?})",
        answered.status,
        answered.x_cache,
        answered.body
    );
    assert!(
        !answered.body.contains(ECHO_MARKER),
        "case {case:?}: a request CloudFront refuses must make no origin request, got {:?}",
        answered.body
    );
    assert!(
        !answered.body.contains(plaintext),
        "case {case:?}: the refusal carries the field the viewer submitted: {:?}",
        answered.body
    );
}

// ---------------------------------------------------------------------------
// Which fields of one body are encrypted
// ---------------------------------------------------------------------------

/// The patterns the profile under test designates: one whole field name, and one "beginning
/// characters followed by a wildcard" — the two forms `EncryptionEntity$FieldPatterns`
/// documents.
const PATTERNS: [&str; 2] = ["secret_field", "card*"];

/// One field of one posted body: its name, the value the viewer sent, and whether [`PATTERNS`]
/// designates it.
struct FieldRow {
    name: &'static str,
    value: &'static str,
    designated: bool,
}

/// One posted body, and what each of its fields is a case of.
struct BodyRow {
    case: &'static str,
    fields: &'static [FieldRow],
}

/// The table of bodies: the rows differ along the form of the name that is (or is not)
/// designated, the form of the value, and how many fields of the body are designated.
const BODIES: &[BodyRow] = &[
    BodyRow {
        case: "a designated field beside one no pattern names",
        fields: &[
            FieldRow {
                name: "secret_field",
                value: "4111111111111111",
                designated: true,
            },
            FieldRow {
                name: "other_field",
                value: "not-a-secret",
                designated: false,
            },
        ],
    },
    BodyRow {
        case: "a prefix pattern designating two fields, and a name that only resembles them",
        fields: &[
            FieldRow {
                name: "card_number",
                value: "5500005555555559",
                designated: true,
            },
            FieldRow {
                name: "cardholder",
                value: "Ada Lovelace",
                designated: true,
            },
            FieldRow {
                name: "car_make",
                value: "a car is not a card",
                designated: false,
            },
        ],
    },
    BodyRow {
        case: "a value carrying the characters the body's own format encodes",
        fields: &[
            FieldRow {
                name: "secret_field",
                value: "pass word&more=yes+no/100%",
                designated: true,
            },
            FieldRow {
                name: "other_field",
                value: "plain value&kept=as sent",
                designated: false,
            },
        ],
    },
    BodyRow {
        case: "a designated field whose value is empty",
        fields: &[
            FieldRow {
                name: "secret_field",
                value: "",
                designated: true,
            },
            FieldRow {
                name: "other_field",
                value: "",
                designated: false,
            },
        ],
    },
    BodyRow {
        case: "a value that is not ASCII",
        fields: &[
            FieldRow {
                name: "secret_field",
                value: "pässwörd-日本語-☂",
                designated: true,
            },
            FieldRow {
                name: "other_field",
                value: "öffentlich",
                designated: false,
            },
        ],
    },
    BodyRow {
        case: "a name differing from a designated one only in case, the patterns being \
               case-sensitive",
        fields: &[
            FieldRow {
                name: "SECRET_FIELD",
                value: "upper-case name",
                designated: false,
            },
            FieldRow {
                name: "Card_Number",
                value: "capitalised name",
                designated: false,
            },
        ],
    },
    BodyRow {
        case: "a body no pattern designates a field of",
        fields: &[
            FieldRow {
                name: "other_field",
                value: "one",
                designated: false,
            },
            FieldRow {
                name: "another_field",
                value: "two",
                designated: false,
            },
        ],
    },
];

/// RULE: a behaviour naming a field-level encryption configuration forwards the viewer's body
/// with every field the selected profile's patterns designate replaced by the base64 of that
/// value encrypted under the profile's public key, and every other field as the viewer sent it.
///
/// The rows are [`BODIES`], and the bound on one encryption is driven after them: a value as long
/// as the key encrypts is encrypted, and one byte past it is a request CloudFront does not
/// forward at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_designated_fields_of_a_posted_form_reach_the_origin_as_ciphertext() {
    let topology = Topology::new();
    let case = "a form POST through a behaviour naming a field-level encryption configuration";
    let function = "cf-edge-fle-echo";
    let origin = topology
        .public_function_url_host_running(case, function, FORM_ECHO_SOURCE)
        .await;
    let client = ClientKeyPair::generate();
    let public_key = created_public_key(&topology, case, "fle-request-path", &client).await;
    let profile = created_profile(&topology, case, "fle-request-path", &public_key, &PATTERNS).await;
    let config = created_config(
        &topology,
        case,
        &FleConfig {
            reference: "fle-request-path",
            content_types: &[(FORM_MEDIA_TYPE, &profile)],
            forward_unknown_content_type: true,
            query_args: None,
        },
    )
    .await;
    let (id, _arn, edge) = topology
        .created_distribution(
            case,
            &distribution_xml("fle-request-path", &origin, &config, &[]),
        )
        .await;
    // A configuration that has not reached the edge locations serves no viewer, and the CLOCK
    // owes that propagation: the condition is polled for, never a span waited out.
    topology.awaited_deployment(case, &id).await;
    let domain = edge.domain.clone();
    // Where a viewer DIALS that name: the address this emulator opened for the distribution.
    let address = edge.address.clone().unwrap_or_else(|| {
        panic!("case {case:?}: this distribution was given no address of its own to dial")
    });

    for row in BODIES {
        let pairs: Vec<(&str, &str)> = row
            .fields
            .iter()
            .map(|field| (field.name, field.value))
            .collect();
        let answered = post_form(
            &topology,
            &domain,
            "/post",
            FORM_MEDIA_TYPE,
            &form_body(&pairs),
        )
        .await;
        assert_eq!(
            (answered.status, answered.x_cache.as_str()),
            (200, MISS),
            "case {:?}: expected the origin's 200 with X-Cache {MISS:?}, got {} with {:?} (body \
             {:?})",
            row.case,
            answered.status,
            answered.x_cache,
            answered.body
        );
        let raw = received_body(&answered.body, row.case);
        let received = fields(&raw);
        assert_eq!(
            received.len(),
            row.fields.len(),
            "case {:?}: expected the origin to receive {} fields, got {received:?}",
            row.case,
            row.fields.len()
        );
        for field in row.fields {
            if field.designated {
                assert_encrypted(&received, &raw, row.case, field.name, field.value, &client);
            } else {
                assert_verbatim(&received, row.case, field.name, field.value);
            }
        }
    }

    // The published bound of one RSA-OAEP encryption: a designated value exactly that long is
    // encrypted, and one byte past it cannot be encrypted at all — so the request is refused
    // rather than forwarded with the field in the clear.
    let longest = "p".repeat(MAX_ENCRYPTED_PLAINTEXT);
    let case = "a designated value exactly as long as the key encrypts";
    let answered = post_form(
        &topology,
        &domain,
        "/post",
        FORM_MEDIA_TYPE,
        &form_body(&[("secret_field", &longest)]),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the origin's 200, got {} with {:?}",
        answered.status, answered.body
    );
    let raw = received_body(&answered.body, case);
    assert_encrypted(&fields(&raw), &raw, case, "secret_field", &longest, &client);

    let too_long = "p".repeat(MAX_ENCRYPTED_PLAINTEXT + 1);
    let case = "a designated value one byte longer than the key encrypts";
    let answered = post_form(
        &topology,
        &domain,
        "/post",
        FORM_MEDIA_TYPE,
        &form_body(&[("secret_field", &too_long)]),
    )
    .await;
    assert_refused(&answered, case, &too_long);

    // And the same form, submitted by a viewer that DIALS the published `DomainName` with
    // `https://` the way a client handed that name does: the encryption is the edge's, so what
    // the origin receives on that address is the same ciphertext.
    let case = "a viewer that submits the form at the published address over TLS";
    let plaintext = "4111111111111111";
    let (status, body) = dial_over_tls(
        case,
        &address,
        format!("https://{domain}/post"),
        Some((
            FORM_MEDIA_TYPE,
            form_body(&[("secret_field", plaintext), ("other_field", "not-a-secret")]),
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the published address to serve the origin's answer, got \
         {status} with {body:?}"
    );
    let raw = received_body(&body, case);
    let received = fields(&raw);
    assert_encrypted(&received, &raw, case, "secret_field", plaintext, &client);
    assert_verbatim(&received, case, "other_field", "not-a-secret");
}

// ---------------------------------------------------------------------------
// Which profile encrypts one request's fields, and what an unresolved mapping does
// ---------------------------------------------------------------------------

/// The patterns of the SECOND profile, which designates another field of the same body under
/// another key pair.
const OTHER_PATTERNS: [&str; 1] = ["card*"];

/// The `fle-profile` value the query arg-profile mapping maps to that second profile.
const NAMED_PROFILE: &str = "secure-checkout";

/// The path pattern of the behaviour whose configuration forwards NO request it cannot encrypt.
const STRICT_PATTERN: &str = "/strict/*";

/// The path pattern of the behaviour whose configuration maps NO query argument to a profile
/// and does not forward a request that names one.
const NAMED_STRICT_PATTERN: &str = "/named-strict/*";

/// RULE: the profile a request names in its `fle-profile` query argument takes precedence over
/// the one its content type selects, and a request no profile is selected for is forwarded as
/// the viewer sent it or refused by CloudFront, as the flag of the mapping that did not resolve
/// says.
///
/// The rows differ along which mapping selected the profile (the content type, the query
/// argument), the profile selected (two profiles over two key pairs, designating different
/// fields), whether the value the viewer named resolves at all, the content type of the body
/// (one the configuration maps, one it does not) and the flag of the behaviour's own
/// configuration (both settings, on three behaviours of one distribution).
#[tokio::test]
async fn the_profile_selected_for_a_request_decides_which_of_its_fields_are_encrypted() {
    let topology = Topology::new();
    let case = "two profiles behind one distribution's field-level encryption configurations";
    let function = "cf-edge-fle-profiles";
    let origin = topology
        .public_function_url_host_running(case, function, FORM_ECHO_SOURCE)
        .await;
    let content_type_client = ClientKeyPair::generate();
    let named_client = ClientKeyPair::generate();
    let content_type_key =
        created_public_key(&topology, case, "fle-by-content-type", &content_type_client).await;
    let named_key = created_public_key(&topology, case, "fle-by-query-arg", &named_client).await;
    let content_type_profile = created_profile(
        &topology,
        case,
        "fle-by-content-type",
        &content_type_key,
        &PATTERNS[..1],
    )
    .await;
    let named_profile = created_profile(
        &topology,
        case,
        "fle-by-query-arg",
        &named_key,
        &OTHER_PATTERNS,
    )
    .await;
    // The default behaviour's configuration: the content type selects one profile by default,
    // the query argument the other, and an unknown content type is forwarded. Its mapping maps
    // a query argument to a profile, so the flag it is STORED with refuses a request naming
    // any other profile, and the rows below read that flag as stored.
    let forwarding = created_config(
        &topology,
        case,
        &FleConfig {
            reference: "fle-selection-forwarding",
            content_types: &[(FORM_MEDIA_TYPE, &content_type_profile)],
            forward_unknown_content_type: true,
            query_args: Some((&[(NAMED_PROFILE, &named_profile)], true)),
        },
    )
    .await;
    // The ordered behaviour's: the same content type mapping, and nothing forwarded unencrypted.
    let strict = created_config(
        &topology,
        case,
        &FleConfig {
            reference: "fle-selection-strict",
            content_types: &[(FORM_MEDIA_TYPE, &content_type_profile)],
            forward_unknown_content_type: false,
            query_args: None,
        },
    )
    .await;
    // And a third: it states a query argument section that maps NO query argument to a
    // profile, so it designates nothing by query argument and a request naming one travels on
    // to the origin — which is what separates a request refused for the profile it named from
    // one refused for its content type.
    let named_strict = created_config(
        &topology,
        case,
        &FleConfig {
            reference: "fle-selection-named-strict",
            content_types: &[(FORM_MEDIA_TYPE, &content_type_profile)],
            forward_unknown_content_type: true,
            query_args: Some((&[], false)),
        },
    )
    .await;
    let (id, _arn, edge) = topology
        .created_distribution(
            case,
            &distribution_xml(
                "fle-selection",
                &origin,
                &forwarding,
                &[
                    (STRICT_PATTERN, &strict),
                    (NAMED_STRICT_PATTERN, &named_strict),
                ],
            ),
        )
        .await;
    topology.awaited_deployment(case, &id).await;
    let domain = edge.domain.clone();

    let body = form_body(&[
        ("secret_field", "4111111111111111"),
        ("card_number", "5500005555555559"),
    ]);

    // The content type selects the default profile: it designates `secret_field` and nothing
    // else, so the card number travels as the viewer sent it.
    let case = "a form body under no query argument, so the content type selects the profile";
    let answered = post_form(&topology, &domain, "/post", FORM_MEDIA_TYPE, &body).await;
    assert_eq!(
        (answered.status, answered.x_cache.as_str()),
        (200, MISS),
        "case {case:?}: expected the origin's 200 with X-Cache {MISS:?}, got {} with {:?}",
        answered.status,
        answered.x_cache
    );
    let raw = received_body(&answered.body, case);
    let received = fields(&raw);
    assert_encrypted(
        &received,
        &raw,
        case,
        "secret_field",
        "4111111111111111",
        &content_type_client,
    );
    assert_verbatim(&received, case, "card_number", "5500005555555559");

    // The profile the viewer NAMES takes precedence: it designates `card*` and not
    // `secret_field`, and it encrypts under its own key pair.
    let case = "the same body naming a profile in its fle-profile query argument";
    let answered = post_form(
        &topology,
        &domain,
        &format!("/post?{PROFILE_QUERY_ARG}={NAMED_PROFILE}"),
        FORM_MEDIA_TYPE,
        &body,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the origin's 200, got {} with {:?}",
        answered.status, answered.body
    );
    let raw = received_body(&answered.body, case);
    let received = fields(&raw);
    assert_encrypted(
        &received,
        &raw,
        case,
        "card_number",
        "5500005555555559",
        &named_client,
    );
    assert_verbatim(&received, case, "secret_field", "4111111111111111");

    // A profile the mapping does not carry, on a configuration whose mapping maps a query
    // argument at all: such a request asks for encryption the configuration cannot give, so
    // the flag the configuration is STORED with does not forward it — CloudFront's own
    // refusal, and the origin never sees the body.
    let case = "a query argument naming a profile the mapping does not carry";
    let answered = post_form(
        &topology,
        &domain,
        &format!("/post?{PROFILE_QUERY_ARG}=no-such-profile"),
        FORM_MEDIA_TYPE,
        &body,
    )
    .await;
    assert_refused(&answered, case, "4111111111111111");

    // The same request on the behaviour whose mapping maps NO query argument to a profile: it
    // designates nothing by query argument, so a request naming one is nothing to do with it
    // and the body reaches the origin as the viewer sent it — the content type's default
    // profile does not step in for a request that named one either.
    let case = "a query argument naming a profile on a mapping that maps none";
    let answered = post_form(
        &topology,
        &domain,
        &format!("/named-strict/post?{PROFILE_QUERY_ARG}={NAMED_PROFILE}"),
        FORM_MEDIA_TYPE,
        &body,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the origin's 200, got {} with {:?}",
        answered.status, answered.body
    );
    let raw = received_body(&answered.body, case);
    let received = fields(&raw);
    assert_verbatim(&received, case, "secret_field", "4111111111111111");
    assert_verbatim(&received, case, "card_number", "5500005555555559");

    // And the same behaviour with NO query argument: its content type mapping resolves, so the
    // designated field is encrypted under the profile that mapping names.
    let case = "a form body naming no profile on that same behaviour";
    let answered = post_form(
        &topology,
        &domain,
        "/named-strict/post",
        FORM_MEDIA_TYPE,
        &body,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the origin's 200, got {} with {:?}",
        answered.status, answered.body
    );
    let raw = received_body(&answered.body, case);
    assert_encrypted(
        &fields(&raw),
        &raw,
        case,
        "secret_field",
        "4111111111111111",
        &content_type_client,
    );

    // A content type the configuration maps no profile for, under the flag that forwards it:
    // the body reaches the origin exactly as the viewer sent it.
    let document = "{\"secret_field\":\"4111111111111111\"}";
    let case = "a body under a content type the configuration maps no profile for, forwarded";
    let answered = post_form(&topology, &domain, "/post", JSON_MEDIA_TYPE, document).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the origin's 200, got {} with {:?}",
        answered.status, answered.body
    );
    let raw = received_body(&answered.body, case);
    assert_eq!(
        raw, document,
        "case {case:?}: expected the body to reach the origin as the viewer sent it, got {raw:?}"
    );

    // The same unknown content type on the behaviour whose configuration forwards nothing it
    // cannot encrypt: refused by CloudFront, with no origin request.
    let case = "the same unknown content type on a behaviour that forwards nothing unencrypted";
    let answered = post_form(
        &topology,
        &domain,
        "/strict/post",
        JSON_MEDIA_TYPE,
        document,
    )
    .await;
    assert_refused(&answered, case, "4111111111111111");

    // And a form body on that same behaviour, which its content type mapping does resolve: the
    // designated field is encrypted under the profile that mapping names.
    let case = "a form body on the behaviour that forwards nothing unencrypted";
    let answered = post_form(&topology, &domain, "/strict/post", FORM_MEDIA_TYPE, &body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the origin's 200, got {} with {:?}",
        answered.status, answered.body
    );
    let raw = received_body(&answered.body, case);
    let received = fields(&raw);
    assert_encrypted(
        &received,
        &raw,
        case,
        "secret_field",
        "4111111111111111",
        &content_type_client,
    );
    assert_verbatim(&received, case, "card_number", "5500005555555559");
}
