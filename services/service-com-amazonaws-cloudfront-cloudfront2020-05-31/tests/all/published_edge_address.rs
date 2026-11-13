// SPDX-License-Identifier: CC0-1.0

//! What a distribution PUBLISHES as its domain name, and the configurations CloudFront
//! refuses — end to end over HTTP, through the same restXml path a client takes.
//!
//! Three rules are under test here:
//!
//! * a distribution reports, as its `DomainName`, an address a client on this machine can
//!   DIAL: the `host:port` of the listener its edge is served on. The name is not a
//!   decoration — the suite opens a socket to it and fetches an object through it, which is
//!   what a viewer handed that name does — and both schemes a viewer may dial are served at
//!   that one address;
//! * a MULTI-TENANT distribution's configuration may state none of the members the service
//!   documents as supported only on a standard distribution (or as a connection group's to
//!   carry): each is refused `InvalidArgument` (HTTP 400) with the sentence that names it,
//!   on the create and on the update alike, while the same configuration without it is
//!   accepted and reads back carrying none of them — and the same member on a STANDARD
//!   distribution is accepted, which is what makes the connection mode the thing deciding;
//! * a cache behaviour's `FunctionAssociation` names a CloudFront function published to the
//!   `LIVE` stage — "your functions must be published to the `LIVE` stage to associate them
//!   with a cache behavior" — so an association naming an unpublished function, or one this
//!   account does not hold at all, is refused `InvalidArgument` and creates nothing, while
//!   the SAME configuration lands once the function has been published.
//!
//! The rows span what each rule generalises over: every member the model carries the
//! connection-mode note on, at the configuration level and inside both shapes a cache
//! behaviour takes (the default one and an ordered one); the three states a function
//! association's ARN can be in (no such function, a function with no published stage, a
//! published one); and, for the published address, a `GET` of an object and a method the
//! behaviour does not allow, dialled in the clear.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s and the function helpers
//! [`crate::function_publish`]'s, reused rather than copied.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    multi_tenant_direct_only_message, MULTI_TENANT_DOMAIN_NAME,
};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_reachable_authority,
    past_propagation, update_distribution, Answer, API_VERSION, CUSTOM_VIEWER_CERTIFICATE, HOST,
};
use crate::function_lifecycle::{create_table, Create};
use crate::function_publish::{create_one, publish_function};

// ---------------------------------------------------------------------------
// The configurations under test
// ---------------------------------------------------------------------------

/// The `TenantConfig` a multi-tenant row carries, so the configuration it submits is a
/// multi-tenant one by the member that "only supports multi-tenant distributions" as well
/// as by its connection mode.
const TENANT_CONFIG: &str = "<TenantConfig><ParameterDefinitions>\
     <member><Name>tenantName</Name><Definition><StringSchema>\
     <Required>false</Required></StringSchema></Definition></member>\
     </ParameterDefinitions></TenantConfig>";

/// One `DistributionConfig` document: one origin, one default cache behaviour, and whatever
/// the row adds inside that behaviour, inside an ordered behaviour and at the configuration
/// level.
fn config_xml(
    reference: &str,
    behavior_extra: &str,
    ordered_extra: Option<&str>,
    config_extra: &str,
) -> String {
    let ordered = ordered_extra
        .map(|extra| {
            format!(
                "<CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
                 <PathPattern>/assets/*</PathPattern><TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{extra}\
                 </CacheBehavior></Items></CacheBehaviors>"
            )
        })
        .unwrap_or_default();
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>{HOST}</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{behavior_extra}\
         </DefaultCacheBehavior>{ordered}\
         <Comment>{reference}</Comment>\
         <Enabled>true</Enabled>{config_extra}\
         </DistributionConfig>"
    )
}

/// Where a row states the member under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    /// At the configuration level.
    Config,
    /// Inside the default cache behaviour.
    DefaultBehavior,
    /// Inside an ordered cache behaviour.
    OrderedBehavior,
}

/// One member a multi-tenant distribution's configuration may not state: the model's name
/// for it, the XML a row states it in, and where it sits.
#[derive(Debug, Clone, Copy)]
struct DirectOnly {
    /// The MODEL's name for the member, which is what the refusal names.
    member: &'static str,
    /// The fragment a row adds to state it.
    xml: &'static str,
    at: Where,
}

/// Every member the model documents as supported only on a standard distribution — "this
/// field only supports standard distributions. You can't specify this field for multi-tenant
/// distributions" — or as a connection group's to carry, read off `DistributionConfig`,
/// `DefaultCacheBehavior` and `CacheBehavior`
/// (`smithy_query(query="shape",shape="DistributionConfig,DefaultCacheBehavior,CacheBehavior")`),
/// plus the legacy `ForwardedValues` cache key the three deprecated TTLs travel with.
///
/// `AnycastIpListId` is not among the rows: it names an Anycast static IP list this account
/// would have to hold, so a row stating one would be refused for the list rather than for
/// the connection mode.
const DIRECT_ONLY: &[DirectOnly] = &[
    DirectOnly {
        member: "Aliases",
        xml: "<Aliases><Quantity>1</Quantity><Items><CNAME>tenant.example.com</CNAME>\
              </Items></Aliases>",
        at: Where::Config,
    },
    DirectOnly {
        member: "PriceClass",
        xml: "<PriceClass>PriceClass_100</PriceClass>",
        at: Where::Config,
    },
    DirectOnly {
        member: "IsIPV6Enabled",
        xml: "<IsIPV6Enabled>true</IsIPV6Enabled>",
        at: Where::Config,
    },
    DirectOnly {
        member: "ContinuousDeploymentPolicyId",
        xml: "<ContinuousDeploymentPolicyId>cdp-0001</ContinuousDeploymentPolicyId>",
        at: Where::Config,
    },
    DirectOnly {
        member: "Staging",
        xml: "<Staging>false</Staging>",
        at: Where::Config,
    },
    DirectOnly {
        member: "ForwardedValues",
        xml: "<ForwardedValues><QueryString>false</QueryString>\
              <Cookies><Forward>none</Forward></Cookies></ForwardedValues>",
        at: Where::DefaultBehavior,
    },
    DirectOnly {
        member: "MinTTL",
        xml: "<MinTTL>0</MinTTL>",
        at: Where::DefaultBehavior,
    },
    DirectOnly {
        member: "DefaultTTL",
        xml: "<DefaultTTL>86400</DefaultTTL>",
        at: Where::DefaultBehavior,
    },
    DirectOnly {
        member: "MaxTTL",
        xml: "<MaxTTL>31536000</MaxTTL>",
        at: Where::DefaultBehavior,
    },
    DirectOnly {
        member: "SmoothStreaming",
        xml: "<SmoothStreaming>false</SmoothStreaming>",
        at: Where::DefaultBehavior,
    },
    DirectOnly {
        member: "TrustedSigners",
        xml: "<TrustedSigners><Enabled>true</Enabled><Quantity>1</Quantity>\
              <Items><AwsAccountNumber>000000000000</AwsAccountNumber></Items></TrustedSigners>",
        at: Where::OrderedBehavior,
    },
    DirectOnly {
        member: "SmoothStreaming",
        xml: "<SmoothStreaming>true</SmoothStreaming>",
        at: Where::OrderedBehavior,
    },
];

impl DirectOnly {
    /// This row's document under `reference`, for a configuration of the connection mode
    /// `tenancy` states — a multi-tenant one when it carries the tenant members.
    ///
    /// An `Aliases` row carries a custom viewer certificate beside the alias, because a
    /// configuration that declares an alternate domain name has to name one whichever
    /// connection mode it states: the member under test is then the only thing the two
    /// sides of the row differ on.
    fn document(self, reference: &str, tenancy: &str) -> String {
        let certificate = if self.member == "Aliases" {
            CUSTOM_VIEWER_CERTIFICATE
        } else {
            ""
        };
        let (behavior, ordered, config) = match self.at {
            Where::Config => (
                String::new(),
                None,
                format!("{}{certificate}{tenancy}", self.xml),
            ),
            Where::DefaultBehavior => (self.xml.to_string(), None, tenancy.to_string()),
            Where::OrderedBehavior => (String::new(), Some(self.xml), tenancy.to_string()),
        };
        config_xml(reference, &behavior, ordered, &config)
    }

    /// A short label for this row, so every case's caller reference is its own.
    fn slug(self) -> String {
        format!("{}-{:?}", self.member, self.at).to_lowercase()
    }
}

/// The configuration-level members that make a configuration MULTI-TENANT.
const TENANT_MEMBERS: &str = "<ConnectionMode>tenant-only</ConnectionMode>";

/// The one that makes it STANDARD, stated so the rows cover both sides of the rule.
const DIRECT_MEMBERS: &str = "<ConnectionMode>direct</ConnectionMode>";

/// The `Id` of a distribution a create answered.
fn created_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 201,
        "case {case}: expected HTTP 201, got {} with {}",
        answer.status, answer.raw
    );
    answer
        .root()
        .find_text("Id")
        .unwrap_or_else(|| panic!("case {case}: the create carries no Id: {}", answer.raw))
        .to_string()
}

// ---------------------------------------------------------------------------
// The connection-mode rule
// ---------------------------------------------------------------------------

/// Every member only a standard distribution has is refused on a multi-tenant
/// configuration, with the sentence that names it, and leaves nothing behind — while the
/// same member on a standard distribution is accepted.
#[tokio::test]
async fn a_multi_tenant_configuration_may_state_no_direct_only_member() {
    let router = emulator();

    for (index, row) in DIRECT_ONLY.iter().enumerate() {
        let case = format!("{}/{:?}", row.member, row.at);
        let slug = row.slug();

        // The multi-tenant side: refused, naming the member.
        let refused = create_distribution(
            &router,
            &row.document(&format!("ref-tenant-{index}-{slug}"), TENANT_MEMBERS),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400 on a multi-tenant configuration, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        let expected = multi_tenant_direct_only_message(row.member);
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );

        // The standard side: the same member is a member of this configuration, so it
        // lands. `TrustedSigners` names an account rather than a key group, which the
        // standard create admits as the configuration's own.
        let accepted = create_distribution(
            &router,
            &row.document(&format!("ref-direct-{index}-{slug}"), DIRECT_MEMBERS),
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected HTTP 201 on a standard configuration, got {} with {}",
            accepted.status, accepted.raw
        );
    }
}

/// A multi-tenant configuration that states none of them is accepted, reads back carrying
/// none of them — the service supplies no default for a member that is no member of this
/// distribution — and keeps the `MinimumProtocolVersion` it declared, there being no
/// CloudFront domain name whose fixed security policy would override it.
#[tokio::test]
async fn a_multi_tenant_distribution_reads_back_without_the_direct_only_members() {
    let router = emulator();
    let certificate = "<ViewerCertificate><CloudFrontDefaultCertificate>true\
         </CloudFrontDefaultCertificate><SSLSupportMethod>sni-only</SSLSupportMethod>\
         <MinimumProtocolVersion>TLSv1.2_2021</MinimumProtocolVersion></ViewerCertificate>";
    let created = create_distribution(
        &router,
        &config_xml(
            "ref-tenant-clean",
            "",
            None,
            &format!("{certificate}{TENANT_MEMBERS}{TENANT_CONFIG}"),
        ),
    )
    .await;
    let id = created_id(&created, "a clean multi-tenant configuration");

    for (context, answer) in [
        ("the create", created),
        ("the read", get_distribution(&router, &id).await),
    ] {
        let config = answer
            .root()
            .descendant("DistributionConfig")
            .unwrap_or_else(|| {
                panic!("case {context}: the answer carries no DistributionConfig: {}", answer.raw)
            });
        for member in [
            "Aliases",
            "PriceClass",
            "IsIPV6Enabled",
            "ContinuousDeploymentPolicyId",
            "Staging",
        ] {
            assert!(
                config.child(member).is_none(),
                "case {context}: a multi-tenant distribution must report no {member}, got {}",
                answer.raw
            );
        }
        let tls = config
            .text_at("ViewerCertificate/MinimumProtocolVersion")
            .unwrap_or_default();
        assert_eq!(
            tls, "TLSv1.2_2021",
            "case {context}: expected the declared MinimumProtocolVersion, got {tls:?}"
        );
        let domain = answer.root().find_text("DomainName").unwrap_or_default();
        assert_eq!(
            domain, MULTI_TENANT_DOMAIN_NAME,
            "case {context}: a multi-tenant distribution has no domain of its own, got \
             {domain:?}"
        );
    }
}

/// An UPDATE is held to the same rule: a multi-tenant distribution cannot be moved onto a
/// configuration stating a member only a standard distribution has.
#[tokio::test]
async fn an_update_may_not_add_a_direct_only_member_to_a_multi_tenant_distribution() {
    let router = emulator();
    let created = create_distribution(
        &router,
        &config_xml(
            "ref-tenant-update",
            "",
            None,
            &format!("{TENANT_MEMBERS}{TENANT_CONFIG}"),
        ),
    )
    .await;
    let id = created_id(&created, "the distribution every update row writes to");
    let etag = created.header("etag").to_string();

    for row in DIRECT_ONLY {
        let case = format!("{}/{:?}", row.member, row.at);
        let refused = update_distribution(
            &router,
            &id,
            &row.document("ref-tenant-update", TENANT_MEMBERS),
            Some(&etag),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400 from the update, got {} with {}",
            refused.status, refused.raw
        );
        let expected = multi_tenant_direct_only_message(row.member);
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }

    // The refusals changed nothing: the distribution is still at the version its create
    // minted.
    let read = get_distribution(&router, &id).await;
    assert_eq!(
        read.header("etag"),
        etag,
        "the refused updates must leave the distribution at its version, got {:?}",
        read.header("etag")
    );
}

// ---------------------------------------------------------------------------
// The function-association rule
// ---------------------------------------------------------------------------

/// A `FunctionAssociations` element associating `arn` on a viewer-request event.
fn association_xml(arn: &str) -> String {
    format!(
        "<FunctionAssociations><Quantity>1</Quantity><Items><FunctionAssociation>\
         <FunctionARN>{arn}</FunctionARN><EventType>viewer-request</EventType>\
         </FunctionAssociation></Items></FunctionAssociations>"
    )
}

/// A cache behaviour may associate only a function published to the `LIVE` stage: the same
/// configuration is refused while the function has no published stage and lands once it
/// has, in the default cache behaviour and in an ordered one alike.
#[tokio::test]
async fn a_function_association_names_a_published_stage() {
    let router = emulator();
    let create = renamed(
        &create_table("arn:aws:cloudfront::000000000000:key-value-store/unused")
            .first()
            .cloned()
            .expect("the function create table carries at least one row"),
        "published_stage_association",
    );
    let function_etag = create_one(&router, &create).await;
    let arn = format!("arn:aws:cloudfront::000000000000:function/{}", create.name);
    // A function this account does not hold at all, in the same shape `FunctionARN`'s
    // pattern admits.
    let unknown = "arn:aws:cloudfront::000000000000:function/no-such-function";

    for (index, (case, behavior, ordered)) in [
        ("the default cache behaviour", association_xml(&arn), None),
        (
            "an ordered cache behaviour",
            String::new(),
            Some(association_xml(&arn)),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        // Before the publish: refused, and nothing is created.
        let refused = create_distribution(
            &router,
            &config_xml(
                &format!("ref-unpublished-{index}"),
                &behavior,
                ordered.as_deref(),
                DIRECT_MEMBERS,
            ),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400 before the publish, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert!(
            message.contains(&create.name),
            "case {case}: expected the refusal to name the function {:?}, got {message:?}",
            create.name
        );
    }

    // An ARN naming no function of this account is refused the same way, naming it.
    let unheld = create_distribution(
        &router,
        &config_xml(
            "ref-unheld",
            &association_xml(unknown),
            None,
            DIRECT_MEMBERS,
        ),
    )
    .await;
    assert_eq!(
        unheld.status, 400,
        "case a function this account does not hold: expected HTTP 400, got {} with {}",
        unheld.status, unheld.raw
    );

    // Published, the SAME configuration lands and reads back carrying the association.
    let published = publish_function(&router, &create.name, &function_etag).await;
    assert_eq!(
        published.status, 200,
        "the publish must answer HTTP 200, got {} with {}",
        published.status, published.raw
    );
    let created = create_distribution(
        &router,
        &config_xml(
            "ref-published",
            &association_xml(&arn),
            Some(&association_xml(&arn)),
            DIRECT_MEMBERS,
        ),
    )
    .await;
    let id = created_id(&created, "a published function");
    let read = get_distribution(&router, &id).await;
    let associated = read
        .root()
        .descendant("FunctionAssociation")
        .and_then(|association| association.text_at("FunctionARN").map(str::to_string))
        .unwrap_or_default();
    assert_eq!(
        associated, arn,
        "expected the stored association to name {arn:?}, got {associated:?}"
    );
}

// ---------------------------------------------------------------------------
// The address a distribution publishes
// ---------------------------------------------------------------------------

/// The `DomainName` a create reported, asserted to be an address a client can dial.
async fn published_domain(router: &MultiServiceRouter, reference: &str) -> String {
    let created = create_distribution(
        &router.clone(),
        &config_xml(reference, "", None, DIRECT_MEMBERS),
    )
    .await;
    let case = "the published domain name";
    let _ = created_id(&created, case);
    let domain = created
        .root()
        .find_text("DomainName")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_reachable_authority(&domain),
        "case {case}: expected an address a client can dial, got {domain:?}"
    );
    domain
}

/// Dial `authority` in the clear and send one HTTP/1.1 request, answering the status line
/// and the headers and body that follow it.
///
/// A viewer dials the name the distribution published, with no AWS credential: this is that
/// client, speaking the protocol the address answers on rather than going through the
/// router, so what it proves is that the PUBLISHED address serves.
///
/// The dial names the LOOPBACK address explicitly and takes only the PORT from the
/// published authority: the address under test is one of this emulator's own listeners
/// ([`is_reachable_authority`] has already held it to that), and nothing here opens a
/// connection to anywhere else.
///
/// `pub(crate)` so every suite that reads a published domain name dials it the same way
/// rather than carrying its own copy of this client.
pub(crate) async fn dial(authority: &str, method: &str, path: &str) -> String {
    dial_as(authority, method, path, authority).await
}

/// [`dial`] with the HOST the viewer addressed stated separately from the authority it
/// DIALLED.
///
/// The two are the same for a distribution's own edge domain, which a viewer reaches by
/// dialling the name it published. They differ wherever the published address serves
/// traffic for a name of its own: a distribution tenant is reached through its connection
/// group's routing endpoint under the TENANT's domain, which is the name its DNS record
/// points at that endpoint.
///
/// `pub(crate)` so every suite that dials a published address uses this one client.
pub(crate) async fn dial_as(authority: &str, method: &str, path: &str, host: &str) -> String {
    let port = authority
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|error| panic!("the published address {authority} is no address: {error}"))
        .port();
    let mut stream = ::tokio::time::timeout(
        Duration::from_secs(5),
        ::tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .unwrap_or_else(|_| panic!("the published address {authority} did not accept a connection"))
    .unwrap_or_else(|error| panic!("the published address {authority} refused a connection: {error}"));
    let request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("the request to {authority} was not sent: {error}"));
    let mut answer = Vec::new();
    let read = ::tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer))
        .await
        .unwrap_or_else(|_| panic!("the published address {authority} did not answer in time"));
    read.unwrap_or_else(|error| panic!("the answer from {authority} was not read: {error}"));
    String::from_utf8_lossy(&answer).to_string()
}

/// The address a distribution publishes ANSWERS: a viewer that dials it in the clear is
/// served the object from the origin the behaviour targets, reported as a cache miss, and a
/// method the behaviour does not allow is refused by CloudFront itself at that same address.
#[tokio::test]
async fn the_published_domain_name_answers_a_viewer() {
    let router = emulator();
    let domain = published_domain(&router, "ref-published-address").await;
    // The distribution has to have reached the edge locations before a viewer is served.
    ::tokio::time::sleep(past_propagation()).await;

    // The origin is this service's own control-plane endpoint, whose
    // `GET /2020-05-31/distribution` answers a `DistributionList` — an origin this emulator
    // really serves, so what comes back is the origin's own answer.
    let served = dial(&domain, "GET", &format!("/{API_VERSION}/distribution")).await;
    assert!(
        served.starts_with("HTTP/1.1 200"),
        "expected the published address to serve the origin's answer, got {served:?}"
    );
    assert!(
        served.contains("DistributionList"),
        "expected the origin's own document, got {served:?}"
    );
    assert!(
        served.to_ascii_lowercase().contains("x-cache: miss from cloudfront"),
        "expected the edge to report a cache miss, got {served:?}"
    );

    // The same address, a method the default cache behaviour does not allow: CloudFront
    // refuses it itself, so no origin answer is served at all.
    let refused = dial(&domain, "DELETE", &format!("/{API_VERSION}/distribution")).await;
    assert!(
        refused.starts_with("HTTP/1.1 403"),
        "expected CloudFront's own refusal of a method the behaviour disallows, got {refused:?}"
    );
    assert!(
        !refused.contains("DistributionList"),
        "expected no origin bytes in a refusal CloudFront produced, got {refused:?}"
    );
}

/// Each distribution publishes an address of its OWN, and each of those addresses serves
/// the distribution it was minted for: the published name is what resolves the
/// distribution, so two distributions are two addresses and a viewer that dials one is
/// served by that one.
#[tokio::test]
async fn each_distribution_publishes_an_address_of_its_own() {
    let router = emulator();
    let first = published_domain(&router, "ref-published-first").await;
    let second = published_domain(&router, "ref-published-second").await;
    assert_ne!(
        first, second,
        "expected each distribution to publish an address of its own, got {first:?} for both"
    );
    ::tokio::time::sleep(past_propagation()).await;

    for (case, domain) in [("the first", &first), ("the second", &second)] {
        let served = dial(domain, "GET", &format!("/{API_VERSION}/distribution")).await;
        assert!(
            served.starts_with("HTTP/1.1 200"),
            "case {case}: expected {domain} to serve the origin's answer, got {served:?}"
        );
        assert!(
            served
                .to_ascii_lowercase()
                .contains("x-cache: miss from cloudfront"),
            "case {case}: expected the edge to report a cache miss, got {served:?}"
        );
    }
}

/// The same function create under another name, so this suite runs against a function of
/// its own.
fn renamed(create: &Create, name: &str) -> Create {
    let mut renamed = create.clone();
    renamed.name = name.to_string();
    renamed
}

/// A viewer request to an address no distribution publishes is served by nothing: the
/// control plane answers its own routes, so the edge claims only the names it minted.
#[tokio::test]
async fn an_address_no_distribution_published_serves_no_edge_traffic() {
    let router = emulator();
    let _ = published_domain(&router, "ref-published-other").await;
    let answer = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution"),
        None,
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected the control plane's own listing on its own endpoint, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.header("x-cache"),
        "",
        "expected no edge reporting on a control-plane answer, got {:?}",
        answer.header("x-cache")
    );
}
