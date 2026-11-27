// SPDX-License-Identifier: CC0-1.0

//! The EDGE ENFORCEMENTS a client meets at the one address a deployed distribution PUBLISHES,
//! over a Lambda function URL origin.
//!
//! RULE: `Distribution$DomainName` is "the distribution's CloudFront domain name" — the value a
//! viewer is handed and composes `https://<DomainName>/<object>` from — so a distribution this
//! control plane reports `Deployed` publishes there an address this emulator ANSWERS its edge
//! on (the one listener opened for that distribution), and every decision CloudFront makes for
//! itself at the edge is made at THAT address against the request that arrives there: the
//! matched behaviour's `AllowedMethods` decides whether the viewer's method is served at all,
//! the distribution's `Restrictions.GeoRestriction` whether the viewer's country is, and the
//! matched behaviour's `TrustedKeyGroups` whether the signed URL carrying the request holds —
//! its `AWS:SourceIp` condition against the address the edge itself observes for the
//! connection. A refusal is CloudFront's own `403` with `X-Cache: Error from cloudfront`, none
//! of the origin's bytes and NO origin request at all; an admission carries the origin's own
//! bytes. A name in a zone Amazon Web Services publishes its own endpoints in resolves nowhere
//! near the machine this emulator runs on, so a client handed one opens no connection at all
//! and not one of those enforcements is observable.
//!
//! So each row here holds ONLY what a create or a read reported — the published `DomainName` —
//! dials that value, states it as the host it addressed, and reads the verdict off the wire.
//! Whether the origin was reached is a fact of LAMBDA's own `Invocations` series for the
//! function behind the origin, so a refusal that quietly fetched the origin fails here too.
//!
//! The rows span what the rule generalises over: WHICH control decides (the method gate, the
//! geographic restriction, the trusted key group), which way it decides (a method the behaviour
//! lists and two it does not, a viewer the blocklist names and one it does not, a signature that
//! holds and three that do not), and — for the signed rows — the reason it does not hold (no
//! signature at all, a source-address condition naming another range, a key the group does not
//! carry). Every subject is a distribution of its own, so each one's published address is held
//! to being its own.
//!
//! The topology, the dial and the reading of one answer are [`crate::edge_lambda_url_origin`]'s
//! and the signer is [`crate::edge_signed_access`]'s — reused rather than copied.

use crate::distribution_lifecycle::{is_published_edge_address, API_VERSION};
use crate::edge_lambda_url_origin::{
    element, viewer_ip, Answered, Edge, Topology, ALL_VIEWER_AND_CLOUDFRONT, MISS, OPEN_PATH,
};
use crate::edge_signed_access::{
    create_key_group, custom_policy, custom_query, far_future, upload_public_key, Signer,
    CACHING_DISABLED, ERROR,
};

/// The path pattern of the behaviour that trusts a key group, and a path under it.
const PRIVATE_PATTERN: &str = "/private/*";
const PRIVATE_PATH: &str = "/private/object";

/// Two-letter uppercase country codes `GeoRestriction$Items` takes ("the `Location` element is a
/// two-letter, uppercase country code"), neither of which is the country the edge reports for a
/// viewer of this emulator: New Zealand and Brazil, as ISO 3166-1 alpha-2 assigns them. Each row
/// using them asserts that first, so a blocklist naming "elsewhere" really does name elsewhere.
const ELSEWHERE: [&str; 2] = ["NZ", "BR"];

/// A source-address range no loopback viewer of this emulator lies in, for the signed row whose
/// policy condition must not hold. RFC 1918 reserves it for private networks.
const ANOTHER_RANGE: &str = "10.0.0.0/8";

/// What one row expects of the edge at the published address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The origin answered it: a `200` carrying the origin's own bytes.
    Served,
    /// CloudFront refused it itself with `403`, under the error code it states for that
    /// refusal where the refusal carries one — a generated page (the method gate) carries
    /// none.
    Refused(Option<&'static str>),
}

/// One subject: a deployed distribution of its own, carrying the configuration whose control
/// the rows against it exercise.
struct Subject {
    case: &'static str,
    /// The distribution's id, which the published address is read back against.
    id: String,
    /// The published `DomainName` and the address it resolves to on this machine.
    edge: Edge,
}

/// One `DistributionConfig` over the function URL `origin`: a default behaviour listing `GET`
/// and `HEAD` alone, the managed origin request policy that forwards the `CloudFront-*` headers
/// (without which no origin reports the country or the address the edge observed), the
/// `restrictions` element this subject carries and the extra `behaviors` it carries.
fn config_xml(reference: &str, origin: &str, restrictions: &str, behaviors: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>function-url</Id><DomainName>{origin}</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         <OriginSslProtocols><Quantity>1</Quantity>\
         <Items><SslProtocol>TLSv1.2</SslProtocol></Items></OriginSslProtocols>\
         </CustomOriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>function-url</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></AllowedMethods>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         <OriginRequestPolicyId>{ALL_VIEWER_AND_CLOUDFRONT}</OriginRequestPolicyId>\
         </DefaultCacheBehavior>{behaviors}\
         <Comment>enforcement at the published domain name</Comment>\
         {restrictions}\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// The `Restrictions` element blocklisting `locations`, as `GeoRestriction` states one.
fn blocklist(locations: &[String]) -> String {
    let items: String = locations
        .iter()
        .map(|location| format!("<Location>{location}</Location>"))
        .collect();
    format!(
        "<Restrictions><GeoRestriction><RestrictionType>blacklist</RestrictionType>\
         <Quantity>{}</Quantity><Items>{items}</Items></GeoRestriction></Restrictions>",
        locations.len()
    )
}

/// The ordered behaviour that trusts `key_group`, which is what makes a signed URL the only way
/// through [`PRIVATE_PATTERN`].
fn trusted_behavior(key_group: &str) -> String {
    format!(
        "<CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
         <PathPattern>{PRIVATE_PATTERN}</PathPattern>\
         <TargetOriginId>function-url</TargetOriginId>\
         <TrustedKeyGroups><Enabled>true</Enabled><Quantity>1</Quantity>\
         <Items><KeyGroup>{key_group}</KeyGroup></Items></TrustedKeyGroups>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <AllowedMethods><Quantity>2</Quantity>\
         <Items><Method>GET</Method><Method>HEAD</Method></Items></AllowedMethods>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         <OriginRequestPolicyId>{ALL_VIEWER_AND_CLOUDFRONT}</OriginRequestPolicyId>\
         </CacheBehavior></Items></CacheBehaviors>"
    )
}

/// Hold one subject's published value to the rule: it is an address a client dials AS GIVEN, it
/// is the address this emulator opened that distribution's edge on, and a later read of the
/// distribution reports the same value.
async fn assert_published_address(topology: &Topology, subject: &Subject) {
    let case = subject.case;
    let domain = &subject.edge.domain;
    assert!(
        is_published_edge_address(domain),
        "case {case:?}: expected a published DomainName a client can dial as given \
         (host:port), got {domain:?}"
    );
    let opened = topology.edge_address(&subject.id);
    assert_eq!(
        opened.as_deref(),
        Some(domain.as_str()),
        "case {case:?}: expected the published DomainName to be the address this emulator \
         opened for the distribution, got {opened:?} against {domain:?}"
    );
    let (status, raw) = topology
        .cloudfront(
            case,
            "GET",
            &format!("/{API_VERSION}/distribution/{}", subject.id),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetDistribution, got {status} with {raw}"
    );
    let read = element(&raw, "DomainName");
    assert_eq!(
        read, *domain,
        "case {case:?}: expected the read to report the published address {domain:?}, got \
         {read:?}"
    );
}

/// The whole rule over a table: every enforcement CloudFront makes for itself is made at the
/// address the distribution published, over a Lambda function URL origin, against a viewer that
/// holds nothing but that published value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_edge_enforcement_is_decided_at_the_published_domain_name() {
    let topology = Topology::new();
    let setup = "a Lambda function URL origin behind an enforcing distribution";
    let function = "cf-published-domain-enforcement";
    let origin = topology.public_function_url_host(setup, function).await;

    // The subject the method rows run against, and the baseline the other subjects are
    // configured from: the country and the address the EDGE reports for a viewer of this
    // emulator, read where a client reads them — off the origin's own report of a request that
    // was served at the published address.
    let methods_case = "a behaviour whose AllowedMethods lists GET and HEAD alone";
    let (methods_id, _arn, methods_edge) = topology
        .created_distribution(
            methods_case,
            &config_xml("published-enforcement-methods", &origin, "", ""),
        )
        .await;
    topology.awaited_deployment(methods_case, &methods_id).await;
    let baseline = Answered::of_stating(methods_case, &methods_edge, "GET", OPEN_PATH, &[]).await;
    assert_eq!(
        baseline.status, 200,
        "setup {setup:?}: expected the origin's answer through the published address, got {} \
         with {:?}",
        baseline.status, baseline.raw
    );
    let report = baseline.report(methods_case);
    let country = report["country"].as_str().unwrap_or_default().to_string();
    assert!(
        country.len() == 2 && country.chars().all(|letter| letter.is_ascii_uppercase()),
        "setup {setup:?}: expected the origin to be told a two-letter uppercase country code, \
         got {country:?} in {report}"
    );
    assert!(
        !ELSEWHERE.contains(&country.as_str()),
        "setup {setup:?}: the countries this suite names as elsewhere ({ELSEWHERE:?}) must not \
         be the one the edge reports ({country:?})"
    );
    let observed = viewer_ip(report["address"].as_str().unwrap_or_default());
    assert!(
        observed.parse::<::std::net::IpAddr>().is_ok(),
        "setup {setup:?}: expected the origin to be told the viewer's address, got \
         {observed:?} in {report}"
    );

    // The geographic subjects: one blocklisting the country this viewer is reported to be in,
    // one blocklisting countries it is not.
    let blocked_case = "a distribution whose blocklist names the viewer's country";
    let (blocked_id, _arn, blocked_edge) = topology
        .created_distribution(
            blocked_case,
            &config_xml(
                "published-enforcement-blocked",
                &origin,
                &blocklist(std::slice::from_ref(&country)),
                "",
            ),
        )
        .await;
    let elsewhere_case = "a distribution whose blocklist names no country of this viewer's";
    let elsewhere: Vec<String> = ELSEWHERE.iter().map(|code| (*code).to_string()).collect();
    let (elsewhere_id, _arn, elsewhere_edge) = topology
        .created_distribution(
            elsewhere_case,
            &config_xml(
                "published-enforcement-elsewhere",
                &origin,
                &blocklist(&elsewhere),
                "",
            ),
        )
        .await;

    // The signed subject: an ordered behaviour that trusts a key group holding ONE key.
    let signer = Signer::generate();
    let stranger = Signer::generate();
    let key = upload_public_key(&topology.router, "published-enforcement-key", &signer).await;
    let other =
        upload_public_key(&topology.router, "published-enforcement-stranger", &stranger).await;
    let (group, _etag) =
        create_key_group(&topology.router, "published-enforcement", &[key.as_str()]).await;
    let signed_case = "a behaviour that trusts a key group";
    let (signed_id, _arn, signed_edge) = topology
        .created_distribution(
            signed_case,
            &config_xml(
                "published-enforcement-signed",
                &origin,
                "",
                &trusted_behavior(&group),
            ),
        )
        .await;

    let subjects = [
        Subject {
            case: methods_case,
            id: methods_id,
            edge: methods_edge,
        },
        Subject {
            case: blocked_case,
            id: blocked_id,
            edge: blocked_edge,
        },
        Subject {
            case: elsewhere_case,
            id: elsewhere_id,
            edge: elsewhere_edge,
        },
        Subject {
            case: signed_case,
            id: signed_id,
            edge: signed_edge,
        },
    ];
    for subject in &subjects {
        topology.awaited_deployment(subject.case, &subject.id).await;
        assert_published_address(&topology, subject).await;
    }
    // Each distribution publishes an address of its OWN: a viewer holding one of these values
    // reaches one distribution and no other.
    for (index, subject) in subjects.iter().enumerate() {
        for other in subjects.iter().skip(index + 1) {
            assert_ne!(
                subject.edge.domain, other.edge.domain,
                "case {:?}: expected an address of its own, got the one {:?} published \
                 ({:?})",
                subject.case, other.case, subject.edge.domain
            );
        }
    }

    // The signed URLs, written for the name the signed subject PUBLISHED — the host a viewer
    // dialling it states — and for the scheme that viewer dials, which is what the policy's
    // `Resource` is matched against.
    let signed_domain = subjects[3].edge.domain.clone();
    let resource = format!("http://{signed_domain}{PRIVATE_PATTERN}");
    let pinned = custom_policy(
        &resource,
        far_future(),
        None,
        Some(&format!("{observed}/32")),
    );
    let ranged = custom_policy(&resource, far_future(), None, Some(ANOTHER_RANGE));
    let plain = custom_policy(&resource, far_future(), None, None);

    // (the subject the row addresses, the case, the method, the path and query, the verdict)
    let rows: Vec<(usize, String, &str, String, Expected)> = vec![
        (
            0,
            "a method the matched behaviour lists".to_string(),
            "GET",
            OPEN_PATH.to_string(),
            Expected::Served,
        ),
        (
            0,
            "a POST, which the matched behaviour does not list".to_string(),
            "POST",
            OPEN_PATH.to_string(),
            Expected::Refused(None),
        ),
        (
            0,
            "a DELETE, which it does not list either".to_string(),
            "DELETE",
            OPEN_PATH.to_string(),
            Expected::Refused(None),
        ),
        (
            1,
            "a viewer in a country the blocklist names".to_string(),
            "GET",
            OPEN_PATH.to_string(),
            Expected::Refused(None),
        ),
        (
            2,
            "a viewer in no country the blocklist names".to_string(),
            "GET",
            OPEN_PATH.to_string(),
            Expected::Served,
        ),
        (
            3,
            "a request carrying no signature at all".to_string(),
            "GET",
            PRIVATE_PATH.to_string(),
            Expected::Refused(Some("MissingKey")),
        ),
        (
            3,
            "a signed URL whose policy pins the address the edge observes".to_string(),
            "GET",
            format!(
                "{PRIVATE_PATH}?{}",
                custom_query(&pinned, &signer.sign(&pinned), &key)
            ),
            Expected::Served,
        ),
        (
            3,
            "a signed URL whose policy pins another source range".to_string(),
            "GET",
            format!(
                "{PRIVATE_PATH}?{}",
                custom_query(&ranged, &signer.sign(&ranged), &key)
            ),
            Expected::Refused(Some("AccessDenied")),
        ),
        (
            3,
            "a signed URL signed by a key the group does not carry".to_string(),
            "GET",
            format!(
                "{PRIVATE_PATH}?{}",
                custom_query(&plain, &stranger.sign(&plain), &other)
            ),
            Expected::Refused(Some("InvalidKey")),
        ),
    ];

    for (subject_index, case, method, path_and_query, expected) in rows {
        let subject = &subjects[subject_index];
        let case = format!("{case} at {:?}", subject.edge.domain);
        let before = topology.invocations(&case, function).await;
        let answered =
            Answered::of_stating(&case, &subject.edge, method, &path_and_query, &[]).await;
        let after = topology.invocations(&case, function).await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: expected the origin's answer through the published \
                     address, got {} with {:?}",
                    answered.status, answered.raw
                );
                assert!(
                    answered.carries_the_origins_bytes() && answered.reports_cache(MISS),
                    "case {case:?}: expected the origin's own bytes for this request \
                     ({MISS:?}), got {:?}",
                    answered.raw
                );
                assert!(
                    after > before,
                    "case {case:?}: an admitted request must reach the origin, but the \
                     function's invocations stayed at {before} (now {after})"
                );
            }
            Expected::Refused(code) => {
                assert_eq!(
                    answered.status, 403,
                    "case {case:?}: expected CloudFront's own 403 at the published address, \
                     got {} with {:?}",
                    answered.status, answered.raw
                );
                assert!(
                    answered.reports_cache(ERROR),
                    "case {case:?}: expected the refusal reported as CloudFront's own answer \
                     ({ERROR:?}), got {:?}",
                    answered.raw
                );
                assert!(
                    !answered.carries_the_origins_bytes(),
                    "case {case:?}: a refused viewer must be served none of the origin's \
                     bytes, got {:?}",
                    answered.body
                );
                if let Some(code) = code {
                    assert!(
                        answered.body.contains(&format!("<Code>{code}</Code>")),
                        "case {case:?}: expected the error code {code:?}, got {:?}",
                        answered.body
                    );
                }
                assert_eq!(
                    after, before,
                    "case {case:?}: a request CloudFront refuses must make no origin request, \
                     but the function's invocations went from {before} to {after}"
                );
            }
        }
    }
}
