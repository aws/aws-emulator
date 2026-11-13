// SPDX-License-Identifier: CC0-1.0

//! The `Location` URI a create announces, end to end over HTTP.
//!
//! RULE: the `Location` header a create reports is an address the CALLER then follows, so it
//! is rooted at the endpoint this emulator answered that create on — scheme, host and port
//! — with the new resource's own path beneath it. A request that reached the emulator's own
//! endpoint is answered that same address, port included, because that is the only address a
//! client on this machine can dial; a caller that resolves AWS's own hostnames (a request
//! addressed to one of this service's published hosts, on an emulator served at no address
//! of its own) is answered this service's published endpoint, which is the address such a
//! caller resolves. A URI rooted anywhere else sends the client that follows it away from
//! the emulator, where the resource it names does not exist.
//!
//! The rows span what the rule generalises over: the host the request addressed (the
//! emulator's own endpoint as a loopback LITERAL and as a loopback NAME, each on a port of
//! its own, against this service's global host and its regional spelling) crossed with the
//! creates that announce a `Location` — four of them, minting identifiers in their own
//! shapes (`E`-prefixed tokens, a UUID, an `I`-prefixed token) and one of them under a
//! collection NESTED in another resource. Each announced URI is then FOLLOWED on the host it
//! was announced to, which is the whole point of the header: the resource the create just
//! made answers there.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds; the one addition is its request helper
//! that takes the `Host` a request carries.

use crate::distribution_lifecycle::{
    batch_xml, call_from_host, config, create_distribution, emulator, Answer, API_VERSION, HOST,
};

/// This service's own published endpoint, which is what a caller that resolves AWS's own
/// hostnames is announced on an emulator served at no address of its own
/// (`service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::SERVICE_ENDPOINT`, read
/// through the harness's own constant so one spelling serves both).
const SERVICE_HOST: &str = HOST;

/// One host a request below is addressed to, and the authority the `Location` it is answered
/// with has to be rooted at.
struct Addressed {
    case: &'static str,
    /// The `Host` the request carries.
    host: &'static str,
    /// The authority the announced URI must name.
    authority: &'static str,
    /// Whether that authority is the emulator's OWN endpoint, which is the case where
    /// following the announced URI has to reach the resource over this same harness.
    own_endpoint: bool,
}

/// The hosts every create below is issued from.
fn addressed_hosts() -> Vec<Addressed> {
    vec![
        Addressed {
            case: "the emulator's own loopback endpoint",
            host: "127.0.0.1:39461",
            authority: "127.0.0.1:39461",
            own_endpoint: true,
        },
        Addressed {
            case: "the same endpoint by its loopback name on another port",
            host: "localhost:9297",
            authority: "localhost:9297",
            own_endpoint: true,
        },
        Addressed {
            case: "this service's own global host",
            host: SERVICE_HOST,
            authority: SERVICE_HOST,
            own_endpoint: false,
        },
        Addressed {
            case: "this service's host in a regional spelling",
            host: "cloudfront.us-east-1.amazonaws.com",
            authority: SERVICE_HOST,
            own_endpoint: false,
        },
    ]
}

/// One create that announces a `Location`, as a row issues it.
struct Create {
    case: &'static str,
    /// The collection path the new resource lives under, between the API version and the
    /// identifier the `Location` ends at.
    collection: String,
    /// The request body.
    body: String,
    /// A value the REQUEST supplied that the `Location` must not address the resource by.
    caller_input: String,
}

/// A minimal `CloudFrontOriginAccessIdentityConfig`: both members are required on the wire.
fn origin_access_identity_xml(reference: &str) -> String {
    format!(
        "<CloudFrontOriginAccessIdentityConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Comment>announced location</Comment>\
         </CloudFrontOriginAccessIdentityConfig>"
    )
}

/// A minimal `ResponseHeadersPolicyConfig`: `Name` is its only required member.
fn response_headers_policy_xml(name: &str) -> String {
    format!(
        "<ResponseHeadersPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         </ResponseHeadersPolicyConfig>"
    )
}

/// The creates one row issues, each under identifiers of its own so no two rows collide on
/// a caller reference or a name.
///
/// `distribution` is the id the nested create addresses its collection under — the
/// invalidation route is `/distribution/{Id}/invalidation`, so the row that exercises a
/// NESTED collection needs a distribution to nest it in.
fn creates(row: usize, distribution: &str) -> Vec<Create> {
    vec![
        Create {
            case: "CreateDistribution",
            collection: "distribution".to_string(),
            body: config("announced location", &format!("ref-location-{row}")).xml(),
            caller_input: format!("ref-location-{row}"),
        },
        Create {
            case: "CreateCloudFrontOriginAccessIdentity",
            collection: "origin-access-identity/cloudfront".to_string(),
            body: origin_access_identity_xml(&format!("oai-location-{row}")),
            caller_input: format!("oai-location-{row}"),
        },
        Create {
            case: "CreateResponseHeadersPolicy",
            collection: "response-headers-policy".to_string(),
            body: response_headers_policy_xml(&format!("policy-location-{row}")),
            caller_input: format!("policy-location-{row}"),
        },
        Create {
            case: "CreateInvalidation",
            collection: format!("distribution/{distribution}/invalidation"),
            body: batch_xml(&format!("invalidation-location-{row}"), &["/index.html"]),
            caller_input: format!("invalidation-location-{row}"),
        },
    ]
}

/// The `Id` the created resource reports, which is what its `Location` has to end at.
fn created_id(answer: &Answer, case: &str) -> String {
    answer
        .root()
        .find_text("Id")
        .unwrap_or_else(|| panic!("case {case}: the create carries no Id: {}", answer.raw))
        .to_string()
}

/// Every create announces its new resource at the endpoint the request reached, and nowhere
/// else: the host (and port) is the one the request addressed when that host is the
/// emulator's own, and this service's published endpoint for a caller that resolves AWS's
/// own hostnames. The path beneath it is the resource's own, ending at the identifier the
/// SERVICE generated — never at a value the request supplied — and following it on the host
/// it was announced to reaches the resource the create just made.
#[tokio::test]
async fn a_create_announces_the_endpoint_the_request_reached() {
    let router = emulator();
    for (row, addressed) in addressed_hosts().into_iter().enumerate() {
        let Addressed {
            case: host_case,
            host,
            authority,
            own_endpoint,
        } = addressed;
        // The distribution the nested create below nests its collection in, made over the
        // same host so one row is one client throughout.
        let host_distribution = create_distribution(
            &router,
            &config("the distribution an invalidation belongs to", &format!("ref-host-{row}")).xml(),
        )
        .await;
        let distribution_id = created_id(&host_distribution, host_case);

        for create in creates(row, &distribution_id) {
            let case = format!("{host_case} / {}", create.case);
            let answer = call_from_host(
                &router,
                host,
                "POST",
                &format!("/{API_VERSION}/{}", create.collection),
                Some(&create.body),
                &[],
            )
            .await;
            assert_eq!(
                answer.status, 201,
                "case {case}: expected the create to be accepted, got {} with {}",
                answer.status, answer.raw
            );
            let id = created_id(&answer, &case);
            let path = format!("/{API_VERSION}/{}/{id}", create.collection);
            let location = answer.header("location").to_string();
            assert_eq!(
                location,
                format!("https://{authority}{path}"),
                "case {case}: expected the new resource's path rooted at the endpoint the \
                 request reached ({authority}), got {location:?}"
            );
            let segment = location.rsplit('/').next().unwrap_or_default().to_string();
            assert_ne!(
                segment, create.caller_input,
                "case {case}: Location must not address the resource by the request's own \
                 value {:?}",
                create.caller_input
            );

            // A resource whose read route is that path is REACHED by following the header,
            // on the very host it was announced to: that is what makes the announced
            // address usable rather than merely well formed. `CreateResponseHeadersPolicy`
            // and `CreateInvalidation` are read on their own paths too, so every row is
            // followed.
            let followed = call_from_host(&router, host, "GET", &path, None, &[]).await;
            assert_eq!(
                followed.status, 200,
                "case {case}: following {location:?} must reach the resource, got {} with {}",
                followed.status, followed.raw
            );
            let read_id = created_id(&followed, &case);
            assert_eq!(
                read_id, id,
                "case {case}: following Location must reach the created resource {id:?}, got \
                 {read_id:?}"
            );
            // The emulator's own endpoint is announced WITH the port the request addressed
            // it on, which is what makes the URI dialable from this machine; a caller that
            // resolves AWS's own hostnames is announced the published host, which carries
            // none.
            let announced_authority = location
                .strip_prefix("https://")
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            assert_eq!(
                announced_authority.contains(':'),
                own_endpoint,
                "case {case}: expected a port in the announced authority only for the \
                 emulator's own endpoint, got {announced_authority:?}"
            );
        }
    }
}
