// SPDX-License-Identifier: CC0-1.0

//! A presigned URL whose credential travels in the `AWSAccessKeyId` / `Signature` / `Expires`
//! presentation — the other form AWS accepts a query credential in — is authenticated by that
//! form, and the request it names is then served or refused exactly as its signature says.
//!
//! Every assertion drives a real HTTP request through a router with request authentication ON,
//! as the emulator runs it, so what is pinned is the wire behaviour the fetcher of such a URL
//! sees: the status, the bytes, and the `<Code>` and `<Message>` of S3's error document.
//!
//! The rules under test:
//!
//! 1. WHICH PRESENTATION a query credential travels in is a protocol concern, not a
//!    per-operation one: a URL signed in this presentation, fetched inside the window it asks
//!    for, is served the answer the operation it addresses would give — HTTP 200 and the
//!    object's exact bytes for a read of a stored key, the object's size for a HEAD of one, the
//!    write for a `PUT`, and `NoSuchKey` for a read of a key the bucket does not hold.
//! 2. A URL of that presentation whose signature does not verify is refused HTTP 403 with the
//!    code `SignatureDoesNotMatch` and the fixed sentence the signing layer words for that one
//!    condition; one naming a key this emulator holds no credential for is refused
//!    `InvalidAccessKeyId`, the code that says the credential itself is unknown; and one whose
//!    window has passed is refused `AccessDenied` with the sentence AWS words an expiry in.
//!    The three are what lets a client tell a tampered URL from an unknown key and from one
//!    that has simply run out.
//!
//! The signature every row carries is computed HERE, by this suite's own reading of the
//! published derivation (`string_to_sign` below) — the client's half of it — so what is driven
//! is a URL a client's signer and this emulator both compute the same signature for rather
//! than a value taken from the service under test.
//!
//! The rows differ along what the rules generalise over: the FORM of the key (a plain name,
//! `/` separators, a space, non-ASCII UTF-8), the method the URL was signed for, the extra
//! query it carries beside the credential, the address form it was signed for (path-style and
//! virtual-hosted-style), the window it asks for, and — for the refusals — which half of the
//! credential is wrong and how far past its window the fetch is.
//!
//! The boundary of both rules is the third one: this surface judges a request only where it
//! could VERIFY one, which is where this process holds the secret of the key the URL names. A
//! URL naming any other credential is left exactly as it was — refused by the framework's own
//! `InvalidAccessKeyId` where signatures are verified, and SERVED where they are not, which is
//! what keeps an emulator running with request authentication off from demanding a credential
//! it cannot check.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object};
use super::signature_refusal_wording::{Emulator, REGIONAL_ENDPOINT_HOST};

/// The host every row is addressed to unless it names another, and so the host its fetch
/// carries.
const HOST: &str = REGIONAL_ENDPOINT_HOST;

/// The bucket every row of this suite addresses.
const BUCKET: &str = "query-credential-serving";

/// The code S3 answers a request whose signature did not verify, and the fixed sentence it
/// carries. Spelled out here rather than imported, so a wording regression fails this suite.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";
/// See [`SIGNATURE_MISMATCH_CODE`].
const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check your \
     key and signing method.";

/// The code S3 answers a credential naming a key it holds nothing for, and that code's own
/// fixed sentence.
const UNKNOWN_KEY_CODE: &str = "InvalidAccessKeyId";
/// See [`UNKNOWN_KEY_CODE`].
const UNKNOWN_KEY_MESSAGE: &str = "The AWS Access Key Id you provided does not exist in our records.";

/// The code and sentence a request whose window has passed is refused with.
const EXPIRED_CODE: &str = "AccessDenied";
/// See [`EXPIRED_CODE`].
const EXPIRED_MESSAGE: &str = "Request has expired";

/// An access key id this emulator never issued.
const UNISSUED_ACCESS_KEY_ID: &str = "AKIDNOTISSUEDBYTHISEMULATOR";

/// The characters a path keeps as they are: the unreserved set plus the `/` separator, which
/// is how a client writes a key into a URL.
const PATH_ENCODE_SET: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The characters a query parameter's value keeps: the unreserved set alone, so the `+`, `/`
/// and `=` of a base64 signature all travel percent-encoded.
const QUERY_ENCODE_SET: &::percent_encoding::AsciiSet = &PATH_ENCODE_SET.add(b'/');

/// `text` percent-encoded as a path.
fn encoded_path(text: &str) -> String {
    ::percent_encoding::utf8_percent_encode(text, PATH_ENCODE_SET).to_string()
}

/// `text` percent-encoded as a query parameter's value.
fn encoded_value(text: &str) -> String {
    ::percent_encoding::utf8_percent_encode(text, QUERY_ENCODE_SET).to_string()
}

/// The string the signature of a request covers: "HTTP-VERB + "\n" + Content-MD5 + "\n" +
/// Content-Type + "\n" + Expires + "\n" + CanonicalizedAmzHeaders + CanonicalizedResource"
/// (Amazon S3 API Reference, *REST Authentication*), with the `Expires` of a query credential
/// standing in for the `Date` a header-signed request carries.
///
/// This suite sends no `Content-MD5`, no `Content-Type` and no `x-amz-` header, so those lines
/// are empty; `resource` is the path-style address the URL names, plus the parameters that are
/// part of the resource.
fn string_to_sign(method: &str, resource: &str, expires: u64) -> String {
    format!("{method}\n\n\n{expires}\n{resource}")
}

/// The signature `string_to_sign` has under `secret`: "the HMAC-SHA1 of the UTF-8 encoding of
/// the StringToSign, base64-encoded" (same section).
fn signature_for(secret: &str, string_to_sign: &str) -> String {
    use ::base64::Engine as _;
    use ::hmac::Mac as _;

    let mut mac = <::hmac::Hmac<::sha1::Sha1> as ::hmac::Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(string_to_sign.as_bytes());
    ::base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// One presigned URL in this presentation: the credential trio appended to `endpoint` +
/// `path`, with `query` beside it, signed for `resource` — the path-style address the
/// derivation names, which for a virtual-hosted URL carries the bucket its host does.
struct Presigned {
    /// The URL a client hands out.
    url: String,
    /// The signature it carries, so a row can alter it.
    signature: String,
}

/// The URL for `method` on `path` (decoded, as the model lays it out) at `endpoint`, signed by
/// `signer` for `resource` and valid until `expires` seconds past the epoch.
///
/// `query` is the query the URL carries BESIDE its credential, already encoded as a client
/// writes it (empty for a URL carrying none): whether a parameter of it is part of the
/// resource the signature names is the derivation's own question, so the row states
/// `resource` itself.
fn presigned(
    signer: (&str, &str),
    endpoint: &str,
    method: &str,
    path: &str,
    resource: &str,
    query: &str,
    expires: u64,
) -> Presigned {
    let (access_key_id, secret) = signer;
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .map(str::to_string)
        .collect();
    let signature = signature_for(secret, &string_to_sign(method, resource, expires));
    pairs.push(format!("AWSAccessKeyId={}", encoded_value(access_key_id)));
    pairs.push(format!("Signature={}", encoded_value(&signature)));
    pairs.push(format!("Expires={expires}"));
    Presigned {
        url: format!("{endpoint}{}?{}", encoded_path(path), pairs.join("&")),
        signature,
    }
}

/// `url` with its signature replaced by `signature`, which is how a row drives a URL whose
/// signature does not verify.
fn with_signature(presigned: &Presigned, signature: &str) -> String {
    presigned.url.replace(
        &format!("Signature={}", encoded_value(&presigned.signature)),
        &format!("Signature={}", encoded_value(signature)),
    )
}

/// `signature` with its first character altered, as a client tampering with a handed-out URL
/// does. The replacement is chosen so it always differs from the character it replaces.
fn tampered(signature: &str) -> String {
    let mut characters: Vec<char> = signature.chars().collect();
    characters[0] = match characters[0] {
        'A' => 'B',
        _ => 'A',
    };
    characters.into_iter().collect()
}

/// Fetch `url` the way a client fetches a presigned URL: no `Authorization` header at all, and
/// nothing but the `host` it was signed for.
async fn fetch(emulator: &Emulator, host: &str, method: &str, url: &str, body: &[u8]) -> Answer {
    emulator.fetch(host, method, url, &[], body).await
}

/// Stage one fixture call, unsigned: the bucket and the objects these reads resolve.
async fn staged(emulator: &Emulator, method: &str, uri: &str, body: &[u8]) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/20260101/us-east-1/s3/aws4_request, SignedHeaders=host, \
         Signature=not-verified",
        emulator.access_key_id
    );
    emulator
        .send_body(
            method,
            uri,
            &[("authorization".to_string(), authorization)],
            body,
        )
        .await
}

/// `length` bytes whose value depends on the position, so a served body that is not the stored
/// one cannot pass by accident.
fn payload(seed: u8, length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// A window that is open for far longer than this suite can take to send its requests, so no
/// row's answer depends on how fast the host runs.
const OPEN_WINDOW: Duration = Duration::from_secs(60 * 60);

/// The instant a row's window runs to, `ahead` past this clock.
fn window_until(ahead: Duration) -> u64 {
    (SystemTime::now() + ahead)
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_secs()
}

/// RULE 1: a URL signed in this presentation, fetched inside its window, is served the answer
/// the operation it addresses gives.
#[tokio::test]
async fn a_query_credential_url_is_served_the_answer_its_operation_gives() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // The rows differ along the FORM of the key: a plain name, `/` separators, a space, and
    // non-ASCII UTF-8 — every one of which a client encodes into the URL and the signature
    // covers as encoded.
    let keys: [(&str, &str); 5] = [
        ("a plain name", "object.bin"),
        ("a key with / separators", "fetch/deep/object.txt"),
        ("a key with a space", "model data.tar.gz"),
        ("a key with non-ASCII characters", "données/café.txt"),
        ("a key with query punctuation", "a=b&c.txt"),
    ];
    let mut stored: Vec<(&str, &str, Vec<u8>)> = Vec::new();
    for (index, (case, key)) in keys.into_iter().enumerate() {
        let body = payload(index as u8, 256 + index);
        let written = staged(
            &emulator,
            "PUT",
            &format!("/{BUCKET}/{}", encoded_path(key)),
            &body,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the object must be stored first, got {} with body {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        stored.push((case, key, body));
    }

    emulator.enforce_signatures(true);
    let signer = (
        emulator.access_key_id.as_str(),
        emulator.secret_access_key.as_str(),
    );
    let until = window_until(OPEN_WINDOW);
    for (case, key, body) in &stored {
        let path = format!("/{BUCKET}/{key}");
        let resource = encoded_path(&path);
        let url = presigned(
            signer,
            &format!("https://{HOST}"),
            "GET",
            &path,
            &resource,
            "",
            until,
        );
        let served = fetch(&emulator, HOST, "GET", &url.url, b"").await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a query-credential GET inside its window must serve the object, got \
             {} with body {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body.as_ref(),
            body.as_slice(),
            "case {case:?}: expected the stored bytes, got {} bytes",
            served.body.len()
        );
    }

    // The same rule over the METHOD the URL was signed for: a HEAD of a stored key reports its
    // size, and a PUT writes the object the fetch carries.
    let (_, first_key, first_body) = &stored[0];
    let head_path = format!("/{BUCKET}/{first_key}");
    let head = presigned(
        signer,
        &format!("https://{HOST}"),
        "HEAD",
        &head_path,
        &encoded_path(&head_path),
        "",
        until,
    );
    let answered = fetch(&emulator, HOST, "HEAD", &head.url, b"").await;
    assert_eq!(
        answered.status, 200,
        "case \"a query-credential HEAD\": expected HTTP 200, got {}",
        answered.status
    );
    assert_eq!(
        answered.header("content-length"),
        Some(first_body.len().to_string()),
        "case \"a query-credential HEAD\": expected the object's size, got {:?}",
        answered.header("content-length")
    );

    let written_path = format!("/{BUCKET}/uploaded.bin");
    let put = presigned(
        signer,
        &format!("https://{HOST}"),
        "PUT",
        &written_path,
        &encoded_path(&written_path),
        "",
        until,
    );
    let uploaded = payload(7, 97);
    let answered = fetch(&emulator, HOST, "PUT", &put.url, &uploaded).await;
    assert_eq!(
        answered.status, 200,
        "case \"a query-credential PUT\": expected HTTP 200, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    let read_back = presigned(
        signer,
        &format!("https://{HOST}"),
        "GET",
        &written_path,
        &encoded_path(&written_path),
        "",
        until,
    );
    let served = fetch(&emulator, HOST, "GET", &read_back.url, b"").await;
    assert_eq!(
        served.body.as_ref(),
        uploaded.as_slice(),
        "case \"a query-credential PUT\": the bytes it wrote must read back, got {} bytes with \
         status {}",
        served.body.len(),
        served.status
    );

    // And over the extra query such a URL carries: a response override is part of the RESOURCE
    // the signature names, so it is signed as well as sent.
    let overridden_resource = format!(
        "{}?response-content-type=application/octet-stream",
        encoded_path(&head_path)
    );
    let overridden = presigned(
        signer,
        &format!("https://{HOST}"),
        "GET",
        &head_path,
        &overridden_resource,
        "response-content-type=application%2Foctet-stream",
        until,
    );
    let answered = fetch(&emulator, HOST, "GET", &overridden.url, b"").await;
    assert_eq!(
        answered.status, 200,
        "case \"a query-credential GET carrying a response override\": expected HTTP 200, got {} \
         with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    assert_eq!(
        answered.body.as_ref(),
        first_body.as_slice(),
        "case \"a query-credential GET carrying a response override\": expected the stored \
         bytes, got {} bytes",
        answered.body.len()
    );

    // The boundary of the rule: such a fetch resolves the OPERATION the address names, so a key
    // the bucket does not hold is absent rather than unauthorized.
    let absent_path = format!("/{BUCKET}/absent.bin");
    let absent = presigned(
        signer,
        &format!("https://{HOST}"),
        "GET",
        &absent_path,
        &encoded_path(&absent_path),
        "",
        until,
    );
    let refused = fetch(&emulator, HOST, "GET", &absent.url, b"").await;
    assert_eq!(
        refused.error_code(),
        "NoSuchKey",
        "case \"an absent key\": expected NoSuchKey, got {:?} with status {}",
        refused.error_code(),
        refused.status
    );
}

/// RULE 1, over the OPERATION the URL addresses: this presentation authenticates a request to
/// whichever of this service's operations it names, so a listing of a bucket's contents and a
/// read of a bucket's own subresource are served it too — and a parameter that is NOT part of
/// the resource is sent without being named in the string the signature covers.
#[tokio::test]
async fn a_query_credential_url_is_served_whichever_operation_it_addresses() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let written = staged(
        &emulator,
        "PUT",
        &format!("/{BUCKET}/listed.bin"),
        &payload(5, 48),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {}",
        written.status
    );
    emulator.enforce_signatures(true);
    let signer = (
        emulator.access_key_id.as_str(),
        emulator.secret_access_key.as_str(),
    );
    let until = window_until(OPEN_WINDOW);
    let bucket = format!("/{BUCKET}");
    let resource = encoded_path(&bucket);

    // (case, the query the URL carries beside its credential, the resource its signature
    // names, a fragment the served document must carry)
    let rows: [(&str, &str, String, &str); 4] = [
        (
            // The listing of what the bucket holds: no parameter of it is part of the
            // resource.
            "the listing of a bucket's contents",
            "",
            resource.clone(),
            "<Key>listed.bin</Key>",
        ),
        (
            // `list-type` names the listing's own version and is no subresource, so the
            // resource the signature names is the bucket alone.
            "the V2 listing, whose list-type is no part of the resource",
            "list-type=2",
            resource.clone(),
            "<Key>listed.bin</Key>",
        ),
        (
            // A SUBRESOURCE, which is part of the resource: the string to sign names it.
            "a read of the bucket's access control list",
            "acl",
            format!("{resource}?acl"),
            "<AccessControlPolicy",
        ),
        (
            "a read of the bucket's versioning configuration",
            "versioning",
            format!("{resource}?versioning"),
            "<VersioningConfiguration",
        ),
    ];

    for (case, query, resource, expected) in rows {
        let url = presigned(
            signer,
            &format!("https://{HOST}"),
            "GET",
            &bucket,
            &resource,
            query,
            until,
        );
        let served = fetch(&emulator, HOST, "GET", &url.url, b"").await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        let document = String::from_utf8_lossy(&served.body).to_string();
        assert!(
            document.contains(expected),
            "case {case:?}: expected a document carrying {expected:?}, got {document:?}"
        );
    }
}

/// RULE 1, over the ADDRESS the URL is signed for: where the bucket travels is not part of
/// authentication, so a URL naming its bucket in the host is served the same object a
/// path-style one is — the resource its signature names carries the bucket either way.
#[tokio::test]
async fn a_query_credential_url_is_served_at_every_address_form() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let body = payload(11, 321);
    let written = staged(&emulator, "PUT", &format!("/{BUCKET}/addressed.bin"), &body).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
    emulator.enforce_signatures(true);
    let signer = (
        emulator.access_key_id.as_str(),
        emulator.secret_access_key.as_str(),
    );
    let until = window_until(OPEN_WINDOW);

    // (case, the endpoint the URL is signed for, the host it is fetched at, the path it names)
    let rows: [(&str, String, String, String); 3] = [
        (
            "the Regional endpoint, path-style",
            format!("https://{HOST}"),
            HOST.to_string(),
            format!("/{BUCKET}/addressed.bin"),
        ),
        (
            // The address an emulator is really reached at: a local endpoint on a port.
            "a local endpoint on a port, path-style",
            "https://localhost:9297".to_string(),
            "localhost:9297".to_string(),
            format!("/{BUCKET}/addressed.bin"),
        ),
        (
            "the Regional endpoint, virtual-hosted-style",
            format!("https://{BUCKET}.{HOST}"),
            format!("{BUCKET}.{HOST}"),
            "/addressed.bin".to_string(),
        ),
    ];

    for (case, endpoint, host, path) in rows {
        // The resource the derivation names is the PATH-STYLE address of the object, whichever
        // form the URL addressed it in.
        let resource = encoded_path(&format!("/{BUCKET}/addressed.bin"));
        let url = presigned(signer, &endpoint, "GET", &path, &resource, "", until);
        let served = fetch(&emulator, &host, "GET", &url.url, b"").await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a query-credential GET must serve the object, got {} with body {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body.as_ref(),
            body.as_slice(),
            "case {case:?}: expected the stored bytes, got {} bytes",
            served.body.len()
        );
    }
}

/// RULE 2: which half of the credential is wrong decides the refusal — a signature that does
/// not verify, a key id this emulator holds nothing for, and a window that has passed are
/// three different answers.
#[tokio::test]
async fn a_query_credential_that_does_not_verify_is_refused_in_aws_own_words() {
    let mut emulator = Emulator::new();
    emulator.enforce_signatures(false);
    let created = staged(&emulator, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    let body = payload(3, 64);
    let written = staged(&emulator, "PUT", &format!("/{BUCKET}/refused.bin"), &body).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {}",
        written.status
    );
    emulator.enforce_signatures(true);
    let issued = (
        emulator.access_key_id.as_str(),
        emulator.secret_access_key.as_str(),
    );
    let open = window_until(OPEN_WINDOW);
    // A window that closed long before this suite started, so no row's answer depends on how
    // fast the host runs either.
    let closed = window_until(Duration::ZERO) - 3_600;

    // (case, method, path, the URL under test, the code and sentence it earns)
    let signed = |method: &str, path: &str, until: u64| {
        presigned(
            issued,
            &format!("https://{HOST}"),
            method,
            path,
            &encoded_path(path),
            "",
            until,
        )
    };
    let object = format!("/{BUCKET}/refused.bin");
    let nested = format!("/{BUCKET}/fetch/deep/object.txt");

    let read = signed("GET", &object, open);
    let head = signed("HEAD", &object, open);
    let write = signed("PUT", &object, open);
    let deep = signed("GET", &nested, open);
    let other_secret = presigned(
        (emulator.access_key_id.as_str(), "not-the-secret-it-issued"),
        &format!("https://{HOST}"),
        "GET",
        &object,
        &encoded_path(&object),
        "",
        open,
    );
    let other_resource = presigned(
        issued,
        &format!("https://{HOST}"),
        "GET",
        &object,
        // Signed for ANOTHER resource than the one it addresses, which is what a URL whose
        // path was edited after signing carries.
        &encoded_path(&nested),
        "",
        open,
    );
    let unissued = presigned(
        (UNISSUED_ACCESS_KEY_ID, "whatever-secret-it-claims"),
        &format!("https://{HOST}"),
        "GET",
        &object,
        &encoded_path(&object),
        "",
        open,
    );
    let expired = signed("GET", &object, closed);
    let expired_write = signed("PUT", &object, closed);

    let rows: [(&str, &str, String, &str, &str); 9] = [
        (
            // The recorded instance: one character of a handed-out URL's signature altered.
            "a read whose signature was altered in its first character",
            "GET",
            with_signature(&read, &tampered(&read.signature)),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a HEAD whose signature was altered",
            "HEAD",
            with_signature(&head, &tampered(&head.signature)),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a write whose signature was altered",
            "PUT",
            with_signature(&write, &tampered(&write.signature)),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a read of a nested key whose signature was altered",
            "GET",
            with_signature(&deep, &tampered(&deep.signature)),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a read carrying a signature that is no base64 at all",
            "GET",
            with_signature(&read, "not-a-signature"),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a read signed with a secret this emulator never issued",
            "GET",
            other_secret.url.clone(),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a read whose signature covers another resource than it addresses",
            "GET",
            other_resource.url.clone(),
            SIGNATURE_MISMATCH_CODE,
            SIGNATURE_MISMATCH_MESSAGE,
        ),
        (
            "a read naming a key id this emulator never issued",
            "GET",
            unissued.url.clone(),
            UNKNOWN_KEY_CODE,
            UNKNOWN_KEY_MESSAGE,
        ),
        (
            "a read whose window closed an hour ago",
            "GET",
            expired.url.clone(),
            EXPIRED_CODE,
            EXPIRED_MESSAGE,
        ),
    ];

    for (case, method, url, code, message) in rows {
        let refused = fetch(&emulator, HOST, method, &url, b"").await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the {code} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            message,
            "case {case:?}: expected AWS's sentence for that condition, got {:?}",
            refused.error_message()
        );
    }

    // A write refused for its credential writes nothing: the refusal stands in front of the
    // operation, so the object it addressed is the one that was already there.
    let refused_write = fetch(&emulator, HOST, "PUT", &expired_write.url, b"tampered").await;
    assert_eq!(
        refused_write.status, 403,
        "case \"a write whose window closed\": expected HTTP 403, got {}",
        refused_write.status
    );
    let served = fetch(
        &emulator,
        HOST,
        "GET",
        &signed("GET", &object, window_until(OPEN_WINDOW)).url,
        b"",
    )
    .await;
    assert_eq!(
        served.body.as_ref(),
        body.as_slice(),
        "case \"a write whose window closed\": the stored object must be unchanged, got {} bytes \
         with status {}",
        served.body.len(),
        served.status
    );
}

/// The bucket the suite below addresses, on an emulator of its own.
const UNVERIFIED_BUCKET: &str = "query-credential-unverified";

/// Dispatch one request to a router that verifies no signatures, carrying no `Authorization`
/// header at all — the shape of a presigned fetch.
async fn fetch_unverified(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    url: &str,
    body: &[u8],
) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host)
        .body(::bytes::Bytes::copy_from_slice(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// RULES 1 and 2, at their shared BOUNDARY: this surface judges a URL only where it could
/// verify one — where this process holds the secret of the key the URL names — so an emulator
/// that verifies no signatures serves such a URL exactly as it did before this surface
/// existed, rather than refusing a credential it cannot check.
///
/// Every row names a credential whose secret this process holds nothing for, which is every
/// credential at all on an emulator that publishes no key pair; the same URL on an emulator
/// that DOES verify signatures is refused `InvalidAccessKeyId` by the boundary that resolves
/// credentials (the row of that name in
/// [`a_query_credential_that_does_not_verify_is_refused_in_aws_own_words`]).
#[tokio::test]
async fn an_emulator_that_verifies_no_signatures_serves_such_a_url() {
    let router = router();
    bucket(&router, UNVERIFIED_BUCKET).await;
    let body = payload(13, 128);
    let written = put_object(&router, UNVERIFIED_BUCKET, "served.bin", &body, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object must be stored first, got {} with body {:?}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );

    let unresolvable = (UNISSUED_ACCESS_KEY_ID, "no-secret-this-process-holds");
    let path = format!("/{UNVERIFIED_BUCKET}/served.bin");
    // (case, the window the URL asks for)
    let rows: [(&str, u64); 3] = [
        ("a window that is still open", window_until(OPEN_WINDOW)),
        (
            // Even a window that has closed: judging it needs the credential this emulator
            // cannot resolve, so nothing here judges the URL at all.
            "a window that closed an hour ago",
            window_until(Duration::ZERO) - 3_600,
        ),
        ("a window of a single second", 1),
    ];
    for (case, until) in rows {
        let url = presigned(
            unresolvable,
            &format!("https://{HOST}"),
            "GET",
            &path,
            &encoded_path(&path),
            "",
            until,
        );
        let served = fetch_unverified(&router, HOST, "GET", &url.url, b"").await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: an emulator verifying no signatures must serve such a URL, got {} \
             with body {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body.as_ref(),
            body.as_slice(),
            "case {case:?}: expected the stored bytes, got {} bytes",
            served.body.len()
        );
    }

    // And the write such a URL asks for is performed, not refused: it reads back at the key it
    // addressed.
    let written_path = format!("/{UNVERIFIED_BUCKET}/uploaded.bin");
    let uploaded = payload(17, 64);
    let put = presigned(
        unresolvable,
        &format!("https://{HOST}"),
        "PUT",
        &written_path,
        &encoded_path(&written_path),
        "",
        window_until(OPEN_WINDOW),
    );
    let answered = fetch_unverified(&router, HOST, "PUT", &put.url, &uploaded).await;
    assert_eq!(
        answered.status, 200,
        "case \"a write through such a URL\": expected HTTP 200, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    let read_back = presigned(
        unresolvable,
        &format!("https://{HOST}"),
        "GET",
        &written_path,
        &encoded_path(&written_path),
        "",
        window_until(OPEN_WINDOW),
    );
    let served = fetch_unverified(&router, HOST, "GET", &read_back.url, b"").await;
    assert_eq!(
        served.body.as_ref(),
        uploaded.as_slice(),
        "case \"a write through such a URL\": the bytes it wrote must read back, got {} bytes \
         with status {}",
        served.body.len(),
        served.status
    );
}

