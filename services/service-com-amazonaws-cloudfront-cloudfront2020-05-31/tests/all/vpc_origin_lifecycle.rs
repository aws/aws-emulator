// SPDX-License-Identifier: CC0-1.0

//! The CloudFront VPC origin control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rules under test:
//!
//! 1. a VPC origin is PERSISTED by the call that creates it, which MINTS its identity (an
//!    id, the ARN built from that id and a version token) and stores the endpoint
//!    configuration whole — so `GetVpcOrigin` answers from that record, member for member,
//!    and the create reports the resource's own `Location` and `ETag`;
//! 2. a VPC origin CloudFront has just created is `Deploying`, and the CLOCK — not a later
//!    call — owes it `Deployed`, without changing anything else about the record;
//! 3. a request no VPC origin can be built from is refused and stores nothing: an
//!    `OriginSslProtocols` whose `Quantity` disagrees with its `Items` is
//!    `InconsistentQuantities`, an endpoint `Arn` that is no ARN at all is `InvalidArgument`
//!    with the one sentence this service states for that form, and a tag set no resource can
//!    carry is refused by the service's one tagging rule;
//! 4. an `Id` no VPC origin answers to is `EntityNotFound`, whatever its form;
//! 5. the tags a create carries are the tags the tagging surface reports for the VPC
//!    origin's ARN;
//! 6. the ARN a create mints is an ARN the resource-policy surface attaches a policy to,
//!    which is what makes a created VPC origin the resource that surface addresses.
//!
//! The table rows differ along everything these rules generalise over: both port numbers
//! (including each published bound and the defaults the model documents), every value
//! `OriginProtocolPolicy` admits, whether the configuration names `OriginSslProtocols` at all
//! and how many protocols it lists, each value `IpAddressType` admits and its absence, the
//! shape of the endpoint name and ARN, and whether the create carries tags.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MALFORMED_ARN_MESSAGE, RESOURCE_POLICY_RESOURCE_TYPE, STATUS_DEPLOYED, VPC_ORIGIN_ID_PREFIX,
    VPC_ORIGIN_NOT_FOUND_MESSAGE, VPC_ORIGIN_STATUS_DEPLOYING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, past_propagation, Answer, Node, ACCOUNT_ID, API_VERSION,
};
use crate::distribution_tags::{
    list_tags_for_resource, reported_tags, tag, tag_sets_no_resource_can_carry, tags_xml,
    ReportedTags, SubmittedTag,
};
use crate::distributions_by_web_acl_id::encode_label;
use crate::key_value_store_lifecycle::{escape, is_version_token, member};
use crate::resource_policy_lifecycle::{
    get_resource_policy, policy, put_resource_policy, reported_document,
};

// ---------------------------------------------------------------------------
// The endpoint configurations under test
// ---------------------------------------------------------------------------

/// One `VpcOriginEndpointConfig` a client submits, and the tags it submits beside it.
#[derive(Debug, Clone)]
struct Endpoint {
    case: &'static str,
    name: String,
    /// "The ARN of the CloudFront VPC origin endpoint configuration" — the resource in the
    /// VPC the origin fronts, so each row names one a caller really would.
    arn: String,
    http_port: i32,
    https_port: i32,
    /// One of `OriginProtocolPolicy`'s values (`http-only`, `match-viewer`, `https-only`,
    /// from `smithy_query(query="shape",shape="OriginProtocolPolicy")`).
    origin_protocol_policy: &'static str,
    /// The `SslProtocol` values the configuration lists, or `None` where it names no
    /// `OriginSslProtocols` wrapper at all (`SSLv3`, `TLSv1`, `TLSv1.1`, `TLSv1.2`, from
    /// `smithy_query(query="shape",shape="SslProtocol")`).
    ssl_protocols: Option<Vec<&'static str>>,
    /// One of `VpcIpAddressType`'s values (`ipv4`, `dualstack`, from
    /// `smithy_query(query="shape",shape="VpcIpAddressType")`), or `None` where the
    /// configuration names none.
    ip_address_type: Option<&'static str>,
    tags: Vec<SubmittedTag>,
}

impl Endpoint {
    /// The `CreateVpcOriginRequest` document naming this configuration.
    fn xml(&self) -> String {
        self.xml_with_quantity(None)
    }

    /// The same document, with `OriginSslProtocols.Quantity` forced to `quantity` — which is
    /// how one row states a `Quantity` that disagrees with the `Items` it carries.
    fn xml_with_quantity(&self, quantity: Option<i32>) -> String {
        let mut out = format!(
            "<CreateVpcOriginRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <VpcOriginEndpointConfig>"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        out.push_str(&format!("<Arn>{}</Arn>", escape(&self.arn)));
        out.push_str(&format!("<HTTPPort>{}</HTTPPort>", self.http_port));
        out.push_str(&format!("<HTTPSPort>{}</HTTPSPort>", self.https_port));
        out.push_str(&format!(
            "<OriginProtocolPolicy>{}</OriginProtocolPolicy>",
            self.origin_protocol_policy
        ));
        if let Some(protocols) = &self.ssl_protocols {
            let stated = quantity.unwrap_or_else(|| i32::try_from(protocols.len()).unwrap_or(0));
            out.push_str(&format!(
                "<OriginSslProtocols><Quantity>{stated}</Quantity><Items>"
            ));
            for protocol in protocols {
                out.push_str(&format!("<SslProtocol>{protocol}</SslProtocol>"));
            }
            out.push_str("</Items></OriginSslProtocols>");
        }
        if let Some(kind) = self.ip_address_type {
            out.push_str(&format!("<IpAddressType>{kind}</IpAddressType>"));
        }
        out.push_str("</VpcOriginEndpointConfig>");
        if !self.tags.is_empty() {
            out.push_str(&tags_xml(Some(&self.tags)));
        }
        out.push_str("</CreateVpcOriginRequest>");
        out
    }

    /// The tag set the tagging surface must report for this VPC origin.
    fn expected_tags(&self) -> ReportedTags {
        self.tags.iter().cloned().collect()
    }
}

/// The ARN of a load balancer in the caller's VPC, as a VPC origin endpoint names one.
///
/// `pub(crate)` so a sibling suite that needs a VPC origin names its endpoint the same way
/// rather than writing a second ARN by hand.
pub(crate) fn load_balancer_arn(name: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:us-east-1:{ACCOUNT_ID}:loadbalancer/app/{name}/\
         0123456789abcdef"
    )
}

/// The creates rules 1, 2, 5 and 6 are asserted over.
///
/// The rows differ along both ports (the defaults the model documents, 80 and 443, and each
/// published bound of a TCP port), every value `OriginProtocolPolicy` admits, whether the
/// configuration lists `OriginSslProtocols` and how many protocols it lists, each value
/// `IpAddressType` admits and its absence, the shape of the endpoint name and ARN, and
/// whether the create carries tags.
fn create_table() -> Vec<Endpoint> {
    vec![
        Endpoint {
            case: "the documented default ports, http-only, no SSL protocols, no tags",
            name: "orders-alb".to_string(),
            arn: load_balancer_arn("orders"),
            http_port: 80,
            https_port: 443,
            origin_protocol_policy: "http-only",
            ssl_protocols: None,
            ip_address_type: None,
            tags: Vec::new(),
        },
        Endpoint {
            case: "custom ports, https-only, one SSL protocol, ipv4, two tags",
            name: "checkout.internal_1".to_string(),
            arn: load_balancer_arn("checkout"),
            http_port: 8080,
            https_port: 8443,
            origin_protocol_policy: "https-only",
            ssl_protocols: Some(vec!["TLSv1.2"]),
            ip_address_type: Some("ipv4"),
            tags: vec![
                tag("stage", Some("prod")),
                tag("owner", Some("edge-team")),
            ],
        },
        Endpoint {
            case: "the lowest port, match-viewer, every SSL protocol, dualstack, one tag",
            name: "a".to_string(),
            arn: format!("arn:aws:ec2:us-east-1:{ACCOUNT_ID}:instance/i-0123456789abcdef0"),
            http_port: 1,
            https_port: 2,
            origin_protocol_policy: "match-viewer",
            // Every value `SslProtocol` admits, so the stored list is not a chosen few.
            ssl_protocols: Some(vec!["SSLv3", "TLSv1", "TLSv1.1", "TLSv1.2"]),
            ip_address_type: Some("dualstack"),
            tags: vec![tag("owner", None)],
        },
        Endpoint {
            case: "the highest port, http-only, an empty SSL protocol list, no tags",
            name: "n".repeat(64),
            arn: format!(
                "arn:aws:elasticloadbalancing:us-east-1:{ACCOUNT_ID}:loadbalancer/net/edge/\
                 fedcba9876543210"
            ),
            http_port: 65_535,
            https_port: 65_534,
            origin_protocol_policy: "http-only",
            ssl_protocols: Some(Vec::new()),
            ip_address_type: Some("ipv4"),
            tags: Vec::new(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Calling the VPC origin surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/vpc-origin` with the request document `body`.
///
/// `pub(crate)` so a sibling suite that needs a VPC origin creates it over the SAME wire
/// path rather than growing a second caller of this route.
pub(crate) async fn create_vpc_origin(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/vpc-origin"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/vpc-origin/{Id}`.
async fn get_vpc_origin(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/vpc-origin/{id}"),
        None,
        None,
    )
    .await
}

/// The `(Id, Arn)` of the VPC origin `answer` reports, after asserting every member of the
/// document is the record `endpoint` asked for, in `status`.
fn assert_record(
    answer: &Answer,
    endpoint: &Endpoint,
    status: &str,
    context: &str,
) -> (String, String) {
    let case = endpoint.case;
    let vpc_origin: &Node = answer.root();
    let raw = answer.raw.as_str();

    let id = member(vpc_origin, "Id", case, raw);
    assert!(
        id.starts_with(VPC_ORIGIN_ID_PREFIX) && id.len() > VPC_ORIGIN_ID_PREFIX.len(),
        "case {case} ({context}): expected a minted VPC origin id, got {id:?}"
    );

    let arn = member(vpc_origin, "Arn", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/{id}"),
        "case {case} ({context}): expected the ARN built from the Id, got {arn:?}"
    );

    let account = member(vpc_origin, "AccountId", case, raw);
    assert_eq!(
        account, ACCOUNT_ID,
        "case {case} ({context}): expected the owning account, got {account:?}"
    );

    let actual_status = member(vpc_origin, "Status", case, raw);
    assert_eq!(
        actual_status, status,
        "case {case} ({context}): expected Status {status}, got {actual_status:?}"
    );

    for instant in ["CreatedTime", "LastModifiedTime"] {
        let reported = member(vpc_origin, instant, case, raw);
        assert!(
            !reported.is_empty(),
            "case {case} ({context}): expected a {instant}, got {reported:?}"
        );
    }

    let config = vpc_origin
        .at("VpcOriginEndpointConfig")
        .unwrap_or_else(|| panic!("case {case} ({context}): no VpcOriginEndpointConfig: {raw}"));
    assert_eq!(
        member(config, "Name", case, raw),
        endpoint.name,
        "case {case} ({context}): expected the submitted Name"
    );
    assert_eq!(
        member(config, "Arn", case, raw),
        endpoint.arn,
        "case {case} ({context}): expected the submitted endpoint Arn"
    );
    assert_eq!(
        member(config, "HTTPPort", case, raw),
        endpoint.http_port.to_string(),
        "case {case} ({context}): expected the submitted HTTPPort"
    );
    assert_eq!(
        member(config, "HTTPSPort", case, raw),
        endpoint.https_port.to_string(),
        "case {case} ({context}): expected the submitted HTTPSPort"
    );
    assert_eq!(
        member(config, "OriginProtocolPolicy", case, raw),
        endpoint.origin_protocol_policy,
        "case {case} ({context}): expected the submitted OriginProtocolPolicy"
    );
    match &endpoint.ssl_protocols {
        Some(protocols) => {
            let wrapper = config.at("OriginSslProtocols").unwrap_or_else(|| {
                panic!("case {case} ({context}): no OriginSslProtocols: {raw}")
            });
            assert_eq!(
                member(wrapper, "Quantity", case, raw),
                protocols.len().to_string(),
                "case {case} ({context}): expected the submitted Quantity"
            );
            let listed: Vec<String> = wrapper
                .at("Items")
                .map(|items| {
                    items
                        .children_named("SslProtocol")
                        .into_iter()
                        .map(|protocol| protocol.text().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                listed, *protocols,
                "case {case} ({context}): expected the submitted SSL protocols, in order"
            );
        }
        // A member the create left out is a member the record carries none of: the service
        // invents no value for it.
        None => assert!(
            config.at("OriginSslProtocols").is_none(),
            "case {case} ({context}): expected no OriginSslProtocols, got {raw}"
        ),
    }
    match endpoint.ip_address_type {
        Some(kind) => assert_eq!(
            member(config, "IpAddressType", case, raw),
            kind,
            "case {case} ({context}): expected the submitted IpAddressType"
        ),
        None => assert!(
            config.at("IpAddressType").is_none(),
            "case {case} ({context}): expected no IpAddressType, got {raw}"
        ),
    }

    (id, arn)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// RULE 1: a create mints the VPC origin's identity, stores the endpoint configuration whole
/// and reports the resource's `Location` and `ETag` — and the get answers from that record.
/// Across the table, no two VPC origins share an id, an ARN or a version token.
#[tokio::test]
async fn create_persists_the_vpc_origin_the_get_then_reports() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for endpoint in create_table() {
        let case = endpoint.case;
        let created = create_vpc_origin(&router, &endpoint.xml()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected HTTP 202, got {} with {}",
            created.status, created.raw
        );
        let (id, arn) = assert_record(
            &created,
            &endpoint,
            VPC_ORIGIN_STATUS_DEPLOYING,
            "the create",
        );

        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        // The ARN's resource type, spelled out once rather than through the constant
        // `assert_record` builds its expectation from: the model's own VPC origin examples
        // carry `arn:aws:cloudfront::<account>:vpcorigin/<Id>`, which is NOT the way the
        // route spells the type (`/2020-05-31/vpc-origin/{Id}`), so a regression in that
        // constant cannot pass silently.
        assert!(
            arn.starts_with(&format!("arn:aws:cloudfront::{ACCOUNT_ID}:vpcorigin/")),
            "case {case}: expected an arn:aws:cloudfront::<account>:vpcorigin/<Id> ARN, got \
             {arn:?}"
        );
        assert_eq!(
            created.header("location"),
            format!("https://cloudfront.amazonaws.com/{API_VERSION}/vpc-origin/{id}"),
            "case {case}: expected the Location of the resource just created, got {:?}",
            created.header("location")
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The read answers from the stored record: every member of the configuration, the
        // minted identity and the version token the create handed back.
        let read = get_vpc_origin(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {}",
            read.status, read.raw
        );
        let reported_status = read
            .root()
            .text_at("Status")
            .unwrap_or_default()
            .to_string();
        assert!(
            reported_status == VPC_ORIGIN_STATUS_DEPLOYING || reported_status == STATUS_DEPLOYED,
            "case {case}: expected {VPC_ORIGIN_STATUS_DEPLOYING} or {STATUS_DEPLOYED} from the \
             get, got {reported_status:?}"
        );
        let (read_id, read_arn) =
            assert_record(&read, &endpoint, &reported_status, "the get by id");
        assert_eq!(
            (read_id, read_arn),
            (id.clone(), arn.clone()),
            "case {case}: the get must report the record the create stored"
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the create's version token, got {:?}",
            read.header("etag")
        );

        // A read is a REPORT: asking twice answers the same record, and asking never
        // consumes it.
        let again = get_vpc_origin(&router, &id).await;
        assert_eq!(
            again.header("etag"),
            etag,
            "case {case}: a second get must answer the same version token, got {:?}",
            again.header("etag")
        );
    }
}

/// RULE 2: a VPC origin CloudFront has just created is `Deploying`, and the CLOCK owes it
/// `Deployed`: no call asks for the transition, and the transition changes nothing else about
/// the record — not its identity, not its configuration and not the version token an
/// `If-Match` is checked against.
#[tokio::test]
async fn the_clock_settles_deploying_to_deployed() {
    let router = emulator();
    let endpoint = Endpoint {
        case: "deploying settles",
        name: "settles-to-deployed".to_string(),
        arn: load_balancer_arn("settling"),
        http_port: 80,
        https_port: 443,
        origin_protocol_policy: "https-only",
        ssl_protocols: Some(vec!["TLSv1.2"]),
        ip_address_type: Some("ipv4"),
        tags: Vec::new(),
    };

    let created = create_vpc_origin(&router, &endpoint.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {}: expected HTTP 202, got {} with {}",
        endpoint.case, created.status, created.raw
    );
    let (id, arn) = assert_record(
        &created,
        &endpoint,
        VPC_ORIGIN_STATUS_DEPLOYING,
        "the create",
    );
    let etag = created.header("etag").to_string();

    // A caller acting straight after the create still observes the transient status.
    let immediate = get_vpc_origin(&router, &id).await;
    assert_record(
        &immediate,
        &endpoint,
        VPC_ORIGIN_STATUS_DEPLOYING,
        "straight after the create",
    );

    // Once the span has passed, the clock has settled the VPC origin — without any call
    // having asked for it.
    tokio::time::sleep(past_propagation()).await;

    let settled = get_vpc_origin(&router, &id).await;
    let (settled_id, settled_arn) =
        assert_record(&settled, &endpoint, STATUS_DEPLOYED, "after the span");
    assert_eq!(
        (settled_id, settled_arn),
        (id, arn),
        "case {}: the deployment must not change the VPC origin's identity",
        endpoint.case
    );
    assert_eq!(
        settled.header("etag"),
        etag,
        "case {}: a deployment is not a mutation, so the version token stands",
        endpoint.case
    );
}

/// RULE 3: a request no VPC origin can be built from is refused, and the refusal mints
/// nothing — the VPC origins created before it are still the only ones the service holds.
#[tokio::test]
async fn a_request_no_vpc_origin_can_be_built_from_is_refused() {
    let router = emulator();

    // A VPC origin created BEFORE the refusals, so each refusal can be shown to have left
    // the store exactly as it was.
    let kept = create_table()
        .into_iter()
        .next()
        .expect("the create table is not empty");
    let created = create_vpc_origin(&router, &kept.xml()).await;
    let (kept_id, _) = assert_record(&created, &kept, VPC_ORIGIN_STATUS_DEPLOYING, "the create");
    let kept_etag = created.header("etag").to_string();

    // A `Quantity` that disagrees with the `Items` beside it, in both directions and over
    // lists of different sizes.
    let quantity_rows: [(&str, Option<Vec<&'static str>>, i32); 3] = [
        ("one protocol stated as two", Some(vec!["TLSv1.2"]), 2),
        (
            "four protocols stated as one",
            Some(vec!["SSLv3", "TLSv1", "TLSv1.1", "TLSv1.2"]),
            1,
        ),
        ("an empty list stated as one", Some(Vec::new()), 1),
    ];
    for (case, protocols, stated) in quantity_rows {
        let endpoint = Endpoint {
            case: "quantity disagreement",
            name: format!("quantity-{stated}"),
            arn: load_balancer_arn("quantity"),
            http_port: 80,
            https_port: 443,
            origin_protocol_policy: "https-only",
            ssl_protocols: protocols,
            ip_address_type: None,
            tags: Vec::new(),
        };
        let refused = create_vpc_origin(&router, &endpoint.xml_with_quantity(Some(stated))).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InconsistentQuantities",
            "case {case}: expected InconsistentQuantities, got {:?} ({})",
            refused.error_code(),
            refused.raw
        );
    }

    // An endpoint `Arn` that cannot be READ as an ARN at all, in each form that failure
    // takes.
    let arn_rows: [(&str, String); 5] = [
        ("the empty string", String::new()),
        ("a bare word", "not-an-arn".to_string()),
        (
            "the resource part alone",
            format!("{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "the prefix in upper case",
            format!("ARN:aws:ec2:us-east-1:{ACCOUNT_ID}:instance/i-1"),
        ),
        (
            "a URL rather than an ARN",
            "https://orders.internal.example.com".to_string(),
        ),
    ];
    for (case, value) in &arn_rows {
        let endpoint = Endpoint {
            case: "an endpoint Arn that is no ARN",
            name: "arn-form".to_string(),
            arn: value.clone(),
            http_port: 80,
            https_port: 443,
            origin_protocol_policy: "http-only",
            ssl_protocols: None,
            ip_address_type: None,
            tags: Vec::new(),
        };
        let refused = create_vpc_origin(&router, &endpoint.xml()).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400 for {value:?}, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {value:?}, got {:?} ({})",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            MALFORMED_ARN_MESSAGE,
            "case {case}: expected message {MALFORMED_ARN_MESSAGE:?}, got {}",
            refused.raw
        );
    }

    // The service's one tagging rule, on this resource kind: the very rows every other
    // tagged kind is refused over.
    for (case, tags, status, code) in tag_sets_no_resource_can_carry() {
        let endpoint = Endpoint {
            case: "a tag set no resource can carry",
            name: format!("tagging-{case}"),
            arn: load_balancer_arn("tagging"),
            http_port: 80,
            https_port: 443,
            origin_protocol_policy: "http-only",
            ssl_protocols: None,
            ip_address_type: None,
            tags,
        };
        let refused = create_vpc_origin(&router, &endpoint.xml()).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected HTTP {status}, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {:?} ({})",
            refused.error_code(),
            refused.raw
        );
    }

    // Every refusal left the store as it was: the VPC origin created before them still reads
    // back, carrying the version token its own create handed out.
    let read = get_vpc_origin(&router, &kept_id).await;
    assert_eq!(
        read.status, 200,
        "case after the refusals: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        kept_etag,
        "case after the refusals: expected the create's version token, got {:?}",
        read.header("etag")
    );
}

/// RULE 4: an `Id` no VPC origin answers to is `EntityNotFound` with the one sentence this
/// service states for that condition — whatever the id's form, and even while the service
/// holds other VPC origins.
#[tokio::test]
async fn an_id_no_vpc_origin_answers_to_is_entity_not_found() {
    let router = emulator();
    let endpoint = create_table()
        .into_iter()
        .next()
        .expect("the create table is not empty");
    let created = create_vpc_origin(&router, &endpoint.xml()).await;
    let (id, arn) = assert_record(&created, &endpoint, VPC_ORIGIN_STATUS_DEPLOYING, "the create");

    // Rows differing along the FORM of the identifier: a minted id's namespace with another
    // body, a bare word, an upper-case id, a very long id, and the VPC origin's own ARN —
    // which is not the identifier this route addresses one by.
    let rows: [(&str, String); 5] = [
        (
            "a minted namespace with a body no VPC origin holds",
            format!("{VPC_ORIGIN_ID_PREFIX}ABSENT0000000"),
        ),
        ("a bare word", "absent".to_string()),
        ("an upper-case id", id.to_uppercase()),
        (
            "an id of two hundred characters",
            format!("{VPC_ORIGIN_ID_PREFIX}{}", "0".repeat(200)),
        ),
        ("the VPC origin's own ARN", encode_label(&arn)),
    ];
    for (case, identifier) in &rows {
        let answer = get_vpc_origin(&router, identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {identifier:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for {identifier:?}, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            VPC_ORIGIN_NOT_FOUND_MESSAGE,
            "case {case}: expected message {VPC_ORIGIN_NOT_FOUND_MESSAGE:?}, got {}",
            answer.raw
        );
    }

    // The VPC origin the service does hold is untouched by every absent read.
    let read = get_vpc_origin(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case after the absent reads: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
}

/// RULE 5: the tags a create carries are the tags the tagging surface reports for the VPC
/// origin's ARN — one tag set, not a second copy of the submitted list.
#[tokio::test]
async fn the_tags_a_create_carries_are_the_tags_the_tagging_surface_reports() {
    let router = emulator();

    for endpoint in create_table() {
        let case = endpoint.case;
        let created = create_vpc_origin(&router, &endpoint.xml()).await;
        let (_, arn) = assert_record(&created, &endpoint, VPC_ORIGIN_STATUS_DEPLOYING, "the create");

        let read = list_tags_for_resource(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the tag read, got {} with {}",
            read.status, read.raw
        );
        let expected = endpoint.expected_tags();
        assert_eq!(
            reported_tags(read.root()),
            expected,
            "case {case}: expected the tags the create submitted {expected:?}, got {}",
            read.raw
        );
    }
}

/// RULE 6: the ARN a create mints is an ARN the resource-policy surface attaches a policy
/// to — the VPC origin is the resource that surface addresses, so a policy written under the
/// minted ARN reads back under it.
#[tokio::test]
async fn the_minted_arn_is_one_the_resource_policy_surface_accepts() {
    let router = emulator();
    let endpoint = create_table()
        .into_iter()
        .nth(1)
        .expect("the create table has a second row");
    let created = create_vpc_origin(&router, &endpoint.xml()).await;
    let (_, arn) = assert_record(&created, &endpoint, VPC_ORIGIN_STATUS_DEPLOYING, "the create");

    let document = policy("share-the-vpc-origin");
    let written = put_resource_policy(&router, &arn, &document).await;
    assert_eq!(
        written.status, 200,
        "case the minted ARN: expected HTTP 200 from the write, got {} with {}",
        written.status, written.raw
    );
    let read = get_resource_policy(&router, &arn).await;
    assert_eq!(
        read.status, 200,
        "case the minted ARN: expected HTTP 200 from the read, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        reported_document(&read),
        document.as_str(),
        "case the minted ARN: expected the document the write persisted, got {:?}",
        reported_document(&read)
    );
}
