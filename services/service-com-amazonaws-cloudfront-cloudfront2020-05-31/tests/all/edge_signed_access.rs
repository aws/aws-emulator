// SPDX-License-Identifier: CC0-1.0

//! SIGNED URLs, SIGNED COOKIES and the TRUSTED KEY GROUPS behind them, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises the claim, the resolution
//! of the distribution, the cache-behaviour selection, the signed-access decision and the
//! origin fetch — the same path a viewer takes. The rule under test is the one
//! `CacheBehavior$TrustedKeyGroups` states: "when a cache behavior contains trusted key
//! groups, CloudFront requires signed URLs or signed cookies for all requests that match the
//! cache behavior. The URLs or cookies must be signed with a private key whose corresponding
//! public key is in the key group." Which here means, over a table rather than over one URL:
//!
//! * a request that carries NO signature is refused by CloudFront with `403`, `MissingKey`
//!   and no origin bytes, while the same request on a behaviour that names no key group is
//!   served;
//! * a request signed with a key that is in the behaviour's key groups, over a policy whose
//!   conditions hold, is served FROM THE ORIGIN — in both policy forms AWS publishes (a
//!   canned policy's `Expires`, a custom policy's `Policy`) and through both carriers (the
//!   query string of a signed URL, and signed cookies);
//! * a policy CONDITION that does not hold is `403` `AccessDenied`: an expiry that has
//!   passed, a start date still ahead, a `Resource` naming something else, and an
//!   `AWS:SourceIp` range that does not contain the address the request came from — the
//!   address the edge itself observes, which is the one it reports to an origin as
//!   `CloudFront-Viewer-Address`;
//! * a signature that is not the key's, or a policy altered after it was signed, is `403`
//!   `AccessDenied` — the signature is verified, not merely parsed;
//! * a key that is not in the behaviour's key groups is `403` `InvalidKey`, and so is a key
//!   `UpdateKeyGroup` has REMOVED from the group, while the keys still in that group keep
//!   being admitted. Removing a public key from a key group is a revocation.
//!
//! ## The origin this suite points at
//!
//! An edge fetch is a real in-process HTTP request to the origin's own `DomainName`, so the
//! origin here is an endpoint this emulator serves: this service's own control-plane host,
//! whose `GET /2020-05-31/distribution` answers a `DistributionList`. An admitted request is
//! therefore observable as the origin's own `200` and its own bytes, and a refused one as the
//! absence of both.
//!
//! ## How the URLs are signed
//!
//! By a signer written here, the way a client's signer works: a real 2048-bit RSA key pair
//! whose public half is uploaded through `CreatePublicKey`, and signatures made over the exact
//! policy statement bytes with RSA PKCS#1 v1.5 over a SHA-1 digest — CloudFront's own scheme.
//! No RSA implementation this workspace builds will SIGN with SHA-1 (the providers offer that
//! digest for verification only), so the EMSA-PKCS1-v1_5 encoding is composed here over the
//! modular exponentiation `num-bigint` provides; the digest, the key and the verification are
//! all the libraries' own.

use ::base64::Engine as _;
use ::num_bigint::BigUint;
use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use super::distribution_lifecycle::{
    awaited_deployment, call, create_distribution, emulator, API_VERSION, HOST,
};

/// An origin this emulator SERVES: this service's own control-plane endpoint. A viewer path
/// of `/2020-05-31/distribution` fetched from it answers a `DistributionList`.
const SERVED_ORIGIN: &str = HOST;

/// The viewer path the served origin answers a `DistributionList` at — the object every
/// signed URL below is signed for.
const SERVED_PATH: &str = "/2020-05-31/distribution";

/// The path pattern of the behaviour that names NO key group, which is what makes the key
/// group (not the origin, and not the path) the thing deciding. It names a path the served
/// origin answers too, so an admitted request there is the origin's own `200` as well.
const OPEN_PATTERN: &str = "/2020-05-31/key-group*";

/// A viewer path that selects [`OPEN_PATTERN`] — the served origin's `ListKeyGroups`.
const OPEN_PATH: &str = "/2020-05-31/key-group";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: a
/// behaviour under it caches nothing, so every admitted request goes to the origin and no
/// case is answered out of another case's entry.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// `X-Cache` for an answer the origin produced for this request.
const MISS: &str = "Miss from cloudfront";

/// `X-Cache` for an answer the edge cache held.
const HIT: &str = "Hit from cloudfront";

/// `X-Cache` for an answer CloudFront produced itself.
const ERROR: &str = "Error from cloudfront";

/// A fragment of the ORIGIN's own answer: a request that carries it reached the origin.
const ORIGIN_BYTES: &str = "DistributionList";

/// The DigestInfo prefix of a SHA-1 digest, as the EMSA-PKCS1-v1_5 encoding of RFC 8017
/// (section 9.2, note 1) fixes it for that hash: the DER of
/// `DigestInfo { SEQUENCE { sha1 AlgorithmIdentifier, OCTET STRING (20) } }`.
const SHA1_DIGEST_INFO_PREFIX: &[u8] = &[
    0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14,
];

/// The PEM label RFC 7468 section 13 gives an X.509 `SubjectPublicKeyInfo`, which is the
/// envelope `PublicKeyConfig$EncodedKey` carries a public key in.
const PUBLIC_KEY_LABEL: &str = "PUBLIC KEY";

// ---------------------------------------------------------------------------
// The signer
// ---------------------------------------------------------------------------

/// One RSA key pair, held the way the client that signs a CloudFront URL holds one.
struct Signer {
    /// The public half as the PEM `SubjectPublicKeyInfo` `CreatePublicKey` takes.
    public_key_pem: String,
    /// How many octets the modulus — and so every signature — is long.
    modulus_octets: usize,
    /// The quintuple PKCS#1 keeps for the private-key operation of RFC 8017 section 5.1.2.
    prime1: BigUint,
    prime2: BigUint,
    exponent1: BigUint,
    exponent2: BigUint,
    coefficient: BigUint,
}

impl Signer {
    /// A fresh 2048-bit RSA pair: the modulus width CloudFront's own signed-URL procedure
    /// documents for a key group's public key.
    fn generate() -> Self {
        use ::aws_lc_rs::encoding::{AsDer, Pkcs8V1Der, PublicKeyX509Der};
        use ::aws_lc_rs::signature::KeyPair as _;
        use ::pkcs1::der::Decode as _;

        let key = ::aws_lc_rs::signature::RsaKeyPair::generate(
            ::aws_lc_rs::rsa::KeySize::Rsa2048,
        )
        .expect("the provider generates a 2048-bit RSA pair");
        let public: PublicKeyX509Der<'_> =
            AsDer::as_der(key.public_key()).expect("the public half encodes as an SPKI");
        let private: Pkcs8V1Der<'_> =
            AsDer::as_der(&key).expect("the private half encodes as a PKCS#8");
        let info = ::pkcs8::PrivateKeyInfo::try_from(private.as_ref())
            .expect("the generated key is a PKCS#8 PrivateKeyInfo");
        let parts = ::pkcs1::RsaPrivateKey::from_der(info.private_key)
            .expect("a PKCS#8 RSA key carries a PKCS#1 RSAPrivateKey");
        let number =
            |field: ::pkcs1::UintRef<'_>| BigUint::from_bytes_be(field.as_bytes());
        Self {
            public_key_pem: ::pem::encode(&::pem::Pem::new(
                PUBLIC_KEY_LABEL.to_string(),
                public.as_ref().to_vec(),
            )),
            modulus_octets: parts.modulus.as_bytes().len(),
            prime1: number(parts.prime1),
            prime2: number(parts.prime2),
            exponent1: number(parts.exponent1),
            exponent2: number(parts.exponent2),
            coefficient: number(parts.coefficient),
        }
    }

    /// The CloudFront `Signature` of `statement`: "hash and sign the policy statement … use
    /// SHA-1 to hash and RSA to sign", base64-encoded with the three substitutions that let
    /// the value travel in a URL.
    fn sign(&self, statement: &str) -> String {
        let digest = ::aws_lc_rs::digest::digest(
            &::aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY,
            statement.as_bytes(),
        );
        let mut digest_info = SHA1_DIGEST_INFO_PREFIX.to_vec();
        digest_info.extend_from_slice(digest.as_ref());
        // EM = 0x00 || 0x01 || PS || 0x00 || T (RFC 8017 section 9.2).
        let padding = self
            .modulus_octets
            .checked_sub(digest_info.len() + 3)
            .expect("a 2048-bit modulus holds a SHA-1 DigestInfo and its padding");
        let mut encoded = vec![0x00, 0x01];
        encoded.resize(2 + padding, 0xff);
        encoded.push(0x00);
        encoded.extend_from_slice(&digest_info);
        let message = BigUint::from_bytes_be(&encoded);
        // The private-key operation through the Chinese remainder theorem, which is the form
        // RFC 8017 section 5.1.2 states for a key carrying the quintuple.
        let first = message.modpow(&self.exponent1, &self.prime1);
        let second = message.modpow(&self.exponent2, &self.prime2);
        let difference = (&first + &self.prime1 - (&second % &self.prime1)) % &self.prime1;
        let factor = (&self.coefficient * difference) % &self.prime1;
        let signature = (second + &self.prime2 * factor).to_bytes_be();
        // A PKCS#1 signature is exactly as long as the modulus, so a representative with
        // leading zero octets keeps them.
        let mut padded = vec![
            0u8;
            self.modulus_octets
                .checked_sub(signature.len())
                .expect("a signature is no longer than the modulus")
        ];
        padded.extend_from_slice(&signature);
        url_safe_base64(&padded)
    }
}

/// `bytes` in the base64 alphabet CloudFront's signing procedure spells a URL-safe value in:
/// "replace all occurrences of `+`, `=`, and `/` in the base64-encoded version" with `-`, `_`
/// and `~`.
fn url_safe_base64(bytes: &[u8]) -> String {
    ::base64::engine::general_purpose::STANDARD
        .encode(bytes)
        .chars()
        .map(|character| match character {
            '+' => '-',
            '=' => '_',
            '/' => '~',
            other => other,
        })
        .collect()
}

/// A CUSTOM policy statement, as CloudFront's documented JSON: one statement naming
/// `resource`, with the conditions `expires`, `starts` and `source_ip` state.
fn custom_policy(
    resource: &str,
    expires: u64,
    starts: Option<u64>,
    source_ip: Option<&str>,
) -> String {
    let mut conditions = vec![format!("\"DateLessThan\":{{\"AWS:EpochTime\":{expires}}}")];
    if let Some(starts) = starts {
        conditions.push(format!("\"DateGreaterThan\":{{\"AWS:EpochTime\":{starts}}}"));
    }
    if let Some(source_ip) = source_ip {
        conditions.push(format!("\"IpAddress\":{{\"AWS:SourceIp\":\"{source_ip}\"}}"));
    }
    format!(
        "{{\"Statement\":[{{\"Resource\":\"{resource}\",\"Condition\":{{{}}}}}]}}",
        conditions.join(",")
    )
}

/// A CANNED policy statement, which CloudFront rebuilds from the URL and `Expires` alone.
fn canned_policy(resource: &str, expires: u64) -> String {
    format!(
        "{{\"Statement\":[{{\"Resource\":\"{resource}\",\"Condition\":\
         {{\"DateLessThan\":{{\"AWS:EpochTime\":{expires}}}}}}}]}}"
    )
}

/// A Unix instant well ahead of any run of this suite, for a policy that has not expired.
fn far_future() -> u64 {
    now_seconds() + 3_600
}

/// A Unix instant well behind any run of this suite, for a policy that has.
fn far_past() -> u64 {
    now_seconds() - 3_600
}

/// The current Unix instant, in seconds.
fn now_seconds() -> u64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the host clock is at or after the Unix epoch")
        .as_secs()
}

// ---------------------------------------------------------------------------
// Staging the distribution
// ---------------------------------------------------------------------------

/// The `DistributionConfig` this suite serves through: one origin, a DEFAULT behaviour that
/// trusts `key_group` and an ordered [`OPEN_PATTERN`] behaviour that trusts nothing.
fn config(caller_reference: &str, origin: &str, key_group: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>{origin}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <TrustedKeyGroups><Enabled>true</Enabled><Quantity>1</Quantity>\
         <Items><KeyGroup>{key_group}</KeyGroup></Items></TrustedKeyGroups>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></AllowedMethods>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
         <PathPattern>{OPEN_PATTERN}</PathPattern><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></AllowedMethods>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </CacheBehavior></Items></CacheBehaviors>\
         <Comment>edge signed access</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// Upload `signer`'s public half through `CreatePublicKey` and report the id it minted.
async fn upload_public_key(
    router: &MultiServiceRouter,
    tag: &str,
    signer: &Signer,
) -> String {
    let created = call(
        router,
        "POST",
        &format!("/{API_VERSION}/public-key"),
        Some(&format!(
            "<PublicKeyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{tag}</CallerReference><Name>{tag}</Name>\
             <EncodedKey>{}</EncodedKey></PublicKeyConfig>",
            signer.public_key_pem
        )),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "uploading the public key {tag} answered {}: {}",
        created.status, created.raw
    );
    created
        .root()
        .find_text("Id")
        .expect("the created public key reports an id")
        .to_string()
}

/// Create a key group over `keys` and report `(Id, ETag)`.
async fn create_key_group(
    router: &MultiServiceRouter,
    tag: &str,
    keys: &[&str],
) -> (String, String) {
    let items: String = keys
        .iter()
        .map(|key| format!("<PublicKey>{key}</PublicKey>"))
        .collect();
    let created = call(
        router,
        "POST",
        &format!("/{API_VERSION}/key-group"),
        Some(&format!(
            "<KeyGroupConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{tag}</Name><Items>{items}</Items></KeyGroupConfig>"
        )),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "creating the key group {tag} answered {}: {}",
        created.status, created.raw
    );
    (
        created
            .root()
            .find_text("Id")
            .expect("the created key group reports an id")
            .to_string(),
        created.header("etag").to_string(),
    )
}

/// Replace the key group `id`'s membership with `keys` and report the fresh version token.
async fn update_key_group(
    router: &MultiServiceRouter,
    id: &str,
    tag: &str,
    keys: &[&str],
    etag: &str,
) -> String {
    let items: String = keys
        .iter()
        .map(|key| format!("<PublicKey>{key}</PublicKey>"))
        .collect();
    let updated = call(
        router,
        "PUT",
        &format!("/{API_VERSION}/key-group/{id}"),
        Some(&format!(
            "<KeyGroupConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{tag}</Name><Items>{items}</Items></KeyGroupConfig>"
        )),
        Some(etag),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "updating the key group answered {}: {}",
        updated.status, updated.raw
    );
    updated.header("etag").to_string()
}

/// Create one distribution over `key_group` and let the clock deploy it: the domain name
/// CloudFront assigned it.
async fn deployed(
    router: &MultiServiceRouter,
    caller_reference: &str,
    key_group: &str,
) -> String {
    let created =
        create_distribution(router, &config(caller_reference, SERVED_ORIGIN, key_group)).await;
    assert_eq!(
        created.status, 201,
        "creating the distribution answered {}: {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    awaited_deployment(router, caller_reference, &id).await;
    domain
}

// ---------------------------------------------------------------------------
// Viewing
// ---------------------------------------------------------------------------

/// What the edge answered one viewer request with.
struct Viewed {
    status: u16,
    x_cache: String,
    body: String,
}

impl Viewed {
    /// Whether CloudFront's own error body carries the code `code`.
    fn carries_code(&self, code: &str) -> bool {
        self.body.contains(&format!("<Code>{code}</Code>"))
    }
}

/// Send one VIEWER request: no AWS credential, addressed to `host`, carrying `headers`.
async fn view(
    router: &MultiServiceRouter,
    host: &str,
    path_and_query: &str,
    headers: &[(&str, &str)],
) -> Viewed {
    let mut builder = ::http::Request::builder()
        .method("GET")
        .uri(path_and_query)
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let x_cache = response
        .headers()
        .get("x-cache")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    Viewed {
        status,
        x_cache,
        body,
    }
}

/// The query string of a signed URL carrying a CUSTOM policy.
fn custom_query(policy: &str, signature: &str, key_pair_id: &str) -> String {
    format!(
        "Policy={}&Signature={signature}&Key-Pair-Id={key_pair_id}",
        url_safe_base64(policy.as_bytes())
    )
}

/// The query string of a signed URL carrying a CANNED policy.
fn canned_query(expires: u64, signature: &str, key_pair_id: &str) -> String {
    format!("Expires={expires}&Signature={signature}&Key-Pair-Id={key_pair_id}")
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// What one row expects of the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The origin answered it: a `200` with the origin's own bytes.
    Served,
    /// CloudFront refused it itself, under this error code, with no origin bytes.
    Refused(&'static str),
}

/// The whole rule, over a table: what a behaviour that trusts a key group serves, and the
/// code it refuses everything else with.
#[tokio::test]
async fn signed_access_decides_every_request_a_trusted_key_group_covers() {
    let router = emulator();
    let signer = Signer::generate();
    let stranger = Signer::generate();
    let key = upload_public_key(&router, "signed-access-a", &signer).await;
    let other = upload_public_key(&router, "signed-access-b", &stranger).await;
    let (group, _etag) = create_key_group(&router, "signed-access", &[&key]).await;
    let domain = deployed(&router, "signed-access", &group).await;
    let resource = format!("https://{domain}{SERVED_PATH}");
    // The address the edge observes for a viewer that states one, which the source-address
    // condition of a policy is evaluated against.
    const VIEWER: &str = "203.0.113.7";
    const FROM_VIEWER: &[(&str, &str)] = &[("x-forwarded-for", VIEWER)];
    let viewer = VIEWER;
    let from_viewer = FROM_VIEWER;

    let signed = |policy: &str| signer.sign(policy);
    // (case, the viewer path and query, the headers it carries, what the edge must do)
    type Case = (String, String, &'static [(&'static str, &'static str)], Expected);
    let mut cases: Vec<Case> = Vec::new();

    // Nothing at all: there is no credential to judge.
    cases.push((
        "no signature at all".to_string(),
        SERVED_PATH.to_string(),
        &[],
        Expected::Refused("MissingKey"),
    ));
    cases.push((
        "a signature with no Key-Pair-Id".to_string(),
        format!("{SERVED_PATH}?Expires={}&Signature=abc", far_future()),
        &[],
        Expected::Refused("MissingKey"),
    ));
    cases.push((
        "a behaviour that trusts no key group serves an unsigned request".to_string(),
        OPEN_PATH.to_string(),
        &[],
        Expected::Served,
    ));

    // The two policy forms, both admitted.
    let canned = canned_policy(&resource, far_future());
    cases.push((
        "a canned policy that has not expired".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            canned_query(far_future(), &signed(&canned), &key)
        ),
        &[],
        Expected::Served,
    ));
    let plain_custom = custom_policy(&resource, far_future(), None, None);
    cases.push((
        "a custom policy that has not expired".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&plain_custom, &signed(&plain_custom), &key)
        ),
        &[],
        Expected::Served,
    ));
    let wildcard = custom_policy(&format!("https://{domain}/2020-05-31/*"), far_future(), None, None);
    cases.push((
        "a custom policy naming the object by a wildcard".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&wildcard, &signed(&wildcard), &key)
        ),
        &[],
        Expected::Served,
    ));
    let started = custom_policy(&resource, far_future(), Some(far_past()), None);
    cases.push((
        "a custom policy whose start date has passed".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&started, &signed(&started), &key)
        ),
        &[],
        Expected::Served,
    ));

    // The conditions that do not hold.
    let expired_canned = canned_policy(&resource, far_past());
    cases.push((
        "a canned policy that has expired".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            canned_query(far_past(), &signed(&expired_canned), &key)
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));
    let expired = custom_policy(&resource, far_past(), None, None);
    cases.push((
        "a custom policy that has expired".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&expired, &signed(&expired), &key)
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));
    let ahead = custom_policy(&resource, far_future(), Some(far_future()), None);
    cases.push((
        "a custom policy whose start date is still ahead".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&ahead, &signed(&ahead), &key)
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));
    let elsewhere = custom_policy(&format!("https://{domain}/private/*"), far_future(), None, None);
    cases.push((
        "a custom policy naming another object".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&elsewhere, &signed(&elsewhere), &key)
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));
    let other_distribution =
        custom_policy("https://d111111abcdef8.cloudfront.net/2020-05-31/*", far_future(), None, None);
    cases.push((
        "a custom policy naming another distribution's domain".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(
                &other_distribution,
                &signed(&other_distribution),
                &key
            )
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));

    // The source-address condition, against the address the edge itself observes.
    let in_range = custom_policy(&resource, far_future(), None, Some(&format!("{viewer}/32")));
    cases.push((
        "a /32 of the address the request came from".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&in_range, &signed(&in_range), &key)
        ),
        from_viewer,
        Expected::Served,
    ));
    let in_prefix = custom_policy(&resource, far_future(), None, Some("203.0.113.0/24"));
    cases.push((
        "a prefix that holds the address the request came from".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&in_prefix, &signed(&in_prefix), &key)
        ),
        from_viewer,
        Expected::Served,
    ));
    cases.push((
        "the same /32 from another address".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&in_range, &signed(&in_range), &key)
        ),
        &[("x-forwarded-for", "198.51.100.9")],
        Expected::Refused("AccessDenied"),
    ));
    let out_of_range = custom_policy(&resource, far_future(), None, Some("10.0.0.0/8"));
    cases.push((
        "a range that excludes the address the request came from".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&out_of_range, &signed(&out_of_range), &key)
        ),
        from_viewer,
        Expected::Refused("AccessDenied"),
    ));

    // The signature itself.
    cases.push((
        "a policy altered after it was signed".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(
                &custom_policy(&resource, far_future() + 1, None, None),
                &signed(&plain_custom),
                &key
            )
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));
    cases.push((
        "a signature that is not base64 at all".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&plain_custom, "not-base64!!!", &key)
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));
    cases.push((
        "another key's signature under the trusted key's id".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&plain_custom, &stranger.sign(&plain_custom), &key)
        ),
        &[],
        Expected::Refused("AccessDenied"),
    ));

    // The key, which must be in the behaviour's groups.
    cases.push((
        "a key the group does not hold".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&plain_custom, &stranger.sign(&plain_custom), &other)
        ),
        &[],
        Expected::Refused("InvalidKey"),
    ));
    cases.push((
        "a key no account holds".to_string(),
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&plain_custom, &signed(&plain_custom), "K2JVZ9V4VPZD9")
        ),
        &[],
        Expected::Refused("InvalidKey"),
    ));

    for (case, path_and_query, headers, expected) in cases {
        let answer = view(&router, &domain, &path_and_query, headers).await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    (answer.status, answer.x_cache.as_str()),
                    (200, MISS),
                    "case {case:?}: expected the origin's 200 with X-Cache {MISS:?}, got {} \
                     with {:?} (body {:?})",
                    answer.status,
                    answer.x_cache,
                    answer.body
                );
            }
            Expected::Refused(code) => {
                assert_eq!(
                    (answer.status, answer.x_cache.as_str()),
                    (403, ERROR),
                    "case {case:?}: expected CloudFront's own 403 with X-Cache {ERROR:?}, \
                     got {} with {:?} (body {:?})",
                    answer.status,
                    answer.x_cache,
                    answer.body
                );
                assert!(
                    answer.carries_code(code),
                    "case {case:?}: expected the error code {code:?}, got {:?}",
                    answer.body
                );
                assert!(
                    !answer.body.contains(ORIGIN_BYTES),
                    "case {case:?}: the refusal carries origin bytes, so the request reached \
                     the origin: {:?}",
                    answer.body
                );
            }
        }
    }
}

/// SIGNED COOKIES carry the same policy and are judged the same way: the four values travel
/// in the `Cookie` header under the `CloudFront-` prefix, and a request that carries both a
/// signed URL and signed cookies is read as the signed URL.
#[tokio::test]
async fn signed_cookies_carry_the_same_policy_as_a_signed_url() {
    let router = emulator();
    let signer = Signer::generate();
    let key = upload_public_key(&router, "signed-cookies-a", &signer).await;
    let (group, _etag) = create_key_group(&router, "signed-cookies", &[&key]).await;
    let domain = deployed(&router, "signed-cookies", &group).await;
    let resource = format!("https://{domain}{SERVED_PATH}");

    let admitting = custom_policy(&resource, far_future(), None, None);
    let refusing = custom_policy(&resource, far_past(), None, None);
    let cookie = |policy: &str, key_pair_id: &str| {
        format!(
            "CloudFront-Policy={}; CloudFront-Signature={}; CloudFront-Key-Pair-Id={key_pair_id}",
            url_safe_base64(policy.as_bytes()),
            signer.sign(policy)
        )
    };

    // (case, the query string the URL carries, the Cookie header, what the edge must do)
    let cases: [(&str, String, String, Expected); 5] = [
        (
            "cookies carrying a policy that holds",
            String::new(),
            cookie(&admitting, &key),
            Expected::Served,
        ),
        (
            "cookies carrying a policy that has expired",
            String::new(),
            cookie(&refusing, &key),
            Expected::Refused("AccessDenied"),
        ),
        (
            "cookies naming a key the group does not hold",
            String::new(),
            cookie(&admitting, "K2JVZ9V4VPZD9"),
            Expected::Refused("InvalidKey"),
        ),
        (
            "cookies beside other cookies of the viewer's",
            String::new(),
            format!("session=abc; {}; tracking=1", cookie(&admitting, &key)),
            Expected::Served,
        ),
        (
            // "If the viewer request contains both signed cookies and a signed URL,
            // CloudFront uses only the signed URL": the admitting cookies do not rescue a
            // signed URL whose own policy has expired.
            "a signed URL that has expired beside cookies that have not",
            custom_query(&refusing, &signer.sign(&refusing), &key),
            cookie(&admitting, &key),
            Expected::Refused("AccessDenied"),
        ),
    ];

    for (case, query, cookies, expected) in cases {
        let path_and_query = if query.is_empty() {
            SERVED_PATH.to_string()
        } else {
            format!("{SERVED_PATH}?{query}")
        };
        let answer = view(&router, &domain, &path_and_query, &[("cookie", &cookies)]).await;
        match expected {
            Expected::Served => assert_eq!(
                (answer.status, answer.x_cache.as_str()),
                (200, MISS),
                "case {case:?}: expected the origin's 200 with X-Cache {MISS:?}, got {} with \
                 {:?} (body {:?})",
                answer.status,
                answer.x_cache,
                answer.body
            ),
            Expected::Refused(code) => {
                assert_eq!(
                    answer.status, 403,
                    "case {case:?}: expected CloudFront's own 403, got {} with {:?} (body \
                     {:?})",
                    answer.status, answer.x_cache, answer.body
                );
                assert!(
                    answer.carries_code(code) && !answer.body.contains(ORIGIN_BYTES),
                    "case {case:?}: expected the error code {code:?} and no origin bytes, \
                     got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// Removing a public key from a key group REVOKES the URLs signed with it, and only those:
/// the membership is re-read per request, so the other keys of the group keep being
/// admitted and a key put back is admitted again.
#[tokio::test]
async fn removing_a_public_key_from_the_group_revokes_the_urls_it_signed() {
    let router = emulator();
    let first = Signer::generate();
    let second = Signer::generate();
    let key_a = upload_public_key(&router, "revocation-a", &first).await;
    let key_b = upload_public_key(&router, "revocation-b", &second).await;
    let (group, etag) = create_key_group(&router, "revocation", &[&key_a, &key_b]).await;
    let domain = deployed(&router, "revocation", &group).await;
    let resource = format!("https://{domain}{SERVED_PATH}");

    // The two URLs, signed once and asked for again after every membership change: the same
    // bytes, so what changes between the beats is only the group.
    let signed_by_a = {
        let policy = custom_policy(&resource, far_future(), None, None);
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&policy, &first.sign(&policy), &key_a)
        )
    };
    let signed_by_b = {
        let policy = custom_policy(&resource, far_future(), None, None);
        format!(
            "{SERVED_PATH}?{}",
            custom_query(&policy, &second.sign(&policy), &key_b)
        )
    };

    // (case, the group's membership after the change, what A's URL gets, what B's gets)
    let beats: [(&str, &[&str], Expected, Expected); 3] = [
        (
            "both keys in the group",
            &[],
            Expected::Served,
            Expected::Served,
        ),
        (
            "B removed from the group",
            &["A"],
            Expected::Served,
            Expected::Refused("InvalidKey"),
        ),
        (
            "B put back",
            &["A", "B"],
            Expected::Served,
            Expected::Served,
        ),
    ];

    let mut etag = etag;
    for (case, membership, for_a, for_b) in beats {
        if !membership.is_empty() {
            let keys: Vec<&str> = membership
                .iter()
                .map(|name| if *name == "A" { key_a.as_str() } else { key_b.as_str() })
                .collect();
            etag = update_key_group(&router, &group, "revocation", &keys, &etag).await;
        }
        for (signer_name, url, expected) in [("A", &signed_by_a, for_a), ("B", &signed_by_b, for_b)]
        {
            let answer = view(&router, &domain, url, &[]).await;
            match expected {
                Expected::Served => assert_eq!(
                    (answer.status, answer.x_cache.as_str()),
                    (200, MISS),
                    "case {case:?} (the URL signed by {signer_name}): expected the origin's \
                     200 with X-Cache {MISS:?}, got {} with {:?} (body {:?})",
                    answer.status,
                    answer.x_cache,
                    answer.body
                ),
                Expected::Refused(code) => {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?} (the URL signed by {signer_name}): expected \
                         CloudFront's own 403, got {} with {:?} (body {:?})",
                        answer.status, answer.x_cache, answer.body
                    );
                    assert!(
                        answer.carries_code(code) && !answer.body.contains(ORIGIN_BYTES),
                        "case {case:?} (the URL signed by {signer_name}): expected the error \
                         code {code:?} and no origin bytes, got {:?}",
                        answer.body
                    );
                }
            }
        }
    }
}

/// `CloudFront-Viewer-Address` is a header the EDGE generates, from the address it observes
/// for the viewer: a cache policy that keys on it therefore gives two addresses two cache
/// entries and one address one, which is what proves the value is the request's own rather
/// than a constant or something the viewer chose.
#[tokio::test]
async fn the_edge_generates_the_viewers_address_for_every_request() {
    let router = emulator();
    // A cache policy of this account that caches for a minute and puts the generated header
    // in the cache key: `CachePolicyHeadersConfig$HeaderBehavior` `whitelist`.
    let created = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/cache-policy"),
        Some(&format!(
            "<CachePolicyConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>edge-viewer-address</Name><DefaultTTL>60</DefaultTTL><MaxTTL>60</MaxTTL>\
             <MinTTL>0</MinTTL><ParametersInCacheKeyAndForwardedToOrigin>\
             <EnableAcceptEncodingGzip>false</EnableAcceptEncodingGzip>\
             <HeadersConfig><HeaderBehavior>whitelist</HeaderBehavior>\
             <Headers><Quantity>1</Quantity>\
             <Items><Name>CloudFront-Viewer-Address</Name></Items></Headers>\
             </HeadersConfig>\
             <CookiesConfig><CookieBehavior>none</CookieBehavior></CookiesConfig>\
             <QueryStringsConfig><QueryStringBehavior>none</QueryStringBehavior>\
             </QueryStringsConfig></ParametersInCacheKeyAndForwardedToOrigin>\
             </CachePolicyConfig>"
        )),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "creating the cache policy answered {}: {}",
        created.status, created.raw
    );
    let policy = created
        .root()
        .find_text("Id")
        .expect("the created cache policy reports an id")
        .to_string();
    let created = create_distribution(
        &router,
        &format!(
            "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>viewer-address</CallerReference>\
             <Origins><Quantity>1</Quantity><Items><Origin>\
             <Id>primary</Id><DomainName>{SERVED_ORIGIN}</DomainName>\
             <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
             <OriginProtocolPolicy>https-only</OriginProtocolPolicy></CustomOriginConfig>\
             </Origin></Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
             <CachePolicyId>{policy}</CachePolicyId>\
             </DefaultCacheBehavior>\
             <Comment>viewer address</Comment>\
             <PriceClass>PriceClass_All</PriceClass>\
             <Enabled>true</Enabled>\
             </DistributionConfig>"
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "creating the distribution answered {}: {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .find_text("Id")
        .expect("the created distribution reports an id")
        .to_string();
    let domain = created
        .root()
        .find_text("DomainName")
        .expect("the created distribution reports a domain name")
        .to_string();
    awaited_deployment(&router, "viewer-address", &id).await;

    // (case, the address the request came from, the value the viewer STATED for the
    // generated header, the expected X-Cache). The rows differ along the address — none
    // stated, one address, another address, and back to the first — and along a viewer that
    // states the generated header itself, whose value is dropped rather than keyed on, so it
    // shares the entry of the address it really came from.
    let cases: [(&str, Option<&str>, Option<&str>, &str); 6] = [
        ("the first request from the loopback viewer", None, None, MISS),
        ("the same viewer again", None, None, HIT),
        ("a viewer at another address", Some("203.0.113.7"), None, MISS),
        ("that viewer again", Some("203.0.113.7"), None, HIT),
        ("a third address", Some("198.51.100.9"), None, MISS),
        (
            "the loopback viewer stating an address of its own",
            None,
            Some("203.0.113.7"),
            HIT,
        ),
    ];
    for (case, address, stated, expected) in cases {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(address) = address {
            headers.push(("x-forwarded-for", address));
        }
        if let Some(stated) = stated {
            headers.push(("cloudfront-viewer-address", stated));
        }
        let answer = view(&router, &domain, SERVED_PATH, &headers).await;
        assert_eq!(
            answer.x_cache, expected,
            "case {case:?}: expected X-Cache {expected:?}, got {:?} (status {}, body {:?})",
            answer.x_cache, answer.status, answer.body
        );
    }
}
