// SPDX-License-Identifier: CC0-1.0

//! The ACME HTTP surface an ACM ACME endpoint publishes: the wire
//! `AcmeEndpoint$EndpointUrl` advertises.
//!
//! # Not a modelled operation
//!
//! `smithy_query(query="service_info")` reports this service as `awsJson1_1`, and every
//! modelled operation is control plane: it creates, describes, tags and deletes ACME
//! endpoints, accounts and external account bindings. What an ACME client (certbot,
//! lego, acme.sh) then does with the URL `CreateAcmeEndpoint`/`DescribeAcmeEndpoint`
//! hand back is the ACME protocol itself (RFC 8555) — `GET <EndpointUrl>` for the
//! directory, `HEAD <newNonce>` for an anti-replay nonce, a JWS `POST` to the
//! resources the directory names. None of those is a modelled operation and none
//! carries an `X-Amz-Target`: they are addressed by host, path and HTTP method, so
//! nothing in the generated routing table can name them and they are reached through
//! the two seams [`crate::service::EmulatedService`] has for a request the model's
//! routing key cannot name.
//!
//! WHERE it is published is a third thing. An ACME client reads the FIELD NAMES of the
//! answers at the URL it was handed, and the HTTP/1.1 writer below the emulator's front
//! door normalises every name to lower case, so the address the control plane publishes for
//! a client on this machine is a socket of this plane's own, whose answer bytes are written
//! field by field ([`crate::state::acme_socket`]). The requests it accepts are dispatched
//! back through the router and answered HERE, so this module serves a client whichever
//! address it arrived at.
//!
//! An ACME client holds no AWS credential, so the anonymous path matters: the router
//! reaches [`smithy_server_runtime::Service::override_routing_error`] BEFORE it
//! authenticates anything, which is what lets an unsigned ACME client be answered at
//! all. A request to the same URL that DOES present a SigV4 credential is named as an
//! operation ([`ErasedOperation`] below) so the emulator's own authenticator judges
//! that signature first — the same verification a control-plane request gets, rather
//! than a second signature check written into this service.
//!
//! # What is served
//!
//! * `GET <base>/directory` — the ACME directory resource (RFC 8555 §7.1.1): the
//!   resource map a client starts from, every URL in it minted by
//!   [`crate::state::acme_resource_url_at`] at the address the client reached this
//!   surface on — the Region's enrollment host, or the emulator's own endpoint.
//! * `HEAD <base>/new-nonce` -> `200`, `GET <base>/new-nonce` -> `204` — a fresh
//!   anti-replay nonce in the `Replay-Nonce` header (RFC 8555 §7.2), recorded on the
//!   endpoint so the POST that spends it can be checked.
//! * `POST <base>/new-account` — a registration (RFC 8555 §7.3): the JWS is verified
//!   under the key its protected header embeds, the endpoint's external account binding
//!   is enforced, the nonce is spent, and the account record every ACME-account operation
//!   reports is written. The account's own URL is answered in `Location`.
//! * `POST <base>/acct/<id>` — that account reading or updating itself (RFC 8555 §7.3.2,
//!   §7.3.6), verified under the key the registration stored, which is what makes the
//!   account's authority its key and lets `RevokeAcmeAccount` withdraw it.
//!
//! The remaining JWS resources (`newOrder`, `revokeCert`, `keyChange`) are answered `501`
//! with an ACME problem document: this emulator issues no certificate through ACME, and a
//! `200` there would look like an interaction nothing performed. Each is its own gap, to
//! be implemented against its own recorded behaviour.
//!
//! # Order of checks on the JWS wire
//!
//! One request is judged in the order RFC 8555 §6 states the requirements in, so a client
//! is told the FIRST thing that is wrong with it: the media type (§6.2), the JWS form
//! (§6.2), the `url` the header states (§6.4), the signature under the key the request
//! identifies (§6.2), the standing of the account that signed it (§7.3.6 — a deactivated
//! or revoked account may make no further request, whatever nonce it holds), the nonce
//! (§6.5), and only then the interaction's own payload. Every answer on this wire carries
//! a fresh `Replay-Nonce`, which §6.5 requires of a `badNonce` refusal above all: the
//! nonce it hands back is one this endpoint has issued, so the client's next attempt is
//! accepted.
//!
//! # The error envelope
//!
//! An ACME server states its failures as a problem document (RFC 8555 §6.7, RFC 7807),
//! not as the control plane's awsJson error shape: an ACME client reads the `type` urn
//! to decide what to do. [`problem`] is the one builder of that body here.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::service::acme_jws::{ExternalAccountBinding, Jws, SigningKey};
use crate::state;

/// The media type the ACME directory is served as (RFC 8555 §7.1.1 answers a JSON
/// object).
const JSON_CONTENT_TYPE: &str = "application/json";

/// The media type an ACME problem document travels as (RFC 8555 §6.7).
const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

/// The ACME error type for a request that does not address a resource this server
/// publishes: "the request message was malformed" (RFC 8555 §6.7 error registry).
const MALFORMED: &str = "urn:ietf:params:acme:error:malformed";

/// The ACME error type for a failure of the server itself: "the server experienced an
/// internal error" (RFC 8555 §6.7 error registry). It is also what an interaction this
/// emulator does not implement answers with, because that is a fact about the server
/// and not about the client's request.
const SERVER_INTERNAL: &str = "urn:ietf:params:acme:error:serverInternal";

/// The ACME error type for a nonce the server did not issue, or has already seen back:
/// "the client sent an unacceptable anti-replay nonce" (RFC 8555 §6.7 error registry).
const BAD_NONCE: &str = "urn:ietf:params:acme:error:badNonce";

/// The ACME error type for a request whose signer holds no authority for it: "the client
/// lacks sufficient authorization" (RFC 8555 §6.7 error registry) — a revoked or
/// deactivated account, or an external account binding whose MAC does not check out.
const UNAUTHORIZED: &str = "urn:ietf:params:acme:error:unauthorized";

/// The ACME error type for a registration that presented no external account binding:
/// "the request must include a value for the 'externalAccountBinding' field"
/// (RFC 8555 §6.7 error registry).
const EXTERNAL_ACCOUNT_REQUIRED: &str = "urn:ietf:params:acme:error:externalAccountRequired";

/// The ACME error type for a request naming an account this server does not hold: "the
/// request specified an account that does not exist" (RFC 8555 §6.7 error registry).
const ACCOUNT_DOES_NOT_EXIST: &str = "urn:ietf:params:acme:error:accountDoesNotExist";

/// The ACME error type for a JWS signed with an algorithm this server does not support:
/// "the JWS was signed with an algorithm the server does not support" (RFC 8555 §6.7
/// error registry). Its problem document "MUST include an 'algorithms' field with an
/// array of supported 'alg' values" (RFC 8555 §6.2).
const BAD_SIGNATURE_ALGORITHM: &str = "urn:ietf:params:acme:error:badSignatureAlgorithm";

/// The member that document carries them in (RFC 8555 §6.2).
const ALGORITHMS_MEMBER: &str = "algorithms";

/// The ACME error type for a contact URI whose scheme this server does not support: "a
/// contact URL for an account used an unsupported protocol scheme" (RFC 8555 §6.7 error
/// registry).
const UNSUPPORTED_CONTACT: &str = "urn:ietf:params:acme:error:unsupportedContact";

/// The ACME error type for a contact URI this server cannot use: "a contact URL for an
/// account was invalid" (RFC 8555 §6.7 error registry).
const INVALID_CONTACT: &str = "urn:ietf:params:acme:error:invalidContact";

/// The ACME error type for a request past a server-side limit: "the request exceeds a
/// rate limit" (RFC 8555 §6.7 error registry).
const RATE_LIMITED: &str = "urn:ietf:params:acme:error:rateLimited";

/// The detail of a refusal of a path this server publishes nothing at. It carries
/// nothing from the request: what a client branches on is the status and the type urn.
const NO_SUCH_RESOURCE: &str = "This ACME server has no such resource";

/// The detail of a refusal of a URL no ACTIVE ACME endpoint serves.
///
/// One answer for an endpoint that does not exist, one that is still `CREATING`, and
/// one that is `DELETING` or `FAILED`, so probing the data plane tells a caller nothing
/// about another account's endpoints: "after creation, ACME clients can use the
/// endpoint URL" (`CreateAcmeEndpoint` doc), so before that there is nothing here.
const NO_ACTIVE_ENDPOINT: &str = "No active ACME endpoint serves this URL";

/// The detail of a failure to reach this service's own store.
const STORE_UNAVAILABLE: &str = "The ACM ACME endpoint store is unavailable";

/// The detail of a refusal of a body that declares a media type this wire does not carry.
const UNSUPPORTED_MEDIA_TYPE: &str = "An ACME request body is application/jose+json";

/// The detail of a refusal of a JWS whose `url` header is not the URL the request was
/// sent to (RFC 8555 §6.4).
const URL_MISMATCH: &str =
    "The JWS url header does not address the resource this request was sent to";

/// The detail of a refusal of a JWS that identifies its key the wrong way round: a
/// registration that names an account, or a request to an account that embeds a key.
const KEY_IDENTIFICATION: &str =
    "A newAccount request is signed under jwk and every other request under kid";

/// The detail of a refusal of a key no signature can be checked under here.
const UNSUPPORTED_KEY: &str = "The JWS jwk header is not a public key of a supported type";

/// The detail of a refusal of a JWS whose signature does not check out under the key the
/// request identifies.
const SIGNATURE_UNVERIFIED: &str = "The JWS signature does not verify under its own key";

/// The detail of a refusal of a JWS signed with an algorithm this server cannot check.
const SIGNATURE_ALGORITHM: &str =
    "The JWS alg is not one this ACME server checks signatures under";

/// The detail of a refusal of a nonce this endpoint did not issue, or has already seen
/// back (RFC 8555 §6.5).
const UNKNOWN_NONCE: &str = "The JWS nonce is not one this ACME server issued and is still holding";

/// The detail of a refusal of a request to an account URL signed under another account.
const ACCOUNT_MISMATCH: &str = "The JWS kid is not the account this request addresses";

/// The detail of a refusal of a request naming an account this endpoint does not hold.
const NO_SUCH_ACCOUNT: &str = "This ACME server has no such account";

/// The detail of a refusal of a request from an account that is no longer valid — one
/// `RevokeAcmeAccount` revoked, or one its own client deactivated.
const ACCOUNT_NOT_VALID: &str = "This ACME account is no longer valid";

/// The detail of the refusal of an `onlyReturnExisting` registration for a key no account
/// holds (RFC 8555 §7.3.1).
const NO_ACCOUNT_FOR_KEY: &str = "This ACME server has no account registered with that key";

/// The detail of a refusal of a payload that is not the JSON object its interaction
/// carries.
const PAYLOAD_NOT_AN_OBJECT: &str = "The JWS payload is not a JSON object";

/// The detail of a refusal of a registration past the accounts an endpoint may hold.
const TOO_MANY_ACCOUNTS: &str = "This ACME endpoint holds as many accounts as it may";

/// The detail of a refusal of a request that asks for an account status a client may not
/// set (RFC 8555 §7.3.6).
const UNSUPPORTED_STATUS: &str = "An ACME client may only set an account's status to deactivated";

/// The detail of a refusal of a registration that presented no external account binding.
const BINDING_REQUIRED: &str =
    "This ACME server registers an account only with an external account binding";

/// The detail of a refusal of an `externalAccountBinding` member that is not a MAC-signed
/// JWS (RFC 8555 §7.3.4).
const BINDING_MALFORMED: &str = "The externalAccountBinding is not a MAC-signed JWS";

/// The detail of a refusal of an external account binding signed with another MAC
/// algorithm.
const BINDING_ALGORITHM_UNSUPPORTED: &str =
    "An external account binding is signed with HS256";

/// The detail of a refusal of an external account binding whose `url` is not the
/// registration's own.
const BINDING_URL_MISMATCH: &str =
    "The externalAccountBinding url must be the url of the registration";

/// The detail of a refusal of an external account binding bound to another key than the
/// one registering.
const BINDING_KEY_MISMATCH: &str =
    "The externalAccountBinding payload must be the account key being registered";

/// The status an ACME server refuses a registration whose external account binding does
/// not AUTHENTICATE with.
///
/// The credential the registration presents is read and judged, and found not to carry
/// the authority to register: RFC 9110 §15.5.4 words that verdict as `403 (Forbidden)`,
/// and the recorded behaviour of ACM's ACME endpoints grades the RFC 8555 §7.3.4
/// verification failure at that status. A binding whose FORM is wrong (another MAC
/// algorithm, another url, another account key) is a defect in the request rather than a
/// credential that was rejected, so it stays `malformed` at 400.
const BINDING_UNAUTHORIZED_STATUS: u16 = 403;

/// The detail of a refusal of an external account binding this endpoint issued no
/// credentials for.
const BINDING_UNKNOWN: &str =
    "The externalAccountBinding kid is not a key id this ACME endpoint issued";

/// The detail of a refusal of an external account binding whose authority has been
/// withdrawn.
const BINDING_WITHDRAWN: &str =
    "The external account binding these credentials belong to is no longer in force";

/// The detail of a refusal of an external account binding whose MAC does not check out.
const BINDING_MAC_UNVERIFIED: &str =
    "The externalAccountBinding MAC does not verify under the key id it names";

/// The detail of a refusal of a registration with no contact, on an endpoint whose
/// `Contact` is `REQUIRED`.
const CONTACT_REQUIRED: &str = "This ACME endpoint requires contact information";

/// The detail of a refusal of a `contact` member that is not a list.
const CONTACT_NOT_A_LIST: &str = "The contact member is a list of URIs";

/// The detail of a refusal of a `contact` entry that is not a URI.
const CONTACT_NOT_A_URI: &str = "Every contact entry is a non-empty URI";

/// The detail of a refusal of a contact URI longer than this server reads.
const CONTACT_TOO_LONG: &str = "A contact URI is longer than this ACME server accepts";

/// The detail of a refusal of a `contact` list longer than this server accepts.
const TOO_MANY_CONTACTS: &str = "More contact URIs than this ACME server accepts";

/// The detail of a refusal of a contact URI whose scheme this server does not support.
const CONTACT_SCHEME: &str = "This ACME server supports mailto contact URIs";

/// The scheme a contact URI carries — "the 'mailto' scheme" of RFC 8555 §7.3.
const MAILTO_SCHEME: &str = "mailto:";

/// The most contact URIs one account may register.
///
/// A CHOSEN bound, as RFC 8555 publishes none: it caps what one unauthenticated request
/// can store on an account record.
const MAX_CONTACTS: usize = 16;

/// The longest contact URI one account may register, in bytes — chosen for the same
/// reason as [`MAX_CONTACTS`], and above every mailbox address a client would register.
const MAX_CONTACT_LEN: usize = 320;

/// The `contact` member of an account resource and of a registration (RFC 8555 §7.1.2).
const CONTACT_MEMBER: &str = "contact";

/// The `status` member of an account resource (RFC 8555 §7.1.2).
const STATUS_MEMBER: &str = "status";

/// The `externalAccountBinding` member of a registration (RFC 8555 §7.3.4).
const EXTERNAL_ACCOUNT_BINDING_MEMBER: &str = "externalAccountBinding";

/// The MAC algorithm an external account binding is signed with: the one
/// `GetAcmeExternalAccountBindingCredentials` mints a key for, and the `HS256` of
/// RFC 7518 §3.2.
const BINDING_ALGORITHM: &str = "HS256";

/// `status` of a valid account on the ACME wire (RFC 8555 §7.1.2).
const ACME_STATUS_VALID: &str = "valid";

/// `status` of a deactivated account on the ACME wire (RFC 8555 §7.1.2, §7.3.6).
const ACME_STATUS_DEACTIVATED: &str = "deactivated";

/// `status` of a revoked account on the ACME wire (RFC 8555 §7.1.2).
const ACME_STATUS_REVOKED: &str = "revoked";

/// `AcmeEndpoint$Contact` of an endpoint whose clients must register contact information
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeContact")` declares
/// `REQUIRED | NOT_REQUIRED`).
const ACME_CONTACT_REQUIRED: &str = "REQUIRED";

/// The auth schemes a request to a published ACME URL that PRESENTS a credential is
/// verified under.
///
/// SigV4 is the only scheme this service's model declares, and ACM hands out no
/// data-plane credential of another kind. This is NOT a modelled operation, so there is
/// no modelled operation for the router to read schemes off: without this the router
/// would find no scheme for such a request and could not verify the credential it
/// carries.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// One endpoint's ACME HTTP surface, as the one [`ErasedOperation`] the service holds
/// for it.
///
/// Owns the shared engine exactly as a generated operation does, so every answer is
/// computed from the SAME per-service store the control plane's handlers write: the
/// directory of an endpoint is served only while that endpoint's own record says it is
/// `ACTIVE`, and an endpoint `DeleteAcmeEndpoint` removed serves nothing.
pub struct AcmeHttpSurface<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> AcmeHttpSurface<Eng> {
    /// Build the ACME surface from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for AcmeHttpSurface<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req) })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> AcmeHttpSurface<Eng> {
    /// Serve one request to a published ACME URL.
    ///
    /// The host names the Region, the path names the endpoint and the resource, and the
    /// method names the interaction — which is how an ACME server is addressed; there
    /// is no operation name on this wire.
    pub fn serve(&self, req: &RtRequest) -> RtResponse {
        // Which endpoint's resource this request addresses, and the address it reached
        // this surface on — the Region's published enrollment host, or the emulator's own
        // endpoint. Every URL the answer carries is minted on that same address.
        let Some(addressed) = addressed_acme_resource(req) else {
            return problem(404, MALFORMED, NO_SUCH_RESOURCE);
        };
        let (region, route, local) = (
            addressed.region.as_str(),
            &addressed.route,
            addressed.local_endpoint.as_deref(),
        );
        let Some(resource) = resource_of(&route.resource) else {
            return problem(404, MALFORMED, NO_SUCH_RESOURCE);
        };
        let Some(store) = state::service_state(&*self.engine) else {
            return problem(500, SERVER_INTERNAL, STORE_UNAVAILABLE);
        };
        // ONE instant answers the whole request: it materialises the transitions the
        // clock already owes and dates whatever this interaction records.
        let now = SystemTime::now();
        let mut store = store.lock();
        // Serving an endpoint's ACME surface means serving it from the endpoint's own
        // record, up to date with the clock: the `CREATING` -> `ACTIVE` transition the
        // create owed it was due at this instant whether or not anybody called, which
        // is what makes a client that waited for activation able to enrol.
        store.materialize_due(now);
        let serving = store
            .acme_endpoint_in(region, &route.endpoint_id)
            .is_some_and(|endpoint| endpoint.status == state::ACME_ENDPOINT_STATUS_ACTIVE);
        if !serving {
            return problem(404, MALFORMED, NO_ACTIVE_ENDPOINT);
        }

        let directory_url = state::acme_endpoint_url_at(local, region, &route.endpoint_id);
        match (req.method(), resource) {
            // The directory is the one resource an ACME client fetches with a plain
            // `GET` (RFC 8555 §7.1.1); everything else on this wire is a JWS `POST`.
            ("GET", AcmeResource::Directory) => directory(local, region, &route.endpoint_id),
            // `newNonce`: "the server MUST respond with a 200 (OK) to HEAD requests and
            // 204 (No Content) to GET requests" (RFC 8555 §7.2).
            ("HEAD", AcmeResource::NewNonce) => {
                nonce(200, &mut store, region, &route.endpoint_id, &directory_url)
            }
            ("GET", AcmeResource::NewNonce) => {
                nonce(204, &mut store, region, &route.endpoint_id, &directory_url)
            }
            ("POST", AcmeResource::Jws) => {
                let mut answer = jws_interaction(&mut store, &addressed, req, now);
                // "The server MUST include a Replay-Nonce header field in every
                // successful response to a POST request and SHOULD provide it in error
                // responses as well" (RFC 8555 §6.5) — and MUST in a `badNonce` one,
                // which is what lets the client retry. The nonce handed back is recorded
                // as issued by this endpoint, so the next POST can spend it.
                issue_nonce(&mut answer, &mut store, region, &route.endpoint_id);
                index_link(&mut answer, &directory_url);
                answer
            }
            (_, resource) => method_not_allowed(resource),
        }
    }
}

/// Which interaction a JWS `POST` on this surface addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Interaction {
    /// `newAccount`: a registration (RFC 8555 §7.3).
    NewAccount,
    /// An account's own URL, carrying that URL as this service minted it
    /// (RFC 8555 §7.3.2).
    Account(String),
}

/// Serve one JWS `POST` to a resource of an `ACTIVE` endpoint.
///
/// The checks are in the order the module docs state, and every refusal is an ACME
/// problem document: the caller decorates the answer with the fresh nonce and the index
/// link every response on this wire carries.
fn jws_interaction(
    store: &mut state::ComAmazonawsAcmCertificatemanagerState,
    addressed: &AddressedAcmeResource,
    req: &RtRequest,
    now: SystemTime,
) -> RtResponse {
    let (local, region, route) = (
        addressed.local_endpoint.as_deref(),
        addressed.region.as_str(),
        &addressed.route,
    );
    let endpoint_arn = state::acme_endpoint_arn(region, &route.endpoint_id);
    let addressed_url =
        state::acme_resource_url_at(local, region, &route.endpoint_id, &route.resource);
    // Which interaction this is. A resource this emulator performs no interaction for is
    // refused BEFORE the request is read, so each stays the one gap it is rather than
    // becoming a verdict on the client's JWS.
    let interaction = match route.resource.as_str() {
        state::ACME_NEW_ACCOUNT_RESOURCE => Interaction::NewAccount,
        other => match account_url_resource(other) {
            Some(_) => Interaction::Account(addressed_url.clone()),
            None => return not_implemented(&route.resource, &addressed_url),
        },
    };

    // "The body of the POST MUST be a JWS object" sent as `application/jose+json`
    // (RFC 8555 §6.2), so a client that declares any other media type is refused the way
    // RFC 9110 §15.5.16 words it rather than having its body read as something else.
    if let Some(declared) = content_type(req) {
        if !is_jose_json(declared) {
            return problem(415, MALFORMED, UNSUPPORTED_MEDIA_TYPE);
        }
    }
    let jws = match Jws::read(req.body().bytes().unwrap_or_default()) {
        Ok(jws) => jws,
        Err(defect) => return problem(400, MALFORMED, &defect.to_string()),
    };

    // §6.4: "the 'url' header parameter MUST be present" and carries the URL the request
    // was sent to, which is what stops one signed request being replayed at another
    // resource.
    if !addresses_same_resource(&jws.url, &addressed_url) {
        return problem(400, MALFORMED, URL_MISMATCH);
    }

    // The key the signature is checked against: the one the header embeds, for a
    // registration, and otherwise the one the named account registered with. "The 'jwk'
    // field MUST be present in all newAccount requests … the 'kid' field MUST be present
    // in all other requests" (RFC 8555 §6.2).
    let signing_key = match (&jws.key, &interaction) {
        (SigningKey::Embedded(jwk), Interaction::NewAccount) => {
            match ::service_common::jwt::canonical_jwk(jwk) {
                Some(canonical) => canonical,
                None => return problem(400, MALFORMED, UNSUPPORTED_KEY),
            }
        }
        (SigningKey::Account(named), Interaction::Account(account_url)) => {
            // An account speaks for itself: a request to an account's URL is signed by
            // THAT account's key, so a `kid` naming another account is no authority here.
            if named != account_url {
                return problem(400, UNAUTHORIZED, ACCOUNT_MISMATCH);
            }
            match store.acme_account_for(&endpoint_arn, account_url) {
                Some(account) => account.public_key_jwk.clone(),
                None => return problem(400, ACCOUNT_DOES_NOT_EXIST, NO_SUCH_ACCOUNT),
            }
        }
        _ => return problem(400, MALFORMED, KEY_IDENTIFICATION),
    };
    let Ok(verification_key) = ::serde_json::from_str::<::serde_json::Value>(&signing_key) else {
        // Unreachable: the stored key is the canonical JWK this surface wrote.
        return problem(500, SERVER_INTERNAL, STORE_UNAVAILABLE);
    };
    if let Err(defect) = ::service_common::jwt::jwk_signature_checks_out(
        &jws.algorithm,
        &verification_key,
        jws.signing_input.as_bytes(),
        &jws.signature,
    ) {
        return match defect {
            // An algorithm this server cannot check leaves the question undecided, and
            // RFC 8555 §6.2 has its own type for it, carrying the ones it can.
            ::service_common::jwt::SignatureDefect::UnsupportedAlgorithm(_) => {
                bad_signature_algorithm()
            }
            _ => problem(400, MALFORMED, SIGNATURE_UNVERIFIED),
        };
    }

    // The standing of the account that signed it: a deactivated account may make no
    // further request (RFC 8555 §7.3.6), and `RevokeAcmeAccount` "prevent[s] it from
    // requesting or revoking certificates" (`RevokeAcmeAccount` doc). ACM answers that
    // refusal with the `unauthorized` problem type at 400, the status its ACME endpoints
    // are recorded refusing a request on this wire with. It is decided before the nonce
    // is spent, so a revoked account's request is refused for the reason that holds
    // however it was addressed.
    if let Interaction::Account(account_url) = &interaction {
        let standing = store
            .acme_account_for(&endpoint_arn, account_url)
            .map(|account| account.status.clone());
        if standing.as_deref() != Some(state::ACME_ACCOUNT_STATUS_VALID) {
            return problem(400, UNAUTHORIZED, ACCOUNT_NOT_VALID);
        }
    }

    // §6.5: "the server MUST consider each nonce valid only once", so the nonce is SPENT
    // here — a replay of the same request finds nothing to spend and is refused with the
    // type the RFC fixes, beside the fresh nonce the caller attaches.
    let spent = store
        .acme_endpoint_in_mut(region, &route.endpoint_id)
        .is_some_and(|endpoint| endpoint.spend_nonce(&jws.nonce));
    if !spent {
        return problem(400, BAD_NONCE, UNKNOWN_NONCE);
    }

    match interaction {
        Interaction::NewAccount => {
            new_account(store, addressed, &endpoint_arn, &addressed_url, &jws, now)
        }
        Interaction::Account(account_url) => {
            account_interaction(store, &endpoint_arn, &account_url, &jws)
        }
    }
}

/// Register an account, or report the one the presented key already holds (RFC 8555 §7.3).
fn new_account(
    store: &mut state::ComAmazonawsAcmCertificatemanagerState,
    addressed: &AddressedAcmeResource,
    endpoint_arn: &str,
    addressed_url: &str,
    jws: &Jws,
    now: SystemTime,
) -> RtResponse {
    let (local, region, route) = (
        addressed.local_endpoint.as_deref(),
        addressed.region.as_str(),
        &addressed.route,
    );
    let SigningKey::Embedded(account_key) = &jws.key else {
        // Unreachable: the caller established that a registration embeds its key.
        return problem(400, MALFORMED, KEY_IDENTIFICATION);
    };
    let (Some(thumbprint), Some(canonical)) = (
        ::service_common::jwt::jwk_thumbprint(account_key),
        ::service_common::jwt::canonical_jwk(account_key),
    ) else {
        return problem(400, MALFORMED, UNSUPPORTED_KEY);
    };
    // A registration carries a JSON object; a POST-as-GET is not a registration.
    let Some(payload) = jws.payload_object() else {
        return problem(400, MALFORMED, PAYLOAD_NOT_AN_OBJECT);
    };

    // "If the server already has an account registered with the provided account key,
    // then it MUST return … 200 (OK) and provide the URL of that account in the Location
    // header" (RFC 8555 §7.3). The account is found by its KEY, so a client that
    // registers twice holds one account — and one whose account is no longer valid is
    // refused here for the same reason a request to that account's URL is: an account
    // that has been revoked or deactivated is never handed back to its key, and a
    // registration cannot bring it back (§7.3.6 — "the server MUST NOT allow a
    // deactivated account to be reactivated").
    if let Some(existing) = store.acme_account_by_thumbprint(endpoint_arn, &thumbprint) {
        if existing.status != state::ACME_ACCOUNT_STATUS_VALID {
            return problem(400, UNAUTHORIZED, ACCOUNT_NOT_VALID);
        }
        return account_document(200, existing);
    }
    // "If a client sends such a request and an account does not exist, then the server
    // MUST return an error response with status code 400 (Bad Request) and type
    // 'urn:ietf:params:acme:error:accountDoesNotExist'" (RFC 8555 §7.3.1).
    if payload
        .get("onlyReturnExisting")
        .and_then(::serde_json::Value::as_bool)
        == Some(true)
    {
        return problem(400, ACCOUNT_DOES_NOT_EXIST, NO_ACCOUNT_FOR_KEY);
    }

    let contacts = match contacts_of(&payload) {
        Ok(contacts) => contacts,
        Err(refusal) => return *refusal,
    };
    // "Whether ACME clients must provide contact information during account
    // registration" (`AcmeEndpoint$Contact` doc) is the endpoint's own configuration, so
    // the registration is held to what the endpoint was created with.
    let contact_required = store
        .acme_endpoint_in(region, &route.endpoint_id)
        .is_some_and(|endpoint| endpoint.contact == ACME_CONTACT_REQUIRED);
    if contact_required && contacts.is_empty() {
        return problem(400, INVALID_CONTACT, CONTACT_REQUIRED);
    }

    // "ACME accounts are created when clients use external account binding credentials
    // to register" (`ListAcmeAccounts` doc), so a registration that presents no binding
    // this endpoint issued registers nothing.
    let binding_arn = match external_account_binding(
        store,
        endpoint_arn,
        addressed_url,
        &payload,
        account_key,
        now,
    ) {
        Ok(arn) => arn,
        Err(refusal) => return *refusal,
    };

    let account = state::AcmeAccountRecord {
        acme_endpoint_arn: endpoint_arn.to_string(),
        // Server-assigned, under the same endpoint base URL the directory the client
        // fetched is on, and carrying nothing the client sent.
        account_url: state::acme_account_url_at(
            local,
            region,
            &route.endpoint_id,
            &::uuid::Uuid::new_v4().to_string(),
        ),
        public_key_thumbprint: thumbprint,
        public_key_jwk: canonical,
        // "The status of the ACME account" (`AcmeAccount$Status` doc): a registration is
        // valid from the moment it is accepted — RFC 8555 §7.1.2 knows no pending account
        // state — so the clock owes it nothing.
        status: state::ACME_ACCOUNT_STATUS_VALID.to_string(),
        acme_external_account_binding_arn: Some(binding_arn),
        contacts,
        created_at: now,
    };
    if !store.register_acme_account(account.clone()) {
        return problem(429, RATE_LIMITED, TOO_MANY_ACCOUNTS);
    }
    // "The server returns this account object in a 201 (Created) response, with the
    // account URL in a Location header field" (RFC 8555 §7.3).
    account_document(201, &account)
}

/// Serve a request an account makes to its OWN URL: a POST-as-GET read, a contact update
/// (RFC 8555 §7.3.2) or a deactivation (§7.3.6).
fn account_interaction(
    store: &mut state::ComAmazonawsAcmCertificatemanagerState,
    endpoint_arn: &str,
    account_url: &str,
    jws: &Jws,
) -> RtResponse {
    // A POST-as-GET reads the account and changes nothing (RFC 8555 §6.3).
    if jws.is_post_as_get() {
        return match store.acme_account_for(endpoint_arn, account_url) {
            Some(account) => account_document(200, account),
            None => problem(400, ACCOUNT_DOES_NOT_EXIST, NO_SUCH_ACCOUNT),
        };
    }
    let Some(payload) = jws.payload_object() else {
        return problem(400, MALFORMED, PAYLOAD_NOT_AN_OBJECT);
    };
    // The members this server acts on are read BEFORE the record is touched, so a request
    // carrying one it cannot accept changes nothing. "The server MUST ignore any fields
    // in the request object that it does not recognize" (RFC 8555 §7.3.2), so anything
    // else the payload carries is left alone.
    let contacts = if payload.contains_key(CONTACT_MEMBER) {
        match contacts_of(&payload) {
            Ok(contacts) => Some(contacts),
            Err(refusal) => return *refusal,
        }
    } else {
        None
    };
    let deactivating = payload
        .get(STATUS_MEMBER)
        .and_then(::serde_json::Value::as_str)
        == Some(ACME_STATUS_DEACTIVATED);
    if payload.contains_key(STATUS_MEMBER) && !deactivating {
        // "The only valid value for the 'status' field of a request to deactivate an
        // account is 'deactivated'" (RFC 8555 §7.3.6), so no other status a client asks
        // for is served.
        return problem(400, MALFORMED, UNSUPPORTED_STATUS);
    }

    let Some(account) = store.acme_account_for_mut(endpoint_arn, account_url) else {
        return problem(400, ACCOUNT_DOES_NOT_EXIST, NO_SUCH_ACCOUNT);
    };
    if let Some(contacts) = contacts {
        // "The server MUST … update the account's contact information to the value
        // provided" (RFC 8555 §7.3.2): the member REPLACES what the account held, so a
        // later read reports the last update and nothing accumulates.
        account.contacts = contacts;
    }
    if deactivating {
        account.status = state::ACME_ACCOUNT_STATUS_DEACTIVATED.to_string();
    }
    account_document(200, account)
}

/// The external account binding a registration presented, as the ARN of the binding
/// record it authenticated against.
///
/// `Err` carries the refusal, boxed because an HTTP response is far larger than the ARN
/// the happy path reports: the registration presented none, presented one that is not a
/// MAC-signed JWS over the account key, or presented one whose MAC this endpoint's
/// credentials do not account for (RFC 8555 §7.3.4) — the last of which is a credential
/// this server rejected, answered at [`BINDING_UNAUTHORIZED_STATUS`].
fn external_account_binding(
    store: &mut state::ComAmazonawsAcmCertificatemanagerState,
    endpoint_arn: &str,
    addressed_url: &str,
    payload: &::serde_json::Map<String, ::serde_json::Value>,
    account_key: &::serde_json::Value,
    now: SystemTime,
) -> Result<String, Box<RtResponse>> {
    let Some(presented) = payload.get(EXTERNAL_ACCOUNT_BINDING_MEMBER) else {
        return Err(Box::new(problem(
            400,
            EXTERNAL_ACCOUNT_REQUIRED,
            BINDING_REQUIRED,
        )));
    };
    let Some(binding_jws) = ExternalAccountBinding::read(presented) else {
        return Err(Box::new(problem(400, MALFORMED, BINDING_MALFORMED)));
    };
    // §7.3.4 step 1: the MAC algorithm must be one the server issues keys for.
    if !binding_jws.algorithm.eq_ignore_ascii_case(BINDING_ALGORITHM) {
        return Err(Box::new(problem(
            400,
            MALFORMED,
            BINDING_ALGORITHM_UNSUPPORTED,
        )));
    }
    // §7.3.4 step 2: "the 'url' field … MUST be identical to the 'url' field of the
    // outer JWS", i.e. the newAccount URL this request was sent to.
    if !addresses_same_resource(&binding_jws.url, addressed_url) {
        return Err(Box::new(problem(400, MALFORMED, BINDING_URL_MISMATCH)));
    }
    // §7.3.4 step 3: "the payload … MUST be the account key being registered".
    if ::service_common::jwt::jwk_thumbprint(&binding_jws.account_key)
        != ::service_common::jwt::jwk_thumbprint(account_key)
    {
        return Err(Box::new(problem(400, MALFORMED, BINDING_KEY_MISMATCH)));
    }

    // The credentials of the binding the client named, read from the binding record this
    // endpoint holds — `GetAcmeExternalAccountBindingCredentials` reports the same pair,
    // so a client signs with what that operation handed it.
    let named = store
        .acme_external_account_bindings_of(endpoint_arn)
        .find(|binding| binding.key_id == binding_jws.key_id)
        .map(|binding| {
            (
                binding.arn.clone(),
                binding.mac_key.clone(),
                binding.is_revoked(),
                binding.expires_at,
            )
        });
    let Some((arn, mac_key, revoked, expires_at)) = named else {
        return Err(Box::new(problem(
            BINDING_UNAUTHORIZED_STATUS,
            UNAUTHORIZED,
            BINDING_UNKNOWN,
        )));
    };
    // A revocation withdraws the authority the credentials carried, and an expiry does
    // the same at the instant the binding was created to run out
    // (`AcmeExternalAccountBinding$RevokedAt` / `$ExpiresAt` docs).
    if revoked || expires_at.is_some_and(|expires_at| now >= expires_at) {
        return Err(Box::new(problem(
            BINDING_UNAUTHORIZED_STATUS,
            UNAUTHORIZED,
            BINDING_WITHDRAWN,
        )));
    }
    if !binding_mac_checks_out(&mac_key, &binding_jws) {
        return Err(Box::new(problem(
            BINDING_UNAUTHORIZED_STATUS,
            UNAUTHORIZED,
            BINDING_MAC_UNVERIFIED,
        )));
    }
    // "The time at which the external account binding was last used"
    // (`AcmeExternalAccountBinding$LastUsedAt` doc): registering with the credentials IS
    // that use, so the instant is recorded on the record the control plane reports.
    if let Some(binding) = store.acme_external_account_bindings.get_mut(&arn) {
        binding.last_used_at = Some(now);
    }
    Ok(arn)
}

/// Whether the binding's MAC checks out under the key this endpoint minted for it.
///
/// `GetAcmeExternalAccountBindingCredentials` hands the client the MAC key as TEXT — the
/// base64url encoding of the key octets ([`crate::state::ACME_MAC_KEY_LEN`]) — so those
/// octets are what a client signs with. A client that signs with the characters it was
/// handed instead is accepted too: both readings are of the SAME credential, so neither
/// admits anybody who does not hold it.
fn binding_mac_checks_out(mac_key: &str, binding: &ExternalAccountBinding) -> bool {
    let signed = binding.signing_input.as_bytes();
    let decoded = ::service_common::jwt::decode_base64url(mac_key);
    decoded.is_some_and(|octets| {
        ::service_common::jwt::hs256_checks_out(&octets, signed, &binding.signature)
    }) || ::service_common::jwt::hs256_checks_out(mac_key.as_bytes(), signed, &binding.signature)
}

/// The contact URIs a payload's `contact` member carries, held to what an ACME server
/// accepts: "the server SHOULD validate the contact URIs" and answers
/// `unsupportedContact` for a scheme it does not support (RFC 8555 §7.3).
///
/// `Err` carries the refusal, boxed for the reason [`external_account_binding`]'s is. A
/// payload naming no `contact` carries no contacts, which an endpoint whose `Contact` is
/// `NOT_REQUIRED` admits.
fn contacts_of(
    payload: &::serde_json::Map<String, ::serde_json::Value>,
) -> Result<Vec<String>, Box<RtResponse>> {
    let Some(presented) = payload.get(CONTACT_MEMBER) else {
        return Ok(Vec::new());
    };
    let Some(presented) = presented.as_array() else {
        return Err(Box::new(problem(400, INVALID_CONTACT, CONTACT_NOT_A_LIST)));
    };
    if presented.len() > MAX_CONTACTS {
        return Err(Box::new(problem(400, INVALID_CONTACT, TOO_MANY_CONTACTS)));
    }
    let mut contacts = Vec::with_capacity(presented.len());
    for contact in presented {
        let Some(contact) = contact
            .as_str()
            .map(str::trim)
            .filter(|contact| !contact.is_empty())
        else {
            return Err(Box::new(problem(400, INVALID_CONTACT, CONTACT_NOT_A_URI)));
        };
        if contact.len() > MAX_CONTACT_LEN {
            return Err(Box::new(problem(400, INVALID_CONTACT, CONTACT_TOO_LONG)));
        }
        // "The 'mailto' scheme … is the only scheme ACME servers are required to
        // support" (RFC 8555 §7.3), and it is the scheme an ACM ACME account's
        // `Contacts` are read back under.
        if !contact.to_ascii_lowercase().starts_with(MAILTO_SCHEME) {
            return Err(Box::new(problem(400, UNSUPPORTED_CONTACT, CONTACT_SCHEME)));
        }
        contacts.push(contact.to_string());
    }
    Ok(contacts)
}

/// One account resource as RFC 8555 §7.1.2 renders it, with the account's own URL in the
/// `Location` header a client takes its account URL from (§7.3).
///
/// Every member is read off the stored record, including the status, which travels on
/// this wire in the lower-case spelling §7.1.2 fixes — the same state
/// `DescribeAcmeAccount` reports in the model's own spelling.
fn account_document(status: u16, account: &state::AcmeAccountRecord) -> RtResponse {
    let mut document = ::serde_json::Map::new();
    document.insert(
        STATUS_MEMBER.to_string(),
        ::serde_json::Value::String(acme_account_status(&account.status).to_string()),
    );
    if !account.contacts.is_empty() {
        document.insert(
            CONTACT_MEMBER.to_string(),
            ::serde_json::Value::from(account.contacts.clone()),
        );
    }
    let mut answer = response(
        status,
        JSON_CONTENT_TYPE,
        ::serde_json::Value::Object(document).to_string(),
    );
    answer
        .headers_mut()
        .insert("location", account.account_url.clone());
    answer
}

/// An `AcmeAccountStatus` as the ACME wire spells it (RFC 8555 §7.1.2 registers `valid`,
/// `deactivated` and `revoked`).
fn acme_account_status(status: &str) -> &str {
    match status {
        state::ACME_ACCOUNT_STATUS_VALID => ACME_STATUS_VALID,
        state::ACME_ACCOUNT_STATUS_DEACTIVATED => ACME_STATUS_DEACTIVATED,
        state::ACME_ACCOUNT_STATUS_REVOKED => ACME_STATUS_REVOKED,
        // Unreachable: those three are every status this service's model declares.
        other => other,
    }
}

/// The refusal of a JWS signed with an algorithm this server cannot check, carrying the
/// ones it can — "the problem document MUST include an 'algorithms' field" (RFC 8555
/// §6.2).
fn bad_signature_algorithm() -> RtResponse {
    let mut extra = ::serde_json::Map::new();
    extra.insert(
        ALGORITHMS_MEMBER.to_string(),
        ::serde_json::Value::from(::service_common::jwt::VERIFIABLE_SIGNATURE_ALGORITHMS.to_vec()),
    );
    problem_document(400, BAD_SIGNATURE_ALGORITHM, SIGNATURE_ALGORITHM, extra)
}

/// Record a fresh nonce for the endpoint and state it on `answer`.
fn issue_nonce(
    answer: &mut RtResponse,
    store: &mut state::ComAmazonawsAcmCertificatemanagerState,
    region: &str,
    endpoint_id: &str,
) {
    let Some(endpoint) = store.acme_endpoint_in_mut(region, endpoint_id) else {
        // Unreachable: the endpoint was read as `ACTIVE` under this same lock.
        return;
    };
    let fresh = mint_nonce();
    endpoint.issue_nonce(fresh.clone());
    answer.headers_mut().insert(REPLAY_NONCE_FIELD, fresh);
}

/// The media type a request declares, if it declares one.
fn content_type(req: &RtRequest) -> Option<&str> {
    req.headers().get("content-type")
}

/// Whether a declared media type is the one an ACME JWS travels under, ignoring any
/// parameters and the case the client spelled it in (RFC 9110 §8.3).
fn is_jose_json(declared: &str) -> bool {
    declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case(crate::service::acme_jws::JOSE_CONTENT_TYPE)
}

/// Whether a URL a JWS states addresses the same resource as `addressed`: the same host
/// and the same path.
///
/// The scheme and any port are not compared, because they are properties of how the
/// client reached this server rather than of the resource it asked for; the HOST is, so a
/// request signed for one endpoint's resource cannot be presented at another's.
fn addresses_same_resource(stated: &str, addressed: &str) -> bool {
    addressed_resource(stated) == addressed_resource(addressed)
}

/// The `(host, path)` a URL addresses, host lower-cased and the path without a trailing
/// slash.
fn addressed_resource(url: &str) -> (String, String) {
    let authority = authority_of(url).unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    (
        host.to_ascii_lowercase(),
        request_path(url).trim_end_matches('/').to_string(),
    )
}

/// Which ACME resource of an endpoint a path's remainder names, or `None` when the
/// endpoint publishes no such resource.
///
/// The names are the ones [`crate::state::acme_resource_url`] mints the directory's
/// own URLs from, so what is recognised here is exactly what the directory advertises.
fn resource_of(resource: &str) -> Option<AcmeResource> {
    match resource {
        state::ACME_DIRECTORY_RESOURCE => Some(AcmeResource::Directory),
        state::ACME_NEW_NONCE_RESOURCE => Some(AcmeResource::NewNonce),
        state::ACME_NEW_ACCOUNT_RESOURCE
        | state::ACME_NEW_ORDER_RESOURCE
        | state::ACME_REVOKE_CERTIFICATE_RESOURCE
        | state::ACME_KEY_CHANGE_RESOURCE => Some(AcmeResource::Jws),
        // An account's own URL, which a client reaches with a POST-as-GET (RFC 8555
        // §7.3) — the same JWS wire the resources above are on.
        other => account_url_resource(other).map(|_| AcmeResource::Jws),
    }
}

/// The account id in an account resource path (`acct/<id>`), or `None` when the path is
/// not one [`crate::state::acme_account_url`] mints.
fn account_url_resource(resource: &str) -> Option<&str> {
    let id = resource.strip_prefix(state::ACME_ACCOUNT_RESOURCE)?;
    let id = id.strip_prefix('/')?;
    // An account id is minted by this service and carries no `/`, so a deeper path is
    // not an account URL.
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// One resource of an endpoint's ACME surface, as the HTTP wire addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcmeResource {
    /// The directory resource, fetched with `GET` (RFC 8555 §7.1.1).
    Directory,
    /// The `newNonce` resource (RFC 8555 §7.2).
    NewNonce,
    /// A resource a client reaches with a JWS `POST`: `newAccount`, `newOrder`,
    /// `revokeCert`, `keyChange`, or an account's own URL.
    Jws,
}

impl AcmeResource {
    /// The methods this resource is addressed with, as the `Allow` header of a refusal
    /// of any other method lists them (RFC 9110 §15.5.6 requires that header).
    fn allowed_methods(self) -> &'static str {
        match self {
            Self::Directory => "GET",
            Self::NewNonce => "HEAD, GET",
            Self::Jws => "POST",
        }
    }
}

/// The ACME directory resource of one endpoint: the map of the resources a client
/// starts from (RFC 8555 §7.1.1).
///
/// Every URL in it is built by [`crate::state::acme_resource_url_at`] from the endpoint's
/// own id and Region at the address the client fetched this document from — the same
/// function that minted the `EndpointUrl` it was handed — so the map a client follows
/// stays on the address this service answered it on, and nothing in it comes from the
/// request.
///
/// The five members RFC 8555 §7.1.1 requires are present. The optional ones are not
/// advertised: `newAuthz` belongs to pre-authorization, which no modelled operation
/// configures, and `meta` (terms of service, website, CAA identities, whether an
/// external account binding is required) would be claims about this CA that neither
/// ACM's model nor its documentation states.
fn directory(local: Option<&str>, region: &str, endpoint_id: &str) -> RtResponse {
    let resource = |name: &str| state::acme_resource_url_at(local, region, endpoint_id, name);
    let document = ::serde_json::json!({
        "newNonce": resource(state::ACME_NEW_NONCE_RESOURCE),
        "newAccount": resource(state::ACME_NEW_ACCOUNT_RESOURCE),
        "newOrder": resource(state::ACME_NEW_ORDER_RESOURCE),
        "revokeCert": resource(state::ACME_REVOKE_CERTIFICATE_RESOURCE),
        "keyChange": resource(state::ACME_KEY_CHANGE_RESOURCE),
    });
    response(200, JSON_CONTENT_TYPE, document.to_string())
}

/// The HTTP field the nonce is carried in, under the canonical name RFC 8555 §7.2 spells
/// it with — the name an ACME client reads the nonce back by, and the one this service's
/// answer states. HTTP field names are case-insensitive, so what a client matches is this
/// name however the transport that writes it cases the bytes.
const REPLAY_NONCE_FIELD: &str = "Replay-Nonce";

/// The `newNonce` answer: an empty body carrying a fresh nonce.
///
/// RFC 8555 §7.2 fixes all three headers: the `Replay-Nonce` field carries the nonce,
/// the response "MUST include a Cache-Control header field with the no-store
/// directive", and §7.1 asks for the `Link` field pointing at the directory.
///
/// The nonce is RECORDED on the endpoint, which is what this resource DOES: a nonce
/// nothing remembers is one no later POST could be checked against, so issuing it is the
/// write the spec mandates rather than a side effect of a read.
fn nonce(
    status: u16,
    store: &mut state::ComAmazonawsAcmCertificatemanagerState,
    region: &str,
    endpoint_id: &str,
    directory_url: &str,
) -> RtResponse {
    let mut answer = response(status, JSON_CONTENT_TYPE, String::new());
    issue_nonce(&mut answer, store, region, endpoint_id);
    answer.headers_mut().insert("cache-control", "no-store");
    index_link(&mut answer, directory_url);
    answer
}

/// A fresh anti-replay nonce.
///
/// 128 bits of fresh randomness per call, so no two requests are answered the same
/// nonce, rendered as hex — which is within the base64url alphabet RFC 8555 §6.5.1
/// requires a nonce to be spelled in, and whose length is a multiple of four, so a
/// client that base64url-decodes what it was handed can.
fn mint_nonce() -> String {
    ::hex::encode(::uuid::Uuid::new_v4().as_bytes())
}

/// The refusal of an ACME interaction this emulator does not implement.
///
/// `501` with a problem document, never an empty `200`: ordering a certificate, revoking
/// one and rolling an account key over each mint or withdraw something this emulator
/// issues nothing for, so there is no order, certificate or key change behind such a
/// call and a success would report an interaction nothing performed.
fn not_implemented(resource: &str, directory_url: &str) -> RtResponse {
    let detail = format!(
        "This ACME server does not implement the {} interaction yet",
        bounded(resource)
    );
    let mut answer = problem(501, SERVER_INTERNAL, &detail);
    index_link(&mut answer, directory_url);
    answer
}

/// The refusal of a method the addressed resource is not reached with, naming the
/// methods it is (RFC 9110 §15.5.6).
fn method_not_allowed(resource: AcmeResource) -> RtResponse {
    let detail = format!(
        "This ACME resource is addressed with {}",
        resource.allowed_methods()
    );
    let mut answer = problem(405, MALFORMED, &detail);
    answer
        .headers_mut()
        .insert("allow", resource.allowed_methods());
    answer
}

/// Add the `Link` field pointing at the endpoint's directory, which RFC 8555 §7.1 asks
/// every response on this wire to carry so a client can always find the resource map.
fn index_link(answer: &mut RtResponse, directory_url: &str) {
    answer
        .headers_mut()
        .insert("link", format!("<{directory_url}>;rel=\"index\""));
}

/// An ACME failure, as the problem document an ACME server answers with (RFC 8555
/// §6.7): the `type` urn a client branches on and a `detail` sentence for a human.
fn problem(status: u16, problem_type: &str, detail: &str) -> RtResponse {
    problem_document(status, problem_type, detail, ::serde_json::Map::new())
}

/// The same, with the members a particular problem type carries beside those three — the
/// `algorithms` of a `badSignatureAlgorithm` document (RFC 8555 §6.2).
fn problem_document(
    status: u16,
    problem_type: &str,
    detail: &str,
    extra: ::serde_json::Map<String, ::serde_json::Value>,
) -> RtResponse {
    let mut body = ::serde_json::Map::new();
    body.insert(
        "type".to_string(),
        ::serde_json::Value::String(problem_type.to_string()),
    );
    body.insert(
        "detail".to_string(),
        ::serde_json::Value::String(detail.to_string()),
    );
    body.insert("status".to_string(), ::serde_json::Value::from(status));
    body.extend(extra);
    response(
        status,
        PROBLEM_CONTENT_TYPE,
        ::serde_json::Value::Object(body).to_string(),
    )
}

/// One response on this wire: the status, the media type, and the body.
fn response(status: u16, content_type: &str, body: String) -> RtResponse {
    let mut answer = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    answer
        .headers_mut()
        .insert("content-type", content_type.to_string());
    answer
}

/// The longest run of a request's own path this surface quotes back in a message.
const MAX_QUOTED_LEN: usize = 64;

/// `value` as a message quotes it: ASCII graphic characters only and at most
/// [`MAX_QUOTED_LEN`] of them, so a message cannot carry control characters or an
/// unbounded run of a caller's own text.
fn bounded(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(MAX_QUOTED_LEN)
        .collect()
}

/// Whether `req` addresses this service's ACME surface: the hostnames this service
/// PUBLISHES in `AcmeEndpoint$EndpointUrl` for a client that resolves AWS's own hosts, or
/// the reserved path the same surface is published at on the emulator's OWN endpoint.
pub fn addresses_acme_surface(req: &RtRequest) -> bool {
    addresses_acme_surface_at(req.uri(), addressed_host_of_request(req))
}

/// [`addresses_acme_surface`] for the buffered request parts `Service::can_handle` is
/// handed, read by the same rule, so the claim and the answer cannot disagree about which
/// requests this surface serves.
pub fn addresses_acme_surface_of_parts(parts: &http::request::Parts) -> bool {
    addresses_acme_surface_at(&parts.uri.to_string(), addressed_host_of_parts(parts))
}

/// The one reading of that claim: the ACME plane's reserved path, whatever the host
/// (which is how the surface is addressed on the emulator's own endpoint), else an
/// enrollment host, whatever the path — a request to either is this service's to answer,
/// including one whose path names no resource it publishes, which is answered with an
/// ACME problem document rather than the front door's "unknown service".
fn addresses_acme_surface_at(uri: &str, host: Option<&str>) -> bool {
    state::addresses_acme_local_plane(request_path(uri))
        || host.and_then(state::acme_enrollment_region).is_some()
}

/// What an ACME request addresses: the endpoint and resource it names, the Region that
/// endpoint lives in, and the emulator's own endpoint it reached this surface on — `None`
/// for a request addressed to the Region's published enrollment host, which is the
/// address a client that resolves AWS's own hostnames holds.
#[derive(Debug)]
struct AddressedAcmeResource {
    region: String,
    route: state::AcmeRequestPath,
    local_endpoint: Option<String>,
}

/// Which endpoint's ACME resource `req` addresses, or `None` when it addresses none.
///
/// The two addressings are the ones this surface publishes
/// ([`crate::state::acme_resource_url_at`]): the reserved path on the emulator's own
/// endpoint, read first because it names this plane explicitly whatever the host is, and
/// the Region's enrollment host with the endpoint's id as the first path segment.
fn addressed_acme_resource(req: &RtRequest) -> Option<AddressedAcmeResource> {
    let host = addressed_host_of_request(req);
    let path = request_path(req.uri());
    if let Some((region, route)) = state::parse_acme_local_path(path) {
        return Some(AddressedAcmeResource {
            region,
            route,
            local_endpoint: ::service_common::data_plane_addressing::reached_emulator_endpoint(
                req.uri(),
                host,
            ),
        });
    }
    Some(AddressedAcmeResource {
        region: host.and_then(state::acme_enrollment_region)?.to_string(),
        route: state::parse_acme_request_path(path)?,
        local_endpoint: None,
    })
}

/// The host a runtime-API request addressed: its `Host` header, else the authority an
/// absolute-form URI carries.
fn addressed_host_of_request(req: &RtRequest) -> Option<&str> {
    req.headers()
        .get("host")
        .or_else(|| authority_of(req.uri()))
}

/// The host a buffered request addressed.
///
/// Reads the `Host` header an HTTP/1 request carries, else the authority an HTTP/2
/// request carries on its URI (`http` parses `:authority` onto the URI rather than
/// exposing it as a header).
fn addressed_host_of_parts(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization`
/// header a signed SDK request carries, or in the `X-Amz-Credential` query parameter a
/// presigned URL carries.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    request_query(req.uri())
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan
    // over the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The query component of a request URI: everything after the first `?`.
fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}
